# macOS tmux attach：内置 VT 历史、直播渲染与命令时间线实施计划

> 更新时间：2026-08-19（Asia/Shanghai）
> 适用分支：`feature/quickconnect-attach-ui`
> 目标：修复 tmux attach 后“鼠标上划没有历史”的问题，并在 macOS 上统一支持历史滚动、回到底部、直播输出、上次看到位置和 shell 命令时间线。

## 1. 先给结论

core 已经具备这些能力：

- 内置 VT 解析 ANSI、维护当前屏和有界 scrollback；
- scrollback 行带稳定 seq，搜索、last-seen、命令跳转都不依赖文本猜测；
- attach 时可以生成一次性 Surface seed（带样式历史 + 当前屏）；
- `history_max_offset`、viewport 和 seq → offset 已经通过 FFI 暴露；
- OSC 133 A/B/C/D 会生成命令刻度，记录命令文本和退出码，并在行淘汰时同步淘汰刻度。

macOS 端的正确显示路径是“一个 SwiftTerm Surface”：attach 只 seed 一次，之后每个 `%output` 原样增量 feed；滚轮由 SwiftTerm 原生 scrollback 处理。不能在滚轮或 live 输出时反复把 `capture-pane` / `visible_ansi` 重播进 VT，也不能 reset 追逐 Cursor/Codex 的 CUP 帧。

当前协议有一个必须明确的边界：tmux `capture-pane` 返回的是已经被终端消费后的屏幕内容，不保留 attach 之前已经发生的 OSC 133。因此 attach 前的普通文本历史可以恢复，但 attach 前的完整命令刻度不能凭 tmux capture 重建。attach 后收到的 OSC 133 刻度可以完整导航；若产品以后要求恢复 attach 前命令时间线，需要另加 shell 侧持久化 marker 通道（见 §6.1），不能假装 capture 已经包含这些信息。

## 2. 已完成的实现

### 2.1 core / tmux runtime

1. `TerminalState` 保存带样式的 scrollback 行和稳定行 seq；resize、滚屏、清屏不会破坏行索引。
2. tmux attach capture 使用配置的历史行数，建立 Surface seed；capture 边界内的重复/丢失输出由 seed 覆盖规则处理。
3. live `%output` 作为唯一增量源进入同一个 VT；用户在历史位置时仍继续 feed，新输出累计为 native scrollback，不强制拉回底部。
4. viewport API 提供：当前 offset、最大 offset、按 seq 定位、严格 stale seq（淘汰后返回错误而不是误跳到底部）。
5. OSC 133 parser 记录 `A/B/C/D`：命令文本、退出码、稳定 seq；被 bounded scrollback 淘汰的 mark 会被删除。

对应提交：`692e6b7 feat(core): preserve VT history and command marks`。

### 2.2 macOS Surface / FFI

1. `MuxTerminalView` 只在首次 seed 时 reset；live/feed、滚轮、搜索、命令跳转均不 reset。
2. attach seed 进入 SwiftTerm 原生 scrollback，初始位置钉在尾部。
3. AppKit 的真实 `scrollWheel(with:)` 路径已接通；上划更新 core viewport，下划到 0 回到底部。
4. 用户停在历史时，新输出继续进入 VT，并显示 `↓ +N` / 回底按钮；不会覆盖当前历史视口。
5. 命令刻度按钮、`Cmd+Option+↑/↓` 前后跳转、末尾自动回 live 底部已接通。
6. pane 切换和 tab 切换都记录旧 pane 的 last-seen 稳定行；回来后显示“上次看到这里”，点击后跳回旧行并建立新的已读基线。
7. SwiftTerm history capacity 不再只依赖固定值：按 core 当前 `history_max_offset + rows` 动态扩容，只增不缩，保留用户当前视口。

对应提交：

- `8ea589a feat(macos): navigate native scrollback and command timeline`
- `5db0278 test(tmux): cover SSH history parity and raw OSC fixtures`

## 3. 施工顺序（完整实现清单）

### 阶段 A：先固定协议与状态机

1. 规定每个 pane 只有一个 Surface 状态：`Unseeded → Seeded/FollowsTail → UserScrolling`。
2. attach 时先收集 capture seed；seed 完成前的同批 `%output` 由 core seed 覆盖，不能重复 feed；seed 之后只消费增量。
3. 每次 live feed 前不调用 `resetToInitialState`、`visible_ansi` 或历史 dump；CUP/ED 风暴最多丢中间原始帧，不能把几何快照当 live。
4. core viewport 只作为索引/跳转事实源；macOS 的显示滚动始终由 SwiftTerm native scrollback 完成。

验收：同一个 pane 连续输出普通 shell、CUP 全屏程序、OSC 查询和 UTF-8 盒线时，屏幕不重复、不冻结、不白屏；`snapshotResetCount` 除首次 seed 外不增加。

### 阶段 B：历史滚动与回底

1. AppKit 真实滚轮、精确触控板 delta、整数行滚轮都归一到 SwiftTerm `scrollUp/scrollDown`。
2. native scroll callback 反算 core offset；offset 只能钳制在 `[0, history_max_offset]`。
3. 上划后保持用户视口；live 行继续 feed，累计换行数并更新 `↓ +N`。
4. 下划到末尾、点击回底按钮、命令时间线到末尾都执行同一 `scrollToLatest` 路径，并把 core viewport 设为 0。
5. tab/pane 切换只隐藏/显示已存在 Surface，不销毁已 seed 的 VT；真正关闭 pane 才释放视图。
6. history capacity 由 core 查询动态扩容；不得因配置大于 10,000 而截断，也不得在历史浏览期间缩容。

验收：attach 前 40 行离屏 token 可以鼠标上划看到；停在历史期间追加 live 行后，历史画面不被覆盖；下划到底部能看到最新 token；配置 100,000 行时 native scrollback 仍能覆盖 core 返回的范围。

实现约束：

- `[scrollback].lines` 是 core 的单一配置源。macOS 仍使用的 deprecated FFI
  `muxterm_new` / `muxterm_new_connect` / `muxterm_workspace_open` 会在 core
  读取该配置，并同时设置 tmux `capture-pane -S -N` 与 Workspace/PaneBuf 的
  scrollback 上限；不会出现 capture 有历史而索引面只保留固定 10,000 行的分叉。
- 新的 `WorkspaceSpec::with_scrollback_lines` 和 Linux 异步 SSH 收编路径也使用
  同一上限。PaneBuf 的 viewport setter 在 core 内钳制到真实可用范围，前端传入
  过大的 offset 只能落在顶部，不能伪造 stale 位置。
- native capacity 只增不缩；core 配置超过 10,000 行时，容量回归会用超过默认上限
  的历史行验证，不以“固定 10,000 足够”作为通过条件。

### 阶段 C：last-seen 与搜索定位

1. pane 离开可见 Surface（切 pane 或切 tab）时记录 `paneLatestLineSeq`。
2. 回到 pane 后，如果 latest seq 前进，调用 `paneViewportOffsetForSeq` 生成跳转目标；seq 已淘汰时隐藏按钮，绝不能把 stale 目标当 offset 0。
3. 点击 last-seen：先跳到旧行，再把当前尾部设为新的已读基线，避免下一轮 poll 立即重复显示按钮；后续新输出重新触发。
4. 搜索仍由 core index 返回 seq，macOS 只滚 native viewport + SwiftTerm `findNext`，不重播历史帧。

验收：切到另一个 tab/pane 后旧 pane 继续输出，切回能看到 last-seen；点击后能看到旧行且按钮消失；历史淘汰后不误跳底。

### 阶段 D：OSC 133 命令时间线

1. core 只接受合法 A/B/C/D 序列，D 的退出码按完整整数解析。
2. FFI JSON 返回 `seq/command/exit_code/history_offset`；已淘汰的 mark 保留记录时必须返回 `history_offset: null`，macOS 不允许跳转。
3. macOS 过滤掉没有退出码或没有可用 offset 的 mark；成功/失败按钮只展示可定位刻度。
4. `Cmd+Option+↑`：首次选当前尾部最近命令，后续向前；`Cmd+Option+↓` 向后，超过末尾清游标并回 live 底部。
5. 普通 ↑/↓、Cmd+↑/↓、PageUp/PageDown 继续交给 shell/TUI，不抢输入语义。

验收：attach 后运行至少一条成功和一条失败 OSC 133 命令；真实 `NSEvent` 快捷键能前后跳转；最后一次向下跳转 viewport 为 0；命令按钮 tooltip 含完整命令和退出码。

### 阶段 E：SSH 与隔离回归

1. 所有 tmux 测试使用唯一 `tmux -L muxterm-test-*` socket；清理同一 socket，绝不碰默认 server。
2. 本地 tmux 与 loopback SSH tmux 使用同一历史/seed/scrollback/command-mark 合同。
3. macOS AppKit 测试必须覆盖真实滚轮事件、真实 Cmd+Option+Arrow 事件、tab 切换 last-seen，而不是只调用内部 helper。
4. 运行顺序：core 单测 → FFI 合同 → Swift Chrome 单测 → macOS AppKit E2E → 全量门禁。

## 4. 已纳入的回归测试

| 测试 | 覆盖 |
|---|---|
| `HistoryE2ETests` | attach 离屏历史、原生滚轮、live feed、CUP 末帧、回底 |
| `CommandTimelineE2ETests` | OSC 133 成功/失败、真实 Cmd+Option+↑/↓、末尾回底 |
| `LastSeenE2ETests` | 切 tab 离开、旧 pane 继续输出、切回 last-seen、点击定位 |
| `AgentRenderE2ETests` | seed once、历史位置继续 feed、真实 AppKit scroll wheel、主题/光标 |
| `ChromeTests` | KeyBindings、scroll policy、feed policy、查询应答门禁 |
| core workspace capacity tests | 配置 scrollback 超过 10,000 行仍保留完整历史；viewport offset 有界钳制 |
| core OSC 133 parser tests | 合法 B→C→D 补退出码；孤立/重复 D 不污染上一条刻度 |
| `FlatChromeTests` | stale last-seen seq 清除旧跳转目标；没有新行时不显示按钮 |
| SSH history parity | loopback SSH 与本地 tmux 的历史/viewport 合同 |

## 5. 验收门禁

```bash
# 仓库根目录
cargo fmt --all -- --check
cargo check --no-default-features --features ffi
cargo test --no-default-features --lib
cargo clippy --all-targets -- -D warnings
git diff --check

# macOS
cd src/platform/macos
swift test --disable-swift-testing --filter HistoryE2ETests
swift test --disable-swift-testing --filter CommandTimelineE2ETests
swift test --disable-swift-testing --filter LastSeenE2ETests
swift test --disable-swift-testing
```

每次真实 tmux 验证都必须显式带隔离 `-L`；禁止 `tmux kill-server`、`kill-session`、`kill-pane` 作用于默认 server。

## 6. 当前已知边界与后续可选阶段

### 6.1 attach 前命令刻度

这是 tmux 控制协议的数据边界，不是 macOS UI bug：OSC 133 已经被 pane 的终端消费，`capture-pane` 只返回处理后的网格。因此当前实现保证“attach 后命令时间线完整”，但不声称能重建 attach 前的所有命令刻度。

如果产品必须支持 attach 前命令时间线，另开独立变更：

1. 提供 muxterm shell integration（zsh/bash/fish）hook，在 OSC 133 A/C/D 同步写入用户目录下的按 session/pane 划分的 marker 日志；
2. marker 使用版本、session/pane、单调序列、开始/结束时间、命令文本、退出码和可选输出行 seq；
3. core attach 时读取并校验 marker，和 capture 建立 seq/行偏移映射；映射失败的旧刻度只能展示为不可定位，不能误跳 live；
4. SSH 场景通过远端文件/side-channel 获取 marker，失败时降级为“attach 后时间线”；
5. 追加 shell 退出、权限、并发写入、日志轮转和隐私测试。

该阶段不能通过 `capture-pane` 或前端再次解析历史文本“推测”实现，否则会再次引入错误跳转和重复渲染。

## 7. 完成定义

- [x] macOS attach seed 一次，live `%output` 持续 feed。
- [x] 鼠标/触控板上划能显示 attach 前历史；下划/回底能显示最新输出。
- [x] 历史位置不会冻结 live 输出，也不会用 dump/reset 覆盖用户视口。
- [x] `[scrollback].lines` 已贯通 tmux capture、Workspace/PaneBuf、legacy FFI 和 Linux SSH 收编路径；native capacity 按 core 范围只增不缩。
- [x] core 与 macOS 支持稳定行 seq、last-seen、命令刻度和 stale 防护（包括 stale last-seen 目标清除、孤立 OSC 133 D 隔离）。
- [x] 本地 tmux、loopback SSH、真实 AppKit 滚轮和真实快捷键回归已覆盖。
- [ ] 若要求 attach 前完整命令刻度，完成 §6.1 的 shell marker side-channel（独立后续阶段）。
