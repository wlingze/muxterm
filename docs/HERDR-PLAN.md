# HERDR-PLAN.md — Herdr Runtime 接入（Codex 施工单）

> 日期：2026-08-17（`2026-08-17T16:01:48+08:00`）
> 工作目录：`/home/wlz/Developer/self/muxterm`
> 分支：`feature/runtime/support_herdr`（从 `bd01a39` 切出）
> 契约：[`RUNTIME.md`](RUNTIME.md)。产品树：[`WORKSPACE.md`](WORKSPACE.md) §6。像素：[`SURFACE.md`](SURFACE.md)。
> 测试规范：[`TESTING.md`](TESTING.md) §5.10。愿景阶段 D：`PRODUCT-VISION-STRATEGIC-REVIEW.md` §0.3。
> 本机核对：`herdr 0.8.0`，协议 **19**。官方 [Concepts](https://herdr.dev/docs/concepts/)、[Socket API](https://herdr.dev/docs/socket-api/)、[CLI](https://herdr.dev/docs/cli-reference/)。
>
> **你是实现 agent。每个 H 先写测试（必须先红），再写最小实现到绿。禁止改断言、token、widget_name、阈值来「绿」。禁止 `#[ignore]`。禁止 `git add -A`。禁止 Co-authored-by。禁止 push。禁止动用户默认 Herdr / 默认 tmux。**
>
> `fbc77e4` 必须仍是祖先。live 路径禁止 `visible_ansi` → `vte.reset`。

接入难不难：tab/pane/字节和 tmux `-CC` 是同一套产品树，Herdr 的 snapshot + 事件比 `%output` 好接。真正要小心的是 **隔离 named session**、**一条 socket 上多个 Workspace 共享 `HerdrSession`**、以及 **生产代码不要 `Command::new("herdr")`**。按 H0→H4 做，不要一次铺开 agent 侧边栏。

---

## 0. 对照（不要接错层）

| 口头 | Herdr | tmux | Muxterm |
|---|---|---|---|
| socket | named session / `herdr.sock` | `-L` | `HerdrSession` |
| 多个 space | **workspace** `w1` | `tmux ls` 的 session | 池里一格 Workspace |
| tab | tab `w1:t1` | window `@N` | Tab |
| pane | pane `w1:p1` | pane `%N` | Pane |
| 缩进 / new worktree | **worktree** | 无 | `RuntimeCapability::Worktree*` |

Herdr **没有 window 层**。worktree open/create 得到的是**另一格 workspace**，不是当前 Tab 栏多一页。

GUI **只问** `runtime.support()`，禁止 `if spec.runtime == "herdr"`。

---

## 1. 隔离纪律（最高优先级，违反=事故）

本机用户默认 server 是 `/home/wlz/.config/herdr/herdr.sock`（`session list` 里的 `default`）。**测试永远不要连它，永远不要 `herdr server stop`。**

Cursor 已在本机探通过（默认 session 全程还在）：

```text
启动：  herdr --session <NAME> server          # 无头；setsid + stdin/stdout 丢掉
socket：~/.config/herdr/sessions/<NAME>/herdr.sock
播种：  herdr --session <NAME> workspace create --cwd <dir> --label <label>
        herdr --session <NAME> pane send-text <pane_id> <TOKEN>
        herdr --session <NAME> pane send-keys <pane_id> enter
停：    herdr session stop <NAME>
删：    herdr session delete <NAME>            # 去掉 stopped 残留目录
```

`<NAME>` 必须匹配 `muxterm-test-*`。

| 允许 | 禁止 |
|---|---|
| `herdr --session muxterm-test-…` | 不带 `--session` 的 `herdr workspace/pane/…`（本 pane 有 `HERDR_ENV=1`，会打到用户默认 session） |
| `herdr session stop muxterm-test-…` | `herdr server stop`（会停默认 server） |
| 临时 git 仓库建在 `/tmp/muxterm-test-herdr-*` | 在 `/home/wlz/Developer/self/muxterm` 上 `worktree create` |
| 无 `herdr` 二进制 → eprintln skip + return | `#[ignore]` |

生产 `HerdrRuntime` **走 Unix socket JSON**（[Socket API](https://herdr.dev/docs/socket-api/)：`{"id","method","params"}`）。夹具可以用 `herdr` CLI 起 server / 涂 token，和 tmux 测试用 `tmux -L` 起 server、再用 `-CC` 连是同一分工。

---

## 2. 夹具（H1 起必须落地，先于实现）

新建 `tests/support/herdr_test_support.rs`，在 `tests/support/mod.rs` 里 `pub mod herdr_test_support`。

`IsolatedHerdr` 必须：

1. `herdr_available()`：`herdr --version` 成功才 true。
2. `start(label)`：名字 `muxterm-test-herdr-{label}-{nanos}`；spawn `herdr --session NAME server`；等到 socket 文件出现（超时 ~5s）。
3. 保存 `name`、`socket_path`（`session list` / 日志里那条）。
4. `cli(&self) -> Command`：永远 `herdr --session self.name`，禁止依赖环境里的 `HERDR_SESSION` / `HERDR_ENV`。
5. `create_workspace(cwd, label) -> (workspace_id, tab_id, pane_id)`：解析 JSON（**不要传 `--json`**，`workspace create` 本身就打 JSON）。本机形状：
   `{ "result": { "workspace": { "workspace_id": "w1" }, "tab": { "tab_id": "w1:t1" }, "root_pane": { "pane_id": "w1:p1" } } }`
6. `paint(pane_id, token)`：`pane send-text` + `pane send-keys enter`。默认 pane 是用户 shell，token 会出现在 `pane read` 里（本机已验：`HERDR_LIVE_probe3` 可见）。
7. `Drop`：`session stop` 然后 `session delete`。即使测试 panic 也要走。socket 路径不含用户默认 `herdr.sock` 才许 stop。

H4 另要 `TempGitRepo`：`git init` + 一次 empty commit，路径只许 `/tmp/muxterm-test-herdr-*`。Drop 时若有 linked worktree，先 `git worktree remove` 再删目录。

---

## 3. 顺序（不要跳）

每个 H：**先提交（或至少先让 `cargo test` 看到）红测试，再实现。** 一逻辑一英文 commit。

### H0 — `support()`（不需要 herdr 二进制）

`src/core/model/backend.rs`：

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RuntimeCapability {
    PersistDetach,
    Discover,
    MultiTab,
    SplitPane,
    WorktreeList,
    WorktreeCreate,
    WorktreeOpen,
    WorktreeRemove,
}

fn support(&self) -> &'static [RuntimeCapability]; // 加到 trait Runtime
```

填表（[`RUNTIME.md`](RUNTIME.md) §4.1）：

| 实现 | 必须包含 | 必须不含 |
|---|---|---|
| `TmuxRuntime` | PersistDetach, Discover, MultiTab, SplitPane | 全部 `Worktree*` |
| `ShellRuntime` | MultiTab, SplitPane | PersistDetach, Discover, 全部 `Worktree*` |
| `MockRuntime` | 默认空或现有行为；可测试注入 | 默认不含 Worktree* |
| `HerdrRuntime`（H1 才有） | PersistDetach, Discover, MultiTab, SplitPane, WorktreeList/Create/Open | v1 可不含 WorktreeRemove |

`Workspace` / `WorkspacePool` 增加 worktree 入口时：`support()` 没有对应能力 → `Err` / `TaskOutcome::Rejected`，**零 git、零 socket**。

测试：`src/core/model/backend.rs` 或 `src/core/workspace/` 的 `#[cfg(test)]`。

- `tmux_runtime_support_has_no_worktree`
- `shell_runtime_support_has_no_worktree`
- `pool_create_worktree_rejected_without_capability`（Mock 只报 `WorktreeList` 或不报，create 必须拒）

`Cargo.toml` 不必新 crate。

### H1 — socket 客户端 + snapshot

`src/core/runtime/herdr/`：连 **夹具的** socket，`ping`，`session.snapshot`。把 workspace/tab/pane 记进内部 map（Herdr id → 产品 `TabId`/`PaneId`）。还不必进 GTK。

测试 crate：`tests/herdr_session_contract.rs`（`Cargo.toml` 加 `[[test]]`）。

`herdr_named_session_snapshot_sees_painted_workspace`：

1. `IsolatedHerdr::start("snap")`
2. `create_workspace("/tmp", "mux-h1")` + `paint(pane, token)`，token 前缀 `HERDR_LIVE_`
3. `HerdrSession::connect(socket)`（或 `HerdrRuntime` 尚未绑 workspace 的连接）snapshot
4. 必须看到刚才的 `workspace_id`；pane 拓扑非空
5. **禁止** socket 路径等于 `/home/wlz/.config/herdr/herdr.sock`

无 herdr 二进制 skip。

### H2 — 一格 Workspace + 字节进 PaneBuf / VTE

`WorkspaceSpec::herdr(session_name, herdr_workspace_id, socket_path)`：

```text
transport = "local"
runtime   = "herdr"
session   = named session 名（muxterm-test-*）
path      = Herdr workspace_id（w1）
socket    = Some(绝对路径 …/sessions/<NAME>/herdr.sock)
```

`spec.build_runtime()` 构造绑定该 workspace 的 `HerdrRuntime`（内部持有共享 `HerdrSession` 也行，H3 再强制共享）。

直播：`terminal.frame` 解 base64 ANSI → `StateChange::PaneOutput`。attach 快照可用 `pane.read`，**直播不要只靠 pane.read 轮询**。走 [observe 流](https://herdr.dev/docs/persistence-remote/)（`terminal session observe` 那种 NDJSON）。输入：socket `pane.send_input` / `pane.send_keys`。

**core 测试** `tests/herdr_feature_contract.rs`：

`herdr_attach_preexist_token_reaches_workspace`：夹具先涂 `HERDR_LIVE_*`，`Workspace::new` + `connect` 后 `search_workspace` 非空。对齐 `tests/tmux_ssh_feature_contract.rs`。超时参考 SSH_TIMEOUT（15s 量级）。禁止 MockRuntime 喂字节。

**GTK 测试** `tests/linux_herdr_e2e.rs`（`required-features = ["gtk"]`）：

`linux_herdr_attach_shows_preexist_token`：抄 `tests/linux_ssh_e2e.rs` 结构。一个 `AppWindow`。`test_open_spec(WorkspaceSpec::herdr(...))`。断言：

- 有 layout leaf
- VTE `test_pane_vte_text` 含 token
- `test_search_all` 含 token
- `test_workspace_replica_ids()` 含 `"herdr"`（或 id.runtime == herdr），不能误开成本地 tmux

`xvfb-run -a`，`--test-threads=1`，`gtk4::test_synced`。

### H3 — 同一 socket 两格 Workspace

同一 `IsolatedHerdr` 上 `workspace create` 两次，两个 token。

`tests/herdr_multi_workspace_contract.rs`：两个 `Workspace` / 或 `Pool.open_spec` 两次。两个 `search_workspace` 都能命中自己的 token，互不污染。内部必须是**一条** socket（不要 connect 两次各建无关 client 还假装共享；至少不要连默认 sock）。

`tests/linux_herdr_switch_e2e.rs`：GTK 打开两格，`test_activate_workspace`（已有钩子）切过去，VTE 仍有各自 token。

### H4 — worktree list / create / open

只在 **临时 git 仓库** 上测。`workspace create --cwd <temp repo>`。

`tests/herdr_worktree_contract.rs`：

1. `list`：至少一行主 checkout；`path` 是 temp repo；`open_workspace` 对得上当前格。`/tmp` 非 git 目录上 list 应失败或空，且不得 panic。
2. `create`：`--branch muxterm-test-wt-<unique> --path /tmp/muxterm-test-herdr-wt-<unique> --no-focus`。池里多一格；`list` 能看到 `is_linked_worktree`（或等价）；新格 cwd/path 是新 checkout。
3. `open`：对已存在 path 再 open，返回已有 WorkspaceId，不复制一格。

`HerdrRuntime::support()` 含 `WorktreeList` + `WorktreeCreate` + `WorktreeOpen`。

GTK：`tests/linux_herdr_worktree_e2e.rs`。当前格 `support()` 含 `WorktreeList` 时，`muxterm-worktree-create` **存在且可点**（或面板入口可见）。另开一个 **tmux** `AppWindow` 路径（可与现有 `linux_feature_e2e` 回归一起保证）：**找不到** `muxterm-worktree-create`。不要为 tmux 实现假 worktree。

`WorktreeRemove`、agent 通知：**本轮不做。**

---

## 4. 测试怎么写（硬性）

1. **RED 先于 GREEN。** 每个 H 的测试 crate 先 `cargo test` 失败（缺类型、缺方法、search 空），再写 `src/`。
2. 集成测试 `mod support;`，复用 `IsolatedHerdr`，禁止再复制一份 stop/delete。
3. token：`HERDR_LIVE_{label}` / `HERDR_WT_{label}`，断言 `contains`，不要改成更弱的「非空就行」。
4. GTK：无 DISPLAY 用 skip helper（`skip_no_display`），有 xvfb 就必须真跑。同进程一个 `AppWindow`。
5. `Cargo.toml` 每个新 `tests/*.rs` 加 `[[test]]`；GTK 的加 `required-features = ["gtk"]`。
6. 单测不 spawn herdr。H1+ 无二进制 skip，有二进制不许 ignore。
7. 不要用用户仓库做 worktree fixture。

对照代码（只读，不要改那些树）：`/home/wlz/Developer/terminal/` 里 wezterm/ivyterm 的 tmux 客户端只看「第三方怎么吃字节」，Herdr 协议以 herdr.dev 为准。

---

## 5. 怎么跑

无 herdr 的机器：H0 仍须绿；H1+ skip 打印原因。本机有 herdr，H1+ 必须跑。

```bash
cargo fmt --all -- --check
cargo check --features gtk

# H0
cargo test --lib -- tmux_runtime_support_has_no_worktree
cargo test --lib -- shell_runtime_support_has_no_worktree
cargo test --lib -- pool_create_worktree_rejected_without_capability

# H1–H4 core（隔离 named session）
cargo test --test herdr_session_contract -- --test-threads=1
cargo test --test herdr_feature_contract -- --test-threads=1
cargo test --test herdr_multi_workspace_contract -- --test-threads=1
cargo test --test herdr_worktree_contract -- --test-threads=1

# GTK
xvfb-run -a cargo test --features gtk --test linux_herdr_e2e -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_herdr_switch_e2e -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_herdr_worktree_e2e -- --test-threads=1

# 回归（W18 已锁，不许红）
xvfb-run -a cargo test --features gtk --test linux_ssh_e2e -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_feature_e2e -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_reconnect_e2e -- --test-threads=1

cargo clippy --all-targets -- -D warnings
```

`--lib -- <name>` 不要加 `--exact`，除非用完整模块路径。

跑完 `herdr session list`：只许看到用户 `default running`，不许留下 `muxterm-test-* running`。若有 stopped 残留，夹具 Drop 漏了 `session delete`，要修夹具。

---

## 6. Commit 切分（英文 `type(scope):`）

1. `test(herdr): add isolated named-session fixture`
2. `feat(runtime): add RuntimeCapability and support()`
3. `feat(herdr): connect named session and parse snapshot`
4. `feat(herdr): open one workspace and feed pane bytes`
5. `feat(linux): show Herdr pane in GTK after attach`
6. `feat(herdr): share one session across workspaces`
7. `feat(herdr): list/create/open git worktrees`

可以 2+测试 合成一步，但不要把 H0 和 H4 塞进同一个 commit。不 push。

---

## 7. 明确不做

- Herdr 专用侧边栏、agent 列表、插件、graphics、`WorktreeRemove`
- `pane.agent_status_changed` 当身份（tmux OSC/BEL 留下；agent 以后再喂现有 attention）
- `herdr --remote`
- TmuxRuntime 报 `Worktree*`
- platform 拼 `herdr` / `git worktree`
- 产品 Session / 虚拟 Window
- 改 W18 linux tmux GUI 来迁就 Herdr
- 默认 `herdr.sock`、`herdr server stop`、不带 `-L` 的 tmux kill

---

## 8. 完成定义

- [ ] H0–H4 上表全绿，断言未改弱，无 `#[ignore]`
- [ ] `herdr session list` 无测试残留；默认 server 仍 running
- [ ] GUI 只通过 `support()` 露出 worktree
- [ ] 生产路径无 `Command::new("herdr")`（夹具除外）
- [ ] 英文 commit，未 push
- [ ] `fbc77e4` 仍是祖先；live 无 dump
