//! macOS 原生菜单栏（NSMenu）。
//!
//! 结构来自 `model::menu::main_menu()`，这里只负责把它翻译成 AppKit：
//! - 一个 `NSMenu` 主菜单 + 每条子菜单；
//! - 每个条目一个 `NSMenuItem`，`tag` 就是动作表下标；
//! - 点击回调把 `KeyAction` 丢进 channel，gpui 侧的 `watch_menu` 再执行。
//!
//! 为什么不用 gpui 自绘：macOS 的菜单栏归系统所有（屏幕顶部、Cmd 快捷键、
//! 辅助功能），自绘拿不到这些语义。Linux / Windows 才用自绘。

use std::ffi::CString;
use std::sync::{Mutex, OnceLock};

use futures::channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use objc::declare::ClassDecl;
use objc::runtime::{Class, Object, Sel};
use objc::{class, msg_send, sel, sel_impl};

use crate::frontend::gpui::model::menu::{main_menu, Menu, MenuEntry, MenuKey};
use crate::frontend::gpui::model::shortcuts::KeyAction;

/// 动作表：菜单项的 `tag` 是这里的下标。
static ACTIONS: OnceLock<Mutex<Vec<KeyAction>>> = OnceLock::new();
/// 点击回调 → gpui 的桥。只有一个窗口时也够用。
static SENDER: OnceLock<Mutex<Option<UnboundedSender<KeyAction>>>> = OnceLock::new();

fn actions() -> &'static Mutex<Vec<KeyAction>> {
    ACTIONS.get_or_init(|| Mutex::new(Vec::new()))
}

fn sender() -> &'static Mutex<Option<UnboundedSender<KeyAction>>> {
    SENDER.get_or_init(|| Mutex::new(None))
}

/// NSEventModifierFlags 的位值（AppKit 头文件里的常量）。
const MOD_COMMAND: u64 = 1 << 20;
const MOD_SHIFT: u64 = 1 << 17;
const MOD_OPTION: u64 = 1 << 19;
const MOD_CONTROL: u64 = 1 << 18;

/// 方向键在 `keyEquivalent` 里的私有区字符。
fn key_equivalent(key: &MenuKey) -> Option<String> {
    let character = match key.key.as_str() {
        "up" => "\u{F700}".to_string(),
        "down" => "\u{F701}".to_string(),
        "left" => "\u{F702}".to_string(),
        "right" => "\u{F703}".to_string(),
        other => other.to_string(),
    };
    if character.is_empty() {
        return None;
    }
    Some(character)
}

fn ns_string(value: &str) -> *mut Object {
    let c = CString::new(value).unwrap_or_default();
    unsafe { msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()] }
}

/// 菜单项的 target：一个只有一个方法的 NSObject 子类。
extern "C" fn menu_action(_this: &Object, _: Sel, sender_item: *mut Object) {
    let tag: isize = unsafe { msg_send![sender_item, tag] };
    let action = actions()
        .lock()
        .ok()
        .and_then(|actions| actions.get(tag.max(0) as usize).copied());
    if let Some(action) = action {
        if let Ok(guard) = sender().lock() {
            if let Some(sender) = guard.as_ref() {
                let _ = sender.unbounded_send(action);
            }
        }
    }
}

fn target_class() -> &'static Class {
    if let Some(class) = Class::get("MuxtermMenuTarget") {
        return class;
    }
    let mut declaration =
        ClassDecl::new("MuxtermMenuTarget", class!(NSObject)).expect("MuxtermMenuTarget");
    unsafe {
        declaration.add_method(
            sel!(menuAction:),
            menu_action as extern "C" fn(&Object, Sel, *mut Object),
        );
    }
    declaration.register()
}

fn make_target() -> *mut Object {
    let class = target_class();
    unsafe {
        let target: *mut Object = msg_send![class, alloc];
        let target: *mut Object = msg_send![target, init];
        target
    }
}

fn add_item(menu: *mut Object, item: *mut Object) {
    unsafe {
        let _: () = msg_send![menu, addItem: item];
    }
}

fn add_separator(menu: *mut Object) {
    unsafe {
        let separator: *mut Object = msg_send![class!(NSMenuItem), separatorItem];
        add_item(menu, separator);
    }
}

/// 一条子菜单（`title` 为空的第一项是应用菜单）。
fn build_submenu(spec: &Menu, target: *mut Object, actions: &mut Vec<KeyAction>) -> *mut Object {
    unsafe {
        let menu: *mut Object = msg_send![class!(NSMenu), alloc];
        let menu: *mut Object = msg_send![menu, initWithTitle: ns_string(&spec.title)];

        for entry in &spec.entries {
            match entry {
                MenuEntry::Separator => add_separator(menu),
                MenuEntry::Item(item) => {
                    let menu_item: *mut Object = msg_send![class!(NSMenuItem), alloc];
                    let menu_item: *mut Object = msg_send![menu_item, initWithTitle: ns_string(&item.label) action: std::ptr::null::<Object>() keyEquivalent: ns_string("")];
                    match item.action {
                        Some(action) => {
                            // tag 指向动作表：回调只传 tag，不传指针。
                            let tag = actions.len() as isize;
                            actions.push(action);
                            let _: () = msg_send![menu_item, setTag: tag];
                            let _: () = msg_send![menu_item, setTarget: target];
                            let _: () = msg_send![menu_item, setAction: sel!(menuAction:)];
                            if let Some(key) = item.key.as_ref() {
                                if let Some(equivalent) = key_equivalent(key) {
                                    let _: () = msg_send![
                                        menu_item,
                                        setKeyEquivalent: ns_string(&equivalent)
                                    ];
                                    let mut mask = 0u64;
                                    if key.command {
                                        mask |= MOD_COMMAND;
                                    }
                                    if key.shift {
                                        mask |= MOD_SHIFT;
                                    }
                                    if key.option {
                                        mask |= MOD_OPTION;
                                    }
                                    if key.control {
                                        mask |= MOD_CONTROL;
                                    }
                                    let _: () = msg_send![
                                        menu_item,
                                        setKeyEquivalentModifierMask: mask
                                    ];
                                }
                            }
                        }
                        None => {
                            // Swift 有、gpui 还没接的动作：列出但禁用。
                            let _: () = msg_send![menu_item, setEnabled: false];
                        }
                    }
                    add_item(menu, menu_item);
                }
            }
        }
        menu
    }
}

/// 安装主菜单。返回菜单动作的接收端；没有 AppKit 时返回 `None`。
pub fn install_menu() -> Option<UnboundedReceiver<KeyAction>> {
    let (tx, rx) = unbounded();
    if let Ok(mut guard) = sender().lock() {
        *guard = Some(tx);
    }

    let mut table = Vec::new();
    let target = make_target();
    unsafe {
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        if app.is_null() {
            return None;
        }
        let main: *mut Object = msg_send![class!(NSMenu), alloc];
        let main: *mut Object = msg_send![main, init];
        for spec in main_menu() {
            let submenu = build_submenu(&spec, target, &mut table);
            let item: *mut Object = msg_send![class!(NSMenuItem), alloc];
            let item: *mut Object = msg_send![item, init];
            let _: () = msg_send![item, setSubmenu: submenu];
            add_item(main, item);
        }
        let _: () = msg_send![app, setMainMenu: main];
    }

    if let Ok(mut guard) = actions().lock() {
        *guard = table;
    }
    Some(rx)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 方向键要翻成 AppKit 的私有区字符，其它键原样。
    #[test]
    fn arrow_keys_map_to_key_equivalents() {
        let up = MenuKey {
            key: "up".to_string(),
            command: true,
            shift: false,
            option: true,
            control: false,
        };
        assert_eq!(key_equivalent(&up).as_deref(), Some("\u{F700}"));
        let letter = MenuKey {
            key: "t".to_string(),
            command: true,
            shift: false,
            option: false,
            control: false,
        };
        assert_eq!(key_equivalent(&letter).as_deref(), Some("t"));
        let empty = MenuKey {
            key: String::new(),
            command: true,
            shift: false,
            option: false,
            control: false,
        };
        assert_eq!(key_equivalent(&empty), None);
    }
}
