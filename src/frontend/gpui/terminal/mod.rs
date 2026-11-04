//! 终端 Surface：网格、VT 解析、键盘编码。三块都是纯逻辑，可脱离 UI 测试。

pub mod grid;
pub mod keymap;
pub mod parser;

pub use grid::{Attributes, Cell, Color, Grid, Pen};
pub use keymap::{encode, KeyPress};
pub use parser::Surface;
