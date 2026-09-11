# REFACTOR-PLAN-2 — 抽象边界、共享 CoreBridge、Linux 结构

> 调查时间：2026-09-11T15:57:10+08:00（CST）
> 分支：`refactor/muxterm-code-struct`
> worktree：`/home/wlz/Developer/self/muxterm/.worktree/feature-eat-dogfood-0830`
> HEAD：`be4f1527`（第一轮施工已收口；本文是第二轮）
>
> 第一轮施工清单是 [`TASKS.md`](TASKS.md)。本文**不重开**第一轮已锁定的产品树、
> 单一 crate、三条 lane、Scene/EventPump。本文只处理第一轮留下的**抽象泄漏**
> 和 **frontend 结构**。
>
> 接口与可见性规格：[`docs/MODULE-SURFACE.md`](docs/MODULE-SURFACE.md)
> （187 个 `pub mod` 的扫描、去掉 pub 后的调用点、倒逼出的 trait 方法）。
>
> 依据：用户本轮三点（已确认）+ 仓库现状对照
> [`docs/WORKSPACE.md`](docs/WORKSPACE.md)、[`docs/RUNTIME.md`](docs/RUNTIME.md)、
> [`docs/FRONTEND.md`](docs/FRONTEND.md)、[`docs/CATALOG.md`](docs/CATALOG.md)、
> [`docs/PROJECT-STRUCTURE.md`](docs/PROJECT-STRUCTURE.md)。
>
> **本文只是计划。未拍板前不改代码。**

---

## 0. 一句话

第一轮把目录和产品树摆对了，但 **tmux / herdr / ssh 的具体类型仍然从
`runtime/`、`transport/` 漏到 Catalog / FFI / discovery / frontend**；frontend
走了 FFI，却只有 macOS 有名叫 CoreBridge 的层，Linux 仍是 `window.rs` 上帝对象；
i18n 有文件，没有成为所有平台的唯一文案源。

第二轮把三件事收口（已确认）：

1. **默认 `mod`，不要 `pub mod`。** 父模块只 `pub use` 接口。具体 Runtime /
   Transport 关在 provider 目录。外面只用 create + trait + Catalog 注册 id。
   先关可见性，用编译错误倒逼补 trait，而不是把类型再导出。
2. **统一 `frontend/utils/{corebridge,i18n}`。** 所有 Rust 平台只引用这两处；
   macOS Swift CoreBridge 是同一 C ABI 的语言适配，不是第二套协议。
3. **Linux 要分层。** 对标 macOS 的 App / Chrome / Terminal / UI。不重写 GTK 行为。

---

## 1. 第一轮已经完成、本文不重做

| 已完成 | 证据 |
|---|---|
| 单一 crate，`src/core` + `src/frontend` | `src/lib.rs` `#[path]` 展平 |
| frontend 禁止 `use crate::core` | `scripts/check-architecture.sh` |
| Runtime × Transport 独立组合，无 `RuntimeMode` | 同上 |
| Workspace = Runtime(Transport) + path | `docs/WORKSPACE.md` |
| 前端常驻 Scene + EventPump + CommandQueue | linux / macos / tui |
| 文件名曾锁为 `frontend/ffi_client.rs` | `TASKS.md` 已拍板第 5 条 |
| `cargo test --lib --features tui` 绿 | 第一轮收口时 1447 passed |

**本文要重开的唯一旧锁：** `TASKS.md`「文件名锁定为 `frontend/ffi_client.rs`」。
第二轮把它迁到 `frontend/utils/corebridge`（概念名 CoreBridge）。C 符号仍是
`muxterm_*`。其余第一轮锁定不动。

第一轮冻结、第二轮**顺手收**但不单独开题的：

- frontend 仍 `use crate::protocol`（26 个文件）——CoreBridge 应收口 DTO
- `Runtime::as_any` 给 FFI / 测试 downcast 用
- C ABI 边界仍摊平 `StateChange`（保持冻结，不在本轮改 wire）

---

## 2. 目标架构（第二轮要达到的形状）

### 2.1 Core：tmux 只在 `runtime/tmux`

用户原话对应的规范：

> tmux 只存在于 `runtime/tmux`。外面根本看不到这个。外面只用通用的 create +
> interface。标记用 tmux 只是用 tmux 的 **Catalog 注册 id**。transport 同样。

```text
Muxterm（组合根）
  ├── RuntimeRegistry::with_builtins()     唯一允许点名 TmuxDriver/HerdrDriver/ShellDriver
  ├── TransportRegistry::with_builtins()   唯一允许点名 Local/Ssh provider
  ├── Catalog                              只读 id 列表 + inventory + resolve
  ├── WorkspacePool                        Box<dyn Runtime>
  └── Projects                             runtime_id: String / Catalog 查表，不 import 具体类型

runtime/mod.rs 只 pub：
  Runtime, RuntimeProvider, RuntimeRegistry, RuntimeSpec,
  RuntimeBatch, RuntimeCapability, RuntimeError, RuntimeResult,
  WorktreeCreateSpec, WorktreeInfo
  以及 test 用 MockRuntime（core 单测；frontend 禁止）

runtime/{tmux,herdr,shell}     crate-private（`mod` 不是 `pub mod`）
transport/{local,ssh}          crate-private
```

允许的调用：

```text
Catalog / Workspace / FFI / frontend
    → registry.get("tmux")?.new_instance(conn, spec)   // id 是字符串
    → Box<dyn Runtime>
    → runtime.support() / execute / drain_events

禁止：
    crate::runtime::tmux::TmuxRuntime
    crate::runtime::herdr::HerdrSession
    crate::runtime::tmux::status::fetch_snapshot
    workspace.runtime().as_any().downcast_ref::<HerdrRuntime>()
    TargetTransport::attach_backend() → ("tmux-ssh", …)   // 复合 mode 残留
```

「tmux」作为 **id** 可以出现在：

- `RuntimeProvider::id() -> "tmux"`
- `RuntimeInfo.id`、`WorkspaceSpec.runtime`、`WorkspaceId` 的 runtime 段
- Catalog 候选、配置、UI 徽章（问的是 id，不是类型）
- 文案里的产品名（「tmux session」作为用户可见词，不是代码类型）

「tmux」作为 **类型 / 模块 / 协议** 只能出现在 `src/core/runtime/tmux/**`。

transport 对称：

- id `"local"` / `"ssh"` 是 Catalog 注册名
- `SshProcessTransport` / `LocalProcessTransport` / `parse_ssh_config` 只在
  `transport/{local,ssh}`
- 外面只用 `TransportProvider` / `TargetConnection` / `ByteChannel` / `ChannelKind`

组合根例外（必须写进门禁白名单）：

- `src/core/runtime/registry.rs` 的 `with_builtins`
- `src/core/transport/registry.rs` 的 `with_builtins`
- 各 provider 目录内部互引（`tmux/backend.rs` → `tmux/protocol.rs`）

`crate::runtime::` 这个路径本身**不是罪**。Workspace 用
`crate::runtime::{Runtime, RuntimeBatch}` 是对的。罪是 `crate::runtime::tmux::` /
`herdr::` / `shell::` 出现在 provider 目录之外。

### 2.2 Frontend：utils 是唯一共享层

```text
src/frontend/
├── utils/
│   ├── corebridge/     今日 ffi_client.rs。Rust 平台唯一 FFI 口
│   │                   owned DTO、handle 所有权、error envelope
│   └── i18n/           今日 i18n/。唯一文案 catalog + TextKey
├── event_pump.rs       唯一事件消费者（吃 corebridge DTO）
├── command_queue.rs    唯一命令入口
├── view_store.rs       平台无关快照
├── cli/                只引用 utils/{corebridge,i18n} + 上面三个
├── tui/
├── linux/              App / Chrome / Terminal / UI，对标 macos
├── macos/              Swift：CoreBridge/ 是 C ABI 的语言适配，不是第二套协议
└── windows/            占位，挂得上词汇表即可
```

规则：

- Rust frontend **禁止** `use crate::protocol`、`use crate::runtime`、`use crate::workspace`。
  需要的 id / 事件 / layout 全部是 corebridge owned DTO。
- 禁止 `discover_tmux_sessions` 这种带实现名的 API。发现走
  `discover_existing(runtime_id, transport_id, target)`；打开走 `OpenRequest`。
- macOS Swift 不能 `use` Rust 模块。它的 `macos/CoreBridge` 继续包 C ABI，
  **DTO 名字与语义必须与 `utils/corebridge` 对齐**（Tab / Pane / Layout /
  WorkspaceEvent），不要再各自发明一套。
- i18n JSON 只在 `utils/i18n/locales/`。macOS `Resources/i18n` 由构建复制或生成，
  禁止手改第二份。Swift `I18n.swift` 的 key 与 Rust `TextKey` 同源。
- CLI / TUI / Linux /（将来 Windows）全部 `tr(TextKey::…)`，禁止按钮写死
  `"Close"` / `"Cancel"` / `"复制"`。

### 2.3 Linux 结构对标 macOS

macOS 现在是好的参照：

```text
macos/
  App/          窗口、Scene、Sidebar、Settings、QuickConnect
  Chrome/       CommandQueue、SceneStack、PanelModel、KeyBindings
  CoreBridge/   唯一 FFI
  Terminal/     TerminalView / TerminalManager
  UI/           TabBar、StatusBar、PaneLayout
```

Linux 目标（名字用 FRONTEND.md 词汇，目录分组学 macOS）：

```text
linux/
  app/          启动、生命周期、主窗口组装（今日 app.rs + window_bootstrap + lifecycle）
  chrome/       Sidebar、Overlay、CommandPalette、StatusBar、Keymap、SceneStack
  terminal/     PaneSurface、LayoutHost、scrollback
  ui/           Preferences、TargetConfig、Existing picker（今日 tmux_dialog）
```

`window.rs` 不再用 20 个 `#[path = "window_*.rs"]` 把实现缝进一个模块。
状态从 `window::UiState` 拆到 chrome / scene，AppShell 继续只做 widget 骨架。

不改：GTK 行为、Scene 常驻、EventPump 唯一消费者、像素契约。

---

## 3. 现状与偏移（量化）

### 3.1 `crate::runtime::` —— 多数是风格，少数是泄漏

全 `src/` 约 **149+** 处 `crate::runtime::`。拆开看：

| 种类 | 数量级 | 判罚 |
|---|---|---|
| provider 目录内部 `crate::runtime::tmux::protocol` 等 | 绝大多数 | 风格。可改 `super::`，不是本轮必须 |
| 外部用 trait：`Runtime` / `RuntimeBatch` / `RuntimeProvider` | workspace / catalog / muxterm | **正确** |
| **provider 目录之外**的 `crate::runtime::{tmux,herdr,shell}::` | **15 处** | **泄漏，本轮要删** |
| frontend | 1 处：`cli/format.rs` 测试 `use crate::runtime::mock::MockRuntime` | **泄漏** |

15 处具体名单（2026-09-11 调查）：

| 文件 | 用了什么 | 应该改成 |
|---|---|---|
| `catalog/mod.rs:408` | `HerdrSession::new` + `ping` + `workspace_create` | `RuntimeProvider` 的 create/discover；Catalog 不碰 session 类型 |
| `discovery/existing.rs:12` | `HerdrSession` | 删除平行 discovery，或下放到 `runtime/herdr` |
| `protocol/ffi/functions/runtime.rs` | `downcast_ref::<HerdrRuntime>()` 做 probe | `trait Runtime` 上的诊断方法，按 capability 返回 JSON / null |
| `protocol/ffi/functions/transport.rs` | `tmux::status::fetch_snapshot` | Workspace/Runtime 的 status API，或 provider 方法 |
| `protocol/ffi/functions/handle.rs` | `TmuxDriver::legacy_ssh_alias_and_tmux_socket`；`DaemonRuntime` | 走 registry id；daemon 是 shell 形态，经 ShellDriver |
| `protocol/ffi/api_tests.rs` | 直接 `TmuxRuntime` / `ShellRuntime` / `DaemonRuntime` | Catalog + registry，或 `runtime/` 内部集成测试 |
| `protocol/terminal/emulate.rs` 测试 | `tmux::protocol::ControlEscapeDecoder` | 测试搬进 `runtime/tmux`，或 decoder 留在 tmux |
| `workspace/spec.rs` 测试 | `downcast_ref::<TmuxRuntime/ShellRuntime>()` | 断言 `spec.runtime == "tmux"` + `support()`，不 downcast |

使泄漏**合法**的根因：

```text
src/core/runtime/mod.rs     pub mod tmux; pub mod herdr; pub mod shell;
src/core/runtime/tmux/mod.rs   pub use backend::TmuxRuntime;
src/core/transport/mod.rs   pub mod local; pub mod ssh;
src/lib.rs                  pub mod runtime; pub mod transport;
```

`docs/RUNTIME.md` §2 已经写了「具体类型不对外导出 / 禁止 `pub use tmux::TmuxRuntime` /
Catalog 不实现 `TmuxRuntime::new()`」。**文档对，代码没守。**

门禁现状：`check-architecture.sh` 只禁止 **frontend** 出现 `TmuxRuntime|HerdrRuntime|ShellRuntime`，
**不扫** `src/core` 里对 provider 子模块的引用。所以 Catalog / FFI 泄漏是绿的。

### 3.2 其它「外面看得见 tmux」

这些不是 `crate::runtime::tmux::` 路径，但是同一类偏移：

| 位置 | 现象 |
|---|---|
| `projects/target.rs` | `enum TargetRuntime { Shell, Tmux, Herdr }` 写死三种 |
| 同上 `TargetTransport::attach_backend` | local attach → `"tmux"`，ssh attach → `"tmux-ssh"`。复合 mode 残留 |
| `discovery/mod.rs` | 平行发现层：`TmuxSessionInfo`、`list_local_tmux_sessions`、re-export `ssh::config` |
| `catalog/mod.rs` | `tmux_worktree_session()` —— id 策略放 Catalog 可以，函数名绑死 tmux |
| `Runtime::as_any` | 专门为 downcast 留的后门 |
| FFI | `muxterm_discover_tmux_sessions_json`、`muxterm_workspace_herdr_probe_json` |
| frontend CoreBridge | `FfiClient::discover_tmux_sessions`；`new_connect("tmux", …)` |
| linux | `tmux_dialog.rs`；command palette `PaletteAction::TmuxAttach` |
| cli | `tmux_cli.rs` / `tmux_cli_exec.rs`；daemon 直接 create `"tmux"` |
| i18n | `CmdTmuxAttach`、`ChooseTmuxDirectory`、`local_tmux_sessions`；`ChooseWorkspace => "choose_tmux_session"` |

Catalog 用字符串 id `"tmux"` **是对的**。错的是 UI / FFI / Projects 把 id 编进类型和专用函数。

### 3.3 Transport 偏移（同类、更轻）

`crate::transport::` 约 85 处。大部分是 runtime provider 用 `TargetConnection` /
`ByteChannel` / `ChannelKind`——这是允许的（Runtime 必须看见通道契约，不许看见
local/ssh 类型）。

真正的偏移：

- `discovery/mod.rs` re-export `transport::ssh::config::{list_ssh_hosts, parse_ssh_config, SshHostEntry}`
- SSH host 列表应当是 `TransportProvider::list_targets`，经 Catalog 出去
- `transport/{local,ssh}` 仍是 `pub mod`，和 runtime 一样随时能被外面点名

### 3.4 CoreBridge：概念有了，结构没有

| 平台 | 现在 | 问题 |
|---|---|---|
| Rust 共享 | `frontend/ffi_client.rs` **3189 行**，类型名 `FfiClient` | 就是 CoreBridge，但既不叫这个名字，也不在 `utils/` |
| linux | ≥12 个文件 `use FfiClient`，同时 26 个 frontend 文件 `use crate::protocol` | 绕过 wrapper 去拿 Core 内部 id / `StateChange` |
| tui | `ffi_client` + `view_store`；测试仍碰 `protocol::ffi::types` | 同上 |
| cli | `FfiClient`，但 `format.rs` 直接吃 `protocol::{PaneId,TabId,layout,state}` + `MockRuntime` | CLI 没有把 CoreBridge 当唯一边界 |
| macos | `CoreBridge/CoreBridge.swift` **2127 行** + `shim.c` + `muxterm.h` | 这是唯一**名叫** CoreBridge 的平台，所以看起来「只有 macos 有」 |
| windows | 占位 | 本轮不落地，但目录要挂得上 `utils/` |

FRONTEND.md §3 表格把 Linux/TUI/CLI 的 FFI 口写成 `ffi_client`、把 macOS 写成
`CoreBridge`。这是两套名字，不是两套层。第二轮统一成 CoreBridge。

macOS 必须保留 Swift CoreBridge：Swift 不能调用 `frontend/utils/corebridge`。
要统一的是 **契约**（C ABI + owned DTO 形状），不是源文件。

### 3.5 i18n：有 catalog，没有覆盖

| 事实 | 数 |
|---|---|
| Rust catalog | `frontend/i18n/locales/{en,zh-CN}.json` 各 **202** key |
| macOS 第二份 | `macos/Resources/i18n/{en,zh-CN}.json` 各 **221** key（已经漂移 +19） |
| Swift key 枚举 | `macos/App/I18n.swift` **614 行**，与 Rust `TextKey` 平行维护 |
| 引用 Rust i18n 的文件 | **16**（linux 15 + tui/render.rs 1） |
| CLI | **0** |
| linux 76 个 rs 里不用 i18n 的 | **约 60** |
| 写死的按钮文案 | `"Close"` `"Cancel"` `"Bind…"` `"Unbind"` `"Discard"` `"New project…"`；同时 `pane_view.rs` 写死中文「复制/粘贴/上下切分」；`window_worktree.rs`「创建/取消」 |

i18n 模块本身（`TextKey` + JSON parity 测试）是好的。没用好的原因：

1. 不在 `utils/`，平台没有「必须经过这里」的约束
2. macOS 复制了一份 JSON + 另一套 enum
3. 大量 GTK 控件直接写死中英文字符串
4. key 仍按 tmux 命名（`cmd_tmux_attach`），和「外面只认 catalog id」冲突——文案可以继续说 tmux，**代码 key 应改成 attach/create/detach workspace**，具体 runtime 名用参数

### 3.6 Linux 结构 vs macOS

| | macOS | Linux 现在 |
|---|---|---|
| 顶层分组 | App / Chrome / CoreBridge / Terminal / UI | **平坦 76 个 rs** |
| FFI | `CoreBridge/` 一目录 | 散落 `FfiClient` + `crate::protocol` |
| 主窗口 | `App/MainWindow.swift` 组装 Chrome | `window.rs` **803 行** + **20 个 `#[path]` sibling**，合计 **6292 行** 缝在一个模块里 |
| 骨架 | Chrome 与 App 分开 | `app_shell.rs` 只拼 widget；业务状态仍在 `window::UiState` |
| 发现 UI | QuickConnect 走 Candidate | 仍有 `tmux_dialog.rs` |
| 文档承诺 | FRONTEND.md §5 要把 window/preferences/quickconnect 拆成 AppShell/Sidebar/SceneStack | 拆了文件名，**没拆模块边界** |

`docs/PROJECT-STRUCTURE.md` 还写着 `frontend/app_shell.rs`，实际 AppShell 在
`linux/app_shell.rs`，frontend 根上没有。文档已偏。

Linux 不是「没文件」，是「没有和 macOS 同构的层」。第二轮要的是分组，不是再切
20 个 `window_foo.rs`。

### 3.7 偏移总表

```text
目标                                          现在                         偏移
─────────────────────────────────────────────────────────────────────────────
runtime/{tmux,herdr,shell} crate-private      pub mod + pub use 具体类型   大
外面只用 trait + Catalog id                   15 处具体路径 + enum + FFI   大
transport/{local,ssh} crate-private           pub mod + discovery re-export 中
discovery 不是平行层                          core/discovery 仍列 tmux/herdr 中
as_any / downcast 不存在                      trait 上就有                 中
frontend/utils/corebridge                     ffi_client.rs 根上 + 仅 macOS 名叫 CoreBridge
                                              + 26 文件 crate::protocol    大
frontend/utils/i18n 全平台                    16 文件；macOS 第二份 JSON    大
linux App/Chrome/Terminal/UI                  平坦 + window 上帝对象        大
UI/FFI 不出现 tmux 专用函数                   discover_tmux_* / tmux_dialog 中
```

第一轮把「目录在哪」做完了。第二轮做「谁准看见谁」。

---

## 4. 非目标

- 不重写 tmux 控制协议、不新增 Runtime、不实现 windows GUI
- 不拆 Cargo workspace crate
- 不改 C 符号名 `muxterm_*`（可增通用 discover，旧 `discover_tmux_sessions` 标 deprecated 再删）
- 不改 C ABI 的 `StateChange` 摊平（第一轮冻结）
- 不重写 GTK / SwiftTerm 像素路径
- 不把 Swift UI 嵌进 CLI 进程
- 不对默认 tmux / Herdr 做破坏性命令
- 不把 `crate::runtime::{Runtime, RuntimeBatch}` 从 workspace 里拿掉——那是正确依赖
- 不要求 macOS 用 Rust `utils/corebridge` 源文件（语言不允许）

---

## 5. 第二轮锁定（实现时不要重开）

1. **默认 `mod`，禁止默认 `pub mod`。** 父模块只 `pub use` 列得出的接口。
   范本：`src/core/projects/mod.rs`。禁止 `pub use foo::*;`。
2. `runtime/{tmux,herdr,shell}` 与 `transport/{local,ssh}` **对 crate 其余部分 private**。
   Driver 类型 `pub(super)`。组合根只在两个 `registry.rs::with_builtins`。
3. 产品层识别 Runtime / Transport **只用 Catalog 注册的字符串 id**。禁止
   `TargetRuntime` 这类写死变体去分支能力；问能力走 `support()` / `RuntimeInfo`。
4. Catalog / Workspace / FFI **禁止** import 具体 Runtime/Transport 类型。
   禁止 `as_any` downcast。缺的能力按 [`docs/MODULE-SURFACE.md`](docs/MODULE-SURFACE.md) §3
   加 trait **默认方法**，不要把类型再 `pub`。
5. 发现只经 `RuntimeProvider::discover` + `TransportProvider::list_targets`。
   删除作为门面的 `core/discovery`（实现下沉到对应 provider）。
6. Rust frontend 唯一 FFI 口是 `frontend/utils/corebridge`（类型名 `CoreBridge`）。
   禁止 `use crate::protocol` / `use crate::runtime` / `use crate::transport`
   （`utils/corebridge` 碰 `protocol::ffi` 除外）。
7. 文案唯一源是 `frontend/utils/i18n`。macOS JSON 生成自这里。禁止控件写死中英文。
   i18n key 去 tmux 前缀；用户可见文案仍可含 “tmux”。
8. Linux 目录分组：`app/` `chrome/` `terminal/` `ui/`。
   `window.rs` 不再 `#[path]` 缝合实现。
9. CoreBridge 对 frontend 暴露的发现/打开 API 不带实现名：
   `discover_existing(runtime_id, …)` / `open(OpenRequest)`，没有
   `discover_tmux_sessions`。
10. `MockRuntime` 只 `pub(crate)` 给 `src/core` 测试。frontend 测试用 corebridge
    DTO。`tests/` 点名 `TmuxRuntime` 的迁进 provider 目录或改走 FFI；
    **不为测试把模块重新 pub**。
11. 单一 crate、C 符号、Scene/EventPump、不杀用户 tmux/Herdr：沿用第一轮。

关于 worktree session 名：第一轮已拍板 generic tmux worktree =
`{project_id}/{worktree_id}`。第二轮变成 `RuntimeProvider::worktree_session_name`。
Catalog 不再出现 `tmux_worktree_session` 这个名字。

---

## 6. 阶段（先关 `pub`，再用失败名单补接口）

规格在 [`docs/MODULE-SURFACE.md`](docs/MODULE-SURFACE.md) §6。每个 Wave 可独立
`cargo test --lib` / 增量 commit。英文 commit。

**方法：** 不要先把 trait 设计完美再改模块。关掉 `pub mod` → 看谁编不过 →
能改调用点就改；不能就按 MODULE-SURFACE §3 给 trait 加**默认方法** →
下一层。禁止把类型 `pub` 回去消红。

### Wave 1 — provider 子模块私有

`runtime/mod.rs`：`mod tmux; mod herdr; mod shell;`
`tmux/mod.rs`：子模块改 `mod`；只 `pub(super) use TmuxDriver`；删 `pub use TmuxRuntime`。
herdr / shell / `transport/{local,ssh}` 同样。

修 MODULE-SURFACE §2.1 那 15 处。Catalog herdr create / FFI status / herdr probe
在这一波就把 §3 的三个默认方法加上，让调用点改走 trait。

`cargo test --lib` 必须绿。`tests/` 里点名 `TmuxRuntime` / `HerdrSession` 的会红：
把它们迁进 provider 目录或改走 FFI，**不要**把 `pub mod tmux` 加回来。

architecture 脚本：具体路径不得逃出 provider 目录。

### Wave 2 — 父模块精确 `pub use`

`workspace` / `catalog` / `config` / registry 按 MODULE-SURFACE §4。
config 去掉 glob `pub use`。调用点改为 `workspace::WorkspacePool` 这种短名。

### Wave 3 — crate 根 `pub(crate)` + 收窄 `test_support`

`lib.rs` 生产对外只留 `pub mod ffi`。`test_support` 改成窄门面（平台 E2E 夹具 +
Catalog/Muxterm），不再 `pub use` 全部 core。

### Wave 4 — 统一 `frontend/utils/{corebridge,i18n}`

`ffi_client.rs` → `utils/corebridge`，类型名 `CoreBridge`。
`i18n/` → `utils/i18n`。砍 frontend 26 处 `crate::protocol`。
发现 API 去 `discover_tmux_sessions`。macOS JSON 同源。补写死文案。

更新 `TASKS.md` / `FRONTEND.md`：共享层路径从 `ffi_client.rs` 改为
`frontend/utils/corebridge`。

### Wave 5 — Linux 四层

`linux/{app,chrome,terminal,ui}`。拆掉 `window.rs` 的 `#[path]`。
`tmux_dialog` → existing picker。只引用 utils + EventPump/CommandQueue/ViewStore。

macOS 只对齐 DTO/JSON，目录已分层。TUI/CLI 只改 import。

### Wave 6 — 文档与门禁

`FRONTEND.md`、`PROJECT-STRUCTURE.md`、`RUNTIME.md`、`CATALOG.md`、`TASKS.md`、
`scripts/check-architecture.sh` 与 MODULE-SURFACE §7 对齐。

---

## 7. 验收命令（每阶段结束）

```bash
cargo fmt
cargo check --features tui,gtk
cargo test --lib --features tui
scripts/check-architecture.sh
```

新增门禁（Phase A 起逐步打开）：

```bash
# 具体 Runtime 不得逃出 provider 目录
rg -n 'crate::runtime::(tmux|herdr|shell)::' src --glob '*.rs' \
  | rg -v 'src/core/runtime/(tmux|herdr|shell)/'

# frontend 不得碰 Core 内部
rg -n 'crate::(protocol|runtime|transport|workspace|catalog|muxterm)::' src/frontend \
  --glob '*.rs' | rg -v 'src/frontend/utils/corebridge/'
```

tmux / Herdr 测试纪律不变：只 `-L muxterm-test-*` / named `muxterm-test-*`。

---

## 8. 建议施工顺序与切分

不要一个 commit 里同时改可见性、搬 Linux、改 i18n。

1. Wave 1a：`mod tmux/herdr/shell` + `src/` 那 15 处 + 三个 trait 默认方法
2. Wave 1b：`tests/` 迁走具体类型；architecture 脚本
3. Wave 2：workspace/catalog/config 精确导出
4. Wave 3：lib.rs `pub(crate)` + 收窄 test_support
5. Wave 4a：搬家 utils（纯 mv）
6. Wave 4b：砍 `crate::protocol`；CoreBridge API 去 tmux 名；i18n 同源
7. Wave 5：Linux 四层（可按 app → chrome → terminal → ui 四个 commit）
8. Wave 6：文档

估计：Wave 1 最大（可见性 + trait + 测试搬家）；Wave 4 中等偏大；Wave 5 文件多但应零行为变化。

---

## 9. 已拍板的命名

用户已确认三点方向。实现时不再问：

1. Rust 主类型叫 **`CoreBridge`**，目录 `frontend/utils/corebridge`。
   旧名 `FfiClient` 留 type alias 一个 commit 后删。
2. i18n key **本轮去掉 tmux 前缀**。JSON 文案仍可写 “Attach tmux session”。
3. 施工从 **Wave 1：先去掉 `pub mod`** 开始，不先做 Linux 搬家。

其余 §5 作为第二轮锁定。

---

## 10. 和第一轮文档的关系

| 文档 | 第一轮 | 第二轮 |
|---|---|---|
| `TASKS.md` | 施工已完成 | 追加 Phase A–E；改 ffi_client 路径锁 |
| `docs/*` | 产品树正确 | 把「具体类型不导出 / 共用 CoreBridge」写成与代码一致 |
| `REFACTOR-PLAN.md` | 讨论档案（gitignored） | 不再改 |
| `REFACTOR-PLAN-2.md` | — | 本文。拍板后施工以 TASKS.md 为准 |

未拍板前不要开 Phase A。拍板后从 A1 一个可验证 commit 开始。
