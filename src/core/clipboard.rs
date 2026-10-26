//! 跨前端图片粘贴：Core 决定目标、异步传输、输入编码及实例有效性。
//! 前端只把系统剪贴板图片归一化为 PNG；不实现 SSH 或目标路径策略。
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Instant;

use crate::muxterm::Muxterm;
use crate::protocol::task::{Task, TaskOutcome};
use crate::protocol::{PaneId, WorkspaceId};
use anyhow::{bail, Context, Result};

pub const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;

pub(crate) struct PendingImagePaste {
    receiver: Receiver<Result<String>>,
    workspace: WorkspaceId,
    instance: u64,
    pane: PaneId,
    /// false 表示只落盘并返回路径（独立输入框）；true 才向原 pane 写入路径。
    inject: bool,
    /// 开始上传的时刻与字节数：完成/失败时写日志，才能看出「过一会才出现」慢在哪。
    started: Instant,
    bytes: usize,
}

/// 一次图片传输的结果：路径 + 是否已经写进原 pane。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImagePasteReady {
    pub path: String,
    pub injected: bool,
}

pub fn validate_png(bytes: &[u8]) -> Result<()> {
    if bytes.len() < 33
        || bytes.len() > MAX_IMAGE_BYTES
        || !bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        || bytes[8..16] != *b"\0\0\0\rIHDR"
    {
        bail!("clipboard image must be PNG, at most 20 MiB");
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
    if width == 0
        || height == 0
        || width > 16384
        || height > 16384
        || u64::from(width) * u64::from(height) > 64 * 1024 * 1024
    {
        bail!("clipboard image dimensions exceed the supported limit");
    }
    Ok(())
}

/// shell 安全路径；只插入参数，不执行，不自动回车。
fn path_input(path: &str, bracketed: bool) -> Vec<u8> {
    let quoted = if path
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"/._-".contains(&b))
    {
        path.to_string()
    } else {
        format!("'{}'", path.replace('\'', "'\\''"))
    };
    if bracketed {
        format!("\x1b[200~{quoted}\x1b[201~").into_bytes()
    } else {
        quoted.into_bytes()
    }
}

impl Muxterm {
    pub fn start_image_paste(
        &mut self,
        workspace: WorkspaceId,
        pane: PaneId,
        png: Vec<u8>,
    ) -> Result<()> {
        self.begin_image_transfer(workspace, pane, png, true)
    }

    /// 只把图片存到 workspace 所在机器并返回路径，不写 pane。
    ///
    /// 独立输入框用这条通路：路径由前端决定插在哪里，Core 不代替用户输入。
    pub fn start_image_store(
        &mut self,
        workspace: WorkspaceId,
        pane: PaneId,
        png: Vec<u8>,
    ) -> Result<()> {
        self.begin_image_transfer(workspace, pane, png, false)
    }

    fn begin_image_transfer(
        &mut self,
        workspace: WorkspaceId,
        pane: PaneId,
        png: Vec<u8>,
        inject: bool,
    ) -> Result<()> {
        validate_png(&png)?;
        if self.pending_image_paste.is_some() {
            bail!("an image paste is already in progress");
        }
        let ws = self
            .pool
            .get(&workspace)
            .context("image paste workspace is closed")?;
        if ws.state().pane(&pane).is_none() {
            bail!("image paste pane is closed");
        }
        let instance = ws.instance_id();
        let connection = self
            .connections
            .get(
                &workspace.transport,
                workspace.alias.as_deref().unwrap_or(""),
            )
            .context("image paste target connection is unavailable")?;
        let bytes = png.len();
        tracing::info!(
            target = "muxterm::clipboard",
            transport = %workspace.transport,
            alias = workspace.alias.as_deref().unwrap_or(""),
            pane = pane.0,
            bytes,
            inject,
            "image paste upload started"
        );
        let (sender, receiver) = mpsc::channel();
        std::thread::Builder::new()
            .name("muxterm-image-paste".into())
            .spawn(move || {
                let transfer = Instant::now();
                let result: Result<String> = connection
                    .store_temporary_file(&png, "png")
                    .map_err(Into::into);
                tracing::debug!(
                    target = "muxterm::clipboard",
                    transfer_ms = transfer.elapsed().as_millis() as u64,
                    ok = result.is_ok(),
                    "image paste transfer finished"
                );
                let _ = sender.send(result);
            })?;
        self.pending_image_paste = Some(PendingImagePaste {
            receiver,
            workspace,
            instance,
            pane,
            inject,
            started: Instant::now(),
            bytes,
        });
        Ok(())
    }

    /// None 表示仍在上传；Some 表示文件已落到目标机器。
    /// `injected` 为 true 时路径已写进原 pane；为 false 时由调用方拿到路径自行处理。
    pub fn poll_image_paste(&mut self) -> Result<Option<ImagePasteReady>> {
        let job = self
            .pending_image_paste
            .as_ref()
            .context("no image paste in progress")?;
        let result = match job.receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return Ok(None),
            Err(TryRecvError::Disconnected) => {
                Err(anyhow::anyhow!("image transfer worker stopped"))
            }
        };
        let job = self.pending_image_paste.take().unwrap();
        let elapsed_ms = job.started.elapsed().as_millis() as u64;
        let outcome = (|| -> Result<ImagePasteReady> {
            let path = result?;
            let ws = self
                .pool
                .get_mut(&job.workspace)
                .filter(|ws| ws.instance_id() == job.instance)
                .with_context(|| format!("original workspace closed; image retained at {path}"))?;
            if !job.inject {
                // 只存储：原 pane 关掉也没关系，文件已经在目标机器上，
                // 输入框照样能拿到路径。工作区实例被替换才拒绝。
                return Ok(ImagePasteReady {
                    path,
                    injected: false,
                });
            }
            if ws.state().pane(&job.pane).is_none() {
                bail!("original pane closed; image retained at {path}");
            }
            let data = path_input(&path, ws.pane_bracketed_paste(job.pane));
            match ws.execute(Task::WriteRaw {
                target: job.pane,
                data,
            })? {
                TaskOutcome::Done | TaskOutcome::Accepted { .. } => Ok(ImagePasteReady {
                    path,
                    injected: true,
                }),
                TaskOutcome::Rejected { reason } => {
                    bail!("image saved at {path}, but paste failed: {reason}")
                }
            }
        })();
        match &outcome {
            Ok(ImagePasteReady { path, injected }) => {
                let event = if *injected {
                    "image paste finished"
                } else {
                    "image store finished"
                };
                tracing::info!(
                    target = "muxterm::clipboard",
                    pane = job.pane.0,
                    bytes = job.bytes,
                    elapsed_ms,
                    path = %path,
                    inject = *injected,
                    "{event}"
                )
            }
            Err(error) => tracing::warn!(
                target = "muxterm::clipboard",
                pane = job.pane.0,
                bytes = job.bytes,
                elapsed_ms,
                error = %error,
                "image paste failed"
            ),
        }
        outcome.map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_quoted_and_never_executed() {
        assert_eq!(
            path_input("/tmp/muxterm-paste-abc.png", false),
            b"/tmp/muxterm-paste-abc.png"
        );
        assert_eq!(
            path_input("/tmp/a' $(x).png", true),
            b"\x1b[200~'/tmp/a'\\'' $(x).png'\x1b[201~"
        );
    }

    #[test]
    fn rejects_non_images_and_oversized_dimensions() {
        assert!(validate_png(b"not an image").is_err());
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend_from_slice(&1u32.to_be_bytes());
        png.extend_from_slice(&1u32.to_be_bytes());
        png.resize(33, 0);
        assert!(validate_png(&png).is_ok());
        png[16..20].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(validate_png(&png).is_err());
    }

    /// 把 tracing 输出收进内存，用来断言日志里真的有上传耗时和失败原因。
    #[derive(Clone, Default)]
    struct Captured(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for Captured {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
        type Writer = Captured;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    impl Captured {
        fn text(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
        }
    }

    /// 完成 `poll_image_paste` 一次，返回（结果，捕获到的日志）。
    fn finish_paste(
        send: Result<String>,
        drop_sender: bool,
        replace_workspace: bool,
        inject: bool,
    ) -> (Result<Option<ImagePasteReady>>, String) {
        use crate::runtime::MockRuntime;
        use crate::workspace::Workspace;
        let captured = Captured::default();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_max_level(tracing::Level::DEBUG)
            .with_writer(captured.clone())
            .finish();
        let result = tracing::subscriber::with_default(subscriber, || {
            let handle = crate::ffi::muxterm_catalog_new();
            assert!(!handle.is_null());
            let h = unsafe { &mut *handle };
            let id = WorkspaceId::new("local", None, "image-log", "shell", "/tmp");
            let ws = Workspace::new(
                id.clone(),
                "image-log".into(),
                Box::new(MockRuntime::with_single_pane()),
            );
            let instance = ws.instance_id();
            h.pool.insert_connected(ws);
            let (sender, receiver) = mpsc::channel();
            h.pending_image_paste = Some(PendingImagePaste {
                receiver,
                workspace: id.clone(),
                instance,
                pane: PaneId(1),
                inject,
                started: Instant::now() - std::time::Duration::from_millis(1500),
                bytes: 4096,
            });
            if replace_workspace {
                h.pool.insert_connected(Workspace::new(
                    id,
                    "replacement".into(),
                    Box::new(MockRuntime::with_single_pane()),
                ));
            }
            if drop_sender {
                drop(sender);
            } else {
                sender.send(send).unwrap();
            }
            let result = h.poll_image_paste();
            unsafe { crate::ffi::muxterm_free(handle) };
            result
        });
        (result, captured.text())
    }

    #[test]
    fn finished_paste_logs_elapsed_time_size_and_path() {
        let (result, log) =
            finish_paste(Ok("/tmp/muxterm-paste-log.png".into()), false, false, true);
        assert_eq!(
            result.unwrap(),
            Some(ImagePasteReady {
                path: "/tmp/muxterm-paste-log.png".into(),
                injected: true,
            })
        );
        assert!(log.contains("image paste finished"), "{log}");
        assert!(log.contains("/tmp/muxterm-paste-log.png"), "{log}");
        assert!(log.contains("bytes=4096"), "{log}");
        // 起始时间被回拨 1.5s：日志里的耗时必须是真实流逝时间，不是 0。
        let elapsed: u64 = log
            .split("elapsed_ms=")
            .nth(1)
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|value| value.parse().ok())
            .unwrap_or_else(|| panic!("no elapsed_ms in log: {log}"));
        assert!(elapsed >= 1500, "elapsed_ms={elapsed}\n{log}");
    }

    #[test]
    fn transfer_failure_is_logged_with_its_reason() {
        let (result, log) = finish_paste(
            Err(anyhow::anyhow!("SSH image transfer timed out")),
            false,
            false,
            true,
        );
        assert!(result.is_err());
        assert!(log.contains("image paste failed"), "{log}");
        assert!(log.contains("SSH image transfer timed out"), "{log}");
        assert!(!log.contains("image paste finished"), "{log}");
    }

    #[test]
    fn dead_worker_and_replaced_workspace_are_both_logged_as_failures() {
        let (dead, log) = finish_paste(Ok(String::new()), true, false, true);
        assert!(dead.is_err());
        assert!(log.contains("image transfer worker stopped"), "{log}");

        // 上传成功但原 workspace 已被同 ID 的新实例替换：文件已落盘，路径必须出现在日志
        // 里，用户才能找回——这正是「图片传了却没出现路径」时唯一的线索。
        let (replaced, log) =
            finish_paste(Ok("/tmp/muxterm-paste-kept.png".into()), false, true, true);
        assert!(replaced.is_err());
        assert!(log.contains("image paste failed"), "{log}");
        assert!(log.contains("/tmp/muxterm-paste-kept.png"), "{log}");
    }

    #[test]
    fn completion_targets_original_workspace_and_rejects_reopened_instances() {
        use crate::runtime::MockRuntime;
        use crate::workspace::Workspace;
        use std::sync::{Arc, Mutex};
        for replace_original in [false, true] {
            let handle = crate::ffi::muxterm_catalog_new();
            assert!(!handle.is_null());
            let h = unsafe { &mut *handle };
            let id = WorkspaceId::new("local", None, "image-test", "shell", "/tmp");
            let log = Arc::new(Mutex::new(Vec::new()));
            let mut backend = MockRuntime::with_single_pane();
            backend.executed_log = Some(log.clone());
            let ws = Workspace::new(id.clone(), "image-test".into(), Box::new(backend));
            let instance = ws.instance_id();
            h.pool.insert_connected(ws);
            let (sender, receiver) = mpsc::channel();
            h.pending_image_paste = Some(PendingImagePaste {
                receiver,
                workspace: id.clone(),
                instance,
                pane: PaneId(1),
                inject: true,
                started: Instant::now(),
                bytes: 0,
            });
            assert!(h.poll_image_paste().unwrap().is_none());
            if replace_original {
                h.pool.insert_connected(Workspace::new(
                    id.clone(),
                    "replacement".into(),
                    Box::new(MockRuntime::with_single_pane()),
                ));
            } else {
                let other = WorkspaceId::new("local", None, "other", "shell", "/tmp");
                h.pool.insert_connected(Workspace::new(
                    other.clone(),
                    "other".into(),
                    Box::new(MockRuntime::with_single_pane()),
                ));
                h.pool.activate(&other).unwrap();
            }
            sender
                .send(Ok("/tmp/muxterm-paste-test.png".into()))
                .unwrap();
            let result = h.poll_image_paste();
            if replace_original {
                assert!(result.is_err());
            } else {
                assert_eq!(
                    result.unwrap(),
                    Some(ImagePasteReady {
                        path: "/tmp/muxterm-paste-test.png".into(),
                        injected: true,
                    })
                );
            }
            let writes = log
                .lock()
                .unwrap()
                .iter()
                .filter(|task| matches!(task, Task::WriteRaw { .. }))
                .count();
            assert_eq!(writes, usize::from(!replace_original));
            unsafe {
                crate::ffi::muxterm_free(handle);
            }
        }
    }

    /// 独立输入框只拿路径：文件落盘、pane 不收到任何字节。
    /// pane 在传输期间关掉也不影响路径，因为图片已经在目标机器上。
    #[test]
    fn store_only_returns_the_path_without_writing_the_pane() {
        use crate::runtime::MockRuntime;
        use crate::workspace::Workspace;
        use std::sync::{Arc, Mutex};
        for close_pane in [false, true] {
            let handle = crate::ffi::muxterm_catalog_new();
            assert!(!handle.is_null());
            let h = unsafe { &mut *handle };
            let id = WorkspaceId::new("local", None, "image-store", "shell", "/tmp");
            let log = Arc::new(Mutex::new(Vec::new()));
            let mut backend = MockRuntime::with_single_pane();
            backend.executed_log = Some(log.clone());
            let ws = Workspace::new(id.clone(), "image-store".into(), Box::new(backend));
            let instance = ws.instance_id();
            h.pool.insert_connected(ws);
            let (sender, receiver) = mpsc::channel();
            h.pending_image_paste = Some(PendingImagePaste {
                receiver,
                workspace: id,
                instance,
                pane: PaneId(1),
                inject: false,
                started: Instant::now(),
                bytes: 0,
            });
            if close_pane {
                h.pool
                    .get_mut(&WorkspaceId::new(
                        "local",
                        None,
                        "image-store",
                        "shell",
                        "/tmp",
                    ))
                    .unwrap()
                    .execute(Task::ClosePane { target: PaneId(1) })
                    .unwrap();
            }
            sender
                .send(Ok("/tmp/muxterm-paste-store.png".into()))
                .unwrap();
            assert_eq!(
                h.poll_image_paste().unwrap(),
                Some(ImagePasteReady {
                    path: "/tmp/muxterm-paste-store.png".into(),
                    injected: false,
                })
            );
            let writes = log
                .lock()
                .unwrap()
                .iter()
                .filter(|task| matches!(task, Task::WriteRaw { .. }))
                .count();
            assert_eq!(writes, 0, "只存储不能向 pane 写路径");
            unsafe {
                crate::ffi::muxterm_free(handle);
            }
        }
    }
}
