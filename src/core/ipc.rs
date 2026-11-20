//! 跨平台本地 IPC：Unix socket / Windows 命名管道 / TCP loopback。
//!
//! Runtime 侧只认识「端点字符串」，具体形态由平台决定：
//!
//! | 场景 | Unix | Windows |
//! | --- | --- | --- |
//! | 本机 Runtime（Herdr API / shell daemon） | `~/.config/.../herdr.sock` | `\\.\pipe\herdr` |
//! | SSH 转发到远端 socket | `ssh -L <local.sock>:<remote.sock>` | `ssh -L 127.0.0.1:<port>:<remote.sock>` |
//!
//! Windows 的 Win32-OpenSSH 不支持把本地 Unix socket 作为 `-L` 的本地端，
//! 所以 SSH 转发在 Windows 上落到 loopback TCP；远端仍然连 Linux 的
//! Unix socket，功能不变（Herdr、shell daemon 都一样）。
//!
//! 命名管道用 `std::fs::File` 打开：Herdr 的服务端按 byte mode 建管道，
//! 读是阻塞的，所以每个通道带一个读线程把数据搬进 channel，`read_chunk`
//! 保持 Runtime 需要的「非阻塞、拿不到就返回 None」语义。

use std::io;
use std::path::{Path, PathBuf};

#[cfg(windows)]
use std::fs::File;
#[cfg(windows)]
use std::sync::mpsc;

/// 一个本地 IPC 端点。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Endpoint {
    /// 文件系统上的 socket 路径（Unix socket；Windows 上仅 AF_UNIX 场景）。
    Path(PathBuf),
    /// Windows 命名管道，值永远是带 `\\.\pipe\` 前缀的完整路径。
    Pipe(String),
    /// loopback TCP（Windows 上的 `ssh -L` 本地端）。
    Tcp(std::net::SocketAddr),
}

impl Endpoint {
    /// 端点字符串：日志与错误里用，Unix 保持原路径，管道保持完整路径。
    pub fn key(&self) -> String {
        match self {
            Self::Path(path) => path.display().to_string(),
            Self::Pipe(name) => name.clone(),
            Self::Tcp(addr) => format!("tcp://{addr}"),
        }
    }

    fn empty_error() -> io::Error {
        io::Error::new(io::ErrorKind::InvalidInput, "socket path 为空")
    }
}

/// 解析 Runtime 传来的端点字符串。
///
/// - `tcp://127.0.0.1:1234` → [`Endpoint::Tcp`]
/// - `\\.\pipe\name` → [`Endpoint::Pipe`]
/// - 其它：Unix 当路径；Windows 上不含分隔符的裸名字当管道名（Herdr 在
///   Windows 上报的就是管道名，客户端自己补前缀）。
pub fn endpoint_from_raw(raw: &str) -> io::Result<Endpoint> {
    endpoint_from_raw_for(raw, cfg!(windows))
}

/// [`endpoint_from_raw`] 的平台无关版本：Windows 逻辑在任何平台都能测。
pub fn endpoint_from_raw_for(raw: &str, windows: bool) -> io::Result<Endpoint> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(Endpoint::empty_error());
    }
    if let Some(address) = raw.strip_prefix("tcp://") {
        let address = address.parse().map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("非法 TCP 端点 {address}: {error}"),
            )
        })?;
        return Ok(Endpoint::Tcp(address));
    }
    if raw.starts_with(r"\\.\pipe\") {
        return Ok(Endpoint::Pipe(raw.to_string()));
    }
    if windows && !raw.contains(['/', '\\']) && !raw.contains(':') {
        // Herdr 的 Windows 客户端也是这么补的：`\\.\pipe\<name>`。
        return Ok(Endpoint::Pipe(format!(r"\\.\pipe\{raw}")));
    }
    Ok(Endpoint::Path(PathBuf::from(raw)))
}

/// 远端 socket 在 SSH 转发时用的“名字”：Unix 用绝对路径，Windows 用管道名。
///
/// 只在转发本机端需要它：Windows 的 `-L` 远端必须是远端 Unix socket 路径，
/// 所以这里始终把端点字符串当远端路径原样返回。
pub fn remote_path_of(endpoint: &Endpoint) -> Option<&str> {
    match endpoint {
        Endpoint::Path(path) => path.to_str(),
        Endpoint::Pipe(name) => Some(name),
        Endpoint::Tcp(_) => None,
    }
}

/// 本机 socket 是否是命名管道写法。
pub fn is_pipe(raw: &str) -> bool {
    raw.trim().starts_with(r"\\.\pipe\")
}

/// 通道的非阻塞读结果。
#[derive(Debug)]
pub enum Chunk {
    Data(Vec<u8>),
    Pending,
    Closed,
}

/// 跨平台本地字节流。
pub enum LocalStream {
    #[cfg(unix)]
    Unix(std::os::unix::net::UnixStream),
    #[cfg(windows)]
    Pipe(PipeStream),
    Tcp(std::net::TcpStream),
}

impl LocalStream {
    /// 连接一个端点；端点形态与平台不符时返回可读错误。
    pub fn connect(endpoint: &Endpoint) -> io::Result<Self> {
        match endpoint {
            Endpoint::Path(path) => Self::connect_path(path),
            Endpoint::Pipe(name) => Self::connect_pipe(name),
            Endpoint::Tcp(address) => {
                let stream = std::net::TcpStream::connect(address)?;
                stream.set_nodelay(true)?;
                Ok(Self::Tcp(stream))
            }
        }
    }

    #[cfg(unix)]
    fn connect_path(path: &Path) -> io::Result<Self> {
        Ok(Self::Unix(std::os::unix::net::UnixStream::connect(path)?))
    }

    #[cfg(not(unix))]
    fn connect_path(path: &Path) -> io::Result<Self> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!(
                "{} 是 Unix socket 路径，但本平台只支持命名管道或 TCP 端点",
                path.display()
            ),
        ))
    }

    #[cfg(windows)]
    fn connect_pipe(name: &str) -> io::Result<Self> {
        Ok(Self::Pipe(PipeStream::open(name)?))
    }

    #[cfg(not(windows))]
    fn connect_pipe(name: &str) -> io::Result<Self> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("命名管道 {name} 只在 Windows 上可用"),
        ))
    }

    /// 读一块数据；`Chunk::Pending` 表示当前没有可读字节。
    pub fn read_chunk(&mut self) -> io::Result<Chunk> {
        match self {
            #[cfg(unix)]
            Self::Unix(stream) => {
                use std::io::Read;
                let mut buffer = [0u8; 8192];
                match stream.read(&mut buffer) {
                    Ok(0) => Ok(Chunk::Closed),
                    Ok(size) => Ok(Chunk::Data(buffer[..size].to_vec())),
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(Chunk::Pending),
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => Ok(Chunk::Pending),
                    Err(error) => Err(error),
                }
            }
            #[cfg(windows)]
            Self::Pipe(pipe) => pipe.read_chunk(),
            Self::Tcp(stream) => {
                use std::io::Read;
                let mut buffer = [0u8; 8192];
                match stream.read(&mut buffer) {
                    Ok(0) => Ok(Chunk::Closed),
                    Ok(size) => Ok(Chunk::Data(buffer[..size].to_vec())),
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(Chunk::Pending),
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => Ok(Chunk::Pending),
                    Err(error) => Err(error),
                }
            }
        }
    }

    pub fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
        use std::io::Write;
        match self {
            #[cfg(unix)]
            Self::Unix(stream) => stream.write_all(data),
            #[cfg(windows)]
            Self::Pipe(pipe) => pipe.write_all(data),
            Self::Tcp(stream) => stream.write_all(data),
        }
    }

    pub fn set_nonblocking(&self, on: bool) -> io::Result<()> {
        match self {
            #[cfg(unix)]
            Self::Unix(stream) => stream.set_nonblocking(on),
            #[cfg(windows)]
            Self::Pipe(pipe) => {
                // 管道用读线程实现非阻塞；这里只需确认写入侧还活着。
                let _ = on;
                if pipe.is_closed() {
                    Err(io::Error::new(io::ErrorKind::BrokenPipe, "命名管道已关闭"))
                } else {
                    Ok(())
                }
            }
            Self::Tcp(stream) => stream.set_nonblocking(on),
        }
    }

    pub fn shutdown(&mut self) -> io::Result<()> {
        match self {
            #[cfg(unix)]
            Self::Unix(stream) => stream.shutdown(std::net::Shutdown::Both),
            #[cfg(windows)]
            Self::Pipe(pipe) => pipe.shutdown(),
            Self::Tcp(stream) => stream.shutdown(std::net::Shutdown::Both),
        }
    }
}

#[cfg(windows)]
pub struct PipeStream {
    writer: Option<File>,
    receiver: mpsc::Receiver<io::Result<Vec<u8>>>,
    closed: bool,
}

#[cfg(windows)]
impl PipeStream {
    fn open(name: &str) -> io::Result<Self> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(name)?;
        let mut reader = file.try_clone()?;
        let (sender, receiver) = mpsc::channel();
        std::thread::Builder::new()
            .name("muxterm-pipe-reader".into())
            .spawn(move || {
                use std::io::Read;
                let mut buffer = [0u8; 8192];
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) => break,
                        // 对端取消阻塞式 WriteFile 时管道会返回该错误；等价于 EOF。
                        Err(error) if error.raw_os_error() == Some(233) => break,
                        Ok(size) => {
                            if sender.send(Ok(buffer[..size].to_vec())).is_err() {
                                break;
                            }
                        }
                        Err(error) => {
                            let _ = sender.send(Err(error));
                            break;
                        }
                    }
                }
            })?;
        Ok(Self {
            writer: Some(file),
            receiver,
            closed: false,
        })
    }

    fn is_closed(&self) -> bool {
        self.closed || self.writer.is_none()
    }

    fn read_chunk(&mut self) -> io::Result<Chunk> {
        match self.receiver.try_recv() {
            Ok(Ok(bytes)) => Ok(Chunk::Data(bytes)),
            Ok(Err(error)) => Err(error),
            Err(mpsc::TryRecvError::Empty) => Ok(Chunk::Pending),
            Err(mpsc::TryRecvError::Disconnected) => {
                self.closed = true;
                Ok(Chunk::Closed)
            }
        }
    }

    fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
        use std::io::Write;
        let Some(writer) = self.writer.as_mut() else {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "命名管道已关闭"));
        };
        writer.write_all(data)?;
        writer.flush()
    }

    fn shutdown(&mut self) -> io::Result<()> {
        self.closed = true;
        self.writer.take();
        Ok(())
    }
}

/// 取一个空闲的 loopback 端口。
///
/// `ssh -L` 不接受端口 `0`，只能先绑定再释放；调用方紧接着启动 ssh 并重试，
/// 冲突窗口比“固定端口”小得多。
pub fn free_loopback_port() -> io::Result<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_paths_stay_paths() {
        let endpoint = endpoint_from_raw_for("/tmp/herdr.sock", false).unwrap();
        assert_eq!(endpoint, Endpoint::Path(PathBuf::from("/tmp/herdr.sock")));
        assert_eq!(endpoint.key(), "/tmp/herdr.sock");
    }

    #[test]
    fn windows_bare_names_become_pipes() {
        assert_eq!(
            endpoint_from_raw_for("herdr", true).unwrap(),
            Endpoint::Pipe(r"\\.\pipe\herdr".into())
        );
        assert_eq!(
            endpoint_from_raw_for(r"\\.\pipe\herdr-client", false).unwrap(),
            Endpoint::Pipe(r"\\.\pipe\herdr-client".into())
        );
    }

    #[test]
    fn windows_paths_and_tcp_stay_distinct() {
        assert_eq!(
            endpoint_from_raw_for(r"C:\Users\me\herdr.sock", true).unwrap(),
            Endpoint::Path(PathBuf::from(r"C:\Users\me\herdr.sock"))
        );
        assert_eq!(
            endpoint_from_raw_for("tcp://127.0.0.1:4321", true).unwrap(),
            Endpoint::Tcp("127.0.0.1:4321".parse().unwrap())
        );
    }

    #[test]
    fn empty_and_malformed_endpoints_are_rejected() {
        assert_eq!(
            endpoint_from_raw_for("  ", true).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            endpoint_from_raw_for("tcp://127.0.0.1", true)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn pipe_prefix_is_recognised() {
        assert!(is_pipe(r"\\.\pipe\herdr"));
        assert!(!is_pipe("/tmp/herdr.sock"));
        assert!(!is_pipe("herdr"));
    }

    #[test]
    fn free_port_is_usable() {
        let port = free_loopback_port().unwrap();
        let listener = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
        drop(listener);
    }

    #[cfg(unix)]
    #[test]
    fn unix_roundtrip_between_streams() {
        use std::io::Write;

        let path = std::env::temp_dir().join(format!("muxterm-ipc-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let endpoint = Endpoint::Path(path.clone());
        let mut client = LocalStream::connect(&endpoint).unwrap();
        client.set_nonblocking(true).unwrap();
        client.write_all(b"ping").unwrap();
        let (mut server, _) = listener.accept().unwrap();
        use std::io::Read;
        let mut buffer = [0u8; 4];
        server.read_exact(&mut buffer).unwrap();
        assert_eq!(&buffer, b"ping");
        std::thread::sleep(std::time::Duration::from_millis(20));
        server.write_all(b"pong").unwrap();
        let mut received = Vec::new();
        for _ in 0..50 {
            match client.read_chunk().unwrap() {
                Chunk::Data(bytes) => {
                    received.extend_from_slice(&bytes);
                    break;
                }
                Chunk::Pending => std::thread::sleep(std::time::Duration::from_millis(10)),
                Chunk::Closed => break,
            }
        }
        assert_eq!(received, b"pong");
        drop(listener);
        let _ = std::fs::remove_file(&path);
    }
}
