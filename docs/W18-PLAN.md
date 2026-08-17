# W18-PLAN.md — SSH loopback attach + 搜索范围 / 上次看到这里 / 命令刻度 / 回底 +N

> 日期：2026-08-17（`2026-08-17T13:11:46+08:00`）
> 工作目录：`/home/wlz/Developer/self/muxterm`
> 分支：`feat/linux-quickconnect-ui`（**不 push**）
> 先读：`docs/VISION-AUDIT.md`、`docs/W17-PLAN.md`、`docs/WORKSPACE.md` §6、`docs/SURFACE.md`、`docs/TESTING.md` §5.9、`AGENTS.md`
> 愿景：`docs/PRODUCT-VISION-STRATEGIC-REVIEW.md` §2.15.2（重连）§2.15.4（上次看到这里 / 回底 / 命令轨）§6 阶段 C（搜索范围）
>
> **测试已经写好且必须先红。禁止改断言、阈值、token、widget_name 来「绿」。禁止加 `#[ignore]`。**
> 用户这轮要的是：真 SSH attach（ssh 到本机 loopback），其余断言与本地测试同级；再加上搜索三范围、上次看到这里、命令刻度、回底 +N。

---

## 0. 对照代码（只读，不要改那些仓库）

本机已有克隆：`/home/wlz/Developer/terminal/`（见该目录 `README.md`）。实现前 **读** 这些位置，把行为对齐到 Muxterm 的客户端身份（覆盖层，不改 pane 字节）：

| 题目 | 读 |
|---|---|
| 总索引 | `/home/wlz/Developer/terminal/README.md` |
| SSH + tmux `-CC` | `wezterm/mux/src/tmux.rs`、`wezterm/mux/src/ssh.rs`、`wezterm/mux/src/tmux_pty.rs` |
| Linux GTK+VTE 邻居 | `ivyterm/src/ssh.rs`、`ivyterm/src/tmux_api/`、`ivyterm/src/tmux_widgets/terminal/` |
| attach 快照 / 丢 `%output` | `ghostty/src/terminal/tmux/viewer.zig` |
| 命令刻度 / OSC 133 | `iTerm2/sources/VT100Screen/VT100ScreenState.m`（`commandMarkAt` / `lastCommandMark`）、`iTerm2/sources/VT100/VT100Terminal.m`（OSC 133）、`wezterm/docs/shell-integration.md`、`ghostty/src/terminal/Terminal.zig`（OSC133 单测） |
| 远程 tmux 播种 | `cmux` 里 `RemoteTmuxPaneSeed` / discard+snapshot+catch-up（搜这个符号） |

Muxterm 纪律不变：live 路径禁止 `visible_ansi` → `vte.reset`；重连 `seed_raw`；`fbc77e4` 仍是祖先。

---

## 1. 顺序（不要跳）

1. **W18a** 夹具已写：`tests/support/sshd_test_support.rs` 的 `LoopbackSshd` + `tests/support/ssh_tmux_contract.rs`。先让 `tmux_ssh_feature_contract` 绿（core SSH attach）。
2. **W18b** `linux_ssh_e2e`：GTK SSH attach，VTE + `search_all` 含 token（对齐本地 feature attach）。
3. **W18c** `linux_ssh_history_e2e`：对齐 `linux_attach_history_e2e`。
4. **W18d** `linux_ssh_reconnect_e2e`：对齐 `linux_reconnect_e2e`。SSH 查 `window_bell_flag` **必须走 ssh + 远端 `-L`**，禁止对本机 `tmux -L <远端名>`（那会打到错的 server 或什么都没有）。
5. **W18e** `linux_jump_count_e2e`：回底按钮 +N（按钮已有）。
6. **W18f** `linux_search_scope_e2e`：pane / workspace / all + `muxterm-pane-find`。
7. **W18g** `linux_last_seen_e2e`：`muxterm-last-seen`。
8. **W18h** `linux_command_marks_e2e` + `osc133_records_command_marks_with_exit_and_text`：红绿刻度、点击跳转、tooltip 命令。

一逻辑一英文 commit，`type(scope):`，无 Co-authored-by，不 `git add -A`，不 push。做完一个立刻下一个。

**不做：** Herdr、ET、多窗口、Linux 托盘、像素重写、push、连用户 22 端口、`tmux kill-server` 不带 `-L`。

---

## 2. SSH 夹具纪律（最高优先级）

- 测试 **自己** 用 `LoopbackSshd::start` 拉起 sshd（随机端口、自签密钥）。**禁止** `MUXTERM_TEST_SSH_PORT=22`、禁止用户 `~/.ssh/config`。
- 远端 tmux **只** `-L muxterm-test-*`。清理：同一 `-L` 的 `kill-server`，然后 Drop 杀这个 sshd。
- `MUXTERM_SSH_CONFIG_PATH` 指到夹具生成的 `-F` config（`LoopbackSshd::apply_ssh_config_env`）。
- GTK：`AppWindow::new` 不要设本地 `cfg.tmux.socket`；用已有钩子 `test_open_spec(WorkspaceSpec::ssh_tmux(alias, Some(session), Some(remote_socket)))`。
- 无 sshd 二进制才 `eprintln skip` 后 return。有 sshd 就必须跑，**不许 `#[ignore]`**。
- 禁止 MockRuntime 喂字节冒充 SSH。

生产 QuickConnect SSH 仍可 attach 远端默认 socket；**测试路径必须把远端 `-L` 传进 `WorkspaceSpec.socket`**，并登记 `workspace_sockets`（重连要用）。`test_open_spec` 已经把 spec.socket 传进 `spawn_background_connect`。

---

## 3. 已落地的测试（RED → 你实现）

| ID | 文件 | 必须抓住 |
|---|---|---|
| W18a | `tests/tmux_ssh_feature_contract.rs` | 自启 sshd；远端 `/bin/cat` 先涂 `SSH_LIVE_*`；`TmuxRuntime::new_ssh_attach` 后 `search_workspace` 非空 |
| W18b | `tests/linux_ssh_e2e.rs` | GTK：VTE 含 token 且 `search_all` 含 token；`test_workspace_replica_ids` 含 `ssh` |
| W18c | `tests/linux_ssh_history_e2e.rs` | 离屏 token：search 命中；滚到顶 VTE 含 token；点 `muxterm-jump-latest` 回到尾标 |
| W18d | `tests/linux_ssh_reconnect_e2e.rs` | 远端 `detach-client -s`；15s 内水印消失；搜到 `GAP_*`；原 token 还在；BEL → blocked/notify；`resets <= 1` |
| W18e | `tests/linux_jump_count_e2e.rs` | 滚到顶再写 5 行；`muxterm-jump-latest` 的 label 含 `+` 和数字 |
| W18f | `tests/linux_search_scope_e2e.rs` | 控件 `muxterm-search-scope-pane/workspace/all`；pane 范围看不到另一 pane；workspace 看不到另一 ws；all 看得到。`muxterm-pane-find` + `muxterm-pane-find-entry` |
| W18g | `tests/linux_last_seen_e2e.rs` | 切到另一 pane、原 pane 继续写、切回：`muxterm-last-seen` 可见；点了 VTE 含 `LEFT_HERE_*` |
| W18h | `tests/linux_command_marks_e2e.rs` + emulate 单测 | `muxterm-cmd-mark-ok` / `muxterm-cmd-mark-fail`；fail 的 tooltip 含 `CMD_FAIL_*`；点击后 VTE 含失败命令。`TerminalState::command_marks()` 两条，exit 0 然后 1，带 `cmd_ok` / `cmd_fail` |

`AppWindow::test_open_pane_find` 现在是空的：接到与 Ctrl+F 同一条生产路径，显示 `muxterm-pane-find`。

`TerminalState::command_marks` 现在返回空切片：在 OSC 133 A/B/C/D 时写入 `CommandMark { seq, command, exit_code }`。命令文本是 B 与 C 之间的那一行。退出码解析 **整段** `D;<n>`（不要只取第一个字节，否则 12 会变成 1）。

命令刻度是滚动条旁 **极窄覆盖层**（愿景：十像素级，不是侧边栏）。成功绿、失败红。不要改 pane 字节。

上次看到这里：pane 失去可见时记下副本 seq；回来画 `muxterm-last-seen`。客户端覆盖层。

回底 +N：已有 `muxterm-jump-latest`；离开底部期间累计新行数，写进 label。

---

## 4. 怎么跑

```bash
cargo fmt --all -- --check
cargo check --features gtk
cargo test --features gtk --lib osc133_records_command_marks_with_exit_and_text -- --exact
cargo test --test tmux_ssh_feature_contract -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_ssh_e2e -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_ssh_history_e2e -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_ssh_reconnect_e2e -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_jump_count_e2e -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_search_scope_e2e -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_last_seen_e2e -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_command_marks_e2e -- --test-threads=1
# 回归
xvfb-run -a cargo test --features gtk --test linux_reconnect_e2e -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_attach_history_e2e -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_search_highlight_e2e -- --test-threads=1
cargo clippy --all-targets -- -D warnings
```

W17 crate 必须继续绿。

---

## 5. 完成定义

- [ ] 上表全部绿，断言没改弱，没有 `#[ignore]`
- [ ] 未连用户 22 端口；未对默认 tmux `kill-server`
- [ ] 英文 commit，未 push
- [ ] `fbc77e4` 仍是祖先

仍明确以后再做：ET、合盖一小时人手狗食、多窗口、Herdr。
