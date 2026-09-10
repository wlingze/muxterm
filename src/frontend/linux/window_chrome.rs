//! 主窗口 chrome 的主题 CSS 与 provider 生命周期。

use std::cell::RefCell;

use gtk4::gdk;
use gtk4::CssProvider;

use super::Theme;

/// 窗口铬（根背景 / tab / status）随主题变化；badge 色保持品牌色。
pub(super) fn chrome_css(theme: &Theme) -> String {
    let bg = format!(
        "#{:02x}{:02x}{:02x}",
        theme.background.0, theme.background.1, theme.background.2
    );
    let fg = format!(
        "#{:02x}{:02x}{:02x}",
        theme.foreground.0, theme.foreground.1, theme.foreground.2
    );
    format!(
        "
        .muxterm-root {{ background: {bg}; }}
        .tab-bar {{ background: {bg}; }}
        button.tab-button {{
            background-image: none;
            background-color: transparent;
            border: none;
            box-shadow: none;
            min-height: 18px;
            min-width: 0;
            padding: 2px 10px;
            border-radius: 0;
            color: {fg};
            font-weight: 400;
            opacity: 0.55;
        }}
        button.tab-button.tab-active {{
            opacity: 1;
            font-weight: 600;
            background-color: alpha({fg}, 0.12);
            box-shadow: inset 0 -2px 0 {fg};
        }}
        .status-bar {{
            color: {fg};
            padding: 3px 8px;
            font-size: 11px;
            border-top: 1px solid alpha({fg}, 0.12);
        }}
        .muxterm-status-windows {{
            padding: 1px;
            border-radius: 5px;
            background-color: alpha({fg}, 0.035);
        }}
        button.muxterm-status-window {{
            background-image: none;
            background-color: transparent;
            border: 1px solid transparent;
            box-shadow: none;
            min-height: 20px;
            padding: 1px 10px;
            border-radius: 4px;
            color: {fg};
            font-weight: 500;
            opacity: 0.72;
        }}
        button.muxterm-status-window:hover {{
            opacity: 0.92;
            background-color: alpha({fg}, 0.08);
        }}
        button.muxterm-status-window.tab-active {{
            opacity: 1;
            font-weight: 700;
            border-color: alpha({fg}, 0.16);
            background-color: alpha({fg}, 0.20);
            box-shadow: inset 0 -3px 0 {fg};
        }}
        .qc-badge {{ padding: 0 6px; border-radius: 4px; font-size: 9px; color: #fff; }}
        .qc-badge-recent {{ background: #1e66f5; }}
        .qc-badge-project {{ background: #40a02b; }}
        .qc-badge-current {{ background: #df8e1d; }}
        .qc-current {{ background: alpha(#89b4fa, 0.18); }}
        .muxterm-sidebar {{ background: {bg}; border-right: 1px solid alpha({fg}, 0.18); }}
        .muxterm-sidebar-section-header {{
            background-image: none;
            background-color: alpha({fg}, 0.035);
            border: none;
            border-radius: 0;
            min-height: 22px;
            padding: 0;
            color: {fg};
        }}
        .muxterm-sidebar-section-header:hover {{ background-color: alpha({fg}, 0.09); }}
        .muxterm-sidebar-title {{ color: {fg}; font-size: 9.5px; font-weight: 700; letter-spacing: 0.04em; }}
        .muxterm-sidebar-section-arrow {{ color: {fg}; opacity: 0.72; }}
        .muxterm-sidebar-sections > separator {{
            min-height: 1px;
            background: alpha({fg}, 0.18);
        }}
        .muxterm-sidebar-list {{ background: transparent; padding: 1px 3px 3px; }}
        .muxterm-sidebar-row {{ border-radius: 4px; margin: 1px 0; }}
        .muxterm-sidebar-row.active {{ background: alpha({fg}, 0.14); box-shadow: inset 2px 0 0 alpha({fg}, 0.72); }}
        .muxterm-sidebar-close {{
            min-width: 22px;
            min-height: 22px;
            padding: 0;
            border-radius: 4px;
            color: {fg};
            opacity: 0;
        }}
        .muxterm-sidebar-workspace-row:hover .muxterm-sidebar-close {{ opacity: 0.72; }}
        .muxterm-sidebar-close:hover {{
            opacity: 1;
            background-color: alpha({fg}, 0.12);
        }}
        button.muxterm-sidebar-command-visibility {{
            background-image: none;
            background-color: transparent;
            border: none;
            box-shadow: none;
            min-width: 22px;
            min-height: 22px;
            padding: 1px 3px;
            border-radius: 4px;
            color: {fg};
            opacity: 0.58;
        }}
        button.muxterm-sidebar-command-visibility:hover {{
            opacity: 1;
            background-color: alpha({fg}, 0.12);
        }}
        .muxterm-sidebar-row-name {{ color: {fg}; font-size: 11.5px; font-weight: 500; }}
        .muxterm-sidebar-row-detail {{ color: {fg}; opacity: 0.55; font-size: 10px; }}
        .muxterm-sidebar-workspace-shortcut {{
            color: {fg};
            opacity: 0.52;
            font-size: 11px;
            font-weight: 600;
        }}
        .muxterm-sidebar-agent-dot {{ font-size: 10px; }}
        .muxterm-sidebar-agent-dot.running {{ color: #40a02b; }}
        .muxterm-sidebar-agent-dot.done {{ color: #df8e1d; }}
        .muxterm-main-split > separator {{
            min-width: 1px;
            background: alpha({fg}, 0.18);
        }}
        .quick-pick-backdrop {{ background-color: alpha(#000000, 0.42); }}
        .quick-pick-root {{
            background-color: {bg};
            color: {fg};
            border: 1px solid alpha({fg}, 0.24);
            border-radius: 10px;
            box-shadow: 0 18px 44px alpha(#000000, 0.54), inset 0 1px alpha({fg}, 0.08);
        }}
        .quick-pick-entry {{
            min-height: 32px;
            padding: 0 10px;
            background-color: alpha({fg}, 0.065);
            color: {fg};
            border: 1px solid alpha({fg}, 0.20);
            border-radius: 6px;
            box-shadow: inset 0 1px 2px alpha(#000000, 0.22);
        }}
        .quick-pick-entry:focus {{
            border-color: alpha({fg}, 0.42);
            box-shadow: 0 0 0 1px alpha({fg}, 0.12), inset 0 1px 2px alpha(#000000, 0.20);
        }}
        .quick-pick-list {{ background-color: transparent; padding: 2px 6px 5px; }}
        .quick-pick-list row {{
            min-height: 0;
            border-radius: 5px;
            color: {fg};
        }}
        .quick-pick-list row:hover {{ background-color: alpha({fg}, 0.065); }}
        .quick-pick-list row:selected {{ background-color: alpha({fg}, 0.15); }}
        .quick-pick-label, .qc-name {{ font-size: 13px; font-weight: 600; }}
        .quick-pick-detail, .qc-sub {{ font-size: 11px; opacity: 0.62; }}
        .qc-attention-count {{ font-size: 10px; opacity: 0.58; }}
        .quick-pick-root togglebutton {{
            min-height: 24px;
            padding: 1px 9px;
            border-radius: 5px;
        }}
        "
    )
}

pub(super) fn apply_chrome_css(theme: &Theme) {
    thread_local! {
        static PROVIDER: RefCell<Option<CssProvider>> = const { RefCell::new(None) };
    }
    PROVIDER.with(|slot| {
        let mut slot = slot.borrow_mut();
        let css = slot.get_or_insert_with(|| {
            let provider = CssProvider::new();
            if let Some(display) = gdk::Display::default() {
                gtk4::style_context_add_provider_for_display(
                    &display,
                    &provider,
                    gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
                );
            }
            provider
        });
        css.load_from_data(&chrome_css(theme));
    });
}
