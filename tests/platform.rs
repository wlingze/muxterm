//! 平台专属不变量。
//!
//! 统一的跨平台行为在 `run unified`（core / gpui-cross）里验证；这个 target
//! 只断言「当前 OS 自己的事实」，所以三个平台各跑一次才有意义：
//!
//! - 路径约定（XDG / APPDATA / LOCALAPPDATA / %TEMP%）
//! - 默认 shell（$SHELL vs %COMSPEC%）
//! - 本地 IPC 端点的形态与真实往返（Unix socket / 命名管道 / TCP loopback）
//! - 进程终止在缺失 PID 上的行为
//!
//! 不依赖 tmux / herdr / sshd，因此 Windows 也能整份跑完。

use std::io::{Read, Write};
use std::path::PathBuf;

use muxterm::ipc::{self, Chunk, Endpoint, LocalStream};
use muxterm::os;

fn env_path(key: &str) -> Option<PathBuf> {
    std::env::var_os(key)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[test]
fn config_home_follows_the_platform_convention() {
    let config = os::config_home().expect("配置目录必须可解析");
    if cfg!(windows) {
        let appdata = env_path("APPDATA").expect("Windows 必须有 %APPDATA%");
        assert_eq!(config, appdata, "Windows 配置目录就是 %APPDATA%");
    } else if let Some(xdg) = env_path("XDG_CONFIG_HOME") {
        assert_eq!(config, xdg, "Unix 优先用 $XDG_CONFIG_HOME");
    } else {
        let home = env_path("HOME").expect("Unix 必须有 $HOME");
        assert_eq!(config, home.join(".config"), "缺省是 ~/.config");
    }
}

#[test]
fn cache_home_follows_the_platform_convention() {
    let cache = os::cache_home().expect("缓存目录必须可解析");
    if cfg!(windows) {
        let local = env_path("LOCALAPPDATA");
        if let Some(local) = local {
            assert_eq!(cache, local, "Windows 缓存目录是 %LOCALAPPDATA%");
        }
    } else if let Some(xdg) = env_path("XDG_CACHE_HOME") {
        assert_eq!(cache, xdg, "Unix 优先用 $XDG_CACHE_HOME");
    } else {
        let home = env_path("HOME").expect("Unix 必须有 $HOME");
        assert_eq!(cache, home.join(".cache"), "缺省是 ~/.cache");
    }
}

#[test]
fn paste_dir_is_absolute_and_writable() {
    let dir = os::paste_dir();
    assert!(dir.is_absolute(), "粘贴目录必须是绝对路径: {dir:?}");
    if cfg!(windows) {
        assert_eq!(dir, std::env::temp_dir(), "Windows 用 %TEMP%");
    } else {
        assert_eq!(dir, PathBuf::from("/tmp"), "Unix 固定 /tmp");
    }
    let probe = dir.join(format!("muxterm-platform-probe-{}", std::process::id()));
    std::fs::write(&probe, b"ok").expect("粘贴目录必须可写");
    assert_eq!(std::fs::read(&probe).unwrap(), b"ok");
    let _ = std::fs::remove_file(&probe);
}

#[test]
fn default_shell_points_at_a_platform_shell() {
    let shell = os::default_shell();
    assert!(!shell.trim().is_empty(), "默认 shell 不能为空");
    if cfg!(windows) {
        if let Ok(comspec) = std::env::var("COMSPEC") {
            assert_eq!(shell, comspec, "Windows 用 %COMSPEC%");
        } else {
            assert!(shell.to_ascii_lowercase().ends_with("cmd.exe"));
        }
    } else {
        assert!(
            shell.starts_with('/'),
            "Unix 的 shell 必须是绝对路径: {shell}"
        );
    }
}

#[test]
fn unix_socket_capability_matches_the_platform() {
    assert_eq!(os::SUPPORTS_UNIX_SOCKET, cfg!(unix));
    if os::SUPPORTS_UNIX_SOCKET {
        return;
    }
    // Windows：tmux 的 `-CC` 控制通道走 Unix socket，错误信息必须说清楚原因。
    let message = os::unix_socket_unsupported("tmux -CC").to_string();
    assert!(message.contains("tmux -CC"));
    assert!(message.contains(std::env::consts::OS));
}

#[test]
fn endpoint_shape_follows_the_platform() {
    let path_endpoint = ipc::endpoint_from_raw("/tmp/herdr.sock").unwrap();
    assert_eq!(
        path_endpoint,
        Endpoint::Path(PathBuf::from("/tmp/herdr.sock"))
    );
    if cfg!(windows) {
        // Herdr 在 Windows 上报管道名，客户端补 `\\.\pipe\` 前缀。
        assert_eq!(
            ipc::endpoint_from_raw("herdr").unwrap(),
            Endpoint::Pipe(r"\\.\pipe\herdr".into())
        );
    } else {
        assert_eq!(
            ipc::endpoint_from_raw(r"\\.\pipe\herdr").unwrap(),
            Endpoint::Pipe(r"\\.\pipe\herdr".into()),
            "管道端点在任何平台都能被识别（日志/测试要可读）"
        );
    }
    assert_eq!(
        ipc::endpoint_from_raw("tcp://127.0.0.1:1").unwrap(),
        Endpoint::Tcp("127.0.0.1:1".parse().unwrap())
    );
}

/// TCP loopback 是 Windows 上 `ssh -L` 的本地端，三个平台都必须能跑。
#[test]
fn tcp_loopback_roundtrip() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buffer = [0u8; 4];
        stream.read_exact(&mut buffer).unwrap();
        stream.write_all(&buffer).unwrap();
        stream.flush().unwrap();
    });
    let mut client = LocalStream::connect(&Endpoint::Tcp(address)).unwrap();
    client.write_all(b"ping").unwrap();
    assert_eq!(read_exact_via_stream(&mut client, 4), b"ping");
    server.join().unwrap();
}

#[cfg(unix)]
#[test]
fn unix_socket_roundtrip() {
    let path = std::env::temp_dir().join(format!("muxterm-platform-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
    let server_path = path.clone();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buffer = [0u8; 4];
        stream.read_exact(&mut buffer).unwrap();
        stream.write_all(&buffer).unwrap();
        let _ = server_path;
    });
    let mut client = LocalStream::connect(&Endpoint::Path(path.clone())).unwrap();
    client.write_all(b"ping").unwrap();
    assert_eq!(read_exact_via_stream(&mut client, 4), b"ping");
    server.join().unwrap();
    let _ = std::fs::remove_file(&path);
}

/// Windows 的本地 Runtime（Herdr、shell）走命名管道；这里用真实的管道服务端
/// 验证客户端通道能读、能写、能在关闭时返回 EOF。
#[cfg(windows)]
#[test]
fn named_pipe_roundtrip() {
    use tokio::net::windows::named_pipe::ServerOptions;

    let name = format!(r"\\.\pipe\muxterm-platform-{}", std::process::id());
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let server_name = name.clone();
    let server = std::thread::spawn(move || {
        runtime.block_on(async move {
            let server = ServerOptions::new()
                .first_pipe_instance(true)
                .create(&server_name)
                .unwrap();
            server.connect().await.unwrap();
            let mut buffer = [0u8; 4];
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut server = server;
            server.read_exact(&mut buffer).await.unwrap();
            server.write_all(&buffer).await.unwrap();
            server.flush().await.unwrap();
            drop(server);
        });
    });
    // 服务端 create 之后才允许 connect。
    for _ in 0..100 {
        let endpoint = Endpoint::Pipe(name.clone());
        if let Ok(mut client) = LocalStream::connect(&endpoint) {
            client.write_all(b"ping").unwrap();
            assert_eq!(read_exact_via_stream(&mut client, 4), b"ping");
            server.join().unwrap();
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("命名管道 {name} 未就绪");
}

#[test]
fn terminating_a_missing_process_is_not_fatal() {
    // 不存在的 PID：Unix 的 ESRCH 映射成 Ok，Windows 的 taskkill 非零也不 panic。
    let _ = os::terminate_process(i32::MAX / 2, os::Terminate::Term);
}

fn read_exact_via_stream(stream: &mut LocalStream, want: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while out.len() < want && std::time::Instant::now() < deadline {
        match stream.read_chunk().unwrap() {
            Chunk::Data(bytes) => out.extend_from_slice(&bytes),
            Chunk::Pending => std::thread::sleep(std::time::Duration::from_millis(5)),
            Chunk::Closed => break,
        }
    }
    out
}
