//! tmux pane 前台进程订阅的本地增强。
//!
//! tmux 在 Linux 上把 `pane_current_command` 缩成 argv0。订阅同时携带
//! `pane_pid` 后，本地 Runtime 可以从 `/proc` 读取前台进程组完整 argv；
//! SSH Runtime 必须保留远端 tmux 原值，不能拿远端 PID 误查本机 `/proc`。

use crate::core::protocol::terminal::foreground_process_command;

/// node wrapper 需要完整 argv；tmux 的 `#()` 在 server 侧异步执行，因此 SSH
/// 也会读取远端进程。条件分支保证普通 shell/htop/cargo 不启动额外 job。
pub(crate) const PANE_PROCESS_FORMAT: &str = concat!(
    "#{pane_pid}|#{pane_current_command}|",
    "#{?#{==:#{pane_current_command},node},",
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
    let server_argv = fields
        .next()
        .map(str::trim)
        .filter(|argv| !argv.is_empty() && !argv.starts_with("<'"));
    if let Some(server_argv) = server_argv {
        return server_argv.to_string();
    }
    if !local {
        return reported.to_string();
    }
    let Ok(pid) = pid.trim().parse::<u32>() else {
        return reported.to_string();
    };
    resolve(pid)
        .filter(|command| !command.trim().is_empty())
        .unwrap_or_else(|| reported.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::attention::engine::known_agent_process_name;

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
}
