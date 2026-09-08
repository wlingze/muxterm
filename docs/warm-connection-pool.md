# Warm Connection Pool — 已废止

> **历史设计（QuickConnect 阶段 4）。不要按本文实现。**
> 废止日期：2026-09-08。
> 现行契约：[`WORKSPACE.md`](WORKSPACE.md)（WorkspacePool + ConnectionRegistry）、
> [`SURFACE.md`](SURFACE.md) §8–§9（常驻 Scene、去锁）。

## 废止原因

本文描述的 frontend `ConnectionSlot`（持有 `CoreBridge` + `TerminalManager`、
`lifecycle = active | background`、`pollBackground()`、warm vs cold）就是
2026-09-08 dogfood 里秒级激活延迟的根因：

- 每 slot 一把 `bridgeLock`
- 所有后台 poll 与前台校准共用一条串行 `backgroundPollQueue`
- 激活后再做一次「权威校准」

那**不是** tmux 全局锁。详见 [`SURFACE.md`](SURFACE.md) §9。

## 被什么取代

| 旧 | 新 |
|---|---|
| frontend ConnectionPool / WarmConnectionSlot | Core `WorkspacePool`（live Workspace owner） |
| slot 里的 `CoreBridge` 生命周期 | 常驻 Scene；FFI handle 在 Muxterm 组合根 |
| `pollBackground()` | 单 EventPump drain 三 lane 批次 |
| `ConnectionKey` 当连接身份 | Candidate / WorkspaceId；连接复用是 `transport::ConnectionRegistry` |
| 前台只切换渲染、后台继续 poll | 没有前后台之分：已打开 Scene 常驻、事件常流 |
| 点击后再校准 | 点击路径零 Core 调用 |

仍有效、迁到新文档的约束：

- tmux 关闭 = **detach**，禁止 `kill-session`
- 容量是软提醒，不静默 LRU 淘汰仍在使用的 Workspace
- 「切换不必全量重连」是产品目标，由 Core 连接复用 + 常驻 Scene 实现，不是 frontend slot

## 附录：原文摘要（考古）

当时每个 warm slot 持有 CoreBridge、TerminalManager、FrameSnapshot、`lastUsedAt`、
`lifecycle`。`acquire` 命中则 reuse，未命中才 create。后台由 App timer 遍历 slot。

不要把这套搬到 Rust，也不要在 Linux GTK 再实现一份。
