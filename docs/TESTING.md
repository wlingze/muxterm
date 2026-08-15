# Muxterm 测试与开发规范

> 适用：`/home/wlz/Developer/self/muxterm`（当前 Linux 分支 `feat/linux-quickconnect-ui`）。
> 配套文档：[AGENTS.md](../AGENTS.md)、[ARCHITECTURE.md](../ARCHITECTURE.md)、
> [LINUX-PLAN.md](LINUX-PLAN.md)（**当前执行计划**）、[TASKS.md](../TASKS.md)（已冻结）、
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
# SSH 有 sshd 才跑：eval "$(./scripts/ci/setup-sshd.sh)" 后
# xvfb-run -a cargo test --features gtk --test linux_ssh_e2e -- --ignored --test-threads=1
cargo test --all-features
```

- 默认 `cargo test` 通过只是烟雾信号；最终门是 `--all-features` + 上面两条 GTK e2e。
- 每次汇报必须区分：默认套件 / GTK e2e / SSH / all-features 各自的真实退出码与失败项。

## 4. 开发流程（TDD 优先）

1. 读文档：`PRODUCT.md` → `ARCHITECTURE.md` → `AGENTS.md` → `docs/LINUX-PLAN.md` → 本文档。`TASKS.md` 已冻结，不要当工作单。
2. RED：写最小单测或 e2e，先看到失败（真实数据 fixture 优先）。
3. GREEN：写最小实现，只改本功能相关文件。
4. 补测试：增加边界、错误路径、真实 tmux 数据复放。
5. 跑验证门（第 3.6 节），全绿后：
6. 独立 commit：英文 subject `type(scope): description`，body 英文逐条列改动；不加 `Co-authored-by`。
7. 不 push，除非用户明确要求；汇报 commit hash、工作树、测试结果。

共享 GTK 文件（`window.rs`、`layout_host.rs`、`pane_view.rs`、`status_bar.rs`）同一时刻只允许一个 agent 写；并行只用于叶子任务（键位、i18n、隔离集成测试）。

## 5. GUI e2e 编写指南（硬性：不许自创更弱测试）

Phase C 的场景、函数名、断言、怎么跑：**只准**按 [`LINUX-PLAN.md`](LINUX-PLAN.md) §5。
没有写进该节的「能绿就行」测试（`placeholder_compiles`、按英文 `"Save"` 找按钮、
只用 `test_feed_replica` 代替真实 attach）**不算**完成。

### 5.1 四层断言（缺一层就没做完）

| 层 | 对象 | 例子 |
|---|---|---|
| A core | `ReplicaStore` / `RenderPolicy` / `TerminalState` | `last_n_lines` 含 token；CUP 风暴只留末帧 |
| B widget | `widget_name` | `find_by_name(..., "muxterm-new-tab")`，禁止靠 Label 英文 |
| C VTE | `PaneView::visible_text()` | 含 `frame-19`，**不含** `frame-0` |
| D 真 tmux | `tmux -L muxterm-test-… capture-pane` | 含 `echo` 的 token；清理必须同一 `-L` |

SSH 场景另加 loopback `scripts/ci/setup-sshd.sh` + `tests/support/sshd_test_support.rs`；
远端 tmux 也必须 `-L`。无 sshd 时 `#[ignore]`，默认门禁不跑，不算失败。

### 5.2 手段（沿用现有 helper）

- 环境：无 DISPLAY 用 `xvfb-run -a`；`gtk4::test_synced`。无显示就 skip，不要空 assert。
- **同进程至多一个 `AppWindow`**。status bar / PaneView / prefs 用普通 `gtk4::Window`。
- 隔离 tmux：**复用** `tests/support/tmux_test_support.rs`，禁止再复制 `struct IsolatedTmux`。
- 按键：`EventControllerKey` + `simulate_key_press`。
- 等待：`pump_main_loop` / `wait_until` / `wait_until_widget`，禁止裸 `sleep` 当同步。
- 控件：每个可点的生产控件必须有稳定 `widget_name`；测试只 `find_by_name`。

### 5.3 怎么跑（Phase C）

见 `LINUX-PLAN.md` §5.2 / §8。新增 crate：`linux_chrome_e2e`、`linux_render_e2e`、
`linux_live_e2e`、`linux_ssh_e2e`（ignore）。`linux_search_e2e` 本轮仍占位。

### 5.4 检查单

- [ ] 入口（快捷键 / 状态栏按钮 / 真实 attach）有断言
- [ ] core 状态变化有断言
- [ ] widget_name + VTE 文本有断言
- [ ] 持久化写的是 `config.toml`（不是 `preferences.toml`）
- [ ] 真 tmux 用隔离 `-L`；Drop 带同一 `-L` 的 `kill-server`
- [ ] 场景函数名与计划 §5.4 一致，没有放宽「含 frame 即可」这类断言

## 6. 真实 tmux 数据规范

已纳入测试的真实样例（`tests/samples/`）：

- `real-htop.txt` / `real-git_lg.txt` / `real-ls_la.txt`：core parser/client 单测
- `real-codex.txt` 目前是空文件，**不要**当 fixture；CUP 刷屏用计划里的合成帧
- `real-gitlg-osc-query.txt`：镜像模式查询应答不转发（`src/core/protocol/terminal/mirror.rs`）
- `osc-attention-tmux3.7b.txt`：OSC 133 / BEL 透传
- `dogfood-2026-0815-1326.txt`：2026-08-15 SSH attach 日志摘录（session `$4` / 点 tab）

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
| 5 | 统一 status bar（左中右 + 最右三按钮） | ⚠️ 旧 snapshot；Phase C 重做 | ❌ `linux_chrome_e2e` 待补 | ⚠️ 旧 snapshot e2e |
| 5b | 鼠标点 tab 切窗口 | ❌ `$4` session-window-changed 被忽略；list-windows `$0` | ❌ 按钮每 tick 重建；S13 待补 | ✅ dogfood-2026-0815-1326.txt |
| 6 | 主题切换 + 重报色 | ✅ theme/font | ⚠️ 偏好持久化已覆盖；即时切换/颜色重报待补 | ⚠️ 部分 |
| 7 | 状态栏模式切换 | ✅ set_mode | ❌ 待补 | ❌ 待补 |
| 8 | 字体缩放 Ctrl+=/-/0 → config.toml | ✅ font zoom 数值 | ❌ 仍写 preferences.toml；equal 未绑 | — |
| 9 | Pane 全屏 | ✅ layout 状态 | ✅ zoom e2e；本地布局切换待补 | ✅ e2e 真实 tmux |
| 10 | Tab 门禁 + 事件策略 | ✅ tab_gate/event_policy | ❌ 待补 | ❌ 待补 |
| 11 | resize→feed + 输出合并 | ✅ pane_view 25ms | ⚠️ 仍整段 `get_pane_output` 播种，agent 会刷屏 | ✅ core 单测有真实样本 |
| 11b | 终端层末帧渲染 | ❌ `render_policy` 待补 | ❌ `linux_render_e2e` / `linux_live_e2e` 待补 | ❌ 合成 CUP；勿用空的 real-codex.txt |
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
| 23 | 配置页 | ✅ config_edit | ⚠️ linux_prefs_e2e 靠英文 Save/下标，待 widget_name | — |
| 24 | pane-cmd 订阅 | ✅ protocol/backend | — | ✅ tmux_backend scenario5 |
| 25 | attach session id（$4 切 tab） | ✅ backend/protocol | — | ✅ dogfood 摘录 fixture |
| 26 | RenderPolicy 末帧 | ✅ render_policy | ✅ linux_render_e2e S3/S4 | ✅ live CUP 脚本 S9 |
| 27 | 统一 status bar（无 TabBar） | ✅ lifecycle | ✅ linux_chrome_e2e S5/S6/S13a | ✅ live 点 tab S13b |
| 28 | 状态 popover | ✅ ConnectionSummary | ✅ linux_chrome_e2e S7 | — |
| 29 | Ctrl+= 写 config.toml | ✅ keymap/config_edit | ✅ linux_prefs_e2e S10 | — |
| 30 | URL 点击 | ✅ url_detect | ✅ linux_render_e2e S11 | — |
| 31 | 配置页 widget_name | ✅ prefs 控件 | ✅ linux_prefs_e2e | — |
| 32 | 真隔离 tmux echo | ✅ replica | ✅ linux_live_e2e S8 | ✅ capture-pane |
| 33 | loopback SSH 远端 tmux | ✅ CoreBridge | ✅ linux_ssh_e2e S12（ignore） | ✅ 远端 -L capture-pane |
| 25 | URL 点击 | ⚠️ Cell.link 已有 | ❌ UrlOpener / VTE match 待补 | — |
| 26 | 独立 TabBar | — | ⚠️ 仍挂在 window.rs，Phase C 删除 | — |
| 27 | loopback SSH + 远端 -L tmux | — | ❌ `linux_ssh_e2e` `#[ignore]` 待补 | — |

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
- 进入已有大量输出的 pane（或 Codex 刷屏）时画面落在末尾，不从历史重放
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
- GTK 首帧刷屏 → 用 replica `visible_ansi()` 播种，**禁止** `get_pane_output` 整段重放。
- GTK 首帧尺寸错误 → 按 pane 的 cols/rows resize，不要用 client 尺寸代替 pane 尺寸。
- GTK 与 tmux 行列差一 → 用 pane 的 `WxH`，不是 `refresh-client -C` 的 client 尺寸。
- 测试 server 没清理 → 创建与清理必须带同一个 `-L`。
- `*.log` 不许提交；`tests/samples/*.txt` 必须提交。
