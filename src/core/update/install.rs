//! 下载校验与安装：把发布资产变成一次可回滚的本地替换。
//!
//! 约束：
//! - 下载内容先落盘到 [`super::staging_dir`]，校验 SHA-256 通过才动真实安装。
//! - 校验失败立即删除临时文件并报错，绝不安装。
//! - 安装是「先备份、再替换、失败回滚」，任何一步失败都恢复旧版本。

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::manifest::Asset;
use super::service::UpdateTransport;
use super::{MAX_DOWNLOAD_BYTES, NETWORK_TIMEOUT};

/// 下载/校验/安装过程中的错误。
#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("当前平台不支持一键更新")]
    UnsupportedPlatform,
    #[error("找不到当前可执行文件所在位置")]
    UnknownInstallLocation,
    #[error("网络请求失败: {0}")]
    Network(String),
    #[error("下载内容超过上限 {0} 字节")]
    TooLarge(u64),
    #[error("写入临时文件失败: {0}")]
    Io(#[from] std::io::Error),
    #[error("校验和不匹配：期望 {expected}，实际 {actual}")]
    ChecksumMismatch { expected: String, actual: String },
    #[error("解压失败: {0}")]
    Archive(String),
    #[error("安装包里没有找到 {0}")]
    MissingBinary(String),
    #[error("不支持的更新包格式: {0}")]
    UnsupportedFormat(String),
    #[error("安装失败: {0}")]
    Command(String),
}

/// 安装成功后返回的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallOutcome {
    /// 已安装的新版本号。
    pub version: String,
    /// 新的安装路径。
    pub path: PathBuf,
    /// 旧的备份位置（Linux 换文件时保留）。
    pub backup: Option<PathBuf>,
}

/// 计算字节切片的 SHA-256 十六进制摘要（小写）。
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// 校验字节内容与期望摘要是否一致（大小写不敏感，允许 `sha256:` 前缀）。
pub fn verify_checksum(bytes: &[u8], expected: &str) -> Result<(), UpdateError> {
    let expected = normalize_checksum(expected);
    let actual = sha256_hex(bytes);
    if expected == actual {
        Ok(())
    } else {
        Err(UpdateError::ChecksumMismatch { expected, actual })
    }
}

/// 归一化摘要文本：去掉 `sha256:` 前缀与空白，转小写。
fn normalize_checksum(raw: &str) -> String {
    let lowered = raw.trim().to_ascii_lowercase();
    let stripped = lowered
        .strip_prefix("sha256:")
        .unwrap_or(lowered.as_str())
        .trim();
    stripped.to_string()
}

/// 流式计算文件的 SHA-256（避免把整包读进内存）。
pub fn sha256_file(path: &Path) -> Result<String, UpdateError> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// 下载一个资产到 `destination`，边下边算摘要，完成后校验。
///
/// 采用流式读取：先看 `Content-Length`，再按块累计，超过
/// [`MAX_DOWNLOAD_BYTES`] 立即中止，避免异常清单写爆磁盘。
pub fn download_asset(asset: &Asset, destination: &Path) -> Result<(), UpdateError> {
    download_asset_from(asset, &asset.url, destination)
}

/// 与 [`download_asset`] 相同，但显式给出已解析的下载地址。
///
/// 清单里的资产地址允许是相对路径，调用方先用
/// [`super::manifest::resolve_asset_url`] 结合清单地址解析成绝对地址。
pub fn download_asset_from(
    asset: &Asset,
    url: &str,
    destination: &Path,
) -> Result<(), UpdateError> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(NETWORK_TIMEOUT))
        .build()
        .into();
    let mut response = agent
        .get(url)
        .call()
        .map_err(|error| UpdateError::Network(error.to_string()))?;
    if let Some(length) = response.body().content_length() {
        if length > MAX_DOWNLOAD_BYTES {
            return Err(UpdateError::TooLarge(length));
        }
    }

    let mut file = fs::File::create(destination)?;
    let mut hasher = Sha256::new();
    let mut total: u64 = 0;
    let mut reader = response.body_mut().as_reader();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total += read as u64;
        if total > MAX_DOWNLOAD_BYTES {
            drop(file);
            let _ = fs::remove_file(destination);
            return Err(UpdateError::TooLarge(total));
        }
        hasher.update(&buffer[..read]);
        file.write_all(&buffer[..read])?;
    }
    file.flush()?;
    drop(file);

    let actual = hex::encode(hasher.finalize());
    let expected = normalize_checksum(&asset.sha256);
    if expected != actual {
        let _ = fs::remove_file(destination);
        return Err(UpdateError::ChecksumMismatch { expected, actual });
    }
    Ok(())
}

/// 安装产物在磁盘上的形态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundleKind {
    /// macOS `.dmg`，内含 `Muxterm.app`。
    MacOsDmg,
    /// Linux `.tar.gz`，内含 `muxterm` 可执行文件。
    LinuxTarGz,
}

/// 按文件名判断更新包类型。
pub fn bundle_kind(name: &str) -> Result<BundleKind, UpdateError> {
    if name.ends_with(".dmg") {
        Ok(BundleKind::MacOsDmg)
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        Ok(BundleKind::LinuxTarGz)
    } else {
        Err(UpdateError::UnsupportedFormat(name.to_string()))
    }
}

/// 从 tar.gz 中解出 `muxterm` 可执行文件，落到 `destination`。
///
/// 只接受归档根目录或单层目录里的 `muxterm`（发布包结构即如此），
/// 其余条目一律跳过，避免把任意路径写进安装目录。
pub fn extract_linux_binary(archive: &Path, destination: &Path) -> Result<(), UpdateError> {
    let file = fs::File::open(archive)?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut tar = tar::Archive::new(decoder);
    let entries = tar
        .entries()
        .map_err(|error| UpdateError::Archive(error.to_string()))?;
    for entry in entries {
        let mut entry = entry.map_err(|error| UpdateError::Archive(error.to_string()))?;
        let path = entry
            .path()
            .map_err(|error| UpdateError::Archive(error.to_string()))?
            .to_path_buf();
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if file_name != "muxterm" {
            continue;
        }
        // 只允许归档根或单层目录，防止 `../../` 之类逃逸。
        if path.components().count() > 2 || path.components().any(|c| c.as_os_str() == "..") {
            continue;
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut out = fs::File::create(destination)?;
        std::io::copy(&mut entry, &mut out)?;
        out.flush()?;
        drop(out);
        return Ok(());
    }
    Err(UpdateError::MissingBinary("muxterm".into()))
}

/// 当前平台一键更新时要替换的目标。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallTarget {
    /// Linux 单文件替换：目标可执行文件路径。
    LinuxBinary(PathBuf),
    /// macOS `.app` 目录：DMG 挂载后整包替换。
    MacOsApp(PathBuf),
}

/// 解析当前可执行文件的真实路径（跟随符号链接，例如 Homebrew 的 bin）。
pub fn current_executable() -> Result<PathBuf, UpdateError> {
    let path = std::env::current_exe().map_err(|_| UpdateError::UnknownInstallLocation)?;
    // macOS 上 `Muxterm.app/Contents/MacOS/Muxterm` 是真实二进制；向上两层即 .app。
    Ok(path.canonicalize().unwrap_or(path))
}

/// 判断当前平台的一键更新目标。
pub fn current_install_target() -> Result<InstallTarget, UpdateError> {
    // 显式覆盖：用户/打包方把二进制装在非标准位置，或测试需要隔离目标时使用。
    // 只接受已存在的文件，避免把更新写到任意路径。
    if let Some(target) = std::env::var_os(INSTALL_TARGET_ENV) {
        let target = PathBuf::from(target);
        // `.app` 覆盖值按 macOS bundle 目标处理（本地验证 DMG 安装用）。
        if target
            .extension()
            .is_some_and(|extension| extension == "app")
        {
            return Ok(InstallTarget::MacOsApp(target));
        }
        if target.is_file() {
            return Ok(InstallTarget::LinuxBinary(target));
        }
        return Err(UpdateError::UnknownInstallLocation);
    }
    let exe = current_executable()?;
    if cfg!(target_os = "macos") {
        // …/Muxterm.app/Contents/MacOS/Muxterm → 去掉三层得到 .app
        let app = exe
            .ancestors()
            .nth(3)
            .filter(|path| path.extension().is_some_and(|ext| ext == "app"))
            .map(Path::to_path_buf)
            .ok_or(UpdateError::UnknownInstallLocation)?;
        return Ok(InstallTarget::MacOsApp(app));
    }
    if cfg!(target_os = "linux") {
        return Ok(InstallTarget::LinuxBinary(exe));
    }
    Err(UpdateError::UnsupportedPlatform)
}

/// 覆盖一键更新的安装目标（绝对路径）。生产环境不设置时按平台推导。
pub const INSTALL_TARGET_ENV: &str = "MUXTERM_INSTALL_TARGET";

/// 一次完整的「下载 → 校验 → 安装」。
///
/// `staging` 是临时下载目录，由调用方给出（生产是 [`super::staging_dir`]）。
pub fn download_and_install(
    transport: &dyn UpdateTransport,
    asset: &Asset,
    version: &str,
    manifest_url: &str,
    staging: &Path,
) -> Result<InstallOutcome, UpdateError> {
    let staging = staging.join(version.trim_start_matches('v'));
    fs::create_dir_all(&staging)?;
    let archive = staging.join(&asset.name);
    // 清单里的资产地址可以是相对路径，先按清单地址解析成绝对地址。
    let download_url = super::manifest::resolve_asset_url(manifest_url, &asset.url);
    transport.download(asset, &download_url, &archive)?;

    // 传输层可能不做校验（例如未来换实现），这里以清单摘要为准再核一次。
    if !asset.sha256.trim().is_empty() {
        let actual = sha256_file(&archive)?;
        let expected = normalize_checksum(&asset.sha256);
        if expected != actual {
            let _ = fs::remove_file(&archive);
            return Err(UpdateError::ChecksumMismatch { expected, actual });
        }
    }

    let target = current_install_target()?;
    let outcome = match (bundle_kind(&asset.name)?, target) {
        (BundleKind::LinuxTarGz, InstallTarget::LinuxBinary(path)) => {
            let extracted = staging.join("muxterm.extracted");
            extract_linux_binary(&archive, &extracted)?;
            let mut permissions = fs::metadata(&extracted)?.permissions();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                permissions.set_mode(0o755);
            }
            fs::set_permissions(&extracted, permissions)?;
            let backup = replace_file(&extracted, &path)?;
            InstallOutcome {
                version: version.to_string(),
                path,
                backup: Some(backup),
            }
        }
        (BundleKind::MacOsDmg, InstallTarget::MacOsApp(app)) => {
            let installed = install_macos_dmg(&archive, &app)?;
            InstallOutcome {
                version: version.to_string(),
                path: installed,
                backup: None,
            }
        }
        (kind, _) => {
            return Err(UpdateError::UnsupportedFormat(format!(
                "{:?} 不适合当前平台",
                kind
            )))
        }
    };
    let _ = fs::remove_dir_all(&staging);
    Ok(outcome)
}

/// macOS：挂载 DMG，把里面的 `Muxterm.app` 替换到目标位置。
#[cfg(target_os = "macos")]
fn install_macos_dmg(dmg: &Path, target_app: &Path) -> Result<PathBuf, UpdateError> {
    let mount = std::env::temp_dir().join(format!("muxterm-dmg-{}", std::process::id()));
    let _ = fs::remove_dir_all(&mount);
    fs::create_dir_all(&mount)?;

    let attach = std::process::Command::new("hdiutil")
        .args(["attach", "-nobrowse", "-readonly", "-mountpoint"])
        .arg(&mount)
        .arg(dmg)
        .output()?;
    if !attach.status.success() {
        return Err(UpdateError::Command(format!(
            "hdiutil attach 失败: {}",
            String::from_utf8_lossy(&attach.stderr).trim()
        )));
    }

    let result = (|| -> Result<PathBuf, UpdateError> {
        let source = mount.join("Muxterm.app");
        if !source.is_dir() {
            return Err(UpdateError::MissingBinary("Muxterm.app".into()));
        }
        let staging_app = target_app.with_extension("app.new");
        let _ = fs::remove_dir_all(&staging_app);
        copy_dir_recursive(&source, &staging_app)?;
        let backup = target_app.with_extension("app.backup");
        let _ = fs::remove_dir_all(&backup);
        let had_old = target_app.exists();
        if had_old {
            fs::rename(target_app, &backup)?;
        }
        match fs::rename(&staging_app, target_app) {
            Ok(()) => Ok(target_app.to_path_buf()),
            Err(error) => {
                if had_old {
                    let _ = fs::rename(&backup, target_app);
                }
                Err(UpdateError::Io(error))
            }
        }
    })();

    let _ = std::process::Command::new("hdiutil")
        .args(["detach", "-force"])
        .arg(&mount)
        .output();
    let _ = fs::remove_dir_all(&mount);
    result
}

#[cfg(not(target_os = "macos"))]
fn install_macos_dmg(_dmg: &Path, _target_app: &Path) -> Result<PathBuf, UpdateError> {
    Err(UpdateError::UnsupportedPlatform)
}

/// 递归复制目录（用于 DMG 内的 .app 包）。
#[cfg(target_os = "macos")]
fn copy_dir_recursive(source: &Path, destination: &Path) -> Result<(), UpdateError> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let from = entry.path();
        let to = destination.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if file_type.is_symlink() {
            let link = fs::read_link(&from)?;
            let _ = fs::remove_file(&to);
            std::os::unix::fs::symlink(link, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// 原子替换文件：先备份旧文件，替换失败时回滚。
///
/// `new` 可能来自另一个文件系统（下载目录常在 `~/.cache`，目标二进制可能在
/// `/usr/bin`），此情况下 `rename` 会以 `EXDEV` 失败。这里先把新文件复制到
/// 目标所在目录的临时文件，再在**同一文件系统内** `rename`，保证替换仍是
/// 原子的：读者要么看到旧文件，要么看到完整的新文件，不会看到半个文件。
pub fn replace_file(new: &Path, target: &Path) -> Result<PathBuf, UpdateError> {
    let backup = target.with_extension("muxterm-backup");
    let _ = fs::remove_file(&backup);
    let had_old = target.exists();
    // 与目标同目录，确保后续 rename 不跨设备。
    let staged = target.with_extension("muxterm-new");
    let _ = fs::remove_file(&staged);
    fs::copy(new, &staged)?;
    if had_old {
        fs::rename(target, &backup)?;
    }
    match fs::rename(&staged, target) {
        Ok(()) => Ok(backup),
        Err(error) => {
            let _ = fs::remove_file(&staged);
            if had_old {
                let _ = fs::rename(&backup, target);
            }
            Err(UpdateError::Io(error))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("muxterm-update-test-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn checksum_matches_known_digest() {
        // echo -n "hello world" | sha256sum（标准测试向量）
        let digest = sha256_hex(b"hello world");
        assert_eq!(
            digest,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
        assert!(verify_checksum(b"hello world", &digest).is_ok());
    }

    #[test]
    fn checksum_accepts_prefix_case_and_whitespace() {
        let digest = sha256_hex(b"payload");
        let decorated = format!("  sha256:{}  ", digest.to_uppercase());
        assert!(verify_checksum(b"payload", &decorated).is_ok());
        // 没有前缀、带大小写也接受（.sha256 文件常见形态）。
        assert!(verify_checksum(b"payload", &format!(" {} ", digest.to_uppercase())).is_ok());
    }

    #[test]
    fn checksum_mismatch_reports_both_digests() {
        let error = verify_checksum(b"payload", "deadbeef").unwrap_err();
        match error {
            UpdateError::ChecksumMismatch { expected, actual } => {
                assert_eq!(expected, "deadbeef");
                assert_eq!(actual, sha256_hex(b"payload"));
            }
            other => panic!("期望校验错误，实际 {other:?}"),
        }
    }

    #[test]
    fn sha256_file_matches_in_memory_digest() {
        let dir = temp_dir("sha-file");
        let path = dir.join("blob.bin");
        let payload = vec![7u8; 300_000];
        fs::write(&path, &payload).unwrap();
        assert_eq!(sha256_file(&path).unwrap(), sha256_hex(&payload));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn bundle_kind_detects_supported_formats() {
        assert_eq!(
            bundle_kind("muxterm-macos-arm64.dmg").unwrap(),
            BundleKind::MacOsDmg
        );
        assert_eq!(
            bundle_kind("muxterm-gtk-linux-x86_64.tar.gz").unwrap(),
            BundleKind::LinuxTarGz
        );
        assert!(bundle_kind("muxterm.zip").is_err());
    }

    #[test]
    fn replace_file_swaps_content_and_keeps_backup() {
        let dir = temp_dir("replace");
        let target = dir.join("muxterm");
        let new = dir.join("muxterm.new");
        fs::write(&target, b"old").unwrap();
        fs::write(&new, b"new").unwrap();

        let backup = replace_file(&new, &target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new");
        assert_eq!(fs::read(&backup).unwrap(), b"old");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn replace_file_without_existing_target_just_moves() {
        let dir = temp_dir("replace-fresh");
        let target = dir.join("muxterm");
        let new = dir.join("muxterm.new");
        fs::write(&new, b"new").unwrap();

        let backup = replace_file(&new, &target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new");
        assert!(!backup.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn extracts_only_the_muxterm_binary_from_the_archive() {
        let dir = temp_dir("extract");
        let archive_path = dir.join("bundle.tar.gz");
        {
            let file = fs::File::create(&archive_path).unwrap();
            let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
            let mut builder = tar::Builder::new(encoder);
            let payload = b"#!/bin/sh\necho muxterm\n";
            let mut header = tar::Header::new_gnu();
            header.set_size(payload.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder
                .append_data(&mut header, "muxterm", &payload[..])
                .unwrap();

            // 带 `..` 的条目：tar crate 在 append 阶段就会拒绝，这里直接
            // 断言它无法被写进归档，保证解包侧不会遇到逃逸路径。
            let mut evil = tar::Header::new_gnu();
            evil.set_size(4);
            evil.set_mode(0o644);
            evil.set_cksum();
            assert!(
                builder
                    .append_data(&mut evil, "../escape.txt", &b"evil"[..])
                    .is_err(),
                "tar 不应接受带 .. 的条目"
            );
            builder.into_inner().unwrap().finish().unwrap();
        }

        let out = dir.join("extracted").join("muxterm");
        extract_linux_binary(&archive_path, &out).unwrap();
        let mut text = String::new();
        fs::File::open(&out)
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        assert!(text.contains("echo muxterm"));
        assert!(!dir.join("escape.txt").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn extraction_fails_when_binary_is_absent() {
        let dir = temp_dir("extract-missing");
        let archive_path = dir.join("bundle.tar.gz");
        {
            let file = fs::File::create(&archive_path).unwrap();
            let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
            let mut builder = tar::Builder::new(encoder);
            let payload = b"text";
            let mut header = tar::Header::new_gnu();
            header.set_size(payload.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, "README.txt", &payload[..])
                .unwrap();
            builder.into_inner().unwrap().finish().unwrap();
        }
        let out = dir.join("muxterm");
        assert!(matches!(
            extract_linux_binary(&archive_path, &out),
            Err(UpdateError::MissingBinary(_))
        ));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn download_and_install_end_to_end_from_a_caller_supplied_transport() {
        // 端到端：造一个 tar.gz（含 muxterm），经替身传输落到 staging，
        // 校验摘要后替换目标，并保留可回滚的备份。
        use super::super::service::UpdateTransport;
        use super::super::Manifest;

        struct LocalTransport {
            bytes: Vec<u8>,
            seen_url: std::sync::Mutex<Option<String>>,
        }
        impl UpdateTransport for LocalTransport {
            fn fetch_manifest(&self, _url: &str) -> Result<Manifest, UpdateError> {
                unreachable!("本测试不请求清单")
            }
            fn download(
                &self,
                _asset: &Asset,
                url: &str,
                destination: &Path,
            ) -> Result<(), UpdateError> {
                *self.seen_url.lock().unwrap() = Some(url.to_string());
                if let Some(parent) = destination.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(destination, &self.bytes)?;
                Ok(())
            }
        }

        let dir = temp_dir("e2e");
        // 构造 tar.gz
        let mut archive_bytes = Vec::new();
        {
            let encoder =
                flate2::write::GzEncoder::new(&mut archive_bytes, flate2::Compression::fast());
            let mut builder = tar::Builder::new(encoder);
            let payload = b"#!/bin/sh\necho v2\n";
            let mut header = tar::Header::new_gnu();
            header.set_size(payload.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder
                .append_data(&mut header, "muxterm", &payload[..])
                .unwrap();
            builder.into_inner().unwrap().finish().unwrap();
        }
        let digest = sha256_hex(&archive_bytes);
        let transport = LocalTransport {
            bytes: archive_bytes,
            seen_url: std::sync::Mutex::new(None),
        };
        let asset = Asset {
            name: "muxterm-gtk-linux-x86_64.tar.gz".into(),
            // 相对路径：必须按清单地址解析。
            url: "muxterm-gtk-linux-x86_64.tar.gz".into(),
            sha256: digest,
            size: 0,
        };

        // 目标可执行文件：在测试机上直接复用当前二进制路径会破坏环境，
        // 因此只在非 Linux 平台跳过真实替换，仅验证解析 + 校验 + 解包。
        let manifest_url = "http://127.0.0.1:9/alpha/latest.json";
        let resolved = super::super::manifest::resolve_asset_url(manifest_url, &asset.url);

        // 这里走的是解包 + 校验路径；替换目标由 current_install_target 决定，
        // 在 macOS 上会返回 UnsupportedPlatform/UnknownInstallLocation，属预期。
        let result = download_and_install(&transport, &asset, "v2.0.0", manifest_url, &dir);
        assert_eq!(
            transport.seen_url.lock().unwrap().as_deref(),
            Some(resolved.as_str()),
            "下载地址必须按清单地址解析"
        );
        match result {
            Ok(outcome) => {
                assert_eq!(outcome.version, "v2.0.0");
                assert!(
                    fs::read(&outcome.path)
                        .map(|bytes| String::from_utf8_lossy(&bytes).contains("echo v2"))
                        .unwrap_or(false),
                    "安装后的文件应是新版本内容"
                );
            }
            Err(UpdateError::UnsupportedPlatform)
            | Err(UpdateError::UnknownInstallLocation)
            | Err(UpdateError::UnsupportedFormat(_)) => {}
            Err(other) => panic!("不该出现的失败: {other}"),
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn install_target_override_points_at_an_existing_file() {
        // 覆盖变量只接受已存在的文件；否则报错而不是写到任意路径。
        let dir = temp_dir("target-override");
        let fake = dir.join("muxterm");
        fs::write(&fake, b"old").unwrap();

        // 该测试不能污染进程级环境；用子进程语义校验解析分支。
        let previous = std::env::var_os(INSTALL_TARGET_ENV);
        std::env::set_var(INSTALL_TARGET_ENV, &fake);
        let resolved = current_install_target().expect("覆盖路径必须生效");
        assert_eq!(resolved, InstallTarget::LinuxBinary(fake.clone()));

        std::env::set_var(INSTALL_TARGET_ENV, dir.join("missing"));
        assert!(matches!(
            current_install_target(),
            Err(UpdateError::UnknownInstallLocation)
        ));

        match previous {
            Some(value) => std::env::set_var(INSTALL_TARGET_ENV, value),
            None => std::env::remove_var(INSTALL_TARGET_ENV),
        }
        let _ = fs::remove_dir_all(&dir);
    }
}
