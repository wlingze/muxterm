//! macOS 快捷键边界：在菜单/IME 之前按真实键码解析，并且只投递一次。
//! Apple 的 local monitor 在主线程调用；返回 nil 表示已消费，普通打字原样放行。
//! https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/EventOverview/MonitoringEvents/MonitoringEvents.html

use crate::frontend::gpui::model::{
    physical_key,
    shortcut_editor::ShortcutMap,
    shortcuts::{KeyAction, KeyChord, ShortcutCommand},
};
use objc::runtime::Object;
use objc::{class, msg_send, sel, sel_impl};
use std::cell::RefCell;

#[derive(Default)]
struct State {
    monitor: *mut Object,
    bindings: ShortcutMap,
    modal: bool,
    recording: bool,
    last: Option<(u16, String)>,
}
thread_local! { static STATE: RefCell<State> = RefCell::new(State::default()); }

pub fn configure(bindings: &ShortcutMap, modal: bool, recording: bool) {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.bindings = bindings.clone();
        state.modal = modal;
        state.recording = recording;
    });
}

fn text(value: *mut Object) -> String {
    if value.is_null() {
        return String::new();
    }
    unsafe {
        let bytes: *const std::os::raw::c_char = msg_send![value, UTF8String];
        if bytes.is_null() {
            String::new()
        } else {
            std::ffi::CStr::from_ptr(bytes)
                .to_string_lossy()
                .into_owned()
        }
    }
}

/// 只处理动作修饰键；普通字母、IME 组词完全不经快捷键表。
/// 返回的命令经原生菜单共用的通道回到 GpuiApp，不碰 Core。
pub fn resolve_native(
    code: u16,
    flags: u64,
    layout: &str,
    settings: bool,
) -> Option<ShortcutCommand> {
    let key = physical_key::colemak_char(code).unwrap_or(layout);
    let chord = KeyChord::new(
        flags & (1 << 20) != 0,
        flags & (1 << 17) != 0,
        flags & (1 << 19) != 0,
        flags & (1 << 18) != 0,
        key,
    );
    STATE.with(|state| {
        let state = state.borrow();
        if settings && state.recording {
            return Some(ShortcutCommand::Record(chord));
        }
        if !chord.command && !chord.option && !chord.control {
            return None;
        }
        let action = state.bindings.resolve(&chord)?;
        if settings {
            return match action {
                KeyAction::Quit => Some(ShortcutCommand::Action(action)),
                KeyAction::ClosePane | KeyAction::CloseWindow => {
                    Some(ShortcutCommand::CloseSettings)
                }
                // 设置窗口独占，禁止菜单 key equivalent 把录入动作发给终端。
                _ => None,
            };
        }
        if state.modal && !matches!(action, KeyAction::Quit | KeyAction::OpenSettings) {
            return None;
        }
        Some(ShortcutCommand::Action(action))
    })
}

pub fn install() {
    STATE.with(|state| {
        if !state.borrow().monitor.is_null() { return; }
        unsafe {
            let handler = block::ConcreteBlock::new(|event: *mut Object| -> *mut Object {
                if event.is_null() { return event; }
                let code: u16 = msg_send![event, keyCode];
                let flags: u64 = msg_send![event, modifierFlags];
                let chars: *mut Object = msg_send![event, charactersIgnoringModifiers];
                let layout = text(chars);
                STATE.with(|state| state.borrow_mut().last = Some((code, layout.clone())));
                let window: *mut Object = msg_send![event, window];
                let settings = if window.is_null() { false } else {
                    let title: *mut Object = msg_send![window, title];
                    text(title).ends_with(" — Settings")
                };
                if let Some(command) = resolve_native(code, flags, &layout, settings) {
                    if super::macos_menu::send_command(command) { return std::ptr::null_mut(); }
                }
                event
            }).copy();
            // AppKit 持有 handler 的副本；本地副本正常释放，不泄漏 block。
            let monitor: *mut Object = msg_send![class!(NSEvent), addLocalMonitorForEventsMatchingMask: (1u64 << 10) handler: &*handler];
            state.borrow_mut().monitor = monitor;
        }
    });
}

pub fn uninstall() {
    STATE.with(|state| {
        let monitor = std::mem::replace(&mut state.borrow_mut().monitor, std::ptr::null_mut());
        if !monitor.is_null() {
            unsafe {
                let _: () = msg_send![class!(NSEvent), removeMonitor: monitor];
            }
        }
    });
}

/// gpui 的平台回调兜底也使用同一 Colemak 锚点；不读取陈旧的 NSApp.currentEvent。
pub fn physical_key_for_event(expected: &str) -> Option<&'static str> {
    STATE.with(|state| {
        let state = state.borrow();
        let (code, layout) = state.last.as_ref()?;
        if !layout.eq_ignore_ascii_case(expected) {
            return None;
        }
        physical_key::colemak_char(*code)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真机事件边界与纯逻辑必须采用相同的 Colemak 锚点，布局不能改变动作。
    #[test]
    fn native_dispatch_keeps_all_default_hand_positions() {
        configure(&ShortcutMap::default(), false, false);
        for (code, colemak, qwerty, action) in [
            (5, "d", "g", KeyAction::SplitHorizontal),
            (2, "s", "d", KeyAction::SplitVertical),
            (3, "t", "f", KeyAction::NewTab),
            (15, "p", "r", KeyAction::QuickConnect),
            (14, "f", "e", KeyAction::SearchWorkspace),
        ] {
            for layout in [colemak, qwerty] {
                assert_eq!(
                    resolve_native(code, 1 << 20, layout, false),
                    Some(ShortcutCommand::Action(action))
                );
            }
        }
        assert_eq!(
            resolve_native(5, 0, "d", false),
            None,
            "普通文字不归快捷键处理"
        );
    }

    #[test]
    fn recording_never_executes_the_action_and_modals_keep_their_keys() {
        configure(&ShortcutMap::default(), false, true);
        assert_eq!(
            resolve_native(5, 1 << 20, "d", true),
            Some(ShortcutCommand::Record(KeyChord::command("d")))
        );
        configure(&ShortcutMap::default(), true, false);
        assert_eq!(resolve_native(5, 1 << 20, "d", false), None);
        assert_eq!(resolve_native(5, 1 << 20, "d", true), None);
    }
}
