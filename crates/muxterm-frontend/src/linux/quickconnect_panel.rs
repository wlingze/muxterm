//! QuickConnect 面板：Recent + Project 快速连接（GTK Overlay）。
//!
//! 行为对齐 macOS `QuickConnectController`：搜索、badges、当前连接高亮、
//! 回车连接、双击编辑、末行 New Project。

use gtk4::prelude::*;
use gtk4::Window;

use crate::linux::quickconnect::model::TargetConfig;

#[path = "quickconnect_panel_view.rs"]
mod quickconnect_panel_view;

#[path = "quickconnect_panel_ui.rs"]
mod quickconnect_panel_ui;

#[cfg(test)]
use crate::linux::quickconnect::existing::ExistingTransport;

const PANEL_TEXT_MAX_CHARS: i32 = 64;

/// 测试/生产共用：让当前面板按最新状态重建列表（SSH 探测回来再填）。
pub fn refresh_current() {
    quickconnect_panel_ui::refresh_current();
}

/// 测试/生产共用：关闭当前 QuickConnect 面板（AppWindow 跳转后关面板，W15b）。
///
/// 独立面板测试（linux_search_e2e）不调用它，面板保持打开以便量宽度。
pub fn close_current() {
    quickconnect_panel_ui::close_current();
}

/// 窗口销毁前拆掉 thread_local 回调，避免探测线程 refresh 已死 GTK 控件。
pub fn clear_panel_hooks() {
    quickconnect_panel_ui::clear_panel_hooks();
}

/// 面板回调。
pub struct QuickConnectCallbacks {
    pub on_connect: Box<dyn Fn(TargetConfig)>,
    pub on_edit: Box<dyn Fn(TargetConfig)>,
    pub on_new_project: Box<dyn Fn()>,
}

#[cfg(test)]
pub(crate) use crate::linux::panel_model::filter_panel_items;
pub use crate::linux::panel_model::{
    build_items, build_root_items, build_search_items, existing_items, existing_root_items,
    root_items_with_existing, root_items_with_existing_and_search, ExistingNav, ExistingPanelState,
    PanelItem,
};

/// 弹出三 tab QuickConnect 面板（普通 Overlay，不构造 AppWindow）。
pub use crate::linux::panel_model::PanelShowArgs;

pub fn show(parent: &impl IsA<Window>, args: PanelShowArgs) {
    quickconnect_panel_ui::show(parent, args);
}

#[cfg(test)]
mod tests {
    use super::quickconnect_panel_ui::{visible_action_for_item, VisibleAction};
    use super::*;
    use crate::ffi_client::{ClientCandidateRef, ClientOpenIntent};
    use crate::linux::quickconnect::existing::{ExistingEntry, ExistingRuntime};
    use crate::linux::quickconnect::model::{
        QuickBadge, QuickConnectEntry, TargetRuntime, TargetTransport,
    };
    use crate::linux::quickconnect::store::QuickConnectStore;

    fn cfg(name: &str) -> TargetConfig {
        TargetConfig::new(name, TargetRuntime::Tmux, TargetTransport::Local, "~/x")
    }

    #[test]
    fn filter_keeps_new_project_on_empty_query() {
        let items = vec![
            PanelItem::Target(
                QuickConnectEntry::new(cfg("muxterm"), vec![QuickBadge::Project]),
                false,
            ),
            PanelItem::NewProject,
        ];
        assert_eq!(filter_panel_items(&items, "").len(), 2);
        let hit = filter_panel_items(&items, "mux");
        assert_eq!(hit.len(), 1);
        assert!(matches!(hit[0], PanelItem::Target(_, _)));
        let new_only = filter_panel_items(&items, "new");
        assert_eq!(new_only.len(), 1);
        assert!(matches!(new_only[0], PanelItem::NewProject));
    }

    #[test]
    fn build_items_marks_current_and_appends_new_project() {
        let mut store = QuickConnectStore::in_memory();
        let recent = cfg("recent");
        let project = cfg("project");
        store.recents.push(recent.clone());
        store.upsert_project(&project);
        let items = build_items(&store, Some(&recent));
        assert_eq!(items.len(), 3);
        assert!(matches!(
            &items[0],
            PanelItem::Target(entry, true) if entry.config == recent
        ));
        assert!(matches!(
            &items[1],
            PanelItem::Target(entry, false) if entry.config == project
        ));
        assert!(matches!(items[2], PanelItem::NewProject));
    }

    #[test]
    fn build_items_dedupes_recent_and_project() {
        let mut store = QuickConnectStore::in_memory();
        let dup = cfg("dup");
        store.recents.push(dup.clone());
        store.upsert_project(&dup);
        let items = build_items(&store, None);
        assert_eq!(items.len(), 2, "重复目标只出现一次 + New Project");
        assert!(matches!(&items[0], PanelItem::Target(entry, false) if entry.config == dup));
        assert!(matches!(items[1], PanelItem::NewProject));
    }

    #[test]
    fn build_items_keeps_project_identity_for_project_only_rows() {
        let mut store = QuickConnectStore::in_memory();
        let project = cfg("project");
        store.upsert_project(&project);

        let items = build_items(&store, None);
        assert!(matches!(
            &items[0],
            PanelItem::Target(entry, false)
                if entry.project_id.as_deref() == Some("project@local")
        ));

        store.record_recent(&project);
        let items = build_items(&store, None);
        assert!(matches!(
            &items[0],
            PanelItem::Target(entry, false) if entry.project_id.is_none()
        ));
    }

    #[test]
    fn target_rows_build_project_and_recent_open_requests() {
        let project = QuickConnectEntry::new(cfg("project"), vec![QuickBadge::Project])
            .with_project_id("project@local");
        let VisibleAction::Connect(project_request) =
            visible_action_for_item(&PanelItem::Target(project, false), &ExistingNav::Root)
        else {
            panic!("project target must produce a connect request");
        };
        assert_eq!(project_request.intent, ClientOpenIntent::CreateIfMissing);
        assert!(matches!(
            project_request.candidate,
            ClientCandidateRef::Project { project_id } if project_id == "project@local"
        ));

        let recent = QuickConnectEntry::new(cfg("recent"), vec![QuickBadge::Recent]);
        let VisibleAction::Connect(recent_request) =
            visible_action_for_item(&PanelItem::Target(recent, false), &ExistingNav::Root)
        else {
            panic!("recent target must produce a connect request");
        };
        assert_eq!(recent_request.intent, ClientOpenIntent::AttachOnly);
        assert!(matches!(
            recent_request.candidate,
            ClientCandidateRef::Recent { key } if key == "4:tmux|5:local|0:|6:recent|0:"
        ));
    }

    /// W20b：根列表第 0 项是「已有的连接」Folder，末项 New Project。
    #[test]
    fn build_root_items_puts_existing_connections_first() {
        let mut store = QuickConnectStore::in_memory();
        let project = cfg("project");
        store.upsert_project(&project);
        let items = build_root_items(&store, None);
        assert_eq!(items.len(), 3, "Folder + project + NewProject");
        assert!(matches!(
            &items[0],
            PanelItem::Folder {
                id: "existing-connections",
                ..
            }
        ));
        assert!(matches!(items[2], PanelItem::NewProject));
    }

    #[test]
    fn root_search_includes_existing_workspace_without_duplicate_project() {
        let mut store = QuickConnectStore::in_memory();
        store.upsert_project(&cfg("project"));
        let base = build_root_items(&store, None);
        let existing = ExistingPanelState {
            locals: vec![ExistingEntry {
                title: "orphan".into(),
                runtime: ExistingRuntime::Tmux,
                transport: ExistingTransport::Local,
                tmux_session: Some("orphan".into()),
                tmux_socket: Some("muxterm-test-root-search".into()),
                herdr_session: None,
                herdr_workspace_id: None,
                herdr_socket: None,
            }],
            ..ExistingPanelState::default()
        };

        let root = root_items_with_existing(&base, &existing, "orph");
        let rows = filter_panel_items(&root, "orph");
        assert_eq!(rows.len(), 1);
        assert!(matches!(rows[0], PanelItem::Existing(_)));
    }

    #[test]
    fn root_search_includes_recent_beyond_compact_display_limit() {
        let mut store = QuickConnectStore::in_memory();
        for index in 0..6 {
            store.recents.push(cfg(&format!("recent-{index}")));
        }
        let base = build_root_items(&store, None);
        let search_base = build_search_items(&store, None);
        let existing = ExistingPanelState::default();

        // 空面板仍保持 5 条 Recent 的紧凑展示；非空搜索必须能命中第 6 条。
        assert_eq!(
            base.iter()
                .filter(|item| matches!(item, PanelItem::Target(_, _)))
                .count(),
            5
        );
        let root = root_items_with_existing_and_search(&base, &search_base, &existing, "recent-5");
        let rows = filter_panel_items(&root, "recent-5");
        assert_eq!(rows.len(), 1);
        assert!(matches!(&rows[0], PanelItem::Target(entry, _) if entry.config.name == "recent-5"));
    }

    #[test]
    fn root_search_shows_loading_until_existing_discovery_finishes() {
        let store = QuickConnectStore::in_memory();
        let base = build_root_items(&store, None);
        let existing = ExistingPanelState {
            probe_inflight: true,
            ..ExistingPanelState::default()
        };

        let root = root_items_with_existing(&base, &existing, "tmux");
        assert!(root.iter().any(|item| matches!(item, PanelItem::Loading)));
    }

    /// C9：Home 是扁平 runtime list，不是 local/SSH 目录。
    #[test]
    fn existing_items_home_is_flat_local_and_ssh_self() {
        let local = ExistingEntry {
            title: "mux-dup".into(),
            runtime: ExistingRuntime::Tmux,
            transport: ExistingTransport::Local,
            tmux_session: Some("mux-dup".into()),
            tmux_socket: None,
            herdr_session: None,
            herdr_workspace_id: None,
            herdr_socket: None,
        };
        let ssh_self = ExistingEntry {
            title: "mux-dup".into(),
            runtime: ExistingRuntime::Tmux,
            transport: ExistingTransport::Ssh {
                name: "self".into(),
            },
            tmux_session: Some("mux-dup".into()),
            tmux_socket: None,
            herdr_session: None,
            herdr_workspace_id: None,
            herdr_socket: None,
        };
        let items = existing_items(
            ExistingNav::Home,
            &[local],
            &["self".to_string()],
            false,
            |_| vec![ssh_self.clone()],
        );
        assert!(matches!(items[0], PanelItem::Back));
        assert!(
            !items.iter().any(|i| matches!(
                i,
                PanelItem::Folder {
                    id: "existing-local" | "existing-ssh",
                    ..
                }
            )),
            "禁止本地/SSH 目录: {items:?}"
        );
        assert!(
            !items.iter().any(|i| matches!(i, PanelItem::Host { .. })),
            "禁止 Host 行: {items:?}"
        );
        let existing: Vec<&ExistingEntry> = items
            .iter()
            .filter_map(|i| match i {
                PanelItem::Existing(e) => Some(e),
                _ => None,
            })
            .collect();
        assert_eq!(existing.len(), 2, "local + ssh-self 必须双份: {items:?}");
        assert!(existing
            .iter()
            .any(|e| { e.title == "mux-dup" && matches!(e.transport, ExistingTransport::Local) }));
        assert!(existing.iter().any(|e| {
            e.title == "mux-dup"
                && matches!(&e.transport, ExistingTransport::Ssh { name } if name == "self")
        }));
    }

    /// C9：widget_name 含 connect name，双份行才能共存。
    #[test]
    fn existing_row_widget_includes_connect_name() {
        let src = include_str!("quickconnect_panel_ui.rs");
        let start = src
            .find("PanelItem::Existing(entry) =>")
            .expect("Existing 行渲染应存在");
        let chunk = &src[start..];
        assert!(
            chunk[..chunk.find("PanelItem::Host").unwrap_or(chunk.len())]
                .contains("muxterm-existing-row-{}-{}-{}"),
            "Existing 行 widget_name 必须是 runtime-connect-id: {chunk}"
        );
    }

    /// C9：Home 空 + 探测中 → Loading；探测完空 → Empty；有行 → Existing。
    #[test]
    fn existing_items_home_empty_or_loading() {
        let loading = existing_items(ExistingNav::Home, &[], &[], true, |_| vec![]);
        assert!(matches!(loading[1], PanelItem::Loading));

        let empty = existing_items(ExistingNav::Home, &[], &[], false, |_| vec![]);
        assert!(matches!(empty[1], PanelItem::Empty { .. }));
    }

    #[test]
    fn existing_attach_request_preserves_typed_identity() {
        let tmux = ExistingEntry {
            title: "matrix".into(),
            runtime: ExistingRuntime::Tmux,
            transport: ExistingTransport::Local,
            tmux_session: Some("matrix".into()),
            tmux_socket: Some("muxterm-test-existing".into()),
            herdr_session: None,
            herdr_workspace_id: None,
            herdr_socket: None,
        }
        .open_request();
        assert_eq!(tmux.intent, crate::ffi_client::ClientOpenIntent::AttachOnly);
        let crate::ffi_client::ClientCandidateRef::Existing { identity } = tmux.candidate else {
            panic!("tmux row must produce an Existing candidate reference");
        };
        assert_eq!(identity.runtime_id, "tmux");
        assert_eq!(identity.transport_id, "local");
        assert_eq!(identity.target, "local");
        assert_eq!(identity.session.as_deref(), Some("matrix"));
        assert_eq!(identity.socket.as_deref(), Some("muxterm-test-existing"));
        assert!(identity.workspace_id.is_none());

        let herdr = ExistingEntry {
            title: "worktree".into(),
            runtime: ExistingRuntime::Herdr,
            transport: ExistingTransport::Local,
            tmux_session: None,
            tmux_socket: None,
            herdr_session: Some("named".into()),
            herdr_workspace_id: Some("w223".into()),
            herdr_socket: Some("/tmp/herdr.sock".into()),
        }
        .open_request();
        let crate::ffi_client::ClientCandidateRef::Existing { identity } = herdr.candidate else {
            panic!("Herdr row must produce an Existing candidate reference");
        };
        assert_eq!(identity.runtime_id, "herdr");
        assert_eq!(identity.transport_id, "local");
        assert_eq!(identity.target, "local");
        assert_eq!(identity.workspace_id.as_deref(), Some("w223"));
        assert_eq!(identity.session.as_deref(), Some("named"));
        assert_eq!(identity.socket.as_deref(), Some("/tmp/herdr.sock"));
    }

    /// W20：filter 对 Folder/Existing/Back 生效，Back 始终保留。
    #[test]
    fn filter_handles_existing_variants() {
        let items = vec![
            PanelItem::Folder {
                id: "existing-connections",
                title: "已有的连接".into(),
            },
            PanelItem::Back,
            PanelItem::Existing(ExistingEntry {
                title: "w1".into(),
                runtime: ExistingRuntime::Herdr,
                transport: ExistingTransport::Local,
                tmux_session: None,
                tmux_socket: None,
                herdr_session: Some("default".into()),
                herdr_workspace_id: Some("w1".into()),
                herdr_socket: None,
            }),
        ];
        let hit = filter_panel_items(&items, "已有");
        assert_eq!(hit.len(), 2, "Folder 按 title 过滤 + Back 始终保留");
        assert!(matches!(hit[0], PanelItem::Folder { .. }));
        let back = filter_panel_items(&items, "zzz");
        assert_eq!(back.len(), 1, "Back 始终保留");
        assert!(matches!(back[0], PanelItem::Back));
        let herdr = filter_panel_items(&items, "herdr @");
        assert_eq!(herdr.len(), 2, "Existing 按 subtitle 过滤 + Back 始终保留");
        assert!(
            herdr
                .iter()
                .any(|item| matches!(item, PanelItem::Existing(_))),
            "Existing 必须留下: {herdr:?}"
        );
        assert!(
            herdr.iter().any(|item| matches!(item, PanelItem::Back)),
            "Back 始终保留: {herdr:?}"
        );
        let at_herdr = filter_panel_items(&items, "@herdr");
        assert!(
            at_herdr
                .iter()
                .any(|item| matches!(item, PanelItem::Existing(_))),
            "@herdr 必须命中 Existing: {at_herdr:?}"
        );
    }

    /// C7：探测结束后空 host 表必须是 Empty，不能继续 Loading。
    #[test]
    fn ssh_hosts_empty_after_probe_must_not_stay_loading() {
        let src = include_str!("panel_model.rs");
        let start = src
            .find("pub struct ExistingPanelState")
            .expect("ExistingPanelState 应存在");
        let rest = &src[start..];
        let end = rest.find("\n///").unwrap_or(rest.len());
        let struct_src = &rest[..end.min(500)];
        assert!(
            struct_src.contains("probe_inflight"),
            "ExistingPanelState 必须有 probe_inflight（探测中 true / 完成后 false），空 host 才能从 Loading 变成 Empty。struct={struct_src}"
        );
    }

    #[test]
    fn filter_matches_subtitle_and_path() {
        let ssh = TargetConfig::new(
            "srv",
            TargetRuntime::Tmux,
            TargetTransport::Ssh {
                name: "ryzen".into(),
            },
            "~/work",
        );
        let items = vec![PanelItem::Target(
            QuickConnectEntry::new(ssh, vec![]),
            false,
        )];
        assert_eq!(filter_panel_items(&items, "ryzen").len(), 1);
        assert_eq!(filter_panel_items(&items, "work").len(), 1);
        assert_eq!(filter_panel_items(&items, "nomatch").len(), 0);
    }

    fn existing(
        title: &str,
        runtime: ExistingRuntime,
        transport: ExistingTransport,
    ) -> ExistingEntry {
        ExistingEntry {
            title: title.into(),
            runtime,
            transport,
            tmux_session: (runtime == ExistingRuntime::Tmux).then(|| title.to_string()),
            tmux_socket: None,
            herdr_session: (runtime == ExistingRuntime::Herdr).then(|| "default".to_string()),
            herdr_workspace_id: (runtime == ExistingRuntime::Herdr).then(|| title.to_string()),
            herdr_socket: None,
        }
    }

    #[test]
    fn filter_at_runtime_and_host_selects_existing_tmux_and_project() {
        let existing_ryzen = ExistingTransport::Ssh {
            name: "ryzen".into(),
        };
        let ryzen = TargetTransport::Ssh {
            name: "ryzen".into(),
        };
        let items = vec![
            PanelItem::Host {
                alias: "ryzen".into(),
            },
            PanelItem::Host {
                alias: "mac".into(),
            },
            PanelItem::Existing(existing(
                "dev",
                ExistingRuntime::Tmux,
                existing_ryzen.clone(),
            )),
            PanelItem::Existing(existing(
                "agents",
                ExistingRuntime::Herdr,
                existing_ryzen.clone(),
            )),
            PanelItem::Existing(existing(
                "local-dev",
                ExistingRuntime::Tmux,
                ExistingTransport::Local,
            )),
            PanelItem::Target(
                QuickConnectEntry::new(
                    TargetConfig::new("muxterm", TargetRuntime::Tmux, ryzen, "~/muxterm"),
                    vec![QuickBadge::Project],
                ),
                false,
            ),
        ];

        let hit = filter_panel_items(&items, "@tmux @ryzen");
        assert!(
            hit.iter()
                .any(|item| matches!(item, PanelItem::Existing(e) if e.title == "dev")),
            "ryzen 上已有的 tmux workspace 必须能选中连接: {hit:?}"
        );
        assert!(
            hit.iter().any(
                |item| matches!(item, PanelItem::Target(entry, _) if entry.config.name == "muxterm")
            ),
            "ryzen 上的 tmux project 必须能选中连接: {hit:?}"
        );
        assert!(
            hit.iter()
                .any(|item| matches!(item, PanelItem::Host { alias } if alias == "ryzen")),
            "@ryzen 也应留下 host 行: {hit:?}"
        );
        assert!(hit.iter().all(|item| match item {
            PanelItem::Existing(e) => {
                e.runtime == ExistingRuntime::Tmux
                    && matches!(&e.transport, ExistingTransport::Ssh { name } if name == "ryzen")
            }
            PanelItem::Target(entry, _) => {
                entry.config.runtime == TargetRuntime::Tmux
                    && matches!(&entry.config.transport, TargetTransport::Ssh { name } if name == "ryzen")
            }
            PanelItem::Host { alias } => alias == "ryzen",
            _ => false,
        }));

        let prefix = filter_panel_items(&items, "@tmux @ry");
        assert!(
            prefix
                .iter()
                .any(|item| matches!(item, PanelItem::Existing(e) if e.title == "dev")),
            "@ry 必须前缀命中 ryzen: {prefix:?}"
        );
        assert_eq!(
            filter_panel_items(&items, "@tmux")
                .iter()
                .filter(|item| matches!(item, PanelItem::Host { .. }))
                .count(),
            0
        );
    }
}
