//! PaneBuf：工作区里一个 pane 的搜索/提醒/peek 缓冲。
//!
//! 每个 PaneBuf 持有：
//! - `TerminalState`：可见屏 + 有界 scrollback（搜索/提醒/peek 的事实源）
//! - 有界原始字节环（`buffer_cap` 上限，丢最旧；live 显示仍走 VTE 原始字节）
//! - `viewport`：滚动偏移（0 = 底部直播；跳转后恢复）
//!
//! Live GUI **只** `vte.feed` 原始字节；PaneBuf 只给搜索/提醒/peek，禁止
//! dump `visible_ansi` 当显示。

use crate::core::attention::signal::AttentionSignal;
use crate::core::buffer_cap::{append_capped, MAX_PANE_OUTPUT_BYTES};
use crate::core::protocol::terminal::emulate::TerminalState;

/// 一个 pane 的缓冲（scrollback + byte ring + viewport）。
pub struct PaneBuf {
    terminal: TerminalState,
    /// 有界原始字节环（丢最旧；供 peek / 小终端播种）。
    byte_ring: Vec<u8>,
    /// 当前 viewport 滚动偏移（0 = 底部直播）。
    viewport: u32,
}

impl PaneBuf {
    pub fn new(cols: usize, rows: usize, scrollback_max: usize) -> Self {
        Self {
            terminal: TerminalState::with_scrollback(
                cols.max(1),
                rows.max(1),
                scrollback_max.max(1),
            ),
            byte_ring: Vec::new(),
            viewport: 0,
        }
    }

    /// 喂入 pane 输出（自动 resize）。
    ///
    /// 注意力信号留在 `TerminalState`，由 [`Self::take_attention_signals`]
    /// 取走；这里**不** drain（`Workspace::feed_events` 丢弃返回值会丢信号）。
    pub fn feed(&mut self, bytes: &[u8], cols: u16, rows: u16) -> Vec<AttentionSignal> {
        self.terminal
            .resize(usize::from(cols.max(1)), usize::from(rows.max(1)));
        append_capped(&mut self.byte_ring, bytes, MAX_PANE_OUTPUT_BYTES);
        self.terminal.feed(bytes);
        Vec::new()
    }

    /// 取走尚未消费的注意力信号（GUI 在 refresh 后调用）。
    pub fn take_attention_signals(&mut self) -> Vec<AttentionSignal> {
        self.terminal.take_attention_signals()
    }

    /// 最近一次 feed 的 seq + 最后非空行（注意力引擎用）。
    pub fn last_line_seq(&self) -> (String, u64) {
        (
            self.terminal.last_non_empty_line().unwrap_or_default(),
            self.terminal.latest_seq(),
        )
    }

    /// 搜索可见屏 + scrollback，返回 (seq, line) 命中。
    pub fn search(&self, query: &str) -> Vec<(u64, String)> {
        self.terminal.search(query)
    }

    /// 最近 n 行（可见屏 + scrollback）。
    pub fn last_n_lines(&self, n: usize) -> Vec<String> {
        self.terminal.last_n_lines(n)
    }

    /// scrollback 中某 seq 的行索引（W17c 搜索跳转滚到命中行用）。
    pub fn line_index_by_seq(&self, seq: u64) -> Option<usize> {
        self.terminal.line_index_by_seq(seq)
    }

    /// 有界原始字节环（peek / 小终端播种用）。
    pub fn raw_bytes(&self) -> &[u8] {
        &self.byte_ring
    }

    /// 取走 OSC/CSI 查询应答（渲染层回写用）。
    pub fn take_reply(&mut self) -> Vec<u8> {
        self.terminal.take_reply()
    }

    /// 可见网格 ANSI（首屏播种用；禁止当 live 显示）。
    pub fn visible_ansi(&self) -> Vec<u8> {
        self.terminal.visible_ansi()
    }

    /// 滚动窗口的几何 ANSI（offset 行前、rows 行；0=底部直播）。
    pub fn scroll_ansi(&self, offset: u32, rows: u32) -> Vec<u8> {
        let lines = self.terminal.scroll_window(offset, rows as usize);
        if lines.is_empty() {
            return Vec::new();
        }
        let mut tmp = TerminalState::new(self.terminal.cols(), rows.max(1) as usize);
        for (i, line) in lines.iter().enumerate() {
            if i + 1 == lines.len() {
                tmp.feed(line.as_bytes());
            } else {
                tmp.feed(format!("{line}\r\n").as_bytes());
            }
        }
        tmp.visible_ansi()
    }

    /// 网格是否全空。
    pub fn is_blank(&self) -> bool {
        self.terminal.is_blank()
    }

    /// 是否处于 bracketed paste 模式。
    pub fn bracketed_paste(&self) -> bool {
        self.terminal.bracketed_paste
    }

    /// 当前 viewport 滚动偏移。
    pub fn viewport(&self) -> u32 {
        self.viewport
    }

    /// 设置 viewport 滚动偏移（跳转后恢复）。
    pub fn set_viewport(&mut self, offset: u32) {
        self.viewport = offset;
    }

    pub fn cols(&self) -> usize {
        self.terminal.cols()
    }

    pub fn rows(&self) -> usize {
        self.terminal.rows()
    }
}
