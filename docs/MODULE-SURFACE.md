# MODULE-SURFACE.md — 模块可见性与对外接口

产品树：[`WORKSPACE.md`](WORKSPACE.md)。Runtime：[`RUNTIME.md`](RUNTIME.md)。
前端：[`FRONTEND.md`](FRONTEND.md)。施工计划：[`../REFACTOR-PLAN-2.md`](../REFACTOR-PLAN-2.md)。

调查时间：2026-09-11T15:57:10+08:00（CST）。

**一句话：** 默认 `mod`，不要 `pub mod`。父模块只 `pub use` 接口类型。
具体实现关在自己的目录里。编译器倒逼每一个外部引用，用来补 trait，而不是把类型再 `pub` 回去。

---

## 1. 规则

### 1.1 默认可见性

| 写法 | 何时用 |
|---|---|
| `mod foo;` | **默认。** 子模块对父模块私有 |
| `pub use foo::{Trait, Type};` | 父模块对外的接口。列得出名字才导出 |
| `pub(super) use foo::Driver;` | 只给父模块的组合根（例如 `registry.rs`） |
| `pub(crate) use foo::MockRuntime;` | 只给同 crate 的 core 单测 |
| `pub mod foo;` | **禁止作为默认。** 只有 C ABI（`ffi`）和明确的测试门面 |

禁止：

- `pub mod tmux;` 再让外面写 `crate::runtime::tmux::TmuxRuntime`
- `pub use foo::*;`（`config/mod.rs` 现在就是这样）
- 为了让某一个调用点编过，把整个子模块重新 `pub`

好的范本已经在树里：[`src/core/projects/mod.rs`](../src/core/projects/mod.rs)
是 `mod project;` + `pub use project::Project;`。第二轮所有 `mod.rs` 向它对齐。

### 1.2 谁可以看到谁

```text
src/lib.rs
  pub mod ffi                         唯一稳定对外 ABI（C 符号 muxterm_*）
  pub(crate) mod {runtime, transport, workspace, catalog, ...}
  pub(crate) mod frontend
  #[doc(hidden)] pub mod test_support 仅测试；逐步收窄，见 §6

runtime/mod.rs
  对外（crate 内）：Runtime, RuntimeProvider, RuntimeRegistry, RuntimeSpec,
                    RuntimeBatch, RuntimeCapability, RuntimeError, ...
  私有：mod tmux; mod herdr; mod shell; mod mock;
  组合根：registry.rs 用 super::tmux::provider::TmuxDriver（TmuxDriver 为 pub(super)）

transport/mod.rs 对称：ByteChannel / TargetConnection / TransportProvider / Registry
  私有：mod local; mod ssh;

frontend/
  平台只引用 utils/{corebridge,i18n} + event_pump / command_queue / view_store
  禁止 crate::protocol / crate::runtime / crate::workspace / crate::catalog
```

`src/main.rs` 与 lib 同一 crate，所以 `pub(crate) mod frontend` 足够启动。
`tests/*.rs` 是**另一个 crate**，只能看见 `pub` 项。这是 `test_support` 存在的原因，
不是把 `runtime::tmux` 做成生产 API 的理由。

### 1.3 现在有多敞

2026-09-11 扫描：`src/` 里 **187** 个 `pub mod`、**87** 个 `pub use`。

crate 根（`lib.rs`）把 core 的每一层都 `pub mod` 了。`runtime/`、`workspace/`、
`protocol/`、`linux/` 的子模块几乎全部 `pub mod`。`projects/` 是少数已经按
「私有模块 + 精确导出」写的层。

`crate::<layer>` 引用量（同一次扫描）：

| 层 | core 内 | frontend | 含义 |
|---|---|---|---|
| protocol | 323 | 46（26 个文件） | frontend 在绕过 FFI wrapper |
| runtime | 160 | 1（CLI 测试 MockRuntime） | 多数合法 trait；少数具体类型泄漏 |
| frontend | 0 | 394 | 平台内部互引，linux 未分层 |
| transport | 86 | 0 | 多数是通道契约；ssh config 从 discovery 漏出 |
| workspace | 60 | 0 | 经 `workspace::pool::` 点名子模块 |
| projects | 43 | 0 | 导出面相对干净 |
| config | 34 | 0 | glob `pub use` |
| catalog | 20 | 0 | 还 `pub mod inventory/resolver` |

---

## 2. 去掉 `pub mod` 之后，外面到底在用什么

下面不是猜测，是把「子模块变成私有」后**会编不过**的外部点。这些点用来决定
trait 补什么方法，而不是决定把模块再公开。

### 2.1 具体 Runtime / Transport（第一刀）

`crate::runtime::{tmux,herdr,shell}::` 在 provider 目录之外 **15 处**：

| 调用点 | 现在用的具体类型 | 倒逼出的接口 |
|---|---|---|
| `catalog/mod.rs` Herdr create | `HerdrSession::new` + `ping` + `workspace_create` | `RuntimeProvider::create_identity` |
| `discovery/existing.rs` | `HerdrSession` | 删除平行 discovery；走 `RuntimeProvider::discover`（已有） |
| `ffi/functions/runtime.rs` herdr probe | `downcast_ref::<HerdrRuntime>()` | `Runtime::diagnostics(pane) -> Option<Value>`，默认 `None` |
| `ffi/functions/transport.rs` status | `tmux::status::fetch_snapshot` | `RuntimeProvider::status_snapshot` |
| `ffi/functions/handle.rs` | `TmuxDriver::legacy_ssh_alias_and_tmux_socket`；`DaemonRuntime` | 遗留构造改走 Catalog id；daemon 是 shell 形态 |
| `ffi/api_tests.rs` | 直接 `TmuxRuntime` / `ShellRuntime` | 改 Catalog+registry，或搬进 provider 目录测试 |
| `protocol/terminal/emulate.rs` 测试 | `tmux::protocol::ControlEscapeDecoder` | 测试搬进 `runtime/tmux` |
| `workspace/spec.rs` 测试 | downcast `TmuxRuntime` / `ShellRuntime` | 断言 `spec.runtime` id + `support()` |
| `runtime/mod.rs` 自身测试 | 构造三种 Runtime | 留在 `runtime/` 内部，合法 |

`crate::runtime::mock` 被 workspace / catalog 单测使用（合法 `pub(crate)`），
以及 `frontend/cli/format.rs` 测试（不合法，改 DTO fixture）。

`crate::transport::ssh` 被 `discovery/mod.rs` re-export（不合法）。
`transport/connection.rs` 点名 `local` / `ssh` 具体 process transport：这是
transport 层内部适配，允许，但 `local`/`ssh` 模块本身仍应 private。

反向依赖：`runtime/herdr/provider.rs` 调 `crate::discovery::existing::discover_ssh_herdr`。
发现实现应下沉到 herdr，Catalog 只调 `RuntimeProvider::discover`。

### 2.2 组合根（保留，但不要 `pub mod`）

这些是**正确**的跨层依赖，去掉 `pub mod` 之后应改成父模块导出的类型名：

| 调用方 | 现在 | 改为 |
|---|---|---|
| `muxterm.rs` / `catalog/mod.rs` | `runtime::registry::RuntimeRegistry` | `crate::runtime::RuntimeRegistry`（`pub use registry::RuntimeRegistry`） |
| 同上 | `transport::registry::{TransportRegistry, ConnectionRegistry}` | `crate::transport::{TransportRegistry, ConnectionRegistry}` |
| `workspace/*` | `runtime::{Runtime, RuntimeBatch, ...}` | 不变，这就是接口 |
| `projects/service.rs` | `transport::ChannelRequest` | 不变 |

`RuntimeRegistry::with_builtins` / `TransportRegistry::with_builtins` 是**唯一**
允许写下 `TmuxDriver` / `HerdrDriver` / `ShellDriver` / `LocalTransport` /
`SshTransport` 的地方。Driver 类型 `pub(super)`，不进 `runtime/mod.rs` 的 `pub use`。

### 2.3 `test_support`（最大的隐形 `pub`）

`lib.rs` 的 `pub mod test_support` 把 core 全部 `pub use` 出去。
`tests/` 里 **81** 个文件走这条路。其中直接点名具体实现的包括：

- `runtime::tmux::backend::TmuxRuntime`（tmux 集成 / sendkeys / split / control conformance）
- `runtime::tmux::protocol::Message` / `tmux::client` / `tmux::command`
- `runtime::herdr::{HerdrRuntime, HerdrSession, observe, wire, forward}`
- `runtime::shell::{ShellRuntime, daemon, daemon_client}`
- `transport::ssh`
- `discovery::existing`
- `workspace::{spec, pool, workspace, terminal_model}`

Rust 要点：`tests/*.rs` 是独立 crate，**看不到** `pub(crate)`，也看不到
库编译时的 `#[cfg(test)]` 私有项。所以：

1. 测协议/后端的测试，搬进 `src/core/runtime/{tmux,herdr,shell}/` 当单元测试。
2. 测产品行为的测试，只经 `ffi` / `frontend::utils::corebridge`。
3. Linux E2E 需要 `AppWindow`：留一个**窄**的 `test_support::platform`，只导
   平台测试夹具，不导 `TmuxRuntime`。
4. 禁止靠「把 tmux 模块重新 pub」来喂 `tests/`。

### 2.4 frontend 现在越界的 Core 引用

26 个文件 `use crate::protocol`。分类：

| 允许（第二轮之后） | 不允许 |
|---|---|
| `frontend/utils/corebridge/**` 碰 `protocol::ffi`（它就是 C ABI wrapper） | linux `window*.rs` / `layout_host.rs` / `view_store.rs` 直接拿 `PaneId` / `WorkspaceId` / `StateChange` |
| — | CLI `format.rs` 拿 `protocol::{layout,state,command}` + `MockRuntime` |
| — | CLI `daemon.rs` 拿 `protocol::daemon::{Request,Response}`（daemon 协议应经 corebridge 或留在 core，由 FFI/socket 说） |

另：`frontend` 对 `crate::fault` / `crate::logging` 的引用发生在 binary 入口
（同 crate，`pub(crate)` 即可），不要变成 CoreBridge 的业务 API。

---

## 3. 倒逼后的接口（设计，不是再导出类型）

### 3.1 `trait RuntimeProvider`

已有：`id` / `name` / `support` / `channel_requirements` / `namespaces` /
`discover` / `new_instance`。

补上被具体类型堵住的洞：

```rust
fn create_identity(
    &self,
    connection: &dyn TargetConnection,
    spec: &RuntimeSpec,
) -> RuntimeResult<RuntimeSpec> {
    Err(RuntimeError::Unsupported { operation: "CreateIdentity" })
}

fn status_snapshot(
    &self,
    connection: &dyn TargetConnection,
    session: &str,
) -> RuntimeResult<serde_json::Value> {
    Err(RuntimeError::Unsupported { operation: "StatusSnapshot" })
}

fn worktree_session_name(&self, project_id: &str, worktree_id: &str) -> Option<String> {
    let _ = (project_id, worktree_id);
    None
}
```

- Catalog 的 herdr create 只调 `create_identity`，不再 `HerdrSession::new`。
- FFI `muxterm_status_snapshot_json` 经 Catalog/`registry.get(id)` 调
  `status_snapshot`，不再 `tmux::status`。
- 第一轮的 `{project}/{worktree}` session 名变成 provider 可选策略；
  Catalog 不再出现 `tmux_worktree_session` 这个函数名。tmux provider 返回
  那个字符串；其它 provider 返回 `None`。

id 是 `&'static str`（`"tmux"` / `"herdr"` / `"shell"`）。这就是外面唯一能
提到 tmux 的方式。

### 3.2 `trait Runtime`

已有：`connect` / `execute` / `drain_events` / `shutdown` / `support` /
worktree 三个 / `status_subscriptions_active` / `traffic_bytes` /
`set_foreground`。

改：

```rust
// 删除
fn as_any(&self) -> &dyn Any;

// 增加（默认空）
fn diagnostics(&self, pane: PaneId) -> Option<serde_json::Value> {
    let _ = pane;
    None
}
```

FFI herdr probe 调 `diagnostics`；非 herdr 返回 `{ok: true, probe: null}`。
**禁止 downcast。**

`status_subscriptions_active` 和 `traffic_bytes` 已经是 trait 方法，FFI
`runtime.rs` 里那两段是正确用法，保留。

### 3.3 `trait TransportProvider`

已有：`id` / `list_targets` / `connect` / `supported_channels`。

SSH host 列表走 `list_targets`，不要 `discovery` re-export `ssh::config`。
`SshHostEntry` 若 FFI 还需要，变成 Catalog/FFI DTO，不从 `transport::ssh` 漏出。

### 3.4 不要补进 trait 的东西

| 现象 | 处理 |
|---|---|
| `workspace/spec` 测试 downcast | 改断言，不给 trait 加 `kind()` |
| emulate 用 `ControlEscapeDecoder` | 测试搬家 |
| CLI 用 `MockRuntime` | frontend 测试用 corebridge DTO |
| `TargetRuntime` 三变体 | 改 Catalog id 字符串；`attach_backend() → "tmux-ssh"` 删除 |
| `as_any` | 删，不留 cfg(test) 后门给 `tests/` 用（它们也看不见） |

---

## 4. 各层 `mod.rs` 目标面

只列**对外名字**。未列出的子模块全部 `mod`，不 `pub mod`。

### 4.1 `src/lib.rs`

```text
pub(crate) mod activity;
pub(crate) mod buffer_cap;
pub(crate) mod catalog;
pub(crate) mod config;
pub(crate) mod executable;
pub(crate) mod fault;
pub(crate) mod logging;
pub(crate) mod muxterm;          // cfg(feature = "ffi")
pub(crate) mod projects;
pub(crate) mod protocol;
pub(crate) mod render_policy;
pub(crate) mod runtime;
pub(crate) mod transport;
pub(crate) mod url_detect;
pub(crate) mod workspace;
pub(crate) mod frontend;

pub mod ffi { pub use crate::protocol::ffi::*; }

// 删除作为门面的 discovery（实现下沉到 provider）
// test_support：见 §6，从「全量 pub use」收到窄门面
```

### 4.2 `runtime/mod.rs`

```text
mod batch;
mod capability;
mod contract;
mod error;
mod mock;
mod provider;
mod registry;
mod herdr;    // 私有
mod shell;    // 私有
mod tmux;     // 私有

pub use batch::{ControlEvent, RenderEvent, RuntimeBatch, RuntimeSignal};
pub use capability::RuntimeCapability;
pub use contract::{Runtime, RuntimeSpec, WorktreeCreateSpec, WorktreeInfo};
pub use error::{RuntimeError, RuntimeResult};
pub use provider::{runtime_supports_channels, RuntimeInfo, RuntimeProvider};
pub use registry::RuntimeRegistry;
pub(crate) use mock::MockRuntime;
```

`tmux/mod.rs`：`mod backend; mod client; ...`，`pub(super) use provider::TmuxDriver;`。
**不要** `pub use backend::TmuxRuntime`。herdr / shell 同样。

### 4.3 `transport/mod.rs`

```text
mod connection;
mod connection_registry;
mod local;
mod provider;
mod registry;
mod ssh;

pub use connection::Connect;
pub use connection_registry::ConnectionRegistry;
pub use provider::{TargetInfo, TransportInfo, TransportProvider};
pub use registry::{with_builtins, TransportRegistry};
pub use ChannelKind / ChannelRequest / ByteChannel / TargetConnection / ...（已在 mod.rs 定义的 trait）
```

`local` / `ssh` 对 transport 父模块 `pub(super)` 出 Driver 即可。
`connection.rs` 作为内部适配可以点名具体 process transport。

### 4.4 `workspace/mod.rs`

现在全是 `pub mod pool` 等，调用方写 `workspace::pool::WorkspacePool`。
改成：

```text
mod pane_buf;
mod pool;
mod provenance;
mod spec;
mod template;
mod template_apply;
mod terminal_model;
mod workspace;

pub use pool::WorkspacePool;
pub use provenance::...;
pub use spec::WorkspaceSpec;
pub use template::{TemplateName, ...};
pub use workspace::Workspace;
```

`pane_buf` / `template_apply` / `terminal_model` 能不导出就不导出。

### 4.5 `catalog/mod.rs`

```text
mod inventory;
mod resolver;
pub use resolver::{OpenRequest, ResolveIntent, ResolveError, ...};
// Catalog 类型留在 mod.rs
```

不要 `pub use crate::runtime::RuntimeProvider` 当 catalog 的类型（调用方从
`runtime` 取 trait，或 catalog 只暴露 `RuntimeInfo` 只读视图）。

### 4.6 `config/mod.rs`

把 `pub mod action_catalog` 等改成 `mod`，把 `pub use action_catalog::*` 改成
列得出的类型名单。禁止 glob。

### 4.7 `protocol/`

对 crate 内：ids、`candidate`、`task`、`layout`、`state`（core 用）。
对 crate 外：只经 `pub mod ffi`。frontend 除 `utils/corebridge` 外禁止
`crate::protocol`。

`protocol/ffi/functions` 可以继续 `pub mod` 给 `ffi/api.rs` 聚合 C 符号——
那是 ABI 面，不是产品层。但 functions 里**禁止** import `runtime::tmux`。

### 4.8 frontend

```text
frontend/mod.rs
  pub(crate) mod utils;          // corebridge + i18n
  pub(crate) mod command_queue;
  pub(crate) mod event_pump;
  pub(crate) mod view_store;
  pub(crate) mod cli;
  pub(crate) mod tui;            // cfg
  pub(crate) mod linux;          // cfg gtk
  pub(crate) mod macos;          // cfg
  pub(crate) mod windows;
  // mirror / mouse / ssh_probe / url_opener / format：能收进 linux/cli 就收

frontend/utils/mod.rs
  pub mod corebridge;            // 平台共享 FFI 口（今日 ffi_client.rs）
  pub mod i18n;                  // 唯一文案 catalog
```

Rust 主类型建议叫 `CoreBridge`（与目录、Swift、文档一致）。可留
`pub type FfiClient = CoreBridge` 一个 commit 后删。

macOS Swift `macos/CoreBridge` **不是**第二套协议：它是同一 C ABI 的语言适配。
DTO 名字与 `utils/corebridge` 对齐。JSON 从 `utils/i18n/locales/` 复制/生成，
禁止手改 `macos/Resources/i18n`。

linux 对外（给 `app.rs` / E2E 窄门面）只需要 `AppWindow` 之类入口。
内部 `mod app; mod chrome; mod terminal; mod ui;`。

---

## 5. 统一 utils + Linux 分层

### 5.1 统一 CoreBridge / i18n

所有 Rust 平台（cli / tui / linux / 将来 windows）只引用：

```text
crate::frontend::utils::corebridge
crate::frontend::utils::i18n
```

再加上通用状态机：`event_pump` / `command_queue` / `view_store`。

CoreBridge 对平台暴露的发现/打开 **不带实现名**：

```text
discover_existing(runtime_id, transport_id, target, socket)
open(OpenRequest)
```

没有 `discover_tmux_sessions`。旧函数最多转发一个 commit。

i18n：`TextKey` + 一份 JSON。控件禁止写死 `"Close"` / `"复制"`。
key 去 tmux 前缀（`cmd_attach_workspace`）；用户可见文案仍可含 “tmux”。

### 5.2 Linux 分层（对标 macOS）

```text
linux/
  app/         启动、生命周期、主窗口组装
  chrome/      Sidebar、Overlay、CommandPalette、StatusBar、Keymap、SceneStack
  terminal/    PaneSurface、LayoutHost、scrollback
  ui/          Preferences、TargetConfig、Existing picker（今日 tmux_dialog）
```

`window.rs` 不再 `#[path = "window_*.rs"]`。`UiState` 按层拆，AppShell 只拼
widget。行为不变。

---

## 6. 施工波次（先关可见性，再补接口）

**不要**先把 trait 设计完美再关模块。顺序是：关 `pub` → 编译失败名单 →
能用现有 trait 就改调用点；不能就按 §3 **加默认方法** → 再关下一层。

### Wave 1 — provider 子模块私有（最大杠杆）

1. `runtime/mod.rs`：`mod tmux; mod herdr; mod shell;`
2. `tmux/mod.rs`：子模块改 `mod`；只 `pub(super) use provider::TmuxDriver`；
   删 `pub use TmuxRuntime`
3. herdr / shell 同样
4. `transport/mod.rs`：`mod local; mod ssh;`
5. 修 §2.1 那 15 处：能改调用点就改；Catalog/FFI 先加 §3 的默认方法让它们编过
6. `check-architecture.sh`：
   `crate::runtime::(tmux|herdr|shell)` 只许出现在对应目录

`test_support` 此波会红：`tests/` 里所有 `runtime::tmux::backend::TmuxRuntime`
将编不过。处理顺序：

- 先让 `src/` 绿（`cargo test --lib`）
- `tests/` 里点名具体 Runtime 的，迁到 provider 目录或改走 FFI
- 不要为了 `tests/` 把 `pub mod tmux` 加回来

### Wave 2 — 父模块改精确 `pub use`

`workspace` / `catalog` / `config` / `runtime`（registry）/ `transport`（registry）
按 §4 改。调用点从 `workspace::pool::WorkspacePool` 改成 `workspace::WorkspacePool`。
config 去掉 glob。

### Wave 3 — crate 根 `pub mod` → `pub(crate) mod`

`lib.rs` 按 §4.1。生产对外只留 `ffi`。`main.rs` 不受影响。
`test_support` 必须同时改成窄门面，否则 `pub use crate::runtime` 会把
`pub(crate)` 再漏到 `tests/`——Rust 不允许这种升级，会直接报隐私错误。

### Wave 4 — frontend utils + 禁止 `crate::protocol`

搬家 `ffi_client` → `utils/corebridge`，`i18n` → `utils/i18n`。
砍 26 个 frontend `crate::protocol`。类型改名 `CoreBridge`。

### Wave 5 — Linux 四层

按 §5.2 搬家。`linux/mod.rs` 只声明 `app/chrome/terminal/ui`。
E2E 经窄 `test_support::platform::linux`。

每波结束：`cargo fmt`、`cargo test --lib --features tui`、对应 `tests/`、
`scripts/check-architecture.sh`。

---

## 7. 门禁（目标）

```bash
# 具体实现不得逃出 provider 目录
rg -n 'crate::runtime::(tmux|herdr|shell)::' src --glob '*.rs' \
  | rg -v 'src/core/runtime/(tmux|herdr|shell)/'

rg -n 'crate::transport::(local|ssh)::' src --glob '*.rs' \
  | rg -v 'src/core/transport/(local|ssh)/' \
  | rg -v 'src/core/transport/connection.rs'

# frontend 不得碰 Core 内部（corebridge 除外）
rg -n 'crate::(protocol|runtime|transport|workspace|catalog|muxterm)::' \
  src/frontend --glob '*.rs' \
  | rg -v 'src/frontend/utils/corebridge/'

# 禁止 glob 再导出
rg -n 'pub use .*::\*;' src --glob '**/mod.rs'

# 默认禁止 pub mod（白名单：ffi、utils/{corebridge,i18n}、test_support）
```

白名单要短。新的 `pub mod` 必须在 review 里解释为什么不能 `mod` + `pub use`。

---

## 8. 非目标

- 不把 `crate::runtime::{Runtime, RuntimeBatch}` 从 workspace 拿掉
- 不为了测试把具体类型重新导出
- 不改 C 符号 `muxterm_*`
- 不重写 tmux 协议、不重写 GTK 像素路径
- 不拆 Cargo crate

---

## 9. 与其它文档

| 文档 | 关系 |
|---|---|
| [`RUNTIME.md`](RUNTIME.md) §2 | 已写「具体类型不对外导出」；本文是可见性落地 |
| [`FRONTEND.md`](FRONTEND.md) | 共享层路径改为 `frontend/utils/{corebridge,i18n}` |
| [`../REFACTOR-PLAN-2.md`](../REFACTOR-PLAN-2.md) | 第二轮施工顺序；以本文为接口规格 |
| [`../TASKS.md`](../TASKS.md) | 拍板后把 Wave 1–5 写进去 |
