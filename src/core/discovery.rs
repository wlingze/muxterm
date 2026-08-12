//! Discovery 层：连接前的无状态查询能力。
//!
//! 设计基线：`docs/TRANSPORT-PROTOCOL-ARCHITECTURE.md` §7。
//!
//! Discovery 不建立长连接，不画进主运行时层。
//! - SSH hosts：只读取 `~/.ssh/config` 的 Host alias，不做 DNS/认证
//! - tmux sessions：`tmux -L list-sessions` 或 `ssh <alias> tmux list-sessions`
//! - 目录列表：`std::fs::read_dir` 或 `ssh <alias> ls`
//!
//! v1：先建立最小 facade，不阻塞 local CLI。

use std::path::{Path, PathBuf};

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
    let mut cmd = std::process::Command::new("tmux");
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
    let mut cmd = std::process::Command::new("tmux");
    if let Some(socket) = socket {
        cmd.args(["-L", socket]);
    }
    cmd.args(["new-session", "-d", "-s", session, "-c", directory]);
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
        shell_quote(directory)
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
    crate::core::transport::ssh::build_ssh_command(alias, remote_command, ssh_config_path)
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn run_ssh_discovery_command(
    program: &str,
    args: &[String],
    timeout: std::time::Duration,
) -> anyhow::Result<(i32, String)> {
    use crate::core::transport::ssh::SshProcessTransport;
    use crate::core::transport::{PtySize, Transport, TransportSignal};
    use std::sync::mpsc as std_mpsc;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut transport = SshProcessTransport::new();
    transport.spawn_exec(program, &arg_refs, PtySize::new(80, 24))?;
    let transport = Arc::new(Mutex::new(transport));
    let (tx, rx) = std_mpsc::channel::<Vec<u8>>();
    let reader_transport = Arc::clone(&transport);
    let reader = std::thread::spawn(move || loop {
        let mut transport = reader_transport.lock().unwrap();
        match transport.read() {
            Ok(Some(data)) => {
                drop(transport);
                if tx.send(data).is_err() {
                    break;
                }
            }
            Ok(None) => {
                drop(transport);
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(_) => break,
        }
    });

    let deadline = Instant::now() + timeout;
    let mut output = Vec::new();
    let mut exit_code = None;
    while Instant::now() < deadline {
        match rx.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(data) => output.extend_from_slice(&data),
            Err(std_mpsc::RecvTimeoutError::Timeout) => {
                let mut transport = transport.lock().unwrap();
                if let Some(code) = transport.try_wait()? {
                    exit_code = Some(code as i32);
                    break;
                }
            }
            Err(std_mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    if exit_code.is_none() {
        let mut transport = transport.lock().unwrap();
        let _ = transport.kill(TransportSignal::Term);
    }
    let _ = reader.join();
    while let Ok(data) = rx.try_recv() {
        output.extend_from_slice(&data);
    }
    if exit_code.is_none() {
        let mut transport = transport.lock().unwrap();
        exit_code = transport.try_wait()?.map(|code| code as i32);
    }

    Ok((
        exit_code.unwrap_or(124),
        String::from_utf8_lossy(&output).into_owned(),
    ))
}

/// 通过 SSH transport 在远端执行 `tmux list-sessions`，解析结果。
///
/// 使用 muxterm 自己的 `SshProcessTransport`（spawn `ssh <alias> -- tmux list-sessions`），
/// 不直接调用 `ssh` 子进程作为产品路径。transport 的 read 非阻塞，
/// 用后台线程收集输出，有硬超时。
pub fn list_ssh_tmux_sessions(
    alias: &str,
    ssh_config_path: Option<&str>,
    remote_socket: Option<&str>,
    timeout: std::time::Duration,
) -> anyhow::Result<Vec<TmuxSessionInfo>> {
    use crate::core::transport::ssh::{build_ssh_command, SshProcessTransport};
    use crate::core::transport::{PtySize, Transport, TransportSignal};
    use std::sync::mpsc as std_mpsc;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    let remote_tmux = if let Some(sk) = remote_socket {
        format!("tmux -L {} list-sessions -F '#{{session_name}},#{{session_windows}},#{{session_attached}},#{{session_created}}'", sk)
    } else {
        "tmux list-sessions -F '#{session_name},#{session_windows},#{session_attached},#{session_created}'".to_string()
    };
    let (program, args) = build_ssh_command(alias, &remote_tmux, ssh_config_path);
    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

    let mut transport = SshProcessTransport::new();
    transport
        .spawn_exec(&program, &arg_refs, PtySize::new(80, 24))
        .map_err(|e| anyhow::anyhow!("SSH transport spawn 失败: {e}"))?;

    let transport = Arc::new(Mutex::new(transport));

    // 后台线程读 transport 输出
    let (tx, rx) = std_mpsc::channel::<Vec<u8>>();
    let read_transport = transport.clone();
    let read_handle = std::thread::spawn(move || loop {
        let mut t = read_transport.lock().unwrap();
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

    // 收集输出直到超时或子进程退出
    let deadline = Instant::now() + timeout;
    let mut all_output = Vec::new();
    while Instant::now() < deadline {
        match rx.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(data) => all_output.extend_from_slice(&data),
            Err(std_mpsc::RecvTimeoutError::Timeout) => {
                // 检查子进程是否已退出
                let mut t = transport.lock().unwrap();
                if let Ok(Some(_)) = t.try_wait() {
                    drop(t);
                    break;
                }
            }
            Err(std_mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    // kill 子进程，等读线程结束（会把剩余 pty 输出投递到 channel），再一次性收尾。
    {
        let mut t = transport.lock().unwrap();
        let _ = t.kill(TransportSignal::Term);
    }
    let _ = read_handle.join();
    // 读线程已退出并关闭 channel：此刻的 rx.try_recv 能取到「最后一块」输出，
    // 避免旧实现只 try_recv 一次就 break 而丢掉尾部数据（CI 负载下偶发空列表）。
    while let Ok(data) = rx.try_recv() {
        all_output.extend_from_slice(&data);
    }

    // Check child exit code: nonzero = SSH or remote command failed
    let exit_code = {
        let mut t = transport.lock().unwrap();
        t.try_wait().ok().flatten()
    };
    if let Some(code) = exit_code {
        if code != 0 {
            let text = String::from_utf8_lossy(&all_output);
            let stderr = text.to_string();
            // tmux list-sessions returns exit 1 when no server running — that's "no sessions"
            // ssh connection failures return 255
            if code == 255 {
                return Err(anyhow::anyhow!(
                    "SSH connection failed (exit {code}): {stderr}"
                ));
            }
            // exit 1 from tmux = no sessions, return empty list (not error)
        }
    }

    Ok(parse_tmux_session_output(&String::from_utf8_lossy(
        &all_output,
    )))
}

/// 通过 SSH transport 在远端执行 `tmux list-panes`，解析结果。
///
/// 使用 muxterm 自己的 `SshProcessTransport`，不直接调用 raw ssh。
/// SSH 远端 pane 信息：(pane_id, active, cols, rows, title)
pub type SshPaneInfo = (u32, bool, u16, u16, String);

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
    let remote_command = format!("ls -1p {}", shell_quote(path));
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

#[cfg(test)]
mod tests {
    use super::*;

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

        let sessions = list_local_tmux_sessions(Some(&socket));
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
}
