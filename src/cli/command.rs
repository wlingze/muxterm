//! CLI 命令解析：把命令行参数解析成结构化命令。
//!
//! 支持 20+ 命令（spec 定义），缩写兼容。
//! 不依赖 clap subcommand（保持 main.rs 的 clap Parser 兼容）。

use crate::core::types::{PaneId, SessionId, TabId, WindowId};

/// CLI 解析错误。
#[derive(Debug, Clone)]
pub enum CliError {
    UnknownCommand(String),
    MissingArg(String),
    InvalidArg(String),
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CliError::UnknownCommand(c) => write!(f, "未知命令: {c}"),
            CliError::MissingArg(a) => write!(f, "缺少参数: {a}"),
            CliError::InvalidArg(a) => write!(f, "无效参数: {a}"),
        }
    }
}

impl std::error::Error for CliError {}

/// CLI 命令（已解析的结构化表示）。
#[derive(Debug, Clone)]
pub enum CliCommand {
    // Session
    NewSession {
        name: Option<String>,
        socket: Option<String>,
    },
    KillSession {
        target: Option<SessionId>,
    },
    ListSessions,
    AttachSession {
        target: SessionId,
    },
    Detach {
        target: Option<SessionId>,
    },
    RenameSession {
        new_name: String,
    },

    // Window
    NewWindow {
        name: Option<String>,
        session: Option<SessionId>,
    },
    KillWindow {
        target: Option<WindowId>,
    },
    ListWindows {
        session: Option<SessionId>,
    },
    SelectWindow {
        target: WindowId,
    },
    RenameWindow {
        new_name: String,
    },

    // Tab
    NewTab {
        name: Option<String>,
        window: Option<WindowId>,
    },
    KillTab {
        target: Option<TabId>,
    },
    ListTabs {
        window: Option<WindowId>,
    },
    SelectTab {
        target: TabId,
    },
    RenameTab {
        new_name: String,
    },

    // Pane
    SplitPane {
        horizontal: bool,
        target: Option<PaneId>,
        size: Option<u16>,
    },
    KillPane {
        target: Option<PaneId>,
    },
    ListPanes {
        tab: Option<TabId>,
    },
    SelectPane {
        target: PaneId,
    },
    ResizePane {
        target: PaneId,
        width: Option<u16>,
        height: Option<u16>,
    },

    // 输入输出
    SendKeys {
        target: Option<PaneId>,
        text: String,
    },
    CapturePane {
        target: Option<PaneId>,
        lines: Option<usize>,
    },

    // 布局查询
    ListLayout {
        window: Option<WindowId>,
    },
    DisplayMessage {
        target: PaneId,
        format: String,
    },
}

/// 解析 CLI 命令行参数（不含程序名）。
///
/// 用法：`muxterm <command> [options]`
/// 示例：`muxterm split-pane -h -t @1`
pub fn parse_cli_command(args: &[String]) -> Result<(CliCommand, Option<String>), CliError> {
    if args.is_empty() {
        return Err(CliError::MissingArg("命令".into()));
    }

    // 检查 --format 参数（可能出现在任意位置）
    let mut format = None;
    let mut filtered: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--format" {
            if i + 1 < args.len() {
                format = Some(args[i + 1].clone());
                i += 2;
                continue;
            }
        } else if let Some(f) = args[i].strip_prefix("--format=") {
            format = Some(f.to_string());
        }
        filtered.push(args[i].clone());
        i += 1;
    }

    let cmd = filtered[0].as_str();
    let rest = &filtered[1..];

    let command = match cmd {
        // Session
        "new-session" | "new" => CliCommand::NewSession {
            name: get_opt_arg(rest, "-n"),
            socket: get_opt_arg(rest, "-s"),
        },
        "kill-session" => CliCommand::KillSession {
            target: get_opt_arg(rest, "-t").and_then(|s| parse_session_id(&s)),
        },
        "list-sessions" | "ls" => CliCommand::ListSessions,
        "attach-session" | "attach" => CliCommand::AttachSession {
            target: get_req_arg(rest, "-t")
                .and_then(|s| parse_session_id(&s))
                .ok_or_else(|| CliError::MissingArg("-t session".into()))?,
        },
        "detach" => CliCommand::Detach {
            target: get_opt_arg(rest, "-t").and_then(|s| parse_session_id(&s)),
        },
        "rename-session" => CliCommand::RenameSession {
            new_name: rest.first().cloned().unwrap_or_default(),
        },

        // Window
        "new-window" | "neww" => CliCommand::NewWindow {
            name: get_opt_arg(rest, "-n"),
            session: get_opt_arg(rest, "-t").and_then(|s| parse_session_id(&s)),
        },
        "kill-window" | "killw" => CliCommand::KillWindow {
            target: get_opt_arg(rest, "-t").and_then(|s| parse_window_id(&s)),
        },
        "list-windows" | "lsw" => CliCommand::ListWindows {
            session: get_opt_arg(rest, "-t").and_then(|s| parse_session_id(&s)),
        },
        "select-window" | "selectw" => CliCommand::SelectWindow {
            target: get_req_arg(rest, "-t")
                .and_then(|s| parse_window_id(&s))
                .ok_or_else(|| CliError::MissingArg("-t window".into()))?,
        },
        "rename-window" | "renamew" => CliCommand::RenameWindow {
            new_name: rest.first().cloned().unwrap_or_default(),
        },

        // Tab
        "new-tab" => CliCommand::NewTab {
            name: get_opt_arg(rest, "-n"),
            window: get_opt_arg(rest, "-t").and_then(|s| parse_window_id(&s)),
        },
        "kill-tab" => CliCommand::KillTab {
            target: get_opt_arg(rest, "-t").and_then(|s| parse_tab_id(&s)),
        },
        "list-tabs" | "lst" => CliCommand::ListTabs {
            window: get_opt_arg(rest, "-t").and_then(|s| parse_window_id(&s)),
        },
        "select-tab" => CliCommand::SelectTab {
            target: get_req_arg(rest, "-t")
                .and_then(|s| parse_tab_id(&s))
                .ok_or_else(|| CliError::MissingArg("-t tab".into()))?,
        },
        "rename-tab" => CliCommand::RenameTab {
            new_name: rest.first().cloned().unwrap_or_default(),
        },

        // Pane
        "split-pane" | "splitp" => CliCommand::SplitPane {
            horizontal: rest.iter().any(|a| a == "-h"),
            target: get_opt_arg(rest, "-t").and_then(|s| parse_pane_id(&s)),
            size: get_opt_arg(rest, "-l").and_then(|s| s.parse().ok()),
        },
        "kill-pane" | "killp" => CliCommand::KillPane {
            target: get_opt_arg(rest, "-t").and_then(|s| parse_pane_id(&s)),
        },
        "list-panes" | "lsp" => CliCommand::ListPanes {
            tab: get_opt_arg(rest, "-t").and_then(|s| parse_tab_id(&s)),
        },
        "select-pane" | "selectp" => CliCommand::SelectPane {
            target: get_req_arg(rest, "-t")
                .and_then(|s| parse_pane_id(&s))
                .ok_or_else(|| CliError::MissingArg("-t pane".into()))?,
        },
        "resize-pane" | "resizep" => CliCommand::ResizePane {
            target: get_req_arg(rest, "-t")
                .and_then(|s| parse_pane_id(&s))
                .ok_or_else(|| CliError::MissingArg("-t pane".into()))?,
            width: get_opt_arg(rest, "-x").and_then(|s| s.parse().ok()),
            height: get_opt_arg(rest, "-y").and_then(|s| s.parse().ok()),
        },

        // 输入输出
        "send-keys" | "send" => CliCommand::SendKeys {
            target: get_opt_arg(rest, "-t").and_then(|s| parse_pane_id(&s)),
            text: get_text_arg(rest),
        },
        "capture-pane" | "capturep" => CliCommand::CapturePane {
            target: get_opt_arg(rest, "-t").and_then(|s| parse_pane_id(&s)),
            lines: get_opt_arg(rest, "-S").and_then(|s| s.parse().ok()),
        },

        // 布局查询
        "list-layout" => CliCommand::ListLayout {
            window: get_opt_arg(rest, "-t").and_then(|s| parse_window_id(&s)),
        },
        "display-message" => CliCommand::DisplayMessage {
            target: get_req_arg(rest, "-t")
                .and_then(|s| parse_pane_id(&s))
                .ok_or_else(|| CliError::MissingArg("-t pane".into()))?,
            format: get_opt_arg(rest, "-F").unwrap_or_default(),
        },

        other => return Err(CliError::UnknownCommand(other.to_string())),
    };

    Ok((command, format))
}

// ── 辅助解析函数 ──────────────────────────────────────────

/// 获取 `-X value` 形式的可选参数。
fn get_opt_arg(args: &[String], flag: &str) -> Option<String> {
    let mut iter = args.iter().enumerate();
    while let Some((i, a)) = iter.next() {
        if a == flag {
            return args.get(i + 1).cloned();
        }
        if let Some(v) = a.strip_prefix(&format!("{flag}=")) {
            return Some(v.to_string());
        }
    }
    None
}

/// 获取 `-X value` 形式的必选参数。
fn get_req_arg(args: &[String], flag: &str) -> Option<String> {
    get_opt_arg(args, flag)
}

/// 获取 send-keys 的文本参数（最后的引号包裹或不带引号的文本）。
fn get_text_arg(args: &[String]) -> String {
    // 跳过 -t 开头的参数对
    let mut skip_next = false;
    let mut text_parts = Vec::new();
    for a in args {
        if skip_next {
            skip_next = false;
            continue;
        }
        if a == "-t" {
            skip_next = true;
            continue;
        }
        if a.starts_with("-t=") || a.starts_with("-") {
            continue;
        }
        text_parts.push(a.trim_matches('"').to_string());
    }
    text_parts.join(" ")
}

fn parse_pane_id(s: &str) -> Option<PaneId> {
    s.strip_prefix('@').and_then(|n| n.parse().ok()).map(PaneId)
}

fn parse_window_id(s: &str) -> Option<WindowId> {
    // 接受 w1 或 1
    let n = s.strip_prefix('w').unwrap_or(s);
    n.parse().ok().map(WindowId)
}

fn parse_tab_id(s: &str) -> Option<TabId> {
    let n = s.strip_prefix('t').unwrap_or(s);
    n.parse().ok().map(TabId)
}

fn parse_session_id(s: &str) -> Option<SessionId> {
    let n = s.strip_prefix('$').unwrap_or(s);
    n.parse().ok().map(SessionId)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(cmd: &str, opts: &[&str]) -> Vec<String> {
        let mut v = vec![cmd.to_string()];
        for o in opts {
            v.push(o.to_string());
        }
        v
    }

    #[test]
    fn parse_new_session() {
        let (cmd, _) = parse_cli_command(&args("new-session", &["-n", "dev"])).unwrap();
        assert!(matches!(cmd, CliCommand::NewSession { name: Some(ref n), .. } if n == "dev"));
    }

    #[test]
    fn parse_new_session_alias() {
        let (cmd, _) = parse_cli_command(&args("new", &["-n", "test", "-s", "sock"])).unwrap();
        assert!(matches!(cmd, CliCommand::NewSession { .. }));
    }

    #[test]
    fn parse_list_sessions_alias() {
        let (cmd, _) = parse_cli_command(&args("ls", &[])).unwrap();
        assert!(matches!(cmd, CliCommand::ListSessions));
    }

    #[test]
    fn parse_split_pane_horizontal() {
        let (cmd, _) = parse_cli_command(&args("split-pane", &["-h", "-t", "@1"])).unwrap();
        match cmd {
            CliCommand::SplitPane {
                horizontal, target, ..
            } => {
                assert!(horizontal);
                assert_eq!(target, Some(PaneId(1)));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parse_split_pane_vertical() {
        let (cmd, _) = parse_cli_command(&args("splitp", &["-v"])).unwrap();
        match cmd {
            CliCommand::SplitPane { horizontal, .. } => assert!(!horizontal),
            _ => panic!(),
        }
    }

    #[test]
    fn parse_kill_pane() {
        let (cmd, _) = parse_cli_command(&args("kill-pane", &["-t", "@2"])).unwrap();
        assert!(matches!(
            cmd,
            CliCommand::KillPane {
                target: Some(PaneId(2))
            }
        ));
    }

    #[test]
    fn parse_list_panes() {
        let (cmd, _) = parse_cli_command(&args("lsp", &["-t", "t1"])).unwrap();
        assert!(matches!(
            cmd,
            CliCommand::ListPanes {
                tab: Some(TabId(1))
            }
        ));
    }

    #[test]
    fn parse_select_pane() {
        let (cmd, _) = parse_cli_command(&args("select-pane", &["-t", "@3"])).unwrap();
        assert!(matches!(cmd, CliCommand::SelectPane { target: PaneId(3) }));
    }

    #[test]
    fn parse_resize_pane() {
        let (cmd, _) =
            parse_cli_command(&args("resizep", &["-t", "@1", "-x", "120", "-y", "40"])).unwrap();
        match cmd {
            CliCommand::ResizePane {
                target,
                width,
                height,
            } => {
                assert_eq!(target, PaneId(1));
                assert_eq!(width, Some(120));
                assert_eq!(height, Some(40));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parse_send_keys() {
        let (cmd, _) = parse_cli_command(&args("send-keys", &["-t", "@1", "echo hello"])).unwrap();
        match cmd {
            CliCommand::SendKeys { target, text } => {
                assert_eq!(target, Some(PaneId(1)));
                assert_eq!(text, "echo hello");
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parse_capture_pane() {
        let (cmd, _) = parse_cli_command(&args("capturep", &["-t", "@1", "-S", "10"])).unwrap();
        match cmd {
            CliCommand::CapturePane { target, lines } => {
                assert_eq!(target, Some(PaneId(1)));
                assert_eq!(lines, Some(10));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parse_new_window() {
        let (cmd, _) = parse_cli_command(&args("new-window", &["-n", "main"])).unwrap();
        assert!(matches!(cmd, CliCommand::NewWindow { name: Some(ref n), .. } if n == "main"));
    }

    #[test]
    fn parse_kill_window() {
        let (cmd, _) = parse_cli_command(&args("killw", &["-t", "w2"])).unwrap();
        assert!(matches!(
            cmd,
            CliCommand::KillWindow {
                target: Some(WindowId(2))
            }
        ));
    }

    #[test]
    fn parse_list_windows() {
        let (cmd, _) = parse_cli_command(&args("lsw", &[])).unwrap();
        assert!(matches!(cmd, CliCommand::ListWindows { .. }));
    }

    #[test]
    fn parse_select_window() {
        let (cmd, _) = parse_cli_command(&args("selectw", &["-t", "w3"])).unwrap();
        assert!(matches!(
            cmd,
            CliCommand::SelectWindow {
                target: WindowId(3)
            }
        ));
    }

    #[test]
    fn parse_new_tab() {
        let (cmd, _) = parse_cli_command(&args("new-tab", &["-n", "shell"])).unwrap();
        assert!(matches!(cmd, CliCommand::NewTab { name: Some(ref n), .. } if n == "shell"));
    }

    #[test]
    fn parse_kill_tab() {
        let (cmd, _) = parse_cli_command(&args("kill-tab", &["-t", "t1"])).unwrap();
        assert!(matches!(
            cmd,
            CliCommand::KillTab {
                target: Some(TabId(1))
            }
        ));
    }

    #[test]
    fn parse_list_tabs() {
        let (cmd, _) = parse_cli_command(&args("lst", &["-t", "w1"])).unwrap();
        assert!(matches!(
            cmd,
            CliCommand::ListTabs {
                window: Some(WindowId(1))
            }
        ));
    }

    #[test]
    fn parse_select_tab() {
        let (cmd, _) = parse_cli_command(&args("select-tab", &["-t", "t2"])).unwrap();
        assert!(matches!(cmd, CliCommand::SelectTab { target: TabId(2) }));
    }

    #[test]
    fn parse_attach_session() {
        let (cmd, _) = parse_cli_command(&args("attach", &["-t", "$1"])).unwrap();
        assert!(matches!(
            cmd,
            CliCommand::AttachSession {
                target: SessionId(1)
            }
        ));
    }

    #[test]
    fn parse_kill_session() {
        let (cmd, _) = parse_cli_command(&args("kill-session", &["-t", "$2"])).unwrap();
        assert!(matches!(
            cmd,
            CliCommand::KillSession {
                target: Some(SessionId(2))
            }
        ));
    }

    #[test]
    fn parse_list_layout() {
        let (cmd, _) = parse_cli_command(&args("list-layout", &["-t", "w1"])).unwrap();
        assert!(matches!(
            cmd,
            CliCommand::ListLayout {
                window: Some(WindowId(1))
            }
        ));
    }

    #[test]
    fn parse_format_flag() {
        let (_, format) = parse_cli_command(&args("ls", &["--format", "json"])).unwrap();
        assert_eq!(format, Some("json".into()));
    }

    #[test]
    fn parse_format_text() {
        let (_, format) = parse_cli_command(&args("ls", &["--format=text"])).unwrap();
        assert_eq!(format, Some("text".into()));
    }

    #[test]
    fn parse_unknown_command() {
        assert!(parse_cli_command(&args("foobar", &[])).is_err());
    }

    #[test]
    fn parse_empty_args() {
        assert!(parse_cli_command(&[]).is_err());
    }
}
