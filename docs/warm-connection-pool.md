# Warm Connection Pool 设计（QuickConnect 阶段 4）

## 目标

QuickConnect 已使用过的目标在切换时不立即关闭连接：前台只切换渲染，
后台连接继续维护（继续 poll、保留 terminal state / snapshot），按用户明确关闭、
TTL / memory pressure 回收；容量阈值只触发提醒，不自动做 LRU 淘汰。目标：
warm switch 明显快于 cold connect。

## ConnectionKey

决定“实际连接身份”的字段，避免仅用 name 冲突：

- `transport`：`local` / `ssh`
- `alias`：SSH host alias（local 为 nil）
- `session`：tmux session 名（shell runtime 也可有名字）
- `runtime`：`tmux` / `shell`
- `path`：起始目录

`Hashable`，字典键。注意：path 变化也构成不同连接身份（不同工作目录），
不能只看 name+transport。

## ConnectionSlot

每个 warm slot 持有：

- `CoreBridge`（真实连接句柄）
- `TerminalManager`（SwiftTerm 视图与状态）
- `FrameSnapshot` / layout（最近渲染快照）
- `lastUsedAt`（单调时钟）
- `lifecycle`：active / background / evicting

协议（Chrome 纯逻辑可测）：

```swift
protocol ConnectionSlotProtocol: AnyObject {
    var key: ConnectionKey { get }
    var lifecycle: ConnectionLifecycle { get set }
    var lastUsedAt: UInt64 { get set }
    func pollBackground()
    func evict(reason: ConnectionEvictionReason)
}
```

`pollBackground()`：后台继续 poll 事件、投喂 TerminalManager，但不调用
同步 `displayIfNeeded()`（保持 warm 而不阻塞）。`evict(reason:)`：
tmux 用 detach 保留 server/session；local shell 单独策略（不能误杀）。

## ConnectionPool

纯逻辑管理（无 AppKit 依赖）：

- active slot 至多一个；其余 background
- `acquire(key, create:)`：命中 active/background → reuse；未命中 → create
- `release(key)`：切到 background，不 shutdown
- `isOverCapacity` / `oldestBackgroundCandidates(limit:)`：超过 `maxSlots` 时提供
  最久未使用的 background 候选给 UI，由用户选择关闭；不会因 acquire 自动 LRU 淘汰
- `evictForCapacity()`：仅供显式容量清理路径使用；TTL 到期仍按策略淘汰
- `evictUnderMemoryPressure()`：策略回调 / 协议单测，不虚构真实压力
- 后台 poll：由 App 层 timer 驱动遍历 background slots，输出队列有上限

## 接入约束

- 已缓存目标切换只复用已有连接/视图状态，不执行 `swapBridge` 的 shutdown 路径
- tmux eviction 用 `detach`（保留 session），禁止 `kill-session`
- local shell 淘汰策略明确后单独实现
- 每个 warm slot 事件队列 / 输出缓存设上限，防止后台积压

## 验证

- 纯逻辑单测：LRU、TTL、acquire/release/evict、memory pressure 策略
- 2-slot 集成测试（local 两个 session 来回切换）
- warm switch 与 cold connect 的 P50/P95 测量（后续阶段）
