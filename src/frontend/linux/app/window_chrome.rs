//! 主窗口 chrome 的主题 CSS 与 provider 生命周期。

use std::cell::RefCell;

use gtk4::gdk;
use gtk4::CssProvider;

use super::Theme;

/// 窗口铬（根背景 / tab / status）随主题变化；badge 色保持品牌色。
pub(super) fn chrome_css(theme: &Theme) -> String {
    let light = u32::from(theme.background.0)
        + u32::from(theme.background.1)
        + u32::from(theme.background.2)
        > 384;
    let shell_accent = if light { "#087f8c" } else { "#6bd4d0" };
    let agent_accent = if light { "#7953bb" } else { "#cba6f7" };
    let working_accent = if light { "#a46109" } else { "#f2bf68" };
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
        .muxterm-root {{ background: {bg}; font-size: 14px; }}
        button.muxterm-sidebar-workspace-row {{ padding: 10px; margin: 2px 8px; min-height: 30px; background: transparent; background-image: none; border: none; box-shadow: none; color: {fg}; font-size: 14px; font-weight: 500; }}
        .workspace-badge {{ min-width: 24px; min-height: 24px; border-radius: 6px; background: alpha({fg}, 0.08); font-size: 12px; font-weight: 700; }}
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
        // 更新提醒条：贴终端区上方的一条细 banner（有新版本时才显示）。
        .muxterm-update-banner {{
            padding: 4px 10px;
            background: alpha({shell_accent}, 0.14);
            border-bottom: 1px solid alpha({fg}, 0.12);
            color: {fg};
            font-size: 13px;
        }}
        .muxterm-update-banner button.muxterm-update-action {{
            background-image: none;
            background-color: alpha({shell_accent}, 0.22);
            border: 1px solid alpha({shell_accent}, 0.45);
            border-radius: 6px;
            box-shadow: none;
            color: {fg};
            min-height: 22px;
            padding: 1px 10px;
            font-weight: 600;
        }}
        .muxterm-update-banner button.muxterm-update-action:hover {{ background-color: alpha({shell_accent}, 0.34); }}
        .muxterm-update-banner button.muxterm-update-dismiss {{
            min-width: 22px;
            min-height: 22px;
            padding: 0 4px;
            border-radius: 5px;
            background: transparent;
            box-shadow: none;
            color: {fg};
            opacity: 0.6;
        }}
        .muxterm-update-banner button.muxterm-update-dismiss:hover {{ opacity: 1; }}
        .status-bar {{
            color: {fg};
            padding: 3px 8px;
            font-size: 13px;
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
            min-height: 26px;
            min-width: 0;
            padding: 2px 8px;
            border-radius: 6px;
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
            font-weight: 600;
            border-color: transparent;
            background-color: transparent;
            box-shadow: none;
        }}
        .muxterm-tab-group {{ border-radius: 6px; }}
        .muxterm-tab-group.tab-active {{ background: alpha({fg}, 0.075); box-shadow: inset 0 -2px 0 {shell_accent}; }}
        .muxterm-status-bar.agents .muxterm-tab-group.tab-active {{ box-shadow: inset 0 -2px 0 {agent_accent}; }}
        .muxterm-tab-close {{ min-width: 20px; min-height: 24px; padding: 0 4px; border-radius: 5px; background: transparent; box-shadow: none; }}
        .muxterm-tab-close:hover {{ background: alpha({fg}, 0.12); }}
        .qc-badge {{ padding: 2px 7px; border-radius: 5px; font-size: 11px; color: #fff; }}
        .qc-badge-recent {{ background: #1e66f5; }}
        .qc-badge-project {{ background: #40a02b; }}
        .qc-badge-current {{ background: #df8e1d; }}
        .qc-current {{ background: alpha(#89b4fa, 0.18); }}
        .muxterm-sidebar {{ background: alpha({fg}, 0.025); border-right: 1px solid alpha({fg}, 0.10); }}
        .muxterm-sidebar-section-header {{
            background-image: none;
            background-color: alpha({fg}, 0.035);
            border: none;
            border-radius: 0;
            min-height: 32px;
            padding: 2px 6px;
            color: {fg};
        }}
        .muxterm-sidebar-section-header:hover {{ background-color: alpha({fg}, 0.09); }}
        .muxterm-sidebar-title {{ color: {fg}; opacity: 0.65; font-size: 12px; font-weight: 600; letter-spacing: 0.04em; }}
        .muxterm-sidebar-section-arrow {{ color: {fg}; opacity: 0.72; }}
        .muxterm-sidebar-sections > separator {{
            min-height: 1px;
            margin: 0;
            padding: 0;
            border: none;
            background-image: none;
            background: alpha({fg}, 0.18);
        }}
        .muxterm-sidebar-list {{ background: transparent; padding: 4px 8px 8px; }}
        .muxterm-sidebar-row {{ border-radius: 7px; margin: 2px 0; padding: 5px 4px; }}
        .muxterm-sidebar-row.active, button.muxterm-sidebar-workspace-row.active {{ background: alpha({fg}, 0.14); box-shadow: inset 2px 0 0 alpha({fg}, 0.72); }}
        .muxterm-sidebar-close {{
            min-width: 22px;
            min-height: 22px;
            padding: 0;
            border-radius: 4px;
            color: {fg};
            opacity: 0;
        }}
        .muxterm-sidebar-workspace-row:hover .muxterm-sidebar-close {{ opacity: 0.72; }}
        .quick-pick-list row:hover .muxterm-sidebar-close, .quick-pick-list row:selected .muxterm-sidebar-close {{ opacity: 0.72; }}
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
        .muxterm-sidebar-row-name {{ color: {fg}; font-size: 14px; font-weight: 500; }}
        .muxterm-sidebar-row-detail {{ color: {fg}; opacity: 0.65; font-size: 12px; }}
        .muxterm-sidebar-workspace-shortcut {{
            color: {fg};
            opacity: 0.52;
            font-size: 12px;
            font-weight: 600;
        }}
        .muxterm-sidebar-agent-dot {{ font-size: 10px; }}
        .muxterm-sidebar-agent-dot.working, .muxterm-sidebar-agent-dot.running,
        .muxterm-sidebar-row-detail.working, .quick-pick-detail.working {{ color: {working_accent}; }}
        .muxterm-sidebar-agent-dot.idle,
        .muxterm-sidebar-row-detail.idle, .quick-pick-detail.idle {{ color: #6c7086; }}
        .muxterm-sidebar-agent-dot.blocked,
        .muxterm-sidebar-row-detail.blocked, .quick-pick-detail.blocked {{ color: #e64553; }}
        .muxterm-sidebar-agent-dot.done,
        .muxterm-sidebar-row-detail.done, .quick-pick-detail.done {{ color: #179299; }}
        .muxterm-sidebar-row-detail.working, .muxterm-sidebar-row-detail.idle,
        .muxterm-sidebar-row-detail.blocked, .muxterm-sidebar-row-detail.done,
        .quick-pick-detail.working, .quick-pick-detail.idle,
        .quick-pick-detail.blocked, .quick-pick-detail.done {{ opacity: 1; }}
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
            min-height: 40px;
            padding: 2px 12px;
            font-size: 15px;
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
            border-radius: 7px;
            padding: 5px 4px;
            color: {fg};
        }}
        .quick-pick-list row:hover {{ background-color: alpha({fg}, 0.065); }}
        .quick-pick-list row:selected {{ background-color: alpha({fg}, 0.15); }}
        .quick-pick-label, .qc-name {{ font-size: 15px; font-weight: 500; }}
        .quick-pick-detail, .qc-sub {{ font-size: 13px; opacity: 0.68; }}
        .qc-attention-count {{ font-size: 12px; opacity: 0.65; }}
        .muxterm-status-windows .done {{ color: #179299; }}
        .muxterm-status-windows .blocked {{ color: #e64553; }}
        .muxterm-status-windows spinner {{ color: {working_accent}; }}
        .muxterm-status-windows .idle {{ opacity: 0.6; }}
        .muxterm-status-bar.shells {{ border-top-color: alpha({shell_accent}, 0.3); background: alpha({shell_accent}, 0.09); }}
        .muxterm-status-bar.agents {{ border-top-color: alpha({agent_accent}, 0.3); background: alpha({agent_accent}, 0.10); }}
        .quick-pick-tabs {{ border-radius: 8px; padding: 3px; background: alpha({fg}, 0.045); }}
        .quick-pick-tabs button {{ background: transparent; border: none; box-shadow: none; color: alpha({fg}, 0.65); padding: 5px 10px; }}
        .quick-pick-tabs button:checked {{ background: alpha({fg}, 0.10); color: {fg}; }}
        .muxterm-sidebar-toggle {{ min-height: 26px; min-width: 26px; padding: 0 5px; border-radius: 6px; background-image: none; box-shadow: none; }}
        .muxterm-sidebar-toggle:checked {{ background: alpha({fg}, 0.08); }}
        .muxterm-pane-header {{ min-height: 28px; padding: 0 8px; background: alpha({fg}, 0.045); border-bottom: 1px solid alpha({fg}, 0.10); font-size: 13px; }}
        .muxterm-pane-header.active {{ background: alpha({fg}, 0.10); }}
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
