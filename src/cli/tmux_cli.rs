//! CLI tmux 结构化命令：`muxterm tmux session/tab/pane ...`
//!
//! 设计基线：`docs/TRANSPORT-PROTOCOL-ARCHITECTURE.md` §8。
//!
//! 默认 JSON 输出，统一 envelope：
//! ```json
//! {"ok":true,"data":...}
//! {"ok":false,"error":{"code":"...","message":"..."}}
//! ```

/// tmux CLI 命令（已解析）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum TmuxCliCommand {
    Session(SessionCmd),
    Tab(TabCmd),
    Pane(PaneCmd),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum SessionCmd {
    List {
        target: Target,
    },
    New {
        target: Target,
        name: String,
        cwd: Option<String>,
    },
    Attach {
        target: Target,
        name: String,
    },
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum TabCmd {
    List {
        target: Target,
        session: String,
    },
    New {
        target: Target,
        session: String,
        name: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum PaneCmd {
    List {
        target: Target,
        session: String,
        tab: Option<u32>,
    },
    Split {
        target: Target,
        session: String,
        pane: u32,
        direction: SplitDirection,
    },
    SendKeys {
        target: Target,
        session: String,
        pane: u32,
        text: String,
    },
    Capture {
        target: Target,
        session: String,
        pane: u32,
        lines: Option<usize>,
    },
}

/// 连接目标：local 或 SSH alias。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum Target {
    #[default]
    Local,
    Ssh {
        alias: String,
    },
}

impl Target {
    pub fn from_str(s: &str) -> Self {
        if s == "local" {
            Target::Local
        } else {
            Target::Ssh {
                alias: s.to_string(),
            }
        }
    }
}

/// 分割方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SplitDirection {
    Horizontal,
    Vertical,
}

impl SplitDirection {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "h" | "horizontal" => Some(SplitDirection::Horizontal),
            "v" | "vertical" => Some(SplitDirection::Vertical),
            _ => None,
        }
    }
}

/// JSON 输出 envelope。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CliEnvelope {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<CliErrorInfo>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CliErrorInfo {
    pub code: String,
    pub message: String,
}

impl CliEnvelope {
    pub fn ok(data: serde_json::Value) -> Self {
        Self {
            ok: true,
            data: Some(data),
            error: None,
        }
    }

    pub fn error(code: &str, message: &str) -> Self {
        Self {
            ok: false,
            data: None,
            error: Some(CliErrorInfo {
                code: code.to_string(),
                message: message.to_string(),
            }),
        }
    }
}

/// 解析 `muxterm tmux ...` 子命令。
///
/// 输入为 `tmux` 之后的参数（不含 `tmux` 本身）。
/// 例如 `["session", "list", "--target", "local"]`。
pub fn parse_tmux_cli(args: &[String]) -> Result<TmuxCliCommand, String> {
    if args.is_empty() {
        return Err("tmux 子命令需要指定 session/tab/pane".into());
    }

    let sub = args[0].as_str();
    let rest = &args[1..];

    match sub {
        "session" => parse_session_cmd(rest).map(TmuxCliCommand::Session),
        "tab" => parse_tab_cmd(rest).map(TmuxCliCommand::Tab),
        "pane" => parse_pane_cmd(rest).map(TmuxCliCommand::Pane),
        other => Err(format!(
            "未知 tmux 子命令: {other}（应为 session/tab/pane）"
        )),
    }
}

fn get_opt_arg(args: &[String], flag: &str) -> Option<String> {
    let mut iter = args.iter().enumerate();
    for (i, a) in iter.by_ref() {
        if a == flag {
            return args.get(i + 1).cloned();
        }
        if let Some(v) = a.strip_prefix(&format!("{flag}=")) {
            return Some(v.to_string());
        }
    }
    None
}

fn get_req_arg(args: &[String], flag: &str) -> Result<String, String> {
    get_opt_arg(args, flag).ok_or_else(|| format!("缺少参数: {flag} <value>"))
}

fn parse_target(args: &[String]) -> Target {
    get_opt_arg(args, "--target")
        .map(|s| Target::from_str(&s))
        .unwrap_or_default()
}

fn parse_session_cmd(args: &[String]) -> Result<SessionCmd, String> {
    if args.is_empty() {
        return Err("需要指定 session 子命令: list/new/attach".into());
    }
    let sub = args[0].as_str();
    let rest = &args[1..];
    let target = parse_target(rest);

    match sub {
        "list" => Ok(SessionCmd::List { target }),
        "new" => {
            let name = get_req_arg(rest, "--name")?;
            let cwd = get_opt_arg(rest, "--cwd");
            Ok(SessionCmd::New { target, name, cwd })
        }
        "attach" => {
            let name = get_req_arg(rest, "--name")?;
            Ok(SessionCmd::Attach { target, name })
        }
        other => Err(format!(
            "未知 session 子命令: {other}（应为 list/new/attach）"
        )),
    }
}

fn parse_tab_cmd(args: &[String]) -> Result<TabCmd, String> {
    if args.is_empty() {
        return Err("需要指定 tab 子命令: list/new".into());
    }
    let sub = args[0].as_str();
    let rest = &args[1..];
    let target = parse_target(rest);
    let session = get_req_arg(rest, "--session")?;

    match sub {
        "list" => Ok(TabCmd::List { target, session }),
        "new" => {
            let name = get_opt_arg(rest, "--name");
            Ok(TabCmd::New {
                target,
                session,
                name,
            })
        }
        other => Err(format!("未知 tab 子命令: {other}（应为 list/new）")),
    }
}

fn parse_pane_cmd(args: &[String]) -> Result<PaneCmd, String> {
    if args.is_empty() {
        return Err("需要指定 pane 子命令: list/split/send-keys/capture".into());
    }
    let sub = args[0].as_str();
    let rest = &args[1..];
    let target = parse_target(rest);
    let session = get_req_arg(rest, "--session")?;

    match sub {
        "list" => {
            let tab = get_opt_arg(rest, "--tab").and_then(|s| s.parse().ok());
            Ok(PaneCmd::List {
                target,
                session,
                tab,
            })
        }
        "split" => {
            let pane: u32 = get_req_arg(rest, "--pane")?
                .parse()
                .map_err(|_| "--pane 需要数字".to_string())?;
            let dir_str = get_req_arg(rest, "--direction")?;
            let direction = SplitDirection::from_str(&dir_str)
                .ok_or_else(|| format!("无效 direction: {dir_str}（应为 horizontal/vertical）"))?;
            Ok(PaneCmd::Split {
                target,
                session,
                pane,
                direction,
            })
        }
        "send-keys" => {
            let pane: u32 = get_req_arg(rest, "--pane")?
                .parse()
                .map_err(|_| "--pane 需要数字".to_string())?;
            let text = get_req_arg(rest, "--text")?;
            Ok(PaneCmd::SendKeys {
                target,
                session,
                pane,
                text,
            })
        }
        "capture" => {
            let pane: u32 = get_req_arg(rest, "--pane")?
                .parse()
                .map_err(|_| "--pane 需要数字".to_string())?;
            let lines = get_opt_arg(rest, "--lines").and_then(|s| s.parse().ok());
            Ok(PaneCmd::Capture {
                target,
                session,
                pane,
                lines,
            })
        }
        other => Err(format!(
            "未知 pane 子命令: {other}（应为 list/split/send-keys/capture）"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    // ── Session 命令解析 ──

    #[test]
    fn parse_session_list_local() {
        let cmd = parse_tmux_cli(&args(&["session", "list", "--target", "local"])).unwrap();
        assert_eq!(
            cmd,
            TmuxCliCommand::Session(SessionCmd::List {
                target: Target::Local
            })
        );
    }

    #[test]
    fn parse_session_list_default_target() {
        let cmd = parse_tmux_cli(&args(&["session", "list"])).unwrap();
        assert_eq!(
            cmd,
            TmuxCliCommand::Session(SessionCmd::List {
                target: Target::Local
            })
        );
    }

    #[test]
    fn parse_session_list_ssh_target() {
        let cmd = parse_tmux_cli(&args(&["session", "list", "--target", "myserver"])).unwrap();
        assert_eq!(
            cmd,
            TmuxCliCommand::Session(SessionCmd::List {
                target: Target::Ssh {
                    alias: "myserver".into()
                }
            })
        );
    }

    #[test]
    fn parse_session_new() {
        let cmd = parse_tmux_cli(&args(&[
            "session", "new", "--target", "local", "--name", "dev",
        ]))
        .unwrap();
        assert_eq!(
            cmd,
            TmuxCliCommand::Session(SessionCmd::New {
                target: Target::Local,
                name: "dev".into(),
                cwd: None,
            })
        );
    }

    #[test]
    fn parse_session_new_with_cwd() {
        let cmd = parse_tmux_cli(&args(&[
            "session", "new", "--target", "local", "--name", "dev", "--cwd", "/tmp",
        ]))
        .unwrap();
        assert_eq!(
            cmd,
            TmuxCliCommand::Session(SessionCmd::New {
                target: Target::Local,
                name: "dev".into(),
                cwd: Some("/tmp".into()),
            })
        );
    }

    #[test]
    fn parse_session_attach() {
        let cmd = parse_tmux_cli(&args(&[
            "session", "attach", "--target", "local", "--name", "test",
        ]))
        .unwrap();
        assert_eq!(
            cmd,
            TmuxCliCommand::Session(SessionCmd::Attach {
                target: Target::Local,
                name: "test".into(),
            })
        );
    }

    #[test]
    fn parse_session_new_missing_name() {
        let result = parse_tmux_cli(&args(&["session", "new", "--target", "local"]));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("--name"));
    }

    #[test]
    fn parse_session_invalid_sub() {
        let result = parse_tmux_cli(&args(&["session", "delete"]));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("delete"));
    }

    // ── Tab 命令解析 ──

    #[test]
    fn parse_tab_list() {
        let cmd = parse_tmux_cli(&args(&[
            "tab",
            "list",
            "--target",
            "local",
            "--session",
            "dev",
        ]))
        .unwrap();
        assert_eq!(
            cmd,
            TmuxCliCommand::Tab(TabCmd::List {
                target: Target::Local,
                session: "dev".into(),
            })
        );
    }

    #[test]
    fn parse_tab_new() {
        let cmd = parse_tmux_cli(&args(&[
            "tab",
            "new",
            "--target",
            "local",
            "--session",
            "dev",
            "--name",
            "work",
        ]))
        .unwrap();
        assert_eq!(
            cmd,
            TmuxCliCommand::Tab(TabCmd::New {
                target: Target::Local,
                session: "dev".into(),
                name: Some("work".into()),
            })
        );
    }

    #[test]
    fn parse_tab_list_missing_session() {
        let result = parse_tmux_cli(&args(&["tab", "list", "--target", "local"]));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("--session"));
    }

    // ── Pane 命令解析 ──

    #[test]
    fn parse_pane_list() {
        let cmd = parse_tmux_cli(&args(&[
            "pane",
            "list",
            "--target",
            "local",
            "--session",
            "dev",
        ]))
        .unwrap();
        assert_eq!(
            cmd,
            TmuxCliCommand::Pane(PaneCmd::List {
                target: Target::Local,
                session: "dev".into(),
                tab: None,
            })
        );
    }

    #[test]
    fn parse_pane_list_with_tab() {
        let cmd = parse_tmux_cli(&args(&[
            "pane",
            "list",
            "--target",
            "local",
            "--session",
            "dev",
            "--tab",
            "2",
        ]))
        .unwrap();
        assert_eq!(
            cmd,
            TmuxCliCommand::Pane(PaneCmd::List {
                target: Target::Local,
                session: "dev".into(),
                tab: Some(2),
            })
        );
    }

    #[test]
    fn parse_pane_split_horizontal() {
        let cmd = parse_tmux_cli(&args(&[
            "pane",
            "split",
            "--target",
            "local",
            "--session",
            "dev",
            "--pane",
            "1",
            "--direction",
            "horizontal",
        ]))
        .unwrap();
        assert_eq!(
            cmd,
            TmuxCliCommand::Pane(PaneCmd::Split {
                target: Target::Local,
                session: "dev".into(),
                pane: 1,
                direction: SplitDirection::Horizontal,
            })
        );
    }

    #[test]
    fn parse_pane_split_vertical_abbreviated() {
        let cmd = parse_tmux_cli(&args(&[
            "pane",
            "split",
            "--target",
            "local",
            "--session",
            "dev",
            "--pane",
            "3",
            "--direction",
            "v",
        ]))
        .unwrap();
        assert_eq!(
            cmd,
            TmuxCliCommand::Pane(PaneCmd::Split {
                target: Target::Local,
                session: "dev".into(),
                pane: 3,
                direction: SplitDirection::Vertical,
            })
        );
    }

    #[test]
    fn parse_pane_split_invalid_direction() {
        let result = parse_tmux_cli(&args(&[
            "pane",
            "split",
            "--target",
            "local",
            "--session",
            "dev",
            "--pane",
            "1",
            "--direction",
            "diagonal",
        ]));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("diagonal"));
    }

    #[test]
    fn parse_pane_send_keys() {
        let cmd = parse_tmux_cli(&args(&[
            "pane",
            "send-keys",
            "--target",
            "local",
            "--session",
            "dev",
            "--pane",
            "1",
            "--text",
            "echo hello",
        ]))
        .unwrap();
        assert_eq!(
            cmd,
            TmuxCliCommand::Pane(PaneCmd::SendKeys {
                target: Target::Local,
                session: "dev".into(),
                pane: 1,
                text: "echo hello".into(),
            })
        );
    }

    #[test]
    fn parse_pane_capture() {
        let cmd = parse_tmux_cli(&args(&[
            "pane",
            "capture",
            "--target",
            "local",
            "--session",
            "dev",
            "--pane",
            "1",
        ]))
        .unwrap();
        assert_eq!(
            cmd,
            TmuxCliCommand::Pane(PaneCmd::Capture {
                target: Target::Local,
                session: "dev".into(),
                pane: 1,
                lines: None,
            })
        );
    }

    #[test]
    fn parse_pane_capture_with_lines() {
        let cmd = parse_tmux_cli(&args(&[
            "pane",
            "capture",
            "--target",
            "local",
            "--session",
            "dev",
            "--pane",
            "1",
            "--lines",
            "10",
        ]))
        .unwrap();
        assert_eq!(
            cmd,
            TmuxCliCommand::Pane(PaneCmd::Capture {
                target: Target::Local,
                session: "dev".into(),
                pane: 1,
                lines: Some(10),
            })
        );
    }

    #[test]
    fn parse_pane_split_non_numeric_pane() {
        let result = parse_tmux_cli(&args(&[
            "pane",
            "split",
            "--target",
            "local",
            "--session",
            "dev",
            "--pane",
            "abc",
            "--direction",
            "h",
        ]));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("数字"));
    }

    // ── Envelope ──

    #[test]
    fn envelope_ok() {
        let env = CliEnvelope::ok(serde_json::json!({"sessions": []}));
        let json = serde_json::to_string(&env).unwrap();
        assert!(json.contains("\"ok\":true"));
        assert!(json.contains("\"data\""));
    }

    #[test]
    fn envelope_error() {
        let env = CliEnvelope::error("SESSION_NOT_FOUND", "session 'foo' 不存在");
        let json = serde_json::to_string(&env).unwrap();
        assert!(json.contains("\"ok\":false"));
        assert!(json.contains("\"error\""));
        assert!(json.contains("\"code\":\"SESSION_NOT_FOUND\""));
        assert!(json.contains("\"message\":\"session 'foo' 不存在\""));
    }

    #[test]
    fn envelope_ok_no_error_field() {
        let env = CliEnvelope::ok(serde_json::json!("data"));
        let json = serde_json::to_string(&env).unwrap();
        assert!(!json.contains("\"error\""));
    }

    #[test]
    fn envelope_error_no_data_field() {
        let env = CliEnvelope::error("ERR", "msg");
        let json = serde_json::to_string(&env).unwrap();
        assert!(!json.contains("\"data\""));
    }

    // ── Target ──

    #[test]
    fn target_local_default() {
        assert_eq!(Target::default(), Target::Local);
    }

    #[test]
    fn target_from_str_local() {
        assert_eq!(Target::from_str("local"), Target::Local);
    }

    #[test]
    fn target_from_str_ssh() {
        assert_eq!(
            Target::from_str("myserver"),
            Target::Ssh {
                alias: "myserver".into()
            }
        );
    }

    // ── SplitDirection ──

    #[test]
    fn split_direction_from_str_variants() {
        assert_eq!(
            SplitDirection::from_str("horizontal"),
            Some(SplitDirection::Horizontal)
        );
        assert_eq!(
            SplitDirection::from_str("H"),
            Some(SplitDirection::Horizontal)
        );
        assert_eq!(
            SplitDirection::from_str("vertical"),
            Some(SplitDirection::Vertical)
        );
        assert_eq!(
            SplitDirection::from_str("v"),
            Some(SplitDirection::Vertical)
        );
        assert_eq!(SplitDirection::from_str("diagonal"), None);
    }
}
