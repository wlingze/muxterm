# TASKS.md — 施工顺序

产品结构见 [`docs/WORKSPACE.md`](docs/WORKSPACE.md)。Runtime 见 [`docs/RUNTIME.md`](docs/RUNTIME.md)。
像素见 [`docs/SURFACE.md`](docs/SURFACE.md)。前端见 [`docs/FRONTEND.md`](docs/FRONTEND.md)。
目录见 [`docs/PROJECT-STRUCTURE.md`](docs/PROJECT-STRUCTURE.md)。
测试门禁见 [`docs/TESTING.md`](docs/TESTING.md)。

每个阶段保持可编译、可测试。增量提交。tmux 测试只用 `-L muxterm-test-*`。
Herdr 测试只用 named session `muxterm-test-*`。不要对默认 server 执行 `kill-session` /
`kill-server` / `herdr server stop`。

## 已锁定（实现时不要重开）

1. Core 是库。唯一 binary 是薄 `src/main.rs`（无 `mod` 声明）。
2. 所有 frontend 只经 C FFI + 统一 `ffi_client`。
3. Runtime × Transport 独立组合。没有 `LocalTmux` 这类复合 mode。
4. 没有 generic `model` 仓库。
5. provider 自注册；Catalog 只回答「能打开什么」。组合根是 `Muxterm`。
6. Workspace = Runtime(Transport) + path。
7. Project / Worktree 在 `core/projects`，不进 Tab/Pane 树。
8. 三条 lane：Control / Render-data / Activity。
9. Candidate / OpenRequest / WorkspaceSpec 三分；frontend 永不构造 spec。
10. daemon 是 shell 的 daemon 化形态，不是第四种 Runtime。
11. WorkspaceTemplate 只在 create 时应用。
12. 前端常驻 Scene + 单事件泵。没有 warm/cold slot、`bridgeLock`、串行后台队列、前台校准。
13. 单一 crate。模块树是 `src/core` + `src/frontend`。边界靠模块 + `ffi_client` +
    `scripts/check-architecture.sh`，不拆 Cargo workspace crate。

命名：`RuntimeProvider` / `TransportProvider` / `TargetConnection` / `ByteChannel` /
`ConnectionRegistry` / `core/projects/` / `core/activity/`。C 符号保留 `muxterm_`。
库层错误用 `thiserror`。`Box<dyn Runtime>` 用 `#[async_trait]`。

## 实现时再定（不阻塞 Phase 1）

1. `ActivityRecord` status 终值与归档策略
2. Activity FFI 独立 poll 还是并入 workspace poll
3. `gui` 在 macOS 上：进程内嵌 vs 唤起 app bundle
4. windows frontend 落地时间
5. `ffi_client.rs` 最终文件名
6. Recent / last-used 是否落盘（不能污染手写 `config.toml`）
7. `PaneTemplate.command`：`SendKeys` 回车 vs runtime 原生启动
8. generic worktree 在 tmux 上的 session 命名
9. `RuntimeSignal` 变体清单
10. ActivityRecord 展示名带 revision 缓存，还是只带 `WorkspaceId` 由前端 join

## Phase 1 — 一棵 Core module tree + 单一 binary

1. 删除 `src/main.rs` 里的 `mod core; mod platform;`（文件保留，改薄）
2. `src/lib.rs` = private Core modules + public FFI facade
3. `src/main.rs` 只做 `fn main` → lib 的 frontend 启动；clap：无 subcommand=CLI，`tui`，`gui`
4. frontend 暂时原地，只依赖 `muxterm::ffi` 或 `ffi_client`
5. 模块边界按 `src/core` 与 `src/frontend` 画，不预留第二套 crate 目录

验收：`src/main.rs` 无 `mod`；`src/bin/` 无第二 binary；`cargo check/build/test`。

第一刀建议：先去掉 `src/main.rs` 的 `mod` 声明，启动函数改走 `muxterm::`。不要在同一
commit 里搬目录。

## Phase 2 — 拆 generic model

- Runtime/Capability → `core/runtime`
- Workspace/Pool/Spec/Template → `core/workspace`
- Project/Worktree → `core/projects`
- 稳定 DTO → `core/protocol`
- `core/model` 不再作为公共入口

验收：`mod.rs` 不平铺所有 struct；Core 单测绿。

## Phase 3 — Runtime / Transport provider

- 删除 `RuntimeMode`、`create_runtime`、`WorkspaceSpec::build_runtime()`
- `RuntimeProvider::new_instance(conn, spec)` + `channel_requirements()`
- `TransportProvider::supported_channels()` + `TargetConnection::open_channel`
- Catalog 收窄；`Muxterm` 组合根接管 handle
- frontend 改传 `OpenRequest`
- 不兼容组合显式 `ResolveError`

验收：`rg 'WorkspaceSpec' src/frontend` 空；local/SSH × shell/tmux/herdr 无 silent fallback。

## Phase 4 — Pool / Projects / Worktree / Template

- Workspace 持有 Runtime instance + `provenance`
- `create_worktree`：native | generic，完成条件 = 池里新开一格且归组
- Candidate 四类经 resolver
- `WorkspaceTemplate`：`[[templates]]`，只在 create 时经 Task 应用
- 开始拆 `TargetConfig`

验收：Worktree 不进 Tab/Pane 树；attach 不套模板；无 `SplitPane` 时跳过分割并报告。

## Phase 5 — Transport 三层命名 + shell daemon

- 旧 target transport → `TransportProvider`
- 旧字节 Transport → `ByteChannel`
- `ConnectionRegistry` 落地
- daemon wire 从 CLI 模块移到 `runtime/shell/`
- daemon IPC 传三 lane 事件流，删除 `DumpState` 快照 diff
- Core 无 `use crate::frontend` / `use crate::platform`

验收：同一 target 多 Workspace 复用同一 TargetConnection；daemon client 不依赖 CLI module。

## Phase 6 — 三条 lane

- parser 产出 Control / Render；Workspace 产出 ActivityRecord
- `%output` 解码后原始 bytes 直达 `PaneOutput`
- Herdr full/diff → `PaneFrame`/`PaneOutput`
- `core/attention/` → `core/activity/attention/`
- `visible_ansi` 家族离开 live path

验收：20 次全屏重绘不 dump/reset；poll 顺序 topology → activity → frame → output；
`runtime/` 无 `ActivityRecord`；`workspace/` 无 `%output` / `terminal.frame`。

## Phase 7 — FFI 拆分 + frontend 迁 `ffi_client`

- 拆 `ffi/api.rs`（含 activity）
- 统一 `frontend/ffi_client.rs`，删除散装 `ffi_bridge`
- 目录 `cli / tui / linux / macos / windows`
- 一份 `muxterm.h`
- Surface API 只提供 output/frame/history

验收：`rg 'crate::core' src/frontend src/bin` 空；FFI poll bytes 在边界复制。

## Phase 8 — 清理 + CI 结构检查

删除 `RuntimeMode`、旧 factory、`catalog/builtin`、public ANSI dump、无职责的平行 config 模块。
CI 跑 [`docs/TESTING.md`](docs/TESTING.md) 的结构门禁。

## Phase 9 — 前端场景化

契约：[`docs/FRONTEND.md`](docs/FRONTEND.md)。通用层：Scene / SceneStack / ViewStore / EventPump / CommandQueue / Overlay / Settings。

- linux：Scene = `GtkStack` page；拆 `window.rs` 等巨石；删除 `LayoutHost` retain 丢弃。
- macOS：删除 WarmConnectionSlot / `bridgeLock` / `backgroundPollQueue` / 前台权威校准。
  CoreBridge 瘦身为 FFI + DTO。live owner 是 Core WorkspacePool。
- tui：Scene = per-workspace buffer 组。cli：无 Scene。windows：占位。

验收：切 workspace/tab 零 FFI、零锁、无 recapture / reset / 白屏；
`rg 'bridgeLock|backgroundPollQueue|WarmConnectionSlot|ForegroundAuthority'` 在 macOS 前端为空。

切换延迟来自 frontend 自己的锁和串行队列，不是 tmux 全局锁。见
[`docs/FRONTEND.md`](docs/FRONTEND.md) 与 [`docs/SURFACE.md`](docs/SURFACE.md) §9。

## 剩余（本轮）

现有历史 commit 保持不动。后续每个可独立验证的任务一个 commit，粒度中等。

1. ~~收回 workspace crate 到 `src/core` + `src/frontend`。~~
2. ~~迁完旧 QuickConnect / `pane_scroll_ansi` fixture，根测试可编译。~~
3. ~~Phase 6 收尾：生产路径已是 `RuntimeBatch`；C ABI 在边界摊平并冻结。~~
4. ~~Phase 8：空目录、过期文档、TESTING.md 门禁补进 `scripts/check-architecture.sh`。~~
5. ~~Phase 9 TUI：per-workspace Scene buffer 组；切 workspace/tab 不走 FFI 拉帧。~~

## 非目标

不重写 tmux 控制协议；不新增 Runtime；不实现 `pause-after`；不实现 windows frontend；
不为 dump API 保留双 VT；不对默认 tmux/Herdr 做破坏性测试。
不把 frontend 拆成独立 Cargo crate。
