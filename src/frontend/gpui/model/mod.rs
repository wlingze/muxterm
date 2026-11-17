//! 从 macOS `src/frontend/macos/Chrome/` 移植的纯模型规则。
//!
//! 这些模块不依赖 gpui、不依赖 AppKit、不持有状态：输入值 → 输出值。
//! 每个 `fn` 都能在 `#[cfg(test)]` 里逐条对照 Swift 的 `ChromeTests` 断言。
//!
//! 移植原则（见 `docs/gpui-parity/`）：
//! - 行为以 Swift 为准，Linux `frontend/linux/chrome/` 只作参考；
//! - 命名用 Rust 风格（`snake_case`），字段语义保持不变；
//! - Swift 因平台产生差异的地方，注释里写清差异点。

pub mod aggregate;
pub mod attention;
pub mod box_drawing;
pub mod composer;
pub mod directory;
pub mod existing;
pub mod image_paste;
pub mod ime;
pub mod input_encoding;
pub mod layout_projection;
pub mod links;
pub mod menu;
pub mod pane_dimming;
pub mod pane_menu;
pub mod pane_navigation;
pub mod pane_title;
pub mod panel;
pub mod physical_key;
pub mod query_detector;
pub mod query_reply;
pub mod quick_connect;
pub mod replica_id;
pub mod resize_math;
pub mod scroll;
pub mod search;
pub mod selection;
pub mod settings;
pub mod shortcut_editor;
pub mod shortcuts;
pub mod sidebar;
pub mod sidebar_layout;
pub mod status_bar;
pub mod surface_feed;
pub mod target;
pub mod target_editor;
pub mod text_input;
pub mod usage;
pub mod word_selection;
pub mod workspace_picker;
