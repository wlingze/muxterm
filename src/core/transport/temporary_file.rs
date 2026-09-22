//! 本地/SSH 二进制文件传输：不经 PTY，不把图片塞进 shell 输入或命令行参数。
//!
//! 本地直接写 `/tmp`，不走 SSH 复制，也不用 macOS `TMPDIR`（`/var/folders/.../T`）。
//! 过期的 `muxterm-paste-*` 在下一次粘贴时删掉。
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, SystemTime};

use anyhow::{bail, Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const PASTE_DIR: &str = "/tmp";
const PASTE_PREFIX: &str = "muxterm-paste-";
const PASTE_TTL: Duration = Duration::from_secs(60 * 60);

fn paste_name(extension: &str) -> Result<String> {
    let mut random = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut random)?;
    let token: String = random.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!("{PASTE_PREFIX}{token}.{extension}"))
}

/// 删掉 `/tmp` 里超过 TTL 的本进程前缀文件。失败不影响这次粘贴。
pub(super) fn sweep_expired_pastes(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with(PASTE_PREFIX) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let Ok(modified) = meta.modified() else {
            continue;
        };
        if now.duration_since(modified).unwrap_or_default() > PASTE_TTL {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

pub(super) fn store(
    transport: &str,
    target: &str,
    bytes: &[u8],
    extension: &str,
) -> Result<String> {
    if bytes.is_empty() || bytes.len() > 20 * 1024 * 1024 {
        bail!("temporary file must be between 1 byte and 20 MiB");
    }
    if extension.is_empty()
        || extension.len() > 8
        || !extension.bytes().all(|b| b.is_ascii_alphanumeric())
    {
        bail!("invalid temporary file extension");
    }
    let name = paste_name(extension)?;
    match transport {
        "local" => {
            let dir = PathBuf::from(PASTE_DIR);
            sweep_expired_pastes(&dir);
            let path = dir.join(name);
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)?;
            if let Err(error) = file.write_all(bytes).and_then(|_| file.sync_all()) {
                drop(file);
                let _ = std::fs::remove_file(&path);
                return Err(error.into());
            }
            Ok(path.to_string_lossy().into_owned())
        }
        "ssh" => {
            if target.is_empty() || target.starts_with('-') {
                bail!("invalid SSH target");
            }
            let path = format!("/tmp/{name}");
            // 路径只含内部随机十六进制，不包含 clipboard 或用户 shell 文本。
            // 先清掉超过 1 小时的旧粘贴，再 noclobber 创建：失败清理不会误删已有文件。
            let script = format!(
                "find /tmp -maxdepth 1 -type f -name 'muxterm-paste-*' -mmin +60 -exec unlink {{}} \\; 2>/dev/null; umask 077; set -C; (trap 'unlink {path}' HUP INT TERM; cat; count=$(wc -c < {path}); if [ \"$count\" -ne {} ]; then unlink {path}; exit 1; fi) > {path}",
                bytes.len()
            );
            let config = std::env::var("MUXTERM_SSH_CONFIG_PATH").ok();
            let command = format!("sh -c '{}'", script.replace('\'', "'\\''"));
            let (program, mut args) =
                super::ssh::build_ssh_command(target, &command, config.as_deref());
            for arg in &mut args {
                if arg == "-tt" {
                    *arg = "-T".into();
                }
            }
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            runtime.block_on(async {
                let mut child = tokio::process::Command::new(program)
                    .args(args)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .kill_on_drop(true)
                    .spawn()
                    .context("start SSH image transfer")?;
                let mut stdin = child.stdin.take().context("SSH stdin")?;
                let mut stdout = child.stdout.take().context("SSH stdout")?.take(64 * 1024);
                let mut stderr = child.stderr.take().context("SSH stderr")?.take(64 * 1024);
                let operation = async {
                    let mut out = Vec::new();
                    let mut err = Vec::new();
                    let (write, _, _, status) = tokio::join!(
                        async {
                            let result = stdin.write_all(bytes).await;
                            drop(stdin);
                            result
                        },
                        stdout.read_to_end(&mut out),
                        stderr.read_to_end(&mut err),
                        child.wait()
                    );
                    write.context("upload clipboard image")?;
                    if !status?.success() {
                        bail!(
                            "SSH image transfer failed: {}",
                            String::from_utf8_lossy(&err)
                        );
                    }
                    Ok::<_, anyhow::Error>(())
                };
                tokio::time::timeout(Duration::from_secs(30), operation)
                    .await
                    .context("SSH image transfer timed out")??;
                Ok::<_, anyhow::Error>(())
            })?;
            Ok(path)
        }
        _ => bail!("transport does not support temporary file transfer: {transport}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn local_files_are_private_unique_and_byte_exact() {
        let bytes = b"\x89PNG\r\n\x1a\n\0binary\xff";
        let first = store("local", "", bytes, "png").unwrap();
        let second = store("local", "", bytes, "png").unwrap();
        assert_ne!(first, second);
        for path in [first, second] {
            assert!(path.starts_with("/tmp/muxterm-paste-"));
            assert!(path.ends_with(".png"));
            assert!(!path.contains("/var/folders"));
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn local_paste_sweeps_expired_tmp_files() {
        let stale = PathBuf::from(format!(
            "/tmp/muxterm-paste-stale-{}-deadbeef.png",
            std::process::id()
        ));
        std::fs::write(&stale, b"old").unwrap();
        let file = std::fs::File::options().write(true).open(&stale).unwrap();
        file.set_modified(SystemTime::now() - Duration::from_secs(2 * 60 * 60))
            .unwrap();
        drop(file);
        let fresh = store("local", "", b"new", "png").unwrap();
        assert!(!stale.exists(), "超过 1 小时的 /tmp 粘贴必须被清掉");
        assert!(fresh.starts_with("/tmp/muxterm-paste-"));
        std::fs::remove_file(fresh).unwrap();
    }

    #[test]
    fn invalid_upload_never_starts_transport() {
        assert!(store("ssh", "unused", b"x", "png;touch x").is_err());
        assert!(store("local", "", b"", "png").is_err());
        assert!(store("ssh", "-oProxyCommand=bad", b"x", "png").is_err());
    }
}
