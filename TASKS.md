# TASKS.md — macOS UI 功能 → Linux GTK 移植执行计划

> 基线时间：2026-08-13 19:5x CST（Asia/Shanghai）
> 分支：`feat/linux-quickconnect-ui`（ahead `origin/feature/quickconnect-attach-ui` 2 个 commit，未 push）
> 执行方式：Codex 单主 agent 串行做骨架（阶段 0 + A + B），完成后最多 3 个 subagent 并行做叶子（阶段 C）。
> 原则：每个功能点一个独立 commit；`cargo fmt/check/test/clippy` 全绿；不 push；tmux 测试只用隔离 `-L` socket。

---

## 1. 目标与边界

把 macOS 分支上已实现的 14 项 UI 功能完整移植到 Linux GTK4 前端，行为与 macOS 一致。

范围内：
- 只改 `src/platform/linux/`、`src/platform/i18n/` 与必要的 `src/core/` FFI/配置接线。
- 以 `src/platform/macos/` 的 Swift 实现和 git 历史为**只读参考**，不改 macOS。
- 每个功能点独立 commit，可单独 review。

范围外（红线）：
- 不重构未要求的模块；不做性能优化；不新增无关依赖。
- 不 push；默认 server 的 tmux 只允许只读命令。
- 不引入额外 worktree，除非用户明确要求并行跑多个会话。

---

## 2. 当前基线（已验证，2026-08-13）

### 2.1 分支与工作树

- 当前分支 `feat/linux-quickconnect-ui`，最近 commit：
  - `97d95b8 feat(linux): port quickconnect logic models and extend FFI bridge`
  - `1b2941a feat(core): add statusbar/pool config, new actions and quickconnect i18n keys`
- 未提交改动（当前正在进行的 UI 层移植，工作树**当前不编译**）：
  - `src/platform/linux/layout_host.rs`（+68）：全屏 pane 状态、运行期 theme/font 应用、`LayoutHost::new(theme, font, is_tmux_mirror)` 新签名
  - `src/platform/linux/pane_view.rs`（+201）：`resize_to`、`seed_snapshot`、25ms 输出合并 `FEED_COALESCE_MS`、镜像模式丢弃查询应答、运行期 theme/font
  - `src/platform/linux/renderer.rs`（+11）：`apply_font`
  - `src/platform/linux/status_bar.rs`（新文件 375 行，**未注册进 mod.rs**）：GTK4 tmux 兼容 status bar（left/窗口列表/right、justify、tmux/theme 模式、点击切 tab）
- 编译断点：`window.rs` 仍调用 `LayoutHost::new(theme)` 和 `view.sync_full_output()`，与未提交的新 API 不匹配。

### 2.2 已完成（纯逻辑层，均在 `src/platform/linux/quickconnect/`，全部带单元测试）

| 模块 | 对应功能 |
|---|---|
| `model.rs` | TargetConfig / Recent/Project badge / 搜索文本 / name 派生 |
| `store.rs` | Recent/Project 持久化（JSON） |
| `options.rs` | runtime/transport 单选卡片状态 |
| `directory.rs` | 目录补全：路径模型、防抖请求、generation 防竞态 |
| `project_flow.rs` | attach → 失败创建 detached → 再 attach 状态机（local/ssh 共用） |
| `pool.rs` | LRU 容量淘汰、TTL、后台 poll、Recent 派生（泛型 `ConnectionSlotProtocol`） |
| `status_style.rs` | tmux style 解析（256 色/#hex/属性）、justify、snapshot |
| `tab_gate.rs` / `event_policy.rs` | 后台 tab layout 事件策略 |
| `font.rs` | 字体缩放、`Preferences`（theme/statusbar_mode/font_size）TOML 持久化 |

### 2.3 已就绪的 core / FFI 能力

- `Task::TogglePaneFullscreen`：tmux `resize-pane -Z`，本地 shell 由前端布局实现
- `report_pane_colours` / `report_all_pane_colours`：`refresh-client -r` 重报颜色
- `status_snapshot()`：tmux status 快照
- discovery：SSH host、目录列表、tmux session 列表；`create_tmux_session`
- `BridgePane { id, cols, rows, is_active }`：已带 pane 字符格尺寸
- i18n：`src/platform/i18n/locales/{en,zh-CN}.json` 各 173 key，parity 一致，QuickConnect 相关 key 已存在
- 键位：`default_keybindings` 已含 QuickConnect/Quit/字体缩放/全屏

---

## 3. 14 项现状对照表

| # | 功能 | 纯逻辑 | GTK 接线 | 参考（macOS / commit） |
|---|---|---|---|---|
| 1 | QuickConnect 面板 | ✅ model/store | ❌ 无面板 | `QuickConnectController.swift` |
| 2 | TargetConfig 窗口 | ✅ options/directory | ❌ 无窗口 | `TargetConfigWindow.swift` |
| 3 | Project 连接流程 | ✅ project_flow | ❌ 未接线 | `ProjectConnectFlow.swift` |
| 4 | Warm Connection Pool | ✅ pool（泛型） | ❌ 无真实 Slot | `ConnectionPool.swift` / `WarmConnectionSlot.swift` |
| 5 | tmux Status Bar | ✅ status_style | ⚠️ widget 已写未注册 | `StatusBarModel.swift`；6104197/254a407/81e5ec9/55c1cac |
| 6 | 主题切换 + 重报色 | ✅ Preferences/theme | ❌ Action 空壳 | 85e0018/8bae09f |
| 7 | 状态栏模式切换 | ✅ status_bar.set_mode | ❌ 未接线 | 93b0c5c/6104197 |
| 8 | 字体缩放 + 持久化 | ✅ font.rs | ⚠️ renderer 已加，Action 空壳 | 76c41c9 |
| 9 | Pane 全屏 | ✅ core task + layout 状态 | ❌ Action 空壳 | a399f7a |
| 10 | Tab 门禁 + 事件策略 | ✅ tab_gate/event_policy | ❌ dispatch 未用 | 1911098/76db2d3 |
| 11 | 先 resize 再 feed + 输出合并 | ✅ pane_view（未提交） | ⚠️ window 还在调旧 API | 114637c/a9168f9 |
| 12 | 镜像模式丢弃查询应答 | ✅ pane_view（未提交） | ⚠️ window 仍无条件回写 | 3f15d93/44f0409/121285c |
| 13 | 自定义键位扩展 | ✅ keymap/config | ⚠️ 5 个 Action 是空壳 | 8af65dd/c6eb627/1d648d0 |
| 14 | i18n 补齐 | ✅ 173/173 | ⚠️ 新 UI 文案未接 | `Resources/i18n/*.json` |

结论：**真正的剩余工作量几乎全部在 GTK 接线层**，而且高度集中在 `window.rs` / `layout_host.rs` / `pane_view.rs` / `status_bar.rs` 这几个文件上——这正是必须串行的原因。

---

## 4. 执行阶段总览（依赖图）

```text
阶段 0  恢复可编译基线（1 commit）
   │
阶段 A  核心 UI 接线，串行，每项 1 commit
   ├─ A1  item 11  resize→feed + 输出合并
   ├─ A2  item 12  镜像模式丢弃应答
   ├─ A3  item 9   Pane 全屏
   ├─ A4  item 8   字体缩放 + 持久化
   ├─ A5  item 5   Status Bar 接线
   ├─ A6  item 7   状态栏模式切换
   ├─ A7  item 6   主题切换 + 重报色
   └─ A8  item 10  Tab 门禁 + 事件策略
   │
阶段 B  QuickConnect 链路，串行，每项 1 commit
   ├─ B1  item 1   QuickConnect 面板
   ├─ B2  item 2   TargetConfig 窗口
   ├─ B3  item 3   Project 连接流程接线
   └─ B4  item 4   真实 ConnectionSlot + Pool 接入
   │
阶段 C  收尾（可并行叶子）
   ├─ C1  item 13  键位动作补全（只动 keymap/config 测试）
   ├─ C2  item 14  i18n 审计与新 UI 文案
   └─ C3  集成测试（isolated tmux -L e2e）
```

串行原因：A1–A8、B1–B4 全部要改 `window.rs`（Action 分发、事件分发、refresh_ui、连接创建），任何两个任务并行都会在同一文件上冲突。并发只留给阶段 C。

---

## 5. 阶段 0：恢复可编译基线

**目标**：`cargo check` 恢复绿色，不改行为，不引入新功能。

改动：
- `window.rs`：`LayoutHost::new(theme, font, is_tmux_mirror)`，font 从 `cfg.font` 构造，`is_tmux_mirror = uses_tmux`。
- `window.rs`：`sync_full_output(&out)` → `seed_snapshot(&out, pane.cols, pane.rows)`（`BridgePane` 已带尺寸）。
- `mod.rs`：注册 `pub mod status_bar;`，让新 widget 进入编译与测试。

验证：
- `cargo fmt --check`、`cargo check`、`cargo test`、`cargo clippy -- -D warnings`
- 保留当前未提交 diff 的内容不变，只补接线。

Commit：
```text
fix(linux): migrate window to resize/coalesce pane view API
```

---

## 6. 阶段 A：核心 UI 接线（单 agent 串行）

### A1（item 11）先 resize 再 feed + 增量输出合并
- 内容：window 事件分发里 `STATE_PANE_OUTPUT` 走 `feed_output`（合并 25ms 一次 feed）；布局/首次挂载走 `seed_snapshot(out, cols, rows)`；确认 `refresh_ui` 不再调用旧 `sync_full_output`。
- 文件：`pane_view.rs`（已完成）、`window.rs`（迁移）、必要时 `ffi_bridge.rs` 的 BridgePane 尺寸来源。
- 验收：真实 htop/agent 重绘样例下输入行不漂移；`tests/logs` 已有样例可复放；pane_view 单测保留。
- Commit：`fix(linux): resize pane before feeding and coalesce pane output`

### A2（item 12）镜像模式丢弃解析器查询应答
- 内容：`dispatch_event` / `refresh_ui` 里只在 `!is_tmux_mirror` 时 `take_replies + send_input`；tmux/SSH 模式由 `refresh-client -r` 代答 OSC/DA。
- 文件：`window.rs`、`layout_host.rs`（is_tmux_mirror 已加）。
- 验收：`git lg` 字面量不泄漏；用真实 a.log（`tests/logs`）复放。
- Commit：`fix(linux): drop parser query replies in tmux mirror mode`

### A3（item 9）Pane 全屏
- 内容：`Action::TogglePaneFullscreen`：tmux/SSH 模式 `bridge.execute(toggle_pane_fullscreen(pane_id))`；本地模式 `layout.set_fullscreen_pane(Some/None)` 后 `refresh_ui`。
- 文件：`window.rs`、`layout_host.rs`（已实现状态）。
- 验收：tmux 模式发 `resize-pane -Z`；本地模式布局重建为单 pane；再按恢复。
- Commit：`feat(linux): toggle pane fullscreen via tmux zoom and local layout`

### A4（item 8）字体缩放 + TOML 配置 + 持久化
- 内容：`Action::Increase/Decrease/ResetFontSize` → `FontSettings::zoomed` → `layout.set_font_size`，保存 `Preferences.font_size`；启动时 `cfg.font` → `FontSettings`；`Preferences.font_size` 覆盖 config。
- 文件：`window.rs`、`layout_host.rs`、`renderer.rs`（已实现）、`quickconnect/font.rs`（已实现）。
- 验收：Ctrl+Plus/Minus/0 生效；重启后字号保持；`[font] family/size` 生效。
- Commit：`feat(linux): wire font zoom actions and persist preferences`

### A5（item 5）tmux Status Bar 接线
- 内容：`UiState.status` 从 `Label` 换成 `StatusBar`；启动加载 `cfg.statusbar.mode`；轮询 `bridge.status_snapshot()`（间隔取 tmux `status-interval`，无则 1s）调用 `apply`；窗口按钮点击 → `switch_tab`；tab/pane 变化时刷新。
- 文件：`status_bar.rs`（已实现）、`window.rs`、`mod.rs`（注册）、`ffi_bridge.rs`（snapshot 已实现）。
- 验收：left/窗口列表/right 与 tmux 一致；256 色/#hex/属性正确；justify 生效；点击窗口切 tab；status-interval 刷新。
- Commit：`feat(linux): wire tmux-consistent status bar with clickable windows`

### A6（item 7）状态栏模式切换
- 内容：命令面板/键位触发 `statusbar_mode` 切换（tmux ↔ theme）；`StatusBar::set_mode` + `apply`；保存 `Preferences.statusbar_mode`；启动时读取。
- 文件：`status_bar.rs`（已实现）、`window.rs`、`command_palette.rs`、`quickconnect/font.rs`。
- 验收：两种模式即时切换且重启保持；theme 模式忽略 tmux 配色。
- Commit：`feat(linux): add status bar mode switch with persistence`

### A7（item 6）主题切换 + 重报色
- 内容：命令面板/键位切换 light/dark；`layout.apply_theme` + `status_bar.apply_theme`；保存 `Preferences.theme`；切换/连接后 `bridge.report_all_pane_colours`（tmux ≥3.2 才发，参考 16987e7）。
- 文件：`window.rs`、`layout_host.rs`（已实现）、`status_bar.rs`（已实现）、`command_palette.rs`、`quickconnect/font.rs`。
- 验收：切换即时生效；所有已有 pane 变色；tmux 侧 pane 颜色重报；重启保持。
- Commit：`feat(linux): switch light/dark theme and re-report pane colours`

### A8（item 10）Tab 切换门禁 + 事件策略
- 内容：把 `TabSwitchGate` / `EventPolicy` 接进 `dispatch_event`：后台 tab 的 `STATE_LAYOUT_CHANGED` 不触发前台重建；切 tab 时挂起 layout 重绘直到 `STATE_ACTIVE_TAB_CHANGED`。
- 文件：`window.rs`、`quickconnect/tab_gate.rs`、`quickconnect/event_policy.rs`（逻辑已实现）。
- 验收：htop/codex 在后台 tab 时前台不闪不重绘；切换后正确显示。
- Commit：`fix(linux): gate tab switches and ignore background-tab layout events`

---

## 7. 阶段 B：QuickConnect 链路（单 agent 串行）

### B1（item 1）QuickConnect 面板
- 内容：新建 `src/platform/linux/quickconnect_panel.rs`（GTK Window/Popover）：搜索框 + 列表；Recent（前 5 条）+ Project 去重合并 + badges；当前连接高亮（`ConnectionPool::current_target_config` 未接入前先用 `store` + 当前 pane/backend 判定，B4 后切换）；回车连接、双击/编辑入口、New Project 行；`Action::QuickConnect`（Alt+Q）与命令面板打开。
- 文件：新文件 + `window.rs`（Action 接线）+ `command_palette.rs` + `store.rs`（已实现）。
- 验收：面板行为与 `QuickConnectController.swift` 一致；搜索、badges、高亮、连接回调正确。
- Commit：`feat(linux): add quick connect panel with recent/project list`

### B2（item 2）TargetConfig 窗口
- 内容：新建 `target_config_window.rs`：runtime/transport 单选卡片（`TargetOptionSelection`）；SSH alias 可编辑下拉（`discover_ssh_hosts`）；path 输入 + 上级按钮 + 目录异步补全（`DirectorySuggestionController` + glib timeout 防抖 + generation 防竞态）；name 自动派生（`QuickConnect::default_name`，手动编辑后不覆盖）；新建/编辑/保存到 store。
- 文件：新文件 + `window.rs`/`quickconnect_panel.rs`（打开入口）+ `directory.rs`/`options.rs`（已实现）。
- 验收：与 `TargetConfigWindow.swift` 行为一致；快速输入不竞态；SSH 目录走远程 discovery。
- Commit：`feat(linux): add target config window with async directory completion`

### B3（item 3）Project 连接流程接线
- 内容：把 `ProjectConnectFlow` 接到真实 FFI：attach 已有 session → 失败则 `create_tmux_session(directory)` → attach；local/ssh 共用；失败阶段区分（attach/create/attach-created）；面板显示进度/错误。
- 文件：`window.rs`、`quickconnect_panel.rs`、`ffi_bridge.rs`（connect/create 已实现）、`project_flow.rs`（已实现）。
- 验收：isolated tmux 下三种路径（已有 session / 无 session 创建成功 / 创建失败）都有真实 e2e 证据。
- Commit：`feat(linux): wire project connect state machine to ffi bridge`

### B4（item 4）Warm Connection Pool 接入
- 内容：实现真实 `ConnectionSlotProtocol`（持有 CoreBridge handle + PaneView/窗口状态）；`acquire` 复用/创建、LRU 容量淘汰（`[pool] max_slots`）、TTL 淘汰、后台 poll（复用现有 16ms 轮询或独立间隔）；tmux detach 保留 session，local 按策略 shutdown；面板 Recent 改为由 `pool.recent_target_configs` 派生；当前高亮用 `current_target_config`。
- 文件：新 `connection_slot.rs` + `window.rs`、`quickconnect_panel.rs`、`pool.rs`（已实现）、`ffi_bridge.rs`。
- 验收：`ConnectionPoolTests.swift` 对应行为在 Linux 单测覆盖；isolated tmux 下 detach 后 session 仍在、再 connect 复用。
- Commit：`feat(linux): integrate warm connection pool with lru/ttl eviction`

---

## 8. 阶段 C：收尾（可并行叶子）

### C1（item 13）键位动作补全
- 内容：确认并补齐 `Action::Quit`（关窗/退出）、QuickConnect、字体、全屏在 window 的绑定（A3/A4/B1 已接大部分）；自定义键位覆盖默认的测试。
- 文件：`keymap.rs`、`core/config.rs` 默认绑定、`window.rs`（只补 Quit 分支）。
- Commit：`feat(linux): complete custom keybinding actions`

### C2（item 14）i18n 补齐
- 内容：grep `src/platform/linux` 硬编码 UI 字符串（标题、placeholder、按钮、错误）；新面板/窗口文案全部走 `TextKey`；en/zh-CN 同步；保持 parity 测试；对照 macOS `Resources/i18n/*.json` 补缺失 key。
- 文件：`src/platform/i18n/mod.rs`、`locales/*.json`、相关新 UI 文件。
- Commit：`feat(i18n): complete linux ui strings and catalog parity`

### C3 集成测试（isolated tmux e2e）
- 内容：用 `tmux -L muxterm-test-<unique>` 覆盖：status bar snapshot 与 tmux 一致、mirror 模式 git lg 不泄漏、全屏 zoom、project flow attach/create/attach、字体/主题持久化、连接池 detach 保留 session。
- 文件：`tests/` 或 `tests/logs` 样例 + 必要时 `scripts/`。
- Commit：`test(linux): end-to-end cover status bar, mirror, fullscreen and project flow`

---

## 9. 并发策略（重要）

### 9.1 为什么骨架必须串行

阶段 0/A/B 的所有任务都经过 `window.rs`：Action 分发、事件分发、refresh_ui、连接创建、状态持有。两个 agent 同时改 `window.rs` 必然冲突，且冲突合并成本 > 并行收益。这是 Codex 官方文档明确提示的“写密集并行会增加冲突与协调开销”场景。

结论：**阶段 0 + A + B 用 1 个主 agent 连续跑**，每完成一项 `cargo check/test` 并 commit，下一项从干净基线开始。

### 9.2 并发只用于叶子：文件所有权矩阵

| 任务 | 拥有文件 | 是否可并行 |
|---|---|---|
| A1–A8 | `window.rs` + `layout_host.rs`/`pane_view.rs`/`status_bar.rs`/`command_palette.rs` | ❌ 串行 |
| B1–B4 | `window.rs` + 新面板/窗口/连接槽 | ❌ 串行 |
| C1 | `keymap.rs`、`config.rs` 默认绑定 | ✅ 可并行（不碰 window.rs 主体） |
| C2 | `i18n/mod.rs`、`locales/*.json`、新 UI 文案 | ✅ 可并行 |
| C3 | `tests/`、`tests/logs`、`scripts/`（只跑隔离 tmux） | ✅ 可并行 |

硬规则：
1. 同一时刻**一个文件只允许一个 agent 写**。
2. subagent 只产出 diff/测试结果，**不 commit**；主 agent review 后按功能点 commit。
3. 并行任务必须声明白名单（允许改的文件）和黑名单（禁止改的文件，如 `window.rs`）。
4. 任何 agent 不得运行默认 server 上的 tmux 写命令。

### 9.3 具体操作方式（推荐）

- 方式 1（推荐）：1 个交互式 Codex 主会话（`multi_agent` 已 stable=true）。阶段 C 时在 prompt 里显式要求“spawn 2–3 个 subagent，分别做 C1/C2/C3，等全部返回后 review 并逐个 commit”。CLI 里用 `/agent` 查看/切换线程。
- 方式 2：3 个 `codex exec -C <worktree>`，每个独立 worktree/分支，最后主分支逐个 merge。仅在用户明确要求多会话时使用（需先创建 worktree）。
- 模型建议：主 agent 用 `gpt-5.6`；只读探索/移植参考用 `gpt-5.6-terra`（官方建议 fast scans）。subagent 额外耗 token，不要 14 项全撒出去。

### 9.4 为什么不选 OpenCode 多 agent

- 本地 `opencode 1.18.16` 可用，官方文档支持 build/plan primary + general/explore/scout subagent。
- 但它没有比 Codex 更强的“写并行协调”，同一份文件冲突问题一样存在；切换会丢掉本仓库已有的 AGENTS 约定、Codex 记忆与技能；收益只有阶段 C 的叶子任务，不值得迁移。

### 9.5 并发风险与兜底

- 若两个 subagent 违反文件所有权 → 主 agent 拒绝合并，要求重做，不自行“解决冲突”。
- 若 C 阶段三个任务仍互相引用（如 C2 改 B1 新文件文案、C1 改 window.rs）→ 退回串行，按 C1 → C2 → C3 顺序执行。
- 任何一步 `cargo test` 红 → 停在该 commit，先修复再继续，不带着红色基线进入下一项。

---

## 10. 每项验收与提交自检

每个 commit 前：
- [ ] `cargo fmt --check`
- [ ] `cargo check`
- [ ] 相关 `cargo test`（新增逻辑必须有单测）
- [ ] `cargo clippy -- -D warnings`
- [ ] commit 信息 `feat/fix/test/docs/refactor/chore(linux|i18n|core): ...`，英文，无 Co-authored-by
- [ ] 只包含本功能点文件，不夹带无关改动

---

## 11. 测试与 tmux 安全红线

- 需要真实 tmux 的验证一律：
  - 创建：`tmux -L muxterm-test-<唯一后缀> new-session -d -s <name>`
  - 操作：每条命令都带同一个 `-L muxterm-test-<唯一后缀>`
  - 清理：`tmux -L muxterm-test-<唯一后缀> kill-server`（只杀自己的测试 server）
- 默认 server 只允许 `tmux ls / list-sessions / list-windows / list-panes / has-session`。
- 本地已有真实样例 `tests/logs/`（htop/codex/git lg 等）可用于复放，避免依赖 GUI 手动验证。

---

## 12. 完成定义

- 14 项全部在 Linux GTK 可用，行为与 macOS 一致（必要时在 README/PRODUCT 勾选）。
- 每项有独立 commit，`cargo test` 全绿。
- status bar / QuickConnect / TargetConfig / pool 有 isolated tmux 或单测证据。
- 工作树干净（或只有用户确认保留的改动），不 push。

---

## 13. 首次执行步骤（等用户确认后开始）

1. `cargo check` 确认当前红色基线（预期失败于 window.rs 旧 API）。
2. 阶段 0：迁移 window.rs 到新 API，恢复绿色，1 个 commit。
3. 从 A1 开始逐项执行，每项结束后汇报 commit hash + 测试证据。
