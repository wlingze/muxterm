# Muxterm 测试与开发规范

> 适用：`/home/wlz/Developer/self/muxterm`（当前 Linux 分支 `feat/linux-quickconnect-ui`）。
> 配套文档：[AGENTS.md](../AGENTS.md)、[ARCHITECTURE.md](../ARCHITECTURE.md)、
> [WORKSPACE.md](WORKSPACE.md) / [WORKSPACE-PLAN.md](WORKSPACE-PLAN.md)（**当前执行计划**）、
> [SURFACE.md](SURFACE.md) / [SURFACE-PLAN.md](SURFACE-PLAN.md)（F 已冻结）、
> [LINUX-PLAN.md](LINUX-PLAN.md)（Phase E 档案）、[TASKS.md](../TASKS.md)（已冻结）、
> [bugfix-log.md](bugfix-log.md)。

## 1. 四条硬性要求（验收红线）

1. **单元测试一定要有**：所有新增纯逻辑（协议解析、模型、状态机、样式解析等）必须带 `#[cfg(test)]` 单测，覆盖主要分支与边界。
2. **GUI e2e 测试一定要有**：所有用户可见的 Linux GTK 功能必须有 GTK 集成/e2e 用例；纯模型或 FFI 测试通过**不等于**功能完成。
3. **真实 tmux 数据都要在测试里通过**：抓到的真实 tmux 输出（htop / codex / git lg / OSC 查询等）必须落成 fixture 并断言，不能只留在本地日志里。
4. **功能在页面上可明确使用，e2e 覆盖大部分功能**：每个功能要能通过真实 UI 路径操作（键盘/面板/状态栏），且大部分功能有对应 e2e 用例。

## 2. 测试分层与现有资产

| 层 | 载体 | 说明 |
|---|---|---|
| 单元测试 | `src/**` 内 `#[cfg(test)]` | 纯逻辑；当前 75 个源文件含测试，`quickconnect/` 59 个用例 |
| 协议解析 | `src/core/protocol/`、`src/core/runtime/tmux/` | 用 `include_str!` 直接喂 `tests/samples/*.txt` 真实数据 |
| 核心集成 | `tests/cli_integration.rs`、`tmux_backend_integration.rs`、`sendkeys_regression.rs`、`split_regression.rs` | 真实 tmux（隔离 socket）或 CLI 行为 |
| TUI 集成 | `tests/tui_integration.rs`、`streaming_output_integration.rs` | TUI 渲染捕获、时间行为（持续/高频/长行输出） |
| FFI 回归 | `tests/tui_split_ffi_regression.rs`、`tui_wizard_ffi_regression.rs`、`tui_wizard_ssh_ffi_regression.rs` | UI 按键最终走的 FFI 路径 |
| SSH e2e | `tests/ssh_streaming_integration.rs`、`ssh_transport_unit.rs`、`ssh_no_fallback.rs` | loopback sshd，`--ignored` 运行 |
| GUI e2e | `tests/linux_gtk_integration.rs`、`tests/linux_quickconnect_e2e.rs` | Xvfb 下真实 GTK4 窗口 + 隔离 tmux |
| 真实数据 | `tests/samples/*.txt`（进 git）、`tests/logs/*.log`（本地素材，不进 git） | 前者被 core 单测引用；后者用于本地复现 |

## 3. 硬性规则

### 3.1 功能与测试同步

- 新 `pub fn` / 状态机 / 解析逻辑：先写单测（RED），再实现（GREEN）。
- 新用户可见功能：必须同时有 GTK e2e（或先有明确理由说明为什么无法自动化，并人工验收）。
- 修复 bug：先落一个能复现的最小测试（真实数据 fixture 优先），再修。

### 3.2 真实 tmux 数据

- 抓取：`tmux -L muxterm-test-<唯一后缀> -CC new-session -x 100 -y 30`，参考 `tests/samples/grab.sh`。
- 落地：小而可断言的场景进 `tests/samples/*.txt` 并提交；大体积原始日志放 `tests/logs/`（`*.log` 已被 `.gitignore` 排除，不提交）。
- 使用：core 侧用 `include_str!` 喂 parser/client/mirror 单测；GUI 侧复放进 pane 后断言 VTE 可见文本。
- 记录：fixture 文件头注明 tmux 版本（`tmux -V`）、窗口尺寸、复现步骤。

### 3.3 tmux 会话安全（最高优先级）

- 状态变更测试一律用独立 socket：`tmux -L muxterm-test-<唯一后缀>`，清理也带同一个 `-L`。
- 默认 server（不带 `-L`）只允许只读命令：`ls / list-sessions / list-windows / list-panes / has-session`。
- 本机有 `cat=bat` alias：检测 pane 前台命令用 `/bin/cat` + `tmux display-message -p -t %N '#{pane_current_command}'`，不要只看输入回显。
- 复用 `tests/support/tmux_test_support.rs`：`unique_socket` / `create_session` / `kill_server` / `wait_for` / `run_with_timeout`。

### 3.4 SSH 测试

- 需要 loopback sshd：`scripts/ci/setup-sshd.sh` 或 `tests/support/sshd_test_support.rs`。
- SSH 集成测试默认 `--ignored`，有 sshd 时用 `--ignored --test-threads=1` 跑。
- 密钥需求通过 `MUXTERM_TEST_SSH_KEY` 或测试环境已有授权 key 提供；没有授权 key 时不能宣称 SSH 用例通过。

### 3.5 时间行为测试

- 用硬超时 + `wait_for` 轮询，禁止用更长的 `sleep` “修”失败。
- `--all-features` 并发失败时，先 `--test-threads=1` + 独立日志 + 唯一 socket 复现，定位共享资源/时序问题，再修。

### 3.6 验证门（提交前自检）

```bash
cargo fmt --all -- --check
cargo check --features gtk
cargo clippy --all-targets -- -D warnings
cargo test --no-default-features --features tui
cargo test --features gtk --lib quickconnect -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_gtk_integration -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_quickconnect_e2e -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_gtk_support -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_panel_e2e -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_attention_e2e -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_prefs_e2e -- --test-threads=1
# Phase C 新增（落地后必须进门禁）：
xvfb-run -a cargo test --features gtk --test linux_chrome_e2e -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_render_e2e -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_live_e2e -- --test-threads=1
# W13 attach 保真（先建 2tab/3pane 再 attach；禁止用空 session echo 冒充）
cargo test --test tmux_attach_contract -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_workspace_attach_e2e -- --test-threads=1
# W14 功能保真（搜索/Done/mock-codex/tail-f；依赖 W13 播种）
cargo test --test tmux_feature_contract -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_feature_e2e -- --test-threads=1
# SSH 有 sshd 才跑：eval "$(./scripts/ci/setup-sshd.sh)" 后
# cargo test --test tmux_ssh_feature_contract -- --ignored --test-threads=1
# xvfb-run -a cargo test --features gtk --test linux_ssh_e2e -- --ignored --test-threads=1
cargo test --all-features
```

- 默认 `cargo test` 通过只是烟雾信号；最终门是 `--all-features` + 上面两条 GTK e2e。
- 每次汇报必须区分：默认套件 / GTK e2e / SSH / all-features 各自的真实退出码与失败项。

## 4. 开发流程（TDD 优先）

1. 读文档：`docs/WORKSPACE.md` → `docs/WORKSPACE-PLAN.md` → `PRODUCT.md` → `AGENTS.md` → `docs/SURFACE.md` → 本文档。
   `TASKS.md`、`LINUX-PLAN.md`、`SURFACE-PLAN.md` 已冻结，不要当新工作单。F 的 e2e 是回归门，不是本轮要重做的功能。
2. RED：写最小单测或 e2e，先看到失败（真实数据 fixture 优先）。
3. GREEN：写最小实现，只改本功能相关文件。
4. 补测试：增加边界、错误路径、真实 tmux 数据复放。
5. 跑验证门（第 3.6 节），全绿后：
6. 独立 commit：英文 subject `type(scope): description`，body 英文逐条列改动；不加 `Co-authored-by`。
7. 不 push，除非用户明确要求；汇报 commit hash、工作树、测试结果。

共享 GTK 文件（`window.rs`、`layout_host.rs`、`pane_view.rs`、`status_bar.rs`）同一时刻只允许一个 agent 写；并行只用于叶子任务（键位、i18n、隔离集成测试）。

## 5. GUI e2e 编写指南（硬性：不许自创更弱测试）

Phase F 的场景、函数名、断言（`linux_render_e2e` / `linux_live_e2e`）是 **回归门**：W 轮必须保持绿，**不要重写这些用例来迁就 dump**。
新功能按 [`WORKSPACE-PLAN.md`](WORKSPACE-PLAN.md) 写测试（两工作区切回、viewport、带 tab 的搜索）。
C8 ASCII 几何 / E 的 `visible_ansi` 单测可留作 Index，**不算** Surface 完成，也 **不是** live 显示路径。
`present_from_replica` 当直播、CUP 风暴 `resets==1`、只 `contains(TOKEN)` 不数次数 **不算**完成。

### 5.1 四层断言（缺一层就没做完）

| 层 | 对象 | 例子 |
|---|---|---|
| A core | `ReplicaStore` / `RenderPolicy` / `TerminalState` | `last_n_lines` 含 token；CUP 风暴只留末帧 |
| B widget | `widget_name` | `find_by_name(..., "muxterm-new-tab")`，禁止靠 Label 英文 |
| C VTE | `PaneView::visible_text()` | 含 `frame-19`，**不含** `frame-0`；底行 prompt 必须在**最后一行** |
| D 真 tmux | `tmux -L muxterm-test-… capture-pane` | 含 `echo` 的 token；清理必须同一 `-L` |

SSH 场景另加 loopback `scripts/ci/setup-sshd.sh` + `tests/support/sshd_test_support.rs`；
远端 tmux 也必须 `-L`。无 sshd 时 `#[ignore]`，默认门禁不跑，不算失败。

### 5.4 Attach 保真套件（跨平台契约，W13）

`linux_live_e2e` 的空 session + echo **不能**代替 attach。1820.log 是 SSH attach 到已有多 pane / Codex TUI 的 session：白屏、布局错、CPU 打满、零 `%pause`。

**夹具顺序（所有平台相同）：**

1. `tmux -L muxterm-test-<unique> new-session -d` 用 `/bin/cat`（不要默认 shell）。
2. split 成 **3 pane**，再 `new-window` 成 **2 tab**。
3. 每个 pane `send-keys -l` 涂独立 token，`capture-pane -p` 等到 token 出现。
4. **然后** Muxterm attach（Linux：`AppWindow` + `cfg.tmux.socket`；core：`TmuxRuntime::new_with_attach`）。
5. 断言 Surface / core 缓冲，不是只断言 tmux 侧还在。

**文件：**

- `tests/support/workspace_attach_contract.rs` — 无 GUI，macOS/Windows 复用同一套 token/布局/洪水上限。
- `tests/tmux_attach_contract.rs` — core 集成（无 DISPLAY）。
- `tests/linux_workspace_attach_e2e.rs` — GTK VTE + 控件几何。

**硬断言（改实现，不许改阈值来「绿」）：**

- attach 后 8s 内：2 个 tab；当前 tab 3 个 layout leaf。
- 每个已涂 token 的 pane：core `pane_output` **和** Surface 可见文本都含该 token（恰好能搜到，允许 ANSI 包裹）。
- 每个可见 pane 控件宽、高 ≥ 40px（0 尺寸 = 白屏）。
- 切到 tab 2 再切回：tab 1 的 token 还在 Surface（像素缓存）。
- CUP 洪水（`ESC[H ESC[2J` 循环）后 Surface 非空、停在末帧附近；`resets` 增量 ≤ 1。
- 洪水 1s 内 core `PaneOutput` 事件数 ≤ `MAX_OUTPUT_EVENTS_PER_SEC`（400），否则必须已向 tmux 发 pause。对照 1820.log：单 pane 约 1000 事件/秒且 0 pause。

禁止：用 `new-session` 之后立刻 echo 冒充 attach；只 `contains(TOKEN)` 不查 pane 几何；把洪水测试标 `#[ignore]`。

### 5.5 功能保真套件（W14）

规格：[`FEATURE-E2E-PLAN.md`](FEATURE-E2E-PLAN.md)。`linux_search_e2e`（Mock PaneBuf）和 `linux_render_e2e`（静态 sample）**保留作回归，不算本套件**。

| crate | 层 | 必须抓住 |
|---|---|---|
| `tmux_feature_contract` | A core | 搜索播种 token；OSC 133 D + BEL 信号；mock-codex 末帧进 PaneBuf；tail -f 追加进 PaneBuf；tracing target 存在；禁止每条 `%output` `debug!` |
| `linux_feature_e2e` | B+C GTK | **一个** AppWindow：Search tab 命中并跳转 VTE；后台 Done 通知；mock-codex VTE 含 HEADER/PROMPT；tail -f 新行在 VTE |
| `tmux_ssh_feature_contract` | D SSH | `#[ignore]`；远端 `/bin/cat` **先**涂 token 再 `new_ssh_attach`；PaneBuf 能搜到。禁止 MockRuntime |

```bash
cargo test --test tmux_feature_contract -- --test-threads=1
xvfb-run -a cargo test --features gtk --test linux_feature_e2e -- --test-threads=1
# 有 sshd：
eval "$(./scripts/ci/setup-sshd.sh)"
cargo test --test tmux_ssh_feature_contract -- --ignored --test-threads=1
```

禁止：把 Search/Done/flood 标 `#[ignore]`；用 `test_feed_replica` 冒充 `%output`；SSH 测试另建 Mock Workspace 喂字节。

### 5.2 手段（沿用现有 helper）

- 环境：无 DISPLAY 用 `xvfb-run -a`；`gtk4::test_synced`。无显示就 skip，不要空 assert。
- **同进程至多一个 `AppWindow`**。status bar / PaneView / prefs 用普通 `gtk4::Window`。
- 隔离 tmux：**复用** `tests/support/tmux_test_support.rs`，禁止再复制 `struct IsolatedTmux`。
- 按键：`EventControllerKey` + `simulate_key_press`。
- 等待：`pump_main_loop` / `wait_until` / `wait_until_widget`，禁止裸 `sleep` 当同步。
- 控件：每个可点的生产控件必须有稳定 `widget_name`；测试只 `find_by_name`。

### 5.3 怎么跑（Phase F / Surface）

见 `SURFACE-PLAN.md` §0 / §3。继续用已有 crate：`linux_render_e2e`、`linux_live_e2e`。
动手前必读 `docs/SURFACE.md` 与 `tests/samples/dogfood-2026-0815-2105.txt`；
原 `.log` 只许 `rg`，禁止 `include_str!`。Codex TUI fixture **raw feed**，禁止经 `visible_ansi`。

### 5.5 检查单

- [ ] 入口（快捷键 / 状态栏按钮 / 真实 attach）有断言
- [ ] core 状态变化有断言
- [ ] widget_name + VTE 文本有断言
- [ ] 持久化写的是 `config.toml`（不是 `preferences.toml`）
- [ ] 真 tmux 用隔离 `-L`；Drop 带同一 `-L` 的 `kill-server`
- [ ] 场景函数名与 `SURFACE-PLAN.md` 一致；打字 token **恰好一次**；CUP seed 后 `resets` 不涨
- [ ] 几何用例比行号，不只 `contains(TOKEN)`
- [ ] 状态点走 `clicked`，没有 `popover.popup()` 冒充
- [ ] 已读 `dogfood-2026-0815-2105.txt` 与 `codex-tui-sanitized.txt`

## 6. 真实 tmux 数据规范

已纳入测试的真实样例（`tests/samples/`）：

- `real-htop.txt` / `real-git_lg.txt` / `real-ls_la.txt`：core parser/client 单测
- `real-codex.txt` 目前是空文件，**不要**当 fixture；CUP 刷屏用计划里的合成帧
- `real-gitlg-osc-query.txt`：镜像模式查询应答不转发（`src/core/protocol/terminal/mirror.rs`）
- `osc-attention-tmux3.7b.txt`：OSC 133 / BEL 透传
- `dogfood-2026-0815-1326.txt`：2026-08-15 13:26 SSH attach 摘录（session `$4` / 点 tab；Phase C 已修）
- `dogfood-2026-0815-1540.txt`：2026-08-15 15:40（backend 有 capture/%output；切 tab）
- `dogfood-2026-0815-1854.txt`：2026-08-15 18:54 再测（切 tab/状态点好；Codex 仍无画面；log 无 payload）
- `dogfood-2026-0815-2105.txt`：2026-08-15 21:05（闪烁/白屏/越写越长；298k `%output`；`%64` WARN）
- `codex-tui-sanitized.txt`：合成 Codex 风格 TUI；Surface 测试必须 **raw feed**，禁止 `visible_ansi`

新增要求：

1. 每次遇到新的真实输出/复现素材，先抓取落盘（`grab.sh` 参考），确认可稳定复现。
2. 小样本进 `tests/samples/` 并接入断言；大日志进 `tests/logs/` 仅本地使用。
3. 修 bug 时，修复 commit 必须包含该 fixture 的断言，防止回归。

## 7. 功能验收矩阵（当前基线）

> ✅ = 已有自动化用例；⚠️ = 部分覆盖；❌ = 待补。完成声明必须以实际运行结果为准，不以“有代码”为准。

| # | 功能 | 单测 | GUI e2e | 真实 tmux 数据 |
|---|---|---|---|---|
| 1 | QuickConnect 面板 | ✅ model/store/panel 构建 | ❌ 待补（打开/搜索/高亮/回车连接） | ⚠️ 连接流程在 e2e 覆盖 |
| 2 | TargetConfig 窗口 | ✅ options/directory/debounce | ⚠️ SSH toggle debounce 已覆盖；完整窗口流程待补 | ✅ 隔离 tmux 目录发现 |
| 3 | Project 连接流程 | ✅ project_flow | ✅ attach→create→attach | ✅ e2e 真实 tmux |
| 4 | Warm Connection Pool | ✅ pool | ✅ detach 保留 session | ✅ e2e 真实 tmux |
| 5 | 统一 status bar（左中右 + 最右三按钮） | ✅ lifecycle / status_bar | ✅ linux_chrome_e2e S5/S6 | — |
| 5b | 鼠标点 tab 切窗口 | ✅ attach session id（C7.0） | ✅ S13a；live S13b | ✅ dogfood-1326 + 1540（1540 已无「忽略其它 session」） |
| 6 | 主题切换 + 重报色 | ✅ theme/font | ⚠️ 偏好持久化已覆盖；即时切换/颜色重报待补 | ⚠️ 部分 |
| 7 | 状态栏模式切换 | ✅ set_mode | ❌ 待补 | ❌ 待补 |
| 8 | 字体缩放 Ctrl+=/-/0 → config.toml | ✅ keymap/config_edit | ✅ linux_prefs_e2e S10 | — |
| 9 | Pane 全屏 | ✅ layout 状态 | ✅ zoom e2e；本地布局切换待补 | ✅ e2e 真实 tmux |
| 10 | Tab 门禁 + 事件策略 | ✅ tab_gate/event_policy | ❌ 待补 | ❌ 待补 |
| 11 | resize→feed + 输出合并 | ✅ pane_view 25ms | ⚠️ replica 播种在，几何仍有损（C8） | ✅ core 单测有真实样本 |
| 11b | 终端层末帧渲染 | ✅ render_policy | ⚠️ S3/S4/S9 只 contains 末帧 token，不比行号 | ⚠️ 合成 CUP；勿用空的 real-codex.txt |
| 12 | 镜像模式丢弃应答 | ✅ mirror + 真实 OSC 样本 | ✅ mirror e2e | ✅ 真实样本 |
| 13 | 键位扩展 | ✅ keymap defaults | ⚠️ Alt+S/V/1/2 已覆盖；Quit/字体/全屏按键待补 | — |
| 14 | i18n 补齐 | ✅ en/zh parity | ✅ 面板 tab/占位文案在 panel e2e 断言 | — |
| 15 | scrollback 上限/seq/search | ✅ emulate C1.* | — | — |
| 16 | 粘贴安全 | ✅ mirror sanitize | — | — |
| 17 | ReplicaStore + 后台 feed | ✅ replica/pool | — | — |
| 18 | OSC/BEL 注意力信号 | ✅ emulate + E1 fixture | — | ✅ osc-attention-tmux3.7b.txt |
| 19 | 状态机/聚合/静音 | ✅ attention::* | — | — |
| 20 | 三 tab 面板 | ✅ panel_model | ✅ linux_panel_e2e | — |
| 21 | peek/一行答复 | ✅ panel 钩子 | ✅ linux_panel_e2e / linux_attention_e2e | ✅ attention e2e 真实 tmux |
| 22 | 红点/标题 | ✅ attention_ui 字符串 | ✅ linux_attention_e2e | ✅ 注入 BEL + printf |
| 23 | 配置页 widget_name | ✅ config_edit | ✅ linux_prefs_e2e | — |
| 24 | pane-cmd 订阅 | ✅ protocol/backend | — | ✅ tmux_backend scenario5 |
| 25 | URL 点击 | ✅ url_detect | ✅ linux_render_e2e S11 | — |
| 26 | RenderPolicy 末帧 | ✅ render_policy | ✅ linux_render_e2e S3/S4 | ✅ live CUP 脚本 S9 |
| 27 | 独立 TabBar | — | ✅ Phase C 已删第二条带子 | — |
| 28 | 状态 popover 真点击 + 颜色 | ✅ CSS 三色 | ✅ C8.4 clicked；⚠️ 无 SSH 上下行（E4） | — |
| 29 | 几何 visible_ansi ASCII 底行 | ✅ C8.1 snapshot | ✅ C8.2/C8.5 ASCII PROMPT | ⚠️ 不够测 Codex |
| 30 | replica 滚动历史 | ✅ scroll_history | ✅ linux_render_e2e C8.3 | — |
| 31 | 真隔离 tmux echo | ✅ replica | ⚠️ S8 contains TOKEN | ✅ capture-pane |
| 32 | loopback SSH 远端 tmux | ✅ CoreBridge | ✅ linux_ssh_e2e S12（ignore） | ✅ 远端 -L |
| 33 | Codex TUI UTF-8+真彩播种 | ❌ `ch as u8`（E1） | ❌ 待 E2 | ✅ codex-tui-sanitized.txt |
| 34 | CUP 半帧不打烂 VTE | ❌ 仍 feed last_visible_frame（E3） | ❌ 待 E3 | 1854 len 1365/2730 |
| 35 | SSH popover 上下行 | ❌ 无计数 | ❌ 待 E4 | — |
| 36 | Search tab 搜 replica | ⚠️ core search 有 | ❌ placeholder_compiles（E5） | — |
| 37 | 前台 ls 不进 attention | ❌ CommandDone→Done | ❌ 待 E6 | — |
| 38 | attention 小 VTE + mute 下拉 | ❌ Label+Entry | ❌ 待 E6 | — |

## 8. 新增功能验收矩阵模板

| 功能 | 单测用例 | GTK e2e 用例 | 真实 tmux fixture | 页面可用性 |
|---|---|---|---|---|
| （名称） | 覆盖：…… | 覆盖：…… | 文件：…… | 入口：…… |

新功能合入前，表格不允许留空列。

## 9. 页面手动验收清单

自动化无法覆盖的视觉/交互检查，启动 `./build/linux/muxterm` 后逐项确认：

- 默认窗口有一个 tab / 一个 pane，焦点在终端
- Alt+T 新 tab；Alt+D / Alt+Shift+D 水平/垂直分割；Alt+1/2/0 切 tab；Alt+[ / Alt+] 切 pane
- Alt+P 命令面板可用；Alt+Q QuickConnect 面板打开、搜索、连接
- TargetConfig 窗口可编辑保存；目录补全不竞态
- 状态栏显示正确、点击窗口可切 tab；状态栏模式可切换
- 字体 Ctrl+= / Ctrl+- / Ctrl+0 即时生效且写入 `config.toml`；主题切换即时生效
- 只有一条 status bar：左/中/右同步 tmux，最右状态/通知/新建；没有第二条 tab 带
- 进入已有大量输出的 pane（或 Codex 刷屏）时画面落在当前帧且几何正确（提示符在底部），不从历史重放
- 滚轮向上能看到 replica 历史，滚回底部恢复直播
- 点状态点弹出连接摘要；SSH 显示 connecting/connected 颜色，以及 down=/up= 流量
- 前台自己跑完的命令（如 ls）不要出现在 attention；后台等待/完成才提醒
- attention 预览是小终端（可打字）；双击跳转；可放大；禁止提醒可选 5m/10m/30m/1h/4h/24h
- 搜索能搜到 pane 文本并跳转
- Codex/htop 全屏 TUI 头栏和底栏同时在，不要空白或挤成一团
- Pane 全屏进入/恢复；attach / new / ssh 向导三种路径可用；退出干净

### 视觉验收（截图 + 看图模型）

- 需要看像素的验收（主题、字体、布局观感）用截图交给 `gpt-5.6-luna` 看图，再由开发模型（当前 `deepseek-v4-flash`）修改。
- 截图验收只是补充，**不替代**第 3.6 节的断言测试；模型目录声明支持图片不等于实测可用，首次使用前先拿一张截图冒烟。

## 10. 完成定义（DoD）

- [ ] 单测存在且通过（含真实 fixture 断言）
- [ ] GUI e2e 存在且通过（Xvfb + 隔离 tmux）
- [ ] 真实 tmux 数据在测试中通过
- [ ] 功能在页面上按第 9 节可用
- [ ] `fmt` / `check` / `clippy -D warnings` / `all-features` 全绿
- [ ] 独立英文 commit，无 `Co-authored-by`，未 push（除非明确要求）
- [ ] 汇报中明确区分默认 / GTK / SSH / all-features 的验证证据

## 11. 常见坑（务必先查）

- `cat` 是 bat alias → 一律 `/bin/cat`。
- 用裸 `sleep` “修”时序失败 → 换成硬超时轮询。
- `--all-features` 并发 streaming 失败 → 先 `--test-threads=1` 复现，不掩盖。
- 镜像模式查询应答泄漏 → 查 `should_forward_replies` / `real-gitlg-osc-query.txt`。
- GTK 首帧：一次 `capture-pane` 的原始字节 `vte.feed`，**禁止** live 路径 `visible_ansi` dump，也禁止 `get_pane_output` 当滚动历史重放。
- `visible_ansi` 只给 Index（搜索）；禁止 skip 空行当 Index 单测。
- 状态点禁止 `GestureClick` + 测试里直接 `popover.popup()`；必须 `connect_clicked`。
- 镜像 VTE **不要**强制 `scrollback_lines=0`；滚动走 VTE，禁止 replica dump 冒充滚轮。
- CUP 风暴禁止 `vte.reset` 追帧；1365/2730 是前后半，都要 feed。
- dogfood：`dogfood-2026-0815-*.txt` 都要读（含 2105）；原 `test_2026-0815-*.log` 只许 `rg`。
- GTK 首帧尺寸错误 → 按 pane 的 cols/rows resize，不要用 client 尺寸代替 pane 尺寸。
- GTK 与 tmux 行列差一 → 用 pane 的 `WxH`，不是 `refresh-client -C` 的 client 尺寸。
- 测试 server 没清理 → 创建与清理必须带同一个 `-L`。
- `*.log` 不许提交；`tests/samples/*.txt` 必须提交。
