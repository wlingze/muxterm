//! macOS 主线程、真实 NSEvent → 本地监视器 → 菜单通道的正式回归。
//! 不需要辅助功能授权，不触碰真实终端/用户会话，不修改键盘输入源。
#![allow(unexpected_cfgs)]

#[cfg(target_os = "macos")]
fn main() {
    use muxterm::frontend::gpui::{
        model::{
            physical_key,
            shortcut_editor::{self, ShortcutMap},
            shortcuts::{KeyAction, ShortcutCommand},
        },
        platform,
    };
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    extern "C" {
        fn CGEventCreateKeyboardEvent(
            source: *const std::ffi::c_void,
            key: u16,
            down: bool,
        ) -> *mut std::ffi::c_void;
        fn CGEventSetFlags(event: *mut std::ffi::c_void, flags: u64);
        fn CFRelease(value: *const std::ffi::c_void);
    }
    unsafe {
        let pool: *mut Object = msg_send![class!(NSAutoreleasePool), new];
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let mut receiver = platform::install_menu().expect("native menu receiver");
        platform::install_physical_key_tracker();
        platform::configure_shortcuts(&ShortcutMap::default(), false, false, false);
        // 全目录所有可打印键的默认动作都走真正的生产监视器，不直调 handle_key。
        let mut count = 0;
        for row in shortcut_editor::catalog() {
            for chord in row.defaults {
                let Some(code) = physical_key::key_code(&chord.key) else {
                    continue;
                };
                let flags = (if chord.command { 1 << 20 } else { 0 })
                    | (if chord.shift { 1 << 17 } else { 0 })
                    | (if chord.option { 1 << 19 } else { 0 })
                    | (if chord.control { 1 << 18 } else { 0 });
                let event = CGEventCreateKeyboardEvent(std::ptr::null(), code, true);
                assert!(!event.is_null());
                CGEventSetFlags(event, flags);
                let native: *mut Object = msg_send![class!(NSEvent), eventWithCGEvent: event];
                CFRelease(event);
                let _: () = msg_send![app, sendEvent: native];
                assert_eq!(
                    receiver.try_recv().expect("receiver connected"),
                    ShortcutCommand::Action(row.action),
                    "native keyCode={code}, chord={chord:?}"
                );
                assert!(
                    receiver.try_recv().is_err(),
                    "one event must not reach both view and menu"
                );
                count += 1;
            }
        }
        // 显式禁用的默认键不能经原生菜单绕回来。
        let map = ShortcutMap::from_values(
            &serde_json::json!({"shortcuts":{"preset":"colemak","overrides":[{"action":"new_tab","bindings":[]}]}}),
        );
        platform::configure_shortcuts(&map, false, false, false);
        assert_eq!(
            platform::macos_keys::resolve_native(3, 1 << 20, "t", false),
            None
        );
        assert_eq!(
            map.resolve(&muxterm::frontend::gpui::model::shortcuts::KeyChord::command("d")),
            Some(KeyAction::SplitHorizontal)
        );
        platform::macos_keys::uninstall();
        let _: () = msg_send![pool, drain];
        println!("native macOS shortcuts: {count} real NSEvents dispatched once; disable verified");
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {
    println!("native macOS shortcut regression: not applicable on this platform");
}
