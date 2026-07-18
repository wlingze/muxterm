//! tmux client ↔ UI 事件桥接（按需连接，不启动即连）。
//!
//! 用户在 tmux 集成对话框里选 attach/新建 session 后，上层调用 `spawn_bridge`
//! 启动后台 tokio task 跑 tmux `-CC`。事件经 `std::sync::mpsc` 跨线程送到 UI
//! 线程，UI 线程用 `glib::timeout_add_local`（16ms）轮询 `try_recv` 派发。
//! UI → tmux 命令走 `tokio::sync::mpsc`，后台 task 串行 `send_raw` 写 pty。

use std::sync::{mpsc as std_mpsc, Arc};

use gtk4::glib;
use tokio::sync::mpsc;

use crate::tmux::client::{ConnectMode, TmuxClient, TmuxClientConfig, TmuxEvent};
use crate::tmux::protocol::{Message, PaneId, WindowId};

/// 跨线程传递的 UI 事件。
#[derive(Debug, Clone)]
pub enum UiEvent {
    PaneOutput { pane: PaneId, data: Vec<u8> },
    WindowAdd { window: WindowId },
    WindowClose { window: WindowId },
    WindowRenamed { window: WindowId, name: String },
    SessionChanged { sid: u32, name: Option<String> },
    Exit { reason: Option<String> },
    Connected,
    Error { msg: String },
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

/// 启动 tmux 桥接。`on_event` 在 UI 线程被调用（通过 glib timeout 轮询）。
///
/// 返回 `CommandSender` 供 UI 线程把命令字符串发到后台写循环。
pub fn spawn_bridge<F>(config: TmuxClientConfig, on_event: F) -> Option<CommandSender>
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

    Some(CommandSender {
        tx: cmd_tx,
        rt: rt_handle,
    })
}

/// 构造一个 attach 模式的 `TmuxClientConfig`。
pub fn attach_config(session: &str) -> TmuxClientConfig {
    TmuxClientConfig {
        mode: Some(ConnectMode::Attach {
            target: Some(session.to_string()),
        }),
        cols: Some(100),
        rows: Some(30),
        ..Default::default()
    }
}

/// 构造一个 new-session 模式的 `TmuxClientConfig`。
pub fn new_session_config(name: Option<String>) -> TmuxClientConfig {
    TmuxClientConfig {
        mode: Some(ConnectMode::NewSession { name }),
        cols: Some(100),
        rows: Some(30),
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
        Message::SessionChanged { session, name } => Some(UiEvent::SessionChanged {
            sid: session.0,
            name,
        }),
        Message::Exit { reason } => Some(UiEvent::Exit { reason }),
        _ => None,
    }
}
