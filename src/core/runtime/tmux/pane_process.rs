//! tmux pane 前台进程订阅的本地增强。
//!
//! tmux 在 Linux 上把 `pane_current_command` 缩成 argv0。订阅同时携带
//! `pane_pid` 后，本地 Runtime 可以从 `/proc` 读取前台进程组完整 argv；
//! SSH Runtime 必须保留远端 tmux 原值，不能拿远端 PID 误查本机 `/proc`。

use crate::protocol::terminal::foreground_process_command;

/// node wrapper 需要完整 argv；tmux 的 `#()` 在 server 侧异步执行，因此 SSH
/// 也会读取远端进程。条件分支保证普通 shell/htop/cargo 不启动额外 job。
pub(crate) const PANE_PROCESS_FORMAT: &str = concat!(
    "#{pane_pid}|#{pane_current_command}|",
    "#{?#{||:#{==:#{pane_current_command},node},#{==:#{pane_current_command},nodejs},#{==:#{pane_current_command},npx},#{==:#{pane_current_command},bun}},",
    "#(ps -ww -o args= -p $(ps -o tpgid= -p #{pane_pid} 2>/dev/null) 2>/dev/null),}"
);

pub(crate) fn resolve_subscription_value(value: &str, local: bool) -> String {
    resolve_subscription_value_with(value, local, foreground_process_command)
}

fn resolve_subscription_value_with<F>(value: &str, local: bool, resolve: F) -> String
where
    F: FnOnce(u32) -> Option<String>,
{
    let mut fields = value.splitn(3, '|');
    let Some(pid) = fields.next() else {
        return value.trim().to_string();
    };
    let Some(reported) = fields.next() else {
        return value.trim().to_string();
    };
    let reported = reported.trim();
    // #() 返回的是异步缓存；进程已回到 shell 或换成具名命令时，旧 argv
    // 不得复活上一个 agent，也不应为每个 shell 同步启动两次本机 ps。
    if !matches!(reported, "node" | "nodejs" | "npx" | "bun") {
        return reported.to_string();
    }
    let server_argv = fields
        .next()
        .map(str::trim)
        .filter(|argv| !argv.is_empty() && !argv.starts_with("<'"));
    if let Some(server_argv) = server_argv.filter(|argv| is_wrapper_argv(argv)) {
        return server_argv.to_string();
    }
    if server_argv.is_some() {
        tracing::debug!(target: "muxterm::tmux", reported, local,
            "ignored conflicting cached foreground argv");
    }
    if !local {
        return reported.to_string();
    }
    let Ok(pid) = pid.trim().parse::<u32>() else {
        return reported.to_string();
    };
    resolve(pid)
        .filter(|command| is_wrapper_argv(command))
        .unwrap_or_else(|| reported.to_string())
}

/// #() 的缓存和当前 pane_current_command 不是原子采样。既然当前命令仍是
/// wrapper，旧 shell/ps argv 不能作为 agent 退出的证据；本地 ps 补查也会竞态。
/// 完整 node server.js 仍保留，用来区分同一 wrapper 下真正更换的程序。
fn is_wrapper_argv(command: &str) -> bool {
    let executable = command
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_matches(['\'', '"'])
        .rsplit('/')
        .next()
        .unwrap_or("");
    matches!(executable, "node" | "nodejs" | "npx" | "bun")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::attention::engine::known_agent_process_name;

    #[test]
    fn wrapped_agent_identity_survives_cached_shell_and_animated_redraws() {
        use crate::activity::attention::{
            clock::FakeClock, engine::AttentionEngine, screen::ScreenSnapshot,
        };
        use crate::config::AttentionConfig;
        let mut engine = AttentionEngine::new(
            AttentionConfig::default(),
            FakeClock::new(std::time::Instant::now()),
        );
        // 1651 日志关联到本地 pane 130；以下是边界竞态的合成输入，不冒充日志原文。
        engine.set_process_name("muxterm", 130, Some("node /opt/bin/codex --yolo".into()));
        for (seq, raw) in [
            "42|node|-zsh",
            "42|node|",
            "42|node|node /opt/bin/codex --yolo",
        ]
        .iter()
        .cycle()
        .take(60)
        .enumerate()
        {
            let observed = resolve_subscription_value_with(raw, false, |_| None);
            engine.set_process_name("muxterm", 130, Some(observed));
            let star = if seq % 2 == 0 { "✦" } else { "✧" };
            let screen = ScreenSnapshot::from_text(&format!("{star} input\nesc to interrupt"));
            engine.apply_with_screen("muxterm", 130, &[], "", seq as u64, Some(&screen));
            engine.on_became_visible("muxterm", 130);
            let pane = &engine.snapshot()[0].panes[0];
            assert!(pane.process_is_agent, "agent disappeared at step {seq}");
            assert_eq!(pane.agent_name.as_deref(), Some("codex"));
        }
        // 真实 shell 事实仍应立刻移除 agent，不能靠永久粘住身份掩盖问题。
        let observed =
            resolve_subscription_value_with("42|zsh|node /opt/bin/codex", false, |_| None);
        engine.set_process_name("muxterm", 130, Some(observed));
        assert!(!engine.snapshot()[0].panes[0].process_is_agent);
    }

    #[test]
    fn stale_shell_argv_cannot_replace_a_live_wrapper() {
        for stale in ["-zsh", "/bin/zsh -l", "ps -ww -o args= -p 42", "-"] {
            let value = format!("42|node|{stale}");
            assert_eq!(
                resolve_subscription_value_with(&value, false, |_| {
                    panic!("remote PID must stay remote")
                }),
                "node"
            );
            assert_eq!(
                resolve_subscription_value_with(&value, true, |_| {
                    Some("node /opt/bin/codex --yolo".into())
                }),
                "node /opt/bin/codex --yolo"
            );
        }
    }

    #[test]
    fn raced_local_foreground_lookup_keeps_reported_wrapper() {
        assert_eq!(
            resolve_subscription_value_with("42|node|", true, |_| { Some("/bin/zsh -l".into()) }),
            "node"
        );
        // 完整的其他 node 脚本仍是退出 agent 的有效证据。
        assert_eq!(
            resolve_subscription_value_with("42|node|node server.js", false, |_| None),
            "node server.js"
        );
    }

    #[test]
    fn captured_wrapped_codex_uses_full_foreground_argv() {
        let fixture =
            include_str!("../../../../tests/samples/tmux-agent-process-observation-2026-0901.txt");
        let leader = fixture
            .lines()
            .find(|line| line.contains("|node|node /usr/bin/codex "))
            .expect("captured Codex process-group leader");
        let fields = leader.split('|').collect::<Vec<_>>();
        let pane_pid = fields[2].parse::<u32>().expect("pane pid");
        let reported = fields[4];
        let argv = fields[10].to_string();

        let resolved =
            resolve_subscription_value_with(&format!("{pane_pid}|{reported}"), true, |_| {
                Some(argv)
            });
        assert_eq!(known_agent_process_name(&resolved), Some("codex"));
    }

    #[test]
    fn ssh_subscription_never_reads_a_remote_pid_from_local_proc() {
        let value = resolve_subscription_value_with("279627|node|", false, |_| {
            panic!("remote pane PID must not be inspected on the local host")
        });
        assert_eq!(value, "node");
    }

    #[test]
    fn ssh_subscription_prefers_server_side_foreground_argv() {
        let value = resolve_subscription_value_with(
            "279627|node|node /usr/bin/codex -m glm-5.3-flash",
            false,
            |_| panic!("server argv must avoid local proc inspection"),
        );
        assert_eq!(value, "node /usr/bin/codex -m glm-5.3-flash");
    }

    #[test]
    fn old_or_malformed_subscription_values_keep_working() {
        assert_eq!(
            resolve_subscription_value_with("zsh", true, |_| None),
            "zsh"
        );
        assert_eq!(
            resolve_subscription_value_with("not-a-pid|htop", true, |_| None),
            "htop"
        );
    }

    #[test]
    fn current_shell_or_named_command_overrides_cached_agent_argv() {
        for command in ["zsh", "-bash", "htop", "cursor-agent"] {
            let value = format!("42|{command}|node /usr/bin/codex --yolo");
            for local in [false, true] {
                assert_eq!(
                    resolve_subscription_value_with(&value, local, |_| {
                        panic!("only ambiguous wrappers need a process lookup")
                    }),
                    command
                );
            }
        }
    }
}
