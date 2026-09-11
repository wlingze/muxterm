# FRONTEND.md — 前端契约

产品树：[`WORKSPACE.md`](WORKSPACE.md)。像素定律：[`SURFACE.md`](SURFACE.md)。
配置：[`CONFIG.md`](CONFIG.md)。打开路径：[`CATALOG.md`](CATALOG.md)。
施工：[`../TASKS.md`](../TASKS.md) Phase 7 / 9。

**一句话：** 所有 frontend（cli / tui / linux / macos / windows）只经 C FFI + 统一
`frontend/utils/corebridge`（CoreBridge）使用 Core。已打开的 Workspace 各有一棵常驻
Scene；切换 = 换可见场景，点击路径零 Core 调用、零锁等待。

页面结构是平台无关的。差异只剩语言、widget 树、主线程桥、平台集成。

---

## 1. 硬约束

1. 只走 C FFI。Rust frontend 用共享安全 wrapper `frontend/utils/corebridge`
   （handle 所有权、DTO、borrowed bytes 复制、error envelope）。文案走
   `frontend/utils/i18n`。禁止散装 `ffi_bridge`，禁止 `use crate::core::...`。
2. frontend 只见 `Candidate` / `OpenRequest`。永不构造 `WorkspaceSpec`。
3. 问能力用 `support()`，禁止 `if runtime == "herdr"`。
4. 一个已打开 Workspace = 一棵 Scene；每 pane 恰好一个常驻 `PaneSurface`。
5. **唯一** FFI 事件消费者是 `EventPump`。UI 线程不直接碰 C handle。
6. UI → Core 只经 `CommandQueue`（异步入队，合并同类命令，不等结果）。
7. 没有 warm/cold slot、`bridgeLock`、串行后台 poll 队列、前台权威校准。
   live owner 是 Core `WorkspacePool`；连接复用是 `ConnectionRegistry`。
8. live 画面只吃 `PaneOutput` / `PaneFrame` / `PaneHistory`。禁止 `visible_ansi`。
9. 设置页由 Schema/Manifest 生成；frontend 不硬编码字段，不直接读写 `config.toml`。
10. windows 本轮只占位，但必须能挂上本文词汇表。

---

## 2. 通用层（平台无关）

本层是词汇表 + 状态机 + 契约。每个平台用本语言实现同一套名字；禁止第二套概念。

### 2.1 组件

| 组件 | 职责 | 禁止 |
|------|------|------|
| `AppShell` | 窗口骨架：Sidebar + SceneStack + 主区槽位 + Overlay 挂载点 | 含业务状态 |
| `Scene`（WorkspaceScene） | 一个已打开 Workspace 的完整视图树：TabBar + PaneGrid（每 pane 一个常驻 PaneSurface）+ 场景内状态。从 open 到 close 常驻 | 因不可见而销毁 / reset |
| `SceneStack` | 持有全部 Scene，切换 = 换可见子树 | 切换时调用 Core |
| `PaneSurface` | 每 pane **恰好一个**常驻 VT + renderer；新建时可接收一次 Surface seed | reset 追帧；把 Index 的 `visible_ansi` 当 live 输入 |
| `ViewStore` | per-WorkspaceId 的 UI 只读快照：topology / activity map / per-pane render mailbox | 被 Core 线程直写（必须经主线程桥） |
| `EventPump` | **唯一** FFI 事件消费者：drain Pool 批次 → 写 ViewStore | 存在第二个事件消费者 |
| `CommandQueue` | UI → Core 的 Task 通道；合并同类命令；不等结果 | 同步阻塞等 Core |
| `Overlay` | QuickPanel / CommandPalette / Search / AttentionPanel | 属于某个 Scene |
| `SettingsView` | schema-driven 设置页 | 手写字段 form |

一个页面 = 一个 Model（状态）+ 一个 View（绘制）。页面之间禁止互相 import；共享只有
design token（间距 / 字号 / 色板 / 圆角）和通用组件。

### 2.2 状态流

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

FFI 入口只剩两个：EventPump（事件）与 CommandQueue（命令）。Core 内部串行化。
UI 线程从不直接碰 handle，所以不需要 `bridgeLock`。拓扑常流，没有「缓存过期后再校准」。

### 2.3 页面骨架

```text
AppWindow
├── Chrome：Sidebar（workspace 列表 + activity badge + runtime/transport 徽章）
│            + 主区（TabBar / PaneGrid / StatusBar）
├── Overlay：QuickPanel、CommandPalette、Search、AttentionPanel
└── 独立窗口：Settings、TargetConfig
```

- **侧边栏**：数据 = ViewStore 的 Pool 快照 + Activity lane badge。点击 = 换可见 Scene。
  按 `provenance.project` 归组，worktree 缩进。tmux / shell / herdr 同一套 UI 数据。
- **快速面板**：fuzzy 候选 = Catalog `Candidate`（Project / Worktree / Existing / Recent）
  + 已开 workspace + activity record 跳转。选择产出 `OpenRequest`。
- **命令面板**：Action Catalog，不按 GTK/AppKit 类型存盘。
- **设置页**：schema-driven，见 §4。

### 2.4 交互

- 切 workspace / tab：只改 SceneStack 可见子树 + focus，零 Core 调用。
- 命令合并：同一目标的连续 SwitchTab / activate 只发最后一个。
- focus：Scene 变可见 → focus 其 active pane；键盘只路由给可见 Scene 的 focus pane。
- 关闭：close workspace 是 Scene 的唯一销毁点。detach ≠ destroy。
- 嵌套分割：每次只替换当前叶子 pane，不重新平铺全树。
- 焦点操作后回到终端，不落到工具栏。不要底部输入框发 send-keys。
- Tab 显示：序号 + 名字；多 pane 可加数量后缀。不要每个 pane 一个 Notebook tab。
- 关 GUI 窗：有 `PersistDetach` 的 Runtime **detach**；shell **shutdown**。
- 迟到输入不得改投新的 active pane。

### 2.5 渲染（像素细节见 SURFACE）

- pane 创建即建 PaneSurface，pane 关闭即毁，**切换不碰它**。
- 隐藏 Scene 不绘制，但 VT 继续 feed，画面始终是最新的。
- 部分渲染：VT 标 dirty rows，同一帧内多次 `PaneOutput` 合并成一次重绘。
- 隐藏 pane 三档资源策略（按 transport 成本配置）：`live` / `coalesce` / `pause`。
  这是资源旋钮，不是正确性条件：切过去永远先看到最后已知帧，不允许白屏等待。
- resize：只对可见 Scene 发 `Task::Resize`；隐藏 Scene 记 pending size，可见时一次性应用。
- gap / `%pause`：该 pane fenced，异步补权威 baseline；不堵点击路径。

Linux Scene 容器是 `GtkStack`：一次只显示一个子 widget，子页面仍留在树里
（[GTK4 GtkStack](https://docs.gtk.org/gtk4/class.Stack.html)）。常驻多 Scene 的稳态绘制成本 ≈ 单 Scene。

---

## 3. 平台特殊层

边界只有三样：widget 树、主线程桥、平台集成。其余全部来自 §2。

| 通用组件 | linux（Rust + GTK4） | macOS（Swift） | windows | tui（ratatui） | cli |
|---|---|---|---|---|---|
| Scene 容器 | `GtkStack` page | NSView / SwiftUI 视图树 | 待定 | per-workspace buffer 组 | 无 Scene |
| PaneSurface | vte4 或自绘 | `TerminalView` | 待定 | 单元格 buffer | 无 |
| 主线程桥 | glib MainContext | `DispatchQueue.main` / `@MainActor` | 待定 | tokio → render tick | 无 |
| ViewStore | `Rc<RefCell<…>>` 或等价 store | `ObservableObject` + `@Published` | 待定 | owned struct 直读 | 无 |
| Settings | `AdwPreferencesWindow`（schema-driven） | SwiftUI Settings scene | 待定 | `$EDITOR` | 无 |
| 平台集成 | libnotify、xdg | NSNotification、bundle、菜单栏 | 通知 / 任务栏 | 无 | 无 |
| 输入编码 | keymap → SendKeys 字节 | TerminalInputEncoding → SendKeys | 待定 | crossterm → 字节 | 参数直译 |
| FFI 口 | `utils/corebridge` | Swift CoreBridge（同一 C ABI） | 待定 | `utils/corebridge` | `utils/corebridge` |

cli 不受渲染契约约束：命令直译 Core 调用。windows 框架选型到实现期再定，必须实现 §2 词汇表。

Linux 渲染调研见 [`RENDERING-OPTIMIZATION.md`](RENDERING-OPTIMIZATION.md)。

---

## 4. 配置

frontend **不直接读写文件**。读 = FFI 配置快照；写 = draft transaction（字段级三方合并）；
`ConfigChanged` → EventPump → ViewStore 热应用主题 / 快捷键 / 字号，不重启、不重建 Scene。

设置页只渲染 Schema + Settings Manifest。新增配置项不改前端代码。
相关节：`[theme]`、`[shortcuts]`、`[platform.*]`、`[[templates]]`。详见 [`CONFIG.md`](CONFIG.md)。

---

## 5. 各前端落地

目录：`src/frontend/{cli,tui,linux,macos,windows}` + `utils/{corebridge,i18n}`。
共享 FFI、EventPump、i18n 与各平台 frontend 都在 `src/frontend/`。linux / macOS 已是常驻
Scene；TUI 用 per-workspace buffer 组。按 [`../TASKS.md`](../TASKS.md) 验收。

**linux（GTK4）：**

- 删除散装 `ffi_bridge.rs`，改走 `utils/corebridge`。
- 目录分层：`app/` `chrome/` `terminal/` `ui/`（对标 macOS）。
- 按 §2.3 把主窗拆成 AppShell / Sidebar / SceneStack / StatusBar / Overlay。
- `notebook.rs` → SceneStack（GtkStack）。`layout_host.rs` 不得 `retain` 丢掉不可见 pane 的 widget。
- `pane_view.rs` / `renderer.rs` 收成 PaneSurface（VT + damage 渲染）。
- `theme.rs` / `keymap.rs` 消费 `ConfigChanged`。

**macOS：**

- CoreBridge 保留为唯一 FFI 口，瘦身为 FFI 函数 + DTO 解码。`Tab` / `Pane` / `LayoutNode`
  快照模型映射到三 lane 事件。
- 没有 WarmConnectionSlot、没有 `bridgeLock`、没有 `backgroundPollQueue`、没有前台权威校准。
- `MainWindow` = AppShell + SceneStack。`TerminalManager` = PaneSurface 常驻池（按 PaneId 持有，
  销毁只发生在 pane close）。
- 连接复用上交 Core `ConnectionRegistry`。
- 页面：Sidebar / QuickPanel / CommandPalette / Settings / TargetConfig / AttentionPanel。
  Settings 走 schema。

切换延迟不是 tmux 全局锁。`bridgeLock` + 串行后台队列 + pause/capture 会把激活拖到数秒。
证据与对照见 [`SURFACE.md`](SURFACE.md) §9。

**tui：** Scene = per-workspace buffer 组；EventPump = tokio select；Overlay 是全屏 replace。

**cli：** 无 Scene。无 subcommand 即 CLI。

**windows：** 占位目录。

---

## 6. 性能预算

- 切 workspace / tab：首帧 ≤ 1 帧（60Hz = 16ms），路径零 FFI、零锁。
- 输入 → send：UI 线程不阻塞，CommandQueue 入队即返回。
- 稳态：无变化不 redraw；chatty pane 不拖慢邻居（lane 隔离 + 三档策略）。
- 内存：每 pane VT scrollback 有界；隐藏 workspace 可降档 pause。
- 启动：窗口首帧不等任何远端数据，Scene 异步填充。

---

## 7. 验收

- 切换 workspace / tab 的点击路径零 FFI（e2e 插桩：activate 到首帧之间无 Core 调用、无锁等待）。
- 快速连点：同一 workspace 只发最后一个目标。
- 隐藏 workspace 的 Control / Activity 常流（sidebar badge 实时）。
- 隐藏 chatty pane：回看先显示最后已知帧，再异步补基线，不白屏。
- 高速输出下其它 pane / 其它 workspace 不被拖慢。
- 设置修改经 draft transaction；`ConfigChanged` 热应用，不重启。
- macOS 前端无 `bridgeLock` / 串行后台队列 / 前台校准；激活日志不再出现秒级 `elapsed_ms`。
- `rg 'bridgeLock|backgroundPollQueue|WarmConnectionSlot|ForegroundAuthority'` 在 macOS 前端为空。
- `rg 'ffi_bridge'` 在 frontend 为空。
- `rg 'crate::core'` 在 `src/frontend` 为空。
- 切 workspace / tab 不再 recapture / reset / 白屏（[`SURFACE.md`](SURFACE.md) §5）。

像素级定律（One Surface、No dump、Seed once、Follow tail）以 [`SURFACE.md`](SURFACE.md) §3 为准。

---

## 8. 禁止

- frontend 连接池 / warm slot / 每 slot 一把锁
- 第二个 FFI 事件泵
- 点击路径上等待 SSH / pause / capture
- 用 Index dump 补画
- `LayoutHost` 按当前布局 `retain` 丢掉其它 pane 的 VT
- 页面互相 import；巨石 `window.rs` 继续堆功能
- 手写 Settings 字段表
- 为 Herdr 单独做侧边栏或 agent 列表
