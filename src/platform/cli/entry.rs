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
        // Session
        NewSession { .. } => None,
        KillSession { .. } => Some(Task::Shutdown),
        AttachSession { .. } => None,
        Detach { .. } => None,
        RenameSession { .. } => None,

        // Window
        NewWindow { name, .. } => Some(Task::NewWindow {
            name: name.clone(),
            command: None,
            workdir: None,
        }),
        KillWindow { target } => {
            let wid = target.or_else(|| state.active_window().map(|w| w.id))?;
            Some(Task::CloseWindow { target: wid })
        }
        SelectWindow { target } => Some(Task::SwitchWindow { target: *target }),
        RenameWindow { new_name } => {
            let wid = state.active_window()?.id;
            Some(Task::RenameWindow {
                target: wid,
                name: new_name.clone(),
            })
        }

        // Tab
        NewTab { name, window } => {
            let wid = window.or_else(|| state.active_window().map(|w| w.id))?;
            Some(Task::NewTab {
                window: wid,
                name: name.clone(),
                command: None,
                workdir: None,
            })
        }
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
        } => {
            let cols = width.unwrap_or(80);
            let rows = height.unwrap_or(24);
            Some(Task::ResizePane {
                target: *target,
                cols,
                rows,
            })
        }

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
        ListSessions
        | ListWindows { .. }
        | ListTabs { .. }
        | ListPanes { .. }
        | ListLayout { .. }
        | DumpState => None,
        DisplayMessage { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::backend::mock::MockBackend;
    use crate::core::model::TerminalModel;

    fn make_model() -> TerminalModel {
        TerminalModel::new(Box::new(MockBackend::with_single_pane()))
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
    fn list_sessions_returns_none() {
        let model = make_model();
        let task = cli_command_to_task(&CliCommand::ListSessions, model.state());
        assert!(task.is_none());
    }
}
