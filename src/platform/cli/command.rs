//! CLI 命令解析：把命令行参数解析成结构化命令。
//!
//! 支持 20+ 命令（spec 定义），缩写兼容。
//! 不依赖 clap subcommand（保持 main.rs 的 clap Parser 兼容）。

use crate::core::types::{PaneId, TabId};

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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum CliCommand {
    // Workspace
    NewWorkspace {
        name: Option<String>,
        socket: Option<String>,
    },
    CloseWorkspace {
        target: Option<String>,
    },
    ListWorkspaces,
    AttachWorkspace {
        target: String,
    },
    Detach {
        target: Option<String>,
    },
    RenameWorkspace {
        new_name: String,
    },

    // Tab
    NewTab {
        name: Option<String>,
    },
    KillTab {
        target: Option<TabId>,
    },
    ListTabs,
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
    /// 调整 tmux 控制 client 的整体字符格尺寸。
    ResizeClient {
        width: u16,
        height: u16,
    },

    // 输入输出
    SendKeys {
        target: Option<PaneId>,
        text: String,
    },
    /// 向 pane 写入原始字节（TUI→daemon IPC 用，data 为原始字节）。
    WriteRaw {
        target: Option<PaneId>,
        data: Vec<u8>,
    },
    CapturePane {
        target: Option<PaneId>,
        lines: Option<usize>,
    },

    // 布局查询
    ListLayout,
    DisplayMessage {
        target: PaneId,
        format: String,
    },

    /// 导出完整状态快照（TUI DaemonBackend 同步用）。
    DumpState,
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
        // Workspace
        "new-workspace" | "new-session" | "new" => CliCommand::NewWorkspace {
            name: get_opt_arg(rest, "-n"),
            socket: get_opt_arg(rest, "-s"),
        },
        "close-workspace" | "kill-session" => CliCommand::CloseWorkspace {
            target: get_opt_arg(rest, "-t"),
        },
        "list-workspaces" | "list-sessions" | "ls" => CliCommand::ListWorkspaces,
        "attach-workspace" | "attach-session" | "attach" => CliCommand::AttachWorkspace {
            target: get_req_arg(rest, "-t")
                .ok_or_else(|| CliError::MissingArg("-t workspace".into()))?,
        },
        "detach" => CliCommand::Detach {
            target: get_opt_arg(rest, "-t"),
        },
        "rename-workspace" | "rename-session" => CliCommand::RenameWorkspace {
            new_name: rest.first().cloned().unwrap_or_default(),
        },

        // Tab（tmux new-window = 我们的 new-tab）
        "new-tab" | "new-window" | "neww" => CliCommand::NewTab {
            name: get_opt_arg(rest, "-n"),
        },
        "kill-tab" => CliCommand::KillTab {
            target: get_opt_arg(rest, "-t").and_then(|s| parse_tab_id(&s)),
        },
        "list-tabs" | "lst" => CliCommand::ListTabs,
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
        "resize-pane" | "resizep" => {
            let target = get_req_arg(rest, "-t")
                .and_then(|s| parse_pane_id(&s))
                .ok_or_else(|| CliError::MissingArg("-t pane".into()))?;
            let (width, height) = parse_resize_dimensions(rest, false)?;
            CliCommand::ResizePane {
                target,
                width,
                height,
            }
        }
        "resize-client" | "resizec" => {
            let (Some(width), Some(height)) = parse_resize_dimensions(rest, true)? else {
                return Err(CliError::MissingArg("-x cols 与 -y rows".into()));
            };
            CliCommand::ResizeClient { width, height }
        }

        // 输入输出
        "send-keys" | "send" => CliCommand::SendKeys {
            target: get_opt_arg(rest, "-t").and_then(|s| parse_pane_id(&s)),
            text: get_text_arg(rest),
        },
        "write-raw" => CliCommand::WriteRaw {
            target: get_opt_arg(rest, "-t").and_then(|s| parse_pane_id(&s)),
            data: get_text_arg(rest).into_bytes(),
        },
        "capture-pane" | "capturep" => CliCommand::CapturePane {
            target: get_opt_arg(rest, "-t").and_then(|s| parse_pane_id(&s)),
            lines: get_opt_arg(rest, "-S").and_then(|s| s.parse().ok()),
        },

        // 布局查询
        "list-layout" => CliCommand::ListLayout,
        "display-message" => CliCommand::DisplayMessage {
            target: get_req_arg(rest, "-t")
                .and_then(|s| parse_pane_id(&s))
                .ok_or_else(|| CliError::MissingArg("-t pane".into()))?,
            format: get_opt_arg(rest, "-F").unwrap_or_default(),
        },
        "dump-state" => CliCommand::DumpState,

        other => return Err(CliError::UnknownCommand(other.to_string())),
    };

    Ok((command, format))
}

// ── 辅助解析函数 ──────────────────────────────────────────

/// 获取 `-X value` 形式的可选参数。
fn get_opt_arg(args: &[String], flag: &str) -> Option<String> {
    let iter = args.iter().enumerate();
    for (i, a) in iter {
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

/// 解析 resize 的字符格尺寸。
///
/// `resize-pane` 允许只指定一个轴，交给 core 的 `ResizePaneAxis`；
/// `resize-client` 则要求同时指定宽和高。所有无效尺寸在 CLI 层报告，
/// 不把它们静默变成默认的 80x24。
fn parse_resize_dimensions(
    args: &[String],
    require_both: bool,
) -> Result<(Option<u16>, Option<u16>), CliError> {
    let width = parse_resize_dimension(args, "-x", "cols")?;
    let height = parse_resize_dimension(args, "-y", "rows")?;
    if require_both && (width.is_none() || height.is_none()) {
        return Err(CliError::MissingArg("-x cols 与 -y rows".into()));
    }
    if !require_both && width.is_none() && height.is_none() {
        return Err(CliError::MissingArg("-x cols 或 -y rows".into()));
    }
    Ok((width, height))
}

fn parse_resize_dimension(
    args: &[String],
    flag: &str,
    label: &str,
) -> Result<Option<u16>, CliError> {
    if args.iter().any(|arg| arg == flag) && get_opt_arg(args, flag).is_none() {
        return Err(CliError::MissingArg(format!("{flag} {label}")));
    }
    get_opt_arg(args, flag)
        .map(|value| {
            value
                .parse::<u16>()
                .map_err(|_| CliError::InvalidArg(format!("{flag} 需要 1..65535 的整数")))
        })
        .transpose()
}

/// 获取 send-keys 的文本参数（最后的引号包裹或不带引号的文本）。
fn get_text_arg(args: &[String]) -> String {
    // 跳过 -t/-L/-s 等 flag 及其取值，避免全局参数污染文本
    let mut skip_next = false;
    let mut text_parts = Vec::new();
    for a in args {
        if skip_next {
            skip_next = false;
            continue;
        }
        if a == "-t" || a == "-L" || a == "-s" || a == "--socket" || a == "--session" {
            skip_next = true;
            continue;
        }
        if a.starts_with("-t=")
            || a.starts_with("-L=")
            || a.starts_with("-s=")
            || a.starts_with("--socket=")
            || a.starts_with("--session=")
            || a.starts_with('-')
        {
            continue;
        }
        text_parts.push(a.trim_matches('"').to_string());
    }
    text_parts.join(" ")
}

fn parse_pane_id(s: &str) -> Option<PaneId> {
    s.strip_prefix('@').and_then(|n| n.parse().ok()).map(PaneId)
}

fn parse_tab_id(s: &str) -> Option<TabId> {
    let n = s.strip_prefix('t').unwrap_or(s);
    n.parse().ok().map(TabId)
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
    fn parse_new_workspace() {
        let (cmd, _) = parse_cli_command(&args("new-session", &["-n", "dev"])).unwrap();
        assert!(matches!(cmd, CliCommand::NewWorkspace { name: Some(ref n), .. } if n == "dev"));
    }

    #[test]
    fn parse_new_workspace_alias() {
        let (cmd, _) = parse_cli_command(&args("new", &["-n", "test", "-s", "sock"])).unwrap();
        assert!(matches!(cmd, CliCommand::NewWorkspace { .. }));
    }

    #[test]
    fn parse_list_workspaces_alias() {
        let (cmd, _) = parse_cli_command(&args("ls", &[])).unwrap();
        assert!(matches!(cmd, CliCommand::ListWorkspaces));
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
    fn parse_resize_pane_single_axis() {
        let (cmd, _) = parse_cli_command(&args("resize-pane", &["-t", "@1", "-x", "60"])).unwrap();
        assert!(matches!(
            cmd,
            CliCommand::ResizePane {
                target: PaneId(1),
                width: Some(60),
                height: None
            }
        ));

        let (cmd, _) = parse_cli_command(&args("resizep", &["-t", "@2", "-y", "18"])).unwrap();
        assert!(matches!(
            cmd,
            CliCommand::ResizePane {
                target: PaneId(2),
                width: None,
                height: Some(18)
            }
        ));
    }

    #[test]
    fn parse_resize_client_requires_both_axes() {
        let (cmd, _) =
            parse_cli_command(&args("resize-client", &["-x", "120", "-y", "36"])).unwrap();
        assert!(matches!(
            cmd,
            CliCommand::ResizeClient {
                width: 120,
                height: 36
            }
        ));
        assert!(parse_cli_command(&args("resize-client", &["-x", "120"])).is_err());
        assert!(parse_cli_command(&args("resize-pane", &["-t", "@1"])).is_err());
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
    fn parse_send_keys_skips_global_l_and_s() {
        // -L/-s 是全局参数，不能拼进 send-keys 文本
        let (cmd, _) = parse_cli_command(&args(
            "send-keys",
            &["-t", "@0", "echo hi", "-L", "sock1", "-s", "demo"],
        ))
        .unwrap();
        match cmd {
            CliCommand::SendKeys { target, text } => {
                assert_eq!(target, Some(PaneId(0)));
                assert_eq!(text, "echo hi");
            }
            other => panic!("unexpected: {other:?}"),
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
    fn parse_new_tab_and_new_window_alias() {
        let (cmd, _) = parse_cli_command(&args("new-tab", &["-n", "shell"])).unwrap();
        assert!(matches!(cmd, CliCommand::NewTab { name: Some(ref n), .. } if n == "shell"));
        let (cmd, _) = parse_cli_command(&args("new-window", &["-n", "main"])).unwrap();
        assert!(matches!(cmd, CliCommand::NewTab { name: Some(ref n), .. } if n == "main"));
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
        let (cmd, _) = parse_cli_command(&args("lst", &[])).unwrap();
        assert!(matches!(cmd, CliCommand::ListTabs));
    }

    #[test]
    fn parse_select_tab() {
        let (cmd, _) = parse_cli_command(&args("select-tab", &["-t", "t2"])).unwrap();
        assert!(matches!(cmd, CliCommand::SelectTab { target: TabId(2) }));
    }

    #[test]
    fn parse_attach_workspace() {
        let (cmd, _) = parse_cli_command(&args("attach", &["-t", "demo"])).unwrap();
        match cmd {
            CliCommand::AttachWorkspace { target } => assert_eq!(target, "demo"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn parse_close_workspace() {
        let (cmd, _) = parse_cli_command(&args("close-workspace", &["-t", "demo"])).unwrap();
        assert!(matches!(
            cmd,
            CliCommand::CloseWorkspace {
                target: Some(ref t)
            } if t == "demo"
        ));
    }

    #[test]
    fn parse_list_layout() {
        let (cmd, _) = parse_cli_command(&args("list-layout", &[])).unwrap();
        assert!(matches!(cmd, CliCommand::ListLayout));
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

    /// write-raw 的 data 必须按原始字节保留（含 ESC/OSC 引导字节）。
    #[test]
    fn parse_write_raw_preserves_bytes() {
        let payload = "\u{1b}]10;rgb:0000/0000/0000\u{1b}\\";
        let (cmd, _) = parse_cli_command(&args("write-raw", &["-t", "@1", payload])).unwrap();
        match cmd {
            CliCommand::WriteRaw {
                target: Some(PaneId(1)),
                data,
            } => {
                assert_eq!(data, payload.as_bytes());
            }
            _ => panic!("应为 WriteRaw"),
        }
    }

    /// display-message 支持 tmux 风格 -F format。
    #[test]
    fn parse_display_message_format_flag() {
        let (cmd, _) = parse_cli_command(&args(
            "display-message",
            &["-t", "@0", "-F", "#{pane_current_command}"],
        ))
        .unwrap();
        assert!(matches!(
            cmd,
            CliCommand::DisplayMessage {
                target: PaneId(0),
                ref format
            } if format == "#{pane_current_command}"
        ));
    }

    /// dump-state 导出完整快照（TUI/daemon 同步用）。
    #[test]
    fn parse_dump_state() {
        let (cmd, _) = parse_cli_command(&args("dump-state", &[])).unwrap();
        assert!(matches!(cmd, CliCommand::DumpState));
    }
}
