//! Discovery 层：连接前查询（SSH hosts / tmux sessions / Herdr workspaces / 目录）。
//!
//! 设计基线：`docs/TRANSPORT-PROTOCOL-ARCHITECTURE.md` §7。
//! 现行门面：`docs/CATALOG.md`（Driver.list / Transport.list_targets / Inventory）。
//! platform 不要直接调本模块；走 Catalog。
//!
//! - SSH hosts：只读取 `~/.ssh/config` 的 Host alias，不做 DNS/认证
//! - tmux sessions：`tmux -L list-sessions` 或 `ssh <alias> tmux list-sessions`
//! - Herdr：本地 socket JSON；SSH 可用 `ssh … herdr session list`（不是 Runtime）
//! - 目录列表：`std::fs::read_dir` 或 `ssh <alias> ls`

use std::path::{Path, PathBuf};

use crate::core::quickconnect::model::ProjectExistence;

pub mod existing;

/// SSH Host 条目（从 `~/.ssh/config` 读取）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SshHostEntry {
    /// Host 别名（如 "myserver"）。
    pub alias: String,
    /// HostName（如 "server.example.com"）。
    pub hostname: String,
    /// Port（默认 22）。
    pub port: u16,
    /// User。
    pub user: String,
}

/// tmux session 信息。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TmuxSessionInfo {
    pub name: String,
    pub windows: u32,
    pub attached: bool,
    pub created: u64,
}

/// 目录条目。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FsEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: u64,
}

#[derive(Debug, Clone, Default)]
struct SshConfigBlock {
    patterns: Vec<String>,
    options: Vec<(String, String)>,
}

/// 解析 OpenSSH config 中用于连接发现的字段。
///
/// 这里只解析 Host/HostName/User/Port。认证、ProxyJump、Include 等连接行为
/// 仍然完全交给系统 `ssh`，因此 Muxterm 不会复制或替换用户的 SSH 配置。
pub fn parse_ssh_config(text: &str) -> Vec<SshHostEntry> {
    let mut global = SshConfigBlock::default();
    let mut blocks = Vec::new();
    let mut current: Option<SshConfigBlock> = None;

    for raw_line in text.lines() {
        let line = strip_config_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, raw_value)) = split_config_line(line) else {
            continue;
        };
        let key = key.to_ascii_lowercase();
        if key == "host" {
            if let Some(block) = current.take() {
                blocks.push(block);
            }
            current = Some(SshConfigBlock {
                patterns: split_config_words(raw_value),
                options: Vec::new(),
            });
        } else if let Some(block) = current.as_mut() {
            block.options.push((key, unquote_config_value(raw_value)));
        } else {
            global.options.push((key, unquote_config_value(raw_value)));
        }
    }
    if let Some(block) = current {
        blocks.push(block);
    }

    let mut aliases = Vec::new();
    for block in &blocks {
        for pattern in &block.patterns {
            if !pattern.starts_with('!') && !has_glob(pattern) && !aliases.contains(pattern) {
                aliases.push(pattern.clone());
            }
        }
    }

    aliases
        .into_iter()
        .map(|alias| {
            let hostname = ssh_config_value(&alias, "hostname", &global, &blocks)
                .unwrap_or_else(|| alias.clone());
            let user = ssh_config_value(&alias, "user", &global, &blocks).unwrap_or_default();
            let port = ssh_config_value(&alias, "port", &global, &blocks)
                .and_then(|value| value.parse().ok())
                .filter(|port: &u16| *port > 0)
                .unwrap_or(22);
            SshHostEntry {
                alias,
                hostname,
                port,
                user,
            }
        })
        .collect()
}

/// 列出用户现有 SSH 配置中的 Host alias。
///
/// `path` 仅用于测试或显式配置；未传入时优先使用 `MUXTERM_SSH_CONFIG_PATH`，
/// 否则读取默认的 `~/.ssh/config`。不存在配置文件视为没有可发现的主机。
pub fn list_ssh_hosts(path: Option<&Path>) -> anyhow::Result<Vec<SshHostEntry>> {
    let path = path
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("MUXTERM_SSH_CONFIG_PATH").map(PathBuf::from))
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join(".ssh").join("config"))
        });
    let Some(path) = path else {
        return Ok(Vec::new());
    };
    let mut visited = Vec::new();
    match load_ssh_config(&path, &mut visited) {
        Ok(text) => Ok(parse_ssh_config(&text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error.into()),
    }
}

fn load_ssh_config(path: &Path, visited: &mut Vec<PathBuf>) -> std::io::Result<String> {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if visited.contains(&path) {
        return Ok(String::new());
    }
    visited.push(path.clone());

    let text = std::fs::read_to_string(&path)?;
    let mut expanded = String::new();
    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
    for raw_line in text.lines() {
        let line = strip_config_comment(raw_line).trim();
        let is_include =
            split_config_line(line).is_some_and(|(key, _)| key.eq_ignore_ascii_case("include"));
        if !is_include {
            expanded.push_str(raw_line);
            expanded.push('\n');
            continue;
        }

        let Some((_, raw_patterns)) = split_config_line(line) else {
            continue;
        };
        for pattern in split_config_words(raw_patterns) {
            for include_path in expand_include_pattern(&pattern, base_dir) {
                match load_ssh_config(&include_path, visited) {
                    Ok(included) => expanded.push_str(&included),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
            }
        }
    }
    Ok(expanded)
}

fn expand_include_pattern(pattern: &str, base_dir: &Path) -> Vec<PathBuf> {
    let expanded = if let Some(rest) = pattern.strip_prefix("~/") {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join(rest))
            .unwrap_or_else(|| PathBuf::from(pattern))
    } else {
        let path = PathBuf::from(pattern);
        if path.is_absolute() {
            path
        } else {
            base_dir.join(path)
        }
    };
    if !has_glob(&expanded.to_string_lossy()) {
        return if expanded.is_file() {
            vec![expanded]
        } else {
            Vec::new()
        };
    }

    let Some(parent) = expanded.parent() else {
        return Vec::new();
    };
    let Some(file_pattern) = expanded.file_name().and_then(|name| name.to_str()) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = match std::fs::read_dir(parent) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_file()
                    && path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| glob_matches(file_pattern, name))
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    paths.sort();
    paths
}

fn split_config_line(line: &str) -> Option<(&str, &str)> {
    let mut fields = line.splitn(2, char::is_whitespace);
    let key = fields.next()?.trim();
    let value = fields.next()?.trim();
    (!key.is_empty() && !value.is_empty()).then_some((key, value))
}

fn strip_config_comment(line: &str) -> &str {
    let mut quote = None;
    let mut escaped = false;
    for (index, ch) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '\'' | '"' => {
                if quote == Some(ch) {
                    quote = None;
                } else if quote.is_none() {
                    quote = Some(ch);
                }
            }
            '#' if quote.is_none()
                && line[..index]
                    .chars()
                    .next_back()
                    .is_none_or(char::is_whitespace) =>
            {
                return &line[..index];
            }
            _ => {}
        }
    }
    line
}

fn split_config_words(value: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut escaped = false;
    for ch in value.chars() {
        if escaped {
            word.push(ch);
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '\'' | '"' if quote == Some(ch) => quote = None,
            '\'' | '"' if quote.is_none() => quote = Some(ch),
            c if c.is_whitespace() && quote.is_none() => {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
            c => word.push(c),
        }
    }
    if escaped {
        word.push('\\');
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

fn unquote_config_value(value: &str) -> String {
    split_config_words(value).join(" ")
}

fn ssh_config_value(
    alias: &str,
    key: &str,
    global: &SshConfigBlock,
    blocks: &[SshConfigBlock],
) -> Option<String> {
    global
        .options
        .iter()
        .find(|(option, _)| option == key)
        .map(|(_, value)| value.clone())
        .or_else(|| {
            blocks
                .iter()
                .filter(|block| ssh_block_matches(alias, &block.patterns))
                .flat_map(|block| block.options.iter())
                .find(|(option, _)| option == key)
                .map(|(_, value)| value.clone())
        })
}

fn ssh_block_matches(alias: &str, patterns: &[String]) -> bool {
    let mut has_positive = false;
    let mut positive_match = false;
    for pattern in patterns {
        if let Some(pattern) = pattern.strip_prefix('!') {
            if glob_matches(pattern, alias) {
                return false;
            }
        } else {
            has_positive = true;
            positive_match |= glob_matches(pattern, alias);
        }
    }
    !has_positive || positive_match
}

fn has_glob(value: &str) -> bool {
    value.contains(['*', '?'])
}

fn glob_matches(pattern: &str, value: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let value: Vec<char> = value.chars().collect();
    let mut dp = vec![vec![false; value.len() + 1]; pattern.len() + 1];
    dp[0][0] = true;
    for i in 0..pattern.len() {
        for j in 0..=value.len() {
            if !dp[i][j] {
                continue;
            }
            if pattern[i] == '*' {
                dp[i + 1][j] = true;
                if j < value.len() {
                    dp[i][j + 1] = true;
                }
            } else if j < value.len() && (pattern[i] == '?' || pattern[i] == value[j]) {
                dp[i + 1][j + 1] = true;
            }
        }
    }
    dp[pattern.len()][value.len()]
}

/// 列出本地 tmux server 的 session。
///
/// 执行 `tmux -L <socket> list-sessions -F '...'`，解析 TSV 输出。
/// 不建立 tmux -CC 控制连接，只是一次性 exec。
pub fn list_local_tmux_sessions(socket: Option<&str>) -> Vec<TmuxSessionInfo> {
    let mut cmd = std::process::Command::new(crate::core::executable::resolve_tmux_binary());
    if let Some(s) = socket {
        cmd.args(["-L", s]);
    }
    cmd.args([
        "list-sessions",
        "-F",
        "#{session_name},#{session_windows},#{session_attached},#{session_created}",
    ]);
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());

    let output = match cmd.output() {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };

    parse_tmux_session_output(&String::from_utf8_lossy(&output.stdout))
}

/// 在本地 tmux server 中创建 detached session。
pub fn create_local_tmux_session(
    socket: Option<&str>,
    session: &str,
    directory: &str,
) -> anyhow::Result<()> {
    let mut cmd = std::process::Command::new(crate::core::executable::resolve_tmux_binary());
    if let Some(socket) = socket {
        cmd.args(["-L", socket]);
    }
    // 展开 `~`：QuickConnect 的 path 常写 `~/Developer/...`，tmux 不认字面 ~。
    let directory = crate::core::config::expand_config_value(directory);
    cmd.args(["new-session", "-d", "-s", session, "-c", &directory]);
    let output = cmd.output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "创建本地 tmux session 失败: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// 通过系统 SSH 在远端创建 detached tmux session。
pub fn create_ssh_tmux_session(
    alias: &str,
    ssh_config_path: Option<&str>,
    remote_socket: Option<&str>,
    session: &str,
    directory: &str,
    timeout: std::time::Duration,
) -> anyhow::Result<()> {
    let socket_args = remote_socket
        .map(|socket| format!("-L {} ", shell_quote(socket)))
        .unwrap_or_default();
    let remote_command = format!(
        "tmux {socket_args}new-session -d -s {} -c {}",
        shell_quote(session),
        shell_quote_remote_path(directory)
    );
    let (program, args) = build_ssh_command_for_discovery(alias, &remote_command, ssh_config_path);
    let (exit_code, output) = run_ssh_discovery_command(&program, &args, timeout)?;
    if exit_code == 0 {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "创建远端 tmux session 失败 (exit {exit_code}): {}",
            output.trim()
        ))
    }
}

fn run_ssh_discovery_command(
    program: &str,
    args: &[String],
    timeout: std::time::Duration,
) -> anyhow::Result<(i32, String)> {
    // 短命令：ssh Command + stdout，禁止 PTY（-tt 会灌 MOTD/提示符）。
    use std::process::Command;
    use std::time::Instant;

    let mut cmd = Command::new(program);
    cmd.args(args);
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = cmd.spawn()?;
    let mut stdout = child.stdout.take().expect("stdout 已 piped");
    let (tx, rx) = std::sync::mpsc::channel::<std::io::Result<Vec<u8>>>();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let result = std::io::Read::read_to_end(&mut stdout, &mut buf).map(|_| buf);
        let _ = tx.send(result);
    });
    let deadline = Instant::now() + timeout;
    loop {
        match rx.recv_timeout(std::time::Duration::from_millis(50)) {
            Ok(Ok(bytes)) => {
                let status = child.wait()?;
                return Ok((
                    status.code().unwrap_or(124),
                    String::from_utf8_lossy(&bytes).into_owned(),
                ));
            }
            Ok(Err(e)) => return Err(e.into()),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Ok((124, String::new()));
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                let status = child.wait()?;
                return Ok((status.code().unwrap_or(124), String::new()));
            }
        }
    }
}

/// 通过 discovery 短命令在远端执行 `tmux list-sessions`，解析结果。
///
/// 走 `build_ssh_command_for_discovery`（BatchMode + ConnectTimeout=2，无 -tt）
/// + `run_ssh_discovery_command`（ssh Command + stdout），禁止 attach PTY。
pub fn list_ssh_tmux_sessions(
    alias: &str,
    ssh_config_path: Option<&str>,
    remote_socket: Option<&str>,
    timeout: std::time::Duration,
) -> anyhow::Result<Vec<TmuxSessionInfo>> {
    let remote_tmux = if let Some(sk) = remote_socket {
        format!("tmux -L {} list-sessions -F '#{{session_name}},#{{session_windows}},#{{session_attached}},#{{session_created}}'", sk)
    } else {
        "tmux list-sessions -F '#{session_name},#{session_windows},#{session_attached},#{session_created}'".to_string()
    };
    let (program, args) = build_ssh_command_for_discovery(alias, &remote_tmux, ssh_config_path);
    let (exit_code, output) = run_ssh_discovery_command(&program, &args, timeout)?;
    if exit_code == 255 {
        return Err(anyhow::anyhow!(
            "SSH connection failed (exit {exit_code}): {}",
            output.trim()
        ));
    }
    // exit 1 from tmux = no sessions, return empty list (not error)
    Ok(parse_tmux_session_output(&output))
}

fn parse_tmux_session_output(text: &str) -> Vec<TmuxSessionInfo> {
    text.lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.trim_end_matches('\r').split(',').collect();
            if parts.len() != 4 {
                return None;
            }
            Some(TmuxSessionInfo {
                name: parts[0].to_string(),
                windows: parts[1].parse().unwrap_or(0),
                attached: parts[2] == "1",
                created: parts[3].parse().unwrap_or(0),
            })
        })
        .collect()
}

fn build_ssh_command_for_discovery(
    alias: &str,
    remote_command: &str,
    ssh_config_path: Option<&str>,
) -> (String, Vec<String>) {
    // 列出/探活是短命令：BatchMode + ConnectTimeout=2，不要 -tt（那是 attach）。
    let program = "ssh".to_string();
    let mut args = Vec::new();
    if let Some(path) = ssh_config_path {
        args.push("-F".to_string());
        args.push(path.to_string());
    }
    args.push("-o".to_string());
    args.push("BatchMode=yes".to_string());
    args.push("-o".to_string());
    args.push("ConnectTimeout=2".to_string());
    args.push(alias.to_string());
    if !remote_command.is_empty() {
        args.push("--".to_string());
        args.push(remote_command.to_string());
    }
    (program, args)
}

pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Quote a path for a remote POSIX shell while preserving the conventional
/// `~/...` spelling used by Project configs. A plain `shell_quote("~/x")`
/// produces `'~/x'`, where POSIX shells deliberately do not expand `~`.
///
/// Only the leading home shorthand is left as an unquoted `$HOME`; every
/// user-provided suffix remains single-quoted, so spaces/quotes are safe.
pub fn shell_quote_remote_path(value: &str) -> String {
    if value == "~" {
        return "$HOME".to_string();
    }
    if let Some(rest) = value.strip_prefix("~/") {
        return format!("$HOME/{}", shell_quote(rest));
    }
    shell_quote(value)
}

pub type SshPaneInfo = (u32, bool, u16, u16, String);

/// 通过 SSH transport 在远端执行 `tmux list-panes`，解析结果。
///
/// 使用 muxterm 自己的 `SshProcessTransport`，不直接调用 raw ssh。
/// SSH 远端 pane 信息：(pane_id, active, cols, rows, title)
pub fn list_ssh_tmux_panes(
    alias: &str,
    ssh_config_path: Option<&str>,
    remote_socket: Option<&str>,
    session: &str,
    timeout: std::time::Duration,
) -> anyhow::Result<Vec<SshPaneInfo>> {
    use crate::core::transport::ssh::{build_ssh_command, SshProcessTransport};
    use crate::core::transport::{PtySize, Transport, TransportSignal};
    use std::sync::mpsc as std_mpsc;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    let remote_tmux = if let Some(sk) = remote_socket {
        format!(
            "tmux -L {} list-panes -t {} -F '#{{pane_id}},#{{pane_active}},#{{pane_width}},#{{pane_height}},#{{pane_title}}'",
            sk, session
        )
    } else {
        format!(
            "tmux list-panes -t {} -F '#{{pane_id}},#{{pane_active}},#{{pane_width}},#{{pane_height}},#{{pane_title}}'",
            session
        )
    };
    let (program, args) = build_ssh_command(alias, &remote_tmux, ssh_config_path);
    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

    let mut transport = SshProcessTransport::new();
    transport
        .spawn_exec(&program, &arg_refs, PtySize::new(80, 24))
        .map_err(|e| anyhow::anyhow!("SSH transport spawn 失败: {e}"))?;

    let transport = Arc::new(Mutex::new(transport));
    let (tx, rx) = std_mpsc::channel::<Vec<u8>>();
    let rt = transport.clone();
    let read_handle = std::thread::spawn(move || loop {
        let mut t = rt.lock().unwrap();
        match t.read() {
            Ok(Some(data)) => {
                drop(t);
                if tx.send(data).is_err() {
                    break;
                }
            }
            Ok(None) => {
                drop(t);
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(_) => break,
        }
    });

    let deadline = Instant::now() + timeout;
    let mut all_output = Vec::new();
    while Instant::now() < deadline {
        match rx.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(data) => all_output.extend_from_slice(&data),
            Err(std_mpsc::RecvTimeoutError::Timeout) => {
                let mut t = transport.lock().unwrap();
                if let Ok(Some(code)) = t.try_wait() {
                    if code != 0 && code != 1 {
                        let text = String::from_utf8_lossy(&all_output);
                        return Err(anyhow::anyhow!(
                            "SSH remote pane list failed (exit {code}): {text}"
                        ));
                    }
                    drop(t);
                    break;
                }
            }
            Err(std_mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    {
        let mut t = transport.lock().unwrap();
        let _ = t.kill(TransportSignal::Term);
    }
    let _ = read_handle.join();
    // 读线程结束后再 drain 尾部数据（见 list_ssh_tmux_sessions 的说明）。
    while let Ok(d) = rx.try_recv() {
        all_output.extend_from_slice(&d);
    }

    // Check child exit code: nonzero (except 1 = tmux no server) = error
    let exit_code = {
        let mut t = transport.lock().unwrap();
        t.try_wait().ok().flatten()
    };
    if let Some(code) = exit_code {
        if code != 0 && code != 1 {
            let text = String::from_utf8_lossy(&all_output);
            if code == 255 {
                return Err(anyhow::anyhow!(
                    "SSH connection failed (exit {code}): {text}"
                ));
            }
            // Other nonzero = remote command failed
            return Err(anyhow::anyhow!(
                "SSH remote pane list failed (exit {code}): {text}"
            ));
        }
    }

    let text = String::from_utf8_lossy(&all_output);
    let panes: Vec<SshPaneInfo> = text
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.trim_end_matches('\r').split(',').collect();
            if parts.len() < 5 {
                return None;
            }
            let pane_id = parts[0].strip_prefix('%')?.parse().ok()?;
            let active = parts[1] == "1";
            let cols: u16 = parts[2].parse().unwrap_or(80);
            let rows: u16 = parts[3].parse().unwrap_or(24);
            let title = parts[4..].join(",");
            Some((pane_id, active, cols, rows, title))
        })
        .collect();

    Ok(panes)
}

/// 列出本地 SSH 配置里的 Host alias（从 `~/.ssh/config` 读取）。
///
/// 只解析 `Host` 条目，忽略通配符 `Host *`（不作为可选机器）。不做 DNS/认证。
/// 解析失败（无配置 / 无法读取）返回空列表而非错误。
pub fn list_local_ssh_hosts(ssh_config_path: Option<&str>) -> Vec<String> {
    let path = ssh_config_path
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into()))
                .join(".ssh")
                .join("config")
        });
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };

    let mut hosts = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("Match ") {
            continue;
        }
        if line.len() < 5 || !line[..5].eq_ignore_ascii_case("Host ") {
            continue;
        }
        let rest = line[5..].trim();
        if rest.is_empty() || rest == "*" {
            continue;
        }
        for alias in rest.split_whitespace() {
            hosts.push(alias.to_string());
        }
    }
    hosts.sort();
    hosts.dedup();
    hosts
}

/// 列出本地目录条目（名字 + 是否目录），用于「创建新 session 时选目录」。
///
/// 非目录 / 无权限返回空列表。
pub fn list_local_dir(path: &std::path::Path) -> Vec<FsEntry> {
    let Ok(entries) = std::fs::read_dir(path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        out.push(FsEntry {
            name,
            is_dir,
            size: 0,
            modified: 0,
        });
    }
    out.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.name.cmp(&b.name)));
    out
}

/// 列出远端目录条目（名字 + 是否目录），用于「SSH 创建新 session 时选目录」。
///
/// 通过系统 `ssh <alias> -- ls -1p <path>` 执行；`-p` 让目录名带 `/` 后缀，
/// 便于区分文件与目录且不依赖 GNU find 的 `-printf`（远端可能是 macOS/BSD）。
/// 失败（SSH 连接失败 / 目录不存在）返回错误。
pub fn list_remote_dir(
    alias: &str,
    path: &str,
    ssh_config_path: Option<&str>,
    timeout: std::time::Duration,
) -> anyhow::Result<Vec<FsEntry>> {
    let remote_command = format!("ls -1p {}", shell_quote_remote_path(path));
    let (program, args) = build_ssh_command_for_discovery(alias, &remote_command, ssh_config_path);
    let (exit_code, output) = run_ssh_discovery_command(&program, &args, timeout)?;
    if exit_code != 0 {
        return Err(anyhow::anyhow!(
            "SSH 目录列表失败 (exit {exit_code}): {}",
            output.trim()
        ));
    }
    let mut out = Vec::new();
    for raw in output.lines() {
        let line = raw.trim_end_matches('\r');
        if line.is_empty() || line == "." || line == ".." {
            continue;
        }
        let is_dir = line.ends_with('/');
        let name = line.trim_end_matches('/');
        if name.is_empty() {
            continue;
        }
        out.push(FsEntry {
            name: name.to_string(),
            is_dir,
            size: 0,
            modified: 0,
        });
    }
    out.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.name.cmp(&b.name)));
    Ok(out)
}

/// 探测本地 Project path 是否为目录。
///
/// `metadata` 让不存在与其它 I/O 错误分开：不存在/普通文件显示灰态，
/// 权限等无法判断的错误不误报为不存在。
pub fn probe_local_directory(path: &str) -> ProjectExistence {
    let path = crate::core::config::expand_config_value(path);
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => ProjectExistence::Exists,
        Ok(_) => ProjectExistence::Missing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => ProjectExistence::Missing,
        Err(_) => ProjectExistence::Unknown,
    }
}

/// `test -d` 的远端探测退出码映射。
fn classify_project_directory_exit(exit_code: i32) -> ProjectExistence {
    match exit_code {
        0 => ProjectExistence::Exists,
        1 => ProjectExistence::Missing,
        _ => ProjectExistence::Unknown,
    }
}

/// 通过 SSH 探测远端 Project path 是否为目录。
///
/// 探测使用无 PTY 的短命令；SSH 连接失败、权限/执行错误或超时均返回
/// `Unknown`，只有 `test -d` 的明确成功/失败分别显示存在/不存在。
pub fn probe_remote_directory(
    alias: &str,
    path: &str,
    ssh_config_path: Option<&str>,
    timeout: std::time::Duration,
) -> ProjectExistence {
    let remote_command = format!("test -d {}", shell_quote_remote_path(path));
    let (program, args) = build_ssh_command_for_discovery(alias, &remote_command, ssh_config_path);
    match run_ssh_discovery_command(&program, &args, timeout) {
        Ok((exit_code, _)) => classify_project_directory_exit(exit_code),
        Err(_) => ProjectExistence::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static PATH_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn ssh_host_entry_serializable() {
        let e = SshHostEntry {
            alias: "myserver".into(),
            hostname: "example.com".into(),
            port: 22,
            user: "alice".into(),
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("\"alias\":\"myserver\""));
        let back: SshHostEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back, e);
    }

    #[test]
    fn parse_ssh_config_resolves_aliases_and_defaults() {
        let config = r#"
            Host *
                User shared
                Port 2200

            Host archmini work
                HostName 192.168.5.17
                User wlz
                IdentityFile "~/.ssh/id_ed25519"

            Host *.internal !blocked.internal
                HostName internal.example

            Host blocked.internal
                HostName blocked.example
        "#;

        assert_eq!(
            parse_ssh_config(config),
            vec![
                SshHostEntry {
                    alias: "archmini".into(),
                    hostname: "192.168.5.17".into(),
                    port: 2200,
                    user: "shared".into(),
                },
                SshHostEntry {
                    alias: "work".into(),
                    hostname: "192.168.5.17".into(),
                    port: 2200,
                    user: "shared".into(),
                },
                SshHostEntry {
                    alias: "blocked.internal".into(),
                    hostname: "blocked.example".into(),
                    port: 2200,
                    user: "shared".into(),
                },
            ]
        );
    }

    #[test]
    fn parse_ssh_config_alias_options_come_from_first_matching_block() {
        let config = "Host *\n User global\n\nHost dev\n User specific\n HostName dev.example\n";
        let hosts = parse_ssh_config(config);
        assert_eq!(hosts[0].user, "global");
        assert_eq!(hosts[0].hostname, "dev.example");
    }

    #[test]
    fn parse_ssh_config_ignores_wildcard_only_hosts() {
        let hosts = parse_ssh_config("Host *\n  ServerAliveInterval 30\n");
        assert!(hosts.is_empty());
    }

    #[test]
    fn list_ssh_hosts_expands_include_and_tilde() {
        let root =
            std::env::temp_dir().join(format!("muxterm-discovery-include-{}", std::process::id()));
        let include_dir = root.join("included");
        std::fs::create_dir_all(&include_dir).unwrap();
        std::fs::write(
            include_dir.join("hosts.conf"),
            "Host included\n  HostName included.example\n",
        )
        .unwrap();
        let main = root.join("config");
        std::fs::write(
            &main,
            "Include included/*.conf\nHost local\n  HostName local.example\n",
        )
        .unwrap();

        let hosts = list_ssh_hosts(Some(&main)).unwrap();
        assert_eq!(
            hosts
                .iter()
                .map(|host| host.alias.as_str())
                .collect::<Vec<_>>(),
            ["included", "local"]
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn parse_tmux_session_output_handles_crlf() {
        let sessions = parse_tmux_session_output("dev,2,1,123\r\nwork,1,0,456\n");
        assert_eq!(sessions.len(), 2);
        assert!(sessions[0].attached);
        assert_eq!(sessions[1].name, "work");
    }

    fn fn_src<'a>(src: &'a str, name: &str) -> &'a str {
        let sig = format!("fn {name}(");
        let start = src.find(&sig).unwrap_or_else(|| panic!("missing {sig}"));
        let rest = &src[start..];
        let after = &rest[sig.len()..];
        let mut rel = after.len();
        for pat in ["\nfn ", "\npub fn "] {
            if let Some(i) = after.find(pat) {
                rel = rel.min(i);
            }
        }
        &rest[..sig.len() + rel]
    }

    /// C7：列出 SSH 用短超时、BatchMode，不要强制 pty（那是 attach）。
    #[test]
    fn discovery_ssh_command_is_batch_short_timeout_no_forced_tty() {
        let (_, args) =
            build_ssh_command_for_discovery("local", "tmux list-sessions", Some("/tmp/x"));
        assert!(
            args.windows(2)
                .any(|w| w[0] == "-o" && w[1] == "BatchMode=yes"),
            "应含 BatchMode=yes: {args:?}"
        );
        assert!(
            args.windows(2)
                .any(|w| w[0] == "-o" && w[1] == "ConnectTimeout=2"),
            "应含 ConnectTimeout=2: {args:?}"
        );
        assert!(
            !args.iter().any(|a| a == "-tt" || a == "-t"),
            "列出不要分配 pty: {args:?}"
        );
        assert!(
            !args.iter().any(|a| a.contains("ConnectTimeout=10")),
            "列出不要用 attach 的 10s: {args:?}"
        );
        assert!(args.contains(&"-F".to_string()), "{args:?}");
        assert!(args.contains(&"local".to_string()), "{args:?}");
    }

    /// C7：list_ssh_tmux_sessions 必须走 discovery 命令，禁止 attach PTY transport。
    #[test]
    fn list_ssh_tmux_sessions_must_not_use_attach_transport() {
        let src = include_str!("discovery.rs");
        let body = fn_src(src, "list_ssh_tmux_sessions");
        assert!(
            body.contains("build_ssh_command_for_discovery"),
            "list_ssh_tmux_sessions 必须用 discovery 命令: {body}"
        );
        assert!(
            !body.contains("SshProcessTransport"),
            "列出禁止 SshProcessTransport PTY: {body}"
        );
        assert!(
            !body.contains("build_ssh_command("),
            "列出禁止调用 attach 用的 build_ssh_command: {body}"
        );
    }

    /// C7：discovery 短命令禁止 portable-pty / SshProcessTransport。
    #[test]
    fn run_ssh_discovery_command_must_not_use_pty() {
        let src = include_str!("discovery.rs");
        let body = fn_src(src, "run_ssh_discovery_command");
        assert!(
            !body.contains("SshProcessTransport"),
            "run_ssh_discovery_command 必须是 ssh Command + stdout，不要 PTY: {body}"
        );
    }

    #[test]
    fn tmux_session_info_serializable() {
        let s = TmuxSessionInfo {
            name: "dev".into(),
            windows: 3,
            attached: true,
            created: 1234567890,
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"name\":\"dev\""));
    }

    #[test]
    fn fs_entry_serializable() {
        let e = FsEntry {
            name: "test.txt".into(),
            is_dir: false,
            size: 1024,
            modified: 1234567890,
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("\"is_dir\":false"));
    }

    #[test]
    fn remote_path_quote_expands_home_without_losing_safe_suffix_quoting() {
        assert_eq!(shell_quote_remote_path("~"), "$HOME");
        assert_eq!(
            shell_quote_remote_path("~/Project/my repo/it's"),
            "$HOME/'Project/my repo/it'\\''s'"
        );
        assert_eq!(shell_quote_remote_path("/tmp/my repo"), "'/tmp/my repo'");
    }

    #[test]
    fn project_directory_probe_exit_codes_are_stable() {
        assert_eq!(classify_project_directory_exit(0), ProjectExistence::Exists);
        assert_eq!(
            classify_project_directory_exit(1),
            ProjectExistence::Missing
        );
        assert_eq!(
            classify_project_directory_exit(2),
            ProjectExistence::Unknown
        );
        assert_eq!(
            classify_project_directory_exit(255),
            ProjectExistence::Unknown
        );
    }

    #[test]
    fn local_project_probe_distinguishes_directory_and_missing_path() {
        let path = std::env::temp_dir().join(format!(
            "muxterm-project-probe-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir(&path).expect("测试目录应能创建");
        assert_eq!(
            probe_local_directory(path.to_str().expect("临时路径应为 UTF-8")),
            ProjectExistence::Exists
        );
        std::fs::remove_dir(&path).expect("测试目录应能清理");
        assert_eq!(
            probe_local_directory(path.to_str().expect("临时路径应为 UTF-8")),
            ProjectExistence::Missing
        );
    }

    #[test]
    fn list_local_tmux_sessions_nonexistent_socket() {
        // 使用一个不存在的 socket 名，应返回空而非 panic
        let sessions = list_local_tmux_sessions(Some("muxterm-test-nonexistent-xyz"));
        assert!(sessions.is_empty(), "不存在的 socket 应返回空列表");
    }

    #[test]
    fn create_local_tmux_session_uses_detached_isolated_socket() {
        // 安全要求：任何真实 tmux 测试必须用独立 socket，且清理也带同一个 -L。
        let socket = format!(
            "muxterm-test-create-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        );
        let dir = std::env::temp_dir();
        let dir = dir.to_str().unwrap_or("/tmp");

        // tmux 不可用（CI/无 head）时跳过，不破坏默认 server。
        if create_local_tmux_session(Some(&socket), "proj", dir).is_err() {
            let _ = std::process::Command::new("tmux")
                .args(["-L", &socket, "kill-server"])
                .output();
            return;
        }

        // CI 慢机器上 new-session 返回后 server 可能尚未就绪：轮询等待
        // session 出现（最多 5s），消除 tmux server 启动竞态。
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let sessions = loop {
            let sessions = list_local_tmux_sessions(Some(&socket));
            if sessions.iter().any(|s| s.name == "proj") || std::time::Instant::now() >= deadline {
                break sessions;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        };
        assert!(
            sessions.iter().any(|s| s.name == "proj"),
            "isolated socket 必须能看到刚创建的 session"
        );
        assert!(
            sessions.iter().all(|s| !s.attached),
            "new-session -d 必须是 detached，不能抢用户已 attach 的会话"
        );

        // 清理：只杀自己的测试 server。
        let cleanup = std::process::Command::new("tmux")
            .args(["-L", &socket, "kill-server"])
            .output();
        assert!(cleanup.is_ok(), "清理测试 socket 必须成功");
    }

    /// GUI app（Finder 启动）PATH 没有 Homebrew：仍要能创建 local tmux
    /// session，并且 `~/...` 工作目录要展开成真实路径。
    #[test]
    fn create_local_tmux_session_works_without_homebrew_in_path() {
        let _guard = PATH_LOCK.lock().unwrap();
        let old_path = std::env::var_os("PATH");
        std::env::set_var("PATH", "/usr/bin:/bin");

        let socket = format!(
            "muxterm-test-gui-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        );
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        let dir = format!(
            "{}/muxterm-tilde-{}",
            home.trim_end_matches('/'),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        );
        std::fs::create_dir_all(&dir).unwrap();
        let tilde_dir = dir.replacen(&home, "~", 1);

        let result = create_local_tmux_session(Some(&socket), "proj", &tilde_dir);
        match old_path {
            Some(v) => std::env::set_var("PATH", v),
            None => std::env::remove_var("PATH"),
        }
        let _ = std::fs::remove_dir_all(&dir);

        if result.is_err() {
            let _ = std::process::Command::new("tmux")
                .args(["-L", &socket, "kill-server"])
                .output();
            return; // 环境无 tmux 时跳过
        }
        let sessions = list_local_tmux_sessions(Some(&socket));
        assert!(
            sessions.iter().any(|s| s.name == "proj"),
            "受限 PATH 下仍应能创建 session"
        );
        let _ = std::process::Command::new("tmux")
            .args(["-L", &socket, "kill-server"])
            .output();
    }
}
