//! Runtime trait：一个统一的终端运行时抽象。
//!
//! TmuxRuntime 和 ShellRuntime 都实现此 trait，TerminalModel 持有 `Box<dyn Runtime>`。
//!
//! 设计要点：
//! - Runtime 维护并 `&mut self` 更新内部 State，实现 `State` trait 的只读视图。
//! - Runtime 接收 `Task`，把它映射到具体动作（tmux 命令 / 本地 spawn）。
//! - Runtime 通过通道推送 `StateChange` 事件（异步），前端/TerminalModel 订阅。
//! - 连接（connect）和关闭（shutdown）是异步方法。
//! - 协议解析器（`tmux::protocol`）、命令构造器（`tmux::command`）是 Runtime 的内部实现细节，不暴露给 TerminalModel。
use crate::core::model::state::{BackendStatus, State, StateChange};
use crate::core::model::task::{Task, TaskOutcome};
use async_trait::async_trait;

/// Runtime 能力位：一个实现返回它**真会做**的子集。
///
/// GUI / Pool / CLI 只根据 `support()` 切片决定要不要画入口、点了会不会被拒。
/// 禁止 `if spec.runtime == "herdr"` 之类的实现名判断。
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
    /// Runtime 的全部 pane 共享一个 client viewport；窗口 resize 应发
    /// `ResizeClient`，而不是按每个 Surface 发 `ResizePane`。
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
    /// 池里已打开该 checkout 的 WorkspaceId（Herdr `open_workspace_id` 映射后）。
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
///
/// 一个 Runtime 实例 = 一个 session 来源（本地 tmux / 远程 ssh tmux / 纯本地 shell）。
/// 同一时刻可能存在多个 Runtime（多 session），TerminalModel 聚合它们的 State。
///
/// 生命周期：
/// 1. `connect()` — 建立 connection / spawn tmux -CC / 初始化本地 shell
/// 2. `execute(Task)` — 反复执行任务
/// 3. `poll_events()` / `take_events()` — 取状态变更事件
/// 4. `shutdown()` — 释放 runtime 资源；tmux 会清理 control client
#[async_trait]
pub trait Runtime: State + Send {
    /// 类型擦除下行转换（测试 / 诊断用）。
    fn as_any(&self) -> &dyn std::any::Any;

    /// 建立连接（spawn tmux / 启动本地 shell）。
    /// 成功后 `status()` 应为 `Connected`。
    async fn connect(&mut self) -> anyhow::Result<()>;

    /// 同步执行一个 Task（不阻塞事件循环；内部若需 I/O 用 `tokio::spawn` 后台执行）。
    /// 返回 `Ok(Done)` 表示已派发；`Ok(Rejected{..})` 表示目标/状态不允许。
    /// 状态变更通过随后的事件流（`take_events`）推送。
    fn execute(&mut self, task: &Task) -> anyhow::Result<TaskOutcome>;

    /// 非阻塞拉取所有尚未消费的状态变更事件（FIFO）。
    /// 前端用 `glib::timeout_add_local` 16ms 轮询；TerminalModel 也用它聚合。
    fn take_events(&mut self) -> Vec<StateChange>;

    /// 当前运行时状态（`State::status` 的便捷别名，语义一致）。
    fn runtime_status(&self) -> BackendStatus {
        self.status()
    }

    /// 当前 Runtime 真会做的能力子集（GUI/Pool 只据此决策）。
    fn support(&self) -> &'static [RuntimeCapability] {
        &[]
    }

    /// 列出当前仓库 checkout（需 `WorktreeList`；无能力默认 Err）。
    fn list_worktrees(&self) -> anyhow::Result<Vec<WorktreeInfo>> {
        Err(anyhow::anyhow!("runtime 不支持 WorktreeList"))
    }

    /// 创建 worktree 并返回新格 spec（需 `WorktreeCreate`；无能力默认 Err）。
    fn create_worktree_spec(
        &self,
        _spec: &WorktreeCreateSpec,
    ) -> anyhow::Result<crate::core::workspace::spec::WorkspaceSpec> {
        Err(anyhow::anyhow!("runtime 不支持 WorktreeCreate"))
    }

    /// 打开已有 checkout 并返回新格 spec（需 `WorktreeOpen`；无能力默认 Err）。
    fn open_worktree_spec(
        &self,
        _path: &str,
    ) -> anyhow::Result<crate::core::workspace::spec::WorkspaceSpec> {
        Err(anyhow::anyhow!("runtime 不支持 WorktreeOpen"))
    }

    /// 当前运行时是否已启用 status bar 订阅（`refresh-client -B`）。
    /// 非 tmux 运行时 / tmux < 3.2 返回 false，前端回退轮询。
    fn status_subscriptions_active(&self) -> bool {
        false
    }

    /// 当前连接的读写字节计数 `(down, up)`；非 SSH 运行时默认 0。
    fn traffic_bytes(&self) -> (u64, u64) {
        (0, 0)
    }

    /// 关闭运行时并释放资源；显式 tmux 分离请使用 `Task::Detach`。
    /// 关闭后 `status()` 应为 `Exited` 或 `Disconnected`。
    async fn shutdown(&mut self) -> anyhow::Result<()>;
}

pub mod mock;

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

    /// H0：tmux 必须报 PersistDetach/Discover/MultiTab/SplitPane，
    /// 且 v1 绝不报任何 Worktree*（避免 GUI 为假能力画入口）。
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

    /// H0：shell 只报 MultiTab/SplitPane，不报 PersistDetach/Discover/Worktree*。
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
