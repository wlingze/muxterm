//! macOS 物理键位记录器。
//!
//! 为什么需要：`Keystroke.key` 是「当前键盘布局产出的字符」。Colemak / Dvorak
//! 把字母挪了位置，按字符匹配的 Cmd 组合就会漂到别的动作上 —— 用户报的
//! 「cmd-d / cmd-t / cmd-s 全失败」就是这么来的（详见 `model/physical_key.rs`）。
//!
//! 取物理键位有两条路，都不能用：
//! - `NSApp.currentEvent`：在视图的 `performKeyEquivalent:` 回调里它指向的是
//!   `NSEventTypeAppKitDefined`（type 13），对它发 `keyCode` 会抛
//!   `NSInternalInconsistencyException`（本机实测）。菜单回调里同理。
//! - gpui 的 `Keystroke`：只有字符，没有键码。
//!
//! 所以用本地事件监视器：`NSEvent` 的 keyDown 监视器在窗口 / 输入法之前拿到
//! **真正的那个 NSEvent**，把 `keyCode` + 布局字符记下来，事件原样放行。
//! 随后 gpui 的 `handle_key` 用记录值换算成 QWERTY 字符。
//!
//! 只记录、不改派发：快捷键仍然由 gpui 的 `handle_key` 执行一次，输入法、
//! 菜单、文本编辑的既有行为一律不动。

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};

use objc::runtime::Object;
use objc::{class, msg_send, sel, sel_impl};

thread_local! {
    /// 最近一次派发的按键：`(虚拟键码, 布局字符)`。
    static LAST_KEY: RefCell<Option<(u16, String)>> = const { RefCell::new(None) };
}

static INSTALLED: AtomicBool = AtomicBool::new(false);

/// `NSEventMaskKeyDown`（`NSKeyDownMask`，AppKit 头文件里的位值）。
const MASK_KEY_DOWN: u64 = 1 << 10;

/// NSString → String（keyDown 的布局字符都是短 ASCII / 单字符）。
fn ns_string_to_rust(value: *mut Object) -> String {
    if value.is_null() {
        return String::new();
    }
    unsafe {
        let utf8: *const std::os::raw::c_char = msg_send![value, UTF8String];
        if utf8.is_null() {
            return String::new();
        }
        std::ffi::CStr::from_ptr(utf8)
            .to_string_lossy()
            .into_owned()
    }
}

/// 安装一次性 keyDown 监视器。必须在主线程、NSApp 起来之后调用。
///
/// 无头测试不装（没有 AppKit 就没有 keyDown 事件），此时
/// [`physical_key_for_event`] 恒返回 `None`，行为与只按布局字符匹配时一致。
pub fn install() {
    if INSTALLED.swap(true, Ordering::Relaxed) {
        return;
    }
    unsafe {
        let handler = block::ConcreteBlock::new(|event: *mut Object| -> *mut Object {
            if !event.is_null() {
                let code: u16 = msg_send![event, keyCode];
                let chars: *mut Object = msg_send![event, charactersIgnoringModifiers];
                let text = ns_string_to_rust(chars);
                LAST_KEY.with(|slot| {
                    *slot.borrow_mut() = Some((code, text));
                });
            }
            event
        });
        let handler = handler.copy();
        let _: *mut Object = msg_send![
            class!(NSEvent),
            addLocalMonitorForEventsMatchingMask: MASK_KEY_DOWN
            handler: &*handler
        ];
        // AppKit 只 retain block 的副本直到监视器移除；这个进程级监视器永不
        // 移除，所以自己也不能释放。
        std::mem::forget(handler);
    }
}

/// 取「当前这条按键」的物理键位（US QWERTY 字符）。
///
/// `expected_layout_key` 是 gpui 已经解析出的布局字符。记录值与它对不上，
/// 说明监视器记的是别的按键（或根本没有 AppKit），返回 `None` 让调用方
/// 回落到布局字符 —— 宁可不猜，也不能把按键路由错。
pub fn physical_key_for_event(expected_layout_key: &str) -> Option<&'static str> {
    // 只读不清：监视器对每个 keyDown 都会重写，读两次拿到的是同一个按键；
    // 清掉反而会因为在别的分支里先读过一次而丢掉记录。
    LAST_KEY.with(|slot| {
        let (code, chars) = slot.borrow().clone()?;
        if !chars.eq_ignore_ascii_case(expected_layout_key) {
            return None;
        }
        crate::frontend::gpui::model::physical_key::us_char(code)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 监视器记录的键码要能翻成 QWERTY 字符；布局字符对不上就不猜。
    #[test]
    fn recorded_key_must_match_the_layout_char() {
        LAST_KEY.with(|slot| *slot.borrow_mut() = Some((2, "s".to_string())));
        // Colemak：物理 D（键码 2）产出 's'，换成 QWERTY 字符是 'd'。
        assert_eq!(physical_key_for_event("s"), Some("d"));
        // 只读不清：同一条按键再问一次仍然给同样的答案。
        assert_eq!(physical_key_for_event("s"), Some("d"));
    }

    #[test]
    fn mismatched_layout_char_is_not_guessed() {
        LAST_KEY.with(|slot| *slot.borrow_mut() = Some((2, "s".to_string())));
        assert_eq!(physical_key_for_event("d"), None);
        // 大小写不同但确实不是同一条按键，同样不能猜。
        assert_eq!(physical_key_for_event("D"), None);
    }

    #[test]
    fn unknown_key_code_has_no_qwerty_char() {
        LAST_KEY.with(|slot| *slot.borrow_mut() = Some((123, "left".to_string())));
        assert_eq!(physical_key_for_event("left"), None);
    }

    /// 大写布局字符（Shift 组合）也要认，不能因为大小写把快捷键丢掉。
    #[test]
    fn shift_uppercased_layout_char_still_matches() {
        LAST_KEY.with(|slot| *slot.borrow_mut() = Some((17, "G".to_string())));
        assert_eq!(physical_key_for_event("g"), Some("t"));
    }
}
