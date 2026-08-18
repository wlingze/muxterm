//! 从 main.rs 抽取的共享逻辑，供 daemon 调用。
//!
//! `cli_command_to_task` 把 CliCommand 映射到 TerminalModel 的 Task，
//! daemon 和 CLI 直接调用模式都需要它。

use crate::core::model::layout::SplitDir;
use crate::core::model::task::Task;
use crate::platform::cli::CliCommand;

/// 把 CliCommand 转成 TerminalModel 的 Task。
///
/// 查询命令（list-*, capture-pane, display-message）返回 None。
pub fn cli_command_to_task(
    cmd: &CliCommand,
    state: &dyn crate::core::model::state::State,
) -> Option<Task> {
    use crate::core::model::task::Task;
    use crate::core::protocol::terminal::input::KeyEvent;
    use CliCommand::*;

    match cmd {
        Config { .. } => None,

        // Workspace
        NewWorkspace { .. } => None,
        CloseWorkspace { .. } => Some(Task::Shutdown),
        AttachWorkspace { .. } => None,
        Detach { .. } => Some(Task::Detach),
        RenameWorkspace { new_name } => Some(Task::RenameWorkspace {
            name: new_name.clone(),
        }),

        // Tab
        NewTab { name } => Some(Task::NewTab {
            name: name.clone(),
            command: None,
            workdir: None,
        }),
        KillTab { target } => {
            let tid = target.or_else(|| state.active_tab().map(|t| t.id))?;
            Some(Task::CloseTab { target: tid })
        }
        SelectTab { target } => Some(Task::SwitchTab { target: *target }),
        RenameTab { new_name } => {
            let tid = state.active_tab()?.id;
            Some(Task::RenameTab {
                target: tid,
                name: new_name.clone(),
            })
        }

        // Pane
        SplitPane {
            horizontal, target, ..
        } => {
            let pid = target.or_else(|| state.active_pane().map(|p| p.id));
            let dir = if *horizontal {
                SplitDir::Horizontal
            } else {
                SplitDir::Vertical
            };
            Some(Task::SplitPane {
                target: pid,
                dir,
                command: None,
                workdir: None,
            })
        }
        KillPane { target } => {
            let pid = target.or_else(|| state.active_pane().map(|p| p.id))?;
            Some(Task::ClosePane { target: pid })
        }
        SelectPane { target } => Some(Task::SwitchPane { target: *target }),
        ResizePane {
            target,
            width,
            height,
        } => match (width, height) {
            (Some(cols), Some(rows)) => Some(Task::ResizePane {
                target: *target,
                cols: *cols,
                rows: *rows,
            }),
            (Some(size), None) => Some(Task::ResizePaneAxis {
                target: *target,
                dir: SplitDir::Horizontal,
                size: *size,
            }),
            (None, Some(size)) => Some(Task::ResizePaneAxis {
                target: *target,
                dir: SplitDir::Vertical,
                size: *size,
            }),
            (None, None) => None,
        },
        ResizeClient { width, height } => Some(Task::ResizeClient {
            cols: *width,
            rows: *height,
        }),

        // 输入输出
        SendKeys { target, text } => {
            let pid = target.or_else(|| state.active_pane().map(|p| p.id))?;
            let keys = text.chars().map(KeyEvent::Char).collect();
            Some(Task::SendKeys { target: pid, keys })
        }
        WriteRaw { target, data } => {
            let pid = target.or_else(|| state.active_pane().map(|p| p.id))?;
            Some(Task::WriteRaw {
                target: pid,
                data: data.clone(),
            })
        }
        CapturePane { .. } => None,

        // 查询命令
        ListWorkspaces | ListTabs | ListPanes { .. } | ListLayout | DumpState => None,
        DisplayMessage { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::backend::mock::MockRuntime;
    use crate::core::model::TerminalModel;

    fn make_model() -> TerminalModel {
        TerminalModel::new(Box::new(MockRuntime::with_single_pane()))
    }

    #[test]
    fn split_pane_horizontal_maps_to_task() {
        let model = make_model();
        let task = cli_command_to_task(
            &CliCommand::SplitPane {
                horizontal: true,
                target: Some(crate::core::types::PaneId(1)),
                size: None,
            },
            model.state(),
        );
        assert!(matches!(
            task,
            Some(Task::SplitPane {
                dir: SplitDir::Horizontal,
                ..
            })
        ));
    }

    #[test]
    fn list_workspaces_returns_none() {
        let model = make_model();
        let task = cli_command_to_task(&CliCommand::ListWorkspaces, model.state());
        assert!(task.is_none());
    }

    #[test]
    fn detach_maps_to_explicit_core_task() {
        let model = make_model();
        let task = cli_command_to_task(&CliCommand::Detach { target: None }, model.state());
        assert_eq!(task, Some(Task::Detach));
    }

    #[test]
    fn resize_pane_single_axis_maps_to_axis_task() {
        let model = make_model();
        let horizontal = cli_command_to_task(
            &CliCommand::ResizePane {
                target: crate::core::types::PaneId(1),
                width: Some(60),
                height: None,
            },
            model.state(),
        );
        assert!(matches!(
            horizontal,
            Some(Task::ResizePaneAxis {
                dir: SplitDir::Horizontal,
                size: 60,
                ..
            })
        ));

        let vertical = cli_command_to_task(
            &CliCommand::ResizePane {
                target: crate::core::types::PaneId(1),
                width: None,
                height: Some(18),
            },
            model.state(),
        );
        assert!(matches!(
            vertical,
            Some(Task::ResizePaneAxis {
                dir: SplitDir::Vertical,
                size: 18,
                ..
            })
        ));
    }

    #[test]
    fn resize_client_maps_to_task() {
        let model = make_model();
        let task = cli_command_to_task(
            &CliCommand::ResizeClient {
                width: 120,
                height: 36,
            },
            model.state(),
        );
        assert_eq!(
            task,
            Some(Task::ResizeClient {
                cols: 120,
                rows: 36,
            })
        );
    }

    /// write-raw 的原始字节必须原样进入 Task::WriteRaw。
    #[test]
    fn write_raw_maps_to_task_with_bytes() {
        let model = make_model();
        let data = b"\x1b]10;rgb:0000/0000/0000\x1b\\".to_vec();
        let task = cli_command_to_task(
            &CliCommand::WriteRaw {
                target: Some(crate::core::types::PaneId(1)),
                data: data.clone(),
            },
            model.state(),
        );
        assert_eq!(
            task,
            Some(Task::WriteRaw {
                target: crate::core::types::PaneId(1),
                data,
            })
        );
    }
}
