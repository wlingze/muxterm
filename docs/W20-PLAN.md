# W20-PLAN.md — 快速连接：Herdr runtime +「已有的连接」

> 日期：2026-08-17（`2026-08-17T19:09:11+08:00`）
> 工作目录：`/home/wlz/Developer/self/muxterm`
> 分支：`feature/runtime/support_herdr`
> 先读：[`W19-PLAN.md`](W19-PLAN.md) 与 [`W21-PLAN.md`](W21-PLAN.md)（都必须先绿）→ 本文件 → [`RUNTIME.md`](RUNTIME.md) §8 → [`TESTING.md`](TESTING.md) §5.13 → `quickconnect_panel.rs` / `target_config_window.rs` / `discovery.rs`
> 远程 Herdr 核对：官方 [Socket API](https://herdr.dev/docs/socket-api/)（`workspace.list`）、[How to work](https://herdr.dev/docs/how-to-work/)（SSH 上跑 Herdr）、[Persistence](https://herdr.dev/docs/persistence-remote/)（named session 路径）。核对时间 2026-08-17T19:09:11+08:00。
>
> **你是实现 agent。W19 与 W21 门禁未绿不要开始本文件。每个 W20x 先红测试再实现。禁止改断言 / widget_name / token。禁止 `#[ignore]`。禁止 `git add -A`。禁止 Co-authored-by。禁止 push。禁止连用户默认 Herdr 做测试。禁止 `herdr server stop`。禁止对默认 tmux `kill-server`。生产代码禁止 `Command::new("herdr")`。GUI 禁止 `if runtime == "herdr"`，问 `support()` / `TargetRuntime` 枚举。`fbc77e4` 必须仍是祖先。**

用户要的不是两套一级 UI（tmux 一栏、Herdr 一栏）。一级仍是 **预设项目**。最上固定一格目录 **「已有的连接」**。进去按 **本地 / SSH** 两个目录，目录里 tmux session 和 Herdr workspace **同一套项目行**（名字 + `runtime @ transport` 副标题）。新建项目的 runtime 要能选 Herdr。

---

## 0. 交互（锁死，不要自作主张改层级）

```
工作区 tab
  已有的连接          muxterm-existing-connections     ← 永远第一行
  <Recent / Project 原样>
  新建项目            __new_project__

点「已有的连接」
  ← 返回              muxterm-existing-back
  本地                muxterm-existing-local
  SSH                 muxterm-existing-ssh

点「本地」
  ← 返回
  每一行 = 本机一条活着的 tmux session 或 Herdr workspace
  展示与 Project 行相同（title + subtitle `tmux @ local` / `herdr @ local`）
  widget_name = muxterm-existing-row-<runtime>-<id>
  空：dim label muxterm-existing-empty，不要 panic

点「SSH」
  ← 返回
  只列出「探测到至少一条 tmux 或 Herdr」的 Host
  行样式对齐现在的 SSH 选择（含 W15 可达性灯）
  widget_name = muxterm-existing-host-<alias>
  探测中：muxterm-existing-ssh-loading，列表可先空，回来再填
  禁止在 GTK 线程里同步 ssh

点某个 Host
  ← 返回
  该机上的 tmux session + Herdr workspace，副标题 `tmux @ <alias>` / `herdr @ <alias>`
```

点活 session 行 = **只 attach**，不要走 ProjectConnectFlow 的「没有就 create」。失败：日志 + 已有错误路径，不许 panic。

搜索框：在子目录里只过滤当前层。`已有的连接` 自己要能被「已有」「existing」搜到。

---

## 1. 新建项目：Herdr runtime 卡

`TargetRuntime` 增加 `Herdr`（`as_str` / `from_str` / serde `herdr`）。旧 `quickconnect.toml` 没有 herdr 的继续能读。

`target_config_window.rs` 的 `option_card` **必须** `set_widget_name`：

| 卡 | widget_name |
|---|---|
| tmux | `muxterm-runtime-tmux` |
| shell | `muxterm-runtime-shell` |
| herdr | `muxterm-runtime-herdr` |
| local | `muxterm-transport-local` |
| ssh | `muxterm-transport-ssh` |

i18n：`attach_create_herdr` / 中文「attach / 创建 Herdr workspace」。

保存后 `connect_target`：

- `Herdr` + local：默认 socket `$HOME/.config/herdr/herdr.sock`（可用 `HerdrSession`）。没有 socket → 错误，不要 `herdr` CLI 去拉 server。
- 已有 workspace：按 name/path 对 `workspace.list`，命中就 `WorkspaceSpec::herdr(session, workspace_id, socket)`。
- 没有：`workspace.create`（socket JSON，禁止 `Command::new("herdr")`）。
- `Herdr` + ssh：先保证远端 Herdr **已经在跑**（W20d 探测）。用 Unix socket 转发连远端 `herdr.sock`（见 §4），不要 `herdr --remote`（那会在远端装/启 server）。

测试里 New Project 不许打用户默认 socket：注入 `MUXTERM_TEST_HERDR_SOCKET` 或 `test_open_spec(WorkspaceSpec::herdr(…IsolatedHerdr…))`。

---

## 2. Discovery（core，无 GTK）

新模块 `src/core/discovery/existing.rs`（或 `discovery.rs` 里一组函数），返回 `Vec<ExistingEntry>`。GUI 只渲染。

```rust
pub struct ExistingEntry {
    pub title: String,
    pub runtime: TargetRuntime,       // Tmux | Herdr（Shell 不出现在已有的连接）
    pub transport: TargetTransport,
    pub tmux_session: Option<String>,
    pub herdr_session: Option<String>, // "default" 或 named session 名
    pub herdr_workspace_id: Option<String>,
    pub herdr_socket: Option<String>,  // 本机绝对路径；SSH 在 prepare 之后才是本地转发路径
}
```

副标题复用 `QuickConnect::subtitle` 那种 `runtime @ transport`。

### 2.1 本地 tmux

已有 `list_local_tmux_sessions(None)`。**只读**默认 server 的 `list-sessions`（AGENTS.md 允许）。测试传入 `-L muxterm-test-*`，不要在测试里打默认 server。

### 2.2 本地 Herdr

只读 socket，**禁止** `server.stop` / `session stop`（除非 Drop IsolatedHerdr 且名字匹配 `muxterm-test-*`）。

扫描：

1. `$HERDR_SOCKET_PATH` 若设了（测试用）
2. `$HOME/.config/herdr/herdr.sock`（用户默认；测试必须 mock/override，**禁止**单测连这个）
3. `$HOME/.config/herdr/sessions/*/herdr.sock`（named session）

每个能 `ping` / `workspace.list` 的 socket → 每个 workspace 一条 `ExistingEntry`。连不上就跳过，不要 panic。

测试：`IsolatedHerdr` + `workspace create`，`discover_local_herdr(config_dir_override)` 必须看到那条，且 **不得**出现用户默认 `w2`。

### 2.3 SSH tmux

已有 `list_ssh_tmux_sessions(alias, …, timeout)`。超时 2s，失败 = 没有 tmux。

### 2.4 SSH Herdr

不要 `herdr --remote`。与 tmux 一样：`ssh <alias> -- …` 只读。

1. `ssh -o BatchMode=yes -o ConnectTimeout=2 <alias> -- herdr session list --json`
   - 失败 / 没有 `herdr` → 该 host 无 Herdr
   - 清掉远端环境里的 `HERDR_ENV` / `HERDR_SESSION`：`env -u HERDR_ENV -u HERDR_SESSION herdr session list --json`
2. 对每个 running session，用 json 里的 `socket_path` 记下；workspace 列表用  
   `ssh … -- env -u HERDR_ENV herdr --session <name> workspace list`  
   （默认 session 用 `herdr workspace list`，同样 `env -u`）
3. 解析 JSON 的 `result.workspaces[].workspace_id` / `label`

生产 GUI **列出**可以走 ssh+CLI（discovery 层，和现在 `list_ssh_tmux_sessions` 同类）。生产 **attach** 禁止 CLI 当 Runtime：见 §4。

测试：`LoopbackSshd` + 本机 `IsolatedHerdr`。sshd 的 PATH 要能找到 `herdr`。`MUXTERM_SSH_CONFIG_PATH` 指向夹具。禁止端口 22。无 sshd / 无 herdr 才 eprintln skip。

### 2.5 并发与超时

SSH 多 host 必须后台线程 + channel 回 GTK（抄 W15 SSH 灯）。硬超时 2s/host。最多同时 4 路。GTK 主线程禁止 `Command::output`。

---

## 3. 面板模型

`PanelItem` 增加：

```rust
enum PanelItem {
    Target(QuickConnectEntry, bool),
    NewProject,
    Folder { id: &'static str, title: String },   // 已有的连接 / 本地 / SSH
    Back,
    Existing(ExistingEntry),
    Host { alias: String },
    Loading,
    Empty { title: String },
}
```

`build_root_items(store, current)`：**第一项** Folder `existing-connections`，然后现有 `build_items` 的 Recent/Project，最后 NewProject。改现有 `build_items_marks_current_and_appends_new_project`：len 变成 4，`items[0]` 是 Folder。

导航纯函数（可单测）：

```rust
enum ExistingNav { Root, Home, Local, SshHosts, SshHost { alias: String } }
fn existing_items(nav, locals, hosts, remote_of_alias) -> Vec<PanelItem>
```

点 Folder / Back 只改 `nav` + 刷新 list，不要关面板。

`filter_panel_items`：Folder 用 title；Existing 用 title+subtitle；Back 始终保留。

---

## 4. Attach

| 行 | spec |
|---|---|
| 本地 tmux | `WorkspaceSpec::local_tmux(Some(session), None)` |
| 本地 Herdr | `WorkspaceSpec::herdr(session_name, workspace_id, socket_path)` |
| SSH tmux | `WorkspaceSpec::ssh_tmux(alias, Some(session), None)` |
| SSH Herdr | 先把远端 Unix socket 转到本机临时路径，再 `WorkspaceSpec::herdr(…, local_fwd)`；`WorkspaceId.transport` 必须是 `"ssh"`，`alias` 必须是 Host |

SSH Herdr 转发（core，不是 GUI）：

```text
ssh -nNT -o BatchMode=yes -o ExitOnForwardFailure=yes \
    -L <local.sock>:<remote_socket_path> <alias>
```

`remote_socket_path` 用 `session list --json` 的绝对路径，不要在 `-L` 里写 `~`。转发进程跟 Workspace 走，Drop 时杀掉。`HerdrRuntime` 仍只认本机 `UnixStream`（现有 `HerdrSession`）。

不要实现 `herdr --remote` 瘦客户端。

`connect_target` 增加 `TargetRuntime::Herdr` 分支。从已有的连接点进来的 TargetConfig 要带上 socket / workspace_id（`path` = workspace_id，可选 `socket` 字段；不要改旧 project 的 `unique_id` 公式，live 行用 `ExistingEntry` 自己的 id）。

`WorkspaceSpec` 若还没有 ssh+herdr 的 id 形状：`WorkspaceId { transport: ssh, alias, session: herdr_session, runtime: herdr, path: workspace_id }`。`build_runtime` 的 herdr 臂继续用 `spec.socket` 当本机路径（转发后的）。

---

## 5. 测试清单（先红）

| ID | 文件 | 必须抓住 |
|---|---|---|
| W20a | `src/core/quickconnect/model.rs` | `TargetRuntime::Herdr`；`from_str("herdr")`；`subtitle` 含 `herdr @` |
| W20b | `quickconnect_panel.rs` 单测 | `build_root_items` 第 0 项 Folder existing-connections；第末 NewProject |
| W20c | 同文件 | `existing_items(Home)` 含 Local + SSH 两个 Folder + Back |
| W20d | `src/core/discovery/existing.rs` 或单测 | IsolatedHerdr：local discover 含刚 create 的 workspace_id；**不含**用户默认 w2 |
| W20e | `tests/existing_ssh_contract.rs` | LoopbackSshd + 远端 `-L muxterm-test-*` tmux session 出现在 discover；再加 IsolatedHerdr 出现 herdr 行 |
| W20f | `tests/linux_panel_e2e.rs`（同一 Window 生命周期） | `find_by_name(..., "muxterm-existing-connections")`；click 后有 `muxterm-existing-local` 和 `muxterm-existing-ssh`；Back 回到根且 New Project 还在 |
| W20g | `tests/linux_target_config_e2e.rs` 或 panel crate | `muxterm-runtime-herdr` 存在；点它后保存出 `TargetRuntime::Herdr` |
| W20h | `tests/linux_existing_e2e.rs` | 一个 AppWindow：IsolatedHerdr 播种 token → 面板本地目录出现该 workspace → click → VTE/`search_all` 含 token |

W20f 继续遵守 `linux_panel_e2e`「整个 crate 一个 Window」；不要新开第二个 present。

W20h：`test_open_spec` 已能开 Herdr；本测试要走 **面板 click**，不要测试里直接 `test_open_spec` 冒充点了已有的连接。可以 `test_show_panel` + `find_by_name` + `activate`。

---

## 6. i18n keys（en + zh-CN 都要加）

| key | zh-CN | en |
|---|---|---|
| `existing_connections` | 已有的连接 | Existing connections |
| `existing_local` | 本地 | Local |
| `existing_ssh` | SSH | SSH |
| `existing_back` | 返回 | Back |
| `existing_empty` | 没有正在运行的 tmux 或 Herdr | No running tmux or Herdr |
| `existing_probing` | 正在检查远程… | Checking remotes… |
| `attach_create_herdr` | attach / 创建 Herdr workspace | attach / create Herdr workspace |
| `internal_error` | 内部错误 | Internal error |
| `internal_error_logged` | 详情已写入日志 | Details were written to the log |

---

## 7. 门禁

```bash
cargo test --lib quickconnect -- --test-threads=1
cargo test --lib discovery -- --test-threads=1
cargo test --test existing_ssh_contract -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_panel_e2e -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_existing_e2e -- --test-threads=1
# 回归
xvfb-run -a cargo test --features gtk --test linux_quickconnect_e2e -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_herdr_e2e -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_ssh_e2e -- --test-threads=1
```

---

## 8. 明确不做

- 一级按 tmux / Herdr 分两个顶栏
- 把命令面板 `tmux_dialog` 的 Quick Pick 当已有的连接（样式必须是项目行）
- 测试连 `/home/wlz/.config/herdr/herdr.sock`
- `herdr server stop` / 不带 `-L` 的 `tmux kill-server`
- 生产 `Command::new("herdr")` 当 Runtime（discovery 的 **远程** `ssh … herdr session list` 可以，和 `ssh … tmux list-sessions` 同类）
- 远端没在跑 Herdr 就帮用户装/启动（`herdr --remote` 会装，禁止）
- 改 W13 常量、live `visible_ansi` → reset、revert `fbc77e4`
- 在 W19 未绿时改 `emulate.rs` 以外的「顺便重构」
