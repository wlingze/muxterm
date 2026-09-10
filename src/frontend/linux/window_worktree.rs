//! GTK 主窗口发起的 worktree 创建动作。

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{Box, Button, Entry, Orientation, Window};

use super::window_event_pump::sync_view_store;
use super::{after_activate, parse_workspace_id, UiState};

/// worktree 创建对话框：分支 + 路径，Create 后后台建 checkout 并开新格。
pub(super) fn show_worktree_create_dialog(state: &Rc<RefCell<UiState>>, parent: &Window) {
    let dialog = Window::builder()
        .title("新建 worktree")
        .modal(true)
        .transient_for(parent)
        .default_width(460)
        .build();
    let vbox = Box::new(Orientation::Vertical, 8);
    vbox.set_margin_top(12);
    vbox.set_margin_bottom(12);
    vbox.set_margin_start(12);
    vbox.set_margin_end(12);
    let branch = Entry::builder()
        .placeholder_text("分支名（如 feat/xxx）")
        .build();
    branch.set_widget_name("muxterm-worktree-create-branch");
    let path = Entry::builder()
        .placeholder_text("checkout 路径（如 /tmp/muxterm-test-herdr-wt-1）")
        .build();
    path.set_widget_name("muxterm-worktree-create-path");
    let create = Button::with_label("创建");
    create.set_widget_name("muxterm-worktree-create-confirm");
    let cancel = Button::with_label("取消");
    let row = Box::new(Orientation::Horizontal, 8);
    row.append(&cancel);
    row.append(&create);
    vbox.append(&branch);
    vbox.append(&path);
    vbox.append(&row);
    dialog.set_child(Some(&vbox));

    let dlg = dialog.clone();
    cancel.connect_clicked(move |_| dlg.close());
    let st = state.clone();
    let dlg = dialog.clone();
    create.connect_clicked(move |_| {
        let branch_text = branch.text().to_string();
        let path_text = path.text().to_string();
        if branch_text.trim().is_empty() || path_text.trim().is_empty() {
            return;
        }
        create_worktree(
            &st,
            branch_text.trim().to_string(),
            path_text.trim().to_string(),
        );
        dlg.close();
    });
    dialog.present();
}

/// 通过 Core FFI 创建 native worktree，并在成功后刷新 owned workspace DTO。
fn create_worktree(state: &Rc<RefCell<UiState>>, branch: String, path: String) {
    let source_workspace_id = {
        let s = state.borrow();
        Some(s.active_workspace_key())
    };
    let Some(source_workspace_id) = source_workspace_id else {
        return;
    };
    let result = {
        let s = state.borrow();
        s.event_pump.client().create_native_worktree(
            &source_workspace_id,
            &branch,
            &path,
            None,
            None,
        )
    };
    match result {
        Ok(opened) => {
            let mut s = state.borrow_mut();
            if let Err(error) = sync_view_store(&mut s) {
                tracing::warn!(target = "muxterm::linux", %error, "worktree snapshot refresh failed");
                return;
            }
            if parse_workspace_id(&opened.id).is_some() {
                after_activate(&mut s);
            }
        }
        Err(error) => {
            let detail = error.to_string();
            tracing::error!(
                target = "muxterm::linux",
                "worktree create failed: {detail}"
            );
            state
                .borrow_mut()
                .notification_log
                .push(format!("worktree create failed: {detail}"));
        }
    }
}
