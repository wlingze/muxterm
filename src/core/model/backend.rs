//! Backend trait：一个统一的终端后端抽象。
//!
//! TmuxBackend 和 LocalBackend 都实现此 trait，TerminalModel 持有 `Box<dyn Backend>`。
//!
//! 设计要点：
//! - Backend 维护并 `&mut self` 更新内部 State，实现 `State` trait 的只读视图。
//! - Backend 接收 `Task`，把它映射到具体动作（tmux 命令 / 本地 spawn）。
//! - Backend 通过通道推送 `StateChange` 事件（异步），前端/TerminalModel 订阅。
//! - 连接（connect）和关闭（shutdown）是异步方法。
//! - 协议解析器（`tmux::protocol`）、命令构造器（`tmux::command`）是 Backend 的内部实现细节，不暴露给 TerminalModel。
use crate::core::model::state::{BackendStatus, State, StateChange};
use crate::core::model::task::{Task, TaskOutcome};
use async_trait::async_trait;

/// 终端后端 trait。
///
/// 一个 Backend 实例 = 一个 session 来源（本地 tmux / 远程 ssh tmux / 纯本地 shell）。
/// 同一时刻可能存在多个 Backend（多 session），TerminalModel 聚合它们的 State。
///
/// 生命周期：
/// 1. `connect()` — 建立 connection / spawn tmux -CC / 初始化本地 shell
/// 2. `execute(Task)` — 反复执行任务
/// 3. `poll_events()` / `take_events()` — 取状态变更事件
/// 4. `shutdown()` — detach / kill / 关闭所有子进程
#[async_trait]
pub trait Backend: State {
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

    /// 当前后端状态（`State::status` 的便捷别名，语义一致）。
    fn backend_status(&self) -> BackendStatus {
        self.status()
    }

    /// 关闭后端：detach（tmux）/ kill 所有子进程（local）。
    /// 关闭后 `status()` 应为 `Exited` 或 `Disconnected`。
    async fn shutdown(&mut self) -> anyhow::Result<()>;
}

pub mod mock;
