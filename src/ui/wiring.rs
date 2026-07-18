//! tmux client ↔ UI 事件桥接。
//!
//! 后台 tokio task 跑 tmux client，把 `TmuxEvent` 转成线程安全的 `UiEvent`，
//! 通过 `std::sync::mpsc` 跨线程送到 UI 线程；UI 线程用一个 `glib::timeout`
//! 轮询 source 不断 `try_recv` 并派发给回调。UI → tmux 的命令走
//! `tokio::sync::mpsc`，由后台 task 串行写入 tmux pty。

use std::sync::{mpsc as std_mpsc, Arc};

use gtk4::glib;
use tokio::sync::mpsc;

use crate::tmux::client::{TmuxClient, TmuxClientConfig, TmuxEvent};
use crate::tmux::protocol::{Message, PaneId, WindowId};

/// 跨线程传递的 UI 事件（无 GTK 对象引用，实现 Send）。
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
    /// 发送一条已构造好的命令行到 tmux。
    pub fn send(&self, line: &str) {
        let tx = self.tx.clone();
        let line = line.to_string();
        self.rt.spawn(async move {
            let _ = tx.send(line).await;
        });
    }
}

/// 启动桥接：spawn tmux client，把事件转发到 UI 主循环。
///
/// 返回的 `CommandSender` 供 UI 线程把命令字符串发到后台写循环。
/// `on_event` 在 UI 线程被调用（通过 `glib::timeout_add_local` 轮询）。
pub fn spawn_bridge<F>(config: TmuxClientConfig, on_event: F) -> Option<CommandSender>
where
    F: Fn(&UiEvent) + 'static,
{
    let on_event = Arc::new(on_event);

    // 后台 → UI：std 同步通道（UI 线程 try_recv，非阻塞）
    let (ui_tx, ui_rx) = std_mpsc::channel::<UiEvent>();
    let ui_rx = Arc::new(std::sync::Mutex::new(ui_rx));

    // UI → tmux：tokio 异步通道
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<String>(64);
    let cmd_tx = Arc::new(cmd_tx);

    // 后台 tokio runtime
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

        // 写循环：从 cmd_rx 取命令写入 tmux
        tokio::spawn(async move {
            while let Some(line) = cmd_rx.recv().await {
                if let Err(e) = tmux.send_raw(&line).await {
                    tracing::error!(target = "muxterm::wiring", "发送命令失败: {e}");
                    break;
                }
            }
        });

        // 读循环：从 tmux 取事件，转成 UiEvent 送 UI 通道
        loop {
            match rx.recv().await {
                Some(TmuxEvent::Message(m)) => {
                    if let Some(ev) = to_ui_event(m) {
                        let _ = ui_tx.send(ev);
                    }
                }
                Some(TmuxEvent::ResponseLine { .. }) => {
                    // 命令响应正文行：UI 不直接展示
                }
                Some(TmuxEvent::Exit { .. }) | None => {
                    let _ = ui_tx.send(UiEvent::Exit { reason: None });
                    break;
                }
            }
        }
    });

    // UI 线程侧：轮询 ui_rx，每 16ms 取一批事件派发给 on_event。
    {
        let ui_rx = ui_rx.clone();
        let on_event = on_event.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(16), move || {
            let g = ui_rx.lock().unwrap();
            // 非阻塞取所有就绪事件
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
