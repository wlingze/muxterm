//! 进程采样独立于输出和 UI poll；安静的 Agent 也会被发现。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::Thread;
use std::time::Duration;

use tokio::sync::mpsc;

use super::PtyMsg;
use crate::protocol::PaneId;
use crate::transport::ProcessObserver;

pub(super) struct ProcessWatch {
    stopped: Arc<AtomicBool>,
    thread: Thread,
}

impl ProcessWatch {
    pub(super) fn start(
        pane: PaneId,
        mut observe: ProcessObserver,
        tx: mpsc::Sender<PtyMsg>,
    ) -> std::io::Result<Self> {
        let stopped = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stopped);
        let worker = std::thread::Builder::new()
            .name("muxterm-process-observer".into())
            .spawn(move || {
                let mut previous = None;
                while !worker_stop.load(Ordering::Acquire) && !tx.is_closed() {
                    if let Some(command) = observe().filter(|value| !value.trim().is_empty()) {
                        if worker_stop.load(Ordering::Acquire) {
                            break;
                        }
                        if previous.as_ref() != Some(&command)
                            && tx
                                .try_send(PtyMsg::Process {
                                    pane,
                                    command: command.clone(),
                                })
                                .is_ok()
                        {
                            previous = Some(command);
                        }
                    }
                    std::thread::park_timeout(Duration::from_millis(500));
                }
            })?;
        Ok(Self {
            stopped,
            thread: worker.thread().clone(),
        })
    }
}

impl Drop for ProcessWatch {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        self.thread.unpark();
    }
}
