# FRONTEND-GPUI.md — GPUI 前端方案

> 方案与落地记录。核对时间：2026-11-04T14:22–15:10+08:00（Asia/Shanghai）；
> 终态更新：2026-11-04T19:20+08:00（Asia/Shanghai）。
> 逐文件 parity 清点见 [`gpui-parity/`](gpui-parity/README.md)。
> 相关：[`FRONTEND.md`](FRONTEND.md)（Scene / PaneSurface 契约）、
> [`FRONTEND-PLATFORM.md`](FRONTEND-PLATFORM.md)（框架评估与七个决策门）、
> [`SURFACE.md`](SURFACE.md)（像素定律）、[`PROJECT-STRUCTURE.md`](PROJECT-STRUCTURE.md)。

## 1. 结论

新增第三条前端线 `src/frontend/gpui/`，一份 Rust 代码同时跑 macOS / Linux / Windows。
它**不替换**现有的 Swift（AppKit + SwiftTerm）与 GTK4（vte4）前端，作为并存实现，
先用起来，再按实测决定是否合并。

> **2026-11-09 决策：gpui 成为发布前端。** 对外发布只构建这条线
> （`scripts/build-gpui-macos.sh` → `build/macos/Muxterm.app`）；Swift / GTK4
> 保留为历史参照与兼容实现，不再作为发布目标，也不再要求把 gpui 的行为回接过去。

它把 [`FRONTEND-PLATFORM.md`](FRONTEND-PLATFORM.md) §6 阶段 2 的两个硬门当成设计前提接受下来，
而不是先去证伪：

1. **不能把原生终端子视图塞进 gpui 窗口。** gpui 拥有整个窗口与渲染循环，不提供 NSView / GtkWidget 宿主。
   所以这条前端线自己带一个 CellGrid Surface（§5）。
2. **中文输入法需要自己接。** gpui 0.2.2 有 `EntityInputHandler` / marked text 接口，但候选窗定位要终端自己给。
   现在已接上（§5.3）：marked text 画在光标处，`caret_rect` 返回格坐标换算后的屏幕矩形，
   U+FFFF / U+FFFE 占位符在进 PTY 前被剥掉（对照 macOS `MuxTerminalView` 同一处理）。

选 gpui 的理由只有一条：它是唯一在 macOS 上已有“Rust UI + 终端”先例的候选（`undivisible/herdr-gui`），
且只提供一份 widget 树，能真正消掉 chrome 的移植成本。代价写在 §8。

## 2. 已验证的事实

以下都是 2026-11-04 在本机实测，不是推断。

| 事实 | 证据 |
| --- | --- |
| `gpui` 0.2.2 可在 Rust 1.97.1 / macOS arm64 编译 | 临时 crate 编译通过，仅一处 API 用法错误 |
| 窗口能创建并持续渲染 | `/tmp/gpui-spike` 跑 6 秒无崩溃无报错 |
| 依赖树重 | 首次构建拉入 `blade-graphics`、`metal`、`image`（含 avif/rav1e）等，冷构建分钟级 |
| 许可证 | crate `license = "Apache-2.0"`，与本项目 MIT 兼容 |
| 版本策略 | README 声明 pre-1.0，版本间有破坏性变更 |

## 3. 分层

```text
Core（Muxterm / WorkspacePool）
  │  C FFI
  ▼
CoreBridge / EventPump / CommandQueue     ← 已存在的共享层，一行不改
  │  owned DTO + 事件批次
  ▼
frontend/gpui（本方案）                    ← 单一 widget 树，跨平台
  ├── Scene 容器：WorkspaceScene per workspace，常驻
  ├── Chrome：Sidebar / TabBar / StatusBar
  └── PaneSurface：CellGrid + vte parser（自带 VT）
  │
  ▼
平台胶水：window options / 菜单 / bundle / 剪贴板（按 OS 分叉）
```

边界不变：前端只经 `utils/corebridge`，只吃 `PaneOutput` 原始字节，`EventPump` 仍是唯一事件消费者，
UI → Core 仍只经 `CommandQueue`。

## 4. 目录

```text
src/frontend/gpui/
├── mod.rs              # 模块清单；cfg(feature = "gpui")
├── app.rs              # Application::new().run + 窗口 + 启动装配
├── state.rs            # GpuiApp：EventPump / ViewStore / CommandQueue / surfaces / focus
├── theme.rs            # design token（色板 / 间距 / 字号）
├── model/              # 从 macOS Chrome/ 移植的纯逻辑（不依赖 gpui，逐条对照 Swift 测试）
│   ├── aggregate.rs composer.rs directory.rs image_paste.rs ime.rs
│   ├── input_encoding.rs layout_projection.rs panel.rs query_detector.rs
│   ├── query_reply.rs quick_connect.rs replica_id.rs resize_math.rs scroll.rs
│   ├── search.rs settings.rs shortcuts.rs sidebar.rs status_bar.rs
│   ├── surface_feed.rs target.rs target_editor.rs word_selection.rs
│   └── attention.rs    # 含 KNOWN_AGENTS / attention_rows / strip_terminal_sequences
├── chrome/
│   ├── sidebar.rs / sidebar_view.rs      # 四段侧栏 + 行投影
│   ├── tab_bar.rs / tab_bar_view.rs      # tab 行 + 活动灯
│   ├── status_bar.rs                     # 左摘要 + 右 attention 徽章
│   ├── pane_grid.rs / pane_view.rs       # ClientLayout → 递归分割 / 常驻 Surface 绘制
│   ├── panel.rs / panel_view.rs          # Cmd-P 统一面板（Workspaces/Attention/Search）
│   ├── composer.rs                       # Cmd-I 独立输入框浮层
│   ├── settings.rs                       # Cmd-, 设置窗口（manifest 驱动）
│   └── target_editor.rs                  # 新建 / 编辑 Project
└── terminal/
    ├── grid.rs     # CellGrid：cell / SGR / scrollback / 宽字符（纯逻辑，带测试）
    ├── parser.rs   # vte::Perform 实现 + 视口滚动（纯逻辑，带测试）
    ├── mouse.rs    # 鼠标模式与 X10 / SGR 编码
    └── keymap.rs   # Keystroke → PTY 字节（含 bracketed paste / IME 占位符过滤）
```

`src/main.rs` 仍是唯一 binary。gpui 前端通过 `muxterm gpui` 子命令进入，
`.app` 由 `scripts/build-gpui-macos.sh` 组装（§7）。

## 5. PaneSurface

一个已打开 pane 恰好一个常驻 `CellGrid`，key 是 `(WorkspaceId, PaneId)`。
新建时一次性 seed，之后增量 feed 原始字节；切 tab / 切 workspace 只切可见性，不重建、不 reset
（符合 [`SURFACE.md`](SURFACE.md) §3 定律 1、3、10）。

CellGrid 实现的 VT 范围（每一项都有 `terminal/` 里的字节级测试）：

- 打印字符（UTF-8）；东亚宽字符占 2 格、第二格是 `FILLER`（`\0`），右边缘不劈开而是换行
- 零宽字符直接丢弃（`Cell` 只存一个 `char`，与 SwiftTerm 的 grapheme cluster 是有意分歧，代码注释说明）
- C0：`BS HT LF CR BEL`
- CSI：`A B C D E F G H f J K L M P @ X S T r d s u`、`m`（SGR）、`h/l`
- 私有模式：`25`（光标）、`47/1049`（alternate screen）、`1000/1002/1003`（鼠标）、`1006`（SGR 鼠标）、`2004`（bracketed paste）
- SGR：`0 1 2 3 4 7 8 9`、`30-37 / 40-47`、`90-97 / 100-107`、`38;5;n`、`48;5;n`、`38;2;r;g;b`、`48;2;r;g;b`、`39 / 49`

SGR 属性不是只解析、不画：绘制 run 的 key 是「颜色 + 字体变体」，粗体 / 斜体 /
下划线 / 删除线分别落到 gpui 的 `font_weight(BOLD)` / `italic()` / `underline()` /
`line_through()`。颜色侧对齐 SwiftTerm 1.15（macOS 前端的事实标准）：

| 属性 | 规则 | 对照 |
|---|---|---|
| 粗体提亮 | `Color::Indexed(i)` 且 `i < 7` → `i + 8`（7 是 SwiftTerm 源码里的例外；8–15 已经是高亮色；Truecolor / 默认色不变） | `AppleTerminalView.mapColor(isBold:)` |
| dim（SGR 2） | 前景与背景逐通道 `(fg + bg) * 0.5`，alpha 保持前景 | `NSColor.dimmedColor(towards:)` |
| 反显（SGR 7） | 先交换前景/背景，再对交换后的前景做粗体提亮 | `getAttributes` 先 `swap` 再 `mapColor(isFg:)` |
| 隐藏（SGR 8） | 保留格子宽度与背景，字形换成空格 | xterm 语义（SwiftTerm 忽略该属性，这里按规范隐藏） |

模块单测覆盖切段与颜色（`chrome/pane_view.rs`）、提亮/dim 数学（`theme.rs`）；
`gpui_chrome` 另外断言这些属性真的进了渲染树（`pane-bold` / `pane-italic` /
`pane-underline` / `pane-strike`），普通文本不会误带样式。

### 5.0 制表符与方块：按格子几何绘制

字体里的 `─│┌┐└┘` 各自带留白，按「列号 × 格宽」摆出去必然断线、与宽字符错位；
`█▀▄▌▐` 与八分块在多数等宽字体里宽度也不一致。iTerm2
（`iTermBoxDrawingRenderer`）与 Ghostty 都把这些码点换成几何图形，gpui 走同一条路
（`model/box_drawing.rs`）：

| 范围 | 处理 |
|---|---|
| `U+2500–U+257F` | 四条臂（无/细/粗/双线）画成矩形，端点贴格边，相邻格自然接上；粗细 = 行高 8%（≥1px），粗线翻倍，双线两条细线夹缝 |
| `U+2580–U+259F` | 半块、1/8 块、左/右八分块、四象限：按 1/8 网格填矩形 |
| `U+2591–U+2593` | `░▒▓` 用 25/50/75% 透明度把前景色叠在格子背景上 |

`shapes(ch, cell)` 是纯函数，单测逐条断言「横线左右都贴格边」「竖线到中线」
「双线有缝」「1/8 块底边对齐格底」「四象限拼起来等于整格」。
绘制入口在 `pane_view::paint_box_glyphs`，字形层跳过这些码点（`is_custom`），
所以表格边框在中文/日文/emoji 旁边也不会漂。

光标也按 iTerm2 的观感画：整格填主题 `cursor` 色，字形用格子背景色
（只做前景/背景交换的话，彩色格子上光标会和文字同色）。

### 5.1.1 启动早期的 PaneOutput 不能丢

本地 shell 可能在 `workspace_list` 回来之前就开始输出。`drain_render_events`
过去只按 `view.panes` 遍历，这段输出永远没人取 —— 表现就是「终端一片空白，
进程明明在跑」。现在 drain 的范围是「拓扑里的 pane ∪ 有排队事件的 pane」
（`ViewStore::render_mailbox_pane_ids`），`gpui_chrome` 的
`early_pane_output_reaches_the_surface` 固定这条回归。
- ESC：`7 8 M c D E`
- OSC：`0 / 2` 标题，其余忽略（133 标记由 Core 处理，前端不消费）

### 5.1 视口滚动

滚轮不改网格，只移视口：`Surface::scroll_offset` 决定 `visible_row(row)` 取 scrollback 还是
当前屏。这样 live alternate screen 与光标位置都不被抢走（`SURFACE.md` 定律）。滚动时 pane 不画光标。

滚轮路由优先级与 macOS `TerminalView.scrollWheel` 一致：
`ServerScroll > ApplicationMouse(mouse_reporting && !shift) > ApplicationArrows(alternate_screen) > LocalHistory`。
应用鼠标路径恒用 SGR 编码；方向键回落用裸 `CSI A` / `CSI B`。

`has_server_scroll` 取 `support()` 里的 `ServerScroll` 能力（`GpuiApp::workspace_supports`
懒加载 Core 的 provider 元数据），**不按 runtime 名字分支**。命中时发
`ClientTask::ScrollPane { pane_id, lines }` 交给 Runtime：Herdr 的历史在服务端，前端
本来就没有可滚的 scrollback。tmux / shell 没有这个能力，继续走 `Surface::scroll_offset`
的本地历史（tmux 的 attach 历史由 Core `PaneHistory` 回填）。

### 5.2 宽字符与选词

`model/word_selection.rs` 移植了 macOS `ProgressiveWordSelection` 的三级双击扩展
（Identifier → Word → Path），`WORD_EXTRAS = -.*?+%#@=`，`PATH_EXTRAS = /\~:`。

### 5.2.1 终端选区与复制

`model/selection.rs` 是网格选区：`CellPos`（视口坐标）+ `Kind`（Simple / Word / Line）。
语义对齐 iTerm2 / Ghostty 的默认行为：

| 操作 | 行为 |
| --- | --- |
| 拖选 | 按阅读顺序取区间；跨行时中间行整行选中 |
| 双击 | 走 `word_selection` 三级扩展（Identifier → Word → Path） |
| 三击 | 整行 |
| 单击不拖 | 不算选区（松手即清，`Cmd-C` 不写剪贴板） |
| 拖选松手 | 立即写系统剪贴板（copy-on-select，与 iTerm2 的「Copy to pasteboard on selection」同义） |
| `Cmd-C` / Edit → Copy | 选区文本进系统剪贴板 |
| 右键 | 本地菜单：Copy（无选区时置灰）/ Paste / Select All；不再转发给应用 |

渲染：选中的格子背景按 `SELECTION_ALPHA = 0.3` 叠一层 `theme.accent`，前景不变（反显与
光标格仍然是光标语义优先）。选区里的**尾随空格也要画**，否则选中一片空白看不到高亮。
坐标换算用 `pane_cell_at(rect, origin, cell_width, cell_height, position)`：鼠标事件是
窗口坐标，必须先减掉侧栏宽度与顶部 bar 高度。

生命周期：滚轮进历史、切 pane、切 workspace 都会清掉选区 —— 选区是视口坐标，滚走以后
高亮会盖到别的内容上。松开鼠标不清，方便接着 `Cmd-C`。

复制文本：逐行去掉尾部空白，行之间 `\n` 连接；宽字符占位格（`\0`）跳过。

### 5.2.2 应用鼠标上报与悬停

终端里的 vim / htop / tmux copy-mode 通过 DECSET 9 / 1000 / 1002 / 1003 要鼠标。
`terminal/mouse.rs` 负责协议与编码，`GpuiApp::report_mouse` 负责路由：

| 模式 | 上报 |
| --- | --- |
| 9（X10） | 只报按下 |
| 1000（Normal） | 按下 / 释放 / 滚轮 |
| 1002（Button） | 1000 + 按住拖动 |
| 1003（Any） | 1002 + 无按键悬停 |

1006 只决定编码：开了就一律 SGR（`CSI < code ; col ; row M/m`），否则退回经典
`CSI M` 三字节。修饰键按 xterm 的 4（Shift）/ 8（Alt）/ 16（Control）加在事件码上；
Command 不转发，留给本机链接与快捷键。

规则：

- **Shift / Option 是逃逸通道**：应用要鼠标时按住 Shift（xterm）或 Option（iTerm2）
  仍走本地选字；`pi` 这类 TUI 会打开 1002/1003，靠它才能选中文本。没上报的按下
  不补释放：`mouse_press` 只记已上报的键。
- **移动按格去重**：`mouse_motion_cell` 只在跨格时才发 motion（xterm 的 1002/1003
  语义）；触控板的像素级移动否则会把一次拖动拆成几百条命令。离开 pane 时清缓存，
  再进来同一格可以重新上报。
- **中键**：键号 1（左键 0），按下与释放都走同一张表。**右键不转发**：本机
  右键菜单（复制 / 粘贴 / 全选）优先，用户不必先记快捷键。
- **拖出 pane 再松手**也必须发释放：`on_mouse_up` 只在指针还在元素上时触发，
  所以另挂 `on_mouse_up_out`。
- 坐标一律经 `pane_cell_at(rect, origin, …)`：滚轮与标题条 hover 都必须减掉
  内容区原点（侧栏 / 顶部 bar）**和这一块 pane 自己的 `rect`**。标题条只在
  鼠标进入该 pane 顶部 `TITLE_BAR_HEIGHT`（22pt）时显示。
- 应用没接管（模式关闭或 Shift/Option）时，左键回到本地选区；Command+点击链接
  优先于鼠标上报（vim / tmux 开着鼠标时链接仍要能点开）。

### 5.3 输入法

`GpuiApp` 实现 `EntityInputHandler`：`set_marked_text` 存组合串、`commit_text` 剥占位符后写 PTY、
`caret_rect` 用 `model/ime.rs` 的 `candidate_anchor` 把格坐标换成屏幕矩形（末列换行到上一行末尾）。
组合中的文本画在光标处。

按键路由（对照 macOS `MuxTerminalView.keyDown`）：

| 按键 | 去向 |
| --- | --- |
| 无修饰键的可打印文本（`n` / `i` / 空格 / 已提交中文） | **让给平台输入法**，不编码、不消费；文本经 `insertText` → `replace_text_in_range` → `commit_text` 回来 |
| 控制键 / 功能键 / Alt、Ctrl 组合 | 直接 `encode` 后消费 |
| IME 占位符（U+FFFF/U+FFFE） | 丢弃并消费，一个字节都不发 |

为什么不能先编码再消费：拼音首字母也是普通可打印字符，一旦前端把它写进 PTY 并
`stop_propagation()`，macOS 的 `inputContext` 收不到这次 keyDown，组词永远起不来。
判定在 `terminal/keymap.rs::defers_to_ime`，纯函数有单测。

只有**聚焦的 pane** 注册 `ElementInputHandler`：所有 pane 共用一个 focus handle，
若每个 pane 都注册，平台只保留最后绘制的那个，候选窗会锚到错误的 pane。

### 5.4 pane / workspace 退出

Core 的 `PaneClosed` / `TabClosed` / `BackendStatusChanged` 到达后，前端按下面的顺序收敛：

1. `reconcile_closed_panes`：拓扑与渲染队列里都没有的 pane，丢掉它的 Surface、选区、`⋯` 菜单。
2. `reconcile_workspace_lifecycle`（对照 Swift `maybeCloseIfSessionEnded()`）：
   - 本机 shell 空（最后一个 pane 退出）→ 只发一次 `NewTab`，窗口不留空白；
   - 其他 runtime（tmux / herdr / ssh）后端 `Exited` → 收起这个 workspace（侧栏行、tab、Surface 一起走），
     切到邻近还有 pane 的 workspace；
   - 一个活的 workspace 都没有 → 请求关窗口（`start_pump` 里 `cx.quit()`），不能停在冻住的一帧。

首帧还没回来（`visible == None`）时不触发退出，否则 tmux socket 连不上会一启动就关窗。

### 5.5 图标与菜单栏

图标统一在 `chrome/icons.rs`：24×24 设计网格 + `PathBuilder` 描边/填充，
按目标尺寸等比缩放。

为什么不用 emoji / 字体字形：`👁`、`🚫`、`⚙` 是彩色字体，字号一变就跳基线，
而且各家 fallback 字体粗细不一，看起来像贴纸。也不走 `NSImage(systemSymbolName:)`：
那是 macOS 独占，Linux / Windows 没有。侧栏 marker、尾部按钮、状态栏按钮、
tab 活动标记、panel 标记列、`⋯` 全部走同一套矢量图标；未知 symbol 也映射到
可画图形，绝不回退成 emoji。

菜单栏（`model/menu.rs` → `platform/macos_menu.rs`）：

- **每一项都有动作**。早期版本里有 `action == None` 的灰条（About / Check for
  Updates / Select All / Rename Tab…），用户点了没反应；现在这些全部接上
  （`open_info`、Core `update_check`、`select_all`、改名浮层、`BreakPane`、
  tab 栏位置等）。
- 条目携带 `checked` / `enabled`：`Tab Bar on Top/Bottom` 打勾互斥，没有选区
  时 Copy 置灰。原生菜单 `setAutoenablesItems(false)`，否则 AppKit 会覆盖状态。
- 状态变化用 `refresh_menu` **原地**改 item 的 state / enabled，不重建 NSMenu ——
  重建会换掉 `watch_menu` 的 channel，菜单点击就断了。
- 文案走 i18n；测试断言每条菜单文案都有 key，且中文目录有翻译。

### 5.5.1 快捷键以 **Colemak 手位**为基准（QWERTY 同手位）

用户报障（2026-11-16）：「快捷键 cmd-d / cmd-t / cmd-s 都失败了」，随后明确要求：
**所有默认快捷键都用同手位**。这台机器的输入源是 **Colemak**，它把字母挪了位置：

| 物理键 | Colemak 标签 | QWERTY 标签 | 动作 |
| --- | --- | --- | --- |
| 左手中指内侧 | `⌘D` | `⌘G` | 左右分屏 |
| 左手无名指 | `⌘S` | `⌘D` | 上下分屏 |
| 左手食指内侧 | `⌘T` | `⌘F` | 新建标签 |

结论：**快捷键属于手指位置，不属于字母**。所以预设按 Colemak 标签书写，
解析时按物理键位（`keyCode`）匹配；切到 QWERTY 时同一个物理键仍然执行同一个
动作，只是菜单和设置里显示 `⌘G / ⌘D / ⌘F`。

现在的规则（`model/physical_key.rs` + `model/shortcut_editor.rs` +
`platform/macos_keys.rs`）：

1. 本地事件监视器 `NSEvent addLocalMonitorForEventsMatchingMask:` 在窗口 /
   输入法之前拿到真正的 keyDown，记下 `keyCode` 与布局字符，**事件原样放行**
   —— 只记录，不改变派发，输入法与文本编辑行为一律不动。
2. 物理键位是**唯一权威**：`keyCode` → Colemak 字符 → 动作。查不到就是没绑定，
   **绝不**回落到布局字符去找一个别的动作（否则 Colemak 的 `⌘S` 会二次命中
   「上下分屏」）。只有没有物理键的按键（方向键、中文输入法给的 `【】`）才走
   字符匹配。
3. `ShortcutMap` 是唯一事实源：预设（Colemak 标签）、用户 override（按预设的
   同一套标签存储）、菜单 key equivalent、设置列表、实际派发全部用它。
   `[shortcuts] preset` 从 `colemak` 切到 `qwerty`（或反向）时，草稿里的
   override 键标签会被重写，物理手位保持不变。
4. 普通文字输入永远不经过快捷键表；它只由 IME / 布局决定。

为什么不用 `NSApp.currentEvent` 取 keyCode：实测在视图的
`performKeyEquivalent:` 回调里它指向 `NSEventTypeAppKitDefined`（type 13），
`keyCode` 会抛 `NSInternalInconsistencyException`；gpui 的 `Keystroke` 又只有
字符没有键码。所以只能靠事件监视器记录。

业界对照：Ghostty 用 W3C 物理键码派发快捷键（`ghostty-org/ghostty#7320`），
Zed 明确「先派发快捷键、再交给输入法」（`zed.dev/blog/keyboard-localization`）。

回归测试：`physical_key::tests::*`（三组映射表）、
`shortcut_editor::tests::colemak_is_the_anchor_for_physical_positions`、
`shortcut_editor::tests::qwerty_labels_move_but_the_hand_position_does_not`、
`shortcuts::tests::physical_anchor_never_falls_back_to_a_conflicting_layout_action`、
`tests/gpui_native_shortcuts.rs`（74 条真实 NSEvent 经生产监视器各派发一次）。

### 5.5.2 设置是**独立窗口**，快捷键可编辑

设置不再叠在终端上：主窗口不画遮罩、不变暗，`chrome/settings_window.rs` 开一个
独立 `gpui` 窗口（`— Settings` 标题后缀用于识别），两个窗口共享同一个
`GpuiApp` / `CommandQueue` / `EventPump`，不建第二个 Core handle。

- 页面：200pt 图标侧栏 + 可滚动分组卡片 + 固定底栏（Cancel / Apply）。
- Projects：占满页宽的管理器，新建 / 点行编辑 / 删除，编辑只改草稿、不打开
  workspace；行内显示 `runtime · transport · path`。
- Shortcuts：搜索、录制（必须带修饰键、冲突拒绝）、禁用、恢复；每行并排显示
  Colemak 与 QWERTY 两套标签，并写明「预设只改标签，手位不变」。
- Apply 是**单笔原子 Core 事务**（`ConfigApplyTransaction` + RFC6902 patches）：
  收到成功回执前不关窗口、不清草稿；失败保留草稿并显示错误。
- 录制态绝不执行动作；设置窗口打开时主窗口不处理按键；关设置不关主窗口。

回归测试：`tests/support/gpui_settings.rs`（8 条：全宽 Projects、草稿编辑、
创建/删除、录制/禁用/恢复/取消、双列标签、冲突拒绝、Apply 失败保留、窗口生命周期）、
`tests/gpui_settings_native.rs`（真实多窗口，**需 `MUXTERM_NATIVE_UI_TESTS=1`**）。

真机 GUI 测试纪律（`AGENTS.md` 同步）：默认 `cargo test` 不开窗口；测试里禁止
`cx.activate(true)`；禁止循环 `screencapture` / `window.refresh()`；只截本进程窗口。

### 5.6 侧栏几何与 tab 栏策略

侧栏的可变部分全部是纯模型（`model/sidebar_layout.rs`），渲染不自己算：

- 宽度：默认 240pt，`SIDEBAR_MIN_WIDTH=200` / `SIDEBAR_MAX_WIDTH=480`，
  并且永远给正文留 320pt（对照 `MainWindow.swift` 的
  `sidebarItem.minimumThickness/maximumThickness` 与 `setPosition(240)`）。
  `clamp_sidebar_width` 是**唯一**入口：`app.rs` 算正文 origin 用它，
  `chrome/sidebar.rs` 画侧栏也用它，两处口径不能分叉。
- 分区：对齐 VS Code 侧边栏 —— 四段按顺序排在同一列，各自只占内容的自然高度；
  **折叠 = 只留 26pt 段头**，下一段立刻跟上来；整栏只有 `sidebar-scroll` 一个
  滚动区。曾经按 3:2:1 切固定高度、把 Hidden Commands 钉在整栏底部：折叠的段
  照样占一块空白（看起来没生效），钉底的那段被窗口下沿裁掉（`HIDDEN COMMANDS`
  被遮盖的报障截图）。
- 拖动：`sidebar-divider` 是右侧 5pt 的热区，按下记
  `sidebar_drag_offset`；一旦开始拖，`sidebar-drag-overlay` 铺满窗口接管
  move/up —— gpui 的 `on_mouse_move` 只在 hitbox 被 hover 时触发，鼠标离开
  5pt 热区就会跟丢。

tab 栏宽度策略来自 Core 配置 `[ui] tab_bar_style`（`equal_width` 缺省 /
`fixed` / `compact`）。等宽时 tab 带自己 `flex_1` 吃掉中间所有空间、每个 tab
再 `flex_1` 平分，对照 Swift `StatusBarView.fillsViewport` 的
`distribution = .fillEqually` + trailing constraint。tab 的颜色跟着活动状态走
（working / blocked / done 用状态色，idle 用 muted），不只在活动 tab 上画。

### 5.7 绘制开销与后台输出

2026-11-17 用户报障 CPU 持续约 100%。只读采样实际 app（PID 31493，
15:14:40 +08:00，3 秒/10ms）显示主线程热点为 `paint_text` 经
`paint_glyph → Scene::insert_primitive → BoundsTree::find_max_ordering`。
字形不是排版错了，而是每个字形单独插入绘制顺序树，反复计算重叠边界。

- 字形按**一行一个 `paint_layer`**提交，不改列坐标、字体或格子尺寸。
  与本机 gpui 0.2.2 的 `text_system/line.rs::paint_line` 用法一致。
- `drain_render_events` 始终更新每个常驻 Surface，但只有当前 workspace / tab
  的可见 pane 输出才使终端变脏；后台仍保留最新画面，切过去无需重灌。
- 布局尚未到达的首屏输出仍触发刷新，不能回退为「启动时漏输出」。

回归：`background_workspace_output_does_not_dirty_the_terminal`、
`background_tab_output_does_not_dirty_the_terminal`、`idle_pump_is_not_dirty`、
`early_pane_output_reaches_the_surface`。无头回归不能代替新 app 的 CPU 实测；
不为验证而自动启动/激活用户机器上的窗口。

真机体感与 CPU 只看 `--release` 产物：debug 版的字形排版/绘制慢数倍，
2026-11-17 采样到的 100% CPU 就跑在 debug 的 `build/macos/MuxtermGpui.app` 上
（当时 GPUI 开发包还带 `Gpui` 后缀；2026-11-18 起产物统一叫 `Muxterm.app`）。

在线核对（2026-11-17T15:19:41+08:00）：Zed 官方
[`gpui/text_system/line.rs`](https://github.com/zed-industries/zed/blob/main/crates/gpui/src/text_system/line.rs)
与 [`gpui/scene.rs`](https://github.com/zed-industries/zed/blob/main/crates/gpui/src/scene.rs)
确认按层批量提交的意图；具体 API 签名以本地锁定的 0.2.2 源码为准。

### 5.8 切 pane 只改光影

用户报障：「切换 pane 的时候会出现 pane 的变化一下…应该只有光影的明暗变换才对」。
根因是活动 pane 用 `border_1` 描边：gpui/taffy 的边框挤进内容盒，终端画布
整体位移 1pt、可见列数也跟着变。现在强调色描边画在绝对定位的覆盖层
（`pane-focus-ring`），不占布局；内容盒几何（`pane-text-{pane_id}`）在焦点
切换前后逐像素相同，非活动 pane 仍按 `INACTIVE_PANE_DIM` 压暗。

回归：`switching_focus_only_changes_the_lighting`。

### 5.9 Command+点击打开链接

用户要求：「浏览器 cmd+点击可以打开才行」。命中顺序与 Linux
`terminal/pane_view.rs::open_url_at` 一致：先看格子的 OSC 8 超链接，再用行内
`https?://` 正则（`model/links.rs`，纯函数，尾随标点与不配对括号会被剥掉）；
命中就交给 `cx.open_url`，没命中时仍然是拖选。

悬停反馈：按住 Command 时，指针下的链接**整条**画强调色下划线、指针变成小手
（`hover_link_span` → `row_runs` 在链接左右边界断开，只给链接本身加
`hover_underline`，不碰终端自己的 SGR）。修饰键变化本身也会刷新反馈：用户是
「先按住 Cmd，再悬停」，不能只等鼠标移动。

OSC 8 的 URI 存在 `Grid` 的链接表里，格子只存 `u16` 下标（滚回行也能开）；
链接表只增不删，避免历史格的旧下标指向错行。

回归：`cmd_click_opens_a_url_from_the_terminal`、
`dragging_over_a_url_still_selects_text`、`command_hover_marks_the_link_under_the_pointer`、
`hovering_a_link_underlines_only_the_link`、`model::links` 6 条纯函数测试、
`parser` 的 OSC 8 与纯文本两条。

## 6. 数据流与线程

```text
Core FFI ──poll_workspace_events──▶ EventPump（唯一消费者，主线程）
                                      │ apply
                                      ▼
                                   ViewStore
                                      │ 投影（owned）
                                      ▼
                          GpuiApp（Entity，主线程）──notify──▶ 重绘
                                      │
  键盘 / 点击 / 尺寸 ──CommandQueue──▶ flush ──▶ Core
```

v1 的事件泵跑在主线程的 gpui 异步任务里，每 8–16ms 收一次批次。这样 C handle 全程只被一个线程持有，
不需要主线程桥，也不需要锁。已知代价是：单次 poll 若卡住会掉帧。后续把泵移到专职线程时，
只需把 `bridge.rs` 的通道方向反过来，`ViewStore` 不出主线程的约束不变。

## 7. 构建与打包

```bash
cargo build --no-default-features --features gpui --release
./build/macos/muxterm gpui                     # 直接跑
./scripts/build-gpui-macos.sh --release        # 产出 build/macos/Muxterm.app
```

`gpui` feature = `["dep:gpui", "ffi"]`，默认不启用，现有 GTK / TUI / FFI 构建完全不受影响。

`.app` 结构与 Swift 前端一致：`Contents/MacOS/Muxterm` 是一个 exec 脚本，转发到同 bundle 内的
`muxterm gpui`。这样做是因为项目规则禁止第二个 frontend binary，而 LaunchServices 不能传参。
LaunchServices 会丢弃 stderr，所以启动脚本显式传 `--log-file ${TMPDIR}/muxterm-logs/muxterm-<日期>.log`；
开发包再加 `--debug`。报障时先看这个文件，不要只看（可能为空的）日志目录。
Bundle 可见名称与版本号规则同 Swift：release 用 `dev.muxterm.app`，debug 用 commit 后缀，
避免开发包和历史安装互相抢 LaunchServices 身份。

## 8. 已知缺口（v1 不含）

| 缺口 | 影响 | 后续 |
| --- | --- | --- |
| 提交帧绘制（committed paint） | 没有「已提交但未确认」的第二层 | 按 `terminal.md` 缺口表接 |
| 逐 pane 颜色上报 | Core 的 pane colours 仍取默认 | 接 `ClientCommand::PaneColours` |
| 无障碍树 | 与现状持平（现状也没有） | gpui 已依赖 accesskit，接上即净增 |
| sixel / 图片协议 | 无 | 按需 |

已完成（v1 缺口表里曾经列过的）：输入法 marked text 与候选窗定位、滚轮历史滚动、
设置页、快速面板、鼠标事件上报、宽字符宽度、Cmd-Shift-P 命令面板、Core 驱动的
浅色/深色主题（默认白底黑字 + Menlo 18）、按真实字形度量格子、窗口级
`refresh-client -C` 上报、attach 历史回填（Core `PaneHistory` → 本地 scrollback）、
Cmd-P 面板列出现有 runtime（discovery 驱动，点开即 attach）、
Select All / 右键复制菜单 / 选中即复制、链接 Command+悬停下划线、macOS 原生菜单栏
（NSMenu，结构照抄 Swift `buildMenu`）、快速面板按 Swift `loadWorkspaceItems` 的
根列表分段（Shells / Agents / Open / 编号 workspace / Project / discovery）、
未保存项的 Save 与 Cmd-K 行内动作菜单、`Open` 分步导航 + 末端「运行时 + 名字」确认、
命令面板的 `Muxterm Info` 浮层（version / commit / 调试日志路径 / GitHub issue，
数据走 Core `muxterm_build_info_json`）、提示符末尾光标（尾部空白裁剪不再吃掉光标格）、
本地 `~` 路径展开（目录列表与 `create` 都用绝对路径；SSH 仍交给远端展开）、
命令面板里所有条目都有真实动作（Local / SSH 导航、改名浮层、tab 重排、语言子菜单、
主题切换、状态栏模式）与对应 Core 任务（`rename_tab` / `rename_workspace` / `move_tab`）、
SGR 字体属性真正参与绘制（粗体 / 斜体 / 下划线 / 删除线，粗体按 SwiftTerm 规则提亮
ANSI 0–6，dim 与背景对半混合）

### 分屏拖拽带、pane 标题条、图片粘贴

- 分屏之间留 6pt（`pane_grid::DIVIDER_LENGTH`，对照 `PaneTitleBarGeometry.dividerLength`），
  露出的底色是 `chrome_bg`（= `NSColor.windowBackgroundColor`）。之前 pane 直接相邻，
  比 Swift 挤。
- pane 标题条：`model::pane_title` 移植 `PaneTitleLayoutPolicy` / `PaneTitleBarGeometry` —
  只在可见 pane > 1 且鼠标停在 pane 顶部 22pt 时出现（Swift `updateTitleHover`），
  `reservedHeight` 恒 0（浮在 Surface 上，不改变 cols/rows），空标题退回 `Pane @<id>`。
- pane `⋯` 菜单：`model::pane_menu` 照 `PaneTitleBarView.configureActionMenu()` 铺条目
  （两个分割 / 移动 / 全屏 / 循环布局 / 关闭）；菜单**绑定到被点的 pane**，
  执行前先 `focus_pane` 再 `run_action`（Swift 的 `onActivate?()`），
  所以不会出现「点 A 的分屏、结果 B 被切开」。键盘 ↑↓ / ⏎ / Esc 也走这条路。
- 图片粘贴**只调 Core**：前端读剪贴板 → 转 PNG → `start_image_paste` →
  每个 tick `poll_image_paste` 取路径。前端不拼 `scp`、不起进程；阶段由
  `model/image_paste::ImagePasteTimeline` 驱动横幅（编码 / 上传 / 成功 / 失败）。
  macOS 剪贴板几乎都是 TIFF（截图、Preview、浏览器），所以
  `platform::clipboard_image::to_png` 用 AppKit 的 `NSBitmapImageRep` 先转码；
  转不了码、没有焦点 pane、没有 Core 连接**都会**出失败横幅 —— 静默失败是 bug。
- 结果一到手就**停止轮询** Core（`ImagePasteTimeline::awaits_core`）：
  `poll_image_paste` 是 take 语义，成功结果挂着等 `MINIMUM_VISIBLE` 时再问
  会回 `no image paste in progress`，把成功改写成失败（用户报障：文件其实
  已经生成）。结果阶段（成功/失败）的横幅带可见关闭按钮
  （`image-paste-dismiss` → `dismiss_image_paste`），进度阶段不给 ——
  上传未完成时关掉会让人以为已取消。
  回归：`a_landed_result_stops_asking_core`、
  `the_image_paste_result_banner_can_be_dismissed`、
  `the_image_paste_progress_banner_has_no_close_button`。
- 指示色对齐 Swift 语义色：`working` = systemYellow、`blocked` = systemPink、
  `done` = systemTeal（之前借用 success/danger/warning，侧栏与 tab 看起来偏红偏绿）。
- 普通查询词匹配**所有**字段（名字 / runtime / 传输 / 路径 / session / socket /
  workspace id）—— 与 Swift 的 `QuickConnectStore` 一致；命中名字的加权更高，
  所以搜 `muxterm` 时本机 tmux session 排在第一，不会被泛 `tmux` 命中淹掉。
  `@tmux` / `@local` / `@<alias>` 仍是显式过滤。
- Search tab 直接调 Core 的 `search_all`（同步 FFI），打开 tab 或改动查询就重跑；
  没有 Core 时保留上一份快照，不会因为一次按键清空。
- 窗口标题由 `[ui] show_title_bar` 控制：开 = `workspace — Muxterm`，
  关 = 空串（只剩 traffic lights，与 Swift 版观感一致）。
- Attention / Search 的跳转是**完整身份**：workspace → tab（按 pane 反查）→
  显式 `SwitchPane`。只切 workspace + pane 的话，pane 在别的 tab 时屏幕还停在
  原 tab —— 用户看到的就是「跳不到位」（对照 `MainWindow.jumpToPane`）。
- 状态栏右侧读 **Surface 的实时网格**（`pane_grid_size`），不读 store 里等 Core
  回包才更新的 `cols/rows`；窗口一变大，状态栏立刻跟着变。
- 本机 tmux 去重先核对 runtime / transport / socket：同名的 shell workspace 或
  同名 SSH session 不会把本机 tmux session 当成「已打开」过滤掉。
  面板打开时会重新 discovery，先发本机结果再补远端。
- 两个固定聚合槽另有**专属身份色**：Shells = teal、Agents = systemPurple
  （`model::aggregate::AggregateIdentity`）。聚合行是入口，不是连接状态，
  所以永远不借用 blocked/done 的粉与青绿，侧栏行与聚合页 tab 用同一个色。
、终端字形按**列坐标**绘制（`paint_text` 只借用
`shape_line` 的 glyph / font id，位置由 `列号 × 格子宽` 决定；中文回退字体、粗斜体、
emoji 都不会推开同一行后面的竖线）、设置浮层与 Swift 同构（680×680、200pt 左栏、
搜索框、分类/字段、52pt 底部条：dirty 摘要 + Cancel + Apply；Esc 取消、⏎ 应用、
Tab 换分类、↑↓ 换字段、←→/空格改开关与枚举；有改动先确认再关）、
Project / discovery 行用 Core 记账的 attach 频率排序（`attach-history.json`，保留天数读
`[quick_panel].attach_history_days`）；**已打开的 workspace 不参与频次排序**，
行首编号、列表顺序与侧栏 / `Cmd-Ctrl-1..9` 完全一致。

### 8.2 快速面板与 Swift 的差异

`src/frontend/gpui/model/workspace_picker.rs` 是纯状态机（阶段、草案、Project 文档）；
`state/picker.rs` 接 FFI；`chrome/panel.rs` / `panel_view.rs` 负责像素与动作集。
对照 `UnifiedPanelController.swift` 的已知差异，都是缺 Core 能力，不是设计选择：

| 差异 | Swift | gpui 现状 |
| --- | --- | --- |
| Project 排序 | `store.recents + store.projects` 按 attach 次数（`WorkspacePickerOrder`） | 同一份规则，数据来自 Core（`attach-history.json` → `muxterm_workspace_usage_json`）；活跃项固定第一，其余次数降序、最近使用降序，无记录项保持配置顺序。已打开的 workspace 不参与：它们按打开顺序排，与行首编号一致 |
| 行内 `Update runtime` | workspace 行在 `canUpdateRuntime` 时给 Update | 没有，Core 侧 handoff 还没接进 gpui |
| 提醒行的 `Show` | 已隐藏时给 `Show` | 只给 `Hide`；侧栏 Hidden commands 段可反向切回 |
| `Open preview` | 提醒行可开预览 | 没有预览窗口 |
| `Cmd-K` 选中项 | 全局命令（Action Catalog）在独立面板 | 同一份行内动作，但不带全局命令 |
| 本地化 | i18n 全量 | 命令面板 / 改名浮层 / 侧栏段头走 `i18n` catalog；其余 chrome 文案仍硬编码英文 |
| 终端选区复制 | 有 | 有：拖选 / 双击选词 / 三击整行 / `Cmd-C`（§5.2.1）；应用鼠标上报见 §5.2.2 |
| 活动 / Agents | Core 推送 + 轮询 | 每秒拉一次 `muxterm_attention_snapshot`，按 replica id join 到产品 workspace id |
| 命令刻度轨 | 有（`CommandMarkRail`） | 未移植（`docs/gpui-parity/tests.md` §5.2 缺口表） |
| `Muxterm Info` | 独立小窗口（`MuxtermInfoWindowController`） | 同内容的前端浮层，Esc 关闭 |
| 现有 runtime 去重键 | `transport\|runtime\|session\|socket\|workspace_id\|name` | `(transport, target, runtime, session, socket, workspace_id)`：与 Swift 同语义，不按 name 去重（同 session 多个 Herdr workspace 是不同身份） |


## 8.1 跨平台 TODO（2026-11-05 实测记录）

一份 widget 树是真的：`src/frontend/gpui/` 里 `cfg(target_os)` / `objc` / `winapi` / `gtk::`
出现次数都是 0。但「编到别的机器上就行」目前还不成立，下面每一条都是实测过的：

| 平台 | 现状 | 要做什么 |
| --- | --- | --- |
| Windows | **编不过**。`src/core/transport/mod.rs` 无条件声明 `local` / `ssh`，它们无条件 `use portable_pty`、`libc::SIGHUP`，而 `portable-pty` / `libc` 只在 `[target."cfg(unix)".dependencies]` 下；`connection.rs` 也无条件用 `std::os::unix::net::UnixStream`，`src/core` 里 `cfg(windows)` 出现 0 次。`src/frontend/windows/mod.rs` 只有 4 行占位。 | 把 Unix-only 模块加 `#[cfg(unix)]`，并决定 Windows 的 PTY 后端（ConPTY）与 runtime 范围；在那之前至少用 `compile_error!` 明说「Windows 不支持」，不要把一堆悬空符号抛给用户。 |
| Linux 构建 | **依赖链编不过**：`gpui 0.2.2 → gpui_http_client → zed-async-tar → xattr 0.2.3`，`xattr` 在 `cfg(target_os = "linux")` 分支里用 `libc::ENOATTR`，而 Linux 只有 `ENODATA`。`cargo check --target x86_64-unknown-linux-gnu --features gpui` 停在这里，还没轮到本项目代码。 | 加 `[patch.crates-io] xattr`（或 vendor 一份把 Linux 的 `ENOATTR` 映射到 `ENODATA`）。打完这个补丁后本库可以完整编过 Linux target（实测 2m02s 出 `libmuxterm-*.rmeta`）。 |
| 字体 fallback | 默认 `[font] family` 读 Core 配置（缺省 Menlo 18）。仓库里的 `assets/fonts/JetBrainsMono-Regular.ttf` 没有通过 `TextSystem::add_fonts` 注册；配置的字体不存在时 gpui 会落到它的 fallback 栈（含比例字体），格子对齐开始漂。 | 启动时 `cx.text_system().add_fonts(...)` 注册内置字体，并把 Core 的 `[font] fallback` 接进 `Font.fallbacks`。 |
| 快捷键 | `handle_key` 用 `keystroke.modifiers.platform`：macOS=Command，Linux=Super，Windows=Win。已消费 `[shortcuts] preset` 与 `overrides`（Colemak 手位为锚）。`primary_key` 还没被 gpui 消费。 | 接 `primary_key`（Linux 上 GTK 前端用 Alt），或明确「gpui 前端只支持 macOS 语义」。 |
| 菜单栏 | macOS 走原生 NSMenu（`platform/macos_menu.rs`，结构 = `model/menu.rs`）。Linux / Windows 还没有菜单条（`platform::install_menu()` 返回 `None`）。 | 自绘菜单条：同一份 `model::menu` 数据，用 gpui 元素画在窗口顶部。 |
| CI | `.github/workflows/*.yml` 里 `gpui` 出现 0 次。 | 加一个 `ubuntu-latest` 的 `cargo check --no-default-features --features gpui`。 |
| 打包 | 只有 `scripts/build-gpui-macos.sh`，没有 Linux `.desktop` / Windows 打包。 | 与各平台 frontend 一起补。 |
| 图标 | macOS `.app` 内置 `MuxtermLogo.icns` + `CFBundleIconFile`（与 Swift 同一份资源）。 | Linux / Windows 用同一张 PNG 生成 `.desktop` / `.ico`。 |

## 9. 验收

- `cargo check --no-default-features --features gpui` 通过。
- `cargo test --no-default-features --features gpui --lib` 通过（2026-11-16 实测 **2010 passed**，3 ignored）。
- 集成测试（每个都实测通过）：
  - `--test gpui_chrome`（146，含设置独立窗口 / 双布局快捷键管理 / Cmd-P 面板几何与空状态、命令面板每条命令的动作、改名浮层、tab 重排、语言/主题/状态栏、
    设置页的 Esc / ⏎ / Cancel / Apply 与放弃修改确认（有改动先问；无头测试走同一套渲染兜底）、
    面板按 Core attach 频率排序（活跃项第一）、
    SGR 粗体/斜体/下划线/删除线真的进渲染树、
    侧栏两个固定聚合槽与 `shell` 行隐藏、空本机 shell 只补一次 `NewTab`、
    拼音首字母让给输入法且提交只进一次 PTY、结束的 workspace 收起并切走 / 最后一个结束时请求关窗、
    关掉的 pane 丢掉 Surface 与选区、
    `Cmd-[` / `Cmd-]` 按当前 tab 布局选目标（含 zoom）、面板三条身份去重
    （provenance / canonical target / 名字）、活动快照填充 Agents/Commands、Info 复制按钮、
    空闲 pump 不重绘、侧栏可拖宽度与三段分区、tab 等宽铺满与活动状态色、
    Cmd-V 图片优先粘贴、空输入框光标在占位文案之前）：侧栏 / tab / 状态栏 / Cmd-P 面板（含 discovery 到的现有 runtime）/
    Cmd-Shift-P 命令面板 / 输入框 / 设置 / Open 入口 / 历史回填 / 菜单动作，并断言真实布局
    （侧栏 240pt、24pt 单条栏、命令面板 600×360），全部走真实 `GpuiApp`。
  - `--test gpui_workspace_picker`（13）：根列表四段顺序与编号、Open 三层导航、目录完成后
    的「运行时 + 名字」确认（默认 Herdr + 路径末段）、空名字拒绝、Esc 逐层退回、
    Cmd-K 行内动作集与浮层位置、Save 点击只写 projects（保留 template 等未知字段）、
    已保存/未保存两套动作、Herdr attach 带 `workspace_id`、面板吞键不落 PTY。
  - `--test gpui_surface_events`（6）：滚轮、宽字符、鼠标报告、IME 提交、候选窗锚定。
  - `--test gpui_surface_e2e`（1）：隔离 tmux 里真实 shell 输出落到格子上。
  - `--test gpui_acceptance`（13）：启动 workspace、本机 shell 启动即显示 Shells 聚合
    （侧栏只有两个聚合槽、聚合 tab 带来源）、按键进 shell、Cmd-T 加 tab、Cmd-D 分屏与
    Cmd-[ / Cmd-] 换 pane（焦点跟随）、本机 shell 退出只关自己的 pane、隔离 tmux 会话结束
    收起 workspace 并请求关窗、Rename Tab / Move Tab 走 Core 往返、活动快照 join 到
    前端 workspace id、面板 Open 全流程真的建出 workspace、第二个 workspace 经 Catalog 打开。
  - `--test gpui_acceptance_herdr`（2）：named Herdr session 出现在侧栏，输入有回显。
  - `--test gpui_harness_smoke`（1）：无头 harness 自身可用。
- 空闲开销（2026-11-06 实测，`muxterm gpui -L <隔离 socket>` 空转 6 秒后连采 3 次）：
  轮询固定 8ms 时 **2.9% / 3.0% / 2.9% CPU**，改成「空闲退避到 64ms、有事件立刻回 8ms」
  加「tmux status 无变化不重绘」后 **0.8% / 0.7% / 0.7% CPU**，RSS 89MB 左右不变。
- `muxterm gpui` 打开窗口，侧栏列出 workspace，tab 栏与 pane 网格按 `ClientLayout` 渲染。
- `build/macos/Muxterm.app` 存在、`codesign --verify` 通过、双击可启动。

所有真机测试都用隔离 socket / named session：tmux 走 `-L muxterm-test-*`（清理带同一个 `-L`），
Herdr 走 `muxterm-test-*` session（只 `session stop` + `session delete`）。

## 10. 不做什么

- 不删 Swift / GTK 前端，不把 `.app` 默认入口换掉。
- 不为 gpui 单独做侧边栏或 agent 列表（沿用 `FRONTEND.md` §2.3 的词表）。
- 不在 gpui 侧新建连接池、warm slot、第二事件泵。
- 不把 Index 的 `visible_ansi` 喂进 CellGrid（`SURFACE.md` §3 定律 2、9）。
