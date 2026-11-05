//! 平台相关的接线。纯逻辑一律放在 `model/`，这里只做「gpui ↔ 操作系统」。
//!
//! 目前只有 macOS 原生菜单栏；Linux / Windows 的自绘菜单条还没做，见
//! `docs/FRONTEND-GPUI.md` §8.1 的 TODO。

#[cfg(target_os = "macos")]
pub mod macos_menu;

#[cfg(target_os = "macos")]
pub use macos_menu::install_menu;

/// 安装窗口菜单栏，返回菜单动作的接收端。
///
/// 非 macOS 平台先返回 `None`：没有原生菜单栏，自绘菜单条还没实现。
#[cfg(not(target_os = "macos"))]
pub fn install_menu() -> Option<
    futures::channel::mpsc::UnboundedReceiver<crate::frontend::gpui::model::shortcuts::KeyAction>,
> {
    None
}
