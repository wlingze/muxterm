//! 跨前端图片粘贴：Core 决定目标、异步传输、输入编码及实例有效性。
//! 前端只把系统剪贴板图片归一化为 PNG；不实现 SSH 或目标路径策略。
use std::sync::mpsc::{self, Receiver, TryRecvError};

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
        let (sender, receiver) = mpsc::channel();
        std::thread::Builder::new()
            .name("muxterm-image-paste".into())
            .spawn(move || {
                let result = connection
                    .store_temporary_file(&png, "png")
                    .map_err(Into::into);
                let _ = sender.send(result);
            })?;
        self.pending_image_paste = Some(PendingImagePaste {
            receiver,
            workspace,
            instance,
            pane,
        });
        Ok(())
    }

    /// None 表示仍在上传；Some(path) 表示已向原 pane 粘贴路径。
    pub fn poll_image_paste(&mut self) -> Result<Option<String>> {
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
        let path = result?;
        let ws = self
            .pool
            .get_mut(&job.workspace)
            .filter(|ws| ws.instance_id() == job.instance)
            .with_context(|| format!("original workspace closed; image retained at {path}"))?;
        if ws.state().pane(&job.pane).is_none() {
            bail!("original pane closed; image retained at {path}");
        }
        let data = path_input(&path, ws.pane_bracketed_paste(job.pane));
        match ws.execute(Task::WriteRaw {
            target: job.pane,
            data,
        })? {
            TaskOutcome::Done | TaskOutcome::Accepted { .. } => Ok(Some(path)),
            TaskOutcome::Rejected { reason } => {
                bail!("image saved at {path}, but paste failed: {reason}")
            }
        }
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

    #[test]
    fn completion_targets_original_workspace_and_rejects_reopened_instances() {
        use crate::runtime::mock::MockRuntime;
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
                assert_eq!(result.unwrap(), Some("/tmp/muxterm-paste-test.png".into()));
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
}
