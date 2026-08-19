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

    /// 搜索命中 seq 对应的 viewport 偏移；`None` 表示该稳定行已被淘汰。
    ///
    /// 偏移让该行出现在滚动窗口顶部，与 `scroll_window(offset, rows)` 对齐。
    pub fn viewport_offset_for_seq_checked(&self, seq: u64) -> Option<u32> {
        let index = self.terminal.line_index_by_seq(seq)?;
        let total = self.terminal.scrollback_lines() + self.terminal.visible_snapshot().len();
        Some(total.saturating_sub(index + self.terminal.rows()) as u32)
    }

    /// 兼容旧调用方的 viewport 查询（0 = 可见屏或未找到）。
    pub fn viewport_offset_for_seq(&self, seq: u64) -> u32 {
        self.viewport_offset_for_seq_checked(seq).unwrap_or(0)
    }

    /// OSC 133 命令刻度（W18h）。
    pub fn command_marks(&self) -> &[crate::core::protocol::terminal::emulate::CommandMark] {
        self.terminal.command_marks()
    }

    /// 当前刻度之前最近的一条命令（`0` 表示从尾部开始）。
    pub fn previous_command_mark(
        &self,
        current_seq: u64,
    ) -> Option<&crate::core::protocol::terminal::emulate::CommandMark> {
        self.terminal.previous_command_mark(current_seq)
    }

    /// 当前刻度之后最近的一条命令（`0` 表示从头部开始）。
    pub fn next_command_mark(
        &self,
        current_seq: u64,
    ) -> Option<&crate::core::protocol::terminal::emulate::CommandMark> {
        self.terminal.next_command_mark(current_seq)
    }

    pub fn last_successful_command(
        &self,
    ) -> Option<&crate::core::protocol::terminal::emulate::CommandMark> {
        self.terminal.last_successful_command()
    }

    pub fn last_failed_command(
        &self,
    ) -> Option<&crate::core::protocol::terminal::emulate::CommandMark> {
        self.terminal.last_failed_command()
    }

    /// 一次性 Surface seed：历史和当前屏进入同一个原生 VT。
    pub fn surface_seed_ansi(&self) -> Vec<u8> {
        self.terminal.surface_seed_ansi()
    }

    /// 当前终端最新稳定行 ID（可见屏也包含）。
    pub fn latest_line_seq(&self) -> u64 {
        self.terminal.latest_line_seq()
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

    /// 还能往历史上滚的最大 offset（与 `scroll_window` 对齐：0=底部）。
    ///
    /// 首屏只播种可见网格不等于丢掉历史：GUI 滚轮必须用这个上限喂
    /// `scroll_ansi`，而不是依赖 VTE/SwiftTerm 本地 scrollback。
    pub fn history_max_offset(&self, rows: u32) -> u32 {
        let n = self.terminal.scrollback_lines() + self.terminal.visible_snapshot().len();
        n.saturating_sub(rows.max(1) as usize) as u32
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 首屏丢掉 capture 重放之后，离屏 token 必须仍能从 scroll_ansi 读到。
    #[test]
    fn history_max_offset_keeps_offscreen_token_in_scroll_ansi() {
        let mut buf = PaneBuf::new(80, 24, 1000);
        buf.feed(b"HIST_TOKEN\r\n", 80, 24);
        for i in 0..40 {
            buf.feed(format!("pad-{i:02}\r\n").as_bytes(), 80, 24);
        }
        buf.feed(b"HIST_TAIL\r\n", 80, 24);

        let max = buf.history_max_offset(24);
        assert!(max > 0, "40 行 pad 必须能滚离底部, max={max}");

        let top_bytes = buf.scroll_ansi(max, 24);
        let top = String::from_utf8_lossy(&top_bytes);
        assert!(
            top.contains("HIST_TOKEN"),
            "滚到顶必须看见离屏 token。got={top}"
        );

        let live_bytes = buf.scroll_ansi(0, 24);
        let live = String::from_utf8_lossy(&live_bytes);
        assert!(live.contains("HIST_TAIL"), "底部必须是尾标。got={live}");
        assert!(
            !live.contains("HIST_TOKEN"),
            "底部直播不得含离屏 token。got={live}"
        );
    }
}
