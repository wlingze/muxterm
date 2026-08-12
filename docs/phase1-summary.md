# Phase 1 总结：Project attach→create fallback、卡片高亮、目录补全（2026-08-12）

> 时间核验：2026-08-12T20:22+08:00（Asia/Shanghai）。
> 本阶段严格按 brief 执行前三项；**未实现 warm connection pool**（Phase 4，留待下阶段）。
> 未提交、未 push；原任务已核验的两处 A/B 改动（MainWindow.swift、TerminalView.swift）保持在工作区。

## 完成内容

### 1. Project attach → create fallback（local + ssh 共用语义）

- 新增纯逻辑状态机 `ProjectConnectFlow`（Chrome 层，无 AppKit 依赖）：
  - session 名 = 显式 name 优先，空 name 用 path basename（与 `~/.local/bin/twork` 对齐）；
  - 状态：`attachExisting → createDetached → attachCreated → done`；
  - 只有 attach 明确失败才创建 detached session；创建成功后再 attach 同一 session；
  - 失败分阶段：attach 已有 session / create / attach 已创建 session，分别展示。
- `MainWindow.connect(config:)`：
  - tmux runtime（local/ssh）与 shell+ssh 都走 Project 流程；
  - local shell 保持原有 `CoreBridge.connect(local)` 行为；
  - `activeProjectFlow` 身份比较防止旧连接异步回调覆盖新连接。
- `ConnectionDiscovery` 新增 `createSession(named:target:directory:)`：显式 session 名创建，**不再生成随机 `muxterm-<stem>-<suffix>`**。
- Rust `discovery.rs` 新增隔离 socket 真实测试：`new-session -d` 创建后列表可见、detached、清理只杀自己的 `-L` server。

### 2. TargetConfigWindow runtime/transport 卡片可见高亮

- 卡片改为无边框大按钮 + 自绘 layer：
  - selected：accent 背景（alpha 0.18）+ 2px accent 边框 + 粗体 + `✓ ` 前缀；
  - unselected：controlBackground + 1px separator 边框 + secondaryLabel；
  - 深浅色均由系统动态色（controlAccentColor / labelColor / separatorColor）驱动。
- AX：`setAccessibilityRole(.radioButton)`、稳定 identifier（`muxterm.target.runtime.tmux.selected` 等）、`setAccessibilityValue("selected"/"unselected")`。
- 纯状态模型 `TargetOptionSelection` 保证 runtime/transport 各自始终恰好一个选中。

### 3. 目录输入 / 补全 / 异步请求正确性

- 新增纯模型：
  - `DirectoryPathModel`：列表基目录、输入前缀、候选替换当前段、上级目录、`.`/`..`/`~`/`/`/尾斜杠归一化；
  - `DirectorySuggestionController`：generation + path + transport + alias 四元组请求 guard；
  - 候选只接受目录名、按当前输入前缀过滤、稳定排序；
  - 选择候选 = 进入该目录并替换输入段；完整路径候选忽略；重复选择幂等。
- AppKit 接入：
  - `NSComboBox.completes = false`（不再让系统默认文本补全改 path）；
  - `controlTextDidChange` 更新纯模型 + 120ms debounce 再发列表请求；
  - 异步响应只有与当前请求完全一致才更新/清空候选；
  - action 与 selectionDidChange 双触发幂等；SSH 无 alias 时不发请求、不回退本地。

## 真实测试结果

- Swift 定向（新增 26 个测试）：
  - `swift test --disable-swift-testing --filter ProjectConnectFlowTests` → 8 passed
  - `swift test --disable-swift-testing --filter TargetOptionSelectionTests` → 4 passed
  - `swift test --disable-swift-testing --filter DirectorySuggestionControllerTests` → 15 passed（含 1 个新增后为 15；共新增 26）
  - `swift test --disable-swift-testing --filter FlatChromeTests` → 2 passed（回归）
- Swift 全量：`swift test --disable-swift-testing` → **95 passed, 0 failed**（含全部回归）
- Swift 构建：`swift build` → Build complete（无警告）
- Rust 定向：`cargo test --no-default-features --features ffi --lib discovery` → **11 passed**（含新增 `create_local_tmux_session_uses_detached_isolated_socket` 真实 tmux 测试；全程独立 `-L muxterm-test-*` socket，清理带同一 `-L`）
- `cargo fmt --check` → 通过；`cargo clippy --no-default-features --features ffi --lib` → 无警告
- 额外手动隔离 socket 验证（带 `-L muxterm-test-*`）：
  - `tmux new-session -d -s proj -c /tmp` rc=0；`list-sessions` 显示 `proj,1,0`（detached）
  - attach 不存在的 session rc=1，stderr `can't find session: nonexistent`（确认 attach 失败语义可触发 fallback）
  - 重复创建同名 session rc=1（确认 create 失败语义）
  - 每次均 `tmux -L <同一socket> kill-server` 清理，未触碰默认 server

## 改动文件

- 新增：
  - `src/platform/macos/Chrome/Phase1QuickConnect.swift`（纯逻辑：ProjectConnectFlow / TargetOptionSelection / TargetOptionAccessibility / DirectoryPathModel / DirectorySuggestionController）
  - `src/platform/macos/ChromeTests/Phase1QuickConnectTests.swift`（26 个新测试）
  - `docs/phase1-summary.md`（本文件）
- 修改：
  - `src/platform/macos/App/MainWindow.swift`
  - `src/platform/macos/App/ConnectionDiscovery.swift`
  - `src/platform/macos/App/TargetConfigWindow.swift`
  - `src/core/discovery.rs`（新增隔离 socket 真实测试）

## 未证实项 / 后续风险

- **未做真实 SSH 端到端**：本机无法连接 `ryzen`/其他 alias；SSH 的 create/attach 仅与 local 共用同一状态机与 Rust FFI 路径，需在可达主机上人工验证。
- **未做 AppKit UI smoke/AX 自动化**：卡片视觉高亮只由纯状态模型 + 样式代码保证，未跑 XCUITest（需要 GUI 会话）；建议下阶段用 XCUITest 断言 AX identifier/value。
- **twork 多窗口初始化未实现**：按 brief 要求另作小步（Monitor/Workstation/Playground + split），只有先有测试再做；当前创建只保证 cwd/session 正确，不会隐式改用户已有项目。
- **CoreBridge attach 失败分类**：当前 attach 失败以 `CoreBridge(...)` 抛错为准（tmux `can't find session` 会触发 fallback）；若失败是超时/网络抖动，同样会尝试 create，可能落到“创建后 attach”分支并给出错误区分，行为可接受但需真实环境确认。
- **macOS 默认 `cargo test`/`cargo build` 会被 vte4-sys 阻断**：本机无 `vte-2.91-gtk4`（Linux GTK 依赖）。按项目 build-macos.sh 约定使用 `--no-default-features --features ffi/tui` 验证，全量默认构建只能在 Linux/GTK 环境跑。
- **未提交/push**：全部改动保持在工作区，等待下一步指令。
