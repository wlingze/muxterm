# W17-PLAN.md — Linux tmux 1.0 测试门禁

> 日期：2026-08-17（`2026-08-17T12:07:06+08:00`）
> 工作目录：`/home/wlz/Developer/self/muxterm`
> 分支：`feat/linux-quickconnect-ui`（**不 push**）
> 先读：`docs/VISION-AUDIT.md`、`docs/W16-PLAN.md`、`docs/WORKSPACE.md` §6、`docs/SURFACE.md`、`docs/TESTING.md` §5.8、`AGENTS.md`
> 愿景 1.0：`docs/PRODUCT-VISION-STRATEGIC-REVIEW.md` §2.14.2 **A + B + C**；断线重连是 §2.15.2 公开发布硬门禁。
>
> **测试已经写好且必须先红。禁止改断言、阈值、token、widget_name 来「绿」。**
> 用户这轮不能狗食。1.0 是否完成只看这些测试。

---

## 0. 1.0 还差什么

W15/W16 已经绿：attach 历史、断线水印（杀 server）、blocked 看见不熄、正则、流量、跨 tab 搜索、peek 回复、连接超时、SSH 灯。

还没锁进测试的 1.0 项：

1. **自动重连**（session 还在）。W16b 只测了杀 server 的水印。愿景：合盖/SSH 掉线之后回来，零对话框，断线期间的输出和 bell 还在。
2. **Scroll lock**。回底按钮有了；向上看历史时新输出把人拽回去，按钮就没意义。
3. **搜索滚到 seq + 高亮**。W15b 只切 tab。`on_jump_pane` 现在丢掉 `SearchRow.seq`。
4. **Done 语义 live**：前台 OSC 133 D 不通知；后台 Done 看见才熄；静音后 BEL 不再亮。

不做（不是 1.0）：Herdr、ET、多窗口、上次看到这里、命令轨、Linux 托盘、把 SSH `#[ignore]` 改成默认绿、像素重写、push。

粘贴剥控制字符单测已有。GioSink 无 app 不 panic 的单测已补（应直接绿）。

---

## 1. 顺序（不要跳）

1. **W17a** `linux_reconnect_e2e`：`detach-client -s`，session 仍在，自动重连（**当前红**）
2. **W17b** `linux_scroll_lock_e2e`：滚到顶后新行不得出现在可见区（写计划时已绿，保持）
3. **W17c** `linux_search_highlight_e2e`：离屏命中跳转后 VTE 含 token + `muxterm-search-highlight`（**当前红**）
4. **W17d** `linux_attention_1_0_e2e`：前台静默 / 后台 Done 看见即熄 / 静音（写计划时已绿，保持）

一逻辑一英文 commit，`type(scope):`，无 Co-authored-by，不 `git add -A`，不 push。做完一个立刻下一个，不要等用户。

---

## 2. 夹具纪律

- tmux **只** `-L muxterm-test-*`。清理同一 `-L` 的 `kill-server`。
- pane 用 `/bin/cat`。Done 用 `tests/scripts/osc133_d_only.py`（**禁止**拿 `osc133_done.py`，它在 D 之后又写 BEL，会把 Done 盖成 Blocked）。
- GTK：无 DISPLAY skip；`xvfb-run -a`；`gtk4::test_synced`；每个 e2e crate **一个** AppWindow。
- `--test-threads=1`。禁止加长裸 sleep 当修复。
- 禁止 live `visible_ansi` → `vte.reset`。禁止回滚 `d1181679`。禁止把本轮断言标 `#[ignore]`。
- 不要削弱 `MIN_PANE_PX` / `MAX_OUTPUT_EVENTS_PER_SEC=400`。
- 重连播种用 `seed_raw`，不要 `seed_snapshot`/`vte.reset`。

---

## 3. 已落地的测试（RED → 你实现）

### W17a 自动重连

`tmux detach-client -s <session>` 拆掉 `-CC` client，**不**杀 session。

| 文件 | 断言 |
|---|---|
| `tests/linux_reconnect_e2e.rs` | detach 后 `has-session` 仍真。15s 内水印消失且 `search_all` 找到断线期间写入的 `GAP_*`。原来的 `RECONN_LIVE_*` 还在。断线期间 BEL → blocked 或 notify。`test_active_pane_resets() <= 1`。无模态框，窗口仍在。 |

实现要点：

- tmux Disconnected/Exited：**不要**拆 VTE。指数退避重连同一 socket/session（`WorkspacePool` 复用同一个 Workspace，不要新建一个丢掉 PaneBuf）。
- 重连成功：`capture_pane_with_history` 一次补洞，`seed_raw` 喂 VTE，隐藏 `muxterm-disconnect-overlay`。
- 断线期间的 BEL 不会再以 `%output` 出现。重连后查 `#{window_bell_flag}` / `#{pane_bell}`（或等价），重新推导 Blocked。内容靠 capture 补齐。
- 快恢复可以水印一闪而过；慢则维持水印。15s 内必须连回来。不要弹 `GtkMessageDialog`。

### W17b Scroll lock

| 文件 | 断言 |
|---|---|
| `tests/linux_scroll_lock_e2e.rs` | 滚到顶看见 `HIST_OFFSCREEN_*`。再往 pane 写 `LOCK_NEW_*`：`search_all` 能找到，VTE 可见区仍含离屏 token、**不含** `LOCK_NEW_*`。`muxterm-jump-latest` 仍可见。 |

实现要点：VTE `feed` 时若 `vadjustment` 不在底部，不要把 value 设到 upper。跟随只在「本来就在底部」时发生。

### W17c 搜索滚到命中 + 高亮

`on_jump_pane(ws, pane)` 丢掉了 `SearchRow.seq`。

| 文件 | 断言 |
|---|---|
| `tests/linux_search_highlight_e2e.rs` | 默认在尾部，可见区没有离屏 token。点 `muxterm-search-hit-*` 后面板关、VTE 可见区含 token、`muxterm-search-highlight` 可见。 |

实现要点：跳转回调带上 seq（或行文本）。切 tab/pane 之后把 VTE 滚到该行。高亮是客户端覆盖层，不要改 pane 字节。widget_name 必须是 `muxterm-search-highlight`。

### W17d Done / 前台静默 / 静音

| 文件 | 断言 |
|---|---|
| `tests/linux_attention_1_0_e2e.rs` | 前台 `send_command_done_no_bel`：done 计数 0，log 无 complete/done/完成。后台同一脚本：done≥1 且有 notify；`test_switch_pane` 到该 pane 后 done=0。Attention 面板 `muxterm-attention-mute-1h` 之后红点 0；再 BEL 红点仍 0，通知条数不增加。 |

---

## 4. 怎么跑

```bash
cargo fmt --all -- --check
cargo check --features gtk
cargo test --features gtk --lib gio_sink_without_app -- --exact
xvfb-run -a cargo test --features gtk --test linux_reconnect_e2e -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_scroll_lock_e2e -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_search_highlight_e2e -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_attention_1_0_e2e -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_attach_history_e2e -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_disconnect_e2e -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_live_e2e -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_render_e2e -- --test-threads=1
cargo clippy --all-targets -- -D warnings
```

W15/W16 crate 必须继续绿。

---

## 5. 完成定义（这就是 Linux tmux 1.0 测试门禁）

- [ ] 上面四个 e2e 绿，断言没改弱
- [ ] `gio_sink_without_app_does_not_panic` 绿
- [ ] W15/W16 与 `linux_live_e2e` / `linux_render_e2e` / `linux_workspace_attach_e2e` 仍绿
- [ ] 英文 commit，未 push

绿了之后，1.0 测试意义上的 A+B+C 在 Linux tmux 路径上锁住了。仍缺的（文档写明，不要假装做完）：真 SSH attach（`#[ignore]`）、自动重连的「合盖一小时」人手、ET、多窗口、上次看到这里。
