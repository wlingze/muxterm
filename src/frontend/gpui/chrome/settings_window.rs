//! 独立设置窗口。只持有主 GpuiApp 的 entity；没有第二个 Core/事件泵/命令队列。
use crate::frontend::gpui::state::GpuiApp;
use gpui::{
    div, prelude::*, px, size, AppContext, Bounds, Context, Entity, FocusHandle, Render,
    Subscription, TitlebarOptions, Window, WindowBounds, WindowHandle, WindowOptions,
};

pub struct SettingsWindow {
    owner: Entity<GpuiApp>,
    focus: FocusHandle,
    _subscription: Subscription,
}

impl SettingsWindow {
    pub fn open(owner: Entity<GpuiApp>, cx: &mut gpui::App) -> anyhow::Result<WindowHandle<Self>> {
        let bounds = Bounds::centered(None, size(px(840.0), px(680.0)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("Muxterm — Settings".into()),
                appears_transparent: true,
                ..Default::default()
            }),
            window_min_size: Some(size(px(680.0), px(520.0))),
            ..Default::default()
        };
        cx.open_window(options, move |window, cx| {
            let close_owner = owner.downgrade();
            window.on_window_should_close(cx, move |_, cx| {
                close_owner
                    .update(cx, |owner, cx| {
                        owner.settings_cancel(cx);
                        !owner.settings_open
                    })
                    .unwrap_or(true)
            });
            cx.new(|cx: &mut Context<Self>| {
                let focus = cx.focus_handle();
                window.focus(&focus);
                let subscription = cx.observe(&owner, |_, _, cx| cx.notify());
                Self {
                    owner,
                    focus,
                    _subscription: subscription,
                }
            })
        })
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.owner.read(cx).settings_open {
            window.remove_window();
            return div().into_any_element();
        }
        let owner = self.owner.clone();
        let input_owner = self.owner.clone();
        let focus = self.focus.clone();
        let content = owner.update(cx, |owner, cx| owner.render_settings_content(window, cx));
        div()
            .id("settings-window")
            .debug_selector(|| "settings-window".into())
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(move |_, event, window, cx| {
                owner.update(cx, |owner, cx| owner.handle_settings_key(event, window, cx));
            }))
            .child(content)
            // 设置窗口也有 IME 入口；仍然使用 owner 上的草稿，不把中文提交到 PTY。
            .child(
                gpui::canvas(
                    |_, _, _| (),
                    move |bounds, _, window, cx| {
                        window.handle_input(
                            &focus,
                            gpui::ElementInputHandler::new(bounds, input_owner.clone()),
                            cx,
                        );
                    },
                )
                .absolute()
                .inset_0()
                .size_full(),
            )
            .into_any_element()
    }
}
