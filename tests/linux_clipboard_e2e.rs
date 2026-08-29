//! Linux clipboard e2e for VTE selection copy.

#![cfg(feature = "gtk")]

mod support;

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::Widget;
use vte4::prelude::*;

use muxterm::platform::linux::pane_view::PaneView;
use muxterm::platform::linux::quickconnect::font::FontSettings;

use support::linux_gtk::*;

#[test]
fn selected_text_copy_reaches_clipboard() {
    if skip_no_display() {
        return;
    }
    gtk4::test_synced(|| {
        gtk_test_framework_smoke();
        let view = PaneView::new(1, &load_theme(), &FontSettings::default(), false, 10_000);
        let win = gtk4::Window::builder()
            .title("clipboard-e2e")
            .default_width(480)
            .default_height(240)
            .child(&view.widget())
            .build();
        win.present();
        gtk4::test_widget_wait_for_draw(&win);

        view.feed_output(b"MUXTERM_COPY_TOKEN\r\n");
        view.flush_deferred_feed();
        pump_main_loop(100);
        view.terminal().select_all();
        pump_main_loop(100);

        let selected = view
            .selected_text()
            .expect("selected text must be available after select_all");
        assert!(
            selected.contains("MUXTERM_COPY_TOKEN"),
            "selection text should contain token, got {selected:?}"
        );

        view.copy_clipboard();
        let clipboard = view.widget().clipboard();
        let got = Rc::new(RefCell::new(None::<String>));
        let got_cb = got.clone();
        clipboard.read_text_async(gtk4::gio::Cancellable::NONE, move |result| {
            *got_cb.borrow_mut() = result.ok().flatten().map(|text| text.to_string());
        });
        pump_main_loop(300);

        let text = got.borrow().clone().unwrap_or_default();
        assert!(
            text.contains("MUXTERM_COPY_TOKEN"),
            "clipboard should contain selected token, got {text:?}"
        );

        win.set_child(None::<&Widget>);
        win.destroy();
        pump_main_loop(100);
    });
}
