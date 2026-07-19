//! 终端管理抽象（跨平台可复用，无 GUI 依赖）。
//!
//! - [`process`]：pty 创建、子进程 spawn、exit / 信号、进程名
//! - [`scrollback`]：纯环形行缓冲（不解析 ANSI）
//! - [`input`]：键盘事件 → pty 字节流

pub mod input;
pub mod process;
pub mod scrollback;

#[allow(unused_imports)] // 供平台层与后续模块选用
pub use input::{encode, ArrowDir, KeyEvent};
#[allow(unused_imports)]
pub use process::{
    get_process_info, get_process_name, kill, spawn_program, ProcessHandle, SpawnError,
};
#[allow(unused_imports)]
pub use scrollback::ScrollbackBuffer;

/// 终端大小（字符格 + 像素 cell）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalSize {
    pub cols: u16,
    pub rows: u16,
    pub cell_width: u16,
    pub cell_height: u16,
}

impl TerminalSize {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            cols,
            rows,
            cell_width: 0,
            cell_height: 0,
        }
    }
}

/// 进程信息（用于标题更新）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessInfo {
    pub pid: u32,
    /// basename，如 `"bash"`。
    pub name: String,
    /// 如 `"/usr/bin/bash"`；不可得时为空。
    pub full_path: String,
    pub argv: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_size_new_defaults_pixels() {
        let s = TerminalSize::new(80, 24);
        assert_eq!(s.cols, 80);
        assert_eq!(s.rows, 24);
        assert_eq!(s.cell_width, 0);
        assert_eq!(s.cell_height, 0);
    }

    #[test]
    fn process_info_fields() {
        let info = ProcessInfo {
            pid: 1,
            name: "bash".into(),
            full_path: "/usr/bin/bash".into(),
            argv: vec!["/usr/bin/bash".into(), "-l".into()],
        };
        assert_eq!(info.name, "bash");
        assert_eq!(info.argv.len(), 2);
    }
}
