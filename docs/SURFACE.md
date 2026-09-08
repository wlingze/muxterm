# SURFACE.md — 单面架构与常驻场景

> 机制名：**Surface**（中文：**单面**）
> 契约冻结：2026-09-08（`2026-09-08T15:02:22+08:00`，Asia/Shanghai）
> 像素定律（§3）自 2026-08-15 起有效；2026-08-24 纠偏见 §7。
> 2026-09-08：常驻 Scene + 单事件泵；删除 warm/cold slot、`bridgeLock`、串行后台队列、前台校准（§8–§9）。
> 产品层级：[`WORKSPACE.md`](WORKSPACE.md)。Runtime：[`RUNTIME.md`](RUNTIME.md)。
> Herdr full/diff：[`HERDR-RUNTIME-STABILITY.md`](HERDR-RUNTIME-STABILITY.md) §4–§5。
> 参考树：`/home/wlz/Developer/terminal/`（只读，不进本仓库）

**一句话：** 每个已打开 pane 只有一个前端 VT 负责画面；Runtime 交出原始字节，Workspace 不画像素。
每个已打开 Workspace 一棵常驻 Scene。切换 = 换可见场景，点击路径零 Core 调用、零锁等待。
**禁止**把 Index 网格再序列化成 ANSI 灌进 VTE/SwiftTerm。

---

## 0. 为什么 dump 路径是错的（历史，2026-08-15）

Linux 当时的显示路径（Phase C–E 叠出来的）：

```text
程序 → tmux server VT
     → %output ANSI
     → ReplicaStore / TerminalState     ← 仿真器 1
     → visible_ansi() 再序列化
     → VTE reset + feed                 ← 仿真器 2
     → 像素
```

这是 **三重仿真**（tmux 自己已经是一份）。真机后果（`test_2026-0815-2105.log`，SSH
`yaklang-workspace` + `muxterm`）：Codex 闪烁、切 pane 白屏、输入像「整句越来越长」、
滚轮走 dump replica 历史。

`1365` / `2730` 不是两份完整帧，是同一帧被 tmux 切碎的前后半，必须按序 `feed`。
用户贴的「同一段话越来越长」是验收用例：可见文本里必须 **只有一行** 该句。

这条路径已经作废。下文定律仍然以它为反面教材。

### 0.1 输入通道（与画面病分开）

官方 `tmux.1`：`-l` = 字面 UTF-8；`-H` = 每个参数一个十六进制 ASCII 字节。
ivyTerm 按键走 `-H`，剪贴板走 `-l`。Surface 输入必须是 **字节通道 `-H`**（或等价）。
不要指望单改 `-H` 治好「句子越来越长」。

---

## 1. 三角色（2026-09-08 用三条 lane 重述）

| 角色 | 对应 lane / 组件 | 职责 | 禁止 |
|---|---|---|---|
| **Control** | Control lane + Workspace 拓扑 | 树、layout、焦点、gap barrier、Task | 不画像素；不解析 `%output` |
| **Surface** | Render-data lane + 常驻 PaneSurface | 每个进入产品拓扑的 pane **恰好一个常驻** VT；隐藏时不绘制但仍 `feed` | 不 `reset` 追帧；不吃 `visible_ansi` |
| **Index / Activity** | Index + Activity lane | 同一字节流的只读副本：搜索、OSC 133、BEL、commands/agents | **永不**把网格再编码回 Surface |

本地 shell（无 tmux）也是 Surface：VT 连 PTY。tmux 时 PTY 换成解转义后的 `%output` 管道，
**仿真器个数不变**。

事实源是 **pane 字节流 + 拓扑**，不是「core 网格 dump」。

### 1.1 别人怎么做；切过去为什么快

| 项目 | 连接 / 协议 | 显示 | 「中间层」 |
|---|---|---|---|
| iTerm2 | `TmuxGateway` + `TmuxController` | 每个 pane 一个 `PTYSession` | **没有**第二份网格 dump。历史只在第一次 capture 填 scrollback |
| ivyTerm | `TmuxAPI` | 每个 pane 一个 VTE；tab 里 widget **留着** | 无 replica dump |
| Ghostty | tmux `Viewer` | core `Terminal`；GUI 叫 **`Surface`** | Viewer 不是第二仿真器 |
| WezTerm | `TmuxDomain` | `TmuxPty` 假 PTY → 仍是那一个仿真器 | Domain 是连接，不是 dump |
| cmux | remote tmux control | `TerminalSurface.processRemoteOutput` | seed 结构，不是每帧重拍 |

**同一条 tmux 里换 window/pane：** widget 不拆，切过去是 show/hide，**不是**重连、不是再 capture、不是 dump。
**多条 tmux / 多机：** 多个连接（Muxterm 里是多个 Workspace + 可复用 TargetConnection）。
**没打开的 window：** 可以不建 VT；**已打开的**后台仍吃字节。

Muxterm 切 tab 曾经慢且白，是因为做了别人不做的两件事：

1. `refresh_ui` 再 `present_from_replica`（重播网格 + reset）
2. `LayoutHost::apply_layout` `panes.retain(当前布局)`——换 window 就把上一窗的 VTE **扔掉**

Surface 方案要快，必须：**别的 tab 的 PaneSurface 留着**，只从 widget 树摘下、再挂回去。
2026-09-08 把同一条推广到 **Workspace 级常驻 Scene**。

旧文档把「多路 + 快切」命名为 `ConnectionPool` / `WarmConnectionSlot`。那套机制 **已删除**（§8–§9）。
连接复用在 Core `ConnectionRegistry`；像素常驻在 frontend Scene。

---

## 2. 参考实现（已克隆到 `/home/wlz/Developer/terminal`）

只读。改 Muxterm 时对照，不要抄 UI。

| 目录 | 项目 | 对我们的结论 |
|---|---|---|
| `iterm2/` | iTerm2 | **模仿对象。** 显示面 = 唯一 VT |
| `ivyterm/` | ivyTerm | Linux 应抄：解转义 → `vte.feed`；capture 一次；`send-keys -H` |
| `ghostty/` | Ghostty Viewer | 与 ivyTerm 同一状态机 |
| `wezterm/` | WezTerm | 「字节当 PTY」= Surface |
| `cmux/` | cmux | seed = snapshot + discard + catch-up |
| `tmux/` | tmux | 客户端必须会 pause，否则 20 万行打爆 |

### 2.1 共同状态机

```text
attach
  → 建 Surface（空 VT）
  → 权威 baseline（可见屏 snapshot / full frame）  [pending]
  → 此期间到达的增量进 discarded 队列，不画
  → baseline 到齐：feed 快照，标记 synced
  → catch-up 按序 feed
  → 稳态：每个 PaneOutput 只 feed(raw)，永不 reset
切 pane / 切 tab / 切 workspace
  → 已 synced 的 Surface 只显示/隐藏，不 reset
  → 新 pane 走上面的 seed，不做 visible_ansi
```

### 2.2 输入

ivyTerm / psmux：`send-keys -t %N -H`。Muxterm Surface 输入必须是字节通道 `-H`（或等价）。

---

## 3. 硬性定律（违反 = 回归）

1. **One Surface.** 一个进入产品拓扑的 pane，显示路径上只有一个常驻 VT parse；当前不可见不等于未注册。
2. **No dump.** `visible_ansi` / `present_from_replica` **不得**出现在 live `%output` 或 CUP 风暴路径。Index 自用。
3. **No reset to chase frames.** `vte.reset` 只允许新建 Surface 或确认的 resize 错格；用户 Ctrl-L 必须走终端输入。
4. **Seed once.** baseline 完成前不画 live；完成后一次性 feed；再增量。
5. **Follow tail.** 直播 `history_offset=0`；新输出后视口在底。禁止用 replica dump 模拟滚动。
6. **Bytes in, bytes out.** 键盘 → `send-keys -H`；`%output` 解转义 → 原样 feed。
7. **`%pause` 是数据丢失屏障，不是切 tab 刷新。** 正常切换禁止 pause + capture；OutputGap / 平台缓存溢出 / tmux 主动 `%pause` 必须保持该 pane fenced，安装一次权威 snapshot/full frame 后才能恢复 live。洪水 pane 的主动 `pause-after` **尚未实现**（TODO，见 §7.4）。
8. **Pane id.** 控制协议里 pane 是 `%N`。`缺少 @ 前缀: %64` 必须当 bug 修。
9. **Index never becomes Surface.** `visible_ansi` / `surface_seed_ansi` / `scroll_ansi` / `paneSurfaceSeedANSI` **不得**进 VTE/SwiftTerm。Herdr `pane.read` 也只播种 Index。
10. **Open Surfaces keep eating.** Surface 以 `(WorkspaceId, PaneId)` 为 key 常驻；隐藏 tab 与隐藏 workspace 的 PaneSurface 继续 `feed` 原始字节，只是不绘制。切回时只能 show/hide。
11. **No recapture on navigation.** 已经 seed 过的 pane，切 tab/pane/workspace 不得再抓屏。
12. **History is lines, not a stream.** 第一次打开时 Runtime 把 capture 解析成行写入 Surface scrollback，不是 VT `feed()` 重放。

Ctrl-L 属于终端输入，不是 UI 的 `vte.reset`。

本地 shell 不受 4、7、11、12 约束（它有真 PTY）。定律 1、2、9、10 对本地同样成立。

`visible_ansi`、`surface_seed_ansi`、`scroll_ansi` **不是** live Surface API。迁移期间若 Index/诊断仍需要，放在 Core 内部并标为临时。

---

## 4. 和旧计划的关系

| 旧物 | Surface 下 |
|---|---|
| `LINUX-PLAN` Phase C/D/E | 控制面/chrome 可留；**显示路径作废** |
| ReplicaStore | 降为 Index |
| `scroll_history` + 几何 ANSI | 删除显示用途 |
| WarmConnectionSlot / ConnectionPool / `bridgeLock` | **删除**。见 §8–§9 |
| `set_foreground` 作为 frontend 产品 API | **删除**。Herdr control/observe 是 adapter 内部 |
| 搜索 / attention | Activity lane + Overlay；小终端也是 Surface，禁止 dump |

---

## 5. 测试金字塔（必须能抓住 2105 和 0908）

### 5.1 小：协议

- `%output` 解转义与原文 bytes 恒等（含 CUP、UTF-8 盒线、真彩）
- `send-keys -H` 对任意 `[u8]` roundtrip
- `parse_line("%… %64 …")` 接受 `%` pane id
- `%pause` / `%extended-output` 能 parse
- 384KiB live redraw 在有界 lane 前合并；多 pane 交错保序，不能互相制造 `OutputGap`

### 5.2 中：Surface

- `surface_live_feed_does_not_reset`：synced 后 20 帧 CUP，`RenderTrace.resets` 不增加
- `surface_typing_overwrites_in_place`：完整句在 `visible_text` **恰好一次**
- `surface_codex_fixture_raw_feed`
- `surface_seed_drops_output_until_capture`
- `seeded_output_gap_replaces_partial_sgr_before_live_resumes`
- 切 pane/tab/workspace 的 `resets` 增量有界
- 点击路径零 FFI（e2e 插桩：activate 到首帧之间无 Core 调用、无锁等待）

### 5.3 大：隔离 tmux e2e（`-L muxterm-test-*`）

- 打 `MUXTERM_TYPE_TOKEN`，5s 内 VTE **恰好一份** token
- 再开一个 window，点 tab：VTE 非空、`resets` 不暴涨
- 隔离 tmux 里跑合成 CUP 脚本：停在末帧，无白屏
- 滚轮：shell 输出 200 行后向上能看到 `line-0`（native scrollback，不是 replica dump）

禁止：`include_str!` 大体积 `*.log`；只 `contains(TOKEN)` 不数出现次数。

---

## 6. 参考树维护

`/home/wlz/Developer/terminal/` 不提交。核查时间：`2026-08-15T21:26:10+08:00`；
§7 核查 `2026-08-24T16:55:34+08:00`。源码以克隆树为准。

| 声明 | 来源 |
|---|---|
| `%output` 进唯一 VT；`%pause` | iTerm2 `TmuxGateway.m`；tmux `control.c` / `tmux.1` CONTROL MODE |
| 未 synced 丢弃 live；`vte.feed`；capture 一次；`-H` | ivyTerm `feed_output`；`tmux_api/send.rs` |
| capture 完成前 ignore `%output` | Ghostty `src/terminal/tmux/viewer.zig` |
| seed = snapshot + discard + catch-up | cmux `RemoteTmuxPaneSeed.swift` |
| `-H` 十六进制字节 | `tmux.1` send-keys；[OpenBSD tmux.1](https://man.openbsd.org/tmux.1) |
| 控制模式是文本，客户端自己画 | [tmux wiki Control Mode](https://github.com/tmux/tmux/wiki/Control-Mode) |

GTK4 `GtkStack` 一次只显示一个子 widget，子页面仍留在树里
（[class.Stack](https://docs.gtk.org/gtk4/class.Stack.html)，核对 2026-09-08）。
常驻多 Scene 的稳态绘制成本 ≈ 单 Scene。

---

## 7. 2026-08-24：字节直达（纠偏）

文档 2026-08-15 已经禁止 dump。实现后来把 Workspace/PaneBuf 当成显示缓存：切 tab
`pause`+capture，再用 `surface_seed_ansi` 灌进 SwiftTerm。卡顿和「历史只能滑一点」都来自这条。

### 7.1 内容去处（2026-09-08：三条 lane）

Runtime 解析 wire 之后交出三类产品数据。Workspace 和前端都看不见 `%output` / `capture-pane` / `$N`。

| Runtime 解析出来的 | 产品事件 | 谁用 |
|---|---|---|
| 控制协议 | Control lane | Workspace 存拓扑；前端改分割和 tab |
| PTY 字节 | `PaneOutput` / `PaneFrame` / `PaneHistory` | **只**进该 pane 的前端 Surface；同一份复制给 Index |
| 权威信号 | `RuntimeSignal` | Workspace 生成 ActivityRecord |

Workspace **不**解析控制协议，不画像素，不把 Index 网格再编码回去。

### 7.2 谁渲染

每个 **已打开** pane 一个前端 VT。PTY 字节 `feed`，禁止 `reset` 追帧。

切 tab / 切 workspace：已有 Surface 只显示/隐藏，继续吃 `PaneOutput`。不要 core 预渲染一帧再贴到前端。

**没有前后台之分。** 所有已打开 Workspace 的 Control / Activity / Render 都常流。隐藏 Scene 不绘制，
但 VT 继续 feed。资源不够时按 pane 降档（live / coalesce / pause），这是资源旋钮，不是正确性条件：
切过去永远先看到最后已知帧，任何档位都不允许白屏等待。

池默认以 20 个 Workspace 作为软提醒阈值；超过后由用户选择关闭长期未使用项，不静默移除连接。

### 7.3 已落地的实现要点

- 切到已经 seed 过的 tab：Runtime 不再 `pause` / `capture-pane`。
- OutputGap / `%pause`：按 pane pause → visible capture → `PaneSnapshot` reset → continue。失败保持 fenced。
- 第一次 seed 用 `PaneSnapshot`；历史按行回填（`PaneHistory`），不 `feed()` `-S -N`。
- 禁止 `paneSurfaceSeedANSI` / `visible_ansi` 当显示。

### 7.4 TODO（洪水 `pause-after`）

某个 pane 的 Surface 跟不上时，只对 **该 pane** `refresh-client -A %N:pause`，追上再 continue。
代码里搜 `TODO(surface-7.4)`。现在不要用 pause 当切 tab 手段。

### 7.5 不卡顿的验收

- 已打开的 tab/pane/workspace 切换：没有 pause、没有 capture、没有 reset、没有 FFI、没有锁。
- 打字和 TUI 跟 `PaneOutput` 走，不跟 Index dump 走。
- 某个 pane 刷爆时允许掉帧；TODO 落地后只 pause 那一个 pane。
- 切换首帧 ≤ 1 帧（60Hz 下 16ms 预算）。

---

## 8. 常驻 Scene + 单事件泵（2026-09-08）

平台无关词汇（各前端用本语言实现同一套）：

```text
Core（Muxterm / WorkspacePool）
  │  poll 批次（topology → activity → frame → output）
  ▼
EventPump（唯一 FFI 事件消费者；经 ffi_client）
  │  按 WorkspaceId 分发；经主线程桥写 ViewStore
  ▼
ViewStore → Scene / Sidebar / Overlay

反向：UI 手势 → CommandQueue → Core Task → MutationSettled → ViewStore
```

- **Scene**：一个已打开 Workspace 的完整视图树（TabBar + PaneGrid + 每 pane 一个 PaneSurface）。从 open 到 close 常驻。
- **SceneStack**：切换 = 换可见子树。切换时不调 Core。
- **CommandQueue**：合并同一目标的连续 SwitchTab / activate，只发最后一个。
- **resize**：只对可见 Scene 发 `Task::Resize`；隐藏 Scene 记 pending size。

去锁论证：旧设计的锁来自「后台 poll 与前台激活并发碰同一个 C handle」。新设计里 FFI 入口只剩
EventPump 与 CommandQueue，Core 内部串行化；UI 线程从不直接碰 handle。并发访问是锁存在的前提；
入口收敛后前提消失。拓扑常流 ⇒ 缓存永远权威 ⇒ 没有 warm/cold。

Linux：Scene = `GtkStack` page。删除 `LayoutHost::apply_layout` 的 retain 丢弃。
macOS：删除 WarmConnectionSlot / `bridgeLock` / `backgroundPollQueue` / 前台权威校准。
CoreBridge 瘦身为 FFI + DTO 解码。ConnectionPool 的 warm/cold 概念删除；live owner 是 Core WorkspacePool。

一个 poll batch 必须先提交最终 topology，再发 activity，然后 frame，最后 output。
迟到输入不得改投新的 active pane。

---

## 9. 切换延迟不是 tmux 全局锁（2026-09-08 dogfood）

侧栏快速切换时看到的「远端校准在抢锁」、以及 `workspace activation ready … elapsed_ms≈3400 / 5260`，
**不是**「多个客户端挂到同一个 tmux session 上，抢了一把 tmux 全局锁」。

### 9.1 真正卡住的是 Muxterm 自己的两条串行路径

1. 每个 warm slot 一把 `bridgeLock`（`NSLock`），保护同一个 C ABI handle 不被后台 poll 和前台激活同时碰。
2. 所有后台 poll 和权威拓扑刷新共用同一条串行队列 `muxterm.macos.background-poll`。

SSH 上的 `refresh-client -A %N:pause` + `capture-pane` 会把**这一条**控制通道占住几百毫秒到二十秒。
队列和锁因此一起变长。

### 9.2 证据要点

- 三个 Workspace = 三条独立 `-CC`，挂的是 **不同 session**（legion / muxterm / yaklang-workspace）。
- 其它 session 的 `%session-window-changed` 被主动忽略，不是 3–5 秒激活延迟的原因。
- `muxterm_get_tabs` / `muxterm_get_panes` 读的是 Core 内存态。校准慢，是因为轮到它之前队列已被 pause/capture 占住。
- 首屏 seed timeout 20s（`INITIAL_SEED_TIMEOUT`）会把控制通道堵死。
- 快速连点会 `generation++` 作废旧校准，并把堆积的 `SwitchTab`（`@61/@0/@61/@0`）在校准结束后一口气重放。

pause 的作用域是 `refresh-client -A "%N:pause"`：只停**本 client** 上这个 pane 的 control output，
不会把远端 session 里的 shell 停住，也不会把其它 client 锁死。

再开 iTerm / 另一个 `tmux attach` 到**同一个** session 会抢 current-window 和 client 尺寸，
可能造成「窗跳来跳去」，但解释不了 `bridgeLock` 等待和 20s seed deadline。

### 9.3 故障链

```text
侧栏点 Workspace
  → activate() generation++
  → 立刻 paint 缓存（用户已看到新画面）
  → 权威刷新进入 backgroundPollQueue

同时：其它 warm slot 正在同一条队列上 drainBackgroundEvents
      reader-output-lane 溢出 → mark_output_gap → pause + capture-pane（SSH RTT）

backgroundPollQueue 串行
  → withBridge 等 bridgeLock
  → captureForegroundAuthority（内存快照本身很快）
  → elapsed_ms=3s~5s
  → 重放堆积的 SwitchTab
```

### 9.4 架构对应（不是补丁列表）

| 旧机制 | 新机制 |
|---|---|
| 每 slot 一把 `bridgeLock` | 无锁：FFI 只剩 EventPump / CommandQueue |
| 单条串行 `backgroundPollQueue` | EventPump 一个循环 drain Pool 已合并的多工作区批次 |
| 激活后 `captureForegroundAuthority` | 无校准：Control / Activity 对所有已打开 workspace 常流 |
| warm / cold slot | Scene 常驻；Core WorkspacePool 是唯一 live owner |
| `pendingForegroundActions` 重放 | CommandQueue 合并同目标命令 |
| gap fenced + 20s seed 堵住激活 | fenced 保留（数据正确性），但只影响该 pane 的异步补基线；首帧用最后已知画面 |

验证时继续用独立 tmux socket，禁止对用户默认 server 做 `kill-session` / `kill-server`。

原诊断全文曾记在未跟踪的 dogfood 笔记里；现行契约以本节为准。
