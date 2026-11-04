//! 检测 pane 输出里的「终端查询」序列。
//!
//! 移植自 `chrome/FlatChrome.swift` 的 `TerminalQueryDetector`。
//!
//! 目前仅用于诊断与测试：tmux 控制模式下查询由 tmux 自己代答，前端不再
//! 据此放行 VT 解析器的应答。保留它是为了在镜像路径上给出可观测证据。

/// 支持识别的查询类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryKind {
    /// OSC 10/11/12 动态颜色查询。
    OscDynamicColor(i64),
    /// CSI 设备属性（DA）。
    CsiDeviceAttributes,
    /// CSI 设备状态（DSR）。
    CsiDeviceStatus,
    /// kitty 键盘协议能力查询。
    KittyKeyboard,
}

/// 扫描字节流，返回找到的查询类型（去重，保序）。
pub fn queries(bytes: &[u8]) -> Vec<QueryKind> {
    if bytes.is_empty() {
        return Vec::new();
    }
    let mut found: Vec<QueryKind> = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != 0x1b {
            index += 1;
            continue;
        }
        if index + 1 >= bytes.len() {
            break;
        }
        match bytes[index + 1] {
            b']' => {
                if let Some((kind, consumed)) = parse_osc_query(bytes, index) {
                    if !found.contains(&kind) {
                        found.push(kind);
                    }
                    index += consumed;
                    continue;
                }
            }
            b'[' => {
                if let Some((kind, consumed)) = parse_csi_query(bytes, index) {
                    if !found.contains(&kind) {
                        found.push(kind);
                    }
                    index += consumed;
                    continue;
                }
            }
            _ => {}
        }
        index += 2;
    }
    found
}

/// 是否存在任意查询（供 feed 门禁用）。
pub fn contains_query(bytes: &[u8]) -> bool {
    !queries(bytes).is_empty()
}

/// 解析 OSC 查询：`ESC ] 10 ; ?` 或 `ESC ] 11;?`，以 BEL / ST 结尾。
fn parse_osc_query(bytes: &[u8], start: usize) -> Option<(QueryKind, usize)> {
    let mut index = start + 2;
    let code_start = index;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        index += 1;
    }
    if index == code_start || index >= bytes.len() {
        return None;
    }
    let code = std::str::from_utf8(&bytes[code_start..index])
        .ok()?
        .parse::<i64>()
        .ok()?;
    if code != 10 && code != 11 && code != 12 {
        return None;
    }
    while index < bytes.len() && bytes[index] == b' ' {
        index += 1;
    }
    if index >= bytes.len() || bytes[index] != b';' {
        return None;
    }
    index += 1;
    while index < bytes.len() && bytes[index] == b' ' {
        index += 1;
    }
    if index >= bytes.len() || bytes[index] != b'?' {
        return None;
    }
    index += 1;
    // 跳过直到 BEL / ST。
    while index < bytes.len() && bytes[index] != 0x07 {
        if bytes[index] == 0x1b && index + 1 < bytes.len() && bytes[index + 1] == b'\\' {
            index += 2;
            return Some((QueryKind::OscDynamicColor(code), index - start));
        }
        index += 1;
    }
    if index >= bytes.len() {
        return None;
    }
    Some((QueryKind::OscDynamicColor(code), index - start + 1))
}

/// 解析 CSI 查询：`ESC [ c` / `ESC [ ? … c` / `ESC [ n` / `ESC [ ? … u`。
fn parse_csi_query(bytes: &[u8], start: usize) -> Option<(QueryKind, usize)> {
    let mut index = start + 2;
    let mut saw_question = false;
    if index < bytes.len() && bytes[index] == b'?' {
        saw_question = true;
        index += 1;
    }
    // kitty 能力查询形如 `CSI > 4;0 u`：`>` 后跟数字 / 分号。
    let mut saw_greater = false;
    if index < bytes.len() && bytes[index] == b'>' {
        saw_greater = true;
        index += 1;
    }
    while index < bytes.len() && (bytes[index].is_ascii_digit() || bytes[index] == b';') {
        index += 1;
    }
    if index >= bytes.len() {
        return None;
    }
    match bytes[index] {
        b'c' => Some((QueryKind::CsiDeviceAttributes, index - start + 1)),
        b'n' => Some((QueryKind::CsiDeviceStatus, index - start + 1)),
        b'u' if saw_question || saw_greater => Some((QueryKind::KittyKeyboard, index - start + 1)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 对照 `testDetectsOSCDynamicColorQueries`：codex 启动查询。
    #[test]
    fn detects_osc_dynamic_color_queries() {
        let raw = b"\x1b]10;?\x07\x1b]11;?\x07";
        assert_eq!(
            queries(raw),
            vec![
                QueryKind::OscDynamicColor(10),
                QueryKind::OscDynamicColor(11)
            ]
        );
        assert!(contains_query(raw));
    }

    /// 对照 `testDetectsOSCQueryWithSTTerminator`。
    #[test]
    fn detects_osc_query_with_st_terminator() {
        assert_eq!(
            queries(b"\x1b]12;?\x1b\\"),
            vec![QueryKind::OscDynamicColor(12)]
        );
    }

    /// 对照 `testDetectsCSIDeviceAttributes`。
    #[test]
    fn detects_csi_device_attributes() {
        let raw = b"\x1b[c\x1b[?65;4;1;2;6;21;22;17;28c";
        assert!(queries(raw).contains(&QueryKind::CsiDeviceAttributes));
    }

    /// 对照 `testDetectsCSIDeviceStatus`。
    #[test]
    fn detects_csi_device_status() {
        assert_eq!(queries(b"\x1b[6n"), vec![QueryKind::CsiDeviceStatus]);
    }

    /// 对照 `testDetectsKittyKeyboardQueries`。
    #[test]
    fn detects_kitty_keyboard_queries() {
        assert!(contains_query(b"\x1b[?u"));
        assert!(contains_query(b"\x1b[>4;0u"));
    }

    /// 对照 `testIgnoresPlainOutputWithoutQueries`。
    #[test]
    fn ignores_plain_output_without_queries() {
        let raw = b"hello world\r\n\x1b[31mred\x1b[0m";
        assert!(!contains_query(raw));
        assert!(queries(raw).is_empty());
    }

    /// 对照 `testIgnoresOSCColorSetNotQuery`。
    #[test]
    fn ignores_osc_color_set_not_query() {
        let raw = b"\x1b]10;#ffffff\x07\x1b]11;#000000\x07";
        assert!(!contains_query(raw));
    }

    /// 同一查询重复出现只记一次，保持首次出现顺序。
    #[test]
    fn repeated_queries_are_deduplicated_in_order() {
        let raw = b"\x1b[6n\x1b]11;?\x07\x1b[6n";
        assert_eq!(
            queries(raw),
            vec![QueryKind::CsiDeviceStatus, QueryKind::OscDynamicColor(11)]
        );
    }

    /// `CSI u` 没有 `?` / `>` 前缀时不是 kitty 查询（避免误吃按键回复）。
    #[test]
    fn plain_csi_u_is_not_a_query() {
        assert!(!contains_query(b"\x1b[1u"));
    }
}
