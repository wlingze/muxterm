//! `latest.json` 的纯解析与版本比较。
//!
//! 输入是发布流程产出的 JSON 文本，输出是强类型 [`Manifest`]。这里不碰
//! 网络与文件系统，便于单元测试覆盖格式漂移与恶意清单。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// 单个平台的发布资产。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Asset {
    /// 资产文件名（不含路径）。
    pub name: String,
    /// 直接下载地址。
    pub url: String,
    /// 期望的 SHA-256 十六进制摘要（小写）。
    pub sha256: String,
    /// 期望的字节数；用于下载前的合理性检查。
    #[serde(default)]
    pub size: u64,
}

/// 更新清单：一次发布里全部平台的资产 + 版本号。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    /// 版本号，通常形如 `v1.2.3`。
    pub version: String,
    /// 对应的 git tag；缺省时回退到 `version`。
    #[serde(default)]
    pub tag: Option<String>,
    /// 是否为预发布；正式版为 false。
    #[serde(default)]
    pub prerelease: bool,
    /// 发布对应的源码提交。
    #[serde(default)]
    pub source_sha: Option<String>,
    /// 发布时间（ISO-8601，UTC）。
    #[serde(default)]
    pub published_at: Option<String>,
    /// 资产 key → 资产。key 见 [`super::platform`]。
    #[serde(default)]
    pub assets: BTreeMap<String, Asset>,
}

/// 清单解析失败的原因。
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ManifestError {
    #[error("更新清单不是合法 JSON: {0}")]
    Json(String),
    #[error("更新清单缺少 version")]
    MissingVersion,
    #[error("更新清单里没有当前平台的资产: {0}")]
    MissingAsset(String),
    #[error("资产 {key} 的 sha256 字段为空")]
    MissingChecksum { key: String },
    #[error("资产 {key} 的下载地址为空")]
    MissingUrl { key: String },
}

impl Manifest {
    /// 从 JSON 文本解析清单并校验关键字段。
    pub fn parse(raw: &str) -> Result<Self, ManifestError> {
        let manifest: Self =
            serde_json::from_str(raw).map_err(|error| ManifestError::Json(error.to_string()))?;
        if manifest.version.trim().is_empty() {
            return Err(ManifestError::MissingVersion);
        }
        Ok(manifest)
    }

    /// 取指定平台资产的引用，并校验下载所需的字段齐备。
    pub fn asset(&self, key: &str) -> Result<&Asset, ManifestError> {
        let asset = self
            .assets
            .get(key)
            .ok_or_else(|| ManifestError::MissingAsset(key.to_string()))?;
        if asset.url.trim().is_empty() {
            return Err(ManifestError::MissingUrl {
                key: key.to_string(),
            });
        }
        if asset.sha256.trim().is_empty() {
            return Err(ManifestError::MissingChecksum {
                key: key.to_string(),
            });
        }
        Ok(asset)
    }

    /// 清单里的版本是否比 `current` 更新。
    ///
    /// 版本号允许带 `v` 前缀和预发布后缀（`v1.2.3-dev.abcd`）；按 semver
    /// 语义比较，无法解析时保守返回 false（宁可漏一次提醒，也不误报）。
    pub fn is_newer_than(&self, current: &str) -> bool {
        match (parse_version(&self.version), parse_version(current)) {
            (Some(latest), Some(current)) => latest > current,
            _ => false,
        }
    }
}

/// 把清单里的资产地址解析成可直接下载的绝对地址。
///
/// 清单里的 `url` 允许是相对路径（发布流程故意如此）：
/// - 正式版从 `.../releases/latest/download/latest.json` 解析 → 同目录资产；
/// - 测试版从某个 release 的 `latest.json` 解析 → 该 release 的资产；
/// - alpha 产物下载到本地后用 `python3 -m http.server` 起服务，也能解析到
///   localhost，无需为每个通道写死不同地址。
pub fn resolve_asset_url(manifest_url: &str, asset_url: &str) -> String {
    let asset_url = asset_url.trim();
    if asset_url.starts_with("http://") || asset_url.starts_with("https://") {
        return asset_url.to_string();
    }
    // 以清单地址所在目录为基准拼接；清单地址自身必须是绝对 URL。
    match manifest_url.rsplit_once('/') {
        Some((base, _)) if base.starts_with("http") => {
            format!("{base}/{}", asset_url.trim_start_matches('/'))
        }
        _ => asset_url.to_string(),
    }
}

/// 把 `v1.2.3` / `1.2.3-dev.abcdef` 解析为 semver。
fn parse_version(raw: &str) -> Option<semver::Version> {
    let trimmed = raw.trim().trim_start_matches('v');
    semver::Version::parse(trimmed).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "version": "v1.4.0",
      "tag": "v1.4.0",
      "prerelease": false,
      "source_sha": "abc123",
      "published_at": "2026-09-22T00:00:00Z",
      "assets": {
        "macos-arm64": {
          "name": "muxterm-macos-arm64.dmg",
          "url": "https://example.invalid/muxterm-macos-arm64.dmg",
          "sha256": "aa11",
          "size": 123
        },
        "linux-gui-x86_64": {
          "name": "muxterm-gtk-linux-x86_64.tar.gz",
          "url": "https://example.invalid/muxterm-gtk-linux-x86_64.tar.gz",
          "sha256": "bb22",
          "size": 456
        }
      }
    }"#;

    #[test]
    fn parses_release_manifest() {
        let manifest = Manifest::parse(SAMPLE).expect("样例清单必须可解析");
        assert_eq!(manifest.version, "v1.4.0");
        assert!(!manifest.prerelease);
        assert_eq!(manifest.assets.len(), 2);
        let asset = manifest.asset("macos-arm64").expect("macOS 资产必须存在");
        assert_eq!(asset.name, "muxterm-macos-arm64.dmg");
        assert_eq!(asset.size, 123);
    }

    #[test]
    fn rejects_malformed_json_and_missing_version() {
        assert!(matches!(
            Manifest::parse("{not json"),
            Err(ManifestError::Json(_))
        ));
        assert_eq!(
            Manifest::parse(r#"{"version":"   "}"#),
            Err(ManifestError::MissingVersion)
        );
    }

    #[test]
    fn reports_missing_platform_asset() {
        let manifest = Manifest::parse(SAMPLE).unwrap();
        assert_eq!(
            manifest.asset("linux-gui-aarch64"),
            Err(ManifestError::MissingAsset("linux-gui-aarch64".into()))
        );
    }

    #[test]
    fn rejects_asset_without_checksum_or_url() {
        let raw = r#"{"version":"v1.0.0","assets":{"macos-arm64":{"name":"a.dmg","url":"","sha256":"aa"}}}"#;
        let manifest = Manifest::parse(raw).unwrap();
        assert_eq!(
            manifest.asset("macos-arm64"),
            Err(ManifestError::MissingUrl {
                key: "macos-arm64".into()
            })
        );

        let raw = r#"{"version":"v1.0.0","assets":{"macos-arm64":{"name":"a.dmg","url":"https://x","sha256":""}}}"#;
        let manifest = Manifest::parse(raw).unwrap();
        assert_eq!(
            manifest.asset("macos-arm64"),
            Err(ManifestError::MissingChecksum {
                key: "macos-arm64".into()
            })
        );
    }

    #[test]
    fn compares_versions_with_and_without_v_prefix() {
        let manifest = Manifest::parse(SAMPLE).unwrap();
        assert!(manifest.is_newer_than("v1.3.9"));
        assert!(manifest.is_newer_than("1.3.9"));
        assert!(!manifest.is_newer_than("v1.4.0"));
        assert!(!manifest.is_newer_than("v1.4.1"));
    }

    #[test]
    fn prerelease_ordering_follows_semver() {
        // 2.0.0 正式版比 2.0.0-rc.1 新（semver 规则）。
        let final_release = Manifest::parse(r#"{"version":"v2.0.0","assets":{}}"#).unwrap();
        assert!(final_release.is_newer_than("v2.0.0-rc.1"));

        // 2.0.0-rc.2 比 2.0.0-rc.1 新，但比 2.0.0 旧。
        let rc2 = Manifest::parse(r#"{"version":"v2.0.0-rc.2","assets":{}}"#).unwrap();
        assert!(rc2.is_newer_than("v2.0.0-rc.1"));
        assert!(!rc2.is_newer_than("v2.0.0"));

        // 更高补丁号自然更新。
        let patch = Manifest::parse(r#"{"version":"v2.0.1","assets":{}}"#).unwrap();
        assert!(patch.is_newer_than("v2.0.0-rc.1"));
    }

    #[test]
    fn unparseable_versions_never_claim_an_update() {
        let raw = r#"{"version":"nightly","assets":{}}"#;
        let manifest = Manifest::parse(raw).unwrap();
        assert!(!manifest.is_newer_than("v1.0.0"));
        assert!(!Manifest::parse(SAMPLE)
            .unwrap()
            .is_newer_than("not-a-version"));
    }

    #[test]
    fn relative_asset_urls_resolve_against_the_manifest_location() {
        let stable = "https://github.com/o/r/releases/latest/download/latest.json";
        assert_eq!(
            resolve_asset_url(stable, "muxterm-macos-arm64.dmg"),
            "https://github.com/o/r/releases/latest/download/muxterm-macos-arm64.dmg"
        );
        let beta = "https://github.com/o/r/releases/download/v1.2.3-beta.1/latest.json";
        assert_eq!(
            resolve_asset_url(beta, "/muxterm-gtk-linux-x86_64.tar.gz"),
            "https://github.com/o/r/releases/download/v1.2.3-beta.1/muxterm-gtk-linux-x86_64.tar.gz"
        );
        let local = "http://127.0.0.1:8000/alpha/latest.json";
        assert_eq!(
            resolve_asset_url(local, "muxterm-macos-arm64.dmg"),
            "http://127.0.0.1:8000/alpha/muxterm-macos-arm64.dmg"
        );
    }

    #[test]
    fn absolute_asset_urls_are_left_untouched() {
        let manifest = "https://example.invalid/latest.json";
        assert_eq!(
            resolve_asset_url(manifest, "https://cdn.example/x.dmg"),
            "https://cdn.example/x.dmg"
        );
    }
}
