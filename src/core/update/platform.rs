//! 平台 → 发布资产 key 的映射。
//!
//! 清单里的 key 由发布流程写死（见 `.github/workflows/release.yml`），
//! 这里只做「当前运行平台应该拿哪一个」的纯函数映射，便于单元测试。

/// 已知的发布资产 key。
pub const MACOS_ARM64: &str = "macos-arm64";
pub const LINUX_GUI_X86_64: &str = "linux-gui-x86_64";

/// 把 (os, arch) 映射为资产 key；无法映射时返回 `None`。
///
/// 目前正式发布只有这两个桌面版本；其余平台（如 Linux ARM、Windows）暂时
/// 没有产物，返回 `None` 让 UI 显示「当前平台暂无更新包」，而不是下载错包。
pub fn asset_key(os: &str, arch: &str) -> Option<&'static str> {
    match (os, arch) {
        ("macos", "aarch64") => Some(MACOS_ARM64),
        ("linux", "x86_64") => Some(LINUX_GUI_X86_64),
        _ => None,
    }
}

/// 当前编译目标的资产 key。
pub fn current_asset_key() -> Option<&'static str> {
    asset_key(std::env::consts::OS, std::env::consts::ARCH)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_assets_map_to_their_platforms() {
        assert_eq!(asset_key("macos", "aarch64"), Some(MACOS_ARM64));
        assert_eq!(asset_key("linux", "x86_64"), Some(LINUX_GUI_X86_64));
    }

    #[test]
    fn unsupported_platforms_have_no_asset() {
        assert_eq!(asset_key("linux", "aarch64"), None);
        assert_eq!(asset_key("macos", "x86_64"), None);
        assert_eq!(asset_key("windows", "x86_64"), None);
    }
}
