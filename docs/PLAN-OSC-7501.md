# OSC 7501 Program Status 实现计划

> 状态：已实现（2026-11-05）。实现提交见 git log；本文件保留为设计记录。
> 实际落地时把解析器放在 `src/core/protocol/terminal/program_status.rs`（计划允许），
> 并让重复合法帧前进 revision 以刷新 TTL。
> 预计工作量：0.5–1 天（含测试）。
> 规范核对时间：2026-10-13 至 2026-11-05。来源见 `PROTOCOL-BACKLOG.md` §2。

## 1. 目标

程序通过 `OSC 7501` 告诉终端自己在干什么，muxterm 接收并落到 Attention / Activity。

验收：一个只发 7501、不发 BEL / OSC 133 的程序，能让对应 pane 正确显示
`working` / `blocked` / `done` / `error` / `idle`，并带一行原因文本。

## 2. 非目标

- 不做铃声、Dock 角标、系统通知等新呈现。现有 Attention 红点复用即可。
- 不改 `PaneStatus` 枚举（见 D3）。
- 不接 OSC 26（Otty Terminal Agent Protocol），不接 OSC 9;4 进度条语义。
- 不做 Herdr 侧：`herdr 0.8.0` 不解析 7501（2026-10-13 实测），上游 PR 5172 仍开着。
  7501 字节会原样留在 pane 里，由我们的 VT 接收。以后 Herdr 若自己吃掉再改接事件。
- 不做 tmux 特例。7501 和 133 一样走 `%output` 原始字节，不需要 `%program-status`。

## 3. 规范要点（revision 0.4，2026-10-10）

```text
OSC 7501 ; key=value ; key=value … ST
```

`ST` = `ESC \`，也接受 `BEL`。键值大小写敏感，顺序无关。

| 键 | 规则 |
|---|---|
| `state` | 必填：`idle` / `working` / `blocked` / `done` / `error` |
| `title` | 谁在报告。省略 = 不变；空值 = 清除；≤64 字节，UTF-8 截断 |
| `status` | 一行原因。省略 = 不变；空值 = 清除；≤127 字节，UTF-8 截断 |
| `ttl` | 秒。超时按 `idle`。省略 = 终端决定。`0` = 不过期 |
| `sticky` | `1`/`0`。只有 `done` / `error` 允许为 `1` |
| `clear` | `1` = 清除 title 与 status（不清 ttl / sticky）。与 `state` 同时出现整条无效 |
| `id` | 同一屏多个状态的标识，不透明字符串。缺省 = 默认槽位 |

优先级（多个 `id` 并存时取最高）：`idle` < `working` < `done` = `error` < `blocked`。
`done` 与 `error` 互不覆盖。

其它规则：非法规格整条作废并保留上一份状态；`clear=1` 不清 `ttl` / `sticky`。

## 4. 现状（已核实，2026-11-05）

- `src/core/protocol/terminal/emulate.rs`
  - `feed()` 逐字节喂 vte 前，先调 `scan_attention_byte()` 扫 OSC 帧。
  - `process_attention_osc()` 按首参 `match` 分发：`133` / `9`（含 `9;4` 进度）/ `99`
    / `777;notify` / `1337` / `52`。**没有 `7501` 分支。**
  - vte 0.13 的 `Handler` 不暴露 `osc_dispatch`（见 `feed()` 里的注释），所以 OSC
    必须在自建扫描器里处理，不能指望 vte 回调。
  - `TerminalState::take_attention_signals()` 取走累积的 `AttentionSignal`。
- `src/core/activity/attention/signal.rs`：`AttentionSignal` 是 `Copy`，只有
  `CommandStart` / `CommandDone` / `AttentionRequest` / `AuthoritativeStatus` /
  `ClearAuthoritativeStatus`。全仓 95 处引用。
- `src/core/activity/attention/state.rs`：`PaneStatus` 五态；`PaneEvent` 八事件；
  `transition()` 有穷举测试。
- `src/core/workspace/pane_buf.rs`：`take_attention_signals()` 与
  `screen_snapshot()` 是 Index 对外两条通路。
- `src/core/activity/attention/engine.rs`：`apply()` 里 runtime 权威优先，
  已知 agent 走屏幕规则，其余走 133/BEL 启发式。
- `src/core/protocol/ffi/functions/attention.rs`：每 pane 输出
  `status` / `acknowledged` / `muted` / `last_line` / `process_name` / `agent_name` 等 JSON。
- 前端 `corebridge.rs` 的 `ClientAttentionStatus::parse()` 只认五个字符串，未知回 `Unknown`。

## 5. 设计决定

**D1：新开一条并行通道，不改 `AttentionSignal` 的 `Copy` 语义。**
7501 带字符串，塞进 `AttentionSignal` 会让它失去 `Copy`，波及 95 处引用。
改为：`TerminalState` 保存 7501 当前状态，`PaneBuf` 转发快照，`AttentionEngine`
新增 `apply_program_status()`。改动是加法，不动既有事件流。

**D2：用快照，不用增量事件。**
`TerminalState::program_status_snapshot()` 返回「当前全部条目 + 版本号」，
版本没变返回 `None`。快照天然幂等，重连、丢帧都不需要补发逻辑。

**D3：不新增 `PaneStatus` 变体。**
三个前端按字符串解析状态，加 `error` 变体会在 `ClientAttentionStatus::parse()`
静默落到 `Unknown`。7501 的状态原文放在新字段 `program_status.state` 里，
Attention 红点继续用五态。`error` 与 `done` 的区分由 `program_status`
和 `ActivityStatus::Failed` 承担，不靠 `PaneStatus`。

**D4：优先级 = 7501 > 屏幕规则 > 133/BEL；但不覆盖 runtime 权威。**
Herdr 结构化 agent 状态（`AuthoritativeStatus`）继续优先，因为它的生命周期更全
（agent 名、静音、ack），而且 Herdr 迟早会自己吃 7501（PR 5172）。7501 仍然保存文本，
只是不改状态。理由写进代码注释。

**D5：`ttl` 在 engine 用 `Clock` 判，不在 `TerminalState` 判。**
`TerminalState` 只存规范字段，不存时间。engine 记录到达时刻，超时按 `idle` 处理，
沿用 `decay_idle_agents()` 的调用点（`attention_snapshot` 前）。

## 6. 实现步骤

### Step 1 — 解析层（`emulate.rs`）

新增数据结构（放 `emulate.rs` 或独立 `program_status.rs`，二选一，不要两处）：

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProgramState { Idle, Working, Blocked, Done, Error }   // 顺序即优先级

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgramStatusEntry {
    pub id: String,              // 缺省 ""
    pub state: ProgramState,
    pub title: Option<String>,   // 当前拼接后的 title
    pub status: Option<String>,
    pub sticky: bool,
    pub ttl_secs: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgramStatusSnapshot {
    pub revision: u64,
    pub entries: Vec<ProgramStatusEntry>,   // 按 id 排序，稳定
}
```

- `process_attention_osc()` 加 `b"7501"` 分支，解析 `key=value`。
- 语义：`title` / `status` 省略 = 保留上次；空值 = 置空。`clear=1` 清 title/status；
  与 `state` 同时出现 → 整条丢弃。`sticky=1` 只在 `done`/`error` 生效，否则整条作废。
  `state` 非法、键重复冲突、超长 → 整条丢弃，保留上一份。
- 长度按 UTF-8 字符边界截断：title 64 字节、status 127 字节。
- 存储：`TerminalState` 加 `program_status: BTreeMap<String, ProgramStatusEntry>`
  + `program_status_revision: u64`（内容变化才自增）。
- 对外：`pub fn program_status_snapshot(&self) -> Option<ProgramStatusSnapshot>`。
- 注意：7501 不参与 `collect_command_byte()`，和其它 OSC 一样由 `terminated_osc` 短路。

测试（`emulate.rs` 的 `mod tests`，用真实字节）：

```text
b"\x1b]7501;state=working;title=claude;status=thinking\x1b\\"
b"\x1b]7501;state=blocked;status=approve?\x07"        // BEL 终止
b"\x1b]7501;clear=1\x1b\\"                            // 只清文本
b"\x1b]7501;state=bogus\x1b\\"                        // 整条丢弃
b"\x1b]7501;state=working;title=" + 100 字节          // 截断到 64
b"\x1b]7501;state=done;sticky=1\x1b\\"
b"\x1b]7501;state=working;sticky=1\x1b\\"             // sticky 非 done/error → 丢弃
```

### Step 2 — 状态机（`state.rs`）

- 新增 `PaneEvent::ProgramIdle`，转移表：除 `Blocked` 外一律 `Idle`；
  `Blocked + ProgramIdle → Idle`（程序说自己空闲，就是没在等人）。
- 新增 `PaneEvent::ProgramDone { failed: bool }`？**不要**。复用现有
  `CommandDone { exit_code: None }`，`error` 的区分交给 Step 4 的 Activity。
- 更新 `transition_table_exhaustive`：事件从 8 个变 9 个，穷举断言同步改。

### Step 3 — 传递（`pane_buf.rs`）

- `PaneBuf` 加 `program_status_snapshot()`，直接转发 `terminal`。
- `reset_visible()` 里清空 7501 状态（和 `pending_attention` 一致），
  因为重放字节会重新生成。
- 不改 `take_attention_signals()` 签名。

### Step 4 — 引擎（`engine.rs`）

- `PaneAttention` 加：
  ```rust
  pub program_status: Option<ProgramStatusSnapshot>,
  pub program_status_seen_at: Instant,
  pub program_status_revision_applied: u64,
  ```
- 新方法 `apply_program_status(&mut self, ws, pane, snapshot: Option<&ProgramStatusSnapshot>,
  now: Instant)`：
  1. `revision` 未变直接返回。
  2. 取最高优先级条目（`state` 的 `Ord`）作为显示状态；文本取同一条。
  3. 若无 runtime 权威（`authoritative == false`）：
     - `working` → `PaneEvent::OutputActivity`
     - `blocked` → `PaneEvent::AttentionRequest`（幂等）
     - `done` → `PaneEvent::CommandDone { exit_code: None }`
     - `error` → `PaneEvent::CommandDone { exit_code: None }`，并标记 failure（见下）
     - `idle` → `PaneEvent::ProgramIdle`
  4. `sticky=false` 的 `done`/`error` 不延长停留：沿用现有 idle 衰减；
     `sticky=true` 保持到 `acknowledge()` 或 `on_became_visible()`。
  5. `ttl_secs` 到期 → 视同 `idle`（`ProgramIdle`），在上面的衰减调用点判定。
- `decay_idle_agents()` 旁边加 `decay_program_status()`，在
  `muxterm_attention_snapshot` 已经调用的同一位置调用，避免新增 FFI 入口。
- 失败标记：`error` 时若该 pane 有进行中的 command/agent 记录，
  `ActivityState` 把它标成 `ActivityStatus::Failed`；没有记录就只改 Attention。
  用 `ActivityState` 现有的按 pane 查找路径，不要新造 store。

测试（用 `FakeClock`）：
- 单条 `working` → `Working`；随后 `blocked` → `Blocked`。
- `sticky=1` 的 `done` 在 10 分钟后仍 `Done`；`acknowledge` 后回 `Idle`。
- `ttl=1` 的 `blocked` 2 秒后回 `Idle`。
- `authoritative == true` 时 7501 不改状态，但文本仍被保存。
- 多 `id` 并存：`working` + `blocked` → 显示 `blocked`。

### Step 5 — 对外（`ffi/functions/attention.rs`）

每 pane JSON 增加一个字段（纯加法，前端可直接忽略）：

```json
"program_status": {
  "state": "blocked",
  "title": "claude",
  "status": "Approve edit?",
  "sticky": true
}
```

无状态时为 `null`。不要改 `status` 字段的取值集合。

### Step 6 — 文档

- `docs/PROTOCOL-BACKLOG.md` §2 顶部标注「已实现，见 `PLAN-OSC-7501.md`」。
- `docs/RUNTIME.md` 第 7 行移除 OSC 7501 的「待接」表述。
- `docs/ACTIVITY-DETECTION.md` 增加一条来源优先级：7501 → runtime 权威 → 屏幕规则 → 133/BEL。

## 7. 验收

```bash
cargo fmt
cargo check
cargo test
cargo clippy --all-targets -- -D warnings
```

手工验证（必须做，不能只跑单测）：

```bash
# 隔离 tmux socket，不碰用户 server
tmux -L muxterm-test-osc7501 new-session -d -s t
# 在 pane 里执行，然后观察 muxterm 的 Attention 侧栏
printf '\033]7501;state=working;title=demo;status=building\033\\'
printf '\033]7501;state=blocked;title=demo;status=approve?\033\\'
printf '\033]7501;state=done;sticky=1;title=demo\033\\'
tmux -L muxterm-test-osc7501 kill-server   # 必须带 -L
```

## 8. 边界与坑

1. **不要**把 7501 包在 `OSC 133;P` / `OSC 133;A` 之间，tmux 会把它收进 capture。
2. tmux passthrough 形态 `\033Ptmux;\033\033]7501;…\033\\\033\\` 要能透传
   （现有 `%output` 反转义已经处理 `tmux;` 包裹，加一条测试确认 7501 也行）。
3. `BEL` 和 `ST` 两种终止都要测。`ST` 里 `ESC` 后面不是 `\` 时不能把字节吞掉。
4. 帧可能被 `%output` 切成多条消息；扫描器是逐字节流式的，不要假设整帧到达。
5. `title` / `status` 截断必须按 UTF-8 边界，不能切坏中文。
6. 多个 `id` 时不要只保留最后一个；优先级合并是规范要求。
7. `error` 不能让红点永远亮；沿用衰减。
8. 不要在 `TerminalState` 里用 `Instant`：它参与指纹哈希，且需要可测。

## 9. 交付物

- 一个 commit：`feat(attention): accept OSC 7501 program status`
- 代码：`emulate.rs` / `state.rs` / `pane_buf.rs` / `engine.rs` /
  `activity/mod.rs` / `ffi/functions/attention.rs`
- 测试：新增 ≥10 条，覆盖第 3 节全部规则
- 文档：`PROTOCOL-BACKLOG.md`、`RUNTIME.md`、`ACTIVITY-DETECTION.md`
