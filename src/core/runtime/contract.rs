//! Runtime 的公共契约。
//!
//! 这里定义的是一个已连接 Runtime 实例必须提供的行为；具体实现位于
//! `runtime/{shell,tmux,herdr}`。旧的 `core::model::backend` 路径暂时通过
//! re-export 保持兼容，迁移完成后将删除该兼容入口。

use crate::core::protocol::state::{BackendStatus, State, StateChange};
use crate::core::protocol::task::{Task, TaskOutcome};
use async_trait::async_trait;

/// Runtime 能力位：一个实现返回它真正支持的子集。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuntimeCapability {
    /// shutdown/关窗后远端还在，能再 attach。
    PersistDetach,
    /// 连接前能列出可 open 的候选。
    Discover,
    /// `NewTab` / `SwitchTab` 有意义。
    MultiTab,
    /// `SplitPane` 有意义。
    SplitPane,
    /// Runtime 的全部 pane 共享一个 client viewport。
    SharedClientResize,
    /// 能列出当前仓库的 checkout。
    WorktreeList,
    /// 能建 checkout 并打开成新 Workspace。
    WorktreeCreate,
    /// 能打开已有 checkout。
    WorktreeOpen,
    /// 能 `git worktree remove`（不删分支）。
    WorktreeRemove,
}

/// 一个 git worktree（产品能力，不是第三棵树）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeInfo {
    pub path: String,
    pub branch: String,
    pub repo_root: String,
    /// 池里已打开该 checkout 的 WorkspaceId。
    pub open_workspace: Option<crate::core::workspace::id::WorkspaceId>,
    /// 是否 linked worktree（false = 主 checkout）。
    pub linked: bool,
}

/// 创建 worktree 的产品规格（core 层，不拼 git 命令）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeCreateSpec {
    pub branch: String,
    pub path: String,
    pub base: Option<String>,
    pub label: Option<String>,
}

/// 终端运行时 trait。
#[async_trait]
pub trait Runtime: State + Send {
    /// 类型擦除下行转换（测试 / 诊断用）。
    fn as_any(&self) -> &dyn std::any::Any;

    /// 建立连接。
    async fn connect(&mut self) -> anyhow::Result<()>;

    /// 同步执行一个 Task。
    fn execute(&mut self, task: &Task) -> anyhow::Result<TaskOutcome>;

    /// 非阻塞拉取所有尚未消费的状态变更事件（FIFO）。
    fn take_events(&mut self) -> Vec<StateChange>;

    /// 当前运行时状态（`State::status` 的便捷别名）。
    fn runtime_status(&self) -> BackendStatus {
        self.status()
    }

    /// 当前 Runtime 真会做的能力子集。
    fn support(&self) -> &'static [RuntimeCapability] {
        &[]
    }

    /// 列出当前仓库 checkout。
    fn list_worktrees(&self) -> anyhow::Result<Vec<WorktreeInfo>> {
        Err(anyhow::anyhow!("runtime 不支持 WorktreeList"))
    }

    /// 创建 worktree 并返回新格 spec。
    fn create_worktree_spec(
        &self,
        _spec: &WorktreeCreateSpec,
    ) -> anyhow::Result<crate::core::workspace::spec::WorkspaceSpec> {
        Err(anyhow::anyhow!("runtime 不支持 WorktreeCreate"))
    }

    /// 打开已有 checkout 并返回新格 spec。
    fn open_worktree_spec(
        &self,
        _path: &str,
    ) -> anyhow::Result<crate::core::workspace::spec::WorkspaceSpec> {
        Err(anyhow::anyhow!("runtime 不支持 WorktreeOpen"))
    }

    /// 当前连接是否启用了 status bar 订阅。
    fn status_subscriptions_active(&self) -> bool {
        false
    }

    /// Pool 前台/后台切换通知。
    fn set_foreground(&mut self, _foreground: bool) {}

    /// 当前连接的读写字节计数 `(down, up)`。
    fn traffic_bytes(&self) -> (u64, u64) {
        (0, 0)
    }

    /// 关闭运行时并释放资源。
    async fn shutdown(&mut self) -> anyhow::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::runtime::shell::ShellRuntime;
    use crate::core::runtime::tmux::backend::TmuxRuntime;

    const WORKTREE_CAPS: [RuntimeCapability; 4] = [
        RuntimeCapability::WorktreeList,
        RuntimeCapability::WorktreeCreate,
        RuntimeCapability::WorktreeOpen,
        RuntimeCapability::WorktreeRemove,
    ];

    #[test]
    fn tmux_runtime_support_has_no_worktree() {
        let rt = TmuxRuntime::new(None);
        let caps = rt.support();
        assert!(caps.contains(&RuntimeCapability::PersistDetach));
        assert!(caps.contains(&RuntimeCapability::Discover));
        assert!(caps.contains(&RuntimeCapability::MultiTab));
        assert!(caps.contains(&RuntimeCapability::SplitPane));
        assert!(caps.contains(&RuntimeCapability::SharedClientResize));
        for c in WORKTREE_CAPS {
            assert!(!caps.contains(&c), "tmux 不应支持 {c:?}");
        }
    }

    #[test]
    fn shell_runtime_support_has_no_worktree() {
        let rt = ShellRuntime::new("$SHELL", "");
        let caps = rt.support();
        assert!(caps.contains(&RuntimeCapability::MultiTab));
        assert!(caps.contains(&RuntimeCapability::SplitPane));
        assert!(!caps.contains(&RuntimeCapability::SharedClientResize));
        assert!(!caps.contains(&RuntimeCapability::PersistDetach));
        assert!(!caps.contains(&RuntimeCapability::Discover));
        for c in WORKTREE_CAPS {
            assert!(!caps.contains(&c), "shell 不应支持 {c:?}");
        }
    }
}
