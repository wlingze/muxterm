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

use crate::frontend::gpui::model::menu::{
    localized_menu_with, Menu, MenuEntry, MenuKey, MenuState,
};
use crate::frontend::gpui::model::shortcuts::{KeyAction, ShortcutCommand};

/// 动作表：菜单项的 `tag` 是这里的下标。
static ACTIONS: OnceLock<Mutex<Vec<KeyAction>>> = OnceLock::new();
/// 主菜单是否已经装到 `NSApp`：没有装时（无头测试）刷新是纯空操作。
static INSTALLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// 点击回调 → gpui 的桥。只有一个窗口时也够用。
static SENDER: OnceLock<Mutex<Option<UnboundedSender<ShortcutCommand>>>> = OnceLock::new();

fn actions() -> &'static Mutex<Vec<KeyAction>> {
    ACTIONS.get_or_init(|| Mutex::new(Vec::new()))
}

fn sender() -> &'static Mutex<Option<UnboundedSender<ShortcutCommand>>> {
    SENDER.get_or_init(|| Mutex::new(None))
}

pub fn send_command(command: ShortcutCommand) -> bool {
    sender()
        .lock()
        .ok()
        .and_then(|sender| {
            sender
                .as_ref()
                .map(|sender| sender.unbounded_send(command).is_ok())
        })
        .unwrap_or(false)
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
        send_command(ShortcutCommand::Action(action));
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
        // tag = -1：刷新状态时跳过所有分隔线（默认 0 会撞上第一项）。
        let _: () = msg_send![separator, setTag: -1isize];
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
                    // tag 指向动作表：回调只传 tag，不传指针。
                    let tag = actions.len() as isize;
                    actions.push(item.action);
                    let _: () = msg_send![menu_item, setTag: tag];
                    let _: () = msg_send![menu_item, setTarget: target];
                    let _: () = msg_send![menu_item, setAction: sel!(menuAction:)];
                    // 勾选 / 置灰来自 model/menu.rs 的运行时状态。
                    let _: () = msg_send![
                        menu_item,
                        setState: if item.checked { 1isize } else { 0isize }
                    ];
                    let _: () = msg_send![menu_item, setEnabled: item.enabled];
                    if let Some(key) = item.key.as_ref() {
                        if let Some(equivalent) = key_equivalent(key) {
                            let _: () =
                                msg_send![menu_item, setKeyEquivalent: ns_string(&equivalent)];
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
                            let _: () = msg_send![menu_item, setKeyEquivalentModifierMask: mask];
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
pub fn install_menu() -> Option<UnboundedReceiver<ShortcutCommand>> {
    install_menu_with_state(MenuState::default())
}

/// 安装主菜单，并把运行时状态（勾选 / 置灰）带进菜单。
///
/// 只建一次：之后用 [`refresh_menu`] 原地改 item 的 state / enabled，
/// 不重建 NSMenu（重建会把 `watch_menu` 的 channel 换掉，菜单点击就断了）。
pub fn install_menu_with_state(state: MenuState) -> Option<UnboundedReceiver<ShortcutCommand>> {
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
        // 自己管 enabled：默认的 autoenable 会用 validateMenuItem 覆盖我们
        // 设的 enabled 状态，Copy 就会永远亮着。
        let _: () = msg_send![main, setAutoenablesItems: false];
        for spec in localized_menu_with(state) {
            let submenu = build_submenu(&spec, target, &mut table);
            let _: () = msg_send![submenu, setAutoenablesItems: false];
            // 顶层菜单项没有标题：菜单栏显示 submenu 的 title。
            let item: *mut Object = msg_send![class!(NSMenuItem), alloc];
            let item: *mut Object = msg_send![item, init];
            let _: () = msg_send![item, setSubmenu: submenu];
            add_item(main, item);
        }
        let _: () = msg_send![app, setMainMenu: main];
        // 显式 retain：避免主菜单被当成临时对象回收成空菜单栏。
        let _: () = msg_send![main, retain];
    }

    if let Ok(mut guard) = actions().lock() {
        *guard = table;
    }
    INSTALLED.store(true, std::sync::atomic::Ordering::Relaxed);
    Some(rx)
}

/// 原地刷新菜单项状态（勾选 / 置灰）；状态没变时不碰 AppKit。
pub fn refresh_menu(state: MenuState) -> bool {
    // 无头测试没有 NSApp 主菜单：任何 AppKit 调用都会跨线程 panic。
    if !INSTALLED.load(std::sync::atomic::Ordering::Relaxed) {
        return false;
    }
    static LAST: Mutex<Option<MenuState>> = Mutex::new(None);
    {
        let Ok(mut last) = LAST.lock() else {
            return false;
        };
        if *last == Some(state) {
            return false;
        }
        *last = Some(state);
    }

    // 按与 `localized_menu_with` 完全一致的结构展开（结构不随状态变化，
    // 只变 checked / enabled），再用 tag 找 item。
    let mut flat = Vec::new();
    for menu in localized_menu_with(state) {
        for entry in menu.entries {
            if let MenuEntry::Item(item) = entry {
                flat.push(item);
            }
        }
    }
    let table_len = actions().lock().map(|table| table.len()).unwrap_or(0);
    if flat.len() != table_len {
        // 不允许重建并丢掉 receiver：保留已连接菜单，报告结构错误。
        tracing::error!("native menu structure changed without an action receiver");
        return false;
    }

    unsafe {
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        if app.is_null() {
            return false;
        }
        let main: *mut Object = msg_send![app, mainMenu];
        if main.is_null() {
            return false;
        }
        let count: isize = msg_send![main, numberOfItems];
        for menu_index in 0..count {
            let top: *mut Object = msg_send![main, itemAtIndex: menu_index];
            if top.is_null() {
                continue;
            }
            let submenu: *mut Object = msg_send![top, submenu];
            if submenu.is_null() {
                continue;
            }
            let items: isize = msg_send![submenu, numberOfItems];
            for item_index in 0..items {
                let item: *mut Object = msg_send![submenu, itemAtIndex: item_index];
                if item.is_null() {
                    continue;
                }
                let tag: isize = msg_send![item, tag];
                if tag < 0 || tag as usize >= flat.len() {
                    continue;
                }
                let spec = &flat[tag as usize];
                let _: () = msg_send![item, setEnabled: spec.enabled];
                let _: () = msg_send![item, setState: if spec.checked { 1isize } else { 0isize }];
            }
        }
    }
    true
}

/// 快捷键显示与派发共享 ShortcutMap；输入源改变只更新 key equivalent，不重建菜单。
pub fn refresh_shortcut_keys(
    map: &crate::frontend::gpui::model::shortcut_editor::ShortcutMap,
    qwerty: bool,
) {
    if !INSTALLED.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    static LAST_MAP: Mutex<
        Option<(
            crate::frontend::gpui::model::shortcut_editor::ShortcutMap,
            bool,
        )>,
    > = Mutex::new(None);
    if let Ok(mut last) = LAST_MAP.lock() {
        if last
            .as_ref()
            .is_some_and(|(last_map, last_layout)| last_map == map && *last_layout == qwerty)
        {
            return;
        }
        *last = Some((map.clone(), qwerty));
    }
    let table = actions()
        .lock()
        .map(|table| table.clone())
        .unwrap_or_default();
    let keys: Vec<_> = table
        .iter()
        .map(|action| map.default_chord(*action))
        .collect();
    unsafe {
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let main: *mut Object = msg_send![app, mainMenu];
        let count: isize = msg_send![main, numberOfItems];
        for menu_index in 0..count {
            let top: *mut Object = msg_send![main, itemAtIndex: menu_index];
            let submenu: *mut Object = msg_send![top, submenu];
            if submenu.is_null() {
                continue;
            }
            let count: isize = msg_send![submenu, numberOfItems];
            for index in 0..count {
                let item: *mut Object = msg_send![submenu, itemAtIndex: index];
                let separator: bool = msg_send![item, isSeparatorItem];
                if separator {
                    continue;
                }
                let tag: isize = msg_send![item, tag];
                let Some(key) = keys.get(tag as usize) else {
                    continue;
                };
                let (equivalent, flags) = if let Some(key) = key {
                    let value = if qwerty {
                        crate::frontend::gpui::model::physical_key::to_qwerty(&key.key)
                    } else {
                        key.key.clone()
                    };
                    let spec = MenuKey {
                        key: value,
                        command: key.command,
                        shift: key.shift,
                        option: key.option,
                        control: key.control,
                    };
                    (
                        key_equivalent(&spec).unwrap_or_default(),
                        (if key.command { MOD_COMMAND } else { 0 })
                            | (if key.shift { MOD_SHIFT } else { 0 })
                            | (if key.option { MOD_OPTION } else { 0 })
                            | (if key.control { MOD_CONTROL } else { 0 }),
                    )
                } else {
                    (String::new(), 0)
                };
                let _: () = msg_send![item, setKeyEquivalent: ns_string(&equivalent)];
                let _: () = msg_send![item, setKeyEquivalentModifierMask: flags];
            }
        }
    }
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
