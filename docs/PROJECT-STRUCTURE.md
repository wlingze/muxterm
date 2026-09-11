# Muxterm 项目结构

产品：[`WORKSPACE.md`](WORKSPACE.md)。Catalog：[`CATALOG.md`](CATALOG.md)。
Runtime：[`RUNTIME.md`](RUNTIME.md)。像素：[`SURFACE.md`](SURFACE.md)。
Config：[`CONFIG.md`](CONFIG.md)。施工：[`../TASKS.md`](../TASKS.md)。

产品树：`Muxterm → WorkspacePool → Workspace = Runtime(Transport) + path → Tab → Pane`。
Window 只是 GUI 体现。shell / tmux / herdr 是 RuntimeProvider；local / ssh 是 TransportProvider。

## 目录

```text
src/
├── lib.rs                         # 唯一 Core library root
├── main.rs                        # 唯一 binary（薄；无 mod 声明）
├── frontend/
│   ├── ffi_client.rs              # 统一安全 FFI wrapper
│   ├── app_shell.rs               # Scene / ViewStore / EventPump 接口
│   ├── cli/
│   ├── tui/
│   ├── linux/
│   ├── macos/
│   └── windows/                   # 占位
└── core/
    ├── muxterm.rs                 # 组合根
    ├── catalog/                   # providers 视图 + inventory + resolver
    ├── runtime/                   # contract / provider / registry / shell|tmux|herdr
    │   └── shell/daemon.rs        # daemon 化形态，不是独立 provider
    ├── transport/                 # provider / connection / channel / registry / local|ssh
    ├── workspace/                 # spec / template / workspace / pool / router / index
    ├── projects/                  # Project / Worktree / service / store
    ├── activity/                  # commands + agents + attention
    ├── protocol/                  # ids / candidate / events / task / snapshot / ffi
    ├── config/                    # document / schema / migration / transaction / store
    ├── terminal/                  # input / index
    └── logging.rs
```

单一 crate。`src/core` 是库模块树，`src/frontend` 是全部 frontend；`src/main.rs` 只做入口。
边界靠 `ffi_client` 与 [`../scripts/check-architecture.sh`](../scripts/check-architecture.sh)，
不按 protocol / runtime / transport 再拆 Cargo workspace crate。

当前代码仍有一部分住在 `crates/muxterm-*`。按 [`../TASKS.md`](../TASKS.md) 收回上表。

## 不要

- 产品 Session、虚拟 Window
- frontend 连接池 / warm slot / `bridgeLock`
- `TransportDriver` 这个名字
- generic `core/model` 作为公共数据仓库
- `catalog/builtin` 里的 concrete Runtime 构造器
- 独立 `runtime/daemon/` provider 目录
- `src/bin/` 下第二个 frontend binary
- `src/main.rs` 声明 Core modules

## 文档地图

| 文档 | 角色 |
|------|------|
| [`../PRODUCT.md`](../PRODUCT.md) | 产品 |
| [`../ARCHITECTURE.md`](../ARCHITECTURE.md) | 分层 |
| [`../TASKS.md`](../TASKS.md) | 施工顺序 |
| [`WORKSPACE.md`](WORKSPACE.md) | 产品树与 FFI |
| [`RUNTIME.md`](RUNTIME.md) | Runtime × Transport |
| [`SURFACE.md`](SURFACE.md) | 像素定律 |
| [`FRONTEND.md`](FRONTEND.md) | 前端 Scene / 页面 / 各平台 |
| [`CATALOG.md`](CATALOG.md) | 能打开什么 |
| [`CONFIG.md`](CONFIG.md) | 配置 |
| [`ID-SYSTEM.md`](ID-SYSTEM.md) | ID |
| [`LAYER-MAPPING.md`](LAYER-MAPPING.md) | 只给 runtime/tmux |
| [`TESTING.md`](TESTING.md) | 测试门禁 |
| [`HERDR-RUNTIME-STABILITY.md`](HERDR-RUNTIME-STABILITY.md) | Herdr stream / identity |
| [`HERDR-TESTING.md`](HERDR-TESTING.md) | Herdr 专项门禁 |
| [`TRANSPORT-PROTOCOL-ARCHITECTURE.md`](TRANSPORT-PROTOCOL-ARCHITECTURE.md) | Transport 三层 |
| [`RENDERING-OPTIMIZATION.md`](RENDERING-OPTIMIZATION.md) | Linux Surface 渲染 |
