# FRONTEND.md — 前端契约

图片粘贴（Core 文件传输 + frontend 剪贴板）：[`CLIPBOARD.md`](CLIPBOARD.md)。

产品树：[`WORKSPACE.md`](WORKSPACE.md)。像素定律：[`SURFACE.md`](SURFACE.md)。
配置：[`CONFIG.md`](CONFIG.md)。打开路径：[`CATALOG.md`](CATALOG.md)。
Agent 对话视图：[`AGENT-CHAT-VIEW.md`](AGENT-CHAT-VIEW.md)。
多前端成本与统一框架评估：[`FRONTEND-PLATFORM.md`](FRONTEND-PLATFORM.md)。
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
  工作区段顺序固定：`Shells`、`Agents` 两个固定聚合行永远在最前（`S` / `A` 地标，
  不可关闭、不可拖动），后面才是真实 workspace 的 `Cmd-Ctrl-N` 编号。
  `shell` runtime 的原始 workspace **不单独占一行**——它的 tab 只经 `Shells` 投影出现，
  否则启动的本机 shell 会以 `shell · exited` 这样的原始行挂在侧栏上。
  启动时若当前就是本机 shell，展示状态即为 `Shells`（聚合行高亮），
  与 macOS 的 `workspacePresentation = .shells` 一致。
- **快速面板**：fuzzy 候选 = Catalog `Candidate`（Project / Worktree / Existing / Recent）
  + 已开 workspace + activity record 跳转。选择产出 `OpenRequest`。
  Workspaces tab 的根列表固定四段顺序，不要按字母或时间重排：
  1. 前端合成的聚合行 `Shells`、`Agents`（不可关闭、不写回 Core）；
  2. `Open`（进入打开流程，见下）；
  3. 已打开的 workspace，按 `Cmd-Ctrl-N` 顺序编号，`Shells`/`Agents` 不占号；
  4. 配置里的 Project，末段是 discovery 到、尚未保存的现有 runtime。
  已保存项带 `Project` 徽章；未保存的现有 runtime 行内给 `Save`，点击只写
  `[[projects]]`，不 attach，也不做乐观置位（成功提交后从 Core 重新读配置）。

  打开流程（`Open`）：选机器（local / SSH alias）→ 该机器上的现有 session 或
  `Open New Path` → 目录 → 确认页。本地路径必须先展开成绝对路径（`~` 只在
  SSH 上留给远端 shell 展开），否则目录列不出来、新建也「找不到路径」。确认页是唯一新增的一步：运行时默认
  `herdr`，可切 `tmux`；名字默认取路径末段，可编辑。确认后一次 `OpenTarget`
  （`create_if_missing`）；现有 session 走 attach-only，永不新建 session。
  中途 `Esc` 逐层退回，退到根列表还按一次才关面板；任何一层都不产生打开请求。
- **命令面板**：Action Catalog，不按 GTK/AppKit 类型存盘。**目录里不允许出现「列出来
  但点不动」的占位项**；每条命令都必须有真实动作：
  - `Local` / `SSH`：直接把快速面板导航到本机会话列表 / 机器列表；
  - `Rename Tab…` / `Rename Workspace…`：输入浮层（带出当前名字）→ Core `rename_*`；
    空名字留在浮层报错，不提交；
  - `Move Tab Left` / `Move Tab Right`：Core `move_tab`，到两端就不发命令；
  - `Language`：换成语言子菜单（跟随系统 / English / 简体中文），选择后立刻生效并
    写回 `[ui] language`；`Esc` 只退回命令目录，不关面板；
  - `Theme`：写完 `theme.name` 后从 Core 快照重读主题（浅 ↔ 深）；
  - `Status Bar Mode`：切 `statusbar.mode`；`tmux` 模式显示 tmux 自己的左右段，
    `theme` 模式用前端主题文案；
  - `Muxterm Info`：打开版本面板（version、commit、当前调试日志文件、GitHub issue
    入口）。版本/commit/日志路径一律由 Core 提供，前端不自己拼。两个复制按钮
    **永远可点**：日志开着复制文件路径，没开复制日志目录，并在面板底部回一句
    「… copied」——不能让用户点了没反应。
  面板文案走 `frontend/utils/i18n` catalog，所以切换语言后目录立刻跟着变。
  根列表里**已经打开的 workspace 不再重复出现**成 Project 或 discovery 行。身份按可信度
  依次判定：Core 写的 `spec.provenance.project_id` → canonical target 完全一致（SSH 主机
  读 `canonical.target`，不是 `alias`；session / socket / Herdr id 一并比较）→
  `canonical.workspace_id` → 名字 + runtime + session。`Shells` / `Agents` 两个特殊槽
  永远在最前面。
- **快速面板里的 Cmd-K**：面板是 key window 时，`Cmd-K` 只换成当前选中行的动作
  （工作区打开 / 关闭 / 更新，连接 attach，提醒跳转 / 隐藏等）。不带命令面板的全局命令。
  输入继续过滤这份短列表。`Esc` 先退出这一层，再恢复进入前的查询和原来的 tab，
  不关掉面板，也不增加第四个 tab。`Cmd-Shift-P` 仍打开独立命令面板。Shells 仍是
  `Cmd-Ctrl-S`。
  动作集按行型固定：聚合行只有 Open；workspace 是 Open / Close workspace；
  Project 与已保存的现有 runtime 是 Connect / Edit / Delete；未保存的现有 runtime
  是 Attach / Save；提醒是 Jump / Hide；`Open` 这类导航行没有行内动作。
- **设置页**：schema-driven，见 §4。

### 2.4 交互

- 切 workspace / tab：只改 SceneStack 可见子树 + focus，零 Core 调用。
- 命令合并：同一目标的连续 SwitchTab / activate 只发最后一个。
- workspace 的前台意图必须在下一次事件泵中、input/resize 之前调用 Core Pool activate；
  不能只更新前端路由，否则 Herdr 会一直保持后台 Observe。点击路径仍不调用 FFI，
  不查询快照、不重新连接、不做前台校准。
- focus：Scene 变可见 → focus 其 active pane；键盘只路由给可见 Scene 的 focus pane。
  **焦点永远跟随 Core 的 active pane**：切 tab、分屏、`Cmd-[` / `Cmd-]`、Core 回拓扑之后，
  前端都要把 focus 同步到当前 tab 的 active pane，光标画在那里。否则用户看到的是
  「切了 tab / 分了屏，光标没了、打字没反应」。手动点 pane 的意图（`SwitchPane`）
  在 Core 确认前不被抢走。
- 活动（Agents / Commands）：前端按固定间隔拉 Core 的
  `muxterm_attention_snapshot`，并按 `WorkspaceId::replica_id()` 把活动条目 join 回
  产品 workspace id。两套 id 对不上时侧栏会永远空着——这是一个真实踩过的坑。
- `Cmd-[` / `Cmd-]`：在当前 tab 的 pane 之间循环。顺序取布局叶子（屏幕几何顺序），
  tmux zoom 时退回当前 tab 的完整快照顺序；结果永远是显式的 `SwitchPane(pane)`，
  不交给 Core 的全局 active pane 推断（否则分屏后可能跳到别的 tab / 没有目标）。
  zoom 中切换会先退出 zoom，再对目标 pane 重新全屏。
- 本机 shell 退出：`shell` + `local`、状态 `exited`、且没有 tab/pane 时，前端自动补一次
  `NewTab`（只发一次，Core 回包后复位），窗口始终留着一个活着的本机 shell。
- 关闭：close workspace 是 Scene 的唯一销毁点。detach ≠ destroy。
- 嵌套分割：每次只替换当前叶子 pane，不重新平铺全树。
- 焦点操作后回到终端，不落到工具栏。不要底部输入框发 send-keys。
- Tab 显示：序号 + 名字；多 pane 可加数量后缀。不要每个 pane 一个 Notebook tab。
- SSH 端口弹层：Forward / Stop 与 Open 各自统一列宽。Open 经本机系统浏览器打开
  `http://<SSH 目标 IP>:<远端端口>`；转发后的地址仍指向本机端口，可点击、复制、用 L 切换 LAN 访问。
  SSH HostName / DNS 由 Core 后台解析并经 FFI 返回；地址未就绪时禁用 Open。
- 关 GUI 窗：有 `PersistDetach` 的 Runtime **detach**；shell **shutdown**。
- 迟到输入不得改投新的 active pane。
- `Cmd-I` 独立输入框：普通竖长窗口（不置顶），按 pane 保存草稿；tab 栏只出现
  **当前 pane 和有内容的 pane**，空草稿不占位置，关闭窗口只收起，草稿一直保留到手动
  清空。单栏 Markdown 语法高亮（原文不变，不做左右分栏预览），常驻 `Local` /
  `SSH <host>` 徽章。`Cmd-K` 打开当前页菜单（复制 / 写入 / 清空），`Cmd-Enter`
  与 Write 按钮同义：写入终端并清空这一页草稿（`Cmd-Z` 可撤销），不自动回车。
  这两个快捷键必须在主菜单之前消费（AppKit 先问菜单的 key equivalent，主菜单里
  `Cmd-Enter`/`Cmd-K` 分别属于 pane zoom 和 Shells），输入框用自己的 local key
  monitor 先拿到事件。粘贴图片走 Core 只
  存储通路（`imageStore`），路径插回发起粘贴的那一页草稿；右下角只提示编码 / 上传 /
  失败，路径插入后不再弹成功提示（路径本身已可见）。

### 2.5 渲染（像素细节见 SURFACE）

- pane 创建即建 PaneSurface，pane 关闭即毁，**切换不碰它**。
- 隐藏 Scene 不绘制，但 VT 继续 feed，画面始终是最新的。
- 部分渲染：VT 标 dirty rows，同一帧内多次 `PaneOutput` 合并成一次重绘。
- 文本属性必须画出来，不能只解析：粗体 / 斜体 / 下划线 / 删除线各有真实字体变体；
  SGR 粗体按 SwiftTerm 规则把 ANSI 0–6 提亮到 8–14，`dim` 与背景对半混合，
  `hidden` 保留格子宽度与背景但不画字形。markdown / agent 输出靠这些属性区分标题和强调，
  只变色会让正文和标题糊成一片。
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
| Scene 容器 | `GtkStack` page | NSView 视图树 | 待定 | per-workspace buffer 组 | 无 Scene |
| PaneSurface | vte4 或自绘 | `TerminalView` | 待定 | 单元格 buffer | 无 |
| 主线程桥 | glib MainContext | `DispatchQueue.main` / `@MainActor` | 待定 | tokio → render tick | 无 |
| ViewStore | `Rc<RefCell<…>>` 或等价 store | 主线程持有的 view store | 待定 | owned struct 直读 | 无 |
| Settings | `AdwPreferencesWindow`（schema-driven） | schema-driven 设置窗口 | 待定 | `$EDITOR` | 无 |
| 平台集成 | libnotify、xdg | NSNotification、bundle、菜单栏 | 通知 / 任务栏 | 无 | 无 |
| 输入编码 | keymap → SendKeys 字节 | TerminalInputEncoding → SendKeys | 待定 | crossterm → 字节 | 参数直译 |
| FFI 口 | `utils/corebridge` | Swift CoreBridge（同一 C ABI） | 待定 | `utils/corebridge` | `utils/corebridge` |

cli 不受渲染契约约束：命令直译 Core 调用。windows 框架选型到实现期再定，必须实现 §2 词汇表。

**macOS 不是 SwiftUI 前端。** 当前 77 个文件 `import AppKit`，0 个文件 `import SwiftUI`；终端视图基于
SwiftTerm。新增 UI 沿用 AppKit，不引入第二套视图技术。

**模型层优先共享。** 侧边栏投影、Attention 排序、目录补全、面板模型、草稿与输入编码这些纯逻辑
应放进共享 Rust 后再给两侧适配，不再按平台各写一份。范围、成本与决策门见
[`FRONTEND-PLATFORM.md`](FRONTEND-PLATFORM.md)。

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
- 纯模型逻辑优先搬进共享 Rust（见 [`FRONTEND-PLATFORM.md`](FRONTEND-PLATFORM.md)），Swift 只保留
  widget 树、主线程桥与平台集成。

**Agent Chat 视图（所有 GUI 前端）：**

pane 可以有两种投影：Terminal（已有 `PaneSurface`）与 Chat（只读事件流 + composer）。
Chat 不改 Surface、不改 pty 尺寸、不建第二个 VT；无 adapter 的 pane 不显示入口。
数据来源、绑定方式、能力位与降级规则见 [`AGENT-CHAT-VIEW.md`](AGENT-CHAT-VIEW.md)。

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
