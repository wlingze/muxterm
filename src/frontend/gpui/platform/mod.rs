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

/// 当前事件对应的 Colemak 锚点（不是 US QWERTY 标签）。
#[cfg(target_os = "macos")]
pub fn physical_key_for_event(expected_layout_key: &str) -> Option<&'static str> {
    macos_keys::physical_key_for_event(expected_layout_key)
}

/// 非 macOS 没有 AppKit 键码，回落到布局字符。
#[cfg(not(target_os = "macos"))]
pub fn physical_key_for_event(_expected_layout_key: &str) -> Option<&'static str> {
    None
}

/// 原生快捷键边界与菜单提示使用同一配置。
pub fn configure_shortcuts(
    map: &crate::frontend::gpui::model::shortcut_editor::ShortcutMap,
    modal: bool,
    recording: bool,
    qwerty_labels: bool,
) {
    #[cfg(target_os = "macos")]
    {
        macos_keys::configure(map, modal, recording);
        macos_menu::refresh_shortcut_keys(map, qwerty_labels);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (map, modal, recording, qwerty_labels);
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
    futures::channel::mpsc::UnboundedReceiver<
        crate::frontend::gpui::model::shortcuts::ShortcutCommand,
    >,
> {
    None
}
