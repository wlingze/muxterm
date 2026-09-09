//! 从 main.rs 抽取的共享逻辑，供 daemon 调用。
//!
//! Compatibility exports for the shell-runtime daemon command mapper.

#[cfg(test)]
use crate::core::protocol::layout::SplitDir;
#[cfg(test)]
use crate::core::protocol::task::Task;
#[cfg(test)]
use crate::platform::cli::CliCommand;

pub use crate::core::runtime::shell::daemon::cli_command_to_task;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::runtime::mock::MockRuntime;
    use crate::core::workspace::terminal_model::TerminalModel;

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
