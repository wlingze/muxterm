//! 单行文本按真实字形宽度截断。
//!
//! 为什么不用 gpui 的 `.truncate()`：`white_space: nowrap` 的文本元素只缓存
//! 第一次测量结果（`TextLayout` 的 `wrap_width` 是 `None`，缓存不比较宽度）。
//! 快速面板的副标题第一次测量发生在 flex basis 阶段，可用宽度还是 0，于是
//! 整行被截成一个「…」并且永远不会重新测量 —— 用户截到的就是每一行都只有
//! 三个点，看起来像数据加载失败。
//!
//! 这里在渲染前按面板宽度算好要显示的字符串，文本元素只负责原样画出来。

/// 省略号：与 gpui `ELLIPSIS` 相同，保持观感一致。
pub const ELLIPSIS: &str = "…";

/// 把 `text` 压成一行：放得下就原样返回，放不下就在字符边界截断并补省略号。
///
/// `width` 是前缀宽度测量器（真实字体、真实字号）。返回值保证：
/// - 不会从 UTF-8 多字节字符中间切开；
/// - 结果宽度不超过 `max_width`（除非省略号本身就放不下）；
/// - 不会比 `text` 更长。
pub fn fit_one_line(text: &str, max_width: f32, width: impl Fn(&str) -> f32) -> String {
    if text.is_empty() || max_width <= 0.0 {
        return String::new();
    }
    if width(text) <= max_width {
        return text.to_string();
    }
    let suffix_width = width(ELLIPSIS);
    let budget = max_width - suffix_width;
    if budget <= 0.0 {
        return ELLIPSIS.to_string();
    }

    // 前缀的结束位置都落在字符边界上，二分找最长能放下的那个。
    let ends: Vec<usize> = text
        .char_indices()
        .map(|(index, ch)| index + ch.len_utf8())
        .collect();
    let (mut low, mut high) = (0usize, ends.len());
    while low < high {
        let mid = (low + high) / 2;
        if width(&text[..ends[mid]]) <= budget {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    let cut = if low == 0 { 0 } else { ends[low - 1] };
    format!("{}{ELLIPSIS}", &text[..cut])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 用户报障：面板副标题整行只剩「…」。
    ///
    /// 用等宽假字模拟真实测量：放得下的字符串必须原样保留，不能被无条件
    /// 截成省略号。
    #[test]
    fn text_that_fits_is_kept_verbatim() {
        let width = |text: &str| text.chars().count() as f32 * 10.0;
        assert_eq!(fit_one_line("tmux @ ryzen", 200.0, width), "tmux @ ryzen");
        assert_eq!(fit_one_line("tmux @ ryzen", 120.0, width), "tmux @ ryzen");
    }

    /// 放不下才截断，并且截出来的前缀 + 省略号不超预算。
    #[test]
    fn overflowing_text_is_cut_on_a_char_boundary() {
        let width = |text: &str| text.chars().count() as f32 * 10.0;
        // 预算 50px，省略号占 10px → 只能留 4 个字符。
        assert_eq!(fit_one_line("abcdefgh", 50.0, width), "abcd…");
        assert_eq!(fit_one_line("abcdefgh", 55.0, width), "abcd…");
        assert_eq!(fit_one_line("abcdefgh", 60.0, width), "abcde…");
    }

    /// 中文/emoji 不能被切成半个字符。
    #[test]
    fn wide_characters_never_split() {
        let width = |text: &str| text.chars().count() as f32 * 20.0;
        let fitted = fit_one_line("中文宽字符", 50.0, width);
        assert_eq!(fitted, "中…");
        assert!(fitted.is_char_boundary(fitted.len()));
    }

    /// 退化宽度不画一行省略号；空文本保持空。
    #[test]
    fn degenerate_widths_stay_empty() {
        let width = |text: &str| text.chars().count() as f32 * 10.0;
        assert_eq!(fit_one_line("abc", 0.0, width), "");
        assert_eq!(fit_one_line("", 100.0, width), "");
        assert_eq!(fit_one_line("abc", 1.0, width), "…");
    }
}
