//! 主窗口终端区的 Overlay 层。

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::prelude::*;

/// 终端区所有常驻覆盖层和它们的轻量 UI 状态。
///
/// Overlay 不拥有 workspace/Core 状态；它只提供固定的 widget 槽位，业务状态仍由
/// `UiState` 和 `ViewStore` 驱动。这样主窗口只负责组装 AppShell，不再负责逐个构造
/// 覆盖层控件。
pub struct OverlayLayer {
    pub(crate) container: gtk4::Overlay,
    pub(crate) jump_latest: gtk4::Button,
    pub(crate) jump_unseen: u32,
    pub(crate) disconnect: gtk4::Label,
    pub(crate) search_highlight: gtk4::Label,
    pub(crate) pane_find: gtk4::Box,
    pub(crate) pane_find_entry: gtk4::Entry,
    pub(crate) last_seen: gtk4::Button,
    pub(crate) command_ok: gtk4::Button,
    pub(crate) command_fail: gtk4::Button,
    pub(crate) command_ok_text: Rc<RefCell<Option<String>>>,
    pub(crate) command_fail_text: Rc<RefCell<Option<String>>>,
}

impl OverlayLayer {
    /// 创建终端 scene 的固定覆盖层，并把 scene widget 作为底层 child。
    pub fn new(scene: &impl IsA<gtk4::Widget>) -> Self {
        let container = gtk4::Overlay::new();
        container.set_hexpand(true);
        container.set_vexpand(true);
        container.set_child(Some(scene));

        let jump_latest = gtk4::Button::with_label("↓");
        jump_latest.set_widget_name("muxterm-jump-latest");
        jump_latest.set_halign(gtk4::Align::End);
        jump_latest.set_valign(gtk4::Align::End);
        jump_latest.set_margin_end(12);
        jump_latest.set_margin_bottom(12);
        jump_latest.set_visible(false);

        let disconnect = gtk4::Label::new(Some("已断开"));
        disconnect.set_widget_name("muxterm-disconnect-overlay");
        disconnect.set_halign(gtk4::Align::Center);
        disconnect.set_valign(gtk4::Align::Center);
        disconnect.add_css_class("muxterm-disconnect-overlay");
        disconnect.set_visible(false);

        let search_highlight = gtk4::Label::new(Some("▮"));
        search_highlight.set_widget_name("muxterm-search-highlight");
        search_highlight.set_halign(gtk4::Align::Start);
        search_highlight.set_valign(gtk4::Align::Center);
        search_highlight.set_margin_start(4);
        search_highlight.add_css_class("muxterm-search-highlight");
        search_highlight.set_visible(false);

        let pane_find = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(6)
            .margin_top(8)
            .margin_start(8)
            .margin_end(8)
            .build();
        pane_find.set_widget_name("muxterm-pane-find");
        pane_find.set_halign(gtk4::Align::Start);
        pane_find.set_valign(gtk4::Align::Start);
        pane_find.add_css_class("muxterm-pane-find");
        let pane_find_entry = gtk4::Entry::new();
        pane_find_entry.set_widget_name("muxterm-pane-find-entry");
        pane_find_entry.set_placeholder_text(Some("find in pane…"));
        pane_find.append(&pane_find_entry);
        pane_find.set_visible(false);

        let last_seen = gtk4::Button::with_label("上次看到这里");
        last_seen.set_widget_name("muxterm-last-seen");
        last_seen.set_halign(gtk4::Align::Start);
        last_seen.set_valign(gtk4::Align::Center);
        last_seen.set_margin_start(4);
        last_seen.add_css_class("muxterm-last-seen");
        last_seen.set_visible(false);

        let command_ok_text = Rc::new(RefCell::new(None::<String>));
        let command_fail_text = Rc::new(RefCell::new(None::<String>));
        let command_ok = gtk4::Button::with_label("✓");
        command_ok.set_widget_name("muxterm-cmd-mark-ok");
        command_ok.set_halign(gtk4::Align::End);
        command_ok.set_valign(gtk4::Align::Center);
        command_ok.set_margin_end(2);
        command_ok.add_css_class("muxterm-cmd-mark-ok");
        command_ok.set_visible(false);
        let command_fail = gtk4::Button::with_label("✗");
        command_fail.set_widget_name("muxterm-cmd-mark-fail");
        command_fail.set_halign(gtk4::Align::End);
        command_fail.set_valign(gtk4::Align::Center);
        command_fail.set_margin_end(2);
        command_fail.add_css_class("muxterm-cmd-mark-fail");
        command_fail.set_visible(false);

        container.add_overlay(&pane_find);
        container.add_overlay(&search_highlight);
        container.add_overlay(&disconnect);
        container.add_overlay(&last_seen);
        container.add_overlay(&command_ok);
        container.add_overlay(&command_fail);
        container.add_overlay(&jump_latest);

        Self {
            container,
            jump_latest,
            jump_unseen: 0,
            disconnect,
            search_highlight,
            pane_find,
            pane_find_entry,
            last_seen,
            command_ok,
            command_fail,
            command_ok_text,
            command_fail_text,
        }
    }
}
