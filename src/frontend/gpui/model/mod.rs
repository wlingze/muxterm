//! 从 macOS `src/frontend/macos/Chrome/` 移植的纯模型规则。
//!
//! 这些模块不依赖 gpui、不依赖 AppKit、不持有状态：输入值 → 输出值。
//! 每个 `fn` 都能在 `#[cfg(test)]` 里逐条对照 Swift 的 `ChromeTests` 断言。
//!
//! 移植原则（见 `docs/gpui-parity/`）：
//! - 行为以 Swift 为准，Linux `frontend/linux/chrome/` 只作参考；
//! - 命名用 Rust 风格（`snake_case`），字段语义保持不变；
//! - Swift 因平台产生差异的地方，注释里写清差异点。

pub mod attention;
pub mod input_encoding;
pub mod layout_projection;
pub mod query_reply;
pub mod replica_id;
pub mod resize_math;
pub mod sidebar;
