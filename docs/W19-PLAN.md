# W19-PLAN.md — 终端模拟器越界 panic + GTK 不可崩溃

> 日期：2026-08-17（`2026-08-17T19:09:11+08:00`）
> 工作目录：`/home/wlz/Developer/self/muxterm`
> 分支：`feature/runtime/support_herdr`
> 日志：`test_2026-0817-1902.log`、`test_2026-0817-1903.log`（用户 19:02–19:04 CST / 11:02–11:04 UTC）
> 先读：本文件 → [`TESTING.md`](TESTING.md) §5.11 → `src/core/protocol/terminal/emulate.rs` → `src/platform/linux/window.rs` 16ms poll → `src/platform/linux/pane_view.rs`
>
> **你是实现 agent。先红测试，再最小实现到绿。禁止改断言来「绿」。禁止 `#[ignore]`。禁止 `git add -A`。禁止 Co-authored-by。禁止 push。禁止 `tmux kill-server` 不带 `-L`。禁止 `herdr server stop`。`fbc77e4` 必须仍是祖先。live 路径禁止 `visible_ansi` → `vte.reset`。**

用户从 Mini 跑 `~/Downloads/muxterm gui --debug --log-file …` 两次，进程直接没了。要求：**可以报错、必须留日志，不许 panic 把 GUI abort。** 先修根因，再加兜底。W20 的面板在这轮绿了之后再做。

---

## 0. 现场（不要猜）

stderr：

```text
thread 'main' panicked at src/core/protocol/terminal/emulate.rs:718:40:
insertion index (is 58) should be <= len (is 50)

thread 'main' panicked at src/core/protocol/terminal/emulate.rs:718:40:
insertion index (is 37) should be <= len (is 23)
```

第二次 panic 是 `panic in a function that cannot unwind`：第一次 panic 穿过 `glib::source::trampoline_local`（`AppWindow::new` 的 16ms poll，`window.rs` 约 L682）。glib 回调是 `extern "C"`，Rust panic 不能跨过去，于是 abort。

日志（`strings test_2026-0817-1902.log` / `1903.log`）：

- `seed_raw pane=0 bytes=37047 cols=284 rows=35`
- `seed_raw pane=1 bytes=288675 cols=284 rows=28`
- 切 tab 后再 `seed_raw pane=3 bytes=292317 cols=284 rows=43`

1902 停在 288KB 那次 seed（stderr 的 abort 不会进 tracing 文件）。第一次命令行显示 `Killed`，第二次 `Aborted (core dumped)`：都是同一次 `Vec::insert` 越界；SIGKILL 多半是 abort/core 在 Mini 上的外壳显示，不要另开一条 OOM 故事。

核对时间：2026-08-17T19:09:11+08:00。

---

## 1. 根因（必须修，不能只 catch）

`TerminalState` 里 `grid` 和 `grid_soft_wrapped` 必须等长。`rows()` 只看 `grid.len()`。

`resize()`（约 L373）只改 `grid`：长高时 `grid.resize(rows, …)`，**从不**动 `grid_soft_wrapped`。

`pane_view::ensure_grid_size`：第一次 seed 时 `seeded == false`，走 `reply_state.resize(cols, rows)`，不是 `TerminalState::new`。默认模型是 80×24。attach 后 tmux 报 284×35 → `grid` 变成 35 行，`grid_soft_wrapped` 还是 24。

然后 `feed` 进 htop / Codex TUI 的 DECSTBM + LF，走到 `linefeed_inner`（L712–718）：

```rust
} else if top < self.rows() && bottom < self.rows() {
    self.grid.remove(top);
    self.grid_soft_wrapped.remove(top);
    self.grid.insert(bottom, vec![Cell::blank(); self.cols()]);
    self.grid_soft_wrapped.insert(bottom, false); // ← L718 炸
}
```

条件用的是 `grid.len()`（已经变高），insert 打在仍是旧高度的 `grid_soft_wrapped` 上。

与日志对齐：

| stderr | 怎么来的 |
|---|---|
| insert 58，len 50 | 先 `new`/recreate 到 50 行（两 vec 齐），再 `resize` 到 59：grid=59、soft 仍=50；DECSTBM 底=58；`remove` 后 soft=50；`insert(58)` |
| insert 37，len 23 | 从默认 24 行 `resize` 到 ~38：soft 仍=24；`remove` 后 23；`insert(37)` |

同文件里 **只改 grid、不改 soft** 的还有：`scroll_up_n` / `scroll_down_n` / `insert_blank_lines` / `delete_lines`。只修 `resize` 不够，IL/DL 之后下一次 LF 仍会炸。

---

## 2. 顺序（不要跳）

1. **W19a** 单测红：resize 变高 + DECSTBM + LF 不得 panic，且 `grid.len() == grid_soft_wrapped.len()`。
2. **W19b** 实现：所有改行数的路径走同一对 `insert_row` / `remove_row` / `resize_rows`；`resize` 同步 soft 向量。
3. **W19c** `feed()` / 行操作越界时 **clamp + `tracing::error`**，禁止 `Vec::insert` panic。
4. **W19d** `src/core/fault.rs`：panic hook 把 payload + backtrace 打进 tracing（进 `--log-file`）。
5. **W19e** GTK：`catch_unwind` 包住 16ms poll 和会 `feed` 的 glib 回调；弹 `muxterm-fault-dialog`；**进程继续**。
6. 回归：已有 emulate 单测 + W13/W18 不要破。一逻辑一英文 commit。

W21（滚轮）在 W19 绿了之后、W20 之前。一逻辑一英文 commit。

---

## 3. W19a 测试（先提交红的）

放在 `src/core/protocol/terminal/emulate.rs` 的 `#[cfg(test)]`，名字必须是这些。

### 3.1 `resize_keeps_soft_wrapped_len_in_lockstep`

```rust
let mut t = TerminalState::new(10, 24);
t.resize(10, 50);
assert_eq!(t.rows(), 50);
assert_eq!(t.rows(), t.grid_soft_wrapped_len_for_test());
t.resize(10, 23);
assert_eq!(t.rows(), 23);
assert_eq!(t.rows(), t.grid_soft_wrapped_len_for_test());
```

加 `pub(crate) fn grid_soft_wrapped_len_for_test(&self) -> usize`（或 `#[cfg(test)]`）。不要为了测把字段改成 pub。

### 3.2 `resize_then_decstbm_lf_does_not_panic`

复现 1903：24 → 38 行，再设部分滚动区，底行 LF。

```rust
let mut t = TerminalState::new(80, 24);
t.resize(284, 38);
// CSI 2;38 r ：表头固定，正文滚动（htop）
t.feed(b"\x1b[2;38r");
t.feed(b"\x1b[38;1H\n\n\n");
assert_eq!(t.rows(), t.grid_soft_wrapped_len_for_test());
```

`#[should_panic]` **禁止**。必须 feed 完还活着。

### 3.3 `resize_50_to_59_partial_region_lf_does_not_panic`

复现 1902 的 58 vs 50：

```rust
let mut t = TerminalState::new(80, 50);
t.resize(284, 59);
t.feed(b"\x1b[2;58r");
t.feed(b"\x1b[58;1H\n");
assert_eq!(t.rows(), t.grid_soft_wrapped_len_for_test());
```

### 3.4 `insert_delete_lines_keep_soft_wrapped_lockstep`

`CSI 3 L` / `CSI 3 M` 之后两 vec 仍等长，再 LF 不得 panic。

### 3.5 `fault_report_captures_message_without_aborting`

`src/core/fault.rs` 单测：`catch_unwind` 里 `panic!("W19_FAULT_TOKEN")`，`report` 之后 `last_message()` 含 token；进程不结束。

---

## 4. 实现约束

### 4.1 emulate（W19b/c）

抽三个私有方法，**所有**行编辑都走它们（`linefeed_inner`、`scroll_*`、`insert_blank_lines`、`delete_lines`、`resize`）：

- `insert_row(idx, cells)`：`idx > len` 则 clamp 到 `len`（等于 append），并 `tracing::error!(target: "muxterm::emulate", idx, len, "insert_row clamped")`
- `remove_row(idx)`：`idx >= len` 则 error 日志并 return
- `resize_rows(new_rows)`：soft 向量与 grid **同一套** grow / truncate / drain

`resize()` 里光标在底行时 `drain(..start)` 必须同时 drain `grid_soft_wrapped`。

不要 `unwrap` / `expect` / `panic!` 在 emulate 热路径。越界 = 日志 + skip / clamp。

### 4.2 fault（W19d）

`src/core/fault.rs`（core，TUI/GTK/CLI 共用）：

- `install_hook()`：在 `init_logging` 成功之后调一次。hook 里 `tracing::error!(target: "muxterm::fault", …)` 打 payload 和 `Backtrace`（`std::backtrace::Backtrace::force_capture()`）。
- `report(where: &str, payload: Box<dyn Any + Send>)`：catch_unwind 的 Err 走这里。
- 记录最近一条（mutex），测试能读。
- hook **不能再 panic**。

### 4.3 GTK（W19e）

glib trampoline 不能 unwind。在 **进入 C 之前** 接住：

1. `window.rs` 16ms `timeout_add_local` 整个闭包包 `catch_unwind(AssertUnwindSafe(|| { … }))`。`Err` → `fault::report("linux.poll", …)` + 弹窗 + `ControlFlow::Continue`（不要 Break，否则轮询停了等于假死）。
2. `pane_view.rs` 的 `feed_reply_state`：`state.feed` 包 catch_unwind；失败则重建 `TerminalState::new(当前 cols/rows)`，不要留半坏 grid。
3. 其它 `timeout_add_local` / `idle_add_local` / 会碰到 emulate 的 `connect_*`：抽 `src/platform/linux/fault_gtk.rs` 的 `run<T>(label, f) -> Option<T>`，不要每个闭包手写一遍。

弹窗：

- `widget_name = "muxterm-fault-dialog"`
- 标题用 i18n 新 key `internal_error`（中文「内部错误」，英文 `Internal error`）
- 正文含「详情已写入日志」+ 第一行 panic 信息。不要把整段 backtrace 堆在对话框里。
- 同时最多一个。OK 按钮 `muxterm-fault-dialog-ok`。
- 测试：`linux_fault_e2e` 调 `AppWindow` 的测试钩子 `test_inject_fault("W19_FAULT_TOKEN")`，断言对话框存在且 tracing/last_message 含 token。不要真的把 emulate 炸穿 glib。

### 4.4 不要做的「全仓库消 unwrap」

不要把整个 crate 的 `unwrap` 改成 `unwrap_or_default`。热路径（emulate + GTK trampoline + `feed_reply_state`）不许 panic。其余 `unwrap` 标在 commit body 里「未动」，以后再清。

已知：`target_config_window.rs` 的 `ListingDebounce` 已经避开 glib `SourceId::remove` unwrap abort，不要改回去。

---

## 5. 门禁

```bash
cargo test --lib --features gtk resize_then_decstbm_lf_does_not_panic -- --test-threads=1
cargo test --lib --features gtk resize_50_to_59_partial_region_lf_does_not_panic -- --test-threads=1
cargo test --lib --features gtk resize_keeps_soft_wrapped_len_in_lockstep -- --test-threads=1
cargo test --lib --features gtk insert_delete_lines_keep_soft_wrapped_lockstep -- --test-threads=1
cargo test --lib fault_report_captures_message_without_aborting -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_fault_e2e -- --test-threads=1
# 回归（不要破）
cargo test --lib protocol::terminal -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_feature_e2e -- --test-threads=1
```

`linux_fault_e2e` 与其它 GTK crate 一样：一个 AppWindow，`gtk4::test_synced`，`--test-threads=1`。

---

## 6. 明确不做

- 不改 W20 面板、不改 Herdr Runtime 行为
- 不把 `feed` 整段吞掉而不修 lockstep
- 不 `herdr server stop`、不杀默认 tmux
- 不 revert `fbc77e4`
- 不把 288KB seed 改成截断来「避免」这次 panic（那是 attach 该播的快照）
