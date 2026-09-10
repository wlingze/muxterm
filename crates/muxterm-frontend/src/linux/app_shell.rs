//! Linux 主窗口的固定骨架（AppShell）。

use gtk4::prelude::*;

use crate::linux::overlay::OverlayLayer;
use crate::linux::quickconnect::status_style::StatusBarMode;
use crate::linux::status_bar::StatusBar;
use crate::linux::theme::Theme;
use crate::linux::workspace_sidebar::WorkspaceSidebar;

/// 标题栏中的前端业务入口。
///
/// 标题栏只保存 GTK signal 的转发点，不读取 Core 状态；实际动作由主窗口
/// 通过闭包接回 frontend 的 state / command queue。
pub(crate) struct HeaderActions {
    quick_connect: gtk4::Button,
    settings: gtk4::Button,
}

impl HeaderActions {
    /// 连接标题栏的快速连接与设置入口。
    pub fn connect_actions<Q, S>(&self, on_quick_connect: Q, on_settings: S)
    where
        Q: Fn() + 'static,
        S: Fn() + 'static,
    {
        self.quick_connect
            .connect_clicked(move |_| on_quick_connect());
        self.settings.connect_clicked(move |_| on_settings());
    }
}

/// 主窗口的固定 widget 骨架。
///
/// AppShell 只负责组装 GTK widget 树；Core 状态、事件泵和命令处理仍由
/// `window::UiState` 持有。页面控制器从这里取出需要接线的 widget 句柄。
pub struct AppShell {
    pub(crate) sidebar: WorkspaceSidebar,
    pub(crate) status: StatusBar,
    pub(crate) overlay: OverlayLayer,
    pub(crate) header: HeaderActions,
}

impl AppShell {
    /// 创建并挂载主窗口骨架。
    pub fn new(
        window: &gtk4::Window,
        scene: &impl IsA<gtk4::Widget>,
        status_mode: StatusBarMode,
        theme: Theme,
    ) -> Self {
        let root = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(0)
            .build();
        root.add_css_class("muxterm-root");

        let sidebar = WorkspaceSidebar::new();
        let header = gtk4::HeaderBar::new();
        header.set_widget_name("muxterm-header-bar");
        header.pack_start(&sidebar.toggle);

        let quick_connect_button = gtk4::Button::with_label("⚡");
        quick_connect_button.set_widget_name("muxterm-quick-connect-button");
        quick_connect_button.set_has_frame(false);
        quick_connect_button.set_can_focus(false);
        header.pack_start(&quick_connect_button);

        let settings_button = gtk4::Button::with_label("⚙");
        settings_button.set_widget_name("muxterm-settings-button");
        settings_button.set_has_frame(false);
        settings_button.set_can_focus(false);
        header.pack_end(&settings_button);

        let title_label = gtk4::Label::new(Some("muxterm"));
        title_label.set_widget_name("muxterm-title-label");
        header.set_title_widget(Some(&title_label));
        window.set_titlebar(Some(&header));

        let status = StatusBar::new(status_mode, theme);
        status.container.add_css_class("status-bar");
        let overlay = OverlayLayer::new(scene);

        // 左侧栏与右侧终端 chrome 是同一个水平 Paned 的两列。Tab/status
        // chrome 属于右列，不能延伸到侧栏下方；Paned 的 handle 同时提供
        // 用户可调宽度，避免用一个 hexpand 空壳制造中间空白。
        let terminal_column = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(0)
            .hexpand(true)
            .vexpand(true)
            .build();
        terminal_column.set_widget_name("muxterm-terminal-column");
        terminal_column.append(&overlay.container);
        terminal_column.append(&status.container);

        let content = gtk4::Paned::new(gtk4::Orientation::Horizontal);
        content.set_widget_name("muxterm-content");
        content.add_css_class("muxterm-main-split");
        content.set_hexpand(true);
        content.set_vexpand(true);
        content.set_wide_handle(false);
        content.set_resize_start_child(false);
        content.set_shrink_start_child(false);
        content.set_resize_end_child(true);
        content.set_shrink_end_child(true);
        content.set_start_child(Some(&sidebar.container));
        content.set_end_child(Some(&terminal_column));
        content.set_position(280);
        root.append(&content);
        window.set_child(Some(&root));

        Self {
            sidebar,
            status,
            overlay,
            header: HeaderActions {
                quick_connect: quick_connect_button,
                settings: settings_button,
            },
        }
    }
}
