//! attach 前 tmux 历史：按行解析，不是 VT 流重放。
//!
//! `capture-pane -S -N -E -1` 只取可见区以上的行。前端写入 SwiftTerm
//! scrollback，禁止 `reset`，也禁止把带可见屏的 `-S -10000` dump `feed()`。

/// 历史捕获与拆行的纯函数，不碰 tmux socket。
pub struct PaneHistoryPolicy;

impl PaneHistoryPolicy {
    /// 去掉 capture 里的 SGR/OSC，Index 和 Surface 都按纯文本行处理。
    pub fn strip_sgr(input: &str) -> String {
        let bytes = input.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == 0x1b {
                if i + 1 < bytes.len() && bytes[i + 1] == b'[' {
                    i += 2;
                    while i < bytes.len() && !(0x40..=0x7e).contains(&bytes[i]) {
                        i += 1;
                    }
                    if i < bytes.len() {
                        i += 1;
                    }
                    continue;
                }
                if i + 1 < bytes.len() && bytes[i + 1] == b']' {
                    i += 2;
                    while i < bytes.len() {
                        if bytes[i] == 0x07 {
                            i += 1;
                            break;
                        }
                        if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'\\' {
                            i += 2;
                            break;
                        }
                        i += 1;
                    }
                    continue;
                }
                i = (i + 2).min(bytes.len());
                continue;
            }
            out.push(bytes[i]);
            i += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    /// 若一次 capture 误带了可见屏，丢掉最后 `visible_rows` 行。
    /// `-E -1` 的响应不应走这条，单测用来锁住「不能把当前屏当历史」。
    pub fn split_history_and_visible(
        lines: &[String],
        visible_rows: usize,
    ) -> (Vec<String>, Vec<String>) {
        if visible_rows == 0 || lines.len() <= visible_rows {
            return (Vec::new(), lines.to_vec());
        }
        let idx = lines.len() - visible_rows;
        (lines[..idx].to_vec(), lines[idx..].to_vec())
    }

    /// 产品事件正文：每行一条，`\\n` 分隔。
    pub fn encode(lines: &[String]) -> Vec<u8> {
        lines
            .iter()
            .map(|line| Self::strip_sgr(line))
            .collect::<Vec<_>>()
            .join("\n")
            .into_bytes()
    }

    pub fn decode(data: &[u8]) -> Vec<String> {
        if data.is_empty() {
            return Vec::new();
        }
        String::from_utf8_lossy(data)
            .split('\n')
            .map(str::to_string)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::PaneHistoryPolicy;

    #[test]
    fn split_drops_visible_rows_not_history_token() {
        let mut lines = vec!["HIST_OFFSCREEN".to_string()];
        for i in 0..40 {
            lines.push(format!("pad-{i:02}"));
        }
        lines.push("HIST_TAIL".into());
        let (history, visible) = PaneHistoryPolicy::split_history_and_visible(&lines, 24);
        assert!(history.iter().any(|line| line == "HIST_OFFSCREEN"));
        assert!(!visible.iter().any(|line| line == "HIST_OFFSCREEN"));
        assert!(visible.iter().any(|line| line == "HIST_TAIL"));
        assert_eq!(visible.len(), 24);
        assert_eq!(history.len(), lines.len() - 24);
    }

    #[test]
    fn encode_is_rows_not_vt_dump() {
        let data = PaneHistoryPolicy::encode(&[
            "\u{1b}[32mHIST_OFFSCREEN\u{1b}[0m".into(),
            "pad-01".into(),
        ]);
        let text = String::from_utf8(data).unwrap();
        assert_eq!(text, "HIST_OFFSCREEN\npad-01");
        assert!(!text.contains('\u{1b}'));
        assert!(!text.contains("[2J"));
    }

    #[test]
    fn decode_roundtrip_keeps_blank_lines() {
        let lines = vec!["a".into(), String::new(), "b".into()];
        let decoded = PaneHistoryPolicy::decode(&PaneHistoryPolicy::encode(&lines));
        assert_eq!(decoded, lines);
    }
}
