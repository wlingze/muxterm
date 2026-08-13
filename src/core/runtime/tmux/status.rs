//! tmux status bar 快照：读取 `show -g` / `show -w -g` 的 status 配置，
//! 用 `display-message -p -F` 展开 status-left/right 与每个 window 的
//! window-status-format，供 GUI 端渲染成 tmux 风格 status bar。
//!
//! 取数通过独立 `tmux`/`ssh tmux` 子进程完成（只读命令，不干扰控制客户端）；
//! 解析逻辑全部是纯函数，便于单元测试。

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::process::Command;

/// 查询目标：本地 tmux 或 SSH 远程 tmux。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatusQueryConfig {
    /// `tmux -L <socket>` 的 socket 名；空 = 默认 socket。
    pub socket: Option<String>,
    /// SSH host alias（`~/.ssh/config`）；None = 本地。
    pub ssh_alias: Option<String>,
    /// 要查询的 tmux session 名。
    pub session: String,
}

/// 一个窗口在 status bar 里的条目。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StatusWindow {
    /// tmux window id（`@N` 的数字部分），对应前端 TabId。
    pub window_id: u32,
    /// tmux window index（受 base-index 影响，通常 1 起）。
    pub index: u32,
    pub name: String,
    /// window flags（如 `*` 表示当前）。
    pub flags: String,
    pub current: bool,
    /// 已展开的 window-status-format（保留 `#[...]` 样式指令，由前端解析）。
    pub text: String,
}

/// tmux status bar 快照（JSON 序列化后给 GUI）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StatusSnapshot {
    pub enabled: bool,
    /// `top` / `bottom`。
    pub position: String,
    pub justify: String,
    pub interval: u64,
    /// 已展开的 status-left（可能为空）。
    pub left: String,
    /// 已展开的 status-right。
    pub right: String,
    pub left_length: usize,
    pub right_length: usize,
    pub status_style: String,
    pub left_style: String,
    pub right_style: String,
    pub separator: String,
    pub window_format: String,
    pub window_current_format: String,
    pub window_style: String,
    pub window_current_style: String,
    pub windows: Vec<StatusWindow>,
    /// 查询失败时的错误信息（GUI 可回退到默认 tab 栏）。
    pub error: Option<String>,
}

impl StatusSnapshot {
    /// status 关闭时的占位快照。
    pub fn disabled(position: &str) -> Self {
        Self {
            enabled: false,
            position: position.to_string(),
            justify: "left".into(),
            interval: 15,
            left: String::new(),
            right: String::new(),
            left_length: 20,
            right_length: 50,
            status_style: String::new(),
            left_style: "default".into(),
            right_style: "default".into(),
            separator: " ".into(),
            window_format: String::new(),
            window_current_format: String::new(),
            window_style: "default".into(),
            window_current_style: "default".into(),
            windows: Vec::new(),
            error: None,
        }
    }
}

/// 执行一条只读 tmux 命令并返回 stdout。
pub fn run_tmux(cfg: &StatusQueryConfig, args: &[&str]) -> Result<String> {
    let output = if let Some(alias) = &cfg.ssh_alias {
        let mut cmd = Command::new("ssh");
        cmd.args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10"]);
        cmd.arg(alias);
        cmd.args(["--", "tmux"]);
        if let Some(sock) = &cfg.socket {
            cmd.args(["-L", sock]);
        }
        cmd.args(args).output()
    } else {
        let mut cmd = Command::new("tmux");
        if let Some(sock) = &cfg.socket {
            cmd.args(["-L", sock]);
        }
        cmd.args(args).output()
    }
    .with_context(|| format!("执行 tmux {} 失败", args.join(" ")))?;

    if !output.status.success() {
        bail!(
            "tmux {} 退出码 {:?}: {}",
            args.join(" "),
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// 解析 `show -g` / `show -w -g` 输出为「选项名 → 值」。
///
/// 每行 `key value`；值可能被引号包裹（含空格时）；数组选项形如
/// `key[0] value`，这里取去掉下标后的基名，先到先得。
pub fn parse_show_output(text: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for raw in text.lines() {
        let line = raw.trim_end_matches('\r');
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once(' ') else {
            continue;
        };
        let key = key.trim();
        let base = key.split('[').next().unwrap_or(key).to_string();
        let value = unquote_option(value.trim());
        out.entry(base).or_insert(value);
    }
    out
}

/// 合并 session 级与 global 级选项：session 显式设置优先，否则用 global。
pub fn merge_session_global(
    session: &HashMap<String, String>,
    global: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut merged = global.clone();
    for (k, v) in session {
        merged.insert(k.clone(), v.clone());
    }
    merged
}

/// 去掉 tmux `show` 输出里包裹字符串值的引号，并反转义。
pub fn unquote_option(value: &str) -> String {
    let v = value.trim();
    let inner = if (v.starts_with('"') && v.ends_with('"') && v.len() >= 2)
        || (v.starts_with('\'') && v.ends_with('\'') && v.len() >= 2)
    {
        &v[1..v.len() - 1]
    } else {
        v
    };
    inner.replace("\\\"", "\"").replace("\\\\", "\\")
}

/// 解析 `list-windows -F '#{window_id}|#{window_index}|#{window_name}|#{window_flags}|#{window_active}'`。
pub fn parse_window_list(text: &str) -> Vec<StatusWindow> {
    let mut windows = Vec::new();
    for raw in text.lines() {
        let line = raw.trim_end_matches('\r');
        let mut parts = line.splitn(5, '|');
        let (Some(window_id), Some(index), Some(name), Some(flags), Some(active)) = (
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
        ) else {
            continue;
        };
        let Ok(window_id) = window_id
            .trim()
            .strip_prefix('@')
            .unwrap_or(window_id.trim())
            .parse::<u32>()
        else {
            continue;
        };
        let Ok(index) = index.trim().parse::<u32>() else {
            continue;
        };
        windows.push(StatusWindow {
            window_id,
            index,
            name: name.to_string(),
            flags: flags.to_string(),
            current: active.trim() == "1",
            text: String::new(),
        });
    }
    windows
}

/// 抓取一次完整的 status bar 快照。
pub fn fetch_snapshot(cfg: &StatusQueryConfig) -> Result<StatusSnapshot> {
    if cfg.session.is_empty() {
        bail!("session 为空");
    }
    // status 选项要读「生效值」：session 级显式设置优先，global 兜底。
    // 只读 `show -g` 会拿到 tmux 默认绿（bg=green），丢掉 ryzen 这类
    // 在 session/global 里自定义的黑色 status bar。
    let session_out = run_tmux(cfg, &["show", "-t", &cfg.session])?;
    let global_out = run_tmux(cfg, &["show", "-g"])?;
    let session_opts = parse_show_output(&session_out);
    let global_opts = parse_show_output(&global_out);
    let opts = merge_session_global(&session_opts, &global_opts);

    let enabled = opts.get("status").map(|s| s == "on").unwrap_or(true);
    let position = opts
        .get("status-position")
        .cloned()
        .unwrap_or_else(|| "bottom".into());
    // 有 tmux 就跟 tmux 保持一致（status on 就渲染）；GUI 是否用 tmux 颜色
    // 由前端 `[statusbar] mode` 决定（tmux / muxterm 主题）。
    if !enabled {
        return Ok(StatusSnapshot::disabled(&position));
    }

    let wsession_out = run_tmux(cfg, &["show", "-w", "-t", &cfg.session])?;
    let wglobal_out = run_tmux(cfg, &["show", "-w", "-g"])?;
    let wsession_opts = parse_show_output(&wsession_out);
    let wglobal_opts = parse_show_output(&wglobal_out);
    let wopts = merge_session_global(&wsession_opts, &wglobal_opts);

    let left_fmt = opts.get("status-left").cloned().unwrap_or_default();
    let right_fmt = opts.get("status-right").cloned().unwrap_or_default();
    let left = if left_fmt.is_empty() {
        String::new()
    } else {
        run_tmux(
            cfg,
            &["display-message", "-p", "-t", &cfg.session, "-F", &left_fmt],
        )?
        .trim_end_matches('\n')
        .trim_end_matches('\r')
        .to_string()
    };
    let right = if right_fmt.is_empty() {
        String::new()
    } else {
        run_tmux(
            cfg,
            &[
                "display-message",
                "-p",
                "-t",
                &cfg.session,
                "-F",
                &right_fmt,
            ],
        )?
        .trim_end_matches('\n')
        .trim_end_matches('\r')
        .to_string()
    };

    let window_fmt = wopts
        .get("window-status-format")
        .cloned()
        .unwrap_or_default();
    let window_current_fmt = wopts
        .get("window-status-current-format")
        .cloned()
        .unwrap_or_default();

    let list = run_tmux(
        cfg,
        &[
            "list-windows",
            "-t",
            &cfg.session,
            "-F",
            "#{window_id}|#{window_index}|#{window_name}|#{window_flags}|#{window_active}",
        ],
    )?;
    let mut windows = parse_window_list(&list);
    for w in &mut windows {
        let fmt = if w.current {
            &window_current_fmt
        } else {
            &window_fmt
        };
        if fmt.is_empty() {
            w.text = format!(" {} ", w.index);
            continue;
        }
        let target = format!("{}:{}", cfg.session, w.index);
        let text = run_tmux(cfg, &["display-message", "-p", "-t", &target, "-F", fmt])?;
        w.text = text
            .trim_end_matches('\n')
            .trim_end_matches('\r')
            .to_string();
    }

    Ok(StatusSnapshot {
        enabled: true,
        position,
        justify: opts
            .get("status-justify")
            .cloned()
            .unwrap_or_else(|| "left".into()),
        interval: opts
            .get("status-interval")
            .and_then(|v| v.parse().ok())
            .unwrap_or(15),
        left,
        right,
        left_length: opts
            .get("status-left-length")
            .and_then(|v| v.parse().ok())
            .unwrap_or(20),
        right_length: opts
            .get("status-right-length")
            .and_then(|v| v.parse().ok())
            .unwrap_or(50),
        status_style: opts.get("status-style").cloned().unwrap_or_default(),
        left_style: opts
            .get("status-left-style")
            .cloned()
            .unwrap_or_else(|| "default".into()),
        right_style: opts
            .get("status-right-style")
            .cloned()
            .unwrap_or_else(|| "default".into()),
        separator: wopts
            .get("window-status-separator")
            .cloned()
            .unwrap_or_else(|| " ".into()),
        window_format: window_fmt,
        window_current_format: window_current_fmt,
        window_style: wopts
            .get("window-status-style")
            .cloned()
            .unwrap_or_else(|| "default".into()),
        window_current_style: wopts
            .get("window-status-current-style")
            .cloned()
            .unwrap_or_else(|| "default".into()),
        windows,
        error: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_show_output_handles_quotes_and_arrays() {
        let text = "\
status on
status-position top
status-left ''
status-right \"#[fg=colour233,bg=colour241,bold] %d/%m \"
status-format[0] \"#[align=left range=left ...]\"
status-interval 15
status-justify centre
";
        let opts = parse_show_output(text);
        assert_eq!(opts.get("status").map(String::as_str), Some("on"));
        assert_eq!(opts.get("status-position").map(String::as_str), Some("top"));
        assert_eq!(opts.get("status-left").map(String::as_str), Some(""));
        assert_eq!(
            opts.get("status-right").map(String::as_str),
            Some("#[fg=colour233,bg=colour241,bold] %d/%m ")
        );
        // 数组选项取基名，先到先得
        assert!(opts.get("status-format").is_some());
        assert_eq!(opts.get("status-interval").map(String::as_str), Some("15"));
        assert_eq!(
            opts.get("status-justify").map(String::as_str),
            Some("centre")
        );
    }

    #[test]
    fn merge_session_global_prefers_session_values() {
        let mut global = HashMap::new();
        global.insert("status-style".into(), "bg=green,fg=black".into());
        global.insert("status-left".into(), "[#{session_name}] ".into());
        let mut session = HashMap::new();
        session.insert("status-style".into(), "bg=black,fg=white".into());

        let merged = merge_session_global(&session, &global);
        assert_eq!(
            merged.get("status-style").map(String::as_str),
            Some("bg=black,fg=white")
        );
        assert_eq!(
            merged.get("status-left").map(String::as_str),
            Some("[#{session_name}] ")
        );
    }

    #[test]
    fn unquote_option_strips_and_unescapes() {
        assert_eq!(unquote_option("\"a b\""), "a b");
        assert_eq!(unquote_option("'x'"), "x");
        assert_eq!(unquote_option(r#""a\"b""#), "a\"b");
        assert_eq!(unquote_option("plain"), "plain");
    }

    #[test]
    fn parse_window_list_reads_index_name_flags_active() {
        let text = "@0|1|sleep|*|1\r\n@1|2|workspace||0\r\n";
        let windows = parse_window_list(text);
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].window_id, 0);
        assert_eq!(windows[0].index, 1);
        assert_eq!(windows[0].name, "sleep");
        assert_eq!(windows[0].flags, "*");
        assert!(windows[0].current);
        assert!(!windows[1].current);
    }

    #[test]
    fn disabled_snapshot_has_position() {
        let s = StatusSnapshot::disabled("top");
        assert!(!s.enabled);
        assert_eq!(s.position, "top");
        assert!(s.windows.is_empty());
    }

    /// 隔离 socket 上的真实 tmux 集成测试（只读，结束后 kill 测试 server）。
    #[test]
    fn fetch_snapshot_reads_real_tmux() {
        let socket = format!("muxterm-test-status-{}", std::process::id());
        let _ = Command::new("tmux")
            .args(["-L", &socket, "kill-server"])
            .output();
        let started = Command::new("tmux")
            .args(["-L", &socket, "new-session", "-d", "-s", "foo", "sleep 30"])
            .status();
        if started.is_err() {
            return; // 环境无 tmux 时跳过
        }
        let cfg = StatusQueryConfig {
            socket: Some(socket.clone()),
            ssh_alias: None,
            session: "foo".into(),
        };
        let snap = fetch_snapshot(&cfg).expect("应能读到 status 快照");
        assert!(snap.enabled);
        assert!(snap.position == "top" || snap.position == "bottom");
        assert!(!snap.windows.is_empty());
        // session 级自定义颜色必须覆盖 global 默认（green → black）。
        let _ = Command::new("tmux")
            .args([
                "-L",
                &socket,
                "set",
                "-t",
                "foo",
                "status-style",
                "bg=black,fg=white",
            ])
            .output();
        let snap_custom = fetch_snapshot(&cfg).expect("自定义 status 快照应可读");
        assert!(
            snap_custom.status_style.contains("black"),
            "session 级 status-style 应生效: {}",
            snap_custom.status_style
        );
        let _ = Command::new("tmux")
            .args(["-L", &socket, "kill-server"])
            .output();
    }
}
