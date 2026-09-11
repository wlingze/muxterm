//! 主窗口的 GTK 输入与生命周期 signal 适配。

use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;

/// 连接主窗口键盘事件，并补回 GTK 在 keyval 中消费掉的 modifier。
///
/// 具体快捷键查找和动作执行仍由 `window::UiState` 决定；本模块只负责
/// GTK controller 的生命周期和平台事件归一化。
pub fn connect_key_handler<F>(window: &gtk4::Window, handler: F)
where
    F: Fn(gdk::Key, gdk::ModifierType) -> glib::Propagation + 'static,
{
    let controller = gtk4::EventControllerKey::new();
    controller.set_propagation_phase(gtk4::PropagationPhase::Capture);
    controller.connect_key_pressed(move |controller, keyval, _keycode, mods| {
        // GTK4 回调里的 mods 可能不含已被 keyval 消费的 Shift；再并上
        // current_event_state，Ctrl+Shift+C 才进 Copy 而不是 Ctrl+C。
        let mods = mods
            | (controller.current_event_state()
                & (gdk::ModifierType::CONTROL_MASK
                    | gdk::ModifierType::SHIFT_MASK
                    | gdk::ModifierType::ALT_MASK
                    | gdk::ModifierType::SUPER_MASK));
        handler(keyval, mods)
    });
    window.add_controller(controller);
}

/// 连接窗口关闭请求；关闭策略由调用方决定是否放行或隐藏窗口。
pub fn connect_close_handler<F>(window: &gtk4::Window, handler: F)
where
    F: Fn() -> glib::Propagation + 'static,
{
    window.connect_close_request(move |_| handler());
}
