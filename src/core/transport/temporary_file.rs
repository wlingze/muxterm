//! 本地/SSH 二进制文件传输：不经 PTY，不把图片塞进 shell 输入或命令行参数。
//!
//! 文件名是内容的 SHA-256。同一目标上已有字节完全相同的文件时复用路径，
//! 不再重写或重传。本地直接写 `/tmp`，不走 SSH 复制，也不用 macOS `TMPDIR`
//! （`/var/folders/.../T`）。过期的 `muxterm-paste-*` 在下一次粘贴时删掉。
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, SystemTime};

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const PASTE_DIR: &str = "/tmp";
const PASTE_PREFIX: &str = "muxterm-paste-";
const PASTE_TTL: Duration = Duration::from_secs(60 * 60);

fn content_token(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

fn paste_file_name(hash: &str, extension: &str) -> String {
    format!("{PASTE_PREFIX}{hash}.{extension}")
}

/// 远端已有同内容文件时打印 HIT 并刷新时间；否则打印 MISS，不接收文件正文。
fn remote_reuse_probe(path: &str, len: usize, hash: &str) -> String {
    format!(
        "target={path}; if [ -f \"$target\" ]; then size=$(wc -c < \"$target\" | tr -d '[:space:]'); if [ \"$size\" = \"{len}\" ]; then got=$(if command -v sha256sum >/dev/null 2>&1; then sha256sum \"$target\" | awk '{{print $1}}'; elif command -v shasum >/dev/null 2>&1; then shasum -a 256 \"$target\" | awk '{{print $1}}'; elif command -v openssl >/dev/null 2>&1; then openssl dgst -sha256 \"$target\" | awk '{{print $NF}}'; else echo missing; fi); if [ \"$got\" = \"{hash}\" ]; then touch \"$target\" 2>/dev/null || true; echo HIT; exit 0; fi; fi; fi; echo MISS"
    )
}

fn remote_store_script(path: &str, len: usize) -> String {
    format!(
        "find /tmp -maxdepth 1 -type f -name 'muxterm-paste-*' -mmin +60 -exec unlink {{}} \\; 2>/dev/null; umask 077; if [ -f {path} ]; then unlink {path}; fi; set -C; (trap 'unlink {path}' HUP INT TERM; cat; count=$(wc -c < {path}); if [ \"$count\" -ne {len} ]; then unlink {path}; exit 1; fi) > {path}"
    )
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

fn local_paste_is_current(path: &Path, bytes: &[u8]) -> Result<bool> {
    match std::fs::read(path) {
        Ok(existing) if existing == bytes => {
            // 只刷新时间和权限，不重写内容。
            if let Ok(file) = std::fs::OpenOptions::new().write(true).open(path) {
                let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
                let _ = file.set_modified(SystemTime::now());
            }
            Ok(true)
        }
        Ok(_) => Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn ssh_run(
    target: &str,
    script: &str,
    stdin_bytes: Option<&[u8]>,
    timeout: Duration,
) -> Result<String> {
    let config = std::env::var("MUXTERM_SSH_CONFIG_PATH").ok();
    let command = format!("sh -c '{}'", script.replace('\'', "'\\''"));
    let (program, mut args) = super::ssh::build_ssh_command(target, &command, config.as_deref());
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
            .stdin(if stdin_bytes.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .context("start SSH image transfer")?;
        let mut stdin = child.stdin.take();
        let mut stdout = child.stdout.take().context("SSH stdout")?.take(64 * 1024);
        let mut stderr = child.stderr.take().context("SSH stderr")?.take(64 * 1024);
        let payload = stdin_bytes.map(|bytes| bytes.to_vec());
        let operation = async {
            let mut out = Vec::new();
            let mut err = Vec::new();
            let (write, _, _, status) = tokio::join!(
                async {
                    if let Some(bytes) = payload {
                        let mut input = stdin.take().context("SSH stdin")?;
                        let result = input.write_all(&bytes).await;
                        drop(input);
                        result.context("upload clipboard image")?;
                    }
                    Ok::<_, anyhow::Error>(())
                },
                stdout.read_to_end(&mut out),
                stderr.read_to_end(&mut err),
                child.wait()
            );
            write?;
            if !status?.success() {
                bail!(
                    "SSH image transfer failed: {}",
                    String::from_utf8_lossy(&err)
                );
            }
            Ok::<_, anyhow::Error>(String::from_utf8_lossy(&out).into_owned())
        };
        tokio::time::timeout(timeout, operation)
            .await
            .context("SSH image transfer timed out")?
    })
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
    let hash = content_token(bytes);
    let name = paste_file_name(&hash, extension);
    match transport {
        "local" => {
            let dir = PathBuf::from(PASTE_DIR);
            sweep_expired_pastes(&dir);
            let path = dir.join(name);
            if local_paste_is_current(&path, bytes)? {
                return Ok(path.to_string_lossy().into_owned());
            }
            if path.exists() {
                let _ = std::fs::remove_file(&path);
            }
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
            // 路径只含内容哈希，不包含 clipboard 或用户 shell 文本。
            // 先问远端是否已有相同字节；命中就不再把图片传一遍。
            let probed = ssh_run(
                target,
                &remote_reuse_probe(&path, bytes.len(), &hash),
                None,
                Duration::from_secs(15),
            )?;
            if probed.lines().any(|line| line.trim() == "HIT") {
                return Ok(path);
            }
            if !probed.lines().any(|line| line.trim() == "MISS") {
                bail!("SSH image probe failed: {probed}");
            }
            ssh_run(
                target,
                &remote_store_script(&path, bytes.len()),
                Some(bytes),
                Duration::from_secs(30),
            )?;
            Ok(path)
        }
        _ => bail!("transport does not support temporary file transfer: {transport}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    fn unique_bytes(label: &str) -> Vec<u8> {
        format!(
            "muxterm-test-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        )
        .into_bytes()
    }

    #[test]
    fn content_token_is_sha256_of_the_bytes() {
        assert_eq!(
            content_token(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn local_files_reuse_identical_bytes_and_replace_a_mismatch() {
        let bytes = unique_bytes("reuse");
        let first = store("local", "", &bytes, "png").unwrap();
        let inode = std::fs::metadata(&first).unwrap().ino();
        let file = std::fs::File::options().write(true).open(&first).unwrap();
        file.set_modified(SystemTime::now() - Duration::from_secs(30 * 60))
            .unwrap();
        drop(file);

        let second = store("local", "", &bytes, "png").unwrap();
        assert_eq!(first, second, "相同内容必须复用路径，不能再造随机文件名");
        assert_eq!(std::fs::metadata(&second).unwrap().ino(), inode);
        assert!(
            SystemTime::now()
                .duration_since(std::fs::metadata(&second).unwrap().modified().unwrap())
                .unwrap_or_default()
                < Duration::from_secs(30)
        );
        assert_eq!(std::fs::read(&second).unwrap(), bytes);
        assert_eq!(
            std::fs::metadata(&second).unwrap().permissions().mode() & 0o777,
            0o600
        );

        std::fs::write(&second, b"different-bytes").unwrap();
        let replaced = store("local", "", &bytes, "png").unwrap();
        assert_eq!(replaced, second);
        assert_eq!(std::fs::read(&replaced).unwrap(), bytes);

        let other = store("local", "", &unique_bytes("other"), "png").unwrap();
        assert_ne!(other, second);
        std::fs::remove_file(second).unwrap();
        std::fs::remove_file(other).unwrap();
    }

    #[test]
    fn remote_probe_checks_the_hash_before_any_upload() {
        let hash = content_token(b"\x89PNG");
        let path = format!("/tmp/{}", paste_file_name(&hash, "png"));
        let probe = remote_reuse_probe(&path, 4, &hash);
        let store_script = remote_store_script(&path, 4);
        assert!(probe.contains(&hash));
        assert!(probe.contains("echo HIT"));
        assert!(probe.contains("echo MISS"));
        assert!(!probe.contains("cat"));
        assert!(!probe.contains("PNG"));
        assert!(store_script.find("unlink").unwrap() < store_script.find("cat").unwrap());
        assert!(store("ssh", "unused", b"x", "png;touch x").is_err());
        assert!(store("local", "", b"", "png").is_err());
        assert!(store("ssh", "-oProxyCommand=bad", b"x", "png").is_err());
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
        let fresh = store("local", "", &unique_bytes("sweep"), "png").unwrap();
        assert!(!stale.exists(), "超过 1 小时的 /tmp 粘贴必须被清掉");
        assert!(fresh.starts_with("/tmp/muxterm-paste-"));
        std::fs::remove_file(fresh).unwrap();
    }
}
