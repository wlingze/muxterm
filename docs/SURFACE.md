# SURFACE.md — 单面架构（Muxterm Surface）

> 机制名：**Surface**（中文：**单面**）
> 状态：调研定案，2026-08-15 21:26 CST；**2026-08-24 纠偏**见 §7（核查 `2026-08-24T16:55:34+08:00`）
> 性质：长期保留的架构文档。Linux 像素路径（F1–F6）已冻结；2026-08-22 补充 Herdr
> source-separation/Ctrl-L 契约。
> **产品层级与工作区池** 见 [`WORKSPACE.md`](WORKSPACE.md)。
> [`PANE-VT.md`](PANE-VT.md) 是讨论稿，以 WORKSPACE.md 为准。§2–3 的「镜子」是 Index，不是显示缓存。
> Herdr 的 full/diff、generation 与 Index 播种规则见
> [`HERDR-RUNTIME-STABILITY.md`](HERDR-RUNTIME-STABILITY.md) §4–§5；专项测试见
> [`HERDR-TESTING.md`](HERDR-TESTING.md)。
> 参考树：`/home/wlz/Developer/terminal/`（只读，不进本仓库）

**一句话：** Director 维持 tmux 连接与拓扑；每个 **已打开** pane 只有一个前端 VT 负责画面；同一字节流可以另有只读 Index 做搜索/通知。**禁止**把第二份网格再序列化成 ANSI 灌进 VTE/SwiftTerm。Core 保活连接，不画像素。

---

## 0. 为什么现在的路线是错的

Linux 当前显示路径（Phase C–E 叠出来的）：

```
程序 → tmux server VT
     → %output ANSI
     → ReplicaStore / TerminalState     ← 仿真器 1
     → visible_ansi() 再序列化
     → VTE reset + feed                 ← 仿真器 2
     → 像素
```

这是 **三重仿真**（tmux 自己已经是一份）。`docs/PRODUCT-VISION-STRATEGIC-REVIEW.md` §2.11.8 写过「双重仿真接缝」；我们又在客户端加了第三份。

真机后果（`test_2026-0815-2105.log`，13:05–13:18 UTC，SSH `yaklang-workspace` + `muxterm`）：

| 现象 | 对应机制 |
|---|---|
| Codex 胡乱闪烁、切 pane 白屏 | CUP 风暴里 `reset(true)+feed(visible_ansi)`，每 25ms 清屏 |
| 跳到 agent 只看到中间，tmux 里滑到末尾才好 | 播的是「某一时刻的网格 dump」，不是 VTE 跟着字节流停在尾部 |
| 输入像「整句越来越长」而不是原地多一个字 | 同一行被当新行追加（缺 `\r`/CUP，或 dump 叠在旧 VTE 上）。**不是** `-l` 打 ASCII 字母本身。2105 里 819 次 `send-keys -l` 是输入通道；画面病在显示路径 |
| 鼠标滚动怪 | VTE `scrollback_lines=0`，滚轮去 dump replica 历史（又一次 reset+ANSI） |

2105 同时：`实时 %output` **298404** 条（pane 39 = 227376，len `2730`×25499 / `1365`×11632）；`解析失败: pane id 缺少 @ 前缀: %64` 共 8 次 WARN（`%pane-mode-changed` 走了 `PaneId::parse`，只认 `@`）。数据在，显示路径在自毁。

用户贴的「同一段话越来越长」是验收用例，不是文案：VTE 里必须 **只有一行** 该句，不能 N 份前缀。

### 0.1 `1365` / `2730` 不是「两份完整帧」

Codex 一类 TUI 的一次重绘会被 tmux 切成连续 `%output` 碎片（常见 1365 然后 2730）。它们是 **同一帧的前后半**，必须按序 `vte.feed`。

`last_visible_frame` 丢掉前半；`present_from_replica` 用第二份网格补回来，于是每 25ms `reset` → 白闪。旧 E-R1 把「VTE 只显示 replica 的 `visible_ansi`」写成正确直播——**那条处方作废**，以本文为准。

生产代码（HEAD `d802f05`）：

- `window.rs` `STATE_PANE_OUTPUT`：已 seeded 则 `feed_output` 原始字节，否则 `present_from_replica(visible_ansi)`
- `pane_view.rs` `flush_pending_feed`：CUP 风暴 `reset` + `replica_ansi_provider()`
- `window.rs` `refresh_ui`：切 tab 再 dump 一次（白屏）
- `renderer.rs` `apply_mirror_policy`：`scrollback_lines=0`

### 0.2 输入通道（与画面病分开）

官方 `tmux.1`：`-l` = 字面 UTF-8；`-H` = 每个参数一个十六进制 ASCII 字节。克隆树 `tmux/cmd-send-keys.c`：`-H` → `KEYC_LITERAL|n`。ivyTerm 按键走 `-H`，剪贴板走 `-l`。Muxterm `send_keys_bytes` 全是 `-l`。F 阶段把 GTK 按键改成 `-H`；不要指望单改 `-H` 治好「句子越来越长」。

---

## 1. 机制名与三角色

| 角色 | 英文 | 职责 | 禁止 |
|---|---|---|---|
| **Director** | 控制面 | `-CC` 连接、session/window/pane 树、layout、`refresh-client`、`send-keys`、`%pause` | 不画像素 |
| **Surface** | 显示面 | 每个进入产品拓扑的 pane **恰好一个常驻** VT；隐藏时可从 widget tree 摘下，但仍按 id `feed` 原始 pane 字节 | 不 `reset` 追帧；不吃 `visible_ansi` |
| **Index** | 索引面 | 同一字节流的只读副本：搜索、attention、peek | **永不**把网格再编码回 Surface |

本地 shell（无 tmux）也是 Surface：VTE 连 PTY。tmux 时 PTY 换成 `%output` 管道，**仿真器个数不变**。

这与愿景文档「Rust core 是事实源、GUI 只渲染」**不冲突**：事实源是 **pane 字节流 + 拓扑**，不是「core 网格 dump」。Index 可以是 core 里的 `TerminalState`，但只供搜索/通知。

**名字先不改。** Surface 只命名「一个 pane 的那张显示面」。多路 tmux 常驻、切过去立刻能看，靠的是 **Director 连接池 + Surface 常驻（hide，不销毁）**，不是另搞一套 dump。见 §1.1。

```
                    ┌──────────── Director ────────────┐
  tmux -CC  ──%──►  │  parse  │  topology  │  commands │
                    └────┬───────────┬─────────────────┘
                         │ raw bytes │
              ┌──────────┴───────────┴──────────┐
              ▼                                 ▼
         Surface (VTE)                      Index (ReplicaStore)
         feed, 无 reset 追帧                search / attention
         像素、滚动、选择                    禁止 visible_ansi→VTE
```

### 1.1 别人怎么叫；有没有中间层；切过去为什么快

| 项目 | 连接 / 协议 | 显示（≈我们的 Surface） | 「中间层」 |
|---|---|---|---|
| iTerm2 | `TmuxGateway` + `TmuxController` | 每个 pane 一个 `PTYSession`（VT100） | **没有**第二份网格 dump。`TmuxHistoryParser` 只在 **第一次** capture 填 scrollback |
| ivyTerm | `TmuxAPI` | 每个 pane 一个 VTE（`TmuxTerminal`）；Adwaita tab 里 widget **留着** | 无 replica dump |
| Ghostty | tmux `Viewer` | core `Terminal`；GUI 叫 **`Surface`**（`src/Surface.zig`：一块能画、能收键的最小 widget，不管它是窗口还是 tab） | Viewer 不是第二仿真器 |
| WezTerm | **`TmuxDomain`**（一种 mux Domain，可同时有多个 Domain） | `TmuxPty` 假 PTY → 仍是 WezTerm **那一个**仿真器 | Domain 是连接，不是 dump |
| cmux | remote tmux control | **`TerminalSurface.processRemoteOutput`** | seed 结构（discard/snapshot/catch-up），不是每帧重拍 |
| Muxterm 现在 | `CoreBridge` / pool | `PaneView` + VTE | **多出来的** `ReplicaStore.visible_ansi` → `reset`：这层该降成 Index |

我们叫 Surface，是因为 Ghostty / cmux / 图形 API（Vulkan、GTK `GdkSurface`）都用这个词表示 **像素落点**。它没有覆盖「多路 tmux」——那一块 Muxterm 已有名字：`ConnectionPool` / `WarmConnectionSlot`。整套机制 = Director（含池）+ **常驻** Surface + Index。名字略窄，先不动。

**他们有「多路 + 快切」，而且这正是 `-CC` 客户端的本意。** 不是我们独创。

- **同一条 tmux 里换 window/pane：** ivyTerm 的 tab 子 widget 不拆；iTerm2 已打开的 window 里 `PTYSession` 一直在吃 `%output`；WezTerm pane 活在 mux 里。切过去是 show/hide（GTK 本来就不画隐藏页），**不是**重连、不是再 capture、不是 dump 网格。
- **多条 tmux / 多机：** WezTerm 多个 Domain；iTerm2 多个 `TmuxController`；Muxterm `ConnectionPool` 已经按 key 保活，切目标不 shutdown。
- **「不重要的不渲染」：** iTerm2 的 `hiddenWindows_`——**没打开**的 tmux window 根本不建 VT，仪表盘里再 open 才 capture 一次。已打开的，后台仍吃字节（跟不上才 `%pause`，再看时 unpause + 补历史，第一次会慢一点）。没有人在已打开的 tab 之间「只 dump 重要行」。

Muxterm 现在切 tab **慢且白**，是因为做了别人不做的两件事：

1. `refresh_ui` 再 `present_from_replica`（重播网格 + reset）
2. `LayoutHost::apply_layout` `panes.retain(当前布局)`——换 window 就把上一窗的 VTE **扔掉**，回来只能再播种

Surface 方案要快，必须：**别的 tab 的 PaneView 留在 HashMap 里**，只从 GTK 树摘下、换窗再挂回去；`%output` 继续 feed 后台 Surface（或以后 `%pause`）。这和「只画当前看得见的」不冲突：GTK 不画隐藏 widget；CPU 上可以选择 pause 洪水 pane。

---

## 2. 参考实现（已克隆到 `/home/wlz/Developer/terminal`）

只读。改 Muxterm 时对照，不要抄 UI。

| 目录 | 项目 | tmux 显示怎么做 | 对我们的结论 |
|---|---|---|---|
| `iterm2/` | iTerm2 | `%output` → `tmuxReadTask:` 进 **该 pane 的 VT100**；历史用 `capture-pane -peqJ` **一次**，`TmuxHistoryParser` 填 scrollback；之后只增量 | **模仿对象。** 显示面 = 唯一 VT |
| `ivyterm/` | ivyTerm（gtk4+VTE，最近邻） | 解转义 `%output` → `vte.feed`；未 synced 丢弃 live；`capture-pane -J -p -eC -S - -E -` 一次；`send-keys -H` | **Linux 应抄这条路径** |
| `ghostty/` `src/terminal/tmux/` | Ghostty Viewer | 每 pane 一个 `Terminal`；capture 完成前 ignore output（源码 TODO 已写明） | 与 ivyTerm 同一状态机 |
| `wezterm/` `mux/src/tmux_pty.rs` | WezTerm | 假 PTY：`%output` 当 read，`SendKeys` 当 write；真正仿真器仍是 WezTerm 那一个 | 「字节当 PTY」= Surface |
| `cmux/` | manaflow-ai/cmux | `surface.processRemoteOutput`；seed = capture 快照 + 丢弃快照前 live + catch-up；**禁止**在 seed 前把 live 当画面 | 种子/追赶模型可抄；他们自认不做本地 reflow |
| `tmux/` | tmux | `control.c`：`%output` 队列、`%pause` | 客户端必须会 pause，否则 20 万行打爆 |
| `alacritty/` | Alacritty | **无** `-CC`。渲染参考（GPU 网格） | 不要从这里抄 tmux |
| `herdr/` | Herdr | 换掉 tmux 的 runtime；`terminal.frame` 给第三方 | 以后接 Runtime，不是本轮 |
| `remux/` | camerondurham/remux | 跨机发现 pane，不渲染 VT | 产品层「工作区池」，不是 Surface |
| `tmex/` `psmux/` | tmex / psmux | tmex 通知；psmux 是 `-CC` 替身，输入测 `send-keys -H` | 输入编码测 `-H` |

cmux 远程 tmux 测试密度极高（`cmuxTests/RemoteTmux*`、`docs/remote-tmux-*.md`）。他们的原则：**tmux 网格是权威，客户端 seed 一次再 feed-forward**，不是每帧重拍。

### 2.1 共同状态机（iTerm2 / ivyTerm / Ghostty / cmux）

```
attach
  → 建 Surface（空 VTE）
  → capture-pane（可见或含历史）  [pending]
  → 此期间到达的 %output 进 discarded 队列，不画
  → capture 到齐：feed 快照，标记 synced
  → 快照之后的 %output catch-up 按序 feed
  → 稳态：每个 %output 只 vte.feed(raw)，永不 reset
切 pane / 切 tab
  → 已 synced 的 Surface 只显示/隐藏，不 reset
  → 新 pane 走上面的 seed，不做 visible_ansi
```

ivyTerm 在 capture 后用若干 `\n` + `ESC[#A` 把视口对齐到底（`scroll_view`）。这解释了「只看到中间、在 tmux 里滑到末尾才好」——视口没钉在尾部。

### 2.2 输入

| 实现 | 编码 |
|---|---|
| ivyTerm | `send-keys -t %N -H` 十六进制字节 |
| psmux 测试 | `-H` 是字节通道，有 roundtrip 单测 |
| Muxterm 现在 | `send_keys_bytes` → `send-keys -l`；2105 全是 `-l` 逐字符 |

`-l` 把 CSI/`\r` 当文字或错误转义。Surface 输入必须是 **字节通道 `-H`**（或等价的 hex），与 ivyTerm 一致。

---

## 3. 硬性定律（违反 = 回归）

1. **One Surface.** 一个进入产品拓扑的 pane，显示路径上只有一个常驻 VT parse；当前不可见
   不等于未注册。
2. **No dump.** `ReplicaStore::visible_ansi` / `present_from_replica` **不得**出现在 live `%output` 或 CUP 风暴路径。Index 自用。
3. **No reset to chase frames.** `vte.reset` 只允许新建 Surface 或确认的 resize 错格；
   用户 Ctrl-L 必须走终端输入，不能由 UI 直接 reset。CUP 风暴用原始帧喂 VTE（或丢
   中间帧只 feed **原始** last frame），不要 dump。
4. **Seed once.** capture 完成前不画 live；完成后一次性 feed；再增量。
5. **Follow tail.** 直播 `history_offset=0`；新输出后视口在底（alt-screen 由字节自己切）。禁止用 replica dump 模拟滚动来「修」TUI。
6. **Bytes in, bytes out.** 键盘 → `send-keys -H`；`%output` 解转义 → 原样 feed。
7. **`%pause` 是流控，不是切 tab 刷新。** 已打开的 Surface 禁止 `pause` + capture。洪水 pane 的 `pause-after` **尚未实现**（TODO，见 §7.4）。
8. **Pane id.** 控制协议里 pane 是 `%N`。`缺少 @ 前缀: %64` 必须当 bug 修，不是忽略。
9. **Index never becomes Surface.** `visible_ansi` / `surface_seed_ansi` / `scroll_ansi` / `paneSurfaceSeedANSI` **不得**进 VTE/SwiftTerm。Herdr `pane.read` / `visible_ansi` 也只播种 Index；只有经过当前 generation/event ordinal/wire seq 过滤的原始 `terminal.frame` 才能进入 Surface。full 建 baseline，diff 追赶；旧 generation 永不重播。
10. **Open Surfaces keep eating.** Surface 以 `(WorkspaceId, PaneId)` 为 key 常驻；隐藏 tab 与后台 workspace 的 PaneView 继续 `feed` 原始 PTY 字节，只是不绘制。`poll_background()` 不能只喂 Index/attention 后丢掉 Surface event；切回时只能 show/hide，不能靠 Index dump 补画。
11. **No pause to recapture an open Surface.** 已经 seed 过的 pane，切 tab/pane 不得再抓屏。从未 seed 的 pane 才走定律 4 一次。
12. **History is lines, not a stream.** 第一次打开时 Runtime 把 capture 解析成行写入 Surface scrollback，不是 VT `feed()` 重放。已打开的 tab 再切回来只显示，不再抓。

Ctrl-L 属于终端输入，不是 UI 的 `vte.reset`。清屏后只允许后续原始 frame/output 改变
像素；切 tab、resize、observer 重连或 Index 更新都不得把旧屏重新 feed 回来。

本地 shell 不受 4、7、11、12 约束（它有真 PTY）。定律 1、2、9、10 对本地同样成立。

---

## 4. 和旧计划的关系

| 旧物 | Surface 下 |
|---|---|
| `LINUX-PLAN` Phase C/D/E | 控制面/chrome 可留；**显示路径作废** |
| ReplicaStore | 降为 Index，继续 feed 同一字节 |
| `scroll_history` + 几何 ANSI | 删除显示用途。滚动用 VTE 自己的 scrollback（shell）或 alt-screen 字节（TUI） |
| `RenderPolicy` + `last_visible_frame` | 只允许作用在 **原始** `%output` 上（丢中间 CUP 帧），结果仍是原始字节，不是 `visible_ansi` |
| C8 ASCII PROMPT 测试 | 可留作 Index 单测，**不能**当 Surface 完成 |
| 搜索 / attention 小终端 | **Surface 绿了再做。** 小终端也是 Surface，吃同一字节，禁止 dump |

---

## 5. 测试金字塔（必须能抓住 2105）

### 5.1 小：Director / 协议

- `%output` 解转义与原文 bytes 恒等（含 CUP、UTF-8 盒线、真彩）
- `send-keys -H` 对任意 `[u8]` roundtrip（对照 psmux `test_send_keys_literal_byte.rs`）
- `parse_line("%… %64 …")` 接受 `%` pane id，不再 WARN `@`
- `%pause` / `%extended-output` 能 parse

### 5.2 中：Surface（GTK VTE，无 AppWindow 或一个 Window）

函数名固定（回归门测试的契约）。

- `surface_live_feed_does_not_reset`：synced 后 20 帧 CUP，`RenderTrace.resets` 不增加；可见 `frame-19`
- `surface_typing_overwrites_in_place`：`\r` + 更长前缀；完整句在 `visible_text` **恰好一次**
- `surface_codex_fixture_raw_feed`：`codex-tui-sanitized` **直接 feed**；头+底+盒线
- `surface_seed_drops_output_until_capture`：capture 前 live 不进 VTE；之后 catch-up 进
- 切 pane/tab 的 `resets` 增量：widget 或 `linux_live_e2e` 的 `isolated_tmux_switch_tab_resets_bounded`

### 5.3 大：隔离 tmux e2e（`-L muxterm-test-*`）

- 建 session，AppWindow attach，`send-keys` 打 `MUXTERM_TYPE_TOKEN`，5s 内 VTE **恰好一份** token，且在底行附近
- 再开一个 window，点 tab：VTE 非空、过程中 `resets` 不暴涨（阈值写进测试，例如切一次 ≤1）
- 隔离 tmux 里跑合成 CUP 脚本（`codex-tui-sanitized` 或 `ESC[H` 20 帧）：停在末帧，无白屏（resets 有界）
- 滚轮：shell 输出 200 行后向上能看到 `line-0`（VTE scrollback，**不是** replica dump API）

禁止：`include_str!` 34MB `*.log`；直接 `popover.popup()`；只 `contains(TOKEN)` 不数出现次数。

---

## 6. 参考树维护

`/home/wlz/Developer/terminal/` 不提交。更新：`git -C <repo> pull --ff-only`（均为 `--depth 1`）。清单见该目录 `README.md`。

核查时间：`2026-08-15T21:26:10+08:00`；§7 核查 `2026-08-24T16:55:34+08:00`。源码以克隆树为准。

| 声明 | 来源 |
|---|---|
| `%output` 进唯一 VT；`%pause` | iTerm2 `TmuxGateway.m` `tmuxReadTask:`；`PTYSession.m` `pausePanes`；tmux `control.c` / `tmux.1` CONTROL MODE |
| 未 synced 丢弃 live；`vte.feed`；capture 一次；`-H` | ivyTerm `tmux_widgets/terminal/mod.rs` `feed_output`；`tmux_api/send.rs` |
| capture 完成前 ignore `%output` | Ghostty `src/terminal/tmux/viewer.zig` 行 21–22 |
| seed = snapshot + discard + catch-up | cmux `Sources/RemoteTmuxPaneSeed.swift` |
| `-H` 十六进制字节 | `tmux.1` send-keys；`cmd-send-keys.c` `args_has(..., 'H')`；[OpenBSD tmux.1](https://man.openbsd.org/tmux.1) |
| 假 PTY | WezTerm `mux/src/tmux_pty.rs` |
| 历史写成格子，不是 ANSI `feed` | iTerm2 `TmuxWindowOpener.m` `capture-pane -peqJN -S -N` → `TmuxHistoryParser` → `VT100Screen.setHistory:` |
| 控制模式是文本，客户端自己画 | [tmux wiki Control Mode](https://github.com/tmux/tmux/wiki/Control-Mode)（核查 `2026-08-24T16:34:51+08:00`） |

---

## 7. 2026-08-24：字节直达（纠偏）

核查：`2026-08-24T16:55:34+08:00`。

文档 2026-08-15 已经禁止 dump。实现后来把 Workspace/PaneBuf 当成显示缓存：切 tab `pause`+capture，再用 `surface_seed_ansi` 灌进 SwiftTerm。卡顿和「历史只能滑一点」都来自这条，不是来自「core 里有 Workspace」。

### 7.1 两路内容，两个去处

`runtime/tmux` 解析 `-CC` 之后只交出两种产品数据。Workspace 和前端都看不见 `%output` / `capture-pane` / `$N`。

| Runtime 解析出来的 | 产品事件 | 谁用 |
|---|---|---|
| 控制协议（窗口树、焦点、layout、pause 通知） | `LayoutChanged` / `ActiveTabChanged` / `PaneResized` / … | Workspace 存拓扑；前端改分割和 tab |
| PTY 字节（解转义后的 pane 输出） | `PaneOutput { pane, data }` | **只**进该 pane 的前端 Surface |

Workspace **不**解析控制协议。它收 Runtime 已经翻译好的 `StateChange`，维护 Tab/Pane 树，把同一份 `PaneOutput` 喂给 Index（搜索/attention）。它不画像素，不把 Index 网格再编码回去。

以后换 Runtime（Herdr 等）只要交出同一套 `StateChange`。产品层不对 tmux 特化。QuickConnect 里可以出现 runtime 名字 `tmux`，那是用户选工作区类型，不是协议泄漏。

前端认的能力是「直连 PTY」还是「镜像 PTY」（查询应答、client 尺寸），不是 `if tmux`。

### 7.2 谁渲染

每个 **已打开** pane 一个前端 VT（VTE / SwiftTerm）。PTY 字节 `feed`，禁止 `reset` 追帧。

切 tab：已有 Surface 只显示/隐藏，继续吃 `PaneOutput`。不要 core 预渲染一帧再贴到前端。

前台 Workspace：该工作区里出 PTY 的 pane 都可以有 Surface（tab 栏上的页都算打开）。后台 Workspace：连接和 Index 仍在池里；像素控件只保留已经建过的，不再新建。池有容量上限，不是无限多个 Workspace 同时养全套 SwiftTerm。

### 7.3 本轮实现

- 切到已经 seed 过的 tab：Runtime 不再 `pause` / `capture-pane`（`initial_capture_done` 直接跳过）。
- 已经 seed 的 Surface：`output-dropped` / OutputGap 只 resume live lane，不再 pause+capture。洪水 pause-after 仍是 TODO(surface-7.4)。
- 前台 Workspace 的 `PaneOutput` / `PaneSnapshot` 进该工作区所有 pane 的 Surface（tab 栏上的页都算打开）；禁止 `paneSurfaceSeedANSI` / `visible_ansi` 当显示。
- 后台 Workspace：core 继续吃字节进 Index；已经建过的 Surface 在**主线程**继续 `feed` 到**该 Workspace 自己的** VT 树。禁止在后台 GCD 队列改 SwiftTerm。不新建 widget，不把 Index dump 当切回来的刷新。
- 切 Workspace / 切已加载的 tab：挂已有 Surface 树，不拆 Auto Layout 重建。
- 第一次 seed 仍用 Runtime 的 `PaneSnapshot`（可见屏 + 模式）。attach 前 tmux 历史在可见屏之后按行回填（`PaneHistory`），写入 native scrollback，不 `reset`，也不把 `-S -N` 当 VT 流 `feed()`。
- Workspace 数量受池上限约束，默认 5，不是无限格。

### 7.4 TODO（本轮不实现）

1. **洪水 `pause-after`。** 某个 pane 的 Surface 跟不上时，只对 **该 pane** `refresh-client -A %N:pause`，追上再 continue。代码里搜 `TODO(surface-7.4)`。现在不要用 pause 当切 tab 手段。不能无限吃 CUP 风暴还保证 60fps。
2. ~~**第一次打开按行填历史。**~~ **已落地（2026-08-25）：** Runtime 在可见屏 seed / continue 之后抓 `capture-pane -peqN -S -N -E -1`（物理行，不要 `-J`），产品事件 `PaneHistory` 带行数据；前端 `muxtermPrependHistoryLines` 按列宽写入 scrollback。禁止把 `-S -10000` 当 VT 流 `feed()`，也禁止为了不卡而永远只抓可见屏。已打开的 tab 再切回来不得再抓。

### 7.5 不卡顿的验收（不是保证任意负载 60fps）

- 已打开的 tab/pane 切换：没有 pause、没有 capture、没有 reset。
- 打字和 TUI 跟 `PaneOutput` 走，不跟 Index dump 走。
- 池里 Workspace 数量受 `WorkspacePoolPolicy.max_slots` 限制（默认 5）。
- 某个 pane 刷爆时允许掉帧；TODO 落地后只 pause 那一个 pane。

---

## 8. Topology batch commit boundary（2026-08-24）

Structural events 和像素事件不能交错提交。一个 poll batch 必须先把最终 topology 应用到
Core，再对每个 affected workspace 执行一次 LayoutHost sync/mount；只有这个 commit boundary
之后才能 feed full frame，最后才能 feed output。structural-only batch 也必须提交一次，不能
因没有 frame/output 而逐事件 inline refresh。

PaneView input callback 只产生 `(WorkspaceId, PaneId, bytes)` FIFO item；GLib poll drain 时
按 owner 路由 `WriteRaw`。切 tab、切 workspace、detach 或 layout rebuild 不得把迟到输入改投
新的 active pane。Attach、Reattach 和 Create 的 Surface mount 都遵守同一 boundary。
