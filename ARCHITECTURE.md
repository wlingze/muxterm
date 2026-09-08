# Muxterm Architecture

产品树：[`docs/WORKSPACE.md`](docs/WORKSPACE.md)。
Runtime：[`docs/RUNTIME.md`](docs/RUNTIME.md)。像素：[`docs/SURFACE.md`](docs/SURFACE.md)。
Catalog：[`docs/CATALOG.md`](docs/CATALOG.md)。配置：[`docs/CONFIG.md`](docs/CONFIG.md)。
目录：[`docs/PROJECT-STRUCTURE.md`](docs/PROJECT-STRUCTURE.md)。施工：[`TASKS.md`](TASKS.md)。

本文写分层与前端通用层。交互细节以 WORKSPACE / SURFACE 为准。没有产品 Session、虚拟 Window、
复合 RuntimeMode 或 frontend 连接池。

## 1. 分层

```text
Frontend（cli / tui / linux / macos / windows）
        │  只经 C FFI + 统一 ffi_client
        ▼
Muxterm library（src/lib.rs 是唯一 Core module root）
        │
        ├── Config
        ├── Catalog            providers 视图 + inventory + resolver
        ├── Projects
        ├── Activity
        ├── transport::ConnectionRegistry
        └── WorkspacePool
              └── Workspace = Runtime(Transport) + path
                    ├── Runtime instance（wire → 三条 lane）
                    ├── Tab → Pane
                    └── Index（ANSI 内容：搜索 / OSC 133 / BEL）
```

依赖方向：**frontend → FFI → Core**。Core 不得 `use crate::frontend`（现状的 `platform`）。frontend 不得 `use` Runtime concrete type、WorkspacePool、discovery、VT emulate。

唯一 binary：薄 `src/main.rs`，无 `mod` 声明，只调用 lib 的 frontend 启动函数。Cargo 惯例：默认可执行文件是 `src/main.rs`（[Cargo Book: Package Layout](https://doc.rust-lang.org/cargo/guide/project-layout.html)）。

目标 crate 分层（Phase 1–2）：`muxterm-protocol` / `muxterm-core` / `muxterm-runtime` / `muxterm-transport` / frontend。拆分完成前用 `rg` 门禁兜底。

## 2. 硬约束

1. Core 是库；frontend 是客户端。
2. Runtime 与 Transport 独立注册、打开时组合。兼容性 = `channel_requirements() ⊆ supported_channels()`。
3. 没有 generic `model` 仓库。struct 放在拥有不变式的领域；跨边界 DTO 放 `protocol`。
4. Catalog 不是 god facade，也不拥有 Pool / 连接。
5. daemon 不是第四种 Runtime，是 shell runtime 的 daemon 化形态。
6. 渲染：原始 `PaneOutput` / `PaneFrame` / `PaneHistory` 进常驻 Surface。`visible_ansi` 家族不是 live API。
7. 问能力用 `support()`，禁止 `if runtime == "herdr"`。

## 3. 打开

```text
Candidate（列表） → OpenRequest（用户选择） → Catalog::resolve → WorkspaceSpec
                                                         → WorkspacePool::open
```

frontend 只见 Candidate / OpenRequest。WorkspaceSpec 是 Core 内部。

## 4. 三条 lane

| Lane | 内容 | 解析 | 出口 |
|------|------|------|------|
| Control | 拓扑 / layout / 焦点 / 状态 / gap barrier | Runtime 解析 wire | Workspace 拥有拓扑真相 |
| Render-data | `PaneOutput` / `PaneFrame` / `PaneHistory` | Runtime 还原原始字节 | Surface 直吃 |
| Activity | `Upsert` / `Remove` + revision | Workspace 归一化；Pool 跨工作区聚合 | 侧栏 badge / 时间线 / 通知 |

一个 poll batch 顺序：topology → activity → frame → output。

## 5. 前端通用层

平台无关词汇（各前端用本语言实现同一套，禁止第二套概念名）：

| 组件 | 职责 |
|------|------|
| AppShell | 窗口骨架：Sidebar + SceneStack + Overlay |
| Scene | 一个已打开 Workspace 的完整视图树；从 open 到 close 常驻 |
| SceneStack | 切换 = 换可见子树；切换时不调 Core |
| PaneSurface | 每 pane 恰好一个常驻 VT |
| ViewStore | per-WorkspaceId 的 UI 只读快照 |
| EventPump | **唯一** FFI 事件消费者 |
| CommandQueue | UI → Core 的 Task；合并同类命令 |
| Overlay | QuickPanel / CommandPalette / Search / Attention |

Linux Scene 容器是 `GtkStack` page：一次只显示一个子 widget，子页面仍留在树里（[GTK4 GtkStack](https://docs.gtk.org/gtk4/class.Stack.html)）。macOS：CoreBridge 是唯一 FFI 口，没有 WarmConnectionSlot / `bridgeLock` / 串行后台队列 / 前台校准，见 [`docs/SURFACE.md`](docs/SURFACE.md) §8–§9。

切 Workspace / Tab 的点击路径：**零 FFI、零锁、首帧 ≤ 1 帧**。

## 6. 交互（各 GUI 必须一致）

- 嵌套分割：每次只替换当前叶子 pane，不重新平铺全树。
- 焦点：操作后焦点回到终端，不落到工具栏。不要底部输入框发 send-keys。
- Tab 显示：序号 + 名字；多 pane 可加数量后缀。不要每个 pane 一个 Notebook tab。
- 关 GUI 窗：有 `PersistDetach` 的 Runtime **detach**；shell **shutdown**。
- 快捷键与命令面板走 Action Catalog，不按 GTK/AppKit 类型存盘。

## 7. 测试与安全

- tmux 测试只用隔离 socket `-L muxterm-test-<unique>`。禁止对默认 server `kill-server` / `kill-session`。
- Herdr 测试只用 named session `muxterm-test-<unique>`。禁止无名字的 `herdr server stop`。
- 结构门禁与矩阵见 [`docs/TESTING.md`](docs/TESTING.md)。
