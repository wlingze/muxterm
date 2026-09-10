//! 目标配置窗口的生命周期与目录补全协调。
//!
//! GTK widget tree 和交互 wiring 位于 `target_config_window_ui`；本模块保留
//! 跨 view 的 debounce/过滤逻辑、测试以及稳定的页面入口。

use std::cell::Cell;

use gtk4::prelude::IsA;
use gtk4::Window;

use crate::frontend::ffi_client::{ClientRuntimeInfo, SshHostEntry};
use crate::frontend::linux::quickconnect::model::TargetConfig;
use crate::frontend::linux::quickconnect::store::QuickConnectStore;

#[cfg(test)]
use crate::frontend::linux::quickconnect::model::TargetTransport;
#[cfg(test)]
use crate::frontend::linux::quickconnect::options::TargetOptionSelection;

#[path = "target_config_window_ui.rs"]
mod target_config_window_ui;

/// 目录补全 debounce：用 generation 作废旧回调。
///
/// 不能对已触发的 `glib::SourceId` 再 `remove()`：glib 0.20 会 unwrap
/// `Failed to remove source`，且发生在 GTK `toggled` trampoline 里无法 unwind，
/// 表现为点 SSH 卡片直接 abort。
#[derive(Default)]
pub(crate) struct ListingDebounce {
    generation: Cell<u64>,
}

impl ListingDebounce {
    pub(crate) fn bump(&self) -> u64 {
        let next = self.generation.get().wrapping_add(1);
        self.generation.set(next);
        next
    }

    pub(crate) fn is_current(&self, token: u64) -> bool {
        self.generation.get() == token
    }
}

/// SSH 未选 alias 时不要发远程 list_dir。
pub(crate) fn should_skip_directory_listing(is_ssh: bool, alias: Option<&str>) -> bool {
    is_ssh && alias.map(str::trim).filter(|s| !s.is_empty()).is_none()
}

/// 打开新建/编辑 Project 窗口。
pub fn show(
    parent: &impl IsA<Window>,
    editing: Option<TargetConfig>,
    store: QuickConnectStore,
    ssh_hosts: Vec<SshHostEntry>,
    runtimes: Vec<ClientRuntimeInfo>,
    on_save: impl Fn(TargetConfig) + 'static,
    on_cancel: impl Fn() + 'static,
) -> Window {
    target_config_window_ui::show(
        parent, editing, store, ssh_hosts, runtimes, on_save, on_cancel,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listing_debounce_invalidates_previous_token() {
        let d = ListingDebounce::default();
        let first = d.bump();
        assert!(d.is_current(first));
        let second = d.bump();
        assert!(!d.is_current(first), "旧 debounce 回调必须作废");
        assert!(d.is_current(second));
        d.bump();
        assert!(!d.is_current(second));
    }

    #[test]
    fn listing_debounce_close_bumps_away_pending() {
        let d = ListingDebounce::default();
        let pending = d.bump();
        d.bump(); // 窗口关闭
        assert!(!d.is_current(pending));
    }

    #[test]
    fn skip_remote_listing_until_ssh_alias_chosen() {
        assert!(should_skip_directory_listing(true, None));
        assert!(should_skip_directory_listing(true, Some("")));
        assert!(should_skip_directory_listing(true, Some("  ")));
        assert!(!should_skip_directory_listing(true, Some("ryzen")));
        assert!(!should_skip_directory_listing(false, None));
    }

    #[test]
    fn selecting_ssh_keeps_empty_alias_until_combo_changes() {
        let mut sel = TargetOptionSelection::default();
        sel.select_transport(TargetTransport::Ssh {
            name: String::new(),
        });
        assert!(sel.transport.is_ssh());
        assert_eq!(sel.transport.create_backend(), ("ssh", Some("")));
    }
}
