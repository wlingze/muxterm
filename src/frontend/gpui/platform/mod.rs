//! 平台相关的接线。纯逻辑一律放在 `model/`，这里只做「gpui ↔ 操作系统」。
//!
//! 目前有 macOS 原生菜单栏与剪贴板图片转码；Linux / Windows 的自绘菜单条
//! 还没做，见 `docs/FRONTEND-GPUI.md` §8.1 的 TODO。

pub mod clipboard_image;

// objc 的 `msg_send!` / `class!` 宏内部会引用 `cargo-clippy` 等 cfg，
// 在 2021 edition 下触发 `unexpected_cfgs`。这是依赖宏的噪音，与本 crate 无关。
#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)]
pub mod macos_menu;

#[cfg(target_os = "macos")]
pub use macos_menu::{install_menu, refresh_menu};

/// 非 macOS 没有原生菜单，刷新是空操作。
#[cfg(not(target_os = "macos"))]
pub fn refresh_menu(_state: crate::frontend::gpui::model::menu::MenuState) -> bool {
    false
}

/// 安装窗口菜单栏，返回菜单动作的接收端。
///
/// 非 macOS 平台先返回 `None`：没有原生菜单栏，自绘菜单条还没实现。
#[cfg(not(target_os = "macos"))]
pub fn install_menu() -> Option<
    futures::channel::mpsc::UnboundedReceiver<crate::frontend::gpui::model::shortcuts::KeyAction>,
> {
    None
}
