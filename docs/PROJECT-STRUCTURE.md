# Muxterm 项目结构

> 契约冻结：2026-09-08（`2026-09-08T15:02:22+08:00`，Asia/Shanghai）
> **§0 是目标树**（施工中）。§1 起是 2026-07/08 的历史记录，不要当施工单。
> 产品：[`WORKSPACE.md`](WORKSPACE.md)。Catalog：[`CATALOG.md`](CATALOG.md)。
> Runtime：[`RUNTIME.md`](RUNTIME.md)。像素：[`SURFACE.md`](SURFACE.md)。
> Config：[`CONFIG.md`](CONFIG.md)。

---

## 0. 目标结构（2026-09-08）

产品树：`Muxterm → WorkspacePool → Workspace = Runtime(Transport) + path → Tab → Pane`。
Window 只是 GUI 体现。shell / tmux / herdr 是 RuntimeProvider；local / ssh 是 TransportProvider。

### 0.1 现状（代码还没搬）

```text
src/lib.rs                 pub mod core; pub mod platform;
src/main.rs                又声明一遍 mod core; mod platform;  ← 禁止（第二棵 module tree）
src/core/{catalog,workspace,runtime,transport,model,protocol,config,config_service,attention,quickconnect}
src/platform/{cli,tui,linux,macos}
```

`src/main.rs` 里的 `mod` 声明必须先删。frontend 目录目标名是 `frontend/`，不是 `platform/`。

### 0.2 目标目录

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
└── core/                          # 单 crate 过渡期；随后拆 workspace crate
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

Cargo workspace 拆分（Phase 1–2）：`muxterm-protocol` / `muxterm-core` / `muxterm-runtime` /
`muxterm-transport` / frontend。用编译器守依赖方向。

### 0.3 不要

- 产品 Session、虚拟 Window
- frontend 连接池 / warm slot / `bridgeLock`
- `TransportDriver` 这个名字
- generic `core/model` 作为公共数据仓库
- `catalog/builtin` 里的 concrete Runtime 构造器
- 独立 `runtime/daemon/` provider 目录
- `src/bin/` 下第二个 frontend binary

---

## 1. 当前目录结构（2026-07-28 基线，历史）

下文是当时的树，**不要按它实现**。保留以免考古断档。

当时：`src/main.rs` + `src/cli/` + `src/core/{types,config,model,backend,tmux,ssh,terminal,ffi}` + `src/platform/`。
产品协议仍写 Session/Window。`DaemonBackend` 被当成一种 backend。

## 2. 2026-08 目标目录（历史，已部分落地又被 2026-09-08 取代）

PR #11 把代码收到 `src/core/` + `src/platform/`。Catalog 一度被定义成「backend 总状态：
两张插件表 + Connect + Inventory + Pool」。`core/model/backend.rs` 仍是 `trait Runtime` 所在地。
`catalog/builtin/` 曾被提议为 Driver 实现落点——**本轮取消**，provider 自注册。

## 3. 文档地图（现行）

| 文档 | 角色 |
|------|------|
| [`../PRODUCT.md`](../PRODUCT.md) | 产品 |
| [`../ARCHITECTURE.md`](../ARCHITECTURE.md) | 分层与前端通用层 |
| [`WORKSPACE.md`](WORKSPACE.md) | 产品树与 FFI |
| [`RUNTIME.md`](RUNTIME.md) | Runtime × Transport |
| [`SURFACE.md`](SURFACE.md) | 像素与常驻 Scene |
| [`CATALOG.md`](CATALOG.md) | 能打开什么 |
| [`CONFIG.md`](CONFIG.md) | 配置契约 |
| [`ID-SYSTEM.md`](ID-SYSTEM.md) | ID |
| [`LAYER-MAPPING.md`](LAYER-MAPPING.md) | 只给 runtime/tmux |
| [`TESTING.md`](TESTING.md) | 测试门禁 |
| [`TRANSPORT-PROTOCOL-ARCHITECTURE.md`](TRANSPORT-PROTOCOL-ARCHITECTURE.md) | Transport 三层 |
