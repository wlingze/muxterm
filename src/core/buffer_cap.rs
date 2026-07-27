//! 有界缓冲：防止挂起/忙等时 pane 输出与半行缓冲涨到数 GB。

/// 单 pane 累计输出上限（字节）。超出时丢弃最旧前缀，保留尾部。
pub const MAX_PANE_OUTPUT_BYTES: usize = 2 * 1024 * 1024;

/// 未完成行（无换行）读缓冲上限。
pub const MAX_INCOMPLETE_LINE_BYTES: usize = 1024 * 1024;

/// 事件队列软上限：超出时优先丢弃最旧的 `PaneOutput` 类事件占用
/// （由调用方在 push 后调用 [`trim_front_while`]）。
pub const MAX_STATE_EVENTS: usize = 8_192;

/// 向 `buf` 追加 `data`，总长超过 `max` 时丢掉最旧前缀，保留尾部。
pub fn append_capped(buf: &mut Vec<u8>, data: &[u8], max: usize) {
    if max == 0 {
        buf.clear();
        return;
    }
    if data.len() >= max {
        buf.clear();
        buf.extend_from_slice(&data[data.len() - max..]);
        return;
    }
    buf.extend_from_slice(data);
    if buf.len() > max {
        let drop_n = buf.len() - max;
        buf.drain(..drop_n);
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
