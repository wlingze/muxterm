//! 证明 gpui 的无头测试框架在本仓库可用。
//!
//! 后续从 macOS ChromeTests / AppE2ETests 移植的测试依赖这套能力：
//! 不开真窗口也能建视图、送按键、改尺寸、读剪贴板。

#![cfg(feature = "gpui")]

use gpui::{div, prelude::*, px, size, Context, Render, Window};

struct HarnessProbe;

impl Render for HarnessProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().id("harness-probe").size_full().child("probe")
    }
}

#[gpui::test]
fn headless_window_accepts_input_and_resize(cx: &mut gpui::TestAppContext) {
    let (_view, cx) = cx.add_window_view(|_, _| HarnessProbe);
    cx.simulate_resize(size(px(800.), px(600.)));
    cx.simulate_keystrokes("a");
    cx.update(|_, _| {});
}
