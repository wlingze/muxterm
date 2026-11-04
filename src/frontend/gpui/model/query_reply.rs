//! 终端查询应答构建器。
//!
//! 移植自 `chrome/TerminalQueryReply.swift`。
//!
//! 终端收到 OSC 10/11/12（动态颜色查询）、CSI DA（设备属性查询）时会向宿主
//! 请求答案；宿主必须以正确的转义序列回复，否则引导字节（ESC）丢失会把
//! `10;rgb:...` 当普通文本，出现 `zsh: command not found: 10`。
//!
//! 字节序列必须逐字节保留，`ChromeTests/SwiftTermQueryReplyTests.swift` 的
//! 断言就是对这些字节的直接比较。

/// ESC（`0x1b`），OSC/CSI 的引导字节。
pub const ESC: u8 = 0x1b;

/// OSC 动态颜色查询回复：`ESC ] <code> ; rgb:RRRR/GGGG/BBBB ESC \`。
///
/// `code` 为 10（前景）、11（背景）、12（光标）；`hex` 为 3 或 6 位十六进制，
/// 允许带 `#` 前缀。
pub fn osc_dynamic_color(code: u32, hex: &str) -> Vec<u8> {
    let rgb = xterm_rgb_from_hex(hex);
    let mut out = Vec::with_capacity(24 + rgb.len());
    out.push(ESC);
    out.push(b']');
    out.extend_from_slice(format!("{code};rgb:{rgb}").as_bytes());
    out.push(ESC);
    out.push(b'\\');
    out
}

/// 把 3/6 位 hex 转成 xterm 的 `RRRR/GGGG/BBBB`（每分量 4 位十六进制）。
///
/// 与 Swift 一致：先把 3 位展开成 6 位，再把每个 2 位分量复制一遍。
/// 长度不是 3 的输入按 6 位规则切片，不足的部分留空——保留这个行为，
/// 不额外做校验，否则会和 Swift 的输出产生字节差异。
pub fn xterm_rgb_from_hex(hex: &str) -> String {
    let clean = hex.strip_prefix('#').unwrap_or(hex);
    let chars: Vec<char> = clean.chars().collect();
    let (r, g, b) = if chars.len() == 3 {
        (
            format!("{0}{0}", chars[0]),
            format!("{0}{0}", chars[1]),
            format!("{0}{0}", chars[2]),
        )
    } else {
        (take(&chars, 0, 2), take(&chars, 2, 2), take(&chars, 4, 2))
    };
    format!("{r}{r}/{g}{g}/{b}{b}")
}

/// 取 `chars[skip..skip+count]`，越界部分丢弃。
fn take(chars: &[char], skip: usize, count: usize) -> String {
    chars.iter().skip(skip).take(count).collect()
}

/// CSI 设备属性（Primary DA）查询回复：`ESC [ ? 65 ; ... c`。
pub fn csi_device_attributes(attrs: &[u32]) -> Vec<u8> {
    let joined = attrs
        .iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join(";");
    let mut out = Vec::with_capacity(8 + joined.len());
    out.push(ESC);
    out.push(b'[');
    out.extend_from_slice(format!("?{joined}c").as_bytes());
    out
}

/// Muxterm 在 DA1 里声明的属性列表。
///
/// 与 `SwiftTermQueryReplyTests.testPrimaryDeviceAttributesReply` 期望的
/// `ESC [ ? 65 ; 4 ; 1 ; 2 ; 6 ; 21 ; 22 ; 17 ; 28 c` 完全一致。
pub const DEFAULT_DEVICE_ATTRIBUTES: &[u32] = &[65, 4, 1, 2, 6, 21, 22, 17, 28];

#[cfg(test)]
mod tests {
    use super::*;

    /// 对照 `SwiftTermQueryReplyTests.testOsc10ForegroundQueryReply`。
    #[test]
    fn osc_10_foreground_reply_matches_swiftterm_bytes() {
        assert_eq!(
            String::from_utf8(osc_dynamic_color(10, "000000")).unwrap(),
            "\u{1b}]10;rgb:0000/0000/0000\u{1b}\\"
        );
    }

    /// 对照 `testOsc11BackgroundQueryReply`：默认白色背景。
    #[test]
    fn osc_11_background_reply_uses_four_digit_components() {
        assert_eq!(
            String::from_utf8(osc_dynamic_color(11, "ffffff")).unwrap(),
            "\u{1b}]11;rgb:ffff/ffff/ffff\u{1b}\\"
        );
    }

    /// `xtermRgb(fromHex:)` 的入参是 8 位/通道的 hex，规则是把每个字节对
    /// 复制一遍得到 16 位分量，所以 `123456` → `1212/3434/5656`。
    ///
    /// 注意与 `SwiftTermQueryReplyTests.testOsc12CursorQueryReplyFallsBackToForeground`
    /// 的区别：那条断言期望 `1234/3456/5678`，因为 SwiftTerm 直接拿 16 位
    /// 通道值拼串，没有经过 hex 展开。两条规则各管一个输入域，都要保留。
    #[test]
    fn osc_12_cursor_reply_expands_each_byte_pair() {
        assert_eq!(
            String::from_utf8(osc_dynamic_color(12, "123456")).unwrap(),
            "\u{1b}]12;rgb:1212/3434/5656\u{1b}\\"
        );
    }

    /// 8 位 hex 展开后与 SwiftTerm 的 16 位输出等价。
    #[test]
    fn eight_bit_hex_matches_sixteen_bit_channel_output() {
        assert_eq!(xterm_rgb_from_hex("ff0000"), "ffff/0000/0000");
        assert_eq!(xterm_rgb_from_hex("123456"), "1212/3434/5656");
    }

    /// 对照 `testPrimaryDeviceAttributesReply`。
    #[test]
    fn primary_device_attributes_matches_expected_list() {
        assert_eq!(
            String::from_utf8(csi_device_attributes(DEFAULT_DEVICE_ATTRIBUTES)).unwrap(),
            "\u{1b}[?65;4;1;2;6;21;22;17;28c"
        );
    }

    /// 对照 `testMultipleQueryRepliesPreserveOrderAndBytes`：两个回复拼接后
    /// 字节顺序不变。
    #[test]
    fn consecutive_replies_keep_order_and_bytes() {
        let mut out = osc_dynamic_color(10, "000000");
        out.extend_from_slice(&csi_device_attributes(DEFAULT_DEVICE_ATTRIBUTES));
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "\u{1b}]10;rgb:0000/0000/0000\u{1b}\\\u{1b}[?65;4;1;2;6;21;22;17;28c"
        );
    }

    #[test]
    fn hex_accepts_hash_prefix_and_three_digit_shorthand() {
        assert_eq!(xterm_rgb_from_hex("#abc"), "aaaa/bbbb/cccc");
        assert_eq!(xterm_rgb_from_hex("abcf00"), "abab/cfcf/0000");
    }

    #[test]
    fn reply_starts_with_esc_bracket_and_ends_with_st_terminator() {
        let out = osc_dynamic_color(11, "ff0000");
        assert_eq!(out[0], ESC);
        assert_eq!(out[1], b']');
        assert_eq!(&out[out.len() - 2..], &[ESC, b'\\']);
    }
}
