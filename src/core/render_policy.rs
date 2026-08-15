//! RenderPolicy：决定一批 pane 输出该增量喂 VTE 还是整帧替换（LINUX-PLAN §2.2）。
//!
//! Codex/Cursor 每帧用 `CSI H` + `CSI 2J` 全屏重绘；VTE 若把中间帧全部
//! 演一遍就是「从顶刷到低」。本模块只保留最后一帧，且首屏永远走
//! `ReplaceVisible`（调用方用 replica 的 `visible_ansi()` 播种）。

/// 渲染意图。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderIntent<'a> {
    /// 安静输出：合并后的字节原样给 VTE。
    Incremental(&'a [u8]),
    /// 全屏重绘 / 首屏 / catch-up：VTE reset 后只吃这一帧。
    ReplaceVisible,
}

/// 帧起点：`CSI H`（含 `CSI 1;1H`）、`CSI 2J`、`CSI ?1049h` / `CSI ?1049l`。
fn is_frame_start(bytes: &[u8], i: usize) -> bool {
    if bytes[i] != 0x1b || i + 1 >= bytes.len() || bytes[i + 1] != b'[' {
        return false;
    }
    let rest = &bytes[i + 2..];
    // CSI H / CSI 1;1H / CSI 2J / CSI ?1049h / CSI ?1049l
    if rest.starts_with(b"H") || rest.starts_with(b"1;1H") || rest.starts_with(b"2J") {
        return true;
    }
    if rest.starts_with(b"?1049h") || rest.starts_with(b"?1049l") {
        return true;
    }
    false
}

/// 从一批字节里取最后一帧：从最后一个帧起点开始（含该起点）。
pub fn last_visible_frame(bytes: &[u8]) -> &[u8] {
    let mut last = None;
    let mut i = 0;
    while i < bytes.len() {
        if is_frame_start(bytes, i) {
            last = Some(i);
        }
        i += 1;
    }
    match last {
        Some(start) => &bytes[start..],
        None => bytes,
    }
}

/// 决定渲染意图。
///
/// 规则（§2.2）：
/// 1. `first_paint == true` → 永远 `ReplaceVisible`。
/// 2. 字节里出现 ≥ 2 次帧起点 → `ReplaceVisible`，VTE 只吃 `last_visible_frame`。
/// 3. 单帧或无 CUP → `Incremental(bytes)`。
/// 4. 空字节 → 忽略（返回 `Incremental(&[])`，调用方应跳过）。
pub fn render_intent(bytes: &[u8], first_paint: bool) -> RenderIntent<'_> {
    if bytes.is_empty() {
        return RenderIntent::Incremental(bytes);
    }
    if first_paint {
        return RenderIntent::ReplaceVisible;
    }
    // 统计「帧」而不是「帧起点序列」：`ESC[H ESC[2J` 是同一帧的两个标记，
    // 只有中间出现过可打印内容才算新的一帧。
    let mut starts = 0usize;
    let mut seen_content = false;
    let mut i = 0;
    while i < bytes.len() {
        if is_frame_start(bytes, i) {
            if starts == 0 || seen_content {
                starts += 1;
            }
            seen_content = false;
            i += 2;
            // 跳过该标记的剩余字节（H / 1;1H / 2J / ?1049h / ?1049l）。
            let rest = &bytes[i..];
            for (j, b) in rest.iter().enumerate() {
                if *b == b'H' || *b == b'J' || *b == b'h' || *b == b'l' {
                    i += j + 1;
                    break;
                }
            }
            continue;
        }
        if bytes[i] != 0x1b {
            seen_content = true;
        }
        i += 1;
    }
    if starts >= 2 {
        RenderIntent::ReplaceVisible
    } else {
        RenderIntent::Incremental(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(n: usize) -> Vec<u8> {
        format!("\x1b[H\x1b[2Jframe-{n}").into_bytes()
    }

    /// S1：20 帧连发，只保留最后一帧。
    #[test]
    fn last_visible_frame_keeps_only_final_cup_frame() {
        let mut all = Vec::new();
        for i in 0..20 {
            all.extend_from_slice(&frame(i));
        }
        let last = last_visible_frame(&all);
        let text = String::from_utf8_lossy(last);
        assert!(text.contains("frame-19"), "应含最后一帧: {text}");
        assert!(!text.contains("frame-0"), "不应含第一帧: {text}");
        assert!(!text.contains("frame-18"), "不应含倒数第二帧: {text}");
    }

    /// 首屏永远 ReplaceVisible。
    #[test]
    fn render_intent_first_paint_is_replace() {
        assert_eq!(render_intent(b"hello", true), RenderIntent::ReplaceVisible);
        assert_eq!(
            render_intent(b"\x1b[H\x1b[2Jframe", true),
            RenderIntent::ReplaceVisible
        );
    }

    /// 安静文本走增量。
    #[test]
    fn render_intent_quiet_text_is_incremental() {
        assert_eq!(
            render_intent(b"hello world\r\n", false),
            RenderIntent::Incremental(b"hello world\r\n")
        );
        assert_eq!(render_intent(b"", false), RenderIntent::Incremental(b""));
    }

    /// 单帧（一次 CUP）仍走增量；两帧以上走替换。
    #[test]
    fn render_intent_single_frame_incremental_multi_replace() {
        let one = frame(0);
        assert_eq!(
            render_intent(&one, false),
            RenderIntent::Incremental(one.as_slice())
        );
        let mut two = frame(0);
        two.extend_from_slice(&frame(1));
        assert_eq!(render_intent(&two, false), RenderIntent::ReplaceVisible);
    }

    /// 备用帧起点（CSI 2J 单独出现、alternate screen 切换）也计数。
    #[test]
    fn frame_starts_include_clear_and_alternate_screen() {
        let bytes = b"\x1b[2Jclear\x1b[?1049halt";
        assert_eq!(render_intent(bytes, false), RenderIntent::ReplaceVisible);
        let last = last_visible_frame(bytes);
        assert!(String::from_utf8_lossy(last).contains("alt"));
    }
}
