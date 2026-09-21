//! 有界缓冲：防止挂起/忙等时 pane 输出与半行缓冲涨到数 GB。

use std::collections::VecDeque;

/// 单 pane 累计输出上限（字节）。超出时丢弃最旧前缀，保留尾部。
pub const MAX_PANE_OUTPUT_BYTES: usize = 2 * 1024 * 1024;

/// 未完成行（无换行）读缓冲上限。
pub const MAX_INCOMPLETE_LINE_BYTES: usize = 1024 * 1024;

/// 事件队列软上限：超出时优先丢弃最旧的 `PaneOutput` 类事件占用
/// （由调用方在 push 后调用 [`trim_front_while`]）。
pub const MAX_STATE_EVENTS: usize = 8_192;

/// 适合长期高频追加的有界字节环。
///
/// 普通 `Vec::drain(..n)` 会在缓冲达到上限后为每个小增量搬动整个尾部；终端 diff
/// 往往没有换行，于是旧实现还会反复扫描完整的 2 MiB 缓冲。这里用 `start`
/// 丢掉前缀，并单独记录换行的绝对位置，使截断和行边界对齐都只处理新增/删除的字节。
/// `as_slice()` 对 `pane_output` 查询是 O(1) 连续视图。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CappedBytes {
    bytes: Vec<u8>,
    start: usize,
    line_breaks: VecDeque<u64>,
    head_offset: u64,
}

impl CappedBytes {
    /// 追加字节并把逻辑长度限制在 `max` 以内。
    pub fn append(&mut self, data: &[u8], max: usize) {
        if max == 0 {
            self.clear();
            return;
        }
        if data.len() >= max {
            self.replace(&data[data.len() - max..]);
            self.align_to_line_start();
            return;
        }

        self.rebase_if_needed(data.len());
        let tail_offset = self.head_offset + self.len() as u64;
        self.line_breaks.extend(
            data.iter()
                .enumerate()
                .filter(|(_, byte)| **byte == b'\n')
                .map(|(index, _)| tail_offset + index as u64),
        );
        self.bytes.extend_from_slice(data);

        if self.len() > max {
            self.discard_front(self.len() - max);
            self.align_to_line_start();
        }
    }

    pub fn clear(&mut self) {
        self.bytes.clear();
        self.start = 0;
        self.line_breaks.clear();
        self.head_offset = 0;
    }

    pub fn len(&self) -> usize {
        self.bytes.len().saturating_sub(self.start)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.bytes[self.start..]
    }

    pub fn to_vec(&self) -> Vec<u8> {
        self.as_slice().to_vec()
    }

    fn replace(&mut self, data: &[u8]) {
        self.clear();
        self.line_breaks.extend(
            data.iter()
                .enumerate()
                .filter(|(_, byte)| **byte == b'\n')
                .map(|(index, _)| index as u64),
        );
        self.bytes.extend_from_slice(data);
    }

    fn discard_front(&mut self, count: usize) {
        debug_assert!(count <= self.len());
        self.start += count;
        self.head_offset += count as u64;
        while self
            .line_breaks
            .front()
            .is_some_and(|offset| *offset < self.head_offset)
        {
            self.line_breaks.pop_front();
        }
        self.compact_if_needed();
    }

    fn compact_if_needed(&mut self) {
        if self.start >= 4096 && self.start * 2 >= self.bytes.len() {
            self.bytes.drain(..self.start);
            self.start = 0;
        }
    }

    fn align_to_line_start(&mut self) {
        let Some(line_break) = self.line_breaks.front().copied() else {
            return;
        };
        let count = (line_break + 1 - self.head_offset) as usize;
        self.discard_front(count);
    }

    fn rebase_if_needed(&mut self, incoming: usize) {
        let required = self.len().saturating_add(incoming) as u64;
        if self.head_offset <= u64::MAX.saturating_sub(required) {
            return;
        }
        for offset in &mut self.line_breaks {
            *offset -= self.head_offset;
        }
        self.head_offset = 0;
    }
}

/// 向 `buf` 追加 `data`，总长超过 `max` 时丢掉最旧前缀，保留尾部。
pub fn append_capped(buf: &mut Vec<u8>, data: &[u8], max: usize) {
    if max == 0 {
        buf.clear();
        return;
    }
    if data.len() >= max {
        buf.clear();
        buf.extend_from_slice(&data[data.len() - max..]);
        align_to_line_start(buf);
        return;
    }
    buf.extend_from_slice(data);
    if buf.len() > max {
        let drop_n = buf.len() - max;
        buf.drain(..drop_n);
        align_to_line_start(buf);
    }
}

/// 丢弃前缀后，把缓冲起点推进到下一个完整换行之后。
///
/// `getPaneOutput` 拿到的「最近尾部」会作为前端终端的新快照；如果它从一条
/// 输出行 / 转义序列中间开始，SwiftTerm 可能吞掉后续 `ESC[2K` / `ESC[1A` /
/// SGR，表现为 agent 输入框逐行堆叠、颜色失效。从行边界开始能让快照尽可能
/// 落在可解析的位置。缓冲内没有换行时保持原样（单行超长，无法更安全）。
fn align_to_line_start(buf: &mut Vec<u8>) {
    if let Some(pos) = buf.iter().position(|&b| b == b'\n') {
        buf.drain(..=pos);
    }
}

/// 半行缓冲过长且仍无换行时，丢掉前缀，避免无界增长。
pub fn trim_incomplete_line(buf: &mut Vec<u8>, max: usize) {
    if max == 0 {
        buf.clear();
        return;
    }
    if buf.len() > max {
        let drop_n = buf.len() - max;
        buf.drain(..drop_n);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capped_bytes_keeps_exact_tail_across_many_newline_free_writes() {
        let mut buf = CappedBytes::default();
        let source = (0..20_000)
            .map(|i| ((i % 200) + 32) as u8)
            .collect::<Vec<_>>();

        for chunk in source.chunks(7) {
            buf.append(chunk, 1_024);
        }

        assert_eq!(buf.len(), 1_024);
        assert_eq!(buf.to_vec(), source[source.len() - 1_024..]);
        assert!(buf.line_breaks.is_empty());
    }

    #[test]
    fn capped_bytes_aligns_evicted_prefix_to_the_next_complete_line() {
        let mut buf = CappedBytes::default();

        buf.append(b"old-partial\nrecent-one\nrecent-two", 24);

        assert_eq!(buf.to_vec(), b"recent-one\nrecent-two");
        assert_eq!(buf.as_slice(), b"recent-one\nrecent-two");
        assert_eq!(buf.len(), 21);
        assert_eq!(buf.line_breaks.len(), 1);
    }

    #[test]
    fn capped_bytes_as_slice_survives_many_newline_free_evictions() {
        let mut buf = CappedBytes::default();
        for i in 0..4_000u32 {
            buf.append(&i.to_le_bytes(), 64);
        }
        assert_eq!(buf.len(), 64);
        assert_eq!(buf.as_slice().len(), 64);
        assert_eq!(buf.as_slice(), buf.to_vec());
    }

    #[test]
    fn append_capped_keeps_tail_under_max() {
        let mut buf = Vec::new();
        append_capped(&mut buf, &[b'a'; 100], 50);
        assert_eq!(buf.len(), 50);
        assert!(buf.iter().all(|&b| b == b'a'));
    }

    #[test]
    fn append_capped_overwrites_when_chunk_larger_than_max() {
        let mut buf = vec![b'z'; 10];
        append_capped(&mut buf, &[b'x'; 200], 30);
        assert_eq!(buf.len(), 30);
        assert!(buf.iter().all(|&b| b == b'x'));
    }

    #[test]
    fn append_capped_preserves_recent_across_many_writes() {
        let mut buf = Vec::new();
        for i in 0..100u8 {
            append_capped(&mut buf, &[i; 1_000], 5_000);
            assert!(buf.len() <= 5_000, "len={}", buf.len());
        }
        assert_eq!(buf.len(), 5_000);
        // 尾部应是较新的字节
        assert_eq!(*buf.last().unwrap(), 99);
    }

    #[test]
    fn append_capped_tail_starts_at_line_boundary() {
        let mut buf = Vec::new();
        // 先塞满无换行数据，再补一行完整输出，触发截断落在行中间。
        for _ in 0..60 {
            append_capped(&mut buf, b"0123456789", 100);
        }
        // 再追加一行以 \n 结尾的数据。
        append_capped(&mut buf, b"tail-line\n", 100);
        // 尾部保留；截断/对齐后不能超过 max+一行。
        assert!(buf.len() <= 101, "len={}", buf.len());
        // 起点要么是空，要么紧跟在某个换行之后（即不是半行中间）。
        if !buf.is_empty() {
            assert_eq!(buf[0], b't', "起点应落在完整行开头");
        }
    }

    #[test]
    fn append_capped_large_chunk_starts_at_line_boundary() {
        let mut buf = Vec::new();
        let mut chunk = Vec::new();
        for i in 0..300 {
            chunk.extend_from_slice(format!("line {i}\n").as_bytes());
        }
        append_capped(&mut buf, &chunk, 100);
        assert!(buf.ends_with(b"line 299\n"), "应保留最新一行");
        assert!(buf.len() <= 101, "允许略超一行但不应失控: {}", buf.len());
    }

    #[test]
    fn trim_incomplete_line_drops_prefix() {
        let mut buf = vec![b'a'; 100];
        trim_incomplete_line(&mut buf, 40);
        assert_eq!(buf.len(), 40);
    }

    #[test]
    fn max_pane_output_is_finite_and_sane() {
        // 用运行时比较避免 clippy::assertions_on_constants
        let max_out = MAX_PANE_OUTPUT_BYTES;
        let max_line = MAX_INCOMPLETE_LINE_BYTES;
        let max_ev = MAX_STATE_EVENTS;
        assert!(max_out <= 8 * 1024 * 1024);
        assert!(max_out >= 64 * 1024);
        assert!(max_line <= max_out);
        assert!(max_ev >= 256);
    }
}
