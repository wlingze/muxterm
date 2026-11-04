# FRONTEND-GPUI.md — GPUI 前端方案

> 方案与落地记录。核对时间：2026-11-04T14:22–15:10+08:00（Asia/Shanghai）。
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
2. **中文输入法尚未接。** gpui 0.2.2 有 `EntityInputHandler` / marked text 接口，但需要终端级的候选窗定位，
   本方案第一版只保证 ASCII 与已有输入法提交后的字符，未接 marked text。这是已知缺口，不是完成态。

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
├── mod.rs          # 模块清单；cfg(feature = "gpui")
├── app.rs          # Application::new().run + 窗口 + 启动装配
├── state.rs        # GpuiApp：EventPump / ViewStore / CommandQueue / Scene 表 / focus
├── bridge.rs       # 事件泵循环 + 主线程桥（把批次写成 owned 投影）
├── theme.rs        # design token（色板 / 间距 / 字号）
├── keymap.rs       # Keystroke → SendKeys 字节
├── chrome/
│   ├── shell.rs    # AppShell：Sidebar + 主区 + StatusBar
│   ├── sidebar.rs  # workspace 列表 + activity badge
│   ├── tab_bar.rs  # tab 行
│   ├── status_bar.rs
│   └── pane_grid.rs # ClientLayout → 递归分割
└── terminal/
    ├── grid.rs     # CellGrid：cell / SGR / scrollback（纯逻辑，带测试）
    ├── parser.rs   # vte::Perform 实现（纯逻辑，带测试）
    └── surface.rs  # PaneSurface：常驻 grid，feed 原始字节，不 reset
```

`src/main.rs` 仍是唯一 binary。gpui 前端通过 `muxterm gpui` 子命令进入，
`.app` 由 `scripts/build-gpui-macos.sh` 组装（§7）。

## 5. PaneSurface

一个已打开 pane 恰好一个常驻 `CellGrid`，key 是 `(WorkspaceId, PaneId)`。
新建时一次性 seed，之后增量 feed 原始字节；切 tab / 切 workspace 只切可见性，不重建、不 reset
（符合 [`SURFACE.md`](SURFACE.md) §3 定律 1、3、10）。

CellGrid 只实现渲染需要的那部分 VT：

- 打印字符（UTF-8，宽字符按 1 格处理，v1 限制）
- C0：`BS HT LF CR BEL`
- CSI：`A B C D E F G H f J K L M P @ X S T r d s u`、`m`（SGR）、`h/l`（私有模式 25 / 1049 / 47 / 2004）
- SGR：`0 1 2 3 4 7 8 9`、`30-37 / 40-47`、`90-97 / 100-107`、`38;5;n`、`48;5;n`、`38;2;r;g;b`、`48;2;r;g;b`、`39 / 49`
- ESC：`7 8 M c D E`
- OSC：`0 / 2` 标题，其余忽略（133 标记由 Core 处理，前端不消费）

刻意不做：真彩图、鼠标上报、sixel、underline style、双向文本。这些在缺口表里列出。

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
| 输入法 marked text / 候选窗 | 中文直接输入不可用 | 接 `EntityInputHandler` + 候选窗定位 |
| 无障碍树 | 与现状持平（现状也没有） | gpui 已依赖 accesskit，接上即净增 |
| scrollback 交互 | 只能用 `Viewport` FFI 翻页 | 补滚轮 → `Task::ScrollPane` |
| 设置页 | 无 | schema-driven，复用 Core 快照 |
| 快速面板 / 命令面板 | 无 | Overlay 层 |
| 鼠标事件上报 / sixel | 无 | 按需 |
| 宽字符与 emoji 宽度 | 按 1 格处理 | 补 `unicode-width` 与字形表 |

## 9. 验收

- `cargo check --no-default-features --features gpui` 通过。
- `cargo test --no-default-features --features gpui --lib gpui` 通过（grid / parser / keymap 纯逻辑测试）。
- `muxterm gpui` 打开窗口，侧边栏列出 workspace，tab 栏与 pane 网格按 `ClientLayout` 渲染。
- 在 pane 里打字能进 shell；窗口缩放后 pane 收到 `resize`。
- `build/macos/MuxtermGpui.app` 存在、`codesign --verify` 通过、双击可启动。

## 10. 不做什么

- 不删 Swift / GTK 前端，不把 `.app` 默认入口换掉。
- 不为 gpui 单独做侧边栏或 agent 列表（沿用 `FRONTEND.md` §2.3 的词表）。
- 不在 gpui 侧新建连接池、warm slot、第二事件泵。
- 不把 Index 的 `visible_ansi` 喂进 CellGrid（`SURFACE.md` §3 定律 2、9）。
