# FEATURE-E2E-PLAN.md — 功能保真测试与实现（交给 Codex）

> 日期：2026-08-16（`2026-08-16T18:50:00+08:00`）
> 工作目录：`/home/wlz/Developer/self/muxterm`
> 分支：`feat/linux-quickconnect-ui`（**不 push**）
> 先读：`docs/WORKSPACE.md` §6、`docs/SURFACE.md`、`docs/TESTING.md` §5.4/§5.5、`docs/WORKSPACE-PLAN.md` W13–W14、`AGENTS.md`
>
> **测试已经写好且必须先红。禁止改断言、阈值、token 名字来「绿」。**

---

## 0. 为什么要做

用户路径（SSH attach 已有多 pane / Codex TUI / 搜索 / 任务跑完）一直靠手动点。旧 e2e 绿了，产品仍白屏、搜不到、完不成通知：

- `linux_live_e2e`：空 session 再 echo
- `linux_search_e2e`：Mock PaneBuf，不经过 AppWindow attach
- `linux_attention_e2e`：`test_feed_replica` 注入 BEL
- `linux_ssh_e2e`：CoreBridge 事件喂进 **另一个 Mock Workspace**
- `linux_render_e2e`：静态 `codex-tui-sanitized.txt` feed 进裸 PaneView
- 1820.log：118 608 条 `实时 %output 交付`，0 `%pause`，仍看不出播种失败

W13 先保证 **attach 已有画面**（非空 PaneBuf / VTE / 布局 / pause）。
W14 在这块地基上测 **用户功能**。W12 分层漏项排在两者之后。

---

## 1. 顺序（不要跳）

1. **W13 绿**：`tmux_attach_contract` + `linux_workspace_attach_e2e`（再确认 `linux_live_e2e` / `linux_render_e2e` 仍绿）
2. **W14 绿**：`tmux_feature_contract` + `linux_feature_e2e`（有 sshd 再跑 `tmux_ssh_feature_contract --ignored`）
3. **W12**：`open_spec` 复用、core 不 import platform CLI、spec 单测。不要和像素/通知混 commit。

每步一逻辑一英文 commit，`type(scope):`，无 Co-authored-by，不 `git add -A`，不 push。

---

## 2. 夹具纪律

- tmux **只** `-L muxterm-test-*`。清理同一 `-L` 的 `kill-server`。禁止默认 server。
- pane 命令用 `/bin/cat`、`/usr/bin/tail`、`python3 -u tests/scripts/mock_codex.py`。不要无路径 `cat`。
- **先**在 tmux 里画 token / 跑脚本，**再** Muxterm attach。空 session echo 不算。
- GTK：无 DISPLAY skip；有显示 `xvfb-run -a` + `gtk4::test_synced`。每个 e2e crate **一个** AppWindow。
- `--test-threads=1`。硬超时轮询，禁止加长裸 sleep 当修复。
- 禁止 live 路径 `visible_ansi` → `vte.reset`。禁止回滚 `fbc77e4`。禁止 `include_str!` 大 dogfood log。
- 不要把 Search / Done / flood 标 `#[ignore]`。SSH 套件本来就是 `#[ignore]`（无 sshd）。

---

## 3. 已落地的测试（RED → 你实现）

| 文件 | 断言 |
|---|---|
| `tests/support/workspace_attach_contract.rs` | 2tab/3pane + token，W13 |
| `tests/tmux_attach_contract.rs` | core `pane_output` 含 token；CUP 洪水 1s ≤ 400 事件 |
| `tests/linux_workspace_attach_e2e.rs` | VTE 非空；pane ≥ 40px；切 tab；洪水 resets ≤ 1 |
| `tests/scripts/mock_codex.py` | CUP 帧 + `TOKEN_HEADER`/`TOKEN_BODY`/`TOKEN_PROMPT` + OSC 133 D + `MOCK_CODEX_DONE` |
| `tests/support/feature_e2e_contract.rs` | 2 pane `/bin/cat`；OSC 133 D；BEL；mock-codex respawn；tail -f |
| `tests/tmux_feature_contract.rs` | 搜索 / CommandDone+BEL / mock-codex PaneBuf / tail-f / tracing 门禁 |
| `tests/linux_feature_e2e.rs` | 一个 AppWindow 串：搜索跳转 VTE、后台 Done 通知、mock-codex VTE、tail-f VTE |
| `tests/tmux_ssh_feature_contract.rs` | SSH attach 已有 token（`--ignored`） |

AppWindow 钩子（已加，不要删）：`test_pane_vte_text` / `test_layout_leaf_ids` / `test_flush_feeds` / `test_poll_output_event_count` / `test_search_all` / `test_switch_pane` / `test_attention_done_count` / `test_notifications_recorded`。

---

## 4. W13 实现要点（仍红就先做这个）

证据：`test_2026-0816-1820.log`（2026-08-16T10:20:51Z SSH `ryzen` / `yaklang-workspace`）。

1. **播种**：attach 已有 session，`capture-pane` 快照必须进 `pane_output` **和** VTE。只有布局、没有字节 = 白屏。
2. **布局**：3 leaf → 3 个面积 ≥ 40px 的控件，≥2 个 GtkPaned。
3. **流控**：忙 pane 发 `refresh-client -A '%N:pause'`（iTerm2 `pausePanes`）。1s 内 `PaneOutput` ≤ 400。
4. **切 tab**：tab2 token 再切回，tab1 token 还在 Surface。

不要改 `MIN_PANE_PX`、`MAX_OUTPUT_EVENTS_PER_SEC=400`、`CUP_FLOOD_FRAMES`、「VTE 不能空」。

---

## 5. W14 实现要点

### 5.1 搜索（生产路径）

`linux_search_e2e` **保留**，但它不是完成定义。

必须：

1. attach 后 `WorkspacePool::search_all(token)` 非空（token 来自 tmux `/bin/cat` 播种）。
2. `test_open_panel(2)` → `muxterm-panel-entry` 设 query → 出现 `muxterm-search-hit-*`。
3. activate 命中行 → `SwitchPane` → 该 pane VTE 含 token。

搜索命中来自 PaneBuf（Index），显示仍是 Surface 原始字节。不要为了搜索 `vte.reset`。

建议日志：`tracing::debug!(target: "muxterm::search", hits = n, query = %query, ...)`。

### 5.2 通知：Blocked **和** 任务完成

已有：`NotificationSink::notify_blocked` + `take_new_blocked_notifications`。BEL → Blocked。

缺口：OSC 133 D → `PaneStatus::Done` 没有桌面通知。

要加：

```rust
// attention_ui.rs
pub trait NotificationSink {
    fn notify_blocked(&self, workspace_id: &str, body: &str);
    fn notify_done(&self, workspace_id: &str, body: &str);
}

// engine.rs
pub fn take_new_done_notifications(&mut self) -> Vec<String>;
```

- RecordingSink：`"{workspace_id}: done"` 或 body 含 `complete`/`finished`/`done`（e2e 按这些词断言）。
- 16ms poll **和** `test_poll_once` 都要 drain done，写入 `notification_log`。
- **只通知非前台 pane。** `apply_attention_from_workspace` 对前台会 `on_became_visible` 把 Done 清成 Idle（E6）。测试已 `test_switch_pane(pane0)` 再给 pane1 发 D。
- 不要把 Done 和 BEL 绑死：单独 D = Done 通知；单独 BEL = Blocked。两者都发时终态是 Blocked（转移表）。

建议日志：`tracing::info!(target: "muxterm::notify", kind = "done"|"blocked", ws = %id)`。

### 5.3 mock-codex.py

不要连真 Codex。脚本在隔离 pane 里画 CUP 帧，停在末帧，发 OSC 133 D。

PaneBuf / VTE 必须同时有 `TOKEN_HEADER` 和 `TOKEN_PROMPT`（允许 `MOCK_CODEX_FRAME=` / `MOCK_CODEX_DONE`）。白屏或只吃半帧 = 失败。

CUP 洪水仍走 W13 pause，不要为了 mock-codex 把 400 上限放宽。

### 5.4 `/bin/cat` 与 `tail -f`

cat：夹具播种（W13/W14）。

tail -f：`respawn-pane` 到 `/usr/bin/tail -f <tmpfile>`，先写 `TAIL_BOOT`，attach/respawn 后再追加 `TAIL_FOLLOW_TOKEN`。**追加行**必须进 PaneBuf 和 VTE，不能停在启动那截。

### 5.5 SSH tmux

`TmuxRuntime::new_ssh_attach(alias, Some(remote_socket), session)`。

远端：`tmux -L <remote_socket> new-session ... -- /bin/cat`，**先** `-l` token，capture 确认，**再** connect。

`MUXTERM_SSH_CONFIG_PATH` 指向 `SshTestEnv` 的临时 config。清理：远端 `kill-server` 必须带同一 `-L`。

禁止再把 SSH 事件喂进 MockRuntime。有空可改 `linux_ssh_e2e.rs` 去掉 Mock，但不许削弱 `tmux_ssh_feature_contract`。

无 sshd：不要把该 crate 从 ignore 拿掉冒充全绿。

### 5.6 Debug 日志

1820.log 不能定位「快照进没进、pause 发没发、VTE feed 了几次」。

**必须出现的 target 字符串（源码门禁已写）：**

| target | 文件 | 记什么 |
|---|---|---|
| `muxterm::tmux::seed` | `backend.rs` | attach capture 完成、每个 pane 快照字节数 |
| `muxterm::tmux::pause` | `backend.rs` | 对哪些 pane 发了 `refresh-client -A` |
| `muxterm::layout` | `layout_host.rs` | apply 后 leaf 数、控件分配 |
| `muxterm::surface` | `pane_view.rs` | feed 摘要（次数/字节），不要每个 fragment |
| `muxterm::search` | `workspace.rs` | query、命中数 |
| `muxterm::notify` | `window.rs` | blocked/done 通知 |

`「实时 %output 交付」` 不得再包在 `tracing::debug!` 里。改 `trace!` 或按 pane 限速（例如每秒 1 条 + 字节计数）。

---

## 6. 怎么跑（汇报真实退出码）

```bash
cargo fmt --all -- --check
cargo check --features gtk
cargo test --test tmux_attach_contract -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_workspace_attach_e2e -- --test-threads=1
cargo test --test tmux_feature_contract -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_feature_e2e -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_live_e2e -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_render_e2e -- --test-threads=1
cargo clippy --all-targets -- -D warnings
```

有 sshd：

```bash
eval "$(./scripts/ci/setup-sshd.sh)"
cargo test --test tmux_ssh_feature_contract -- --ignored --test-threads=1
```

大 e2e 仍红：先拆单测（capture 播种、pause 触发、Done drain），再回来。最终大 e2e 必须绿。

---

## 7. 完成定义

- [ ] W13 两 crate 绿，断言未被改弱
- [ ] W14 core + GTK crate 绿
- [ ] `linux_live_e2e` / `linux_render_e2e` 仍绿
- [ ] 源码含 §5.6 的 tracing target；不再 debug 每条 `%output`
- [ ] 有 sshd 时 SSH 契约绿；没有则保持 `#[ignore]`
- [ ] 英文 commit，未 push

不要做：Herdr Runtime、重做 F 像素、杀默认 tmux、把 Window 映射成 tmux window。
