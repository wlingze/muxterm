//! Pane 视图：VTE4 渲染 + FFI 输入/输出。
//!
//! 不再本地 spawn pty；所有 I/O 走 [`crate::platform::linux::ffi_bridge::CoreBridge`]。

use std::cell::Cell;
use std::rc::Rc;

use gtk4::prelude::*;
use vte4::prelude::*;

use crate::core::config::Theme;
use crate::platform::linux::renderer::{TerminalRenderer, VteRenderer};

/// 一个 pane 的 GTK 视图。
pub struct PaneView {
    renderer: VteRenderer,
    pane_id: Cell<u32>,
    /// 已 feed 到 VTE 的累计输出长度（用于增量同步）。
    fed_len: Cell<usize>,
}

impl PaneView {
    pub fn new(pane_id: u32, theme: &Theme) -> Self {
        let renderer = VteRenderer::new();
        renderer.apply_theme(theme);
        Self {
            renderer,
            pane_id: Cell::new(pane_id),
            fed_len: Cell::new(0),
        }
    }

    pub fn pane_id(&self) -> u32 {
        self.pane_id.get()
    }

    pub fn set_pane_id(&self, id: u32) {
        self.pane_id.set(id);
    }

    pub fn widget(&self) -> gtk4::Widget {
        self.renderer.widget()
    }

    pub fn terminal(&self) -> &vte4::Terminal {
        self.renderer.terminal()
    }

    /// 把增量输出写入 VTE。
    pub fn feed_output(&self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        // TerminalRenderer::render 需要 &mut；VTE feed 内部不需 mut 状态
        self.renderer.terminal().feed(data);
        self.fed_len
            .set(self.fed_len.get().saturating_add(data.len()));
    }

    /// 用完整快照同步（首次挂载 pane 时）。
    pub fn sync_full_output(&self, data: &[u8]) {
        // 简化：清空后重放（VTE 无稳定 reset API 时直接 feed 增量）
        let already = self.fed_len.get();
        if data.len() > already {
            self.feed_output(&data[already..]);
        } else if data.len() < already {
            // 输出被截断：重置计数并全量 feed
            self.fed_len.set(0);
            self.renderer.terminal().feed(data);
            self.fed_len.set(data.len());
        }
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        // VTE set_size 需要 &Terminal；通过 clone 的 widget 调用
        let term = self.renderer.terminal().clone();
        term.set_size(cols.max(1) as i64, rows.max(1) as i64);
    }

    /// 用户按键 → 回调（由 window 转发到 FFI send_input）。
    pub fn connect_input<F: Fn(u32, &[u8]) + 'static>(&self, f: F) {
        let pid = self.pane_id.clone();
        self.renderer
            .terminal()
            .connect_commit(move |_term, text, _len| {
                f(pid.get(), text.as_bytes());
            });
    }

    pub fn grab_focus(&self) {
        self.renderer.terminal().grab_focus();
    }
}

/// 便于在闭包里共享的 PaneView 句柄。
pub type PaneViewRc = Rc<PaneView>;
