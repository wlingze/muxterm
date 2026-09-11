//! UI-to-Core command queue shared by Rust frontends.
//!
//! UI handlers enqueue owned commands and return immediately.  The frontend
//! event loop flushes one batch through `FfiClient`; consecutive navigation
//! and resize commands are coalesced, while input keeps its original order.

use std::collections::VecDeque;

use crate::frontend::ffi_client::{ClientTask, FfiClient};

/// One command waiting for the next frontend flush.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientCommand {
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
        _ => false,
    }
}

fn dispatch(client: &FfiClient, command: ClientCommand) -> i32 {
    match command {
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
    use crate::frontend::ffi_client::{ClientTask, FfiClient};

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
