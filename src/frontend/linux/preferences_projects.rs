//! Preferences 中 Project 管理页的 GTK view/controller。
//!
//! 主设置页只负责 schema-driven draft transaction；Project 列表有自己的
//! 刷新、编辑和文件监视生命周期，因此单独放在这个页面模块中。

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Button, Label, ListBox, Orientation, ScrolledWindow, Window};

use crate::frontend::linux::quickconnect::model::ProjectDocument;
use crate::frontend::linux::quickconnect::store::QuickConnectStore;
use crate::frontend::utils::corebridge::{ClientConfigSnapshot, ClientRuntimeInfo, SshHostEntry};

use super::{install_preferences_css, replace, subwindow_header, ConfigApi};

fn project_documents(snapshot: &ClientConfigSnapshot) -> anyhow::Result<Vec<ProjectDocument>> {
    Ok(serde_json::from_value(snapshot.values["projects"].clone())?)
}

fn persist_projects(
    config: &ConfigApi,
    projects: &[ProjectDocument],
) -> anyhow::Result<ClientConfigSnapshot> {
    config.apply(&[replace("/projects", serde_json::to_value(projects)?)])
}

pub(super) fn show_project_manager(
    parent: &impl IsA<Window>,
    config_path: PathBuf,
    config: ConfigApi,
    runtimes: Vec<ClientRuntimeInfo>,
    hosts: Vec<SshHostEntry>,
    on_changed: Rc<Box<dyn Fn() + 'static>>,
) {
    install_preferences_css();
    let win = Window::builder()
        .title("Projects")
        .transient_for(parent)
        .modal(true)
        .default_width(560)
        .default_height(480)
        .build();
    win.set_widget_name("muxterm-projects-window");
    win.add_css_class("muxterm-preferences-window");
    let root = GtkBox::builder()
        .orientation(Orientation::Vertical)
        .spacing(10)
        .margin_top(16)
        .margin_bottom(16)
        .margin_start(16)
        .margin_end(16)
        .build();
    root.add_css_class("prefs-subwindow");
    root.append(&subwindow_header(
        "Projects",
        "Saved workspace profiles for Quick Connect.",
    ));

    let list = ListBox::new();
    list.set_selection_mode(gtk4::SelectionMode::Single);
    list.add_css_class("prefs-list-card");
    let sw = ScrolledWindow::builder()
        .min_content_height(280)
        .child(&list)
        .build();
    sw.add_css_class("prefs-subwindow-scroll");
    root.append(&sw);

    let actions = GtkBox::builder()
        .orientation(Orientation::Horizontal)
        .spacing(8)
        .halign(Align::End)
        .build();
    let add = Button::with_label("New project…");
    add.add_css_class("suggested-action");
    let close = Button::with_label("Close");
    close.add_css_class("prefs-secondary-action");
    actions.append(&add);
    actions.append(&close);
    root.append(&actions);
    win.set_child(Some(&root));

    let refresh = {
        let list = list.clone();
        let config = config.clone();
        let win_for_rows = win.clone();
        let hosts_for_rows = hosts.clone();
        let runtimes_for_rows = runtimes.clone();
        let on_changed_for_rows = on_changed.clone();
        move || {
            while let Some(child) = list.first_child() {
                list.remove(&child);
            }
            let projects = match config
                .describe()
                .and_then(|snapshot| project_documents(&snapshot))
            {
                Ok(projects) => projects,
                Err(error) => {
                    tracing::warn!(target = "muxterm::config", "读取 Project 配置失败: {error}");
                    return;
                }
            };
            let project_store = QuickConnectStore::from_project_documents(&projects);
            for project in &projects {
                let row = GtkBox::builder()
                    .orientation(Orientation::Horizontal)
                    .spacing(8)
                    .margin_top(10)
                    .margin_bottom(10)
                    .margin_start(12)
                    .margin_end(12)
                    .build();
                row.add_css_class("prefs-project-row");
                let copy = GtkBox::builder()
                    .orientation(Orientation::Vertical)
                    .spacing(2)
                    .hexpand(true)
                    .build();
                let label = Label::new(Some(&project.name));
                label.set_hexpand(true);
                label.set_halign(Align::Start);
                label.add_css_class("prefs-project-name");
                copy.append(&label);
                let detail = Label::new(Some(&format!(
                    "{}  ·  {} / {}",
                    project.path, project.transport.id, project.runtime.id
                )));
                detail.set_halign(Align::Start);
                detail.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
                detail.add_css_class("prefs-project-detail");
                copy.append(&detail);
                row.append(&copy);
                let edit = Button::with_label("Edit…");
                edit.add_css_class("prefs-inline-action");
                let remove = Button::with_label("Remove");
                remove.add_css_class("destructive-action");
                let project_for_edit = project.clone();
                let config_api_for_edit = config.clone();
                let store_for_edit = project_store.clone();
                let on_changed = on_changed_for_rows.clone();
                let win = win_for_rows.clone();
                let hosts = hosts_for_rows.clone();
                let runtimes = runtimes_for_rows.clone();
                edit.connect_clicked(move |_| {
                    let store = Rc::new(RefCell::new(store_for_edit.clone()));
                    let target = match project_for_edit.to_draft() {
                        Ok(target) => target,
                        Err(error) => {
                            tracing::error!(
                                target = "muxterm::config",
                                "Project 解析失败: {error}"
                            );
                            return;
                        }
                    };
                    let store_inner = store.clone();
                    let config = config_api_for_edit.clone();
                    let on_changed = on_changed.clone();
                    crate::frontend::linux::target_config_window::show(
                        &win,
                        Some(target),
                        store.borrow().clone(),
                        hosts.clone(),
                        runtimes.clone(),
                        move |saved| {
                            let mut store = store_inner.borrow_mut();
                            store.upsert_project(&saved);
                            let projects = store.project_documents();
                            drop(store);
                            match persist_projects(&config, &projects) {
                                Ok(_) => on_changed(),
                                Err(error) => tracing::error!(
                                    target = "muxterm::config",
                                    "保存 Project 失败: {error}"
                                ),
                            }
                        },
                        || {},
                    );
                });
                let project_for_remove = project.clone();
                let config_for_remove = config.clone();
                let on_changed = on_changed_for_rows.clone();
                remove.connect_clicked(move |_| {
                    let mut projects = match config_for_remove
                        .describe()
                        .and_then(|snapshot| project_documents(&snapshot))
                    {
                        Ok(projects) => projects,
                        Err(error) => {
                            tracing::error!(
                                target = "muxterm::config",
                                "读取 Project 配置失败: {error}"
                            );
                            return;
                        }
                    };
                    projects.retain(|item| item.id != project_for_remove.id);
                    if persist_projects(&config_for_remove, &projects).is_ok() {
                        on_changed();
                    }
                });
                row.append(&edit);
                row.append(&remove);
                list.append(&row);
            }
        }
    };
    let refresh = Rc::new(RefCell::new(refresh));
    refresh.borrow()();

    // 文件变更（含本窗口的编辑/删除）后自动重建列表，避免自引用闭包。
    if let Ok(monitor) = gtk4::gio::File::for_path(&config_path).monitor_file(
        gtk4::gio::FileMonitorFlags::NONE,
        gtk4::gio::Cancellable::NONE,
    ) {
        let config = config.clone();
        let refresh = refresh.clone();
        monitor.connect_changed(move |_, _, _, _| {
            if config.reload().is_ok() {
                refresh.borrow()();
            }
        });
    }

    {
        let win = win.clone();
        close.connect_clicked(move |_| win.close());
    }
    {
        let win = win.clone();
        let hosts = hosts.clone();
        let runtimes = runtimes.clone();
        let config = config.clone();
        let on_changed = on_changed.clone();
        let refresh = refresh.clone();
        add.connect_clicked(move |_| {
            let projects = match config
                .describe()
                .and_then(|snapshot| project_documents(&snapshot))
            {
                Ok(projects) => projects,
                Err(error) => {
                    tracing::error!(target = "muxterm::config", "读取 Project 配置失败: {error}");
                    return;
                }
            };
            let store = Rc::new(RefCell::new(QuickConnectStore::from_project_documents(
                &projects,
            )));
            let store_inner = store.clone();
            let config_for_save = config.clone();
            let refresh = refresh.clone();
            let on_changed = on_changed.clone();
            crate::frontend::linux::target_config_window::show(
                &win,
                None,
                store.borrow().clone(),
                hosts.clone(),
                runtimes.clone(),
                move |saved| {
                    let mut store = store_inner.borrow_mut();
                    store.upsert_project(&saved);
                    let projects = store.project_documents();
                    drop(store);
                    match persist_projects(&config_for_save, &projects) {
                        Ok(_) => {
                            on_changed();
                            refresh.borrow()();
                        }
                        Err(error) => tracing::error!(
                            target = "muxterm::config",
                            "保存 Project 失败: {error}"
                        ),
                    }
                },
                || {},
            );
        });
    }
    win.present();
}
