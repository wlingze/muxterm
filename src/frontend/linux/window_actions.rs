//! Keyboard, command-palette, clipboard, and pane action wiring.
//!
//! The parent window owns state and the event loop; this module keeps
//! user actions as a separate controller boundary.

use super::window_event_pump::enqueue_workspace_input;
use super::*;

pub(super) fn prepare_core_tab_mutation(s: &mut UiState, task: &ClientTask) {
    let clear = match task {
        ClientTask::NewTab => true,
        ClientTask::CloseTab { tab_id } => *tab_id == s.active_tab,
        _ => false,
    };
    if clear {
        let workspace_key = s.active_workspace_key();
        s.visible_tabs.remove(&workspace_key);
        s.local_tab_overrides.remove(&workspace_key);
    }
}

pub(super) fn handle_action(
    s: &mut UiState,
    action: Action,
    window: &Window,
    state: &Rc<RefCell<UiState>>,
) {
    match action {
        Action::NewTab | Action::NewWindow => {
            prepare_core_tab_mutation(s, &ClientTask::NewTab);
            let _ = s.execute_active_task(ClientTask::NewTab);
            // Accepted 不得手工 refresh：等 16ms 批里 LayoutChanged/MutationSettled。
            return;
        }
        Action::NewPane => {
            let pane = s.active_pane;
            let _ = s.execute_active_task(ClientTask::SplitPane {
                pane_id: pane,
                horizontal: true,
            });
            return;
        }
        Action::NewPaneVertical => {
            let pane = s.active_pane;
            let _ = s.execute_active_task(ClientTask::SplitPane {
                pane_id: pane,
                horizontal: false,
            });
            return;
        }
        Action::SwitchTab1 => switch_tab_n(s, 1),
        Action::SwitchTab2 => switch_tab_n(s, 2),
        Action::SwitchTab3 => switch_tab_n(s, 3),
        Action::SwitchTab4 => switch_tab_n(s, 4),
        Action::SwitchTab5 => switch_tab_n(s, 5),
        Action::SwitchTab6 => switch_tab_n(s, 6),
        Action::SwitchTab7 => switch_tab_n(s, 7),
        Action::SwitchTab8 => switch_tab_n(s, 8),
        Action::SwitchTab9 => switch_tab_n(s, 9),
        Action::SwitchTabLast => {
            let workspace_id = active_workspace_key(s);
            if let Some(tab_id) = s
                .view_store
                .workspace(&workspace_id)
                .and_then(|view| view.tabs.last().map(|tab| tab.id))
            {
                request_switch_tab(s, tab_id);
            }
        }
        Action::SwitchWorkspace1 => switch_workspace_n(s, 1),
        Action::SwitchWorkspace2 => switch_workspace_n(s, 2),
        Action::SwitchWorkspace3 => switch_workspace_n(s, 3),
        Action::SwitchWorkspace4 => switch_workspace_n(s, 4),
        Action::SwitchWorkspace5 => switch_workspace_n(s, 5),
        Action::SwitchPaneNext | Action::SwitchPanePrev => {
            switch_pane_offset(s, matches!(action, Action::SwitchPaneNext));
        }
        Action::CommandPalette => {
            open_command_palette(s, window, state);
            return;
        }
        Action::Search | Action::Unknown => {}
        Action::QuickConnect => {
            // 调用方（快捷键/命令面板）必须先释放 RefMut 再打开面板；
            // 这里只做标记，由 handle_action 的调用方处理。
            let _ = (s, window, state);
            return;
        }
        Action::Quit => {
            window.close();
            return;
        }
        Action::Copy => {
            copy_active_pane(s);
            return;
        }
        Action::Paste => {
            paste_active_pane(s, state);
            return;
        }
        Action::IncreaseFontSize => adjust_font(s, state, 1),
        Action::DecreaseFontSize => adjust_font(s, state, -1),
        Action::ResetFontSize => reset_font(s),
        Action::TogglePaneFullscreen => toggle_fullscreen(s),
    }
    refresh_ui(s);
}

pub(super) fn open_command_palette(s: &UiState, window: &Window, state: &Rc<RefCell<UiState>>) {
    let parent = window.clone();
    let callback_parent = parent.clone();
    let callback_window = parent.clone();
    let callback_state = state.clone();
    let uses_tmux = s.uses_tmux();
    let next_theme = if s.theme_name.eq_ignore_ascii_case("dark") {
        "Light"
    } else {
        "Dark"
    };
    let next_status_mode = match s.status_mode {
        StatusBarMode::Tmux => StatusBarMode::Theme.as_str(),
        StatusBarMode::Theme => StatusBarMode::Tmux.as_str(),
    };
    crate::frontend::linux::command_palette::show_for_runtime(
        &parent,
        uses_tmux,
        next_theme,
        next_status_mode,
        move |id| {
            run_palette_command(&callback_state, &callback_window, &callback_parent, id);
        },
    );
}

pub(super) fn run_palette_command(
    state: &Rc<RefCell<UiState>>,
    window: &Window,
    parent: &Window,
    id: &str,
) {
    let Some(action) = parse_palette_action(id) else {
        tracing::warn!(target = "muxterm::linux", "未知命令面板动作: {id}");
        return;
    };
    match action {
        PaletteAction::Language => {
            let language_parent = parent.clone();
            let callback_state = state.clone();
            crate::frontend::linux::command_palette::show_language(&language_parent, move |_| {
                let mut s = callback_state.borrow_mut();
                maybe_refresh_status(&mut s, true);
            });
        }
        PaletteAction::TmuxDetach => {
            // 命令先入队；下一轮 GTK poll flush 后再关闭，避免 close 直接
            // 销毁窗口而丢掉尚未提交的 detach。
            {
                let mut s = state.borrow_mut();
                let accepted = s.execute_active_task(ClientTask::Detach).is_ok();
                if accepted {
                    s.pending_close = true;
                }
            }
        }
        PaletteAction::SshDisconnect => {
            let s = state.borrow();
            if s.uses_tmux() {
                let _ = s.execute_active_task(ClientTask::Detach);
            }
        }
        PaletteAction::QuickConnect => {
            open_quick_connect(state, window);
        }
        PaletteAction::ToggleTheme => {
            let mut s = state.borrow_mut();
            toggle_theme(&mut s);
        }
        PaletteAction::ToggleStatusBarMode => {
            let mut s = state.borrow_mut();
            toggle_status_mode(&mut s);
        }
        PaletteAction::TogglePaneFullscreen => {
            let mut s = state.borrow_mut();
            toggle_fullscreen(&mut s);
            refresh_ui(&mut s);
        }
        PaletteAction::IncreaseFontSize => {
            let mut s = state.borrow_mut();
            adjust_font(&mut s, state, 1);
        }
        PaletteAction::DecreaseFontSize => {
            let mut s = state.borrow_mut();
            adjust_font(&mut s, state, -1);
        }
        PaletteAction::ResetFontSize => {
            let mut s = state.borrow_mut();
            reset_font(&mut s);
        }
        PaletteAction::Quit => {
            request_quit_close(state, window);
        }
        PaletteAction::NewTab => {
            let mut s = state.borrow_mut();
            prepare_core_tab_mutation(&mut s, &ClientTask::NewTab);
            let _ = s.execute_active_task(ClientTask::NewTab);
            // Accepted 不得手工 refresh：等 LayoutChanged/MutationSettled。
        }
        PaletteAction::NewPane => {
            let s = state.borrow();
            let pane = s.active_pane;
            let _ = s.execute_active_task(ClientTask::SplitPane {
                pane_id: pane,
                horizontal: true,
            });
        }
        PaletteAction::NewPaneVertical => {
            let s = state.borrow();
            let pane = s.active_pane;
            let _ = s.execute_active_task(ClientTask::SplitPane {
                pane_id: pane,
                horizontal: false,
            });
        }
        PaletteAction::ClosePane => {
            let mut s = state.borrow_mut();
            let pane = s.active_pane;
            let _ = s.execute_active_task(ClientTask::ClosePane { pane_id: pane });
            refresh_ui(&mut s);
        }
        PaletteAction::CloseTab => {
            let mut s = state.borrow_mut();
            let tab = s.active_tab;
            let task = ClientTask::CloseTab { tab_id: tab };
            prepare_core_tab_mutation(&mut s, &task);
            let _ = s.execute_active_task(task);
            refresh_ui(&mut s);
        }
        PaletteAction::CloseWindow => window.close(),
        PaletteAction::SwitchPaneNext => {
            let mut s = state.borrow_mut();
            switch_pane_offset(&mut s, true);
            refresh_ui(&mut s);
        }
        PaletteAction::SwitchPanePrev => {
            let mut s = state.borrow_mut();
            switch_pane_offset(&mut s, false);
            refresh_ui(&mut s);
        }
        PaletteAction::SwitchTab(n) => {
            let mut s = state.borrow_mut();
            switch_tab_n(&mut s, n);
        }
        PaletteAction::TmuxAttach => open_tmux_attach(state, parent, false),
        PaletteAction::TmuxNew => open_tmux_attach(state, parent, true),
        PaletteAction::SshConnect => open_ssh_connect(state, parent),
        PaletteAction::SearchPanes => {
            open_panel(state, parent, PanelTab::Search);
        }
        PaletteAction::RenamePane => {
            tracing::info!(target = "muxterm::linux", "命令 {id} 尚未接到 GTK 对话框");
        }
        PaletteAction::Preferences => {
            open_preferences(state, parent);
        }
        PaletteAction::ReloadConfig | PaletteAction::OpenConfig => {
            tracing::info!(target = "muxterm::linux", "命令 {id} 尚未接到 GTK 对话框");
        }
    }
}

pub(super) fn toggle_fullscreen(s: &mut UiState) {
    let pane = s.active_pane;
    if s.uses_tmux() {
        let _ = s.execute_active_task(ClientTask::TogglePaneFullscreen { pane_id: pane });
    } else {
        let next = match s.active_layout().fullscreen_pane() {
            Some(id) if id == pane => None,
            _ => Some(pane),
        };
        s.active_layout_mut().set_fullscreen_pane(next);
    }
}

/// 通过统一 FFI client 的 Core 配置事务写回 config.toml（唯一事实源）。
/// 平台禁止直接解析或写 TOML；失败只记日志，不覆盖用户文件。
pub(super) fn persist_config(client: &FfiClient, dotted: &str, value: serde_json::Value) {
    if let Err(error) = client.config_apply_path(dotted, value) {
        tracing::warn!(target = "muxterm::config", "保存设置失败: {error}");
    }
}

pub(super) fn adjust_font(s: &mut UiState, state: &Rc<RefCell<UiState>>, direction: i32) {
    window_appearance::adjust_font(s, state, direction);
}

pub(super) fn reset_font(s: &mut UiState) {
    window_appearance::reset_font(s);
}

pub(super) fn toggle_theme(s: &mut UiState) {
    window_appearance::toggle_theme(s);
}

pub(super) fn apply_config_snapshot(s: &mut UiState, snapshot: ClientConfigSnapshot) {
    window_appearance::apply_config_snapshot(s, snapshot);
}

pub(super) fn toggle_status_mode(s: &mut UiState) {
    window_appearance::toggle_status_mode(s);
}

pub(super) fn report_all_pane_colours(s: &mut UiState) {
    window_appearance::report_all_pane_colours(s);
}

pub(super) fn copy_pane(s: &UiState, pane_id: u32) {
    if let Some(view) = s.active_layout().pane(pane_id) {
        view.copy_clipboard();
    }
}

pub(super) fn paste_pane(s: &UiState, state: &Rc<RefCell<UiState>>, pane_id: u32) {
    let Some(view) = s.active_layout().pane(pane_id).cloned() else {
        return;
    };
    let pane_id = view.pane_id();
    let bracketed = view.bracketed_paste();
    let st = Rc::downgrade(state);
    let clipboard = view.widget().clipboard();
    clipboard.read_text_async(gtk4::gio::Cancellable::NONE, move |result| {
        let Ok(Some(text)) = result else {
            return;
        };
        let text = crate::frontend::mirror::sanitize_paste(text.as_str(), bracketed);
        let data = crate::frontend::mirror::encode_clipboard_paste(&text, bracketed);
        if data.is_empty() {
            return;
        }
        let Some(st) = st.upgrade() else {
            return;
        };
        let s = st.borrow();
        let workspace_id = active_workspace_key(&s);
        enqueue_workspace_input(&s, &workspace_id, pane_id, &data, false);
    });
}

pub(super) fn copy_active_pane(s: &UiState) {
    copy_pane(s, s.active_pane);
}

pub(super) fn paste_active_pane(s: &UiState, state: &Rc<RefCell<UiState>>) {
    paste_pane(s, state, s.active_pane);
}

pub(super) fn split_pane_from_menu(s: &mut UiState, pane_id: u32, vertical: bool) {
    let _ = s.execute_active_task(ClientTask::SplitPane {
        pane_id,
        horizontal: !vertical,
    });
}

pub(super) fn handle_pane_menu_action(
    state: &Rc<RefCell<UiState>>,
    pane_id: u32,
    action: PaneMenuAction,
) {
    match action {
        PaneMenuAction::Copy => {
            let s = state.borrow();
            copy_pane(&s, pane_id);
        }
        PaneMenuAction::Paste => {
            let s = state.borrow();
            paste_pane(&s, state, pane_id);
        }
        PaneMenuAction::SplitVertical => {
            let mut s = state.borrow_mut();
            split_pane_from_menu(&mut s, pane_id, true);
        }
        PaneMenuAction::SplitHorizontal => {
            let mut s = state.borrow_mut();
            split_pane_from_menu(&mut s, pane_id, false);
        }
    }
}
