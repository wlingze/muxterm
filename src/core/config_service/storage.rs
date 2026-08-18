//! Versioned atomic storage for the configuration document.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConfigRevision(pub String);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigSnapshot {
    pub revision: ConfigRevision,
    /// Values explicitly present in the TOML document. Defaults are omitted.
    pub raw: Value,
    /// Fully resolved values after serde defaults and legacy normalization.
    pub values: Value,
    pub defaults: Value,
    pub schema: Value,
    pub manifest: Value,
    pub action_catalog: Value,
}

/// Update known keys in a TOML document while retaining comments, formatting
/// and unknown extension tables from the user's original file.
pub fn preserve_toml_metadata(original: &str, serialized: &str) -> Result<String> {
    let mut original_doc: toml_edit::DocumentMut =
        original.parse().context("原配置 TOML 不是合法文档")?;
    let next_doc: toml_edit::DocumentMut =
        serialized.parse().context("生成的配置 TOML 不是合法文档")?;
    merge_toml_table(original_doc.as_table_mut(), next_doc.as_table());
    Ok(original_doc.to_string())
}

fn merge_toml_table(dst: &mut toml_edit::Table, src: &toml_edit::Table) {
    for (key, src_item) in src.iter() {
        if let Some(dst_item) = dst.get_mut(key) {
            if let (Some(dst_table), Some(src_table)) =
                (dst_item.as_table_mut(), src_item.as_table())
            {
                merge_toml_table(dst_table, src_table);
            } else {
                *dst_item = src_item.clone();
            }
        } else {
            dst.insert(key, src_item.clone());
        }
    }
}

pub fn revision_for(raw: &str) -> ConfigRevision {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in raw.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    ConfigRevision(format!("{hash:016x}"))
}

pub fn atomic_write(path: &Path, raw: &str) -> Result<()> {
    let parent = path.parent().ok_or_else(|| anyhow!("配置路径没有父目录"))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("创建配置目录失败: {}", parent.display()))?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("config"),
        stamp
    ));
    let existing_mode = fs::metadata(path).ok().map(|metadata| metadata.permissions());
    let mut file =
        fs::File::create(&temp).with_context(|| format!("写入临时配置失败: {}", temp.display()))?;
    file.write_all(raw.as_bytes())
        .with_context(|| format!("写入临时配置失败: {}", temp.display()))?;
    file.sync_all()
        .with_context(|| format!("同步临时配置失败: {}", temp.display()))?;
    if let Some(permissions) = existing_mode {
        fs::set_permissions(&temp, permissions)
            .with_context(|| format!("保留配置权限失败: {}", temp.display()))?;
    }
    if let Err(error) = fs::rename(&temp, path) {
        let _ = fs::remove_file(&temp);
        return Err(error).with_context(|| format!("原子替换配置失败: {}", path.display()));
    }
    if let Ok(parent_file) = fs::File::open(parent) {
        let _ = parent_file.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("muxterm-config-storage-{name}-{}", std::process::id()))
    }

    #[test]
    fn revision_changes_when_content_changes() {
        assert_ne!(revision_for("a = 1"), revision_for("a = 2"));
        assert_eq!(revision_for("a = 1"), revision_for("a = 1"));
    }

    #[test]
    fn atomic_write_creates_file_and_replaces_content() {
        let path = temp_path("write");
        atomic_write(&path, "config_version = 1\n").unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "config_version = 1\n"
        );
        atomic_write(&path, "config_version = 2\n").unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "config_version = 2\n"
        );
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn preserve_toml_metadata_keeps_comments_and_unknown_extensions() {
        let original = "# keep me\n[font]\nfamily = \"Monospace\"\n\n[extensions.vendor]\nflag = true\n";
        let serialized = "[font]\nfamily = \"JetBrains Mono\"\n";
        let merged = preserve_toml_metadata(original, serialized).unwrap();
        assert!(merged.contains("# keep me"));
        assert!(merged.contains("JetBrains Mono"));
        assert!(merged.contains("[extensions.vendor]"));
        assert!(merged.contains("flag = true"));
    }
}
