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

#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)]
pub mod macos_keys;

/// 安装物理键位记录器（macOS）。其它平台是空操作。
#[cfg(target_os = "macos")]
pub fn install_physical_key_tracker() {
    macos_keys::install();
}

/// 非 macOS：没有需要安装的东西。
#[cfg(not(target_os = "macos"))]
pub fn install_physical_key_tracker() {}

/// 当前这条按键的**物理键位**（US QWERTY 字符）。
///
/// 见 `model/physical_key.rs` 与 `platform/macos_keys.rs`：Colemak 这类布局下
/// `Keystroke.key` 给的是布局字符，直接按它匹配会把 Cmd-D 路由成纵向分屏、
/// 并把 Cmd-T / Cmd-S 丢进黑洞。
#[cfg(target_os = "macos")]
pub fn physical_key_for_event(expected_layout_key: &str) -> Option<&'static str> {
    macos_keys::physical_key_for_event(expected_layout_key)
}

/// 非 macOS 没有 AppKit 键码，回落到布局字符。
#[cfg(not(target_os = "macos"))]
pub fn physical_key_for_event(_expected_layout_key: &str) -> Option<&'static str> {
    None
}

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
