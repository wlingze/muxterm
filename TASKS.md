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

## 已拍板（原「实现时再定」）

1. `ActivityRecord` 终值是 `Done { exit_code }` 与 `Failed`。Running/Waiting 不自动淘汰；
   终态记录留在 live store，最多 64 条，超了按 `updated_at` 最旧 Remove；不落盘。
2. Activity FFI 保持独立 `muxterm_activity_take_events_json`。跨工作区 lane 不并入
   pane poll；EventPump 先 topology 再 drain activity。
3. macOS `muxterm gui` 唤起 `Muxterm.app` bundle（`open`；`--debug`/`--log-file` 前台
   exec）。不把 Swift UI 嵌进 CLI 进程。
4. windows frontend 不在本重构落地，只保留 `src/frontend/windows` 占位。
5. Rust FFI wrapper 路径为 `frontend/utils/corebridge`（类型名 `CoreBridge`；
   `FfiClient` 是过渡别名）。C 符号仍是 `muxterm_*`。
6. Recent / last-used 是进程内 `WorkspacePool` 状态，不写 `config.toml`，也不另开
   sidecar。Recent 候选来自 live pool。
7. 模板新建 tab/split 走 Runtime 原生 `NewTab`/`SplitPane` 的 command/cwd/env；
   已存在的 create-time 根 pane 才 `SendKeys` + Enter。
8. generic tmux worktree session = `{project_id}/{worktree_id}`（组件内 `/` 等换成 `-`）。
9. `RuntimeSignal` 只保留 `PaneAgentChanged` 与 `StatusBarSubscription`。标题在 Control；
   命令退出码由 Index/Activity 归一化，不加新 signal。
10. `workspace_name` / `runtime_name` / `transport_name` 作为带 revision 的展示缓存
    写在 record 上；前端不 join WorkspacePool 来画 Activity。

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
6. ~~原「实现时再定」十项已拍板（见上文）。~~
7. ~~第二轮：provider 子模块默认 `mod`；Catalog/FFI 走 trait；
   `frontend/utils/{corebridge,i18n}`；Linux `app/chrome/terminal/ui`。~~
   见 [`docs/MODULE-SURFACE.md`](docs/MODULE-SURFACE.md)。

后续按 [`docs/PRODUCT-DIRECTION.md`](docs/PRODUCT-DIRECTION.md)（2026-10-26）收口。
[`docs/PRODUCT-1.0-REMAINING.md`](docs/PRODUCT-1.0-REMAINING.md) 保留旧交互拆解，不再要求四波全部进入 1.0。
以下是后续路线，本轮只更新文档；每次实现仍按用户当轮指定的任务推进。

8. 持续修复日用阻断：输入/IME/粘贴、显示/滚动、SSH 重连、目标身份与 Project 习惯兼容。
9. 下一项：独立多行编辑框，按目标保留草稿，复用现有 local/SSH 图片投递与路径插入，明确失败语义。
10. 随后分项交付：批量复制/输出阅读、可靠命令边界下的折叠；终端内联图片 + 图片/文件点击预览。
    远程路径正确关联机器，外部打开不暗示副本自动回写。阅读探索见 [`docs/OUTPUT-READING.md`](docs/OUTPUT-READING.md)。
11. 按需：端口转发、已连接工作区搜索定位、Attention 回到正确现场及导航打磨。
12. 进入稳定维护：普通 shell 上手、外部用户试用、兼容性、性能、安装更新和已有核心行为的平台一致性。
    Agent 按轮阅读只做有明确来源的可选探索，不阻塞通用终端发布，不为保持开发节奏扩大范围。

已有记录：macOS Shells + Cmd-K、Agents 槽已落地。Linux 聚合 Scene 与配置绑定、地标/搜索坐标、
Cmd-W/选词等旧待办按真实日用问题重新排优先级，不默认阻塞编辑框或发布。
提升 muxer、新 Window 形态与 Tab 视觉保留为候选，不预排成必做阶段。

## 非目标

不重写 tmux 控制协议；不新增 Runtime；不实现 `pause-after`；不实现 windows frontend；
不为 dump API 保留双 VT；不对默认 tmux/Herdr 做破坏性测试。
不把 frontend 拆成独立 Cargo crate。
本轮方向不包含 ACP 客户端、完整 IDE/文件管理器、agent 编排、手机/云服务或新 Runtime。
