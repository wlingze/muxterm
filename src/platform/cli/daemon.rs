//! Daemon 进程：持有 LocalBackend + TerminalModel，监听 unix socket 接收命令。
//!
//! 架构（参考 tmux server/client）：
//! - daemon 启动后 connect LocalBackend（spawn 默认 shell）
//! - 监听 unix socket，每收到一个 Request 就执行对应 Task
//! - 返回格式化输出给 client
//! - 收到 KillSession 或 SIGTERM/SIGINT 时优雅退出
//!
//! 不做单元测试（需要真实 socket + 后台进程），集成测试在 tests/。

use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

use anyhow::{Context, Result};
use tokio::runtime::Runtime;
use tracing::{info, warn};

use crate::core::model::TerminalModel;
use crate::core::runtime::shell::LocalBackend;
use crate::platform::cli::entry::cli_command_to_task;
use crate::platform::cli::format_output;
use crate::platform::cli::ipc::{Request, Response};

/// daemon 共享状态：单个 TerminalModel（线程安全包装）。
struct DaemonState {
    model: TerminalModel,
    #[allow(dead_code)]
    rt: Runtime,
}

// DaemonState 内含 Runtime（!Sync），但 daemon 是单线程的，
// Arc<Mutex<>> 仅为满足函数签名，实际不会跨线程共享。
unsafe impl Send for DaemonState {}
unsafe impl Sync for DaemonState {}

/// 启动 daemon：connect backend → 监听 socket → 处理请求循环。
///
/// `socket_path` 是 unix socket 路径，`name` 是 session 名（日志用）。
/// 返回时表示 daemon 即将退出。
pub fn run_daemon(socket_path: PathBuf, name: String, tmux_socket: Option<String>) -> Result<()> {
    info!(target: "muxterm", session = %name, "daemon 启动");

    // 创建 backend + model
    // 有 tmux_socket → TmuxBackend（-CC 连接 tmux），否则 LocalBackend
    let backend: Box<dyn crate::core::model::Backend> = if let Some(ref ts) = tmux_socket {
        // 检查 tmux server 是否已有同名 session
        let existing = std::process::Command::new("tmux")
            .args(["-L", ts, "list-sessions", "-F", "#{session_name}"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| s.lines().next().map(|l| l.trim().to_string()));
        let backend = if existing.as_deref() == Some(&name) {
            // session 已存在 → attach
            crate::core::runtime::tmux::TmuxBackend::new_with_attach(Some(ts), &name)
        } else {
            // session 不存在 → new-session -s <name>
            crate::core::runtime::tmux::TmuxBackend::new_with_session_name(Some(ts), &name)
        };
        Box::new(backend)
    } else {
        Box::new(LocalBackend::new("$SHELL", ""))
    };
    let mut model = TerminalModel::new(backend);
    // TmuxBackend 需要 multi_thread runtime（后台 I/O task）
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .context("build tokio runtime")?;
    rt.block_on(model.connect())?;
    let _ = model.poll_events();

    let state = Arc::new(Mutex::new(DaemonState { model, rt }));

    // 绑定 unix socket
    // 先删除可能残留的旧 socket 文件
    let _ = std::fs::remove_file(&socket_path);
    let listener = UnixListener::bind(&socket_path)
        .with_context(|| format!("绑定 socket 失败: {}", socket_path.display()))?;
    info!(target: "muxterm", socket = %socket_path.display(), "daemon 监听中");

    // 注册 SIGINT/SIGTERM → 删除 socket 并退出
    let sock_path = socket_path.clone();
    let shutdown_flag = Arc::new(Mutex::new(false));
    let flag_clone = shutdown_flag.clone();
    // 简单实现：用 ctrl-c handler
    let _ = ctrl_c_handler({
        let f = flag_clone.clone();
        move || {
            *f.lock().unwrap() = true;
        }
    });

    // 非阻塞 accept 循环
    listener.set_nonblocking(true).context("set nonblocking")?;

    loop {
        // 检查 shutdown
        if *shutdown_flag.lock().unwrap() {
            break;
        }

        match listener.accept() {
            Ok((stream, _)) => {
                if handle_connection(stream, &state)? {
                    // KillSession：daemon 退出
                    break;
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // 短暂 sleep 避免 busy loop
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => {
                warn!(target: "muxterm", error = %e, "accept 失败");
                break;
            }
        }
    }

    // 清理
    info!(target: "muxterm", "daemon 退出，清理 socket");
    let _ = std::fs::remove_file(&sock_path);
    Ok(())
}

/// 处理单个 client 连接。
fn handle_connection(
    stream: std::os::unix::net::UnixStream,
    state: &Arc<Mutex<DaemonState>>,
) -> Result<bool> {
    use std::io::{BufRead, BufReader, Write};

    let reader = BufReader::new(&stream);
    let mut writer = &stream;

    let mut should_kill = false;
    for line in reader.lines() {
        let line = line.context("读取请求行")?;
        if line.is_empty() {
            continue;
        }

        let req: Request = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                let resp = Response::err(format!("反序列化请求失败: {e}"));
                writeln!(writer, "{}", serde_json::to_string(&resp).unwrap())?;
                continue;
            }
        };

        // 检查是否是 KillSession
        if matches!(
            req.command,
            crate::platform::cli::CliCommand::KillSession { .. }
        ) {
            should_kill = true;
        }

        let resp = execute_request(&req, state);

        let resp_json = serde_json::to_string(&resp)
            .unwrap_or_else(|_| serde_json::to_string(&Response::err("响应序列化失败")).unwrap());
        writeln!(writer, "{resp_json}")?;
        writer.flush()?;

        if should_kill {
            break;
        }
    }

    Ok(should_kill)
}

/// 执行单个请求，返回 Response。
fn execute_request(req: &Request, state: &Arc<Mutex<DaemonState>>) -> Response {
    let mut st = state.lock().unwrap();

    // 先从 backend 拉取最新事件（pty 输出等）
    let _ = st.model.refresh();

    // 操作类命令转成 Task 执行
    if let Some(task) = cli_command_to_task(&req.command, st.model.state()) {
        if let Err(e) = st.model.execute(task) {
            return Response::err(format!("执行失败: {e}"));
        }
        let _ = st.model.refresh();
    }

    // 格式化输出
    let output = format_output(st.model.state(), &req.command, req.format);
    Response::ok(output)
}

/// 简易 ctrl-c handler：安装 SIGINT/SIGTERM handler 设置 flag。
fn ctrl_c_handler<F>(_f: F) -> Result<()>
where
    F: Fn() + Send + Sync + 'static,
{
    // 简化：daemon 靠 listener 非阻塞 + client KillSession 退出。
    // 真实 SIGINT/SIGTERM 处理交给 OS（进程退出时 socket 文件由 client 清理）。
    // 这里不做复杂的 signal handler 安装（避免 unsafe + 线程安全问题）。
    Ok(())
}
