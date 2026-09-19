//! GTK 主窗口发起的配置与 target 配置窗口动作。

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use anyhow::anyhow;
use gtk4::Window;

use crate::frontend::linux::quickconnect::model::TargetConfigDraft;

use super::window_actions::apply_config_snapshot;
use super::window_overlay::open_quick_connect;
use super::{FfiClient, UiState};
use crate::frontend::linux::preferences_window::ConfigApi;

/// 打开配置页：保存/热加载后重读 config.toml 并应用主题/字体/attention。
pub(super) fn open_preferences(state: &Rc<RefCell<UiState>>, window: &Window) {
    let path = match FfiClient::new_catalog().and_then(|client| client.config_describe()) {
        Ok(snapshot) if !snapshot.path.trim().is_empty() => std::path::PathBuf::from(snapshot.path),
        Ok(_) | Err(_) => {
            tracing::warn!(
                target = "muxterm::linux",
                "Core 未返回配置路径，无法打开配置页"
            );
            return;
        }
    };
    let st = state.clone();
    let hosts = FfiClient::discover_ssh_hosts().unwrap_or_default();
    let runtimes = FfiClient::discover_runtimes().unwrap_or_default();
    let config_api = ConfigApi::from_callbacks(
        {
            let state = Rc::downgrade(state);
            move || with_config_client(&state, FfiClient::config_describe)
        },
        {
            let state = Rc::downgrade(state);
            move |patch| with_config_client(&state, |client| client.config_apply(patch))
        },
        {
            let state = Rc::downgrade(state);
            move || {
                with_config_client(&state, |client| {
                    client.config_reload()?;
                    client.config_describe()
                })
            }
        },
    );
    let snapshot = match config_api.describe() {
        Ok(snapshot) => snapshot,
        Err(error) => {
            tracing::warn!(
                target = "muxterm::config",
                "打开设置页读取配置失败: {error}"
            );
            return;
        }
    };
    let config_for_saved = config_api.clone();
    crate::frontend::linux::preferences_window::show(
        window,
        path,
        config_api,
        snapshot,
        std::boxed::Box::new(move || {
            let snapshot = config_for_saved.describe();
            let mut s = st.borrow_mut();
            // 保存后重新读取 Core FFI 快照；EventPump 外部配置变更走同一函数。
            if let Ok(snapshot) = snapshot {
                apply_config_snapshot(&mut s, snapshot);
            }
        }),
        Some((runtimes, hosts)),
    );
}

fn with_config_client<T>(
    state: &Weak<RefCell<UiState>>,
    operation: impl FnOnce(&FfiClient) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let state = state.upgrade().ok_or_else(|| anyhow!("主窗口状态已销毁"))?;
    let state = state
        .try_borrow()
        .map_err(|error| anyhow!("主窗口状态正在更新: {error}"))?;
    operation(state.event_pump.client())
}

pub(super) fn open_target_config(
    state: &Rc<RefCell<UiState>>,
    window: &Window,
    editing: Option<TargetConfigDraft>,
) {
    let store = state.borrow().qc_store.clone();
    let editing_id = editing
        .as_ref()
        .and_then(|draft| store.project_id_for(draft));
    let hosts = FfiClient::discover_ssh_hosts().unwrap_or_default();
    let runtimes = FfiClient::discover_runtimes().unwrap_or_default();
    let st = state.clone();
    let win = window.clone();
    crate::frontend::linux::target_config_window::show(
        window,
        editing,
        store,
        hosts,
        runtimes,
        {
            let st = st.clone();
            let win = win.clone();
            move |saved| {
                let mut s = st.borrow_mut();
                let mut next = s.qc_store.clone();
                next.save_project(&saved, editing_id.as_deref());
                let result = serde_json::to_value(next.project_documents())
                    .map_err(anyhow::Error::from)
                    .and_then(|projects| {
                        s.event_pump
                            .client()
                            .config_apply_path("projects", projects)
                            .map(|_| ())
                    });
                if let Err(error) = result {
                    use gtk4::prelude::*;
                    let detail = error.to_string();
                    s.notification_log.push(format!("Project: {detail}"));
                    drop(s);
                    let dialog = gtk4::MessageDialog::builder()
                        .transient_for(&win)
                        .modal(true)
                        .message_type(gtk4::MessageType::Error)
                        .buttons(gtk4::ButtonsType::Close)
                        .text(&detail)
                        .build();
                    dialog.connect_response(|dialog, _| dialog.close());
                    dialog.present();
                    return;
                }
                s.qc_store = next;
                drop(s);
                open_quick_connect(&st, &win);
            }
        },
        {
            let st = st.clone();
            let win = win.clone();
            move || {
                open_quick_connect(&st, &win);
            }
        },
    );
}
