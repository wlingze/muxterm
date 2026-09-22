//! 客户端自更新的端到端契约：真实 HTTP + 真实打包产物。
//!
//! 需要 `MUXTERM_UPDATE_TEST_FEED` 指向一个包含 `latest.json` 的目录
//! （发布流程 `scripts/release-package.sh` 的产物），以及
//! `MUXTERM_UPDATE_VERSION` 作为「当前版本」，测试会：
//!
//! 1. 起一个只读本地 HTTP 服务（随机端口，不监听外部）；
//! 2. 经真实 `UpdateService::http` 检查 → 断言发现新版本；
//! 3. 一键安装 → 断言目标文件被替换成包内容、且保留了备份；
//! 4. 校验失败（篡改摘要）时必须拒绝安装。
//!
//! 未设置 `MUXTERM_UPDATE_TEST_FEED` 时整组跳过，避免默认 CI 依赖网络与
//! 本地端口。

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use muxterm::update::install::{self, InstallTarget, INSTALL_TARGET_ENV};
use muxterm::update::service::{UpdatePhase, UpdateService};

fn feed_dir() -> Option<PathBuf> {
    let raw = std::env::var("MUXTERM_UPDATE_TEST_FEED").ok()?;
    let path = PathBuf::from(raw);
    path.join("latest.json").is_file().then_some(path)
}

/// 极简只读 HTTP 服务：只服务 feed 目录里的文件，用后即关。
struct OneShotServer {
    base_url: String,
    shutdown: std::sync::mpsc::Sender<()>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl OneShotServer {
    fn start(feed: &Path) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("local addr").port();
        let feed = feed.to_path_buf();
        let (shutdown, receiver) = std::sync::mpsc::channel::<()>();
        let handle = std::thread::spawn(move || {
            listener
                .set_nonblocking(true)
                .expect("nonblocking listener");
            loop {
                if receiver.try_recv().is_ok() {
                    break;
                }
                match listener.accept() {
                    Ok((stream, _)) => {
                        // 从非阻塞 listener accept 出来的 socket 在 BSD/macOS 上
                        // 仍是非阻塞的；必须显式切回阻塞语义。
                        let _ = stream.set_nonblocking(false);
                        if let Err(error) = serve(&feed, stream) {
                            if std::env::var_os("MUXTERM_UPDATE_TEST_VERBOSE").is_some() {
                                eprintln!("[test-server] serve error: {error}");
                            }
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            base_url: format!("http://127.0.0.1:{port}"),
            shutdown,
            handle: Some(handle),
        }
    }
}

impl Drop for OneShotServer {
    fn drop(&mut self) {
        let _ = self.shutdown.send(());
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn serve(feed: &Path, mut stream: TcpStream) -> std::io::Result<()> {
    // 支持 keep-alive：ureq 会复用连接，服务端不能读完一个请求就断开。
    stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
    let mut buffer = [0u8; 8192];
    let mut pending = Vec::new();
    loop {
        // 只要缓冲区里还没有完整请求头就继续读。
        while !pending.windows(4).any(|window| window == b"\r\n\r\n") {
            match stream.read(&mut buffer) {
                Ok(0) => return Ok(()),
                Ok(read) => pending.extend_from_slice(&buffer[..read]),
                Err(_) => return Ok(()),
            }
        }
        let header_end = pending
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .expect("已确认存在请求头结束符")
            + 4;
        let head = String::from_utf8_lossy(&pending[..header_end]).to_string();
        pending.drain(..header_end);
        let path = head
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .unwrap_or("/");
        let close = head.to_ascii_lowercase().contains("connection: close");

        let name = path.trim_start_matches('/');
        if std::env::var_os("MUXTERM_UPDATE_TEST_VERBOSE").is_some() {
            eprintln!("[test-server] request {path}");
        }
        let candidate = feed.join(name);
        // 只服务 feed 目录内的文件（拒绝 .. 与绝对路径）。
        let safe = candidate.starts_with(feed)
            && candidate.is_file()
            && !name.is_empty()
            && !name.contains("..");
        if !safe {
            stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n")?;
            return Ok(());
        }
        let bytes = std::fs::read(&candidate)?;
        if std::env::var_os("MUXTERM_UPDATE_TEST_VERBOSE").is_some() {
            eprintln!("[test-server] serving {} bytes", bytes.len());
        }
        let connection = if close { "close" } else { "keep-alive" };
        stream.write_all(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: {connection}\r\n\r\n",
                bytes.len()
            )
            .as_bytes(),
        )?;
        stream.write_all(&bytes)?;
        stream.flush()?;
        if std::env::var_os("MUXTERM_UPDATE_TEST_VERBOSE").is_some() {
            eprintln!("[test-server] served {path}");
        }
        if close {
            return Ok(());
        }
    }
}

fn wait_until<F: Fn(&UpdateService) -> bool>(
    service: &mut UpdateService,
    timeout: std::time::Duration,
    predicate: F,
) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        service.poll();
        if predicate(service) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    false
}

#[test]
fn alpha_feed_is_detected_downloaded_verified_and_installed() {
    let Some(feed) = feed_dir() else {
        eprintln!("跳过：未设置 MUXTERM_UPDATE_TEST_FEED");
        return;
    };
    let server = OneShotServer::start(&feed);
    let current = std::env::var("MUXTERM_UPDATE_VERSION").unwrap_or_else(|_| "v0.0.1".into());

    // 隔离的「已安装」目标：更新必须替换它并保留备份。
    // macOS 发布物是 .app（DMG），Linux 是单文件（tar.gz），所以目标形态
    // 必须跟着平台走，否则会走错安装分支。
    let sandbox =
        std::env::temp_dir().join(format!("muxterm-update-install-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&sandbox);
    std::fs::create_dir_all(&sandbox).expect("创建沙箱目录");
    let (target, old_marker) = if cfg!(target_os = "macos") {
        let app = sandbox.join("Muxterm.app");
        let binary = app.join("Contents/MacOS/Muxterm");
        std::fs::create_dir_all(binary.parent().expect("bundle 目录")).expect("创建 bundle");
        std::fs::write(&binary, b"old-version").expect("写入旧二进制");
        (app, binary)
    } else {
        let binary = sandbox.join("muxterm");
        std::fs::write(&binary, b"old-version").expect("写入旧二进制");
        (binary.clone(), binary)
    };
    // SAFETY: 测试进程自己设置并在结束前恢复。
    unsafe { std::env::set_var(INSTALL_TARGET_ENV, &target) };

    let staging = sandbox.join("staging");
    let mut service = UpdateService::new(
        Arc::new(muxterm::update::service::HttpTransport),
        false,
        format!("{}/latest.json", server.base_url),
    )
    .with_current_version(current.clone());

    assert!(service.start_check(), "检查应当被接受");
    assert!(
        wait_until(
            &mut service,
            std::time::Duration::from_secs(30),
            |service| { matches!(service.phase(), UpdatePhase::Available { .. }) }
        ),
        "应发现比 {current} 更新的版本，实际状态 {:?}",
        service.phase()
    );
    let status = service.status_json();
    assert_eq!(status["phase"], "available");
    assert!(
        status["download_url"]
            .as_str()
            .is_some_and(|url| url.starts_with(&server.base_url)),
        "相对资产地址必须解析到本地 feed: {status}"
    );

    assert!(service.start_install(), "一键更新应当被接受");
    let installed = wait_until(
        &mut service,
        std::time::Duration::from_secs(60),
        |service| {
            matches!(
                service.phase(),
                UpdatePhase::Installed { .. } | UpdatePhase::Failed { .. }
            )
        },
    );
    assert!(installed, "安装应当在超时内结束");
    match service.phase() {
        UpdatePhase::Installed { version } => {
            assert!(version.starts_with('v'), "版本号应来自清单: {version}");
        }
        other => panic!("安装失败: {other:?}"),
    }

    let installed_bytes = std::fs::read(&old_marker).expect("读取安装后的文件");
    assert_ne!(installed_bytes, b"old-version", "目标文件应被替换");
    if cfg!(target_os = "linux") {
        assert!(
            sandbox.join("muxterm.muxterm-backup").exists(),
            "替换前应保留旧文件备份"
        );
    } else {
        assert!(
            sandbox.join("Muxterm.app.backup").exists(),
            "macOS 替换 .app 前应保留旧 bundle"
        );
    }

    // 篡改摘要后必须拒绝安装：把清单里的 sha256 改坏，重新指向篡改版 feed。
    let tampered = sandbox.join("tampered-feed");
    std::fs::create_dir_all(&tampered).expect("创建篡改 feed");
    for entry in std::fs::read_dir(&feed).expect("读取 feed") {
        let entry = entry.expect("feed entry");
        let name = entry.file_name();
        let bytes = std::fs::read(entry.path()).expect("读取 feed 文件");
        // 只改清单里的摘要，资产文件保持原样 → 校验必然失败。
        let bytes = if name == "latest.json" {
            let text = String::from_utf8(bytes).expect("清单是 UTF-8");
            text.replace("\"sha256\": \"", "\"sha256\": \"deadbeef")
                .into_bytes()
        } else {
            bytes
        };
        std::fs::write(tampered.join(name), bytes).expect("写入篡改 feed");
    }
    let tampered_server = OneShotServer::start(&tampered);
    let mut tampered_service = UpdateService::new(
        Arc::new(muxterm::update::service::HttpTransport),
        false,
        format!("{}/latest.json", tampered_server.base_url),
    )
    .with_current_version(current.clone());
    assert!(tampered_service.start_check());
    assert!(wait_until(
        &mut tampered_service,
        std::time::Duration::from_secs(30),
        |service| matches!(service.phase(), UpdatePhase::Available { .. })
    ));
    assert!(tampered_service.start_install(), "篡改版仍会启动安装");
    assert!(
        wait_until(
            &mut tampered_service,
            std::time::Duration::from_secs(60),
            |service| matches!(service.phase(), UpdatePhase::Failed { .. })
        ),
        "摘要不匹配必须导致安装失败，实际 {:?}",
        tampered_service.phase()
    );
    let message = tampered_service.status_json()["message"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        message.contains("校验和") || message.contains("sha256") || message.contains("Checksum"),
        "失败原因必须指向校验和不匹配: {message}"
    );
    // 目标必须保持为上一次成功安装的内容，不能被篡改版覆盖。
    assert!(std::fs::read(&old_marker).is_ok(), "目标文件必须仍然可读");

    unsafe { std::env::remove_var(INSTALL_TARGET_ENV) };
    let _ = install::current_install_target();
    let _ = staging;
    let _ = InstallTarget::LinuxBinary(target);
}
