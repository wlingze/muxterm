# SURFACE-PLAN.md — Codex 实施合同（单面架构）

> **2026-08-15 22:50：本文已冻结。** F1–F6 已在 `feat/linux-quickconnect-ui` 落地
> （HEAD `975a94d`：`fix(protocol): parse tmux pane ids with % prefix and %pause`）。
> **下一轮只执行 [`WORKSPACE-PLAN.md`](WORKSPACE-PLAN.md) W1–W8。**
> 架构：[`WORKSPACE.md`](WORKSPACE.md)；像素契约仍以 [`SURFACE.md`](SURFACE.md) 为准（live 禁止 dump）。
> 不要按下面 F 阶段再做一遍，不要重做 C7/C8/E。
>
> 原执行计划（只做 F1–F6）。不要重做 C7 / C8 / Phase E。
> 分支：`feat/linux-quickconnect-ui`（冻结时 HEAD `975a94d`，**不 push**）
> 修订：2026-08-15 21:26 CST；冻结 2026-08-15 22:50 CST（`2026-08-15T22:50:10+08:00`）

Phase E 的 git 提交（`403192b`…`d802f05`）**不算渲染完成**。E-R1「VTE 只显示 replica `visible_ansi`」是错处方，已由 `18b3183` 落地并在 2105 真机上失败。本轮把显示路径翻过来。

---

## 0. 执行合同

1. 先读：`docs/SURFACE.md`、本文、四份 dogfood 摘录、`tests/samples/codex-tui-sanitized.txt`。对照 `/home/wlz/Developer/terminal/{ivyterm,iterm2,ghostty,cmux,tmux}`，不要抄 UI。
2. GUI：`xvfb-run -a` + `gtk4::test_synced`。同进程一个 `AppWindow`。`linux_render_e2e` 继续一个 `gtk4::Window`、场景函数顺序跑。
3. 隔离 tmux `-L muxterm-test-*`。前台命令用 `/bin/cat`。不复制 `IsolatedTmux`。
4. 一次一个 F，先 RED。commit `type(scope): English`，无 Co-authored-by，不 push。
5. **禁止重做** C7 session-id、C8 ASCII 几何（Index 单测可留）。**禁止**把 Phase E 标回「未开始」再做一遍搜索/attention 抛光。
6. **禁止** live 路径 `visible_ansi` → `vte.reset` + `vte.feed`。`present_from_replica` 不得出现在 `%output` / CUP 风暴 / `refresh_ui`。
7. **禁止** `include_str!` 原 `test_2026-0815-*.log`（34MB）。摘录在 `tests/samples/dogfood-*.txt`。
8. **禁止** 默认 tmux server 的 `kill-server` / `kill-session` / `kill-pane`。
9. 搜索 UI、attention 抛光、主题：**F1–F6 绿之前不动**。attention 小 VTE 若仍 `present_from_replica`，F2 一并改成 raw feed，不要新功能。

验证门（每 F 汇报）：

```bash
cargo fmt
cargo check
cargo test
cargo clippy -- -D warnings
xvfb-run -a cargo test --features gtk --test linux_render_e2e -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_live_e2e -- --test-threads=1
```

---

## 1. 现状（代码完成了任务么？）

| 阶段 | git | 真机 |
|---|---|---|
| A–C | 有 | 能连；切 tab / session id 好 |
| D C8 | 有 | 状态点好；Codex 仍乱 |
| E E1–E6 | **有**（到 `d802f05`） | 2105：闪烁、白屏、只见中段、句子越来越长、滚轮错 |
| **F Surface（本轮）** | 未开始 | 显示路径仍是三重仿真 |

2105（`tests/samples/dogfood-2026-0815-2105.txt`）：SSH `ryzen` → `tmux -CC attach -t yaklang-workspace`；298404 条 `%output`；819 次 `send-keys -l`；19 次 `capture-pane`；8 次 `%64` WARN。log **没有** pane 像素。渲染测 fixture + 隔离 tmux，不要 grep 34MB。

对照生产路径（改这些，不是另起炉灶）：

| 文件 | 现在 | Surface |
|---|---|---|
| `src/platform/linux/window.rs` `STATE_PANE_OUTPUT` | seeded→`feed_output`；否则 `present_from_replica` | 未 synced 丢弃；synced 后只 `feed` 原始字节 |
| 同上 `refresh_ui` ~1172 / ~1304 | 再 dump replica | 已有 Surface 只 show/hide，不 reset、不 dump |
| `src/platform/linux/pane_view.rs` `flush_pending_feed` | CUP → `reset` + `replica_ansi_provider` | 只 `vte.feed` 原始缓冲；**禁止** reset 追帧 |
| 同上 `scroll_history` | dump `scroll_ansi` | 删显示用途；滚轮走 VTE |
| `src/platform/linux/renderer.rs` `apply_mirror_policy` | `scrollback_lines=0` | 恢复用户 scrollback（或 capture 历史进 VTE） |
| `src/core/runtime/tmux/command.rs` `send_keys_bytes` | `send-keys -l` | GTK 字节 → `-H` |
| `src/core/runtime/tmux/protocol.rs` `PaneId::parse` | 只认 `@` | `%N` / `@N` / `N`（`%pane-mode-changed %64`） |
| `src/platform/linux/quickconnect_panel.rs` peek | `present_from_replica` | 与 Surface 同一 feed；F2 顺手改，不扩 UI |

`ReplicaStore` / `visible_ansi` **留下做 Index**（搜索、attention 状态）。C8 / E1 的 `emulate.rs` 单测可留。

---

## 2. Dogfood / fixture（动手前全读）

| 文件 | 必须 | 用途 |
|---|---|---|
| `tests/samples/dogfood-2026-0815-1326.txt` | 读 | C：session id。不要再改 |
| `tests/samples/dogfood-2026-0815-1540.txt` | 读 | D：backend 有数据 |
| `tests/samples/dogfood-2026-0815-1854.txt` | 读 | E 仍无画面 |
| `tests/samples/dogfood-2026-0815-2105.txt` | **读** | 闪烁/白屏/越写越长；本轮验收对照 |
| `tests/samples/codex-tui-sanitized.txt` | **读 + 测** | **raw `vte.feed`**，禁止再经 `visible_ansi` |
| `test_2026-0815-2105.log` | **只许 rg** | 禁止 `include_str!` |

参考树（只读）：ivyTerm `feed_output` / `get_initial_output` / `send_keypress`；iTerm2 `tmuxReadTask:`；Ghostty `viewer.zig` TODO ignore output；cmux `RemoteTmuxPaneSeed.swift`；tmux `control.c` `%pause`。

---

## 3. 测试怎么写才算测到病

**禁止**（这些绿过、产品仍坏）：

- 只 `contains(TOKEN)` 不数出现次数
- `present_from_replica` 当作「渲染完成」
- `cup_storm` 断言 `resets == 1`（那是在测白屏）
- ASCII 底行 `PROMPT` 代替 Codex 头+底
- `include_str!` 34MB log

**要**：

| 函数名 | 层 | 断言 |
|---|---|---|
| `surface_typing_overwrites_in_place` | widget `linux_render_e2e` | 连续 feed `\r` + 更长前缀；`visible_text` 里完整句 **恰好 1 次**，不能 `hello` / `hello w` / `hello wo` 三份 |
| `surface_live_feed_does_not_reset` | widget | seed 后 20 个 `\x1b[H\x1b[2Jframe-N`；`resets` 不增加；可见 `frame-19` 无 `frame-0` |
| `surface_codex_fixture_raw_feed` | widget | `codex-tui-sanitized` **直接** `feed`（不经 replica dump）；`TOKEN_HEADER` 与 `TOKEN_FOOTER`/`TOKEN_PROMPT` 同时在；含 `─` |
| `surface_seed_drops_output_until_capture` | widget 或 core | capture 前的 live 不进 VTE；快照 feed 后 catch-up 进 |
| `isolated_tmux_typing_token_appears_once` | `linux_live_e2e` | 隔离 tmux 逐字 `send-keys` `MUXTERM_TYPE_TOKEN`；5s 内 VTE 恰好一份，靠近底 |
| `isolated_tmux_switch_tab_resets_bounded` | `linux_live_e2e` | 第二个 window，点 status tab；VTE 非空；该次切换 `resets` 增量 **≤ 1**（最好 0） |

旧 `linux_render_e2e` 里依赖 dump 的场景 **在 F2 改写**，不要留着 `resets==1` / `replica_ansi_provider` 当门禁：

- `cup_storm_feeds_only_last_frame` → 并入 `surface_live_feed_does_not_reset`
- `cup_half_frames_keep_header_and_prompt` → 两段半帧 **都** `feed`，无 replica provider，无额外 reset；HEADER 和 PROMPT 都在（因为是前后半，不是二选一）
- `codex_tui_fixture_keeps_header_and_prompt` → raw feed
- `first_paint_uses_replica_tail_not_full_replay` / `scroll_up_reveals_replica_history` → F5 改成 VTE scrollback；F2 可先 `#[ignore]` 并在注释写 `SURFACE-PLAN F5`，禁止为了保绿继续 dump

Index 单测（`emulate.rs` `visible_ansi_*`）保留，**不能**当 Surface 完成。

---

## 4. 阶段 F1–F6

### F1 — RED：越写越长 + 切 pane 白屏

**Commit：** `test(linux): add Surface contracts for in-place typing and no-reset CUP`

只加测试，先看到红。

- `tests/linux_render_e2e.rs`：加上表 widget 函数（至少 typing + no-reset；Codex raw feed 可一起加，会红）。
- `tests/linux_live_e2e.rs`：加上表两个隔离函数。同文件已有唯一 `AppWindow`，接进现有 `main`/顺序调用，不要第二个 AppWindow。
- 打字 e2e：对隔离 session 的 pane 逐字符发送（测试 helper 可暂时仍 `-l`，F4 再换生产路径）；断言次数用 `matches().count()` 或拆行后唯一。
- 切 tab：复用已有 `click_status_tab_switches_real_window` 的点法；在 click **前** `clear_render_trace`，click 后看增量。

RED 证据写进汇报。不要改生产代码。

### F2 — GREEN：live 路径只有 raw `vte.feed`

**Commit：** `fix(linux): feed tmux pane bytes into VTE without replica dump`

对照 ivyTerm `feed_output`：synced 则 `vte.feed(&output)`，否则 return。

改：

1. `flush_pending_feed`：删 `replica_ansi_provider` 分支；CUP 风暴 **不要** `terminal.reset`。缓冲里的字节原样（或按序）`feed`。半帧必须都留下。
2. `STATE_PANE_OUTPUT`：删 `present_from_replica` 首屏（首屏改到 F3 capture；本 commit 未 synced 前可以先 raw feed 让 F1 widget 绿，但 **禁止** dump）。Index 仍 `feed_replica_and_engine`。
3. `refresh_ui`：删对已有 pane 的 `present_from_replica`。切 tab ≠ 重播网格。
4. **`LayoutHost::apply_layout`：禁止 `panes.retain(当前布局)` 丢掉其它 tmux window 的 PaneView。** 换 tab 只 unparent/再挂上；VTE 常驻，后台继续 `feed`。没有 Surface 的新 pane 才 `ensure_pane`。切连接仍走现有 `reset()`。这是快切的前提，缺了 F1 的 `resets` 断言会因为「旧 VTE 被扔、新 VTE 再 seed」而永远红。
5. attention / peek 小 VTE：同样 raw feed，禁止 dump。
6. 按 §3 改写 dump 门禁测试。

`present_from_replica` / `reset_and_feed_full` 若无调用者可删；若测试还在用，测试改为 `feed`。

可保留：`RenderPolicy` 只丢弃 **完整** 中间 CUP 帧时，提交的仍是 **原始** last frame，且 **不 reset**。禁止 last_visible_frame 切半帧。

本 F 结束：F1 widget 绿；隔离打字/切 tab 应绿或只剩 seed 时序（交给 F3）。`cargo clippy -D warnings` 绿。

### F3 — 一次 capture + synced 门

**Commit：** `fix(linux): drop live %output until capture-pane seed completes`

对照：ivyTerm `initial_output` / `is_synced`；Ghostty viewer TODO；cmux `RemoteTmuxPaneSeed`（`discardedOutput` + `snapshot` + `catchUpOutput`）。

- 每个 pane：`Unsynced { discarded }` → capture 命令进行中 → feed 快照 → replay catch-up → `Synced`。
- capture：一次，带 `-e`（颜色）。历史用 `-S - -E -`（ivyTerm）或可见区 + VTE scrollback（F5 对齐）。禁止每帧 capture。
- 2105 已有 19 次 `capture-pane`：本 F 后 attach/新 pane 各一次，不要随 CUP 风暴增加。
- 测试：`surface_seed_drops_output_until_capture`。快照前的 token 不得出现在 VTE，除非也在快照里。
- ivyTerm `scroll_view`（若干 `\n` + `ESC[#A`）在 capture 后钉尾：若「只见中段」还在，本 F 或 F5 加上，并写测试。

### F4 — GTK 按键 `send-keys -H`

**Commit：** `fix(core): send GTK pane bytes with send-keys -H`

官方：`tmux.1` `-H` = 每个 key 一个十六进制 ASCII 字节。源码 `cmd-send-keys.c`。psmux `tests-rs/test_send_keys_literal_byte.rs`。ivyTerm `send_keypress`。

- `send_keys_bytes`：`send-keys -t %N -H aa bb …`（每字节两位 hex）。单测：`[0x03, 0x1b, 0xff]` roundtrip。
- 剪贴板大段 UTF-8 可继续 `-l`（ivyTerm `send_quoted_text`）。
- 隔离 e2e：`-H` 打 token，VTE/capture 见到。
- 不要宣称本 F 单独修好「越写越长」。

### F5 — 跟尾 + VTE 滚动

**Commit：** `fix(linux): follow tmux tail with VTE scrollback instead of replica dumps`

- 删 `scroll_history` → `present_from_replica` 的显示路径；`set_scroll_provider` 可留空或删。
- `apply_mirror_policy`：**不要**再强制 `scrollback_lines=0`。用用户 prefs（`preferences_window` 已有）。alt-screen TUI 由字节自己切；VTE 在 alt-screen 不滚历史是正常的。
- 直播 `history_offset` 概念删除或恒 0。
- 测试：隔离或 widget 打 200 行 `line-N`；VTE 可见 `line-199`；模拟滚轮后可见 `line-0`；滚回底恢复 `line-199`。断言 **不得** 调用 `present_from_replica`。
- Codex fixture raw feed：头+底同时在（跟尾，不是 dump 中段）。

### F6 — `%` pane id + `%pause` 解析（实现 pause 可选）

**Commit：** `fix(protocol): parse tmux pane ids with % prefix and %pause`

- `PaneId::parse` 与 `parse_pane_id_lenient` 对齐：`@N` / `%N` / `N`。`parse_pane_mode_changed` 不再 WARN `%64`。
- 单测：`%pane-mode-changed %64 copy-mode`。
- parse `%pause %N` / `%continue %N`（`tmux.1` CONTROL MODE；`control.c` `%%pause %%%u`）。
- **实现** pause 回压：若 F2–F5 之后隔离 CUP 仍打爆，再在本 F 或立刻 F6b 按 iTerm2 `pausePanes` 做。未做 pause 不算 F1–F5 失败，但 2105 量级（13 分钟 30 万条）最终要有。

---

## 5. 明确不做（本轮）

- 重做 QuickConnect / session-id / 状态点颜色
- 搜索 UX、attention 时长下拉抛光（E 已有代码；坏的是小终端 dump）
- macOS / Swift
- 把 Alacritty/Herdr/Remux 当 tmux 客户端抄
- 提交 `/home/wlz/Developer/terminal`
- push

---

## 6. 完成定义

- [ ] F1 先红后绿（或 F1 红、F2 绿，汇报里写清楚）
- [ ] VTE 打字：完整 token **恰好一次**
- [ ] CUP 风暴：seed 后 `resets` 不涨；无白屏
- [ ] 切 tab：`resets` 增量 ≤ 1
- [ ] Codex fixture **raw feed** 头+底+盒线
- [ ] capture 一次 + 门；live 不在 seed 前画
- [ ] GTK 按键 `-H` 有单测
- [ ] 滚轮走 VTE，不 dump replica
- [ ] `%64` 不再 WARN
- [ ] `fmt` / `check` / `test` / `clippy -D warnings` / 两条 GTK e2e
- [ ] 英文 commit，无 Co-authored-by，未 push

人工 dogfood（用户）：SSH attach 真 Codex——不闪、切 pane 不白、输入原地更新、滚轮合理、跳到 agent 能看见尾部（头+输入条）。

---

## 7. 给 Codex 的一段话（可整段贴）

读 `docs/SURFACE.md` 和 `docs/SURFACE-PLAN.md`。Linux 渲染不是缺 UTF-8 dump，是 **三重仿真**：`ReplicaStore.visible_ansi` + `vte.reset`。对照 `/home/wlz/Developer/terminal/ivyterm`（gtk4+vte4，`vte.feed`、synced 门、`send-keys -H`）、`iterm2/sources/tmux`、`ghostty/src/terminal/tmux/viewer.zig`、`cmux/Sources/RemoteTmuxPaneSeed.swift`、`tmux/control.c`。

只做 F1→F6，一 F 一 commit。不要重做 C7/C8/E，不要 push，不要杀用户默认 tmux。搜索/通知抛光冻结。测试必须能抓住 2105：句子只出现一次、切 pane 不 reset 刷屏。Codex TUI 用 `codex-tui-sanitized.txt` **直接 feed**，禁止再 `present_from_replica`。
