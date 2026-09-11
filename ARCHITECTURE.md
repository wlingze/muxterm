# Muxterm Architecture

产品树：[`docs/WORKSPACE.md`](docs/WORKSPACE.md)。
Runtime：[`docs/RUNTIME.md`](docs/RUNTIME.md)。像素：[`docs/SURFACE.md`](docs/SURFACE.md)。
前端：[`docs/FRONTEND.md`](docs/FRONTEND.md)。
Catalog：[`docs/CATALOG.md`](docs/CATALOG.md)。配置：[`docs/CONFIG.md`](docs/CONFIG.md)。
目录：[`docs/PROJECT-STRUCTURE.md`](docs/PROJECT-STRUCTURE.md)。施工：[`TASKS.md`](TASKS.md)。

本文写分层。前端页面、Scene、EventPump、平台映射以 [`docs/FRONTEND.md`](docs/FRONTEND.md) 为准。
没有产品 Session、虚拟 Window、复合 RuntimeMode 或 frontend 连接池。

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

单一 crate：`src/core`（库）+ `src/frontend`（cli / tui / linux / macos / windows）。
依赖方向靠模块边界和 `scripts/check-architecture.sh` 守，不拆 Cargo workspace crate。

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

## 5. 前端

frontend 只经 C FFI + `ffi_client`。已打开 Workspace 各有一棵常驻 Scene；切换零 Core 调用。
完整词汇表、页面骨架、平台映射、性能预算、验收见 [`docs/FRONTEND.md`](docs/FRONTEND.md)。
像素定律见 [`docs/SURFACE.md`](docs/SURFACE.md)。

## 6. 测试与安全

- tmux 测试只用隔离 socket `-L muxterm-test-<unique>`。禁止对默认 server `kill-server` / `kill-session`。
- Herdr 测试只用 named session `muxterm-test-<unique>`。禁止无名字的 `herdr server stop`。
- 结构门禁与矩阵见 [`docs/TESTING.md`](docs/TESTING.md)。
