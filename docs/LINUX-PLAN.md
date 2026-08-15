# LINUX-PLAN.md — Phase E（档案，已冻结）

> **2026-08-15 21:26 冻结。** 本文 E1–E6 已在 git 落地（到 `d802f05`），但 E-R1
> 「VTE 只显示 replica `visible_ansi`」是错处方，2105 真机仍闪烁/白屏/越写越长。
> **当前执行计划：[`WORKSPACE-PLAN.md`](WORKSPACE-PLAN.md)。** F 已冻结：[`SURFACE-PLAN.md`](SURFACE-PLAN.md)。
> 架构：[`WORKSPACE.md`](WORKSPACE.md) / [`SURFACE.md`](SURFACE.md)。
> 不要按本文再做一遍 E，不要把 dump-into-VTE 当正确直播。
>
> 下文保留作历史。分支当时 `feat/linux-quickconnect-ui`。

---

> 分支：`feat/linux-quickconnect-ui`（HEAD `983367d`，ahead 79，**未 push**）
> **已冻结，不是当前执行计划。** 原合同是本文 §7 的 E1–E6。
> 修订：2026-08-15 19:05 CST
> 核查时间：`2026-08-15T19:01:53+08:00`

**决策（用户 2026-08-15 19:01 确认）**：

1. Codex 连上必须看得见、几何不能乱。渲染测试必须吃 **TUI fixture**（盒线 + 真彩 + CUP），不能只用 ASCII `PROMPT`。
2. 状态点已经可用。SSH 还要在 popover 里显示 **上下行流量**。
3. 搜索必须能搜（ReplicaStore 已经有 `search`，UI 是空的）。
4. 前台自己跑的 `ls` **不要**进 attention。看见了就不该提醒。
5. Attention 预览改成 **小 VTE 终端**（不要「文本 peek + 输入框」）。双击跳转。旁边按钮：跳转 / 放大该终端 / 禁止提醒（下拉时长）。
6. 本轮仍不做 macOS 改动。不 push。动手前必读全部 dogfood + `codex-tui-sanitized.txt`。

---

## 0. 总体进度：代码完成了任务么？距离目标多远？

| 阶段 | 目标 | 代码 | 真机 |
|---|---|---|---|
| A 骨架 | GTK 窗口、QuickConnect、隔离 tmux | 有 | 能连 |
| B | ReplicaStore、注意力状态机、配置页 | 有 | 注意力误报；搜索空 |
| C | 一条 bar、session id、RenderPolicy | 有 | 切 tab 好了 |
| D（C8.1–C8.5） | 几何播种、滚动、状态点 | **git 有 5 个 commit** | 状态点好；**Codex 仍空白/乱** |
| **E（本轮）** | 真 TUI 渲染测试、SSH 流量、搜索、attention 小终端 | **未开始** | 1854 仍看不见 Codex |
| 产品目标 | iTerm2 式 Linux tmux GUI + agent 感知 | Phase 1 未完 | 见下表 |

**C8 从代码上看交了卷，从产品上看没交。** 原因：测试全是 ASCII 底行 `PROMPT` / `line-199`，测不到 Codex。

1854 日志（`test_2026-0815-1854.log`）与 1540 同一类：backend **有** `capture-pane` 和 725 条 `%output`（pane 15/25/53，len 1365/2730），**没有**画面字节。所以「怎么测 Codex」= 用合成 TUI fixture，不是再 grep 这份 log。

对照 `visible_ansi` 仍在的两刀：

```rust
out.push(if cell.ch == '\0' { b' ' } else { cell.ch as u8 }); // U+2500 → 0x00
// sgr_fg / sgr_bg 只处理 Named 0-15，Color::Spec 真彩返回 None
```

Codex TUI 几乎全是盒线 + `48;2;…` 输入条。ASCII 测试绿、真机空，完全对得上。

直播路径在 `seeded` 之后仍 `feed_output` 原始半帧，再 `last_visible_frame`。几何首屏随后会被 CUP 半帧打烂。

**距离目标（粗算）**：Linux 日常能用大约还差 **两轮**——E 把 Codex 画对 + 搜索/attention 可用；之后才是搜索完善、主题、M5 体验打磨。Phase 2 agent 感知仍在路线图外。

---

## 1. 执行合同

1. GUI：`xvfb-run -a` + `gtk4::test_synced`。同进程一个 `AppWindow`。
2. 隔离 tmux `-L muxterm-test-*`。`/bin/cat`。不复制 `IsolatedTmux`。
3. 一次一个 E，先 RED。commit `type(scope): English`，无 Co-authored-by，不 push。
4. **禁止重做** C7 session-id、C8 ASCII 几何测试（保留，另加 TUI fixture）。
5. **禁止**镜像 VTE `scrollback_lines > 0`、`get_pane_output` 播种、`include_str!` 原 `.log`。
6. `visible_ansi` 必须写 **UTF-8**（`encode_utf8`），禁止 `ch as u8`。
7. 真彩必须编成 `38;2;r;g;b` / `48;2;r;g;b`（Indexed 256 用 `38;5;n`）。

---

## 2. Dogfood / fixture（动手前全读）

| 文件 | 必须 | 用途 |
|---|---|---|
| `tests/samples/dogfood-2026-0815-1326.txt` | 读 | C 档案：session `$4`。不要再改 session-id |
| `tests/samples/dogfood-2026-0815-1540.txt` | 读 | D：backend 有数据 |
| `tests/samples/dogfood-2026-0815-1854.txt` | **读** | E：仍无画面；log 无 payload |
| `tests/samples/codex-tui-sanitized.txt` | **读 + 测** | 合成 Codex TUI（全 example 文本） |
| `test_2026-0815-1854.log` | **必 rg** | 禁止 include_str |

`rg` 1854 至少：`list-windows -t $1`、`capture-pane`、`实时 %output 交付`、`SwitchTab`、`忽略其它 session`（应无）。

读 fixture：注释到 `PAYLOAD_UTF8_BELOW`，其后是 UTF-8 ANSI。测试跳过注释再 `feed`。

`real-codex.txt` 现在是一行 **shell 提示符** `%output`，**不是** Codex TUI。不要拿它当本轮 fixture。

---

## 3. 根因

### E-R1 Codex 看不见 / 乱（严重）

1. `visible_ansi`：`cell.ch as u8` 截断非 ASCII。`─`（U+2500）变成 `0x00`。
2. `sgr_fg`/`sgr_bg` 丢弃 `Color::Spec` / Indexed>15。Codex 输入条 `48;2;216;216;216` 播种后没了。
3. 首屏 geometric dump 之后，`feed_output` + `last_visible_frame` 吃 1365/2730 半帧，VTE 被切成半屏。
4. C8 测试没有盒线、没有真彩、没有「头+底同时在」。

**正确直播（镜像 TUI）**：replica 吃全部原始字节；VTE **只显示** replica 的几何 `visible_ansi`（UTF-8+真彩）。CUP 风暴 coalesce 结束后 `present_from_replica`，不要把半帧 `vte.feed`。安静 shell（无帧起点）仍可增量。

### E-R2 SSH 无上下行

macOS `TerminalManager.recordTraffic` 只有下行 2s 窗口。Linux `ConnectionSummary` 只有 `kind/host/status`。要在 popover 显示 `down=` `up=`（B/s 或 KiB/s）。从 SSH transport 读写字节计数，不要假造。

### E-R3 搜索不能搜

- `Action::Search` 在 `window.rs` 是空分支。
- `PanelTab::Search` 重建时空的；`search_rows()` 返回空 + 占位。
- `linux_search_e2e` 仍是 `placeholder_compiles`。
- Core 已有 `TerminalState::search` / scrollback search。本轮接到 Search tab：query → 各 pane replica 命中 → 点击跳转。

### E-R4 前台 `ls` 进 attention

状态机：`CommandDone` → `Done`。`BecameVisible` 只在 **切 pane** 时调用，前台跑完 `ls` 不会清。`filter_attention_rows` 列出 **Blocked 和 Done**。本地默认 shell 于是带着 `ls` 出现。

要改：

- 当前前台 pane 的 `CommandDone` → `Idle`，不进列表、不通知。
- 列表排除 `(active_workspace, active_pane)`。
- 前台输出时视为已看见（每次 `STATE_PANE_OUTPUT` 若 `pane == active_pane` 则 `on_became_visible` 或直接不标 Done）。

### E-R5 快速回复发不出；peek 不该是输入框

现在：`Label` peek + `Entry`，Enter → `send_input(line+\r)`。快捷键绑在 Entry 上，焦点不对就发不出。用户要：

- 一块小 VTE（镜像、`scrollback_lines=0`），`present_from_replica`。
- 键直接 `send_input` 到该 pane（真终端，不是一行输入框）。
- 双击小终端 → 与「跳转」同一函数。
- 按钮：`muxterm-attention-jump`、`muxterm-attention-zoom`、`muxterm-attention-mute`。
- 禁止提醒是 **下拉**，时长写死：

| id | 时长 |
|---|---|
| `5m` | 5 minutes |
| `10m` | 10 minutes |
| `30m` | 30 minutes |
| `1h` | 1 hour |
| `4h` | 4 hours |
| `24h` | 24 hours |

`on_mute(ws, pane, Duration)`。默认选 `1h`。不要再写死 3600 而无 UI。

放大：把该 pane 的小 VTE 放大成面板内大预览（或独立 popover VTE），仍 replica 播种；不要新开 tmux 窗口。跳转：已有 `jump_to_attention_pane`。

---

## 4. 测试策略（1854 为什么还是绿）

| 禁止 | 要用 |
|---|---|
| 只 ASCII `PROMPT` / `line-199` | `codex-tui-sanitized.txt`：头行 HEADER、底行 PROMPT、中间 BODY、盒线 `─` |
| `ch as u8` | roundtrip 后 `snapshot()` 仍含 `─` 和 TOKEN_* |
| 半帧 `vte.feed` 当直播 | CUP 风暴后 VTE 仍同时有 HEADER 和 PROMPT |
| `placeholder_compiles` 当搜索完成 | query `TOKEN_BODY` 命中 pane，点击跳转 |
| 前台 `CommandDone` 进列表 | 可见 pane 的 ls 不出现；后台 pane Done 才出现 |
| `popover.popup()` / 无流量字段 | SSH popover 含 `down=` 与 `up=` |
| 把 `real-codex.txt` 当 TUI | 那是 shell 提示符 |

---

## 5. widget_name

沿用 status/dot。新增：

| name | 行为 |
|---|---|
| `muxterm-search-entry` | Search tab 查询（可与面板顶栏共用，但 Search 激活时要搜 replica） |
| `muxterm-search-hit-{ws}-{pane}-{seq}` | 命中行，点击跳转 |
| `muxterm-attention-peek` | 小 VTE 容器 |
| `muxterm-attention-jump` | 跳转到该 pane |
| `muxterm-attention-zoom` | 放大该小终端 |
| `muxterm-attention-mute` | 下拉父控件 |
| `muxterm-attention-mute-5m` 等 | 菜单项，suffix 为表中 id |

Popover 文本增加：`down=` `up=`（SSH 才有；local 可省略或 `down=0B/s up=0B/s`）。

---

## 6. 改哪些文件

| 文件 | 改什么 |
|---|---|
| `emulate.rs` `visible_ansi` | UTF-8 + 真彩/256 SGR |
| `pane_view.rs` `window.rs` | CUP 风暴 → `present_from_replica`，禁止半帧 feed |
| SSH transport / `ConnectionSummary` | 上下行计数 |
| `status_bar.rs` | popover 流量 |
| `window.rs` `Action::Search` | 打开 Search tab |
| `panel_model.rs` `quickconnect_panel.rs` | 搜索结果；attention 小 VTE + 三按钮 + mute 下拉 |
| `attention/engine.rs` `state.rs` | 前台 CommandDone 不标 Done |
| `tests/linux_render_e2e.rs` | 喂 sanitized fixture |
| `tests/linux_search_e2e.rs` | 替换 placeholder |
| `tests/linux_chrome_e2e.rs` | SSH 流量字段 |
| `linux_panel_e2e` / attention e2e | 前台 ls 不出现；mute 时长；小终端 widget_name |

---

## 7. E 提交列表（一次一个，先 RED）

### E1 `fix(core): encode visible ANSI as UTF-8 with truecolor SGR`

- `visible_ansi`：`ch.encode_utf8`；`Color::Spec` → `38;2`/`48;2`；Indexed → `38;5`/`48;5`。
- RED `visible_ansi_preserves_box_drawing_and_truecolor`：feed fixture payload；`snapshot()[0]` 含 `TOKEN_HEADER`；某行含 `TOKEN_BODY`；第 22 或 24 行含 `TOKEN_PROMPT` 或 `TOKEN_FOOTER`；网格含 `─`；dump 字节含 `48;2;216;216;216`（或往返后 bg Spec 仍在）。
- 验证：`cargo test --lib emulate -- --test-threads=1`

### E2 `test(linux): seed VTE from sanitized Codex TUI fixture`

- `linux_render_e2e`：`codex_tui_fixture_keeps_header_and_prompt`
  - 80×24 镜像 PaneView，feed payload → replica → `present_from_replica`
  - `visible_text` 含 HEADER、BODY、PROMPT（或 FOOTER）；第一行不是 PROMPT
  - 含 `─` 或至少 HEADER 与 PROMPT 同时存在
- 验证：`xvfb-run -a cargo test --features gtk --test linux_render_e2e -- --test-threads=1`

### E3 `fix(linux): present replica frames instead of CUP half-frames`

- 镜像路径：coalesce 后若 `render_intent` 是 ReplaceVisible，VTE 走 `present_from_replica(visible_ansi)`，不要 `last_visible_frame` 半帧。
- RED：先播 fixture；再 `feed_output` 两段残缺 CUP（`ESC[H ESC[2J` + 只写了上半屏 HEADER，第二段只写底栏 PROMPT）。flush 后 VTE **同时** 有 HEADER 和 PROMPT（replica 合并了两段）。
- 验证：同上 `linux_render_e2e`

### E4 `feat(linux): show SSH up/down traffic on status popover`

- 计数 SSH 读/写字节；popover：`down=` `up=`。
- RED S7 扩展：`kind=ssh` 时 label 含 `down=` 与 `up=`（测试可注入计数）。
- 验证：`linux_chrome_e2e`

### E5 `feat(linux): search replica lines in Search tab`

- `Action::Search` / 面板 Search：对 replica `search`。
- RED：去掉 `placeholder_compiles`；注入含 `TOKEN_BODY` 的 replica；query 后出现 hit widget；激活 hit 走 jump 回调（Recording）。
- 验证：`xvfb-run -a cargo test --features gtk --test linux_search_e2e -- --test-threads=1`

### E6 `feat(linux): attention mini terminal with jump/zoom/mute durations`

- 前台 `CommandDone` → Idle；列表排除当前 pane。RED：`foreground_command_done_is_not_listed`。
- 小 VTE `muxterm-attention-peek`；双击 = jump。
- 按钮 jump / zoom / mute 下拉（5m/10m/30m/1h/4h/24h）。
- 删掉作为主路径的 reply `Entry`（键打在小 VTE 上）。
- RED：`linux_panel_e2e` 或 attention e2e：有 peek widget；`mute-10m` 回调 Duration=600s；前台 ls 不出现。
- 验证：对应 e2e `--test-threads=1`

---

## 8. Phase E 完成检查单

- [ ] 已读三份 dogfood + `codex-tui-sanitized.txt`，rg 了 1854
- [ ] E1–E6 各一英文 commit，未 push
- [ ] `rg 'ch as u8' src/core/protocol/terminal/emulate.rs` 在 `visible_ansi` 里为 0
- [ ] `rg placeholder_compiles tests/linux_search_e2e.rs` 为 0
- [ ] 镜像 `scrollback_lines` 仍为 0
- [ ] fmt / clippy -D warnings / emulate / linux_render_e2e / linux_chrome_e2e / linux_search_e2e / panel 或 attention e2e

人工：SSH 再 attach 同一 legion/Codex pane，应看见头栏+输入条；滚轮仍有历史；点状态点有 down/up；前台 ls 不弹；attention 小终端能打字；搜索能跳。

---

## 9. 反模式

1. 重做 C7/C8 ASCII 测试当本轮完成。
2. `ch as u8`、丢真彩。
3. 半帧 `vte.feed` 当 Codex 直播。
4. `include_str` `.log` 或把 `real-codex.txt` 当 TUI。
5. 搜索继续 `placeholder_compiles`。
6. 前台 Done 仍进列表。
7. 只加输入框「修」回复。
8. mute 只有 1h 硬编码、没有下拉。
9. 打开镜像 VTE scrollback。
10. 改 macOS / push / 第二个 AppWindow。

---

## 10. 给 Codex 的开场白（整段粘贴）

```
读 AGENTS.md、docs/TESTING.md、docs/LINUX-PLAN.md（Phase E，2026-08-15 19:05）。

动手前必读：
1. tests/samples/dogfood-2026-0815-1326.txt
2. tests/samples/dogfood-2026-0815-1540.txt
3. tests/samples/dogfood-2026-0815-1854.txt
4. tests/samples/codex-tui-sanitized.txt（PAYLOAD_UTF8_BELOW 之后是合成 Codex TUI）
5. rg test_2026-0815-1854.log：list-windows -t $1、capture-pane、实时 %output、SwitchTab、忽略其它 session。禁止 include_str 原日志。

不要重做 C7/C8。C8 的 ASCII PROMPT 测试保留。按 E1→E6 一次一个，先 RED。

根因：
1. visible_ansi 用 ch as u8，盒线变 NUL；真彩 SGR 丢掉。改成 UTF-8 + 38;2/48;2/38;5。
2. seeded 之后 feed_output 半帧 CUP 会打烂 VTE。CUP 风暴要 present_from_replica(完整网格)。
3. SSH popover 加 down= 与 up=。
4. Search tab 接 replica.search，删掉 placeholder_compiles。
5. 前台 pane 的 CommandDone 不要进 attention。peek 改小 VTE；按钮跳转/放大/禁止提醒下拉 5m 10m 30m 1h 4h 24h。键打在小 VTE 上 send_input。

做完一个 E 跑该 E 验证命令，汇报退出码。全部做完跑 §8。不 push。commit 英文 type(scope): subject，无 Co-authored-by。
```

---

## 11. Phase D 档案（不要执行）

HEAD `983367d`：`dcfe69a` `7005a45` `4afeefa` `9048ec8` `983367d`。状态点真机可用。几何 ASCII 测试保留。`visible_ansi` 的 trim/skip 已修，但 UTF-8/真彩/半帧未修。
