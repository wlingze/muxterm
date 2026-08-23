# HERDR-RUNTIME-STABILITY.md — Herdr Runtime 稳定性契约

> 状态：实施前设计定案（2026-08-22）
> 适用分支：`feature/runtime/support_herdr`
> 最终复核：`2026-08-22T19:31:35+08:00`（CST；本机版本、官方 release metadata、
> PR #20 required checks 均已重新只读核对）
> 实施计划：[`../.plan-herdr-runtime-stabilization-20260822.md`](../.plan-herdr-runtime-stabilization-20260822.md)
> 测试契约：[`HERDR-TESTING.md`](HERDR-TESTING.md)

本文是 Herdr runtime 稳定化的专项设计契约。产品层级仍以
[`WORKSPACE.md`](WORKSPACE.md) 为准，Runtime 公共语义以
[`RUNTIME.md`](RUNTIME.md) 为准，像素路径以 [`SURFACE.md`](SURFACE.md)
为准，发现与打开以 [`CATALOG.md`](CATALOG.md) 为准。本文只补充这些总契约
没有展开的 Herdr stream 生命周期、创建收敛和连接身份规则；若本文与上述总契约
冲突，先修正文档冲突，不允许实现 agent 自行选择一份。

---

## 1. 问题与证据

2026-08-22 的四份复现日志暴露的是三条共同故障链，不是四个孤立 UI bug。

| 证据 | 用户可见现象 | 已确认的机制问题 |
|---|---|---|
| `test_2026-0822-1517.log` | tab1 正常；tab2 的 shell 固定在上方约 3/4；其他 tab 乱码；Ctrl-L 后残留旧屏 | attach 时把每个 pane 的大体积 `pane.read visible_ansi` 当 Surface 字节重放；所有 pane 又同时申请 writable control；结构、尺寸和 full frame 的顺序没有锁死 |
| `test_2026-0822-1530.log` | 点击 `+` 创建 tab 后卡住 | `tab.create` 响应被乐观写进本地拓扑，但新 root pane 没有经过统一 seed/stream 初始化；按钮路径还额外同步刷新 UI |
| `test_2026-0822-1531.log` | Alt+T 创建后渲染失败，tab 名为空 | 与按钮路径共享同一个初始化缺口；`name=None` 被序列化成空 label，覆盖 Herdr 的自动数字名 |
| `test_2026-0822-1533.log` | split 在 Herdr 服务端已成功，Muxterm 卡住并占满一个 CPU | 新 pane `@40` 的多个 control stream 相互 takeover；旧 stream 的 Close/Error 能删除并重建新 stream，形成无界循环 |

日志中的 `seed_raw` 可达到 100–393KB；`1533` 连续出现
`terminal attach taken over` 与 `读 Herdr 帧长度失败`。当前实现又把
`ObserveStream` 存在 `Vec` 中，事件只有 pane id，没有 stream generation。因此同一
pane 的旧事件和新事件不可区分，这足以解释 takeover 风暴和 CPU 自旋。

证据强度要分开：上述 wire payload、`Vec` 所有权、空 label 和同步 response 应用都是日志/
源码直接证据；“它们分别造成 3/4 高度、乱码、旧屏复活”的完整因果链仍必须由本文对应的
最小 RED 和修复后 GREEN 闭合。实现 agent 不能把设计推断冒充已经运行过的回归证据，也
不能因偶发手工正常就否定确定性的状态机缺口。

---

## 2. 目标与非目标

### 2.1 目标

1. 一个 Herdr pane 在任意时刻至多有一个 Muxterm stream transition 在执行。
2. 当前前台 pane 持有 writable control；所有后台 pane 使用 read-only observe。
3. 旧 stream 的 Frame/Closed/Error 永远不能修改新 stream 的状态或像素。
4. `pane.read`、`terminal.frame`、Workspace Index 和原生 VTE 各自职责明确。
5. tab/pane 创建只在 Herdr 权威 snapshot 收敛后宣告成功；不能靠一次响应猜最终布局。
6. Project 与 Existing Connection 对同一 Herdr workspace 生成相同的 runtime identity、
   attach spec 身份字段和 `WorkspaceId`；Project 自己的 path/name 元数据不能在归一化时丢失。
7. 所有失败有界、可诊断，并且不能在 GTK 主循环里 busy-loop 或长时间阻塞。

### 2.2 非目标

- 不修改 Herdr 或 herdrm 仓库；`~/Developer/terminal/{herdr,herdrm}` 只读参考，并遵守
  下述 protocol 版本边界。
- 不引入产品层 Session、虚拟 Window 或 Herdr 专属 GUI 分支。
- 不把连接池搬进 platform；Workspace 生命周期仍由 Core 的 Pool 管理。
- 不用 `visible_ansi`、Replica/Index dump 或定时 reset 修像素问题。
- 不远程安装或启动 Herdr；SSH 只连接已存在的 named/default server。
- 不以 loopback SSH 自动化声称任意真实外部 Host、密钥或 ProxyJump 已验证。

### 2.3 Herdr 参考版本边界

2026-08-22T19:31:35+08:00 的最终只读核对结果：

- 本机 client/server 都是 Herdr 0.8.0、protocol 19，`herdr status --json` 报
  `compatible=true`；
- Muxterm `src/core/runtime/herdr/wire.rs` 也固定 `HERDR_PROTOCOL_VERSION = 19`；
- `~/Developer/terminal/herdr` 当前是 `master@9166e07`，其 `wire.rs` 已经是 protocol 20；
- `~/Developer/terminal/herdrm/CLAUDE.md` 单独记录了 0.8.0/protocol 19 的 live socket
  语义。

因此，Herdr 当前 checkout 只能参考 ownership、observe/control、mutation 和事件收敛的
概念，不能直接复制 enum discriminant、字段顺序或握手版本。实现目标以安装的 v0.8.0、
Muxterm protocol-19 wire 测试和官方 v0.8.0 release 为准；握手版本不一致必须明确失败，
禁止自动降级后继续解帧。

---

## 3. 不变量

实现必须同时满足以下不变量；任何一个被破坏都属于回归。

### 3.1 Stream 不变量

- `PaneId` 是 stream registry 的唯一 key，不能再用允许重复项的 `Vec` 表示所有权。
- 每次 start/replace/promote/demote 都创建单调递增的 `generation`。
- reader 线程产生的每个事件都带 `(pane, generation, event_ordinal)`；Frame 另外保留 Herdr
  wire `seq`，不得丢弃。
- Runtime 只接受当前 generation 且 `event_ordinal` 严格递增的事件；Frame 的 wire `seq`
  也必须按本节规则收敛。
- writable control 另有单调递增的 `control_intent_epoch`。generation 标识一次具体 socket
  stream；intent 标识一次用户明确的 control 意图，二者不能混用。
- 外部 takeover 后必须进入 `SuppressedAfterTakeover`；重复 snapshot、poll、resize 或
  reconciliation 不能清掉这个闩锁，也不能再次自动申请 control。
- generation 递增必须发生在旧 stream shutdown 之前；这样旧 reader 即使晚到也已经失效。
- 一个 pane 同时最多存在一个 `Starting` 或 `Backoff` transition。
- Runtime detach/shutdown 后，任何旧事件都不能重新启动 stream。

### 3.2 Surface 不变量

- 一个已经进入产品拓扑的 pane 只有一个常驻 VT Surface；“常驻”不要求它当前挂在可见
  GTK widget tree 上。
- Surface registry 的 key 是 `(WorkspaceId, PaneId)`，不是“当前 active tab 的 pane id”。已在
  产品拓扑中的 pane 即使位于隐藏 tab 或后台 workspace，也保留同一个 PaneView 并继续
  接收 current-generation 原始 frame/output；GTK 只是不绘制隐藏 widget。
- `terminal.frame` 解开的 ANSI 字节可以进入 Surface；`pane.read visible_ansi` 不能进入 Surface。
- full frame 是当前 generation 的原始完整帧，不是永久历史；它不得触发
  `visible_ansi -> reset -> feed`。
- tab/workspace 切换只 show/hide 已存在的 PaneView，不销毁再播种。
- 结构和字符格尺寸必须先收敛，Surface frame/output 后 feed。
- Ctrl-L 清掉的画面不能被旧 snapshot、旧 generation 或 tab 切换恢复。

### 3.3 拓扑不变量

- Herdr `session.snapshot` 和后续 event snapshot 是 Tab/Pane/Layout/Focus 的权威来源。
- `tab.create`/`pane.split` 的直接响应只提供“等待哪个 id”的线索，不是最终拓扑。
- 新 pane 无论先从命令响应还是 snapshot 被看见，都必须通过同一个幂等初始化入口。
- split 完成条件必须同时包含：新 pane 已在 layout 中、split tree 可定位它、
  `focused_pane_id` 与期望一致。

### 3.4 产品边界不变量

- GUI 只能通过统一 Runtime/Pool 接口表达 foreground，禁止
  `if runtime == "herdr"`。
- Project、Recent、Existing 都必须经 `Catalog::open_target` →
  `Catalog::open_resolved`；后者调用 Pool 的 descriptor-aware 打开入口。platform 与这些
  产品入口都不能直接调用裸 `WorkspacePool::open_spec`。
- Herdr session/socket/workspace id 不能只存在于 Linux widget 或 side table。

---

## 4. Pane stream registry

### 4.1 数据模型

`HerdrRuntime` 用 pane-keyed registry 取代 `Vec<ObserveStream>`。概念结构如下；字段名
可以按 Rust 风格微调，但语义不能省略。

```text
PaneStreamSlot
  pane: PaneId
  target: Herdr pane id
  desired_mode: Observe | Control
  actual_mode: Option<Observe | Control>
  generation: u64
  last_event_ordinal: u64
  last_frame_seq: Option<u64>
  state: Absent | Starting | Live | Backoff | Degraded | Stopped
  stream: Option<ObserveStream>
  retry_count: u8
  retry_at: Option<Instant>
  live_since: Option<Instant>
  control_intent_epoch: u64
  control_rearm: Armed | SuppressedAfterTakeover
  surface_baseline: AwaitingFull | Ready
  pre_full_output: ordered queue, max 256 events and 2 MiB total
  pending_input: control-intent-bound ordered queue, max 256 writes and 64 KiB total
  pending_resize: control-intent-bound latest Option<(cols, rows)>
```

reader channel 的事件形状固定为：

```text
Frame  { pane, generation, event_ordinal, wire_seq, bytes, width, height, full }
Closed { pane, generation, event_ordinal, reason }
Error  { pane, generation, event_ordinal, message }
```

`event_ordinal` 由每条 reader 线程从 1 开始递增，不跨 generation 比较，用于 stale
Closed/Error 与测试注入排序。`wire_seq` 必须原样取自 `TerminalFrame.seq`；它是 Herdr
per-client 的单调帧序号，也随新 stream/generation 重置。Frame 收敛规则锁死为：

- `wire_seq <= last_frame_seq`：重复/倒序，丢弃；
- `wire_seq == last_frame_seq + 1`：正常应用；
- 出现缺口且该帧 `full=false`：不能跳过缺失 diff，当前 generation 进入有界失败；
- 出现缺口但该帧 `full=true`：允许它建立新的完整 baseline，并记录 gap 诊断；
- generation 的首个可应用 Surface 帧仍必须是 full；先到的 diff 只进入有界 pre-full queue。

### 4.2 模式选择

Herdr 协议 19 提供两种不同语义：

- `ObserveTerminal`：只读，可有多个 observer，不拥有输入、resize 或 takeover 权。
- `ControlTerminal`：可写，一个终端同时只有一个 controller。

Muxterm 的策略锁死为：

| Workspace/Pane 状态 | desired mode |
|---|---|
| Pool 当前 active workspace 的 active pane | Control |
| active workspace 的隐藏 pane 或隐藏 tab | Observe |
| Pool 后台 workspace 的所有 pane | Observe |
| detach/shutdown/已关闭 pane | Stopped，无 stream |

统一 Runtime trait 增加默认 no-op 的 `set_foreground(bool)`。Pool 在 active/background
转换时调用它；HerdrRuntime 再结合自己的 active pane 计算 desired mode。tmux/shell
不需要实现特殊行为。

切 tab、切 pane和切 workspace 都执行一次 reconciliation：先算出所有 pane 的
desired mode，再对实际 registry 做最小变更。禁止在多个事件 handler 中分别
start/replace 同一 pane。

`desired_mode=Control` 只表示产品希望当前 pane 可写，不等于允许无限 takeover。slot 若为
`SuppressedAfterTakeover`，其 effective mode 保持 Observe，直到新的本地用户 focus edge
或真实 input 创建新的 `control_intent_epoch`。来自 Herdr 的重复 focus snapshot、相同
Pool active 状态和 resize 都不是新的用户意图。

仅由 open/reattach/`set_foreground(true)`/Pool activate 触发的首次 Control 尝试必须
`takeover=false`：没有别的 controller 时可正常获得 control，有冲突时降 Observe。只有
`Task::SwitchPane` 等真实本地 focus edge，或用户第一笔真实 input，才允许该 intent 的首次
Control handshake 使用 `takeover=true`；第一笔 input 进入 intent-bound queue，handshake 后
恰好一次送达。

### 4.3 Promote、demote 与输入

- Observe → Control：先推进 generation 并关闭旧 observe，再启动一个 control；只有
  用户显式切到该 pane 或实际输入时允许 `takeover=true`。
- Control → Observe：先推进 generation，关闭旧 control，再启动 observe；旧 control
  reader 的 EOF/Closed 因 generation 过期而被丢弃。
- 每次本地显式 focus edge，或在 suppressed pane 上收到第一笔真实输入时，递增
  `control_intent_epoch`、清除 takeover suppression，并且至多启动一次 promote。重复的
  权威 focus snapshot 不能冒充这个 edge。suppression 清除后、同一 intent 的
  Starting/Backoff 期间继续到达的输入只追加到该 intent 的有界队列，不能每个 write 都再建
  一个 intent 或重新启动 stream。
- 向非 active pane 写输入前，产品焦点必须先切到该 pane；Runtime 不能在后台静默保留
  第二个 controller。
- resize 只发给当前 control stream。observe 收到的 frame 自带 width/height，但不能
  反向抢占 resize 权。

stream start/handshake 必须由 generation-tagged worker 完成，GTK/Core poll 线程只登记
`Starting`，不能同步等 socket。用户在 Control `Starting/Backoff` 期间产生的 input 按原始
调用边界排队，最多 256 次 write/64 KiB；resize 只保留最后一份。队列绑定
`control_intent_epoch`，而不是某一次 socket generation：同一 intent 的普通自动 retry
可以继续持有队列，Control handshake 成功后先发送最新 resize，再按序且恰好一次 flush
input。worker 完成事件仍必须匹配当前 generation 和 intent；demote、detach、takeover
suppression、intent 被替换或 stale completion 都必须以明确 input-not-delivered 结果清空旧
队列，不能把旧输入写进新 pane。达到上限或最终进入 Degraded 时同样返回/上报明确的
`RuntimeConnect`/input-not-delivered 错误，禁止静默丢键或无界占内存。

### 4.4 错误分类与退避

普通 EOF、socket read 和 frame decode 错误按下列间隔重试：

```text
100ms -> 200ms -> 400ms -> 800ms -> 1600ms
```

初始 start/用户显式 promote 不计 retry。普通故障后最多自动 retry 五次，分别在
`100/200/400/800/1600ms` 到期时启动；第五个 retry 再失败后进入 `Degraded`，且同一时刻
只有一个 in-flight start。只在新的 focus、input，或确实改变 pane terminal
target/close→reopen 状态的 topology revision 到来时重新武装；重复 snapshot、
layout/resize 或同 revision 不能重置 retry budget。测试里的
`automatic_retry_starts <= 5` 只统计这五个自动 retry，不把原始 start 或新的用户动作混进
计数。正常 transport 故障的 retry budget 还可以在 current generation 已收到 full
baseline 并连续保持 Live 10 秒后重置；短暂连上又立刻 EOF 的 flap 不能重置，否则会退化
成永久 100ms 重试。该稳定窗口必须用 fake clock 测试。

`terminal attach taken over` 单独处理：

1. 如果事件 generation 已过期，直接忽略。
2. 如果当前 control generation 被外部 client takeover，推进 generation、标记
   `SuppressedAfterTakeover` 并降成 Observe。
3. reconciliation 即使仍算出产品 desired Control，也必须尊重 suppression，不自动反抢。
4. 等待新的本地 focus edge 或真实输入创建新 intent；该 intent 只尝试一次显式 promote，
   后续普通故障才进入上述有界 retry。

主动 detach、mode replace 和 Runtime drop 产生的 EOF/Error 不算故障，不增加 retry。

---

## 5. Index 与 Surface 字节路径

### 5.1 数据源分工

| 数据源 | Workspace Index | Surface/VTE | 用途 |
|---|---|---|---|
| `pane.read` ANSI | 是，作为 `PaneIndexSnapshot` | 否 | attach 时建立搜索/attention 的无头快照 |
| `terminal.frame full=true` | 替换当前 Index frame | 是，作为当前 generation baseline 原始 feed | 当前完整屏幕 |
| `terminal.frame full=false` | 增量 feed | 是，按 wire seq 原始 feed | 稳态直播 |
| `visible_ansi()` / Replica dump | Index 内部可以生成 | 永不 | 搜索、peek 等无头像素外用途 |

为表达第一行，Core 增加 `StateChange::PaneIndexSnapshot { pane, data }`。Workspace 消费
它，platform dispatch 明确忽略它。已有 `PaneFrame`/`PaneOutput` 继续是 Surface 可消费的
原始字节事件；Runtime 在转换成 StateChange 前已经完成 generation、event ordinal 与 wire
seq 过滤。

### 5.2 初次 attach

每个 pane 的顺序固定为：

```text
snapshot topology
  -> 建 registry slot（desired mode 由 foreground 决定）
  -> pane.read 只播种 Index
  -> start Observe/Control stream
  -> 等待该 generation 的首个 full frame
  -> 记录 Surface baseline
  -> `(WorkspaceId, PaneId)` Surface 已注册且字符格尺寸有效后 raw feed 一次
  -> replay 该 generation full frame 之后缓存的增量
  -> Live
```

“Surface 已注册”不要求它当前可见：初次 topology sync 必须为所有 tab 的 leaves 建立
PaneView/字符格，再把 active tab 放到 GtkStack 可见页。后台 workspace 的 LayoutHost 也留在
`pixel_cache`，不能等用户切过去才从 Index 播种。若 topology 阶段结束后仍找不到目标
Surface，这是 lifecycle failure，事件不得静默丢弃或改从 `pane.read` 补画。

full frame 之前到达的增量按 wire seq 有界缓存，不能先画进空 VTE。上限固定为 256 个
事件或 2 MiB，任一先到即使该 generation 失败并进入同一有界 backoff；禁止丢前半段后把
剩余 diff 当 baseline。收到 full 后丢弃 queue 中 `wire_seq <= full.wire_seq` 的旧 diff，只
按连续 wire seq 追赶更大的增量；追赶队列自身有 gap 仍使 generation 失败，不能越过。
generation 切换时保留已经显示的像素，直到新 full 到达；不得为了“等待重连”主动清屏。

如果 stream start 后 5 秒内 full frame 未到达，Surface 保留旧像素或保持未播种，并把
stream 标为 Degraded；禁止退回 `pane.read visible_ansi` 重放来伪造成功。

### 5.3 GTK 批处理顺序

每个带 `WorkspaceId` 的 poll batch 固定四阶段：

1. Tab/Pane 增删、Layout、Active、Resize 等结构事件更新该 Workspace 的 Core-backed UI
   状态，并为所有 tab leaves 确保常驻 PaneView/字符格。
2. active workspace 最多执行一次可见 mount/chrome refresh；background workspace 只更新它
   自己的 `pixel_cache[WorkspaceId]`，不得切换窗口当前页。
3. 按 `(WorkspaceId, PaneId)` 处理 `PaneFrame` 和首次 Surface feed，包括隐藏 tab/background
   workspace。
4. 按同一 key 处理 `PaneOutput` 增量并 flush parser replies。

`PaneFrame` 必须和 `PaneSnapshot`/`PaneOutput` 一样延迟到结构阶段之后。任何新事件类型
若携带 Surface 字节，也必须归入第三或第四阶段，不能默认落入结构阶段。
`WorkspacePool::poll_background()` 返回的 Surface 事件不能再只喂 attention 后丢弃；必须走
同一个 workspace-aware dispatcher。只有 PaneView 的绘制可因隐藏而省略，原始 VT feed
不能省略。

Core/FFI 边界也必须保留这个 key。现有 `muxterm_poll_events` 会把后台批次拍平成裸
`StateChange`，而 `CStateChange` 没有 `WorkspaceId`；两个 Workspace 都有 `PaneId(1)` 时无法
正确路由。保持旧 `CStateChange` 布局和 `window_id=0`，新增 additive wrapper：

```text
CWorkspaceStateChange {
  workspace_id: *const c_char,
  event: CStateChange,
}

muxterm_poll_workspace_events(handle, out, max_count)
```

新 API 返回 active/background 全部批次并带完整五段 `WorkspaceId` 字符串。旧
`muxterm_poll_events` 降为 active-workspace-only 兼容入口；它仍可在 Core 内 poll 后台供
Index/attention 使用，但不能把失去 WorkspaceId 的后台 Surface event 交给旧消费者。一个
handle 的一个平台实例只能选择一种 poll API，禁止同时调用并竞争同一事件队列。新
Linux/macOS 连接池路径必须使用 workspace-aware 语义；`PaneIndexSnapshot` 在两种 FFI poll
路径里都只由 Core 消费，不序列化给像素层。handle 的 deferred queue 从入队起就保存
`(WorkspaceId, StateChange)`，禁止出队时根据“当前 active”反推；wrapper 的
`workspace_id`/event data 指针与现有事件指针遵守同一生命周期（下一次 poll 或 free 前有效）。
legacy callback 没有 WorkspaceId，因此只允许收到 active workspace event；新 poll 不得把
background event 再经旧 callback 旁路出去。

### 5.4 Follow-tail 与 Ctrl-L

- 首次 baseline/catch-up 完成后，把未主动滚动的 VTE 定位到底部。
- 用户主动离开底部后继续累计 unseen count，不强拉回底部。
- 用户在底部时收到 live output，继续跟随底部。
- Ctrl-L 必须走真实输入通道；清屏后的 frame/output 成为唯一后续像素来源。
- 切 tab、resize、observer 重连和 Index 更新都不得重新 feed 清屏前的数据。

---

## 6. Tab/Pane 创建收敛

### 6.1 Pending mutation

`tab.create` 和 `pane.split` 统一登记一个 pending mutation：

```text
PendingMutation
  mutation_id: u64
  kind: NewTab | SplitPane
  lifecycle_generation: u64
  target_tab: Option<HerdrTabId>
  target_pane: Option<HerdrPaneId>
  tabs_before: Option<Set<HerdrTabId>>
  panes_before: Option<Set<HerdrPaneId>>
  expected_tab: Option<HerdrTabId>
  expected_pane: Option<HerdrPaneId>
  expected_focus: Option<HerdrPaneId>
  enqueued_at: Instant
  dispatched_at: Option<Instant>
  next_probe_at: Option<Instant>
  probe_index: usize
  deadline: enqueued_at + 5s
```

每个 HerdrRuntime 同时最多派发一个拓扑 mutation；后续请求进入最多 32 项的 FIFO。排队时
只记录用户 intent、`enqueued_at` 与 end-to-end deadline；每项真正派发时才填写
`dispatched_at`、`tabs_before/panes_before`、第一笔 probe 时点，不能在 enqueue 时提前取
baseline，也不能让两个请求共享同一 baseline。
队列满或请求在派发前已耗尽 5 秒 end-to-end deadline 时明确失败，不能丢操作或继续猜 id。
异步 mutation 不能借用 `TaskOutcome::Done` 冒充完成：入队成功返回
`TaskOutcome::Accepted { operation_id }`，队列满/无效请求返回 `Rejected`；只有同步完成的
Runtime 操作继续返回 `Done`。最终结果通过产品语义的
`StateChange::MutationSettled { operation_id, kind, result }` 恰好发送一次，其中 result 是
`Completed` 或带 `Queue/Dispatch/AuthorityConvergence/StreamBootstrap` 阶段与原因的
`Failed`。GTK/FFI 可以把 Accepted 当“请求已接收”，但不能据此主动刷新或显示完成。

直接 API 响应只填 expected ids，不直接向产品状态推最终 Layout。若 snapshot 先到，Runtime
只在相对 `tabs_before/panes_before` 恰好出现一个符合 mutation kind/target 的新对象时填入
expected id；多个候选保持 Pending，等待 response，禁止任选一个。response 与已推导 id
不一致时立即报 protocol convergence error。Event subscription 的 snapshot 优先收敛；若
事件迟到，后台按相对 `dispatched_at` 的
`100/250/500/1000/2000/4000ms` 绝对时点请求 snapshot refresh；任何时刻至多一个
in-flight probe，且 probe 不得越过 end-to-end deadline。5 秒仍不满足完成条件则产生带
阶段的失败事件，再以新 baseline 派发 FIFO 下一项。所有 response/probe 还必须匹配
`lifecycle_generation`，detach 后晚到结果直接丢弃。

### 6.2 新 pane 幂等初始化

所有发现新 pane 的入口都调用同一 `ensure_pane_initialized`：

1. 建立 Herdr id ↔ Product PaneId 映射（存在则复用）。
2. 建/更新 PaneInfo 与 Layout 关系。
3. 建 registry slot（存在则 reconcile，不能重复 push）。
4. 为 Index 请求一次 seed；重复结果按 generation/初始化状态去重。
5. 更新 event subscription 的 pane scope。
6. 仅在产品状态确实首次出现时发 PaneAdded。

这样无论“命令响应先到”还是“event snapshot 先到”，结果都相同。

### 6.3 NewTab 名称

- `Task::NewTab { name: None }` 的 `tab.create` JSON 完全省略 `label`。
- `Some(name)` 才传 label，空白显式名称先按现有输入校验拒绝或转为 None。
- UI 使用 Herdr snapshot 的非空 label。
- Herdr 返回缺失/空 label 时，用 protocol-19 bijective base-32 规则解码 public tab id 后缀，
  再以十进制数字显示（例如 `tA -> "10"`）；禁止显示空字符串、原始字母或固定 `new`。
- fallback 数字必须来自权威 public tab id，不是 GUI 当前下标；删除 tab 后允许 id 有缺口，
  禁止为追求 1..N 连续而重命名既有 tab。public id 无法合法解码时是 protocol convergence
  error，不能静默回退成 `0`；失败通知保留 raw id。
- 上述“权威数字 label”是 Herdr raw tab name 契约；tab bar 仍可按既有通用规则加视觉
  顺序前缀。tmux 的 raw window name 不要求改成数字，不能把 Herdr fallback 误套到其它
  Runtime。

### 6.4 Mutation 完成条件

`tab.create` 完成必须同时满足：

- response/snapshot 收敛到同一个 created tab 与 root pane；
- snapshot 已包含 created tab/root pane，root pane 属于该 tab，layout tree 的唯一初始 leaf
  是该 pane；
- Herdr active tab/focused pane、Product active tab/active pane 和 Layout.active 一致；
- root pane registry 的 current generation 已进入 Live，且 `surface_baseline=Ready`；不是只有
  tab 壳、空名字或 Starting worker。

`pane.split` 返回后可以在同一 mutation worker 内请求 `pane.focus`，但不能只读一次 layout
就结束。完成必须同时满足：

- snapshot 已包含 created pane；
- created pane 属于目标 tab；
- layout tree leaves 包含 created pane，split direction/tree 与请求一致；
- `focused_pane_id == created pane`；
- Product `active_pane`、Layout.active 与 Herdr focus 相同；
- created pane registry 的 current generation 已进入 Live，且 `surface_baseline=Ready`；不是
  只有拓扑或 Starting worker、没有可交付的原始 full frame。

任何完成事件都必须在同一次 `lifecycle_generation` 内且只发一次；API response 到达、pane
进入 snapshot、current-generation full baseline 到达是不同里程碑，不能把其中任意一个
单独当作整体完成。新建 pane 的 full-frame deadline 取
`min(stream_started_at + 5s, mutation.deadline)`，从而与 enqueue 起算的 5 秒端到端门槛共用
同一 deadline；不能先等 mutation 5 秒、再额外等 stream 5 秒。Runtime 的 Completed 表示
服务端/Core/可交付 Surface frame 已收敛；GTK 是否把该 frame 喂进唯一 VTE 仍由 L2 e2e
独立证明。

---

## 7. Project 与 Existing Connection 身份

### 7.1 Canonical identity

一个可稳定重连的 Herdr Project 至少需要：

```text
transport       local | ssh
target          local 空 target 或 SSH alias
runtime         herdr
session         named session；default 也显式归一化
socket          local API socket 或 SSH 远端 socket 路径
workspace_id    Herdr workspace id
path            用户项目/worktree 路径；不能再兼任 workspace id
name            用户可见 Project 名
```

`TargetConfig` 与 `WorkspaceSpec` 都增加独立的
`workspace_id: Option<String>`。现有 `session`、`socket` 与新字段全部进入
`quickconnect.toml`；`path` 回到用户项目路径语义，不能既是目录又是 `wN`。

resolver 不能只返回一份随后会丢失 Project 元数据的裸 spec。Core 增加并保存：

```text
ResolvedTarget
  canonical: TargetConfig   # 规范化后可持久化/生成 Recent 的身份与显示字段
  spec: WorkspaceSpec       # Driver 打开的产品规格
```

`ResolvedTarget` 还必须能产生稳定的 `identity_key()`；该 key 只包含
`(transport, target, runtime, session, target-side socket, workspace_id)`，不包含用户可改的
Project `name` 或 `path`。QuickConnect 去重、badge 合并、当前连接高亮和 Pool 复用先比较这个
key，不能继续使用 `name@transport`。尚未解析的新 Project 只能使用带 runtime/transport/
target/path 的 provisional key 展示；一旦 resolver 成功，必须用 canonical identity 覆盖它。

key 构造前的规范化也锁死：Local target 是空串（UI 可显示 `local`），SSH target 是原 Host
alias；Herdr None/空 session 统一为 `default`；socket 使用 discovery/config 给出的 target-side
绝对路径，不在本机对 SSH 路径 `canonicalize()`；workspace_id 去除外围空白后必须非空。
缺任一 Herdr attach identity 的对象只能用 provisional key，不能伪装成 resolved identity。

由 Catalog 打开的 Workspace 持有这份 Core-owned descriptor；测试 mock、旧
CLI 直开路径可以是 `None`。Recents 和当前连接高亮从 descriptor 读取，不再由
`WorkspaceId` 反向构造 `TargetConfig`，也不在 Linux `UiState` 建第二张 side table。

文件与所有权锁死如下，避免实现时把 resolver 放进 platform：

```text
src/core/catalog/resolver.rs
  ResolvedTarget / ResolveIntent / TargetIdentityKey / ResolutionStage
  SessionCandidate -> TargetConfig
  Catalog::resolve_target / open_target / open_resolved
src/core/catalog/driver.rs
  SessionCandidate 的 typed session/socket/workspace_id/project_path 字段
src/core/quickconnect/model.rs
  TargetConfig（持久配置/展示模型；不做 socket 推导或 discovery）
src/core/workspace/workspace.rs
  resolved_target: Option<ResolvedTarget>
```

`SessionCandidate` 的目标形状锁死为 typed fields（字段名可做不改变语义的 Rust 风格微调）：

```text
SessionCandidate
  runtime_id: String
  transport_id: String
  target: String
  session: Option<String>       # Herdr default 必须规范成 "default"；tmux 为 session name
  socket: Option<String>        # target-side server socket；从不放 SSH local forward
  workspace_id: Option<String>  # Herdr wN；tmux 为 None
  project_path: Option<String>  # 权威可得时填写，否则 None
  name: String                  # 非空 candidate/display label
```

迁移完所有消费者后删除无类型 `extra` 和语义重复的结果侧 `namespace`；
`RuntimeDriver::list(..., namespace)` 的过滤参数可以保留。resolver 必须在构造 identity key 前
把 Herdr 的 None/空 session 归一化成 `default`；不得依靠 platform 做这一步。

`Workspace` 是 descriptor 的唯一 Pool 内所有者，`PooledWorkspace` 不复制第二份。descriptor
在一次 open 中作为完整 value 注入，platform 无 setter。复用已打开 slot 时必须先比较
`identity_key`：不同 key 立即报 collision；相同 key 允许 Core 仅以**整值替换**方式补全
原本缺失的 canonical name/path，但 session/socket/workspace_id/WorkspaceId 任一 attach
identity 字段不得改变。这样“immutable”指外部不可逐字段改写且 attach identity 不可变，
同时不会让先由 Existing 打开的空 path 永久污染 Recent。

Catalog 打开 Workspace 时，用户可见名称取规范化后的 `canonical.name`；resolver 必须保证它
非空（优先已保存 Project name，其次权威 candidate label，最后 workspace_id）。不得继续
让 `WorkspaceSpec::name()` 把 Herdr named session 当成 Project/Workspace 显示名。

两种结构的字段语义锁死为：

| 字段 | `TargetConfig` | `WorkspaceSpec` |
|---|---|---|
| `path` | 用户保存的项目/worktree 目录 | 传给 Runtime 的 cwd/项目目录元数据 |
| `workspace_id` | 发现或迁移后持久化的 runtime target id | Driver 最终要 attach 的 target id；Herdr 为 `wN` |
| `session` | named/default Herdr session | 规范化后的 named/default Herdr session |
| `socket` | server 在 target 命名空间中的 socket；SSH 为远端绝对路径 | 同一 target-side socket 身份；SSH Driver 用它建立转发 |

SSH forward 生成的本地 socket 是 `HerdrDriver::open` / Runtime 内部的临时端点，不进入
`TargetConfig`、`WorkspaceSpec`、`WorkspaceId`、Project 或 Recent；Runtime shutdown 时由
forward guard 清理。这样保存的 Project 重启后不会尝试连接一个已经消失的本地转发路径。

`WorkspaceId` 现有字符串 ABI 仍是五段
`transport/alias/session/runtime/identity`，本轮不增加第六段。为兼容旧字符串和 FFI：

- `WorkspaceSpec::id()` 的第五段在 `workspace_id.is_some()` 时取 `workspace_id`，否则取
  `path`；
- 当前 Rust 结构里第五段字段即使仍暂名 `WorkspaceId.path`，它也只是 legacy identity
  slot，不再保证是文件系统路径；
- 禁止从该第五段、`WorkspaceId::replica_id()` 或 Recent 反向写回
  `TargetConfig.path`；Project 路径只能来自 resolved `TargetConfig`/`WorkspaceSpec` 元数据；
- 旧 tmux/shell 的第五段继续使用 path，显示字符串和 FFI 不得改变；同一 Herdr candidate
  从 Project 与 Existing 打开时必须得到相同第五段和完整 `WorkspaceId`。
- 五段 ABI 没有 socket 段；若 Catalog 发现两个不同 `identity_key()` 会生成同一个
  `WorkspaceId`，必须在 `IdentityResolution` 阶段明确报 collision，禁止错误复用池中另一条
  Herdr server。正常 named/default session 在同一 transport target 上应先规范化成唯一
  session/socket 对。

发现层交给 Catalog 的 candidate 必须显式携带 `session`、target-side `socket`、
`workspace_id` 和可获得时的 `project_path`；不得继续把 workspace id 塞进无类型的 `extra`
字符串，也不得由 Linux 根据 `$HOME` 猜 socket。`SessionCandidate -> TargetConfig` 的转换属于
Core/Catalog。若 protocol-19 candidate 没有权威 project path，path 可以暂为空；若同 identity
已有保存的 Project，则合并并保留该 Project 的 name/path。path 是否暂缺不改变
`identity_key()` 或 attach 目标。

### 7.2 统一解析

解析操作必须显式携带意图，不能从“identity 缺字段”猜测是否允许创建：

```text
ResolveIntent
  AttachOnly       # Existing、Recent、已保存 Project 的普通重连
  CreateIfMissing  # 用户本次明确执行“新建 Project”

Catalog::resolve_target(TargetConfig, ResolveIntent) -> ResolvedTarget
```

Catalog 的 TargetConfig → ResolvedTarget 统一 resolver 优先级锁死为：

1. session/socket/workspace_id 完整时按精确身份 attach。
2. 旧配置缺 identity 时，在所选 connect 上列出 Herdr candidates。
3. `path` 若恰好等于 candidate workspace id，先作为旧格式精确迁移。
4. 否则按唯一 workspace label/project name 匹配。
5. 多个匹配返回 ambiguity，列出 session/workspace id；禁止随意选 default。
6. 本地无匹配、`ResolveIntent::CreateIfMissing` 且所选 named/default Herdr server 已运行
   时，才允许在这个已选 session/socket 上按 path/name 创建 workspace；禁止偷偷切到
   default server。CreateIfMissing 本身不等于“任选 server”：若 config 没有明确 session/socket
   且发现不出唯一、已选目标，返回 choice-required/ambiguity，不得自动选 default。
   `AttachOnly`、旧配置迁移和普通 Project 重连都不得创建。
7. SSH 无匹配时失败；禁止为用户启动或安装远端 Herdr。

成功解析后，把规范化的 session/socket/workspace_id 写回该 Project。Existing Connection
本来就携带这些字段，因此两条入口最终必须得到相同 `identity_key()`、attach spec 身份字段
和 `WorkspaceId`。若 Existing 原本没有 project path，则 resolver 合并同 identity 的已保存
Project 元数据后，两条入口的完整 `ResolvedTarget` 也必须相同；没有可合并 Project 时允许
canonical path 为空，但不得用 workspace id 冒充 path。成功 open 时 Pool 保存 descriptor；
后续 Recent 必须与原 canonical TargetConfig 等价，不能因为只剩五段 id 而丢
path/socket/workspace_id。

### 7.3 失败阶段

连接错误必须保留阶段：

```text
Discovery
IdentityResolution
WorkspaceCreate
SocketForward
RuntimeConnect
```

禁止 catch 后把 path 当 workspace id 继续连接。日志记录原始 anyhow chain；用户通知显示
阶段、connect name、session/workspace id（已知时）和简短原因。

---

## 8. 对外与内部接口变更

### 8.1 公共 Core 接口

- `Runtime::set_foreground(&mut self, foreground: bool)`：默认 no-op；Pool 驱动。
- `StateChange::PaneIndexSnapshot { pane, data }`：只给 Index。
- `TaskOutcome::Accepted { operation_id }`：异步 mutation 已进入有界队列，不是完成。
- `StateChange::MutationSettled { operation_id, kind, result }`：异步 mutation 的唯一最终
  Completed/Failed 事件；失败携带产品阶段和原因。
- `TargetConfig.workspace_id: Option<String>`：Project identity，不再复用 path。
- `WorkspaceSpec.workspace_id: Option<String>`：Driver attach target，与项目 path 分离。
- `ResolvedTarget { canonical: TargetConfig, spec: WorkspaceSpec }`：Core-owned 的规范连接描述。
- `ResolveIntent::{AttachOnly, CreateIfMissing}`：把 attach 与显式创建权限分开。
- `Catalog::resolve_target(&TargetConfig, ResolveIntent) -> Result<ResolvedTarget>`：
  Project/Recent/Existing 共享解析器，但调用意图明确。
- `Catalog::open_target(&TargetConfig, ResolveIntent)` / 内部
  `open_resolved(&ResolvedTarget)`：打开并让 Pool 构造持有 descriptor 的 Workspace；低层裸
  `open(&WorkspaceSpec)` 不能用于 Project/Recent/Existing。
- `WorkspacePool::open_resolved(ResolvedTarget, runtime)`（签名可按现有 async/closure 风格
  微调）：唯一 descriptor-aware 收编入口；构造 Workspace 时注入 descriptor，并在复用前做
  identity/WorkspaceId collision 检查。
- `Workspace::resolved_target()`（或等价 Pool 查询）：Core Recent 数据源，platform 只渲染。
- `CWorkspaceStateChange` + `muxterm_poll_workspace_events(...)`：additive FFI；保留
  WorkspaceId 后再把 raw Surface event 交给 platform。旧 `muxterm_poll_events` 只返回 active
  workspace，不能继续混入无身份的 background event。

FFI 采用 additive、ABI-safe 映射：保留旧 `muxterm_execute` 返回码；新增
`muxterm_execute_json` 暴露 Accepted operation id；新增 `STATE_MUTATION_SETTLED=16`，把
settlement JSON 放进既有 `CStateChange.data/data_len`，不改变 C struct 布局。保留旧
`muxterm_workspace_open(...)` 为 descriptor=None 的低层入口；新增
`muxterm_workspace_open_target_json(...)` 传完整 TargetConfig + ResolveIntent。workspace list
JSON 追加 optional `resolved_target`，旧消费者可以忽略。新增 workspace event wrapper 也不
改变 `CStateChange` 的 size/offset；其 `event.window_id` 继续为 0。

上述 resolver/identity 类型固定放在 `src/core/catalog/resolver.rs` 并从
`core::catalog` re-export；禁止在 `platform/linux`、`platform/macos` 或两端各建一份。

这些接口使用产品语言，不暴露 `w1:p1`、ControlTerminal 或 Herdr event 名。

### 8.2 Herdr adapter 内部接口

- `StreamMode`、`PaneStreamSlot`、generation/event ordinal/wire seq event、control intent 与
  takeover suppression。
- `reconcile_stream_modes()`：唯一 mode transition 入口。
- `ensure_pane_initialized()`：唯一新 pane 初始化入口。
- bounded mutation FIFO + `PendingMutation`：序列化创建操作并做权威收敛。

platform 不得直接调用这些内部对象。

---

## 9. 可观测性与故障边界

每次 lifecycle 诊断至少包含：

```text
workspace_id, pane_id, herdr_pane_id,
generation, desired_mode, actual_mode,
control_intent_epoch, control_rearm,
transition, reason, retry_count, event_ordinal, wire_seq,
mutation_id, mutation_queue_depth
```

以下是验收失败，不是可忽略 warning：

- 同 pane 同 generation 出现两个 live stream；
- stale event 改变 registry；
- taken-over 后无用户动作仍反复 control；
- 短暂 Live flap 重置 retry budget，形成永久低间隔重试；
- 两个 mutation 共享 baseline 或从多个 snapshot delta 中任选 id；
- 创建成功但 5 秒后没有 Surface stream；
- batch 在 Layout/resize 前 feed PaneFrame；
- Project resolver 静默回退到 default socket/path-as-id；
- Runtime detach 后 registry 重新启动。

测试应断言 transition/retry 计数，而不是依赖机器瞬时 CPU 百分比；Linux e2e 另外用主循环
响应 watchdog 证明没有 busy-loop。详细场景和阈值见
[`HERDR-TESTING.md`](HERDR-TESTING.md)。

---

## 10. 实施边界

实施按 [`../.plan-herdr-runtime-stabilization-20260822.md`](../.plan-herdr-runtime-stabilization-20260822.md)
的 RED→GREEN commit 顺序执行。实现 agent 不得：

- 合并或跳过 RED 场景来维持中间提交绿色；
- 通过增加 sleep、放宽 token/行号断言或新增 `#[ignore]` 修 CI；
- 使用默认 tmux server 或默认 Herdr session 做破坏性测试；
- 修改 `~/Developer/terminal/{herdr,herdrm}`；
- 在没有真实 `+`/key controller/GLib 主循环证据时宣称生产路径修复；
- 在未完成 local/loopback SSH 四格前宣称 runtime×transport 已覆盖。
