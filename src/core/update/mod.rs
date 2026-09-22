//! 客户端自更新：检查发布、下载、校验与原子安装。
//!
//! 功能主体在 Core：frontend 只把「有可用更新」渲染成提醒与按钮，点击
//! 一键更新时经 FFI 交给这里完成下载与安装。前端不做 HTTP、不解析版本、
//! 不碰安装目录。
//!
//! 分层：
//! - [`manifest`]：更新清单（`latest.json`）的纯解析与版本比较
//! - [`platform`]：当前平台对应的资产 key
//! - [`install`]：下载、校验和验证、安装包落地（dmg / tar.gz）
//! - [`service`]：状态机 + 后台线程，供 FFI 轮询

pub mod install;
pub mod manifest;
pub mod platform;
pub mod service;

use std::path::PathBuf;
use std::time::Duration;

pub use install::{InstallOutcome, InstallTarget, UpdateError};
pub use manifest::{Asset, Manifest, ManifestError};
pub use platform::{asset_key, current_asset_key};
pub use service::{UpdatePhase, UpdateService, UpdateTransport};

/// 发布清单的稳定下载地址（GitHub 的 `latest` 别名总是指向最新正式版）。
pub const DEFAULT_MANIFEST_URL: &str =
    "https://github.com/wlingze/muxterm/releases/latest/download/latest.json";

/// 仓库地址；状态 JSON 里的 release / 下载链接由它拼出。
pub const REPOSITORY_URL: &str = "https://github.com/wlingze/muxterm";

/// 构建期注入的版本号（CI tag 时是 `vX.Y.Z`，本地回退 Cargo 版本）。
pub const BUILD_VERSION: &str = env!("MUXTERM_BUILD_VERSION");

/// 单次网络调用超时。检查更新是后台动作，不阻塞 UI。
pub const NETWORK_TIMEOUT: Duration = Duration::from_secs(15);

/// 更新包体积上限，防御异常清单导致的磁盘写爆。
pub const MAX_DOWNLOAD_BYTES: u64 = 512 * 1024 * 1024;

/// 下载与安装的落盘位置：`~/.cache/muxterm/updates`（或平台等价目录）。
pub fn staging_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("muxterm").join("updates")
}
