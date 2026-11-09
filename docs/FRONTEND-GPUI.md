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
> （`scripts/build-gpui-macos.sh` → `build/macos/MuxtermGpui.app`）；Swift / GTK4
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
- ESC：`7 8 M c D E`
- OSC：`0 / 2` 标题，其余忽略（133 标记由 Core 处理，前端不消费）

### 5.1 视口滚动

滚轮不改网格，只移视口：`Surface::scroll_offset` 决定 `visible_row(row)` 取 scrollback 还是
当前屏。这样 live alternate screen 与光标位置都不被抢走（`SURFACE.md` 定律）。滚动时 pane 不画光标。

滚轮路由优先级与 macOS `TerminalView.scrollWheel` 一致：
`ServerScroll > ApplicationMouse(mouse_reporting && !shift) > ApplicationArrows(alternate_screen) > LocalHistory`。
应用鼠标路径恒用 SGR 编码；方向键回落用裸 `CSI A` / `CSI B`；服务端滚动路径在本前端是 no-op
（gpui 还没有服务端滚动通道，`has_server_scroll` 恒 false）。

### 5.2 宽字符与选词

`model/word_selection.rs` 移植了 macOS `ProgressiveWordSelection` 的三级双击扩展
（Identifier → Word → Path），`WORD_EXTRAS = -.*?+%#@=`，`PATH_EXTRAS = /\~:`。

### 5.3 输入法

`GpuiApp` 实现 `EntityInputHandler`：`set_marked_text` 存组合串、`commit_text` 剥占位符后写 PTY、
`caret_rect` 用 `model/ime.rs` 的 `candidate_anchor` 把格坐标换成屏幕矩形（末列换行到上一行末尾）。
组合中的文本画在光标处；带字节的按键会 `stop_propagation()`，避免同一字符走两遍平台路径。

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
./scripts/build-gpui-macos.sh --release        # 产出 build/macos/MuxtermGpui.app
```

`gpui` feature = `["dep:gpui", "ffi"]`，默认不启用，现有 GTK / TUI / FFI 构建完全不受影响。

`.app` 结构与 Swift 前端一致：`Contents/MacOS/MuxtermGpui` 是一个 exec 脚本，转发到同 bundle 内的
`muxterm gpui`。这样做是因为项目规则禁止第二个 frontend binary，而 LaunchServices 不能传参。

## 8. 已知缺口（v1 不含）

| 缺口 | 影响 | 后续 |
| --- | --- | --- |
| 服务端滚动通道 | Herdr / tmux 的 server-side scroll 走不到 | 等 Core 提供通道后接 `WheelOutcome::Server` |
| 提交帧绘制（committed paint） | 没有「已提交但未确认」的第二层 | 按 `terminal.md` 缺口表接 |
| 逐 pane 颜色上报 | Core 的 pane colours 仍取默认 | 接 `ClientCommand::PaneColours` |
| 无障碍树 | 与现状持平（现状也没有） | gpui 已依赖 accesskit，接上即净增 |
| sixel / 图片协议 | 无 | 按需 |
| clipboard 图片读取 | 只移植了阶段横幅，PNG 读取未接 | 平台层读 clipboard image 后走 `start_image_paste` |
| `.app` 里没有菜单栏 | Cmd 快捷键靠 window key handler | 接 gpui 菜单，或继续走 handler |

已完成（v1 缺口表里曾经列过的）：输入法 marked text 与候选窗定位、滚轮历史滚动、
设置页、快速面板、鼠标事件上报、宽字符宽度、Cmd-Shift-P 命令面板、Core 驱动的
浅色/深色主题（默认白底黑字 + Menlo 18）、按真实字形度量格子、窗口级
`refresh-client -C` 上报、attach 历史回填（Core `PaneHistory` → 本地 scrollback）、
Cmd-P 面板列出现有 runtime（discovery 驱动，点开即 attach）、macOS 原生菜单栏
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
  只在可见 pane > 1 时出现，`reservedHeight` 恒 0（浮在 Surface 上，不改变 cols/rows），
  空标题退回 `Pane @<id>`。
- 图片粘贴**只调 Core**：前端读剪贴板 → 只接受 PNG → `start_image_paste` →
  每个 tick `poll_image_paste` 取路径。前端不拼 `scp`、不起进程；阶段由
  `model/image_paste::ImagePasteTimeline` 驱动横幅（进度 / 成功 / 失败）。
- 指示色对齐 Swift 语义色：`working` = systemYellow、`blocked` = systemPink、
  `done` = systemTeal（之前借用 success/danger/warning，侧栏与 tab 看起来偏红偏绿）。
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
| 终端选区复制 | 有 | 没有鼠标拖选；`Edit → Copy` 仍是禁用项（Info 浮层的复制已可用） |
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
| 快捷键 | `handle_key` 用 `keystroke.modifiers.platform`：macOS=Command，Linux=Super，Windows=Win。`[shortcuts] primary_key` 与 `effective_keybindings` 还没被 gpui 消费。 | 接 `primary_key` / `effective_keybindings`（Linux 上 GTK 前端用 Alt），或明确「gpui 前端只支持 macOS 语义」。 |
| 菜单栏 | macOS 走原生 NSMenu（`platform/macos_menu.rs`，结构 = `model/menu.rs`）。Linux / Windows 还没有菜单条（`platform::install_menu()` 返回 `None`）。 | 自绘菜单条：同一份 `model::menu` 数据，用 gpui 元素画在窗口顶部。 |
| CI | `.github/workflows/*.yml` 里 `gpui` 出现 0 次。 | 加一个 `ubuntu-latest` 的 `cargo check --no-default-features --features gpui`。 |
| 打包 | 只有 `scripts/build-gpui-macos.sh`，没有 Linux `.desktop` / Windows 打包。 | 与各平台 frontend 一起补。 |
| 图标 | macOS `.app` 内置 `MuxtermLogo.icns` + `CFBundleIconFile`（与 Swift 同一份资源）。 | Linux / Windows 用同一张 PNG 生成 `.desktop` / `.ico`。 |

## 9. 验收

- `cargo check --no-default-features --features gpui` 通过。
- `cargo test --no-default-features --features gpui --lib` 通过（2026-11-09 实测 **1948 passed**）。
- 集成测试（每个都实测通过）：
  - `--test gpui_chrome`（81，含命令面板每条命令的动作、改名浮层、tab 重排、语言/主题/状态栏、
    设置浮层的 Esc / ⏎ / Cancel / Apply 与放弃修改确认（有改动先问、点遮罩也走同一条路）、
    面板按 Core attach 频率排序（活跃项第一）、
    SGR 粗体/斜体/下划线/删除线真的进渲染树、
    侧栏两个固定聚合槽与 `shell` 行隐藏、空本机 shell 只补一次 `NewTab`、
    `Cmd-[` / `Cmd-]` 按当前 tab 布局选目标（含 zoom）、面板三条身份去重
    （provenance / canonical target / 名字）、活动快照填充 Agents/Commands、Info 复制按钮、
    空闲 pump 不重绘）：侧栏 / tab / 状态栏 / Cmd-P 面板（含 discovery 到的现有 runtime）/
    Cmd-Shift-P 命令面板 / 输入框 / 设置 / Open 入口 / 历史回填 / 菜单动作，并断言真实布局
    （侧栏 240pt、24pt 单条栏、命令面板 600×360），全部走真实 `GpuiApp`。
  - `--test gpui_workspace_picker`（13）：根列表四段顺序与编号、Open 三层导航、目录完成后
    的「运行时 + 名字」确认（默认 Herdr + 路径末段）、空名字拒绝、Esc 逐层退回、
    Cmd-K 行内动作集与浮层位置、Save 点击只写 projects（保留 template 等未知字段）、
    已保存/未保存两套动作、Herdr attach 带 `workspace_id`、面板吞键不落 PTY。
  - `--test gpui_surface_events`（6）：滚轮、宽字符、鼠标报告、IME 提交、候选窗锚定。
  - `--test gpui_surface_e2e`（1）：隔离 tmux 里真实 shell 输出落到格子上。
  - `--test gpui_acceptance`（11）：启动 workspace、本机 shell 启动即显示 Shells 聚合
    （侧栏只有两个聚合槽、聚合 tab 带来源）、按键进 shell、Cmd-T 加 tab、Cmd-D 分屏与
    Cmd-[ / Cmd-] 换 pane（焦点跟随）、Rename Tab / Move Tab 走 Core 往返、活动快照 join 到
    前端 workspace id、面板 Open 全流程真的建出 workspace、第二个 workspace 经 Catalog 打开。
  - `--test gpui_acceptance_herdr`（2）：named Herdr session 出现在侧栏，输入有回显。
  - `--test gpui_harness_smoke`（1）：无头 harness 自身可用。
- 空闲开销（2026-11-06 实测，`muxterm gpui -L <隔离 socket>` 空转 6 秒后连采 3 次）：
  轮询固定 8ms 时 **2.9% / 3.0% / 2.9% CPU**，改成「空闲退避到 64ms、有事件立刻回 8ms」
  加「tmux status 无变化不重绘」后 **0.8% / 0.7% / 0.7% CPU**，RSS 89MB 左右不变。
- `muxterm gpui` 打开窗口，侧栏列出 workspace，tab 栏与 pane 网格按 `ClientLayout` 渲染。
- `build/macos/MuxtermGpui.app` 存在、`codesign --verify` 通过、双击可启动。

所有真机测试都用隔离 socket / named session：tmux 走 `-L muxterm-test-*`（清理带同一个 `-L`），
Herdr 走 `muxterm-test-*` session（只 `session stop` + `session delete`）。

## 10. 不做什么

- 不删 Swift / GTK 前端，不把 `.app` 默认入口换掉。
- 不为 gpui 单独做侧边栏或 agent 列表（沿用 `FRONTEND.md` §2.3 的词表）。
- 不在 gpui 侧新建连接池、warm slot、第二事件泵。
- 不把 Index 的 `visible_ansi` 喂进 CellGrid（`SURFACE.md` §3 定律 2、9）。
