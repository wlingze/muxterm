# Muxterm 测试与开发规范

> 适用：`/home/wlz/Developer/self/muxterm`（当前 Linux 分支 `feat/linux-quickconnect-ui`）。
> 配套文档：[AGENTS.md](../AGENTS.md)（环境/提交约定）、[ARCHITECTURE.md](../ARCHITECTURE.md)（交互模型）、[TASKS.md](../TASKS.md)（Linux GTK 移植执行计划）、[bugfix-log.md](bugfix-log.md)（历史坑）。

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
cargo test --all-features
```

- 默认 `cargo test` 通过只是烟雾信号；最终门是 `--all-features` + 上面两条 GTK e2e。
- 每次汇报必须区分：默认套件 / GTK e2e / SSH / all-features 各自的真实退出码与失败项。

## 4. 开发流程（TDD 优先）

1. 读文档：`PRODUCT.md` → `ARCHITECTURE.md` → `AGENTS.md` → `TASKS.md` → 本文档。
2. RED：写最小单测或 e2e，先看到失败（真实数据 fixture 优先）。
3. GREEN：写最小实现，只改本功能相关文件。
4. 补测试：增加边界、错误路径、真实 tmux 数据复放。
5. 跑验证门（第 3.6 节），全绿后：
6. 独立 commit：英文 subject `type(scope): description`，body 英文逐条列改动；不加 `Co-authored-by`。
7. 不 push，除非用户明确要求；汇报 commit hash、工作树、测试结果。

共享 GTK 文件（`window.rs`、`layout_host.rs`、`pane_view.rs`、`status_bar.rs`）同一时刻只允许一个 agent 写；并行只用于叶子任务（键位、i18n、隔离集成测试）。

## 5. GUI e2e 编写指南

参考 `tests/linux_gtk_integration.rs` 的既有手段：

- 环境：无 DISPLAY 时用 `xvfb-run -a`；`gtk_test_*` 系列（`test_register_all_types` / `test_widget_wait_for_draw`）保证 GTK 测试类型可用。
- 按键：`EventControllerKey` + `simulate_key_press` 模拟 Alt+S / Alt+V / Alt+T / Alt+1/2 等真实路径。
- 等待：`pump_main_loop(ms)` 推进主循环，`wait_until(app, ms, pred)` 等待状态收敛，不要裸 sleep。
- 断言分三层：
  1. core 状态（`TerminalModel` / layout / snapshot）；
  2. widget 树（`count_paned`、`has_nested_paned`、`widget_label_texts`、`find_toggle_with_title`）；
  3. 页面可见结果（VTE `text_format` 可见文本，如 `assert_active_pane_echo`、tab bar 文本）。
- 真实 tmux 的 e2e 参考 `tests/linux_quickconnect_e2e.rs`：`unique_socket` + `wait_until` + 结束 `drop` 时用同一 socket 清理。

新功能的 e2e 检查单：

- [ ] 打开入口（快捷键/命令面板/状态栏点击）有断言
- [ ] 交互后的 core 状态变化有断言
- [ ] 页面可见变化（文本/布局/样式）有断言
- [ ] 涉及持久化时，重启后保持有断言
- [ ] 涉及真实 tmux 时，用隔离 socket 有真实 e2e

## 6. 真实 tmux 数据规范

已纳入测试的真实样例（`tests/samples/`）：

- `real-htop.txt` / `real-git_lg.txt` / `real-ls_la.txt` / `real-codex.txt`：core parser/client 单测
- `real-gitlg-osc-query.txt`：镜像模式查询应答不转发（`src/core/protocol/terminal/mirror.rs`）
- `new-session*.txt` / `attach*.txt` / `cmd-response.txt` / `2tab-3pane-cc.txt`：协议解析

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
| 5 | tmux Status Bar | ✅ status_style | ⚠️ snapshot 已断言；点击窗口切 tab 待补 | ✅ status snapshot e2e |
| 6 | 主题切换 + 重报色 | ✅ theme/font | ⚠️ 偏好持久化已覆盖；即时切换/颜色重报待补 | ⚠️ 部分 |
| 7 | 状态栏模式切换 | ✅ set_mode | ❌ 待补 | ❌ 待补 |
| 8 | 字体缩放 + 持久化 | ✅ font | ✅ 偏好 roundtrip；按键即时生效待补 | — |
| 9 | Pane 全屏 | ✅ layout 状态 | ✅ zoom e2e；本地布局切换待补 | ✅ e2e 真实 tmux |
| 10 | Tab 门禁 + 事件策略 | ✅ tab_gate/event_policy | ❌ 待补 | ❌ 待补 |
| 11 | resize→feed + 输出合并 | ✅ pane_view | ⚠️ 2tab3pane 键盘流程已覆盖；真实 htop/git lg 的 GTK 复放待补 | ✅ core 单测有真实样本 |
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
| 23 | 配置页 | ✅ config_edit | ✅ linux_prefs_e2e | — |
| 24 | pane-cmd 订阅 | ✅ protocol/backend | — | ✅ tmux_backend scenario5 |

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
- 字体 Ctrl+Plus/Minus/0 即时生效且重启保持；主题切换即时生效
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
- GTK 首帧尺寸错误 → 先 `seed_snapshot(output, cols, rows)` 再喂增量，不要用 client 尺寸代替 pane 尺寸。
- GTK 与 tmux 行列差一 → 用 pane 的 `WxH`，不是 `refresh-client -C` 的 client 尺寸。
- 测试 server 没清理 → 创建与清理必须带同一个 `-L`。
- `*.log` 不许提交；`tests/samples/*.txt` 必须提交。
