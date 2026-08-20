# CATALOG-PLAN.md — Catalog 施工单（Codex）

> 日期：2026-08-17（`2026-08-17T22:19:47+08:00`）
> 工作目录：`/home/wlz/Developer/self/muxterm`
> 分支：`feature/runtime/support_herdr`（HEAD 在写本文时为 `30e0bfa`（pre-rebase `b739494`））
> 契约：[`CATALOG.md`](CATALOG.md)（先读完）。产品树：[`WORKSPACE.md`](WORKSPACE.md) §6。Runtime：[`RUNTIME.md`](RUNTIME.md)。
> 测试：[`TESTING.md`](TESTING.md) §5.12（W21）+ §5.14（Catalog）+ §5.15（C7/C8 Host `local` / 缩放）。结构：[`PROJECT-STRUCTURE.md`](PROJECT-STRUCTURE.md)。
> 滚轮漏项：[`W21-PLAN.md`](W21-PLAN.md)（已在 C0 落地）。
>
> **你是实现 agent。每个 Cx 先确认测试是红的，再写最小实现到绿。禁止改断言、token、widget_name、错误文案子串来「绿」。禁止 `#[ignore]`。禁止 `git add -A`。禁止 Co-authored-by。禁止 push。禁止连用户默认 Herdr。禁止 `herdr server stop`。禁止对默认 tmux `kill-server`。生产代码禁止 `Command::new("herdr")`。GUI / Pool 禁止 `if spec.runtime == "herdr"`。`d1181679` 必须仍是祖先。live 路径禁止 `visible_ansi` → `vte.reset`。不要等用户确认。不要提前把 goal 标 complete。**
>
> **C0–C6 已在 HEAD `54c647f`（pre-rebase `7c74edd`）。不要重做 W21 / 内置 Driver / FFI / 项目卡。本 goal 从 C7 开始，C7 绿再 C8。**

Cursor 已经落下 C7/C8 的红测试和文档。C0–C6（W21 滚轮、内置 Driver、FFI、项目卡）已在 `54c647f`（pre-rebase `7c74edd`），**不要重做、不要改那些绿测试的断言**。

W19 / W20 / W21 已在 HEAD。本 goal 只做 C7 然后 C8。

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

全绿后再停。汇报：commit hash、哪几个测试从红变绿、`d1181679` 仍是祖先。

---

## C7 — SSH 列出（dogfood `test_2026-0818-0133.log`）

> 日期：2026-08-18（`2026-08-18T01:50:34+08:00`）
> 日志：`test_2026-0818-0133.log`（UTC 17:33–17:37 = +08 01:33–01:37）。用户点「已有的连接 → SSH」拿可连接数据失败。

C0–C6 已在 HEAD `54c647f`（pre-rebase `7c74edd`）。**不要重做。** 本步只修列出。不要 GNU Screen。不要弱化 W13 常量。

### 根因（已核对日志，不要再猜）

1. `TmuxDriver::list` → `list_ssh_tmux_sessions` → `build_ssh_command`（**attach** 用的 `-tt` + `ConnectTimeout=10`）+ `SshProcessTransport` PTY。日志 9–11 行：`spawn ssh transport args=["-tt", … "ConnectTimeout=10", "ryzen"|"mac"|"cd", "--", "tmux list-sessions …"]`。W20 `ssh_run` / `ssh_probe_args` 已经是 BatchMode + 2s、无 `-tt`。Catalog 列出没走那条。
2. `build_ssh_command_for_discovery` 现在直接转调 `build_ssh_command`，等于没有 discovery 命令。
3. `drain_existing_ssh` 只把 **非空** entries 的 alias 放进 `hosts`。解析失败 = 空 = host 消失。`existing_items` 在 `hosts` 空时永远 `Loading`，探测结束也转圈。
4. `spawn_existing_ssh_probe` 注释写 4 路并发，代码是 `aliases.into_iter().map` **串行** `Catalog::discover_sessions`。每个 host 还扇出 tmux + herdr。`cd` 这类慢 host 会把整表拖到 10s 级。
5. `open_panel` 在 **GTK 线程** `discover_sessions("local", "")`（本机 tmux + 扫全部 herdr socket，单次 ping 超时 5s）。
6. Host 名叫 `local` 和 Transport id `"local"` 不是一回事。测试必须用 LoopbackSshd **`Host local` → 127.0.0.1**，断言 `discover_sessions("ssh", "local")`，禁止当成 Local 单例。

### 要绿的测试（Cursor 已写下，禁止改断言）

| 测试 | 现在为什么红 |
|---|---|
| `discovery_ssh_command_is_batch_short_timeout_no_forced_tty` | discovery 命令仍是 `-tt` / `ConnectTimeout=10` |
| `list_ssh_tmux_sessions_must_not_use_attach_transport` | `list_ssh_tmux_sessions` 仍 `SshProcessTransport` + `build_ssh_command` |
| `tmux_driver_list_honors_test_remote_socket_env` | `TmuxDriver::list` 不读 `MUXTERM_TEST_REMOTE_TMUX_SOCKET` |
| `catalog_ssh_host_named_local_lists_isolated_tmux_and_runtime_list` | Catalog 列不到隔离 `-L` session；三个 `local` 可能串 |
| `ssh_hosts_empty_after_probe_must_not_stay_loading` | `ExistingPanelState` 没有 `probe_inflight`，空表 = 永远 Loading |
| `spawn_existing_ssh_probe_must_fan_out` | 一个 `thread::spawn` 里串行 map |
| `open_panel_must_not_discover_sessions_on_caller` | `open_panel` 里同步 `discover_sessions("local")` |
| `linux_catalog_ssh_e2e` | 面板 SSH → Host `local` → 隔离 tmux 行 |

### 实现要点

- `build_ssh_command_for_discovery`：`-F`（若有）、`BatchMode=yes`、`ConnectTimeout=2`、alias、`--`、remote。**不要 `-t`/`-tt`。不要改 `build_ssh_command`（attach 仍要 `-tt`）。**
- `list_ssh_tmux_sessions`（以及已走 discovery 的 `create_ssh_tmux_session`）用这条命令 + `Command` 收 stdout（可复用/改 `run_ssh_discovery_command`，不要再 PTY）。超时仍由调用方传入（Driver 用 2s）。
- `TmuxDriver::list` SSH 分支：`std::env::var("MUXTERM_TEST_REMOTE_TMUX_SOCKET").ok()` 传给 `list_ssh_tmux_sessions` 的 `remote_socket`。生产不设 env = 远端默认 server。
- `ExistingPanelState` 加 `probe_inflight: bool`。`spawn_existing_ssh_probe` 开头 true，`drain` 结束 false。`existing_items(SshHosts)`：空 + inflight → Loading；空 + 完成 → Empty；有 host → Host 行。W20「没有格子的 host 不要占满列表」仍成立：host 仍只来自 **有 entries** 的 alias。
- `spawn_existing_ssh_probe`：最多 4 路并发（`chunks(4)` + `thread::scope`，或每 host 一个 join，限制 4）。注释已经这么写了。
- `open_panel` 不要同步 `discover_sessions`。本地列出搬到后台线程，16ms poll / `test_poll_once` 收编，和 SSH probe 同一模式。GTK 线程禁止 `ssh`、禁止扫 herdr socket。

夹具：`LoopbackSshd::start_with_alias(label, "local")` 已有。隔离 tmux `-L muxterm-test-*`。无 sshd 才 eprintln skip，禁止 `#[ignore]`。

Commit：`fix(catalog): list ssh sessions without attach pty`

---

## C8 — 回车 / 缩放不冻 GTK

同一份日志：attach 后 `%pause`/`%continue` 打满；`send-keys -H 0d`；17:37:04 `search_workspace query=""` 扫了 4 个工作区。用户：回车卡、放大缩小卡死。

### 根因

1. `adjust_font` / `reset_font` 在按键路径上对 **全部** `pixel_cache` 调 `set_font_size`（每个 VTE `set_font_desc`），再 **同步** `persist_config` 写 `config.toml`。日志里用户同时挂着本地 + SSH，pane 很多。
2. 回车本身是 `WriteRaw` → 非阻塞 `send-keys`。卡是后续洪水 + 同步缩放/搜空串。不要改 `MAX_OUTPUT_EVENTS_PER_SEC`。
3. Search tab 空 query 仍对每个 workspace `search_workspace("")`（info 日志）。空 query 在 emulate 层立刻空，但 GTK 打开 Search 会扫一遍。

### 要绿的测试

| 测试 | 现在为什么红 |
|---|---|
| `adjust_font_must_not_persist_config_synchronously` | `fn adjust_font` 里直接 `persist_config` |
| `linux_zoom_input_e2e` | 热路径写盘 + 全 cache 改字体，预算很容易超 |

### 实现要点

- `adjust_font`：**先**改当前前台 `LayoutHost` 的字号，立刻返回。`persist_config` 用 `glib::timeout_add_local` 防抖（200–400ms）或后台线程写盘。`linux_prefs_e2e` 的 `persist_config("font.size", …)` 直写路径保持绿。
- 后台 workspace 的 VTE 字号：切到前台时若 `layout.font.size != s.font.size` 再 `set_font_size`。不要在一次 Ctrl+= 里遍历全部 pixel_cache。
- `test_increase_font` / `test_decrease_font` 钩子已在 `AppWindow`（Cursor 写的）。e2e：隔离 tmux attach 之后，缩放和 Enter 必须在几百毫秒内把控制权还给 GTK。
- 空 query：`search_all` / 面板 Search 不要对空串扫 replica（emulate 已返回空）。可跳过调用。

Commit：`fix(linux): debounce font zoom off the gtk key path`

C7 绿之前不要开始 C8。C8 不要动 SSH 列出。每步 `cargo fmt`。禁止 `git add -A`。禁止 push。禁止 `herdr server stop`。禁止不带 `-L muxterm-test-*` 的 `kill-server`。`d1181679` 必须仍是祖先。live 路径禁止 `visible_ansi` → `vte.reset`。

---

## C9 — 扁平已有的连接 + connect name `all`

> 日期：2026-08-19（`2026-08-19T01:41:31+08:00`）
> 契约：[`CATALOG.md`](CATALOG.md) §1.4、[`W20-PLAN.md`](W20-PLAN.md) §0、[`TESTING.md`](TESTING.md) §5.16。
> C0–C8 已在 HEAD `b08718f`（pre-rebase `9767e2c`）。**不要重做 C7/C8。** 不要 GNU Screen。不要弱化 W13 常量。

用户：快速连接「已有的连接」不要多层目录，进去就是可 attach 的 runtime list。命令面板先列机器（connect name：`local` 与 SSH alias 并列），点进去才是该机器的 runtime list。测试只锁 **local + ssh-self 双份**；不要求 archmini/cd。

### 根因

1. W20 锁死 已有的连接 → 本地 / SSH → Host → session。点 SSH 看到的是 host 名，不是可 attach 行。搜索只滤当前层。
2. `muxterm_runtime_list_json` 是插件卡，不是可 attach 行。可 attach 行是 `discover_sessions(transport, target)`，没有 `"all"`。
3. FFI JSON `transport` 是插件 id `"ssh"`，**丢掉 `SessionCandidate.target`（connect name）**。`id` 是 `ssh/tmux/name`，两机同名会撞。
4. 命令面板 SSH Connect 第一层 host 看得到；第二层 `CoreBridge::discover_workspaces` 仍按旧字段 `windows/attached/created` 解 C5 JSON，失败后空表。

### 要绿的测试（Cursor 已写下，禁止改断言）

| 测试 | 现在为什么红 |
|---|---|
| `discover_sessions_all_fans_out_local_and_ssh_targets` | `"all"` 不是 Transport 插件 → 空表 |
| `tmux_driver_list_honors_test_local_socket_env` | 本地 list 不读 `MUXTERM_TEST_LOCAL_TMUX_SOCKET` |
| `catalog_all_lists_local_and_ssh_self_duplicates` | 没有 `all`；本地打到用户默认 server |
| `ffi_discover_sessions_json_includes_target_and_all` | JSON 无 `target`；不接受 `all` |
| `existing_items_home_is_flat_local_and_ssh_self` | Home 仍是 local/ssh Folder |
| `existing_row_widget_includes_connect_name` | widget 仍是 `muxterm-existing-row-{runtime}-{id}` |
| `connect_pick_items_lists_local_then_ssh_aliases` | 只有 SSH host，没有并列的 `local` |
| `existing_connections_navigation` | 仍断言 local/ssh 目录 |
| `linux_catalog_ssh_e2e` | 仍点 SSH → Host local |
| `linux_existing_e2e` | 仍点 `muxterm-existing-local`；widget 无 connect name |

C7 的 `catalog_ssh_host_named_local_lists_isolated_tmux_and_runtime_list` **保持绿**（API 仍支持 `discover_sessions("ssh","local")`）。

### 实现要点

- `Catalog::discover_sessions`：`transport_id == "all"` 时，对 `local` 的单例 target + 每个 SSH target 各 `discover_sessions` 一次，拼接。单个 connect 失败跳过。**不是**注册名叫 `all` 的 Transport。
- JSON：`target` = connect name（本地用 `"local"`，即使 Catalog target id 是 `""`）；`id` = `{transport}/{target}/{runtime}/{name}`。`muxterm_discover_sessions_json` 与 `muxterm_discover_workspaces_json` 同一形状。
- `TmuxDriver::list` 本地分支读 `MUXTERM_TEST_LOCAL_TMUX_SOCKET`（对标 REMOTE env）。生产不设。
- 已有的连接：`existing_items(Home)` = Back + 扁平 Existing 行（local 行 + 各 SSH 行）。禁止 Folder `existing-local`/`existing-ssh`，禁止 `PanelItem::Host`。探测**先** `discover_sessions("local")` 推表，再按 SSH host `chunks(4)` 并发；`probe_inflight` 空表 → Loading，完成空 → Empty。生产 16ms poll 必须调用 `drain_local_existing` 收 channel 并触发 `refresh_current`；GTK e2e 只驱动 GLib 主循环，禁止用 `test_poll_once()` 冒充生产接线。`Catalog::discover_sessions("all")` 本身也最多 4 路并发（FFI 用）。
- widget_name = `muxterm-existing-row-{runtime}-{connect}-{id}`。connect = `local` 或 SSH alias。
- 命令面板：`connect_pick_items` 第一项 `local`，后面 SSH alias。点 `local` → `discover_sessions("local","")`；点 alias → `discover_sessions("ssh", alias)`。第二层 detail 含 `runtime @ connect`。`CoreBridge::discover_workspaces` 必须能解 C5+target JSON（旧 `windows` 字段 `#[serde(default)]`）。
- `open_panel` / GTK 线程仍禁止同步 `ssh` / 扫 herdr socket。
- 不要去重 local 与 ssh-self。不要测用户默认 sock。不要 `herdr server stop`。不要不带 `-L muxterm-test-*` 的 `kill-server`。

夹具：`LoopbackSshd::start_with_alias(..., "self")`；两 env 指向同一 `-L muxterm-test-*`；`apply_ssh_config_env`（测试 ssh config **只有** Host `self`，不会碰到用户的 archmini/cd）。`HERDR_SOCKET_PATH` 指空/隔离，禁止连用户 `herdr.sock`。无 sshd eprintln skip，禁止 `#[ignore]`。

Commit（建议拆两刀）：

1. `feat(catalog): fan out discover_sessions all with connect name`
2. `feat(linux): flatten existing connections into attachable rows`

`d1181679` 必须仍是祖先。live 路径禁止 `visible_ansi` → `vte.reset`。
