//! 滚轮策略（W21）：主屏滚 VTE 历史，alt-screen 把滚轮转成 CSI 方向键。
//!
//! 对齐 iTerm2 alternate-mouse-scroll：alt-screen 下滚轮 = Up/Down。
//! 纯函数，无 GTK 依赖，便于单测。

/// 每「格」滚动的行数。
pub const WHEEL_LINES_PER_NOTCH: i32 = 3;

/// 滚轮动作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WheelAction {
    /// 主屏：动 VTE 视口，不 send-keys。
    ScrollHistory { lines: i32 },
    /// alt-screen：发给 pane 的 CSI 方向键（CUU/CUD）。
    SendToApp { bytes: Vec<u8> },
}

/// `delta_y < 0` = 用户想看上面（历史更早 / Up）。
/// 每「格」3 行。`delta_y == 0` → None。
pub fn wheel_action(alternate_screen: bool, delta_y: f64) -> Option<WheelAction> {
    if delta_y == 0.0 {
        return None;
    }
    let notches = delta_y.round() as i32;
    let lines = notches * WHEEL_LINES_PER_NOTCH;
    if alternate_screen {
        // CSI CUU/CUD，不是 ESC O A。
        let byte = if lines < 0 { b'A' } else { b'B' };
        let mut bytes = Vec::new();
        for _ in 0..lines.abs() {
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
            wheel_action(false, -1.0),
            Some(WheelAction::ScrollHistory { lines: -3 })
        );
        assert_eq!(
            wheel_action(false, 1.0),
            Some(WheelAction::ScrollHistory { lines: 3 })
        );
    }

    /// W21a：alt-screen 向上 = CSI A，出现次数 = 行数。
    #[test]
    fn wheel_action_alt_screen_sends_csi_arrows() {
        let up = wheel_action(true, -1.0).expect("向上应有动作");
        match up {
            WheelAction::SendToApp { bytes } => {
                assert!(bytes.starts_with(b"\x1b[A"), "必须 CSI A 开头: {bytes:?}");
                assert_eq!(bytes.len(), 9, "一格 = 3 行 = 3 次 CSI A");
            }
            other => panic!("alt-screen 向上必须是 SendToApp: {other:?}"),
        }
        let down = wheel_action(true, 1.0).expect("向下应有动作");
        match down {
            WheelAction::SendToApp { bytes } => {
                assert!(bytes.starts_with(b"\x1b[B"), "必须 CSI B 开头: {bytes:?}");
            }
            other => panic!("alt-screen 向下必须是 SendToApp: {other:?}"),
        }
        // 两格 = 两次。
        let two = wheel_action(true, -2.0).expect("两格应有动作");
        match two {
            WheelAction::SendToApp { bytes } => {
                assert_eq!(bytes.len(), 18, "两格 = 6 行 = 6 次 CSI A");
            }
            other => panic!("两格必须是 SendToApp: {other:?}"),
        }
    }

    /// W21a：delta_y == 0 → None。
    #[test]
    fn wheel_action_zero_delta_is_none() {
        assert_eq!(wheel_action(false, 0.0), None);
        assert_eq!(wheel_action(true, 0.0), None);
    }
}
