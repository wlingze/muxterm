//! 滚轮策略（W21）：主屏滚 VTE 历史，alt-screen 把滚轮转成 CSI 方向键。
//!
//! 应用开了 1000/1002/1003 时（Grok 常见：主屏 + SGR 鼠标），滚轮必须
//! 写成 SGR 报告交给 pane，不能滚本地历史。
//! 对齐 iTerm2 alternate-mouse-scroll：无 mouse 的 alt-screen 下滚轮 = Up/Down。
//! 纯函数，无 GTK 依赖，便于单测。

use crate::core::protocol::terminal::mouse::{sgr_wheel, wheel_notches};

/// 每「格」滚动的行数（仅 CSI 方向键路径）。
pub const WHEEL_LINES_PER_NOTCH: i32 = 3;

/// 滚轮动作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WheelAction {
    /// 主屏且无 mouse reporting：动 VTE 视口，不 send-keys。
    ScrollHistory { lines: i32 },
    /// mouse reporting 或 alt-screen：发给 pane 的字节。
    SendToApp { bytes: Vec<u8> },
}

/// `delta_y < 0` = 用户想看上面（历史更早 / Up）。
/// `delta_y == 0` → None。
pub fn wheel_action(
    alternate_screen: bool,
    mouse_reporting: bool,
    delta_y: f64,
    cell: (u16, u16),
) -> Option<WheelAction> {
    if delta_y == 0.0 {
        return None;
    }
    if mouse_reporting {
        let bytes = sgr_wheel(delta_y, cell.0, cell.1)?;
        return Some(WheelAction::SendToApp { bytes });
    }
    let notches = wheel_notches(delta_y) as i32;
    let signed = if delta_y < 0.0 { -notches } else { notches };
    let lines = signed * WHEEL_LINES_PER_NOTCH;
    if alternate_screen {
        // CSI CUU/CUD，不是 ESC O A。
        let byte = if lines < 0 { b'A' } else { b'B' };
        let mut bytes = Vec::new();
        for _ in 0..lines.unsigned_abs() {
            bytes.extend_from_slice(&[0x1b, b'[', byte]);
        }
        Some(WheelAction::SendToApp { bytes })
    } else {
        Some(WheelAction::ScrollHistory { lines })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// W21a：主屏向上滚 = ScrollHistory 3 行。
    #[test]
    fn wheel_action_primary_scrolls_history() {
        assert_eq!(
            wheel_action(false, false, -1.0, (1, 1)),
            Some(WheelAction::ScrollHistory { lines: -3 })
        );
        assert_eq!(
            wheel_action(false, false, 1.0, (1, 1)),
            Some(WheelAction::ScrollHistory { lines: 3 })
        );
    }

    /// W21a：alt-screen 向上 = CSI A，出现次数 = 行数。
    #[test]
    fn wheel_action_alt_screen_sends_csi_arrows() {
        let up = wheel_action(true, false, -1.0, (1, 1)).expect("向上应有动作");
        match up {
            WheelAction::SendToApp { bytes } => {
                assert!(bytes.starts_with(b"\x1b[A"), "必须 CSI A 开头: {bytes:?}");
                assert_eq!(bytes.len(), 9, "一格 = 3 行 = 3 次 CSI A");
            }
            other => panic!("alt-screen 向上必须是 SendToApp: {other:?}"),
        }
        let down = wheel_action(true, false, 1.0, (1, 1)).expect("向下应有动作");
        match down {
            WheelAction::SendToApp { bytes } => {
                assert!(bytes.starts_with(b"\x1b[B"), "必须 CSI B 开头: {bytes:?}");
            }
            other => panic!("alt-screen 向下必须是 SendToApp: {other:?}"),
        }
        // 两格 = 两次。
        let two = wheel_action(true, false, -2.0, (1, 1)).expect("两格应有动作");
        match two {
            WheelAction::SendToApp { bytes } => {
                assert_eq!(bytes.len(), 18, "两格 = 6 行 = 6 次 CSI A");
            }
            other => panic!("两格必须是 SendToApp: {other:?}"),
        }
    }

    /// 0027.log：Grok 主屏 + mouse_all/sgr，滚轮必须 SGR 穿透，不能滚本地历史。
    #[test]
    fn wheel_action_mouse_reporting_sends_sgr_even_on_primary() {
        let up = wheel_action(false, true, -1.0, (12, 8)).expect("mouse 向上");
        match up {
            WheelAction::SendToApp { bytes } => {
                assert_eq!(bytes, b"\x1b[<64;12;8M");
            }
            other => panic!("mouse reporting 必须 SendToApp: {other:?}"),
        }
        let down = wheel_action(true, true, 1.0, (3, 4)).expect("mouse 优先于 alt CSI");
        match down {
            WheelAction::SendToApp { bytes } => {
                assert_eq!(bytes, b"\x1b[<65;3;4M", "同时开 mouse 时发 SGR 不是 CSI B");
            }
            other => panic!("{other:?}"),
        }
    }

    /// W21a：delta_y == 0 → None。
    #[test]
    fn wheel_action_zero_delta_is_none() {
        assert_eq!(wheel_action(false, false, 0.0, (1, 1)), None);
        assert_eq!(wheel_action(true, true, 0.0, (1, 1)), None);
    }
}
