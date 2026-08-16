# W16-PLAN.md — 愿景 1.0 缺口（历史 / 断线水印 / 注意力语义）

> 日期：2026-08-17（审计时本机 `2026-08-17T02:43:19+08:00`）
> 工作目录：`/home/wlz/Developer/self/muxterm`
> 分支：`feat/linux-quickconnect-ui`（**不 push**）
> 先读：`docs/VISION-AUDIT.md`、`docs/WORKSPACE.md` §6、`docs/SURFACE.md`、`docs/TESTING.md` §5.7、`AGENTS.md`
> 对照愿景：`docs/PRODUCT-VISION-STRATEGIC-REVIEW.md` §2.11.8 scrollback、§2.15.1 红点熄灭、§2.15.2 断线、§2.15.4 回底按钮
>
> **W15 必须先绿。** 本轮测试已经写好且必须先红。禁止改断言、阈值、token、widget_name 来「绿」。

---

## 0. 先把 W15 收口

HEAD 在写本计划时已有 W15a/b commit（流量、跨 tab 搜索）。未提交的是 W15e/c/d（peek 回复、连接超时、SSH 灯）。

W15 绿的定义：`docs/W15-PLAN.md` 里那些 crate 全绿，断言没被改弱。然后马上做 W16，不要等用户说继续。

`window.rs` 里若还留着 `eprintln!("DBG apply_attention ...")`，顺手删掉，不要新加调试打印。

---

## 1. 顺序（不要跳）

1. **W16a** attach 历史 + 回底按钮（同一 GTK crate，一个 AppWindow）
2. **W16b** 断线水印
3. **W16c** blocked 熄灭语义 + TOML 正则 live

一逻辑一英文 commit，`type(scope):`，无 Co-authored-by，不 `git add -A`，不 push。

---

## 2. 夹具纪律

- tmux **只** `-L muxterm-test-*`。清理同一 `-L` 的 `kill-server`。
- pane 用 `/bin/cat`。不要无路径 `cat`。
- GTK：无 DISPLAY skip；`xvfb-run -a`；`gtk4::test_synced`；每个 e2e crate **一个** AppWindow。
- `--test-threads=1`。禁止加长裸 sleep 当修复。
- 禁止 live `visible_ansi` → `vte.reset`。禁止回滚 `fbc77e4`。禁止把本轮断言标 `#[ignore]`。
- 不要削弱 `MIN_PANE_PX` / `MAX_OUTPUT_EVENTS_PER_SEC=400`。
- 不要做 Herdr、ET、多窗口、上次看到这里、命令时间轴、像素重写。

---

## 3. 已落地的测试（RED → 你实现）

### W16a 历史播种 + 回底

核实（本机 tmux 3.7b，2026-08-17）：24 行高的 pane 里先写 token 再写 40 行 padding 之后，
`capture-pane -p` **不含** token，`capture-pane -p -S -` **含** token。
官方语义（tmuxr 对 man 的转述，<https://jeroenjanssens.github.io/tmuxr/reference/capture_pane.html>）：
默认只抓可见区；`-S` 为 `-` 表示历史起点；负数是历史行。
不要无界 `-S -` 一次灌整段 history（愿景 §2.15.6 内存预算）。用 `-S -N`，N = `scrollback.lines`（默认 10000）。

| 文件 | 断言 |
|---|---|
| `src/core/runtime/tmux/command.rs` `capture_pane_with_history` | `PaneId(3), 10000` → `capture-pane -e -p -S -10000 -t %3` |
| `src/core/runtime/tmux/backend.rs` 单测 | `backend.rs` 源码含 `capture_pane_with_history(`；**不得**再拼 `capture-pane -e -p -t %{}` |
| `tests/linux_attach_history_e2e.rs` | 夹具：可见 capture 无 token、`-S -` 有 token。attach 后 `search_all` 命中。`test_scroll_pane_to_top` 之后 VTE 含 token。出现可点的 `muxterm-jump-latest`。点完 VTE **不再**含离屏 token（回到尾部） |

实现要点：

- `query_capture_pane` 改发 `cmd::capture_pane_with_history(pane, n)`。`n` 用配置的 scrollback 上限，没有就 10000。
- 快照仍只喂一次，禁止和 live `%output` 双份追加（已有单测必须继续绿）。
- 历史字节进 PaneBuf **和** VTE。只修搜索、VTE 滚到顶仍没有，e2e 第二截会红。
- 回底：用户把 VTE `vadjustment` 拉离底部之后显示按钮；点击把滚动值设回底部并恢复跟随。不要改 pane 内容。

### W16b 断线水印

愿景 §2.15.2：断开不弹窗；最后一帧留下；角落水印「已断开 / 重连中」。

| 文件 | 断言 |
|---|---|
| `tests/linux_disconnect_e2e.rs` | attach 后 VTE 有 token。对该隔离 socket `kill-server`（**必须**带同一 `-L`）。窗口仍 `is_visible`。VTE **仍**含 token（禁止 reset 清空）。`muxterm-disconnect-overlay` 可见。widget 树里没有 `GtkMessageDialog` / `GtkAlertDialog` |

实现要点：

- `BackendStatus::Disconnected`（以及 control client 死掉）不要拆 LayoutHost / 不要 `vte.reset`。
- Overlay 盖在终端上，降饱和或半透明即可，不要新开 `gtk::Window` 当模态。
- `Exited` 不得因为「tmux server 没了」就 `pending_close` 把整窗关掉。用户要看着最后一帧。
- 本轮 **不必** 做自动重连。水印文案写「已断开」即可；「重连中」留给 W17。

### W16c 注意力语义 live

引擎单测已经锁死转移表。缺的是 attach 之后的接线。

| 文件 | 断言 |
|---|---|
| `tests/linux_attention_semantics_e2e.rs` | 两 pane `/bin/cat`。后台 pane 真 BEL → `test_attention_blocked_workspaces() >= 1`。`test_switch_pane` 到该 pane 之后红点仍 ≥ 1。`test_send_input` 之后红点变 0。配置 `blocked_regex = ["NEED_INPUT"]`，后台 pane 写出该行（无 BEL）→ 红点再 ≥ 1 |

依赖 W15e 的 `%output` BEL 路径。W15 没绿之前不要把 BEL 改成 `test_feed_replica` 冒充。

实现要点：

- 保持 `Blocked + BecameVisible` 仍是 Blocked。不要在切 tab / 切 pane 时对 blocked 调「已读」。
- `test_send_input` 已经会 `on_user_input`；生产路径（peek 和 VTE `connect_input`）也必须走同一函数。
- 正则只看 `last_line`，debounce 用配置。非法正则跳过，不要 panic。

---

## 4. 怎么跑

```bash
cargo fmt --all -- --check
cargo check --features gtk
cargo test --lib runtime::tmux::command::tests::capture_pane_with_history -- --exact
cargo test --lib runtime::tmux::backend -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_attach_history_e2e -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_disconnect_e2e -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_attention_semantics_e2e -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_live_e2e -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_render_e2e -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_workspace_attach_e2e -- --test-threads=1
cargo clippy --all-targets -- -D warnings
```

W15 crate 也必须继续绿。

---

## 5. 完成定义

- [ ] W15 全绿（若你接手时还没绿，先做完 W15）
- [ ] 上面三个 e2e + 两条单测绿，断言没改弱
- [ ] `linux_live_e2e` / `linux_render_e2e` / `linux_workspace_attach_e2e` 仍绿
- [ ] 英文 commit，未 push

不要做：Herdr、ET、多窗口、上次看到这里、命令轨、把 Window 映射成 tmux window、杀默认 tmux。
