//! 主窗口的字体、主题和状态栏外观动作。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk4::glib;

use super::{
    apply_chrome_css, fallback_theme, maybe_refresh_status, persist_config, ClientConfig,
    ClientConfigSnapshot, FontSettings, KeyMap, StatusBarMode, UiState,
};

/// C8：字号写盘防抖（300ms），避免 Ctrl+= 热路径同步写 config.toml。
/// 用 generation 作废旧回调，不 remove 已触发的 SourceId（glib 会 panic）。
fn schedule_font_persist(state: &Rc<RefCell<UiState>>, size: f32) {
    thread_local! {
        static FONT_PERSIST_GEN: Cell<u64> = const { Cell::new(0) };
    }
    FONT_PERSIST_GEN.with(|gen| {
        let my_gen = gen.get().wrapping_add(1);
        gen.set(my_gen);
        let state = Rc::downgrade(state);
        glib::timeout_add_local(std::time::Duration::from_millis(300), move || {
            let current = FONT_PERSIST_GEN.with(|g| g.get());
            if current == my_gen {
                if let Some(state) = state.upgrade() {
                    match state.try_borrow() {
                        Ok(s) => persist_config(
                            s.event_pump.client(),
                            "font.size",
                            serde_json::Value::from(f64::from(size)),
                        ),
                        Err(error) => tracing::warn!(
                            target = "muxterm::config",
                            "字号防抖写盘时主窗口状态仍被占用: {error}"
                        ),
                    }
                }
            }
            glib::ControlFlow::Break
        });
    });
}

pub(super) fn adjust_font(s: &mut UiState, state: &Rc<RefCell<UiState>>, direction: i32) {
    let next = FontSettings::zoomed(s.font.size, direction);
    if (next - s.font.size).abs() < f32::EPSILON {
        return;
    }
    s.font.size = next;
    // C8：热路径只改当前前台 LayoutHost，立刻返回；后台 cache 在 activate
    // 时按尺寸差补。写盘防抖 300ms，不阻塞按键。
    s.active_layout_mut().set_font_size(next);
    schedule_font_persist(state, next);
}

pub(super) fn reset_font(s: &mut UiState) {
    s.font.size = s.config_font_size;
    let font = s.font.clone();
    for layout in s.scenes.values_mut() {
        layout.set_font(&font);
    }
    persist_config(
        s.event_pump.client(),
        "font.size",
        serde_json::Value::from(f64::from(s.config_font_size)),
    );
}

pub(super) fn toggle_theme(s: &mut UiState) {
    let next_name = super::toggle_target(&s.theme_name);
    let Ok(snapshot) = s.event_pump.client().config_apply_path(
        "theme.name",
        serde_json::Value::String(next_name.to_string()),
    ) else {
        tracing::error!(
            target = "muxterm::linux",
            "保存主题 {next_name} 失败，保持当前主题"
        );
        return;
    };
    let Some(theme) = snapshot.resolved_theme else {
        tracing::error!(
            target = "muxterm::linux",
            "Core 没有返回主题 {next_name}，保持当前主题"
        );
        return;
    };
    s.theme_name = next_name.to_string();
    s.theme = theme.clone();
    for layout in s.scenes.values_mut() {
        layout.apply_theme(&theme);
    }
    s.status.apply_theme(&theme);
    apply_chrome_css(&theme);
    report_all_pane_colours(s);
}

/// Apply one owned Core configuration snapshot to all live frontend surfaces.
/// This is shared by the settings callback and the EventPump ConfigChanged
/// path so external reloads and in-app edits have identical behavior.
pub(super) fn apply_config_snapshot(s: &mut UiState, snapshot: ClientConfigSnapshot) {
    let resolved_theme = snapshot.resolved_theme.clone();
    let effective_keybindings = snapshot.effective_keybindings.clone();
    let cfg = match serde_json::from_value::<ClientConfig>(snapshot.values) {
        Ok(config) => config,
        Err(error) => {
            tracing::warn!(
                target = "muxterm::config",
                "配置快照解码失败，跳过热应用: {error}"
            );
            return;
        }
    };

    s.keymap = KeyMap::from_bindings(&effective_keybindings);
    let attention_config = cfg.attention.clone();
    if let Err(error) = s.event_pump.client().configure_attention(&attention_config) {
        tracing::warn!(
            target = "muxterm::linux",
            %error,
            "热加载 Core attention 配置失败"
        );
    }
    s.compatibility_activity.set_config(attention_config);

    s.config_font_size = cfg.font.size;
    s.font = FontSettings {
        family: cfg.font.family.clone(),
        size: FontSettings::clamp_size(cfg.font.size),
        fallback: cfg.font.fallback.clone(),
    };
    let font = s.font.clone();
    for layout in s.scenes.values_mut() {
        layout.set_font(&font);
    }

    s.theme_name = cfg.theme.name.to_ascii_lowercase();
    let theme = resolved_theme.unwrap_or_else(fallback_theme);
    s.theme = theme.clone();
    apply_chrome_css(&theme);
    for layout in s.scenes.values_mut() {
        layout.apply_theme(&theme);
    }
    s.status.apply_theme(&theme);
    s.status_mode = StatusBarMode::from_toml(Some(&cfg.statusbar.mode));
    s.status.set_mode(s.status_mode);
    report_all_pane_colours(s);
    maybe_refresh_status(s, true);
}

pub(super) fn toggle_status_mode(s: &mut UiState) {
    let next = match s.status_mode {
        StatusBarMode::Tmux => StatusBarMode::Theme,
        StatusBarMode::Theme => StatusBarMode::Tmux,
    };
    s.status_mode = next;
    s.status.set_mode(next);
    persist_config(
        s.event_pump.client(),
        "statusbar.mode",
        serde_json::Value::String(next.as_str().to_string()),
    );
    maybe_refresh_status(s, true);
}

pub(super) fn report_all_pane_colours(s: &mut UiState) {
    if !s.uses_tmux() {
        return;
    }
    let fg = s.theme.foreground;
    let bg = s.theme.background;
    let _ = (fg, bg);
    let _ = s.event_pump.client().report_all_pane_colours(
        &format!("#{:02x}{:02x}{:02x}", fg.0, fg.1, fg.2),
        &format!("#{:02x}{:02x}{:02x}", bg.0, bg.1, bg.2),
    );
}
