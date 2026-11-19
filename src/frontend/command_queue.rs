//! UI-to-Core command queue shared by Rust frontends.
//!
//! UI handlers enqueue owned commands and return immediately.  The frontend
//! event loop flushes one batch through `FfiClient`; consecutive navigation
//! and resize commands are coalesced, while input keeps its original order.

use std::collections::VecDeque;

use crate::frontend::utils::corebridge::{ClientTask, FfiClient};

/// One command waiting for the next frontend flush.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientCommand {
    PaneColours {
        workspace_id: String,
        pane_id: u32,
        fg: String,
        bg: String,
    },
    Task {
        workspace_id: Option<String>,
        task: ClientTask,
    },
    Input {
        workspace_id: Option<String>,
        pane_id: u32,
        data: Vec<u8>,
        quiet: bool,
    },
    Resize {
        workspace_id: Option<String>,
        pane_id: Option<u32>,
        cols: u16,
        rows: u16,
    },
    ActivateWorkspace(String),
    CloseWorkspace(String),
    /// 用户看到了某个 pane：让 Core 记已读（不改变 Runtime 状态）。
    ///
    /// 对照 macOS `MainWindow.acknowledgeWorkspacePane` / `AttentionOperation.becameVisible`：
    /// 已读是 Core 的语义，前端不能只把行藏起来。
    AttentionViewed {
        workspace_id: String,
        pane_id: u32,
    },
    /// 设置窗口 Apply：一条 manifest 路径一个 patch，走 Core 的配置事务。
    ConfigApply {
        path: String,
        value: serde_json::Value,
    },
    /// 设置整份草稿原子提交；失败时 Core 回滚全部字段。
    ConfigApplyTransaction {
        patches: Vec<crate::frontend::utils::corebridge::ClientJsonPatchOperation>,
    },
    /// Catalog Candidate 身份完整回传（Herdr session/socket/workspace_id 不丢失）。
    OpenCandidate {
        request: crate::frontend::utils::corebridge::ClientOpenRequest,
    },
    /// 目标配置窗口保存：交给 Catalog resolver 打开 workspace。
    OpenTarget {
        target: crate::frontend::utils::corebridge::ClientTarget,
        intent: crate::frontend::utils::corebridge::ClientOpenIntent,
    },
    /// 重命名 tab（Core `rename_tab`；名字由前端输入，不能塞进 CTask）。
    RenameTab {
        workspace_id: String,
        tab_id: u32,
        name: String,
    },
    /// 重命名 workspace（Core `rename_workspace`）。
    RenameWorkspace {
        workspace_id: String,
        name: String,
    },
    /// 重排 tab：把 `from` 放到 `target` 前/后（Core `move_tab`）。
    MoveTab {
        workspace_id: String,
        from: u32,
        target: u32,
        before: bool,
    },
}

#[derive(Debug, Default)]
pub struct CommandQueue {
    pending: VecDeque<ClientCommand>,
}

impl CommandQueue {
    pub fn push(&mut self, command: ClientCommand) {
        if let Some(previous) = self.pending.back_mut() {
            if coalesces(previous, &command) {
                *previous = command;
                return;
            }
        }
        self.pending.push_back(command);
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn drain(&mut self) -> Vec<ClientCommand> {
        self.pending.drain(..).collect()
    }

    /// Submit the current batch without making UI callers wait for a result.
    /// Return codes are retained for diagnostics, not used as synchronous UI
    /// state; Core events remain the source of truth for completion.
    pub fn flush(&mut self, client: &FfiClient) -> Vec<i32> {
        self.drain()
            .into_iter()
            .map(|command| dispatch(client, command))
            .collect()
    }
}

fn coalesces(previous: &ClientCommand, next: &ClientCommand) -> bool {
    match (previous, next) {
        (
            ClientCommand::Task {
                workspace_id: previous_workspace,
                task: ClientTask::SwitchTab { .. },
            },
            ClientCommand::Task {
                workspace_id: next_workspace,
                task: ClientTask::SwitchTab { .. },
            },
        ) => previous_workspace == next_workspace,
        (ClientCommand::ActivateWorkspace(_), ClientCommand::ActivateWorkspace(_)) => true,
        (
            ClientCommand::Resize {
                workspace_id: previous_workspace,
                pane_id: previous_pane,
                ..
            },
            ClientCommand::Resize {
                workspace_id: next_workspace,
                pane_id: next_pane,
                ..
            },
        ) => previous_workspace == next_workspace && previous_pane == next_pane,
        (
            ClientCommand::AttentionViewed {
                workspace_id: previous_workspace,
                pane_id: previous_pane,
            },
            ClientCommand::AttentionViewed {
                workspace_id: next_workspace,
                pane_id: next_pane,
            },
        ) => previous_workspace == next_workspace && previous_pane == next_pane,
        _ => false,
    }
}

fn dispatch(client: &FfiClient, command: ClientCommand) -> i32 {
    match command {
        ClientCommand::PaneColours {
            workspace_id,
            pane_id,
            fg,
            bg,
        } => client.report_workspace_pane_colours(&workspace_id, pane_id, &fg, &bg),
        ClientCommand::Task { workspace_id, task } => workspace_id.map_or_else(
            || client.execute_task(task),
            |workspace_id| client.execute_workspace_task(&workspace_id, task),
        ),
        ClientCommand::Input {
            workspace_id,
            pane_id,
            data,
            quiet,
        } => match workspace_id {
            Some(workspace_id) if quiet => {
                client.send_workspace_input_quiet(&workspace_id, pane_id, &data)
            }
            Some(workspace_id) => client.send_workspace_input(&workspace_id, pane_id, &data),
            None if quiet => client.send_input_quiet(pane_id, &data),
            None => client.send_input(pane_id, &data),
        },
        ClientCommand::Resize {
            workspace_id,
            pane_id,
            cols,
            rows,
        } => match (workspace_id, pane_id) {
            (Some(workspace_id), Some(pane_id)) => {
                client.resize_workspace_pane(&workspace_id, pane_id, cols, rows)
            }
            (Some(workspace_id), None) => client.resize_workspace_client(&workspace_id, cols, rows),
            (None, Some(pane_id)) => client.resize_pane(pane_id, cols, rows),
            (None, None) => client.resize_client(cols, rows),
        },
        ClientCommand::AttentionViewed {
            workspace_id,
            pane_id,
        } => client.workspace_attention_on_became_visible(&workspace_id, pane_id),
        ClientCommand::ConfigApply { path, value } => {
            match client.config_apply_path(&path, value) {
                Ok(_) => 0,
                Err(_) => -1,
            }
        }
        ClientCommand::ConfigApplyTransaction { patches } => match client.config_apply(&patches) {
            Ok(_) => 0,
            Err(error) => {
                tracing::error!("settings transaction rejected: {error}");
                -1
            }
        },
        ClientCommand::OpenCandidate { request } => client.start_open(&request).map_or(-1, |_| 0),
        ClientCommand::OpenTarget { target, intent } => {
            match client.start_open_target(&target, intent) {
                Ok(_) => 0,
                Err(_) => -1,
            }
        }
        ClientCommand::RenameTab {
            workspace_id,
            tab_id,
            name,
        } => client.rename_workspace_tab(&workspace_id, tab_id, &name),
        ClientCommand::RenameWorkspace { workspace_id, name } => {
            client.rename_workspace(&workspace_id, &name)
        }
        ClientCommand::MoveTab {
            workspace_id,
            from,
            target,
            before,
        } => client.move_workspace_tab(&workspace_id, from, target, before),
        ClientCommand::ActivateWorkspace(workspace_id) => {
            client.activate_workspace(&workspace_id).map_or(-1, |_| 0)
        }
        ClientCommand::CloseWorkspace(workspace_id) => {
            client.close_workspace(&workspace_id).map_or(-1, |_| 0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ClientCommand, CommandQueue};
    use crate::frontend::utils::corebridge::{ClientTask, FfiClient};

    #[test]
    fn consecutive_switches_keep_only_the_last_target() {
        let mut queue = CommandQueue::default();
        queue.push(ClientCommand::Task {
            workspace_id: None,
            task: ClientTask::SwitchTab { tab_id: 61 },
        });
        queue.push(ClientCommand::Task {
            workspace_id: None,
            task: ClientTask::SwitchTab { tab_id: 0 },
        });

        assert_eq!(queue.len(), 1);
        assert_eq!(
            queue.drain(),
            vec![ClientCommand::Task {
                workspace_id: None,
                task: ClientTask::SwitchTab { tab_id: 0 },
            }]
        );
    }

    #[test]
    fn input_order_is_preserved_and_not_coalesced() {
        let mut queue = CommandQueue::default();
        for byte in *b"abc" {
            queue.push(ClientCommand::Input {
                workspace_id: None,
                pane_id: 7,
                data: vec![byte],
                quiet: false,
            });
        }

        assert_eq!(queue.len(), 3);
        assert_eq!(queue.drain().len(), 3);
    }

    #[test]
    fn resize_coalesces_only_for_the_same_workspace_and_pane() {
        let mut queue = CommandQueue::default();
        queue.push(ClientCommand::Resize {
            workspace_id: Some("one".into()),
            pane_id: Some(7),
            cols: 80,
            rows: 24,
        });
        queue.push(ClientCommand::Resize {
            workspace_id: Some("one".into()),
            pane_id: Some(7),
            cols: 120,
            rows: 40,
        });
        queue.push(ClientCommand::Resize {
            workspace_id: Some("two".into()),
            pane_id: Some(7),
            cols: 100,
            rows: 30,
        });

        assert_eq!(queue.len(), 2);
        assert_eq!(
            queue.drain()[0],
            ClientCommand::Resize {
                workspace_id: Some("one".into()),
                pane_id: Some(7),
                cols: 120,
                rows: 40,
            }
        );
    }

    /// 对照 macOS `CommandQueueTests.testRepeatedVisibilityAcknowledgeForOnePaneCoalesces`：
    /// 同一 pane 的重复“已看”只发一次，不同 pane 不能合并。
    #[test]
    fn repeated_view_marks_coalesce_only_for_the_same_pane() {
        let mut queue = CommandQueue::default();
        for pane_id in [3, 3, 4] {
            queue.push(ClientCommand::AttentionViewed {
                workspace_id: "one".into(),
                pane_id,
            });
        }

        assert_eq!(queue.len(), 2);
        assert_eq!(
            queue.drain(),
            vec![
                ClientCommand::AttentionViewed {
                    workspace_id: "one".into(),
                    pane_id: 3,
                },
                ClientCommand::AttentionViewed {
                    workspace_id: "one".into(),
                    pane_id: 4,
                },
            ]
        );
    }

    #[test]
    fn flush_dispatches_owned_workspace_batch_and_consumes_it() {
        let client = FfiClient::new_catalog().expect("catalog handle");
        let mut queue = CommandQueue::default();
        queue.push(ClientCommand::Input {
            workspace_id: Some("local//missing/shell/".into()),
            pane_id: 7,
            data: b"x".to_vec(),
            quiet: false,
        });

        let results = queue.flush(&client);

        assert_eq!(results.len(), 1);
        assert_ne!(results[0], 0, "catalog must reject the missing workspace");
        assert!(queue.is_empty());
    }
}
