//! tmux client ↔ UI 事件桥接（按需连接）。
//!
//! 用户连上 tmux 后，后台 tokio task 跑 `tmux -CC`，把 `TmuxEvent` 转成线程安全
//! `UiEvent`，经 `std::sync::mpsc` 跨线程送 UI 线程，UI 线程用
//! `glib::timeout_add_local`（16ms）轮询 `try_recv` 派发。UI → tmux 命令走
//! `tokio::sync::mpsc`，后台 task 串行 `send_raw` 写 pty。
//!
//! **重要**：必须持有 [`TmuxBridge`]（内含 `Runtime`），否则 Runtime drop 会取消
//! 所有 task，表现为 attach 无反应 / `task was cancelled`。
//!
//! attach 后根据配置自动发 `set -g mouse on`（auto_mouse）。

use std::sync::{mpsc as std_mpsc, Arc};

use gtk4::glib;
use tokio::sync::mpsc;

use crate::core::ssh::{SshConfig, SshSession};
use crate::core::tmux::client::{ConnectMode, TmuxClient, TmuxClientConfig, TmuxEvent};
use crate::core::tmux::protocol::{Message, PaneId, WindowId};

#[derive(Debug, Clone)]
pub enum UiEvent {
    PaneOutput {
        pane: PaneId,
        data: Vec<u8>,
    },
    WindowAdd {
        window: WindowId,
    },
    WindowClose {
        window: WindowId,
    },
    WindowRenamed {
        window: WindowId,
        name: String,
    },
    LayoutChange {
        window: WindowId,
        layout: String,
        visible: Option<String>,
    },
    SessionChanged {
        sid: u32,
        name: Option<String>,
    },
    Exit {
        reason: Option<String>,
    },
    Connected,
    Error {
        msg: String,
    },
}

/// 命令发送器（UI 线程持有）。
pub struct CommandSender {
    tx: Arc<mpsc::Sender<String>>,
    rt: tokio::runtime::Handle,
}

impl CommandSender {
    pub fn send(&self, line: &str) {
        let tx = self.tx.clone();
        let line = line.to_string();
        self.rt.spawn(async move {
            let _ = tx.send(line).await;
        });
    }
}

/// 持有 tokio Runtime，保证 tmux I/O task 不被提前 cancel。
pub struct TmuxBridge {
    sender: CommandSender,
    /// 必须存活；drop 会关掉整个 runtime 与 tmux 子进程。
    _runtime: tokio::runtime::Runtime,
}

impl TmuxBridge {
    pub fn sender(&self) -> &CommandSender {
        &self.sender
    }
}

/// 启动 tmux 桥接。`on_event` 在 UI 线程被调用。返回持有 Runtime 的
/// [`TmuxBridge`]——调用方必须长期持有，不可立刻 drop。
pub fn spawn_bridge<F>(
    config: TmuxClientConfig,
    auto_mouse: bool,
    on_event: F,
) -> Option<TmuxBridge>
where
    F: Fn(&UiEvent) + 'static,
{
    let on_event = Arc::new(on_event);

    let (ui_tx, ui_rx) = std_mpsc::channel::<UiEvent>();
    let ui_rx = Arc::new(std::sync::Mutex::new(ui_rx));

    let (cmd_tx, mut cmd_rx) = mpsc::channel::<String>(64);
    let cmd_tx = Arc::new(cmd_tx);

    let rt = tokio::runtime::Runtime::new().ok()?;
    let rt_handle = rt.handle().clone();

    rt_handle.spawn(async move {
        let (mut tmux, mut rx) = match TmuxClient::spawn(config).await {
            Ok(v) => v,
            Err(e) => {
                let _ = ui_tx.send(UiEvent::Error {
                    msg: format!("连接 tmux 失败: {e}"),
                });
                return;
            }
        };
        let _ = ui_tx.send(UiEvent::Connected);

        // 自动开鼠标（tmux 鼠标：滚轮/点击选 pane/拖动分割）。
        if auto_mouse {
            if let Err(e) = tmux.send_raw("set -g mouse on\n").await {
                tracing::warn!(target = "muxterm::wiring", "set -g mouse on 失败: {e}");
            }
        }

        // 写任务持有 handle；读循环只消费 rx。Runtime 由 TmuxBridge 持有。
        tokio::spawn(async move {
            while let Some(line) = cmd_rx.recv().await {
                if let Err(e) = tmux.send_raw(&line).await {
                    tracing::error!(target = "muxterm::wiring", "发送命令失败: {e}");
                    break;
                }
            }
        });

        loop {
            match rx.recv().await {
                Some(TmuxEvent::Message(m)) => {
                    if let Some(ev) = to_ui_event(m) {
                        let _ = ui_tx.send(ev);
                    }
                }
                Some(TmuxEvent::ResponseLine { .. }) => {}
                Some(TmuxEvent::Exit { .. }) | None => {
                    let _ = ui_tx.send(UiEvent::Exit { reason: None });
                    break;
                }
            }
        }
    });

    // UI 线程轮询
    {
        let ui_rx = ui_rx.clone();
        let on_event = on_event.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(16), move || {
            let g = ui_rx.lock().unwrap();
            let mut evs: Vec<UiEvent> = Vec::new();
            while let Ok(ev) = g.try_recv() {
                evs.push(ev);
            }
            drop(g);
            for ev in &evs {
                on_event(ev);
            }
            glib::ControlFlow::Continue
        });
    }

    Some(TmuxBridge {
        sender: CommandSender {
            tx: cmd_tx,
            rt: rt_handle,
        },
        _runtime: rt,
    })
}

/// 启动 SSH → 远程 `tmux -CC` 桥接（事件/命令通道与本地 attach 相同）。
pub fn spawn_ssh_bridge<F>(
    ssh: SshConfig,
    session_name: String,
    auto_mouse: bool,
    on_event: F,
) -> Option<TmuxBridge>
where
    F: Fn(&UiEvent) + 'static,
{
    let on_event = Arc::new(on_event);

    let (ui_tx, ui_rx) = std_mpsc::channel::<UiEvent>();
    let ui_rx = Arc::new(std::sync::Mutex::new(ui_rx));

    let (cmd_tx, mut cmd_rx) = mpsc::channel::<String>(64);
    let cmd_tx = Arc::new(cmd_tx);

    let rt = tokio::runtime::Runtime::new().ok()?;
    let rt_handle = rt.handle().clone();

    rt_handle.spawn(async move {
        let session = match SshSession::connect(ssh).await {
            Ok(s) => s,
            Err(e) => {
                let _ = ui_tx.send(UiEvent::Error {
                    msg: format!("SSH 连接失败: {e}"),
                });
                return;
            }
        };
        let (mut remote, mut rx) = match session.spawn_tmux_cc(&session_name).await {
            Ok(v) => v,
            Err(e) => {
                let _ = ui_tx.send(UiEvent::Error {
                    msg: format!("远程 tmux -CC 失败: {e}"),
                });
                let _ = session.disconnect().await;
                return;
            }
        };
        let _ = ui_tx.send(UiEvent::Connected);

        if auto_mouse {
            if let Err(e) = remote.send_raw("set -g mouse on\n").await {
                tracing::warn!(
                    target = "muxterm::wiring",
                    "remote set -g mouse on 失败: {e}"
                );
            }
        }

        // 保持 SSH session 存活，直到写/读循环结束
        tokio::spawn(async move {
            while let Some(line) = cmd_rx.recv().await {
                if let Err(e) = remote.send_raw(&line).await {
                    tracing::error!(target = "muxterm::wiring", "SSH 发送命令失败: {e}");
                    break;
                }
            }
            let _ = remote.kill().await;
            let _ = session.disconnect().await;
        });

        loop {
            match rx.recv().await {
                Some(TmuxEvent::Message(m)) => {
                    if let Some(ev) = to_ui_event(m) {
                        let _ = ui_tx.send(ev);
                    }
                }
                Some(TmuxEvent::ResponseLine { .. }) => {}
                Some(TmuxEvent::Exit { .. }) | None => {
                    let _ = ui_tx.send(UiEvent::Exit { reason: None });
                    break;
                }
            }
        }
    });

    {
        let ui_rx = ui_rx.clone();
        let on_event = on_event.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(16), move || {
            let g = ui_rx.lock().unwrap();
            let mut evs: Vec<UiEvent> = Vec::new();
            while let Ok(ev) = g.try_recv() {
                evs.push(ev);
            }
            drop(g);
            for ev in &evs {
                on_event(ev);
            }
            glib::ControlFlow::Continue
        });
    }

    Some(TmuxBridge {
        sender: CommandSender {
            tx: cmd_tx,
            rt: rt_handle,
        },
        _runtime: rt,
    })
}

pub fn attach_config(session: &str, socket_args: &[String]) -> TmuxClientConfig {
    TmuxClientConfig {
        mode: Some(ConnectMode::Attach {
            target: Some(session.to_string()),
        }),
        cols: Some(100),
        rows: Some(30),
        extra_args: socket_args.to_vec(),
        ..Default::default()
    }
}

pub fn new_session_config(name: Option<String>, socket_args: &[String]) -> TmuxClientConfig {
    TmuxClientConfig {
        mode: Some(ConnectMode::NewSession { name }),
        cols: Some(100),
        rows: Some(30),
        extra_args: socket_args.to_vec(),
        ..Default::default()
    }
}

fn to_ui_event(m: Message) -> Option<UiEvent> {
    match m {
        Message::Output { pane, content, .. } => Some(UiEvent::PaneOutput {
            pane,
            data: content,
        }),
        Message::WindowAdd { window } => Some(UiEvent::WindowAdd { window }),
        Message::WindowClose { window } => Some(UiEvent::WindowClose { window }),
        Message::WindowRenamed { window, name } => Some(UiEvent::WindowRenamed { window, name }),
        Message::LayoutChange {
            window,
            layout,
            visible_layout,
        } => Some(UiEvent::LayoutChange {
            window,
            layout: layout.raw,
            visible: visible_layout.map(|v| v.raw),
        }),
        Message::SessionChanged { session, name } => Some(UiEvent::SessionChanged {
            sid: session.0,
            name,
        }),
        Message::Exit { reason } => Some(UiEvent::Exit { reason }),
        _ => None,
    }
}
