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
设置页、快速面板、鼠标事件上报、宽字符宽度。

## 9. 验收

- `cargo check --no-default-features --features gpui` 通过。
- `cargo test --no-default-features --features gpui --lib` 通过（2026-11-04 实测 **1847 passed**）。
- 集成测试（每个都实测通过）：
  - `--test gpui_chrome`（26）：侧栏 / tab / 状态栏 / Cmd-P 面板 / 输入框 / 设置 / 新建项目，全部走真实 `GpuiApp`。
  - `--test gpui_surface_events`（6）：滚轮、宽字符、鼠标报告、IME 提交、候选窗锚定。
  - `--test gpui_surface_e2e`（1）：隔离 tmux 里真实 shell 输出落到格子上。
  - `--test gpui_acceptance`（4）：启动 workspace、按键进 shell、Cmd-T 加 tab、第二个 workspace 经 Catalog 打开并可切换。
  - `--test gpui_acceptance_herdr`（2）：named Herdr session 出现在侧栏，输入有回显。
  - `--test gpui_harness_smoke`（1）：无头 harness 自身可用。
- `muxterm gpui` 打开窗口，侧栏列出 workspace，tab 栏与 pane 网格按 `ClientLayout` 渲染。
- `build/macos/MuxtermGpui.app` 存在、`codesign --verify` 通过、双击可启动。

所有真机测试都用隔离 socket / named session：tmux 走 `-L muxterm-test-*`（清理带同一个 `-L`），
Herdr 走 `muxterm-test-*` session（只 `session stop` + `session delete`）。

## 10. 不做什么

- 不删 Swift / GTK 前端，不把 `.app` 默认入口换掉。
- 不为 gpui 单独做侧边栏或 agent 列表（沿用 `FRONTEND.md` §2.3 的词表）。
- 不在 gpui 侧新建连接池、warm slot、第二事件泵。
- 不把 Index 的 `visible_ansi` 喂进 CellGrid（`SURFACE.md` §3 定律 2、9）。
