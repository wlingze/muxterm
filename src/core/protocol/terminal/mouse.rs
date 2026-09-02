//! xterm SGR (1006) 鼠标报告。
//!
//! 应用开了 1000/1002/1003 时，滚轮/点击/悬浮必须写成 CSI 交给 pane，
//! 不能改本地 scrollback。Grok 常见组合是 1003+1006，且不一定进 alt-screen。

/// 左键。
pub const SGR_LEFT: u16 = 0;
/// 中键。
pub const SGR_MIDDLE: u16 = 1;
/// 右键。
pub const SGR_RIGHT: u16 = 2;
/// 滚轮向上。
pub const SGR_WHEEL_UP: u16 = 64;
/// 滚轮向下。
pub const SGR_WHEEL_DOWN: u16 = 65;
/// 1003 无按键移动（hover）。
pub const SGR_HOVER: u16 = 35;
/// 按键按住时的 motion 附加值。
pub const SGR_MOTION: u16 = 32;

/// SGR 1006：`CSI < Pb ; Px ; Py M`（按下/移动）或 `m`（松开）。
/// 坐标 1-based。
pub fn sgr_report(button: u16, col: u16, row: u16, release: bool) -> Vec<u8> {
    let col = col.max(1);
    let row = row.max(1);
    let end = if release { b'm' } else { b'M' };
    let mut out = Vec::with_capacity(16);
    out.extend_from_slice(b"\x1b[<");
    out.extend_from_slice(button.to_string().as_bytes());
    out.push(b';');
    out.extend_from_slice(col.to_string().as_bytes());
    out.push(b';');
    out.extend_from_slice(row.to_string().as_bytes());
    out.push(end);
    out
}

/// GTK 按钮 1/2/3 → SGR 0/1/2。
pub fn gtk_button_to_sgr(button: u32) -> Option<u16> {
    match button {
        1 => Some(SGR_LEFT),
        2 => Some(SGR_MIDDLE),
        3 => Some(SGR_RIGHT),
        _ => None,
    }
}

/// 一格滚轮对应的 SGR 报告（向上 64 / 向下 65）。
pub fn sgr_wheel(delta_y: f64, col: u16, row: u16) -> Option<Vec<u8>> {
    if delta_y == 0.0 {
        return None;
    }
    let button = if delta_y < 0.0 {
        SGR_WHEEL_UP
    } else {
        SGR_WHEEL_DOWN
    };
    let notches = wheel_notches(delta_y);
    let one = sgr_report(button, col, row, false);
    let mut out = Vec::with_capacity(one.len() * notches as usize);
    for _ in 0..notches {
        out.extend_from_slice(&one);
    }
    Some(out)
}

/// 触控板小数 delta 也至少一格，避免 round(±0.3)=0 把滚轮吃掉。
pub fn wheel_notches(delta_y: f64) -> u32 {
    if delta_y == 0.0 {
        return 0;
    }
    let rounded = delta_y.round() as i32;
    if rounded == 0 {
        1
    } else {
        rounded.unsigned_abs()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sgr_report_encodes_press_release_and_wheel() {
        assert_eq!(sgr_report(0, 12, 34, false), b"\x1b[<0;12;34M");
        assert_eq!(sgr_report(0, 12, 34, true), b"\x1b[<0;12;34m");
        assert_eq!(sgr_report(SGR_WHEEL_UP, 5, 6, false), b"\x1b[<64;5;6M");
        assert_eq!(sgr_report(SGR_HOVER, 1, 1, false), b"\x1b[<35;1;1M");
        assert_eq!(sgr_report(0, 0, 0, false), b"\x1b[<0;1;1M", "坐标至少 1");
    }

    #[test]
    fn sgr_wheel_uses_sign_and_repeats_notches() {
        assert_eq!(
            sgr_wheel(-1.0, 8, 3).as_deref(),
            Some(b"\x1b[<64;8;3M".as_slice())
        );
        assert_eq!(
            sgr_wheel(1.0, 8, 3).as_deref(),
            Some(b"\x1b[<65;8;3M".as_slice())
        );
        assert_eq!(
            sgr_wheel(-2.0, 1, 1).unwrap().len(),
            sgr_report(SGR_WHEEL_UP, 1, 1, false).len() * 2
        );
        assert!(sgr_wheel(0.0, 1, 1).is_none());
        assert_eq!(
            sgr_wheel(-0.3, 2, 4).as_deref(),
            Some(b"\x1b[<64;2;4M".as_slice()),
            "小数 delta 也必须发出一格，否则触控板滚不动"
        );
    }

    #[test]
    fn gtk_buttons_map_to_xterm() {
        assert_eq!(gtk_button_to_sgr(1), Some(0));
        assert_eq!(gtk_button_to_sgr(2), Some(1));
        assert_eq!(gtk_button_to_sgr(3), Some(2));
        assert_eq!(gtk_button_to_sgr(4), None);
    }
}
