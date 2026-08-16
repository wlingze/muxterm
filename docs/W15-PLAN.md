# W15-PLAN.md — dogfood UX + 通知 peek/回复（交给 Codex）

> 日期：2026-08-17（`2026-08-17T02:22:54+08:00`）
> 工作目录：`/home/wlz/Developer/self/muxterm`
> 分支：`feat/linux-quickconnect-ui`（**不 push**）
> 先读：`docs/WORKSPACE.md` §6、`docs/SURFACE.md`、`docs/TESTING.md` §5.4–§5.6、`AGENTS.md`
> 用户 dogfood：attach/切 tab 已可用。本轮修流量、搜索跳转、连不上卡死、SSH 灯，以及 **通知 → 选中渲染 pane → 快速回复**。
>
> **测试已经写好且必须先红。禁止改断言、阈值、token 名字来「绿」。**

---

## 0. 总目标对照（现在做到哪）

产品树：`WorkspacePool → Workspace → Tab → Pane`。Window 只是 GUI 体现。tmux 只活在 `runtime/tmux`。

| 块 | 现状 | 还差 |
|---|---|---|
| 本地 tmux attach | W13 播种 + pause + 切 tab 像素缓存。用户 2026-08-17 说切换完美 | 本轮 UX（流量/搜索跳/卡死） |
| 本地 tmux 搜索 Index | `search_all` 能命中（W14） | 跳到**别的 tab**、关面板、长行撑破宽度 |
| 通知 Blocked | 引擎 + `linux_attention_e2e` **注入 replica BEL** | **真 `%output` BEL** → 红点/列表/peek/回复 |
| 通知 Done | W14 `linux_feature_e2e` 后台 OSC 133 D → `notify_done` | 保持；不要和 BEL 绑死 |
| Attention peek + 快速回复 | `linux_panel_e2e` 有跳转/放大/静音；**从不 `test_emit_input`**；peek 字节是假的 | 生产路径：选中行 → 小 VTE 是该 pane 真字节；按键 `WriteRaw` 进隔离 tmux |
| SSH tmux 协议 | `TmuxRuntime::new_ssh_attach`、`ConnectTimeout=10`、`tmux_ssh_feature_contract` `#[ignore]` | GTK **`rt.block_on(open_spec)` 堵主线程**（连 Mac 会冻整窗）；无可达性灯；popover 把累计字节标成 `B/s` |
| Linux 前端 chrome | 状态栏、三 tab 面板、快捷键、偏好 | 本轮四项 UX + 通知闭环 |
| FFI | `muxterm_workspace_*` 给 TUI/CLI；Linux GUI 直接 `WorkspacePool` | 够用。本轮不要重做 FFI |
| macOS / Herdr | 上一代 ConnectionPool；Herdr 未开工 | **本轮不做** |

结论：tmux 产品路径在 W13/W14 之后已经能 attach 看画面。剩下是 **卡死、搜不到格子、流量撒谎、通知不能当对讲机用**。不要开新层。

W15 之后（先别做）：断线水印（VTE 留着、提示重连）；搜索命中行高亮；文档归档/拆 god file。仍然不要 Herdr、不要像素重写、不要 push、不要杀默认 tmux。

---

## 1. 通知：现在测了什么，缺什么

| 路径 | 有没有测试 | 够不够 |
|---|---|---|
| 注入 BEL → 红点/标题/Attention 列表/小 VTE 含 `hello` | `linux_attention_e2e` `test_feed_replica` | 测 chrome，**不是** `%output` |
| 真 tmux OSC 133 D + BEL → `AttentionSignal` | `tmux_feature_contract` | core 信号有；`send_background_bel` 曾是空函数，BEL 靠 `osc133_done.py` 尾巴 |
| 后台 Done → `notify_done` + VTE | `linux_feature_e2e` | 有 |
| 真 `%output` BEL → `notify_blocked` + 选中 peek + 快速回复进 pane | **没有** | 本轮 W15e 必须有 |
| 面板里 peek 按键走 `on_send_input` | `linux_panel_e2e` 只登记回调，**从不触发** | W15e 补 |

`linux_attention_e2e` 文件头写「覆盖真实 printf BEL」是假的，代码只有 replica。不要靠改注释假装测过。live 路径进 `linux_feature_e2e`（已经有一个 AppWindow）。

---

## 2. 顺序（不要跳）

1. **W15a** 流量：`format_bytes` / `format_rate` / 速率差；popover 两行（速率 + 累计），禁止 `1234B/s`
2. **W15b** 搜索：跳转 = 激活工作区 + `SwitchTab(tab_id)` + `SwitchPane` + 尽量滚到命中；关面板；长行 ellipsize，面板宽 ≤ 窗口
3. **W15e** 通知闭环：真 BEL → 通知；选中行小 VTE 是该 pane；peek 按键进 tmux pane
4. **W15c** 连接：`open_spec` 离开 GTK 线程；硬超时 ~8–10s；失败进 `notification_log`；16ms `refresh` 不得永久堵
5. **W15d** SSH 灯：后台探测；QC SSH 行 + host picker 用同一套 `muxterm-ssh-dot-*`

一逻辑一英文 commit，`type(scope):`，无 Co-authored-by，不 `git add -A`，不 push。

---

## 3. 夹具纪律（同 W13/W14）

- tmux **只** `-L muxterm-test-*`。清理同一 `-L` 的 `kill-server`。
- pane 用 `/bin/cat` / `python3 -u tests/scripts/*.py`。不要无路径 `cat`。
- GTK：无 DISPLAY skip；`xvfb-run -a`；`gtk4::test_synced`；每个 e2e crate **一个** AppWindow。
- `--test-threads=1`。禁止加长裸 sleep 当修复。
- 禁止 live `visible_ansi` → `vte.reset`。禁止回滚 `fbc77e4`。禁止把 Search/Done/flood/本轮新断言标 `#[ignore]`。
- 不要削弱 `MIN_PANE_PX` / `MAX_OUTPUT_EVENTS_PER_SEC=400`。

---

## 4. 已落地的测试（RED → 你实现）

### W15a 流量

| 文件 | 断言 |
|---|---|
| `src/core/format.rs` | `1536 → "1.5 KB"`；`1024 → "1.0 KB"`；`999 → "999 B"`；`1048576 → "1.0 MB"`；`format_rate(1536) == "1.5 KB/s"`；`rate_bps` 用时间差，回绕当 0 |
| `tests/linux_chrome_e2e.rs` | popover **不得**含 `1536B/s` / `1234B/s`；必须同时有人类可读累计和 `/s` 速率（KB/MB/B） |

`ConnectionSummary` 已加 `down_rate` / `up_rate`（字节/秒）。`refresh_connection_summary` 要用连续两次 `traffic_bytes()` + 墙钟算速率，不能把累计标成 `/s`。

popover 建议两行（文案可调，断言认单位）：

```
↓ 1.5 KB/s  ↑ 56 B/s
total ↓ 1.5 KB  ↑ 56 B
```

### W15b 搜索跳转 + 宽度

| 文件 | 断言 |
|---|---|
| `tests/linux_search_e2e.rs` | 超长命中行：`muxterm-panel` 分配宽度 ≤ 窗口宽度 |
| `tests/linux_search_jump_e2e.rs` | 2tab 夹具；token 在 **tab 2**；当前 tab 1；Search 激活命中后 `test_active_tab_id` 是 tab 2，VTE 含 token，面板关闭 |

`SearchRow.tab_id` 已经有。`jump_to_attention_pane` 只 `SwitchPane` 不够。实现里按 pane 查 tab 也可以，但结果必须切 tab。

命中 Label：`ellipsize = End`，`hexpand`，不要把整行 `{workspace} · {pane} · {line}` 当最小宽度。

### W15e 通知 → peek → 回复

| 文件 | 断言 |
|---|---|
| `tests/support/feature_e2e_contract.rs` `send_background_bel` | 对仍在 `/bin/cat` 的 pane `send-keys -H` 发 `0x07`（不要 respawn；`-l` 会变成 `^G`） |
| `tests/linux_panel_e2e.rs` | 选中注意力行后 `test_emit_peek_input(b"REPLY_PANEL")` → `on_send_input` 收到 `(legion, 1, b"REPLY_PANEL")` |
| `tests/linux_feature_e2e.rs` | **先于** Done respawn：真 BEL → `test_notifications_recorded` 含 attention/blocked；Attention tab 小 VTE 含 `bg_token`；peek 输入 `W15_REPLY` 后 `capture-pane` 该 pane 含 `W15_REPLY` |

钩子（已加，不要删）：`quickconnect_panel::test_emit_peek_input`、`AppWindow::test_peek_emit_input`（必须走 peek VTE 的 `connect_input`，禁止只 `WriteRaw` 绕过小 VTE）。

跳转按钮仍应 `SwitchTab`+`SwitchPane`（与 W15b 同一函数）。

### W15c 连接不冻 GTK

| 文件 | 断言 |
|---|---|
| `tests/linux_connect_timeout_e2e.rs` | 已有本地 AppWindow；`test_connect_target` 指向 `192.0.2.1`（TEST-NET，应黑洞）；调用 **500ms 内返回**（工作放到后台）；随后主循环还能 `pump`；12s 内 `test_notifications_recorded` 含 fail/timeout/unreachable/refused 一类词 |

禁止在 GTK 线程 `rt.block_on(pool.open_spec)`。`refresh()` / 16ms poll 同样禁止同步死等 SSH。

### W15d SSH 灯

| 文件 | 断言 |
|---|---|
| `src/core/transport/ssh/probe.rs` | `ssh_probe_args` 含 `BatchMode=yes`、`ConnectTimeout=2`、远端 `true`；**不要** `-tt` |
| `tests/linux_panel_e2e.rs` | 注入 `ssh_reach`：`ryzen=Ok` 有 `muxterm-ssh-dot-ryzen` + class `muxterm-ssh-dot-ok`；`dead=Err` → `muxterm-ssh-dot-err` |

生产：面板打开时后台探测，TTL 缓存，**不要** 16ms tick 扫 SSH。loopback sshd = 绿；没有 sshd = 红/灰，禁止 skip-as-pass。QC 列表和 host picker 用同一个 `ssh_dot_widget_name` / `ssh_dot_css_class`。

---

## 5. 实现要点（自由路径，断言不自由）

1. **流量**：`core::format` 已有单测。你要接 `StatusBar::set_connection_summary` 和 `window.rs` 的差量。1024，一位小数，空格：`1.5 KB`。
2. **跳转**：`activate_workspace` + `Task::SwitchTab` + `Task::SwitchPane`；搜索再按 `seq` 滚 VTE（能滚就滚；最低 VTE 文本含 token）。
3. **peek 回复**：选中行 `peek_bytes` 必须是该 pane 原始字节（window.rs 已接 `pane_raw_bytes`）。`connect_input` → `Task::WriteRaw { target: 选中 pane }`，不是前台 pane。
4. **BEL**：后台 pane、E6 前台会清 Done 不会清 Blocked。测试已 `test_switch_pane(pane0)` 再打 pane1。
5. **超时**：`glib::spawn_future_local` / 线程 + idle 回调回 GTK；失败 `notification_log` + tracing。不要只 `tracing::error`。
6. **灯**：`SshReach::{Unknown,Ok,Err}`；widget_name `muxterm-ssh-dot-{alias}`。

---

## 6. 验证门

```bash
cargo fmt
cargo check --features gtk
cargo test --lib format:: -- --test-threads=1
cargo test --lib transport::ssh::probe -- --test-threads=1
cargo test --test tmux_feature_contract -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_chrome_e2e -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_search_e2e -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_search_jump_e2e -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_panel_e2e -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_feature_e2e -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_connect_timeout_e2e -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_render_e2e -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_live_e2e -- --test-threads=1
cargo clippy --all-targets --features gtk -- -D warnings
```

W13/W14 套件必须仍绿。

---

## 7. 禁止

- Herdr、重做 FFI、重做像素、macOS 客户端
- `git add -A`、push、Co-authored-by
- 默认 tmux `kill-server` / `kill-session` / `kill-pane`
- 为了绿把 `192.0.2.1` 改成立刻失败的假实现却仍 `block_on` GTK
- 用 `test_feed_replica` 冒充 W15e 的 `%output` BEL
- 快速回复只改 Recording 回调、不进 tmux `capture-pane`
