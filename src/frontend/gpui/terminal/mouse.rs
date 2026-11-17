//! 鼠标上报：模式跟踪与 xterm 编码。
//!
//! macOS 前端由 SwiftTerm 负责这部分；gpui 自带 VT，所以规则得自己写。协议
//! 只有两套要发：经典 X10（`ESC [ M` + 32 偏移的三字节）与 SGR（`ESC [ < … M`
//! 或 `m`）。SGR 能表达超过 223 列/行，也能区分按下与释放，因此只要应用开了
//! 1006 就一律走 SGR。
//!
//! 跟踪的四个模式位：
//! - 9：X10，只在按下时上报；
//! - 1000：普通，按下 / 释放，不含移动；
//! - 1002：按键拖动时上报移动；
//! - 1003：任何移动都上报。
//!
//! 1006 只管编码格式，不改变上报时机。

/// 应用要求的鼠标上报模式。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MouseMode {
    /// 不上报（默认）。
    #[default]
    Off,
    /// DECSET 9：只报按下。
    X10,
    /// DECSET 1000：按下与释放。
    Normal,
    /// DECSET 1002：按住拖动也报。
    Button,
    /// DECSET 1003：所有移动都报。
    Any,
}

impl MouseMode {
    /// DECSET 模式的编号，便于测试和日志。
    pub fn mode_number(self) -> Option<u16> {
        match self {
            MouseMode::Off => None,
            MouseMode::X10 => Some(9),
            MouseMode::Normal => Some(1000),
            MouseMode::Button => Some(1002),
            MouseMode::Any => Some(1003),
        }
    }

    /// DECSET/DECRST 到模式。只接受这四个编号。
    pub fn from_mode_number(mode: u16) -> Option<MouseMode> {
        match mode {
            9 => Some(MouseMode::X10),
            1000 => Some(MouseMode::Normal),
            1002 => Some(MouseMode::Button),
            1003 => Some(MouseMode::Any),
            _ => None,
        }
    }

    /// 该模式是否报这项事件。
    pub fn accepts(self, kind: MouseEventKind) -> bool {
        match self {
            MouseMode::Off => false,
            // X10 只报按下；释放由协议里的 3 号按钮隐含，不上报。
            MouseMode::X10 => matches!(kind, MouseEventKind::Press(_)),
            MouseMode::Normal => matches!(
                kind,
                MouseEventKind::Press(_) | MouseEventKind::Release(_) | MouseEventKind::Wheel(_, _)
            ),
            MouseMode::Button => matches!(
                kind,
                MouseEventKind::Press(_)
                    | MouseEventKind::Release(_)
                    | MouseEventKind::Wheel(_, _)
                    | MouseEventKind::Drag(_)
            ),
            // 1003：所有移动都报，包括无按键的 hover。
            MouseMode::Any => true,
        }
    }
}

/// 鼠标按钮。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left = 0,
    Middle = 1,
    Right = 2,
}

/// 指针发出的事件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseEventKind {
    Press(Button),
    Release(Button),
    /// 按住某键拖动。
    Drag(Button),
    /// 无按键移动。
    Move,
    /// 滚轮：`up` 为 true 表示向上滚，`amount` 是格数。
    Wheel(bool, u8),
}

/// 鼠标事件的修饰键。xterm 用 4 / 8 / 16 三个位表示 Shift / Meta(Alt) / Control。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MouseModifiers {
    pub shift: bool,
    pub alt: bool,
    pub control: bool,
}

impl MouseModifiers {
    /// 事件码要加的位。
    pub fn bits(self) -> u16 {
        u16::from(self.shift) * 4 + u16::from(self.alt) * 8 + u16::from(self.control) * 16
    }
}

/// 把一次指针事件编码成要写进 pane 的字节。
///
/// `col` / `row` 是 0 基格子坐标；两套协议都按 1 基发送。
pub fn encode(
    kind: MouseEventKind,
    col: usize,
    row: usize,
    mode: MouseMode,
    sgr: bool,
) -> Option<Vec<u8>> {
    encode_with_modifiers(kind, col, row, mode, sgr, MouseModifiers::default())
}

/// 同上，但带上修饰键（xterm 的 4/8/16 位）。
pub fn encode_with_modifiers(
    kind: MouseEventKind,
    col: usize,
    row: usize,
    mode: MouseMode,
    sgr: bool,
    modifiers: MouseModifiers,
) -> Option<Vec<u8>> {
    if !mode.accepts(kind) {
        return None;
    }
    let bits = modifiers.bits();
    let (code, final_byte) = match kind {
        MouseEventKind::Press(button) => (button as u16 + bits, b'M'),
        MouseEventKind::Release(button) => {
            if sgr {
                (button as u16 + bits, b'm')
            } else {
                (3 + bits, b'M')
            }
        }
        MouseEventKind::Drag(button) => (32 + button as u16 + bits, b'M'),
        MouseEventKind::Move => (35 + bits, b'M'),
        MouseEventKind::Wheel(up, amount) => {
            // 一次滚动事件可以带多格：一格一条，保持与 xterm 一致。
            if amount == 0 {
                return None;
            }
            let button = if up { 64 } else { 65 };
            let mut out = Vec::new();
            for _ in 0..amount {
                out.extend(encode_single(button + bits, b'M', col, row, sgr)?);
            }
            return Some(out);
        }
    };
    encode_single(code, final_byte, col, row, sgr)
}

fn encode_single(code: u16, final_byte: u8, col: usize, row: usize, sgr: bool) -> Option<Vec<u8>> {
    let x = col.saturating_add(1);
    let y = row.saturating_add(1);
    if sgr {
        Some(format!("\x1b[<{code};{x};{y}{}", final_byte as char).into_bytes())
    } else {
        // X10 的坐标上限是 223（32 + 191）；超出的格子上报没有意义。
        let bx = 32 + u8::try_from(x).ok().filter(|x| *x <= 191)?;
        let by = 32 + u8::try_from(y).ok().filter(|y| *y <= 191)?;
        Some(vec![0x1b, b'[', b'M', 32 + code as u8, bx, by])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// DECSET 编号与模式一一对应。
    #[test]
    fn mode_numbers_round_trip() {
        for mode in [
            MouseMode::X10,
            MouseMode::Normal,
            MouseMode::Button,
            MouseMode::Any,
        ] {
            assert_eq!(
                MouseMode::from_mode_number(mode.mode_number().unwrap()),
                Some(mode)
            );
        }
        assert_eq!(MouseMode::from_mode_number(1005), None);
        assert_eq!(MouseMode::Off.mode_number(), None);
    }

    /// 关掉上报时一个字节都不发。
    #[test]
    fn off_mode_sends_nothing() {
        assert_eq!(
            encode(
                MouseEventKind::Press(Button::Left),
                0,
                0,
                MouseMode::Off,
                false
            ),
            None
        );
        assert_eq!(
            encode(MouseEventKind::Wheel(true, 1), 0, 0, MouseMode::Off, true),
            None
        );
    }

    /// X10 只报按下：释放与移动都不发。
    #[test]
    fn x10_only_reports_presses() {
        let press = encode(
            MouseEventKind::Press(Button::Left),
            0,
            0,
            MouseMode::X10,
            false,
        )
        .unwrap();
        assert_eq!(press, b"\x1b[M\x20\x21\x21".to_vec());
        assert_eq!(
            encode(
                MouseEventKind::Release(Button::Left),
                0,
                0,
                MouseMode::X10,
                false
            ),
            None
        );
        assert_eq!(
            encode(MouseEventKind::Move, 0, 0, MouseMode::X10, false),
            None
        );
    }

    /// SGR 按下 / 释放：坐标 1 基，释放用 `m`。
    #[test]
    fn sgr_press_and_release_use_one_based_cells() {
        assert_eq!(
            encode(
                MouseEventKind::Press(Button::Left),
                4,
                2,
                MouseMode::Normal,
                true
            )
            .unwrap(),
            b"\x1b[<0;5;3M".to_vec()
        );
        assert_eq!(
            encode(
                MouseEventKind::Release(Button::Left),
                4,
                2,
                MouseMode::Normal,
                true
            )
            .unwrap(),
            b"\x1b[<0;5;3m".to_vec()
        );
        // 右键是 2。
        assert_eq!(
            encode(
                MouseEventKind::Press(Button::Right),
                0,
                0,
                MouseMode::Normal,
                true
            )
            .unwrap(),
            b"\x1b[<2;1;1M".to_vec()
        );
    }

    /// 经典编码里没有按钮号：释放一律发 3。
    #[test]
    fn legacy_release_uses_button_three() {
        assert_eq!(
            encode(
                MouseEventKind::Release(Button::Right),
                0,
                0,
                MouseMode::Normal,
                false
            )
            .unwrap(),
            b"\x1b[M\x23\x21\x21".to_vec()
        );
    }

    /// 拖动带 32 位移；1000 不收移动，1002 收。
    #[test]
    fn drag_needs_button_mode() {
        assert_eq!(
            encode(
                MouseEventKind::Drag(Button::Left),
                1,
                1,
                MouseMode::Normal,
                true
            ),
            None
        );
        assert_eq!(
            encode(
                MouseEventKind::Drag(Button::Left),
                1,
                1,
                MouseMode::Button,
                true
            )
            .unwrap(),
            b"\x1b[<32;2;2M".to_vec()
        );
        // 无按键移动要 1003。
        assert_eq!(
            encode(MouseEventKind::Move, 1, 1, MouseMode::Button, true),
            None
        );
        assert_eq!(
            encode(MouseEventKind::Move, 1, 1, MouseMode::Any, true).unwrap(),
            b"\x1b[<35;2;2M".to_vec()
        );
    }

    /// 滚轮：64 上 / 65 下，一格一条，`amount` 决定条数。
    #[test]
    fn wheel_repeats_once_per_line() {
        assert_eq!(
            encode(
                MouseEventKind::Wheel(true, 1),
                0,
                0,
                MouseMode::Normal,
                true
            )
            .unwrap(),
            b"\x1b[<64;1;1M".to_vec()
        );
        assert_eq!(
            encode(
                MouseEventKind::Wheel(false, 2),
                0,
                0,
                MouseMode::Normal,
                true
            )
            .unwrap(),
            b"\x1b[<65;1;1M\x1b[<65;1;1M".to_vec()
        );
        assert_eq!(
            encode(
                MouseEventKind::Wheel(true, 0),
                0,
                0,
                MouseMode::Normal,
                true
            ),
            None
        );
    }

    /// 经典编码在 223 列/行之后无法表达：宁可不发，也不发错坐标。
    #[test]
    fn legacy_encoding_refuses_out_of_range_cells() {
        assert_eq!(
            encode(
                MouseEventKind::Press(Button::Left),
                190,
                0,
                MouseMode::Normal,
                false
            )
            .is_some(),
            true,
            "191 列仍可表达"
        );
        assert_eq!(
            encode(
                MouseEventKind::Press(Button::Left),
                191,
                0,
                MouseMode::Normal,
                false
            ),
            None
        );
        // SGR 没有这个上限。
        assert!(encode(
            MouseEventKind::Press(Button::Left),
            500,
            0,
            MouseMode::Normal,
            true
        )
        .is_some());
    }

    /// 修饰键按 xterm 的 4 / 8 / 16 位加在事件码上：Shift+Ctrl 左键 = 20。
    #[test]
    fn modifiers_add_the_xterm_bits() {
        assert_eq!(MouseModifiers::default().bits(), 0);
        assert_eq!(
            MouseModifiers {
                shift: true,
                alt: false,
                control: true,
            }
            .bits(),
            20
        );
        assert_eq!(
            encode_with_modifiers(
                MouseEventKind::Press(Button::Left),
                2,
                3,
                MouseMode::Normal,
                true,
                MouseModifiers {
                    shift: true,
                    alt: false,
                    control: true,
                },
            )
            .unwrap(),
            b"\x1b[<20;3;4M".to_vec()
        );
        // 拖动 = 32 + 键号 + 修饰位；无按键移动 = 32 + 3 + 修饰位。
        assert_eq!(
            encode_with_modifiers(
                MouseEventKind::Drag(Button::Left),
                0,
                0,
                MouseMode::Button,
                true,
                MouseModifiers {
                    shift: false,
                    alt: true,
                    control: false,
                },
            )
            .unwrap(),
            b"\x1b[<40;1;1M".to_vec()
        );
        assert_eq!(
            encode_with_modifiers(
                MouseEventKind::Move,
                0,
                0,
                MouseMode::Any,
                true,
                MouseModifiers {
                    shift: false,
                    alt: true,
                    control: false,
                },
            )
            .unwrap(),
            b"\x1b[<43;1;1M".to_vec()
        );
    }
}
