# Dogfood 0907 问题与做法（重做手册）

> 用途：重构落地后，**不要 rebase 这条分支**。以本文为需求，在新树上按条重做。
> 来源：Grok 会话 `01a07ac0-4e2c-79f1-9848-946a024a2fa6`（续 Codex `01a07aaf-e33e-7ae3-85ef-0e4d31101758`）。
> 参考实现：`feature/dogfood-0907` @ `96c5076`（相对当时 `main` `fbd66f2`，11 个提交）。
> 整理：2026-09-09（Asia/Shanghai）。§16 起为同日补记的待做（尚未实现）。
> 1.0 收口顺序见 `docs/PRODUCT-1.0-REMAINING.md`。

本文记的是**使用中碰到的问题和你想要的行为**，不是 git 考古。旧路径只作对照；新树上文件搬家后，按行为验收。

---

## 0. 怎么用这份文档

1. 新分支从重构后的 `main` 拉出。
2. 按第 12 节顺序一条条做；每条可独立验证再提交。§16 起是后续待做（含 §23–25 前端聚合格子），重构后另开提交，不要和已落地的 1–11 混在一起。
3. 对照 `feature/dogfood-0907` 只看「意图和测试」，不要整文件拷贝。
4. 本机配置清理（`~/.config/muxterm` 里不用的文件）不要写进仓库。
5. GUI 问能力继续用 `support()`，禁止 `if runtime == "herdr"`。
6. 测试：macOS 用 `cargo test --no-default-features --features ffi --lib`；tmux 只用隔离 `-L`。

快捷键平台差：Linux `Ctrl+Alt+N`，macOS `Cmd+Ctrl+N`。`Cmd+0` 仍是重置字体，workspace 最后一个是 `Cmd+Ctrl+0`。

---

## 1. 你当时要的全貌

来自 Codex 原话（会话未做完，Grok 接着做）：

1. Workspace 太多才提醒：上限 **20**（soft reminder，不自动踢）。
2. 切换 Workspace：`1–9` 按侧栏**打开顺序**，`0` **永远是最后一个**（不是最近使用）。
3. 侧栏可以拖动 Workspace **改编号顺序**。
4. 复制必须走系统剪贴板：**覆盖**，不能追加。复制 `1` 再复制 `2`，粘贴应是 `2`，不是 `12`。
5. 设置里改 Project 总报错；配置本身也有错误。要能 Apply。
6. 没有配置文件就创建默认；Herdr 默认创建会报错，不要踩 SSH 禁止创建的契约。
7. 新 Tab 用 **当前 Workspace 的 project path**；同一 Tab 里 split pane **继承当前 pane cwd**。

```text
workspace path = /tmp/a
  新 tab        → /tmp/a
  在 /tmp/a/b 里 split pane → /tmp/a/b
  再新 tab      → 仍是 /tmp/a
现在（当时）永远用当前 pane cwd，不对。
```

Grok 开场额外要求：点侧栏 WORKSPACES / AGENTS / COMMANDS **闪烁，而且点了跳不过去**。

后面 dogfood 又补了一批：重复激活、新 Tab 仍走 pane cwd、Agents 被压扁且不能调高度、状态永远 Working、Cmd-R Attention 写成 `local ~`、渐进选词、命令刻度轨、回底胶囊、构建 `zsh: killed`、侧栏消失、NSSplitView 刷屏、诊断要进 `--log-file`、Attention 整行染色。

产品手感对照：`docs/PRODUCT-VISION-STRATEGIC-REVIEW.md` §2.15.4。

---

## 2. Workspace 上限 20、快捷键 1–9 / 0、侧栏改序

**问题**

- 开几个 Workspace 就提示太多（本机 `max_slots = 5`）。
- 只能切前 5 个；没有「永远最后一个」。
- 侧栏顺序不能拖，编号跟手感对不上。

**期望**

- 提醒上限 20，不自动淘汰。
- `1–9` = 侧栏固定打开顺序；`0` = 最后一个，哪怕总共不到 10 个。
- 拖 Workspace 行之后，编号跟着变。

**做法（当时）**

- 动作：`switch_workspace_1..9`、`switch_workspace_last`（index `0` 表示 last）。
- Core `WorkspacePool::reorder(&[WorkspaceId])`：按传入顺序重写 `opened_order`，没出现的格子保持相对顺序接在后面。
- Linux：keymap + `switch_workspace_n`；`n==0` 取 `list().last()`；侧栏编号前 9 个。
- macOS：`WorkspaceShortcutIndex.maximum = 9`；`Cmd+Ctrl+0..9`；拖拽 pasteboard `muxterm.sidebar.workspace`。
- 本机 `~/.config/muxterm/config.toml` 的 `max_slots` 改成 20。代码默认 / `config.example.toml` 已是 20。

**重做注意**

- macOS 当时改的是 GUI `WarmConnectionSlot.openedOrder`，**没有走 Core FFI**。Linux 走 `pool.reorder`。新树如果只有一个池，两边都应打到 Core。
- Linux 当时只接了 reorder 回调，GTK 拖拽 UI 不一定有。
- `Cmd+0` 不得变成切 Workspace。

**验收**

- `Action::switch_workspace_index`：9 → `Some(9)`，last → `Some(0)`。
- `WorkspacePool::reorder` 单测：`[a,b,c]` → `[c,a,b]`，编号 1 变成 c。
- macOS：`KeyBindingsTests.testCmdCtrlDigitsSwitchWorkspacesIncludingLast`。
- 手测：拖第 3 个到最前，`Ctrl+Alt+1` / `Cmd+Ctrl+1` 切到它；`0` 永远切最后一个。

---

## 3. 复制追加、写不进系统剪贴板

**问题**

复制 `1`，粘贴 `1`；再复制 `2`，粘贴变成 `12`。感觉没走系统剪贴板。

**期望**

和系统 Edit 菜单一样：每次复制**替换**当前剪贴板。

**原因**

`NSPasteboard.writeObjects` 在不清空时再挂一个 item，粘贴方拼在一起。

**做法**

`clearContents()` + `setString`；失败再 `setData`。不要先清再 `writeObjects`。

**验收**

复制 `1` 再复制 `2`，系统粘贴板和 pane 内粘贴都是 `2`。

---

## 4. 设置 Apply 报错、缺省配置、Herdr 创建

**问题**

- 改 Project 点 Apply 总失败。
- 没有配置文件时应该自动创建。
- 默认创建 Herdr（尤其 SSH）会报错。

**期望**

同一 runtime/transport/session 的 Project 只留一条；缺文件就写出默认 TOML；不要在 SSH Herdr 上偷偷 `CreateIfMissing`。

**原因**

旧 compact uniqueID（`4:tmux|3:ssh|…`）和 `name@transport` 并存，Apply 撞 `uniqueID`。

**做法**

- `ConfigDocument::normalize_projects()`：按 `logical_key`（runtime + transport + target + session + socket）去重，保留第一条。
- macOS `QuickConnectStore.projectJSON` 按 `uniqueID` 去重。
- `SettingsService::open`：文件不存在则 `create_dir_all` + 原子写入默认文档。
- Herdr SSH 无匹配禁止创建，这是已有契约，不要为「缺省配置」去开。

**本机（不要进仓库）**

当时删过 `~/.config/muxterm` 里不用的 `quickconnect.toml` / `quickconnect.json` / `ui-render.log`。

**验收**

- `from_toml_dedupes_equivalent_projects`
- `open_creates_missing_config_file`
- `testProjectJSONDropsDuplicateUniqueIDs`
- 设置页 Apply 同一 SSH project 不再报 duplicate。

---

## 5. 新 Tab 用 Workspace 路径，split 继承 pane cwd

**问题**

新 Tab 一直跟当前 pane 的 `#{pane_current_path}`。你怀疑是 tmux 默认、改不了。

**期望**

改得了。显式 workdir > Workspace project path > 才退回 pane cwd。SSH 的 `~/…` **不要在 Mac 上展开**，交给远端 tmux。

**原因（第一轮没做完）**

Workspace 层填了 `Task::NewTab.workdir`，但 tmux attach 后 Runtime 自己没有记住 Catalog path，execute 仍走 pane cwd。SSH 若本地 `expand_config_value`，会把 `/Users/…` 送到 ryzen。

**做法**

- `Workspace::apply_workspace_defaults`：`NewTab` 且 workdir 为空 → `resolved_target.canonical.path`，再退 `WorkspaceId.path`。`SplitPane` 不动。
- `TmuxRuntime.workspace_workdir`：Catalog `TmuxDriver::open` 和 attach 都 `set_workspace_workdir`。
- `new_tab_directory`：显式 > workspace path；本地展开 `~`，SSH 原样保留。
- Herdr：`tab.create` / `pane.split` 带 `cwd`（有 workdir 时）。

**验收**

- `new_tab_without_workdir_uses_workspace_project_path`
- `new_tab_without_explicit_dir_uses_workspace_project_path`（命令含 `-c`，不含 `#{pane_current_path}`）
- `ssh_new_tab_keeps_remote_tilde_path`（`-c "~/Developer/self/muxterm"`）
- 手测：project `/tmp/a`，cwd 进 `/tmp/a/b` 后 New Tab 仍在 `/tmp/a`，split 在 `/tmp/a/b`。

---

## 6. 侧栏闪烁、点了跳不过去、点一下整页重激活

**问题**

- 点 WORKSPACES / AGENTS / COMMANDS 行闪一下，跳不过去。
- 已经在当前 Workspace 再点 agent/command，日志里 `workspace activation begin/ready` 大约每秒重复一遍（`elapsed_ms≈24`），布局被重置。
- 60Hz 刷新把整张 agent/command 表 `reloadData`，选中行丢掉。

**期望**

点哪跳哪；当前 Workspace 内跳 pane **不要**走完整 activate。刷新时行身份不变就地改文字/颜色。

**做法**

- Agent/command 身份不变则 `configure*Cell`，变了才 `reloadData`。
- `pendingSidebarTarget`：点击立刻钉住目标，刷新时不要被 live 焦点抢回去；workspace+pane 对上再清。
- 跳转走 `routePanelJump`（和 Attention 同一条），不要 `activate` + `performWhenForegroundReady`。
- `activate(slot:)`：`activeKey == slot.key && bridge === slot.bridge` 直接 return。

**验收**

- `WorkspaceSidebarE2ETests`：agent/command 刷新不增加 reload count。
- 手测：当前 Workspace 点 agent，日志不再刷 activation；目标 pane 成为前台。

---

## 7. Agents 被压扁、分区不能上下拖

**问题**

点一下之后 Agents 突然变矮；WORKSPACES / AGENTS / COMMANDS 高度不能拖。

**期望**

四个分区可独立拖高矮。折叠只留 26pt 标题；展开最小约 80pt。

**原因**

`NSStackView` `fillEqually` 均分高度，一点击/折叠就把 Agents 压没。

**做法**

改成竖直 `NSSplitView`（`isVertical = false`），`autosaveName` 用新名字（当时最终是 `.v2`，旧名会把 0 高度写回去）。折叠区 26pt，展开可拖。

后面侧栏消失、启动警告，都是这条改动的续发，见第 10 节。

**验收**

折叠后标题还在；拖 Agents/Commands 分隔条能改高度；重启后比例还在（autosave 非 0）。

---

## 8. 侧栏永远 Working；Cmd-R Attention 显示 `local ~`

**问题**

- Agents 状态卡在 Working（structured 生命周期不更新）。
- 侧栏已经是「名字 · agent · Tab N」，Cmd-R Attention 却是 `local ~` 这种。

**期望**

状态跟注意力生命周期走。Attention 的内容和侧栏同一套：工作区名、状态、agent、Tab N。不要用本机 `~` 当标题。

**做法**

- `WorkspaceSidebarProjection`：同一 pane 有 attention 快照时，**覆盖** structured `Working`。
- FFI `muxterm_attention_snapshot` 带上 `name`、`transport`、`path`（`resolved_target` 再退 `WorkspaceId.path`）。
- Attention 行从侧栏 chrome 取 workspace / agent / tab。

**验收**

- `testAgentStatusFollowsAttentionWhenStructuredStaysWorking`：structured Working + attention Done → `Done · Codex`。
- Attention 行出现 `muxterm` / `Working` / `Codex` / `Tab 2`，不含 `local`、`~`。

**后续外观**见第 11 节（色块 + 两行）。当时第一轮是单行 `workspace · status · agent · Tab N`。

---

## 9. 终端手感：渐进选词、命令刻度、回底

**问题 / 期望（你的原话）**

> 双击选中一个单词，再次双击选更大的词。`cd a/b/c` 双击 `b` 再双击得到 `a/b/c`。滚动栏按命令状态红/绿。下拉跳到最后。位置和交互要舒服。文档里有想要的效果。

文档：`docs/PRODUCT-VISION-STRATEGIC-REVIEW.md` §2.15.4。

| 能力 | 产品要求 |
| --- | --- |
| 回底 | 鼠标滚上去后，右下角胶囊 `↓ 最新 · +N`；上滚即 scroll-lock，新输出不把人拽回去 |
| 命令刻度 | 滚动条一侧极窄轨（约 10px，悬停展开）；成功绿 / 失败红；点击跳命令；悬停出命令文本；宽度超过一个字符位算违规；跟随时可隐藏 |
| 选词 | 你额外要求**渐进扩大**，不要 iTerm 默认一次吞掉整段路径 |

旧 UI：右上角 ✓ / ✗ 按钮，不是轨。

**选词（第一轮 → 你当场改口径）**

第一轮两层：标识符 → 路径字符 `/\-_.~:@+%#`。

你后来说要以 `feature/dogfood-0907*` 为准，并且拖选用空格切（iTerm / Ghostty）：

```text
点 0907
  第 1 次  0907
  第 2 次  dogfood-0907*
  第 3 次  feature/dogfood-0907*
拖选：按空白分成整词，不要在 / 或 - 处切开
```

中间层扩不开就跳过（`a/b/c` 的 `b` 第二次直接到整段路径）。括号/引号仍交给 SwiftTerm 平衡匹配。TUI 鼠标模式不要动。

三层字符：

- identifier：字母数字下划线（含 CJK）
- word：`-.+%@#?*`
- path：`/\~:`

双击后若继续拖：`dragByTokens`，两端吸附到空白分隔的整词。

SwiftTerm 需要 `setSelectionRange` patch（`scripts/patch-swiftterm.sh`，标记 `MUXTERM_SET_SELECTION`）。

**命令轨 / 回底（当时实现要点）**

- `CommandMarkTick` 来自 OSC 133 / `pane_command_marks`；`exitCode == 0` 绿，非 0 红，没有码 pending。
- 折叠宽 8pt，悬停 14pt；offset 0（最新）靠下。
- 命中：距离近优先，同距离优先失败刻度。
- 点轨底部附近（约 8%）= 回底。
- 回底胶囊文案 `↓ 最新` / `↓ 最新 · +N`。
- macOS 滚动位置 ≥ 0.92 且未到 1.0 时向下吸附。
- Linux：`snap_history_to_latest`（距底 ≤ `page/6` 且正在向下滚才吸）；按钮同样改文案。Linux **没有**重做命令轨。

**验收**

- `ProgressiveWordSelectionTests`：`a/b/c`、`dogfood-0907*` 三层、拖过空格。
- `CommandMarkRailTests`、`JumpLatestCaption`。
- Linux `snap_history_to_latest_only_near_bottom_when_scrolling_down`。
- 手测：双击/三击分支名；拖选不被 `/` 切开；失败命令红点可点；滚上去出现胶囊，点了回尾且恢复跟随。

---

## 10. 构建 SIGKILL、侧栏消失、日志刷终端

### 10.1 `./build/macos/muxterm --help` → `zsh: killed`

**原因**

Apple Silicon 按 inode 缓存代码签名。`cp -f` 覆盖已有 Mach-O，内核用旧签名校验新内容 → SIGKILL。另外 `build-macos.sh` 先编 tui CLI 再编 ffi，ffi 把 `target/*/muxterm` 盖成没有 tui/gui 的二进制。

**做法**

- `install_executable`：拷到 `dst.new.$$` → `chmod +x` → ad-hoc `codesign --sign - --identifier dev.muxterm.cli` → `mv`。
- `smoke_cli_help`：装完必须能跑 `--help`（137 当失败）。
- `build-macos.sh`：**先 ffi 给 Swift，最后再编 tui CLI 并 install**。
- `build-cli.sh` / `build-tui.sh` 走同一套。

**验收**

`./scripts/build-macos.sh` 然后 `./build/macos/muxterm --help` 退出 0。脚本路径是 `scripts/` 不是 `script/`。

### 10.2 侧栏整栏没了

**现象**

日志：`arranged view frame` 全是高度 0，还有 `{0,-25}`，顺序和 arranged views 不一致。WORKSPACES / AGENTS / COMMANDS 看起来消失。

**原因**

分区 SplitView 在 0 尺寸时仍 `setPosition`，autosave 把四个 0 高 pane 写回去。`translatesAutoresizingMaskIntoConstraints = false` 和手排 frame 打架。

**做法**

抽出 `SidebarSectionSplitLayout`：

- 总高度不够（展开最小 80 × n + 折叠 26）→ **不要排**（`heights() == nil`）。
- 折叠固定 26pt；剩余高度分给展开区。
- autosave 换新名，丢掉 0 高度。
- TAMIC 打开，让 SplitView 自己管 subview。

### 10.3 启动仍警告 `{240, 0}` / 子视图 `{0, 0}`

**现象**

侧栏内容已经能看见，但启动还打：delegate `resizeSubviewsWithOldSize:` 后 outer edges 对不齐。Split bounds `{240, 0}`，四个 arranged frame `{0,0,0,0}`。

**原因**

高度为 0 时直接 return，宽度还停在 0，对不上 240。

**做法**

退化布局：`degenerateFrames(count, width)` → `{x:0,y:0,w:width,h:0}`。有真实高度后再走正常 heights。

**验收**

`SidebarSectionSplitLayoutTests`；`testZeroHeightSidebarKeepsArrangedSubviewWidth`。启动不再把侧栏排成全 0。

### 10.4 `--log-file` 时终端仍被刷

**期望**

```text
./build/macos/muxterm gui --debug --log-file test-….log
```

NSSplitView、IMK、`workspace activation begin/ready` 进那个文件，不要印在 TTY。

**做法**

- 有 `--log-file` 时 `freopen` stderr 到该文件（XCTest 进程不要做）。
- FFI `muxterm_log_message` → tracing `muxterm::ui`；header / CoreBridge / Swift `NSLog` 换这条。
- 无 `--log-file` 的 `--debug` 仍可走 stderr。

**验收**

`GuiLogFilePolicyTests`；`log_message_does_not_panic_without_subscriber`。手测：TTY 干净，日志文件里有 UI 行。

`error messaging the mach port for IMKCFRunLoopWakeUpReliable` 当时当作输入法噪音，没当 bug 修。

---

## 11. Attention 整行一种颜色

**问题**

Attention 整行绿或橙，和侧栏不像。

**期望**

和 Agents / Commands / Workspace 行一样：前面一块颜色，**文字用正常前景色**。每项两行，内容对齐侧栏。

```text
[色块]  muxterm
        Working · Codex · Tab 2
```

色：运行绿、未读完成/阻塞橙、已读灰。8pt 圆角色块，不要把 title/detail 染成 systemGreen/Orange。

**做法**

`AttentionRow.title` = 工作区名；`detail` = `status · agent · Tab N`；`indicator` 映射侧栏同一套。`AttentionRowCellView`：色块 + 两行 label。

**验收**

`NativePanelLayoutE2ETests`；`testAttentionRowUsesNormalTextColor`。手测：Cmd-R 打开，行结构与侧栏 Agents 同类，只有色块上色。

---

## 12. 建议重做顺序

按依赖，不要按旧 commit hash：

| 序 | 主题 | 主要落点 |
| --- | --- | --- |
| 1 | 构建安装 / SIGKILL | `scripts/build-*.sh` |
| 2 | 缺省配置、Project 去重、max_slots=20 | config / settings |
| 3 | 快捷键 1–9 / 0 + Core reorder | config + pool + 两端 GUI |
| 4 | NewTab cwd（含 SSH `~`） | Workspace + tmux + herdr |
| 5 | 系统剪贴板覆盖 | macOS TerminalView |
| 6 | 侧栏点选跳转、禁重复 activate、就地刷新 | macOS sidebar / MainWindow |
| 7 | 分区 SplitView + 0 高度退化布局 | sidebar layout |
| 8 | Attention 数据对齐侧栏 | FFI snapshot + projection |
| 9 | Attention 色块两行 | UnifiedPanel |
| 10 | `--log-file` 吃掉 UI stderr | logging FFI + AppDelegate |
| 11 | 渐进选词 + 命令轨 + 回底 | Chrome + TerminalView；Linux 至少回底吸附 |

---

## 13. 观察记录：远程校准「抢锁」（当时没改代码）

你问：远程校准在抢锁，是不是因为多个 client 连着同一个 tmux？并要求记到文件。当时只在对话里解释过，仓库里没有单独文档，记在这里。

**结论：不是 tmux 全局锁，也不是「好几个 Muxterm/iTerm 抢同一个 session」的主因。**

证据日志：`test-2026-0908-1051.log`（约 10:51 CST）。muxterm 激活 `elapsed_ms=3430`、`5263`。同时有 `pane output gap fenced`、`output-dropped` pause/capture、`initial pane seed … deadline`、交错 `select-window -t @61` / `@0`、`忽略其它 session 的 %session-window-changed`。

实际机制：

1. 每个 Workspace 自己的 `WarmConnectionSlot.bridgeLock`（NSLock）串行化这一路 FFI。校准 / `prepareForForeground` / `captureForegroundAuthority` 都要这把锁。
2. 输出缺口会 fence，然后对**当前这条** `-CC` 发 `refresh-client -A %N:pause` + `capture-pane`。SSH 上这一串是串行的，容易到数秒。
3. 侧栏连点（muxterm → legion → muxterm → …）会 generation++，作废上一次校准，再发一轮。`pendingForeground` 叠起来，看起来像抢锁。
4. ConnectionPool 通常是 **一个 Workspace 一条 `tmux -CC`**，连的是不同 session（muxterm / legion / yaklang）。日志里「忽略其它 session 的 window-changed」是故意丢的，不是死锁。
5. 真有另一个 client（iTerm 等）attach **同一个** session，会抢 current-window 和 client size，一般不会让 `bridgeLock` 卡 5 秒。

若重构后还要修：减少「已在前台」时的全量校准（和第 6 节重复 activate 是一类）；output-gap 的 pause/capture 不要和连点 SwitchTab 叠成队。不要把它理解成「先禁多 client」。

---

## 14. 对照：当时的提交（只作索引）

| Commit | 覆盖 |
| --- | --- |
| `5525cf1` feat(workspace): expand shortcuts, reorder, and tab cwd | §2 §4 §5 第一轮（Linux/Core；macOS 快捷键在下一笔） |
| `76e2934` fix(macos): replace clipboard contents and stabilize sidebar jumps | §2 macOS、§3、§6 |
| `64dd3e0` fix(tmux): create new tabs in the workspace project directory | §5 真正记住 Catalog path |
| `55e2c1c` fix(macos): stop repeat activation and align attention with sidebar | §6 后半、§7、§8 |
| `fc66609` feat(terminal): progressive select, command rail, and jump-latest | §9 第一轮 |
| `087e96d` fix(macos): stop SIGKILL after installing the CLI binary | §10.1 |
| `7056d9d` fix(macos): restore sidebar section layout after NSSplitView collapse | §10.2 |
| `54a1794` fix(macos): silence NSSplitView warning on zero-height sidebar | §10.3 |
| `7e6bb31` fix(macos): send GUI diagnostics to --log-file instead of the terminal | §10.4 |
| `6a83061` fix(terminal): grow double-click by hyphen then path, drag by spaces | §9 按 `dogfood-0907*` 改口径 |
| `96c5076` fix(macos): give Attention rows a color block and two-line layout | §11 |

新文件（旧树，搬家后自行对）：

- `ProgressiveWordSelection.swift` + Tests
- `CommandMarkRail.swift` + `CommandMarkRailView.swift` + Tests
- `SidebarSectionSplitLayout.swift` + Tests
- `GuiLogFilePolicy.swift` + Tests

---

## 15. 明确不做 / 未做完

- 没改 TUI 鼠标选词。
- Linux 命令刻度轨没按 macOS 重做（Linux 原先就有自己的 mark 按钮）。
- 「上次看到这里」未读分割线（§2.15.4 主点子）这轮没做。
- `[` / `]` 命令跳转产品里有，这轮没作为完成项。
- IMK mach port 警告没修。
- §13 抢锁只做了诊断，没有代码改动。
- 本机删除的 `~/.config/muxterm` 废文件不要在重做时再碰别人的机器。
- §16 起是 2026-09-09 补记的待做：只记录和设计，这轮不改代码。

---

## 16. 待做：Cmd-W 关 pane，Settings 开着时关 Settings

**问题**

Cmd-W 现在不像关 pane。Settings 开着时按 Cmd-W，期望关掉设置窗，实际会动到主会话/窗口。

**期望**

- 主窗口：Cmd-W = 关**当前 pane**，一个个关。
- Settings（以及其它覆盖层）是 key window 时：Cmd-W = 只关这个窗，不动 tmux/Herdr 会话。
- 关光一个 tab 里的 pane 之后，再 Cmd-W 才轮到下一个 tab / 最后才碰到窗口。不要一按就把整个 Muxterm 窗口拆掉。

**现在为什么不对**

- `KeyBindings` 里 `Cmd+W` → `.closeWindow`。
- File 菜单「关闭窗口」绑了 `keyEquivalent: "w"`，`target` 写死 `MainWindowController.closeActiveWindow`。
- 「关闭 Pane」菜单项的快捷键是空的。
- `closeActivePane()` 已经存在，也注释了「唯一 pane 时关 pane 会触发后端关 window」。
- 主窗口 `routeMonitoredKeyEvent` 对「事件不在主窗口」会放行，但菜单项仍有显式 target，Settings 是 key 时 Cmd-W 还是打到主窗口关会话。

**设计（重做时按这个做）**

关闭分层，从上到下只做一层：

1. Settings / 独立面板是 key window → `performClose` 那个窗。菜单项不要写死主窗口 target，让 AppKit 把 Cmd-W 送给 key window。
2. UnifiedPanel / Command Palette 可见 → dismiss 面板。
3. 否则 `closeActivePane()`。
4. 真正关 Muxterm 窗口用菜单「关闭窗口」，建议 `Cmd+Shift+W`，不要占 Cmd-W。

验收：Settings 开着 Cmd-W 只关设置；多 pane 时每次少一个 pane；最后一个 pane 的行为跟现在的 `close_pane` 契约走，不要先 `closeSessionWindow`。

---

## 17. 待做：Grok / Codex 列表里往下一滚就到底

**问题**

Grok、Codex 这类列表：往上翻历史没问题，往下稍微一滚就跳到最后（实时尾 / 输入行），没法在列表里慢慢往回看。

**原因（高度可疑，待手测确认）**

§9 回底吸附：macOS `JumpLatestCaption.shouldSnapToLatest` 用的是 **整段 scrollback 比例** `scrollPosition >= 0.92`。一万行历史的最后 8% 还有八百行。对话列表通常就堆在缓冲区底部，人只往上翻了几十行，比例仍 ≥ 0.92，任何一次向下滚轮都会 `scrollToLatest()`。

alternate screen 路径已经跳过这段吸附。若 Grok/Codex **没用** alt screen、走的是普通 scrollback，就会中招。

Linux 用的是剩余像素 `page/6`（大约几行），比 0.92 比例克制，但 agent TUI 仍可能嫌近。

**设计**

- 吸附改成「还剩几行」而不是「高度百分比」。建议只剩 1～2 行、并且这一下是在向下滚，才吸到尾。
- Agent TUI（attention `processIsAgent` / structured agent / alt screen）默认不吸附，把滚动交给 TUI 自己。
- 回底胶囊还在：人要回尾就点胶囊或快捷键，不要靠「往下滚一下」。
- 不要为了修这个把「上滚 scroll-lock、新输出不把人拽回去」拆掉。

验收：在 Codex/Grok 会话列表里往上翻一屏，再往下滚若干格，视口应跟着走，不得跳到输入行。只有已经贴着最后一两行时向下滚，才允许回尾。

---

## 18. 待做：双击后拖选用 word（空白分词）

**问题 / 期望**

双击选中之后再拖，应该按 **word** 扩选：空白切开的整词，不要在 `/`、`-` 上断开。这样才好用，也接近 iTerm / Ghostty。

**和 §9 的关系**

`6a83061` 已经按这个口径写了 `tokenSpan` / `dragByTokens`，双击后的 `mouseDragged` 会吸到空白整词。重做时必须保留。

若手感仍不对，优先查：

- `wordDragPivot` 只在渐进双击成功时设置，普通双击或 tmux 鼠标模式可能没走进这条路径；
- `super.mouseDragged` 仍按字符切，后写的 word 吸附被下一拍覆盖；
- 跨行拖选没有按 word 对齐。

验收：`feature/dogfood-0907*` 双击 `0907` 再拖到 `feature`，选区是整段 `feature/dogfood-0907*`，不会停在 `/` 或 `-`。

---

## 19. 待做：Agents / Attention 分不清 local 和 ryzen

**问题**

两个都叫 `muxterm` 的工作区，一个 local、一个 ryzen。Agents 和 Attention 里没有 local / ryzen 这类标记，列表一长就分不出点哪条。

**期望**

Agents、Attention 和 Workspace 行同一套身份：能看见 **哪台机器**。`local` 也要标，不要只在 SSH 时才出现。

**现在**

- Workspace 行已经有 `transport`（`local` / ssh alias 如 `ryzen`）。
- Agents 标题只用 `workspace.name`。
- Attention 标题也是工作区名；§11 之后第二行是 `Working · Codex · Tab 2`，没有 transport。

**设计**

两行都带机器，不要把 transport 藏进 tooltip：

```text
[色块]  muxterm · ryzen
        Working · Codex · Tab 2

[色块]  muxterm · local
        Working · Codex · Tab 2
```

- 标题：`{workspaceName} · {transport}`。`transport` 用侧栏同一套：SSH 用 alias（`ryzen`），本地用 `local`。
- Agents / Commands / Attention 三处同一规则。
- 搜索过滤要能搜到 `ryzen` / `local`。

验收：同时打开 local muxterm 和 ryzen muxterm，Agents 与 Cmd-R 里两条不能只靠「muxterm」撞名。

---

## 20. 待做：键盘切 Agent，以及 Herdr title

### 20.1 快捷键：现在只能鼠标点 Agent

**问题**

Agent 多了以后，习惯上想像切 Workspace / Tab 那样用键盘切，现在只能鼠标。

现有数字键（要保持）：

| 键 | 作用 |
| --- | --- |
| `Cmd+1..9` | 当前 Workspace 的 Tab |
| `Cmd+Ctrl+1..9` / `0` | 侧栏打开顺序的 Workspace |
| `Cmd+[` / `]` | 当前 Tab 里的 pane |

**期望（原话）**

- 参考 `Cmd+Ctrl+1..4` 切 Workspace、`Cmd+1..3` 切 Tab。
- Agent 也要有一套 `Cmd-xxx-1234`。
- 或者 **Ctrl 切快速面板里当前可见的那一列**（Attention / Quick Connect / Search）。

**设计（建议，重做时按此落地）**

三层，互不抢键：

1. **全局切 Agent**：`Cmd+Option+1..9` / `Cmd+Option+0`  
   对象是侧栏 **Agents 列表的固定顺序**（现在的展示序：未读完成优先，然后 running）。`0` 永远是列表最后一条。  
   动作 = 现在点 Agents 行：跨 Workspace 也要跳（`activateSidebarTarget` / `routePanelJump`）。  
   `Cmd+Option+↑/↓` 仍是命令刻度跳转，不要占用。

2. **面板内数字**：UnifiedPanel（Cmd-R Attention、Cmd-P Quick Connect、搜索）是 key 且焦点不在输入框、或带 `Ctrl` 时，`Ctrl+1..9` / `Ctrl+0` 激活**当前列表第 n 行**。  
   终端有焦点时 Ctrl+数字不要抢，避免和程序自己的 Ctrl 冲突；只在面板 key 时生效。  
   这就是「Ctrl 切快速面板里的东西」。

3. **不要**用 `Cmd+Shift+1..9`（和系统/Tab 容易混），也不要动 `Cmd+Ctrl`（Workspace）。

Agent 没有独立编号时，侧栏 Agents 行可以像 Workspace 那样显示 1–9 / 0。面板行同样可以在左侧显示数字。

验收：两个 Workspace 里各有 Codex，`Cmd+Option+1` 跳到 Agents 第一行对应 pane；打开 Attention 后 `Ctrl+2` 跳第二行。

### 20.2 Agent 能不能拿到 title？Herdr 有没有？

**结论：Herdr 已经有 title，产品层没用好。**

Herdr pane/agent 记录上已有：

- `display_agent` / `display_name`：给人看的 agent 名（如 `Codex`、`Pi reviewer`）
- `title`：任务/会话标题（契约测试里是 `"Implement runtime events"`）
- `terminal_title` / `terminal_title_stripped`：终端标题
- `agent` / `kind`：内部名

Linux `agent_title()` 和 macOS `agentName` 都是 **第一个非空**：displayName → name/kind → title。有 `display_name` 时，Herdr 的任务 title 被丢掉，列表里全是「Codex」，几个会话分不开。

tmux 没有 Herdr 这种结构化 title 时，只能退到进程名 / pane title，不强行编造。

**设计**

拆成两个字段，不要互相覆盖：

- `agentName`：`display_name` / kind / 进程名 → `Codex`
- `sessionTitle`：Herdr `title`（空则省略）

第二行：

```text
Working · Codex · Implement runtime events · Tab 2
```

没有 Herdr title 时仍是 `Working · Codex · Tab 2`。Attention 同样。标题太长就尾部截断，hover / accessibility 给全文。

验收：Herdr 契约里 `title = "Implement runtime events"` 的 pane，Agents 和 Attention 必须能看到这段文字，不能只剩 Codex。

### 20.3 tmux 上的 Codex title：能拿到，但不是 Herdr 那种字段

**结论**

- Herdr：`PaneAgentInfo.title` 是协议字段，已经解析（契约测试 `"Implement runtime events"`）。
- tmux：没有同等的 agent JSON。Codex **不会**给 tmux 一条 `title=`。能用的是它写到终端里的 **OSC 0**（`ESC ] 0 ; … BEL`），tmux 会收成 `#{pane_title}`。
- 当前 Muxterm **没有**把这条接到 Agents/Attention。tmux Runtime 不填 `PaneAgentInfo`；`list-panes` 连 `#{pane_title}` 都不订；VT 虽然解析 OSC 0 进 `TerminalState.title`，但没有导出成产品 title。

**Codex 实际发什么（2026-09 核对 openai/codex）**

新版 Codex TUI 会周期性 `set_terminal_title`，默认片段是 `activity` + `thread-name` + `project-name`（用户可用 `/title` 改）。未命名 thread 在开始生成前会省略名字。旧版 Codex 曾经几乎不设有意义的 OSC title。

所以 tmux 上能看到的常常是一整串，例如 `⠋ Working | Implement runtime events | muxterm`，不是干净的 Herdr `title`。

**重做时怎么取（不要去刮 TUI 画面）**

1. 订 `#{pane_title}`（以及 `%output` 里若漏下来的 OSC 0，已经在 `TerminalState.title`）。
2. 进程是 Codex/Grok 时，把 pane title 拆开：去掉 spinner / `Working` / `codex` 前缀，剩下的 thread-name 才进 `sessionTitle`。
3. 画面正则（`›` 提示、状态行）只作没有 OSC 时的弱后备，不要当主路径。
4. Grok 等同理：它如果写 OSC 0/`#{pane_title}` 就能用；没有结构化协议就不要假装有 Herdr title。

验收：tmux 里两个 Codex pane、thread 名不同，Agents/Attention 第二行能区分；没有 OSC 的旧 Codex 允许只有 `Codex`，不要编造。

---

## 21. 待做对照（尚未实现，不要从旧分支抄成「已完成」）

| 节 | 主题 | 重做落点 |
| --- | --- | --- |
| 16 | Cmd-W 关 pane / 关 Settings | 菜单 target、KeyBindings、覆盖层 |
| 17 | 列表向下滚不该整页回尾 | 吸附改成剩余行数；agent TUI 关闭吸附 |
| 18 | 双击后按 word 拖选 | 保留并验证 `dragByTokens` |
| 19 | Agents/Attention 标 local/ryzen | 标题带 transport |
| 20.1 | 键盘切 Agent / 面板 Ctrl+数字 | `Cmd+Option+1..9`；面板 `Ctrl+1..9` |
| 20.2 | Herdr session title | `agentName` 与 `title` 分列显示 |
| 20.3 | tmux Codex title | `#{pane_title}` / OSC 0，不要刮 TUI 画面 |
| 22 | 回底胶囊闪烁 | 迟滞 + 状态不变不重绘；和 §17 一起修 |
| 23 | Shells / Agents 前端聚合 | GUI 投影格子，不是 Core Workspace |
| 24 | Cmd-K 进 Shells | 靠近 Cmd-N；Cmd-N 留给以后新 Window |
| 25 | 从 shell 提升 tmux/herdr | 难在认出 session；不 hook 命令 |
| 26 | 聚合怎么排；Tab 状态 / 设置页 TODO；总规划 | 只记不设计 |

---

## 22. 待做：回底按钮（↓ 最新）有时一直闪

**问题**

右下角「↓ 最新」胶囊有时会不停闪：出现、消失、再出现。人没点它，也没在来回滚。

**和 §17 的关系**

§17 是「往下一滚就到底」。闪烁常常是同一套阈值在打架：刚离开底部按钮出来，吸附又把它吸回去，按钮再藏起来；下一拍输出或滚轮又离开 0.999，按钮再出来。

**现在怎么决定显示**

- 显示条件大致是 core `viewport offset > 0`，或 SwiftTerm `scrollPosition < 0.999`。
- 隐藏：`isAtLatest()` 用 `scrollPosition >= 0.999`。
- 向下吸附：`>= 0.92` 就 `scrollToLatest()`（§9 / §17）。
- 60Hz 刷 snapshot 时每次都调 `setJumpLatestVisible`，里面会改 title、背景色、`needsLayout = true`，即使可见性和文案没变。
- 新输出时如果不在底部，未读行数 +N 会改标题（`↓ 最新` ↔ `↓ 最新 · +N`），宽度跟着跳。
- 命令轨悬停会改胶囊的 trailing，位置也会动一下。

**设计**

1. **迟滞**：离开底部超过约 2 行才显示；只有真正贴尾（最后 1 行内）才隐藏。禁止 0.92 吸附和 0.999 显示共用一条线。
2. **状态不变不重绘**：visible / unseen 没变就不要 `isHidden`、不要改 title、不要 `needsLayout`。
3. **显示切换要去抖**：连续 150–300ms 都「该显示」才显示，都「该藏」才藏。不要每帧翻转。
4. Agent TUI（Codex/Grok / alt screen）默认不显示这颗按钮（滚动交给 TUI，见 §17）。
5. 未读计数可以节流，不要每个 `\n` 都改一遍胶囊文案。

验收：贴着实时尾部时胶囊保持隐藏、不闪。上翻一屏后稳定显示。新输出只更新 `+N`，不要闪灭。Agent 会话列表里不应闪这颗按钮。

---

## 23. 待做：Shells / Agents 都是前端聚合格子

**问题**

- 现在打开全是项目 Workspace。默认 1 号常常没用，关掉；偶尔又想敲两句本地命令。别的终端能随手开一个 local，Muxterm 没有对等入口。
- Agent 散落在各个项目的 tab 里。侧栏能点跳，不能「进一个格子，把它们当 tab 用」。

**原则（重构后也不许破）**

Core 仍是 **一个 Workspace = 一个已 attach 的 Runtime**。Window 只是体现。  
**Shells 和 Agents 都不是新 Runtime，也不是把 pane 搬进一个假 Workspace。** 只是前端把已有格子聚合成两个看起来像 Workspace 的槽，切过去的手感和普通 Workspace 一样（侧栏一行、数字键、同一套激活）。

```text
Shells（聚合：本机 shell + 各机器 shell）
Agents（聚合：各项目里的 agent pane / tab）
普通项目 Workspace  ← 真 Runtime（tmux / herdr）
```

切来切去都在 Workspaces 列表里。

### 23.1 Shells

- 列表里一个固定槽，名字 `Shells`（或同等）。不要启动时塞一个空项目占 1 号。
- **Tab 1 永远本机 local shell。** 其余 tab = 一台机器一个 **ShellRuntime**（SSH 上的普通 shell，不是自动 attach 那边的 tmux）。
- 实现：每个机器一个真的 ShellRuntime Workspace；GUI 把它们画成这一个槽的 tabs。关 remote tab = shutdown 那条 shell，不要 detach 用户 tmux。
- 已在 Shells 里再「新 local」= 再开一个本机 shell tab。

### 23.2 Agents

- 同样一个固定槽。Tab 列表 = 当前所有 agent pane（每个 agent 一页）。
- Surface **借用源 Workspace 的同一个 pane**，不搬 PTY、不合成一个 tmux session。
- 只画当前 tab。在 Agents 里关 tab ≠ 杀 agent，只离开这个视图。
- 不要在投影里 split / new tab 造假拓扑。
- 标题带 `工作区 · 机器 · agent · title`（§19 / §20），两个 muxterm 才分得开。

验收：`Cmd-K` 进 Shells，Tab 1 能敲本地命令；侧栏能进 Agents，切 tab 等于跳到那个 agent 现场，源项目 Workspace 里的 tab 还在。

---

## 24. 待做：进 Shells 用 Cmd-K（不要用 Cmd-N）

**期望**

- **`Cmd-K`**：切到 Shells 聚合槽。已经在里面则保证 Tab 1（local）可用；需要新 local 时在 Shells 里加 tab。
- **`Cmd-N` 先别占。** 以后可能有多 Window，「新 Window」会用离 N 近的键；K 和 N 够近，日用也顺。
- 项目 Workspace 里新 tmux window 仍是 **`Cmd-T`**。
- Muxterm **不要**把 Cmd-K 做成终端清屏（清屏仍是 pane 里 Ctrl-L）。当前 `KeyBindings` 没有 `k`，键是空的。

Linux 对齐：同一个「进 Shells」动作，键位用配置里的绑定，不要写死成 Ctrl-K 清屏。

验收：项目 Workspace 里按 Cmd-K 到 Shells/local；Cmd-T 仍在当前项目新 tab。

---

## 25. 待做：从 shell 提升里面的 tmux / herdr（难在认出是谁）

**问题**

以前在普通 shell 里 `cd` 再 `twork`，就多一个工作区。希望在 Shells 的 local 或某台机器的 shell 里已经跑了 `tmux` / `herdr` 之后，右键或按钮：**推出当前这个客户端，新建/回到普通 shell，再 attach 一个项目 Workspace**。Transport 跟当前这个 shell 走（local 就是 local，SSH 就是 SSH）。

**不要** hook `tmux` / `herdr` 命令本身（套娃、误伤、SSH 更糟）。

**要的行为**

1. 用户点「作为 Workspace 打开」（pane 右键 / 标题栏，不要全局误触快捷键）。
2. 认出当前 pane 前台是 tmux 客户端或 herdr 客户端。
3. **推出：** detach 这个客户端，pane 回到普通 shell；对端 session **继续跑**。
4. Catalog attach 那个 session，新项目 Workspace 进列表并切过去。
5. 之后和 Shells / 普通 Workspace 用数字键来回切。

**为什么比 23 难（重做时要单独估）**

Shells 里的 pane 是 **ShellRuntime 的 PTY**。Muxterm 不是这个内层 tmux 的 `-CC` 客户端，看不到 `%session-changed`。必须从「这个 PTY 的孩子进程」反推 session 身份。

| 情况 | 要拿到什么 | 难点 |
| --- | --- | --- |
| 本机，前台是 `tmux` | socket（`-L`）+ session 名 | 读 pane 前台 argv / `TMUX` 环境（`tmux display` 或 `/proc/<pid>/environ`）。默认 server 不要误 attach 成用户主 server 的错 session。 |
| 本机，前台是 `herdr` | named session / socket | argv 或环境里的名字；禁止无名字的 `herdr server stop`。 |
| SSH 上的 shell tab | 同上，但是 **远端** pid | 不能拿远端 pid 查本机 `/proc`。要经现有 SSH alias 问远端，或只解析标题/环境，失败就报错别造空 Workspace。 |
| 只是普通 `vim` / `htop` | 无 | 按钮应不可用或点了说明「当前不是 tmux/herdr」。 |
| 已经在 Muxterm 的 TmuxRuntime 里 | 无 | 不要对正在 -CC attach 的 pane 再「提升」一次。 |

失败必须可见：认不出、detach 失败、对端没有可 attach 的 control 通道，都留在原 shell，不要开空格子。

**不要在重构前的这棵树上实现。** 只记需求。新树有稳定的 ShellRuntime + Catalog attach 后再做；探测逻辑用 `support()`，禁止 `if runtime == "herdr"`。

验收：Shells/local 里手动 `tmux new -s demo` 后点提升 → 原 pane 回到 shell，新 Workspace attach 到 `demo`；SSH 机器 tab 里同样走 SSH attach。普通进程点提升无事发生。

---

## 26. 这三条怎么样；还缺哪几口（2026-09-09）

### 判断

三条都对，而且都是 **Client 聚合**，没有和 Herdr/Superlogical 抢 Runtime。不要做成 Core 新层。

| 条 | 值不值 | 难度 | 何时 |
| --- | --- | --- | --- |
| **Shells + Cmd-K** | 最高。就是现在关掉的那个废 1 号，和别的终端 Cmd-N 的洞。没有它，Muxterm 不像一台终端。 | 中。多台机器 = 多个 ShellRuntime，GUI 合成 tabs。 | 第三波日用里 **先做**。 |
| **Agents 聚合槽** | 高。B 的列表是发现；这个槽是进去干活。agent 一多，点侧栏会烦。 | 中偏高。Surface 借用、焦点/resize、和源 Workspace 同时开着。只画当前 tab，别一屏拼十个。 | B 列表能跳之后。 |
| **提升 muxer** | 方向对，是 `twork` 的里面那半。 | **最高。** 难在从 Shell PTY 反推内层 session（尤其 SSH）。认错会 attach 到别人的 tmux。 | **后做。** 先有 Shells；过渡期在 shell 里 detach 再 QuickConnect / `muxterm ~/proj` 也能过。 |

`Cmd-K` 进 Shells、`Cmd-N` 留给以后的 Window，这个分工对。

### 其它日用（2026-09-09 口径）

1. **上一工作区 / 两格来回** — 有点怪，问题不大。语义就是刚打开过的那两个互相切，不是 LRU 排行榜。优先级低，有空再做。
2. **合盖重连** — 可以做，但不急。现在基本不关 Muxterm，日用够用。
3. **Tab / Pane 要有状态；Tab 要更好看** — **TODO，先不设计。** 现在 tab/pane 几乎没状态。至少要能看出 working / blocked / done（汇总自 pane，和 Attention 同一套词）。好看是另一笔 TODO，重构后单独开，本文件不画稿。
4. **远端 OSC 52** — 不做优先。在 Muxterm 里直接复制就行。
5. **Cmd-N 新 Window** — 后面可能要做。键留着，现在一扇窗。
6. **设置页** — **要好好做。** 现在有窗，但不是完成项。快捷键、主题、池上限、投递/转发之类都要能在这里改，TOML 仍是唯一事实源。

点通知落到 pane = B 闭环的一部分，跟 3 的 tab/pane 状态是同一条状态管道。

**不要**再加：手机、账号、自己做 mux、hook `tmux` 命令、Core 里假 Workspace、仓库分组（列表不长先不做）。

### 规划（重构落地之后，按这张表）

现在这棵树 **只记不写代码**。新 `main` 上按行为重做，不要 rebase 本分支硬解冲突。

```text
0. 迁 0907 已落地行为（§2–§11、§16–§22 里能独立验证的）
1. B 闭环：peek → 一行答复、红点口径、工作区汇总、机器标记、title
2. Tab/Pane 状态（TODO，无设计稿）+ 设置页收口
3. Shells + Cmd-K；然后 Agents 聚合槽
4. 地标（未读线 / 刻度 / 回底不闪）+ 搜索跳到坐标
5. 图投递、SSH 端口提醒后转发、Cmd-W、选词、URL
6. 后做：提升 muxer、两格来回、新 Window、Tab 视觉
```

没有第六条产品线。Herdr/Superlogical 继续当 Runtime/管道观察，Muxterm 停在窗。
