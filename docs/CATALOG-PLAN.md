# CATALOG-PLAN.md — Catalog 施工单（Codex）

> 日期：2026-08-17（`2026-08-17T22:19:47+08:00`）
> 工作目录：`/home/wlz/Developer/self/muxterm`
> 分支：`feature/runtime/support_herdr`（HEAD 在写本文时为 `b739494`）
> 契约：[`CATALOG.md`](CATALOG.md)（先读完）。产品树：[`WORKSPACE.md`](WORKSPACE.md) §6。Runtime：[`RUNTIME.md`](RUNTIME.md)。
> 测试：[`TESTING.md`](TESTING.md) §5.12（W21）+ §5.14（Catalog）。结构：[`PROJECT-STRUCTURE.md`](PROJECT-STRUCTURE.md)。
> 滚轮漏项：[`W21-PLAN.md`](W21-PLAN.md)（上一轮 goal 跳过了，用户 Mini 上 shell/agent 滚轮都坏）。
>
> **你是实现 agent。每个 Cx 先确认测试是红的，再写最小实现到绿。禁止改断言、token、widget_name、错误文案子串来「绿」。禁止 `#[ignore]`。禁止 `git add -A`。禁止 Co-authored-by。禁止 push。禁止连用户默认 Herdr。禁止 `herdr server stop`。禁止对默认 tmux `kill-server`。生产代码禁止 `Command::new("herdr")`。GUI / Pool 禁止 `if spec.runtime == "herdr"`。`fbc77e4` 必须仍是祖先。live 路径禁止 `visible_ansi` → `vte.reset`。不要等用户确认。不要提前把 goal 标 complete。**

Cursor 已经落下：`docs/CATALOG.md`、本文件、结构/契约补丁、以及 `src/core/catalog/` **类型表面 + 单测**。内置 Driver 还没注册，所以 `with_builtins_*` 和两则 pool 源码断言现在是 **红的**——那是门禁，不是要你改测试。

W19 / W20 已在 HEAD。**W21 还没做**（仓库里没有 `scroll_policy.rs` / `linux_scroll_wheel_e2e.rs`）。先 W21，再 Catalog。不要把 Catalog 写进 emulate/滚轮。

---

## 0. 隔离（违反=事故）

| 允许 | 禁止 |
|---|---|
| `tmux -L muxterm-test-* …`；清理同 `-L kill-server` | 不带 `-L` 的 `kill-server` / `kill-session` / `kill-pane` |
| `herdr --session muxterm-test-*`；Drop 时 `session stop` + `session delete` | `herdr server stop`；连 `/home/wlz/.config/herdr/herdr.sock` |
| 夹具 `IsolatedTmux` / `IsolatedHerdr` | 测试打用户默认 server 冒充发现 |
| SSH 测用 `LoopbackSshd` | GTK 线程同步 `ssh` |

`with_builtins()` **只注册插件，不 connect、不探用户 socket**。

---

## C0 — W21 滚轮（先做，独立 commit）

规格整份：[`W21-PLAN.md`](W21-PLAN.md)。上一轮 goal 写了「W21 在 W20 之前」，实现跳过了。用户 2026-08-17 Mini dogfood：tmux attach 后 **shell 看历史、agent/htop alt-screen 把滚轮交给应用** 都不工作。

根因：`apply_mirror_policy` 把 `enable-fallback-scrolling=false`，又在每次 output 后灌 `DISABLE_MOUSE_TRACKING`。旧测试只 `vadjustment.set_value`，所以一直绿。

本步：

1. 按 W21-PLAN **先写红测试**（`wheel_action` 单测 + `test_emit_scroll` + `tests/linux_scroll_wheel_e2e.rs`）。
2. 最小实现到绿。`scroll_on_output` **保持 false**。不要 replica dump。
3. 独立 commit：`fix(linux): route wheel to VTE history or CSI arrows`

门禁：

```
cargo test --lib wheel_action -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_scroll_wheel_e2e -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_scroll_lock_e2e -- --test-threads=1
```

W21 未绿 **不要**开始 C2（改 builtin / pool / GUI）。C1 的 catalog 单测已经在树里，C0 不要去改那些断言。

---

## C1 — 类型表面（已在树里，不要重写）

已有模块：

```
src/core/catalog/mod.rs
src/core/catalog/connect.rs
src/core/catalog/driver.rs
src/core/catalog/transport.rs
src/core/catalog/inventory.rs
```

已实现（锁 API，保持绿）：

- 插件表是 **Vec**：`runtime_list()` / `transport_list()` = 登记顺序，不要 rank / sort
- `list_order_follows_registration`
- `register_runtime` / `register_transport` / `connect()` 缓存 `Arc<Connect>`
- `discover_sessions` 扇出到已注册、且接受该 transport 的 Driver；单个 Driver `Err` 跳过
- `Catalog::open`：未知 runtime 拒绝；不接受的 transport 拒绝；走 `Driver.open` + `pool.open`；**不**调用 `WorkspaceSpec::build_runtime`

已红（你要绿的）：

| 测试 | 现在为什么红 |
|---|---|
| `with_builtins_runtime_list_is_tmux_herdr_shell` | `with_builtins()` 还是空表 |
| `with_builtins_transport_list_is_local_ssh` | 同上 |
| `with_builtins_herdr_reports_worktree_caps` | 没有 HerdrDriver |
| `with_builtins_shell_rejects_ssh_pair` | 没有 ShellDriver |
| `pool_must_not_special_case_herdr_runtime_string` | `pool.rs` 仍有 `if spec.runtime == "herdr"` |
| `pool_must_not_hold_herdr_sessions_sidecar` | 仍有 `herdr_sessions` 字段 |
| `refresh_inventory_marks_unreachable_without_opening` | `refresh_inventory` 还是 no-op |

**不要**为了让源码断言变绿去改测试字符串。去改 `pool.rs` / 实现 `with_builtins`。

跑：

```
cargo test --lib catalog:: -- --test-threads=1
```

---

## C2 — 内置 Driver / Transport（`with_builtins`）

新建 `src/core/catalog/builtin/`（或等价路径），包装**现有**实现，不要复制 tmux 协议解析。

| 插件 | 包装 | `list` / `open` |
|---|---|---|
| `TmuxDriver` | `TmuxRuntime` | list = 现有 `list_local_tmux_sessions` / SSH 下 `list_ssh_tmux_sessions`（走 Connect，不要 Runtime 里再拼 `ssh` 字符串） |
| `HerdrDriver` | `HerdrRuntime` + `HerdrSession` | list = 现有 `discover_local_herdr` / SSH Herdr 列表；open = `HerdrRuntime::new(session_from_connect, path)` |
| `ShellDriver` | `ShellRuntime` | list 可空；open = `ShellRuntime::new`；`accepted_transports = ["local"]` |
| `Local` | 现有 local transport | `list_targets` 一个 target `""`；`connect` → `Connect` local |
| `Ssh` | 现有 ssh config + 管道 | `list_targets` = `~/.ssh/config` Host；`connect(alias)` → 可复用 Connect |

`with_builtins()`：按下面顺序 `register_*`（表是数组，`runtime_list()` / `transport_list()` 原样返回，不要再 sort）：

- runtime：`tmux`，`herdr`，`shell`（不要 `daemon`）
- transport：`local`，`ssh`

HerdrDriver.list：

- 测试注入 socket / config_dir（沿用 `HERDR_SOCKET_PATH` / 现有 `discover_local_herdr(config_dir)`）。
- **禁止**在 `with_builtins()` 或 `list` 默认路径去连用户 `~/.config/herdr/herdr.sock` 除非调用方明确要扫本机（W20 生产发现可以扫；**单测**必须注入隔离目录，断言不含用户 `w2`）。
- 生产 Runtime 仍禁止 `Command::new("herdr")`。SSH 发现允许 `ssh … herdr session list`。

`TmuxRuntime::new_ssh_attach`：本步起 `TmuxDriver::open` 应走 `Connect`，不要在 Driver 外再分叉 `if transport == "ssh"`. 可以暂时让 Connect 内调用现有 SSH spawn，但 **Catalog::open 不得**再进 `WorkspaceSpec::build_runtime`。

Commit：`feat(catalog): register builtin tmux/herdr/shell drivers`

---

## C3 — Pool 不再认识 Herdr 字符串

`WorkspacePool::open_spec` 改为调 `Catalog` **或**删掉，由 FFI/GUI 只走 `Catalog::open`。

必须删：

```
if spec.runtime == "herdr" { herdr_sessions ... }
herdr_sessions: HashMap<(String, String), Arc<HerdrSession>>
```

同一 named session + socket 的共享：`Connect` / Driver.open 里拿 `Arc<HerdrSession>`，和今天旁路表语义相同，位置不同。

`unknown_runtime_builds_shell`（`spec.rs`）是旧合同。Catalog 路径下未知 runtime **必须 Err**。更新该测试：`build_runtime` 若仍存在，标 deprecated；新断言放 Catalog（已有 `open_rejects_unknown_runtime`）。不要让未知 id 再变成 shell。

`WorkspacePool::list_worktrees` 里 `downcast_ref::<HerdrRuntime>()` 同样是实现名判断。改成：有 `WorktreeList` 就调 Runtime 上的产品方法（没有就给 trait 加默认 `Err`，HerdrRuntime 覆盖）。**不要**在 pool 里写 `if herdr`。

门禁：C1 那两则 `include_str!` 变绿；现有 herdr 合同 / `linux_herdr_*` / `linux_existing_e2e` 仍绿。

Commit：`refactor(catalog): open workspaces through Catalog connects`

---

## C4 — Inventory

实现 `Catalog::refresh_inventory`：

- 对每个 Transport 的 `list_targets()`，尝试 `connect` + 各 Driver `list`（短命令）。
- 失败 → `Reach::Err`；成功 → `Reach::Ok` 并缓存 `sessions`。
- **pool.len() 必须仍为 0**（测试 `refresh_inventory_marks_unreachable_without_opening`）。
- 限并发（不要一次 ssh 所有 Host）。不要在 GTK 线程调用；本步 core 同步 API 即可，GTK 仍用现有 async spawn 调 snapshot。
- 把 `window.rs` 里 W15 一次性 `SshReach` **数据源**换成 snapshot。控件名字不要改（`linux_connect_timeout_e2e` / 现有灯测试保持绿）。

Commit：`feat(catalog): probe inventory without attaching runtimes`

---

## C5 — FFI

`MuxtermHandle` 持有 `Catalog` 而不是裸 `WorkspacePool`（pool 从 `catalog.pool()` 取）。

新增（JSON，风格对齐现有 `muxterm_discover_*_json`）：

| C 名 | 实现 |
|---|---|
| `muxterm_runtime_list_json` | `Catalog::runtime_list` |
| `muxterm_transport_list_json` | `Catalog::transport_list` |
| `muxterm_discover_targets_json` | `discover_targets` |
| `muxterm_discover_sessions_json` | `discover_sessions`（扇出 tmux+herdr） |

`muxterm_discover_workspaces_json` 改为调用 `discover_sessions`，保持 §6.2 JSON 形状，**补上 herdr 候选**。macOS 继续能调旧名字。

`muxterm_workspace_open` 走 `Catalog::open`。

单测放 `src/core/protocol/ffi/api.rs` 或 catalog：JSON 含 `"tmux"` / `"herdr"` / `"shell"`，不含 `"daemon"`。

Commit：`feat(ffi): expose catalog runtime and session discovery`

---

## C6 — GUI 数据驱动（Linux；macOS 只吃 FFI）

新建项目卡：从 `runtime_list()` 画，不要硬编码三张卡的存在性。`widget_name` **保持** `muxterm-runtime-tmux` / `muxterm-runtime-herdr` / `muxterm-runtime-shell`（W20 测试依赖）。

「已有的连接」列表改为 `discover_sessions` / Inventory，不要 platform 直接调 `discovery::existing`。

禁止新增 `if spec.runtime == "herdr"` / `if runtime == "herdr"`。worktree 入口继续 `support()`（已有注释）。

`linux_panel_e2e` / `linux_existing_e2e` / `muxterm-runtime-herdr` 保持绿。

Commit：`feat(linux): drive project cards from catalog runtime_list`

macOS：接新 JSON 即可，**不要**在 Swift 里写 Herdr socket 帧。若本轮来不及改 Xcode，Linux 必须先吃 FFI；在 commit body 写明 macOS 仍走旧 `discover_tmux_sessions` 别名。

---

## 提交与门禁

每个 Cx **一个英文 commit**：`type(scope): subject`，body 英文逐条。不要 squash 进 W19/W21。不要 push。

每步后：

```
cargo fmt
cargo test --lib catalog:: -- --test-threads=1
cargo test --lib wheel_action -- --test-threads=1   # C0 之后
cargo check --features gtk
```

C3 之后加：

```
cargo test --test herdr_session_contract -- --test-threads=1
cargo test --test herdr_multi_workspace_contract -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_herdr_e2e -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_existing_e2e -- --test-threads=1
```

C0 / C6 之后加对应 GTK e2e（`--test-threads=1`，`xvfb-run -a`）。

全绿后再停。汇报：commit hash、哪几个测试从红变绿、`fbc77e4` 仍是祖先。
