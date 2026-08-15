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
pub trait Runtime: State {
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
