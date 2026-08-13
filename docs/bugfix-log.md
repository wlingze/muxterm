# Muxterm Bug 修复日志（2026-08-13）

> 全部按「测试 → 修改 → 测试通过 → commit」完成；日志素材保留在
> `tests/logs/`（本地、不跟踪）。每个 commit 都有对应回归测试。

## 1. 新 pane 光标落到最底部

- 原因：attach 时 `capture-pane` 快照带尾部空白行 + 补 CRLF，新终端光标被推到 pane 底部。
- 测试：`capture_pane_strips_trailing_blank_rows_so_cursor_stays_at_prompt`
- Commit：`9b37429 fix(tmux): keep capture-pane cursor on the prompt row`

## 2. 中文输入法候选态删除键误删原文

- 原因：窗口级 Backspace 直接发 DEL，绕过 IME marked text。
- 测试：无独立单测（AppKit 行为），逻辑修复。
- Commit：`0712152 fix(macos): let the IME handle Backspace during marked text`

## 3. local tmux create session ENOENT（Finder 启动 .app）

- 原因：GUI 无 `/opt/homebrew/bin` PATH；`~/` 未展开。
- 测试：`create_local_tmux_session_works_without_homebrew_in_path`、
  `resolve_tmux_falls_back_when_path_has_no_homebrew`、`expand_config_value ~`
- Commit：`a64aa63` / `f3d6153`

## 4. GUI 日志双写 / 可开多实例

- 原因：CLI 启动器与 app 双写同一 log-file；`open` 可再开实例。
- 验证：lsof 确认单写、单持有者；`LSMultipleInstancesProhibited`。
- Commit：`07cab50 fix(macos): avoid GUI log double-write and duplicate app instances`

## 5. 远程 statusbar 不渲染

- 原因：SSH 参数未 shell 转义，`#{status-left}` 被远端 shell 吃掉；alias 误放 socket 字段。
- 测试：`remote_tmux_command_shell_quotes_formats`、`StatusQueryTargetTests`、
  `fetch_ssh_snapshot_reads_remote_status`（ryzen 实测通过）
- Commit：`55c1cac` / `ed5e78f` / `6fb35c3`

## 6. 本地 statusbar 显示默认绿

- 原因：只读 `status-style`（默认 `bg=green`），忽略 `status-bg colour234`。
- 测试：`effective_status_style_overrides_default_green_with_status_bg_fg`、
  `merge_session_global_prefers_session_values`
- Commit：`55c1cac fix(tmux): match real status bar colors and fix remote status fetch`

## 7. 主题切换后 codex 输入框白/黑不一致

- 原因：只给当前 tab 重报颜色，后台 tab 沿用旧 OSC 10/11 代答。
- 测试：`report_all_pane_colours_covers_every_tab`
- Commit：`8bae09f fix(macos): re-report theme colours to every pane after switch/connect`

## 8. 默认深色（应浅色）+ 命令面板无主题

- 测试：`MuxtermThemeTests`、`MuxtermTerminalColorsTests`、KeyBindings 测试
- Commit：`85e0018 feat(macos): add light/dark theme system with palette switch`

## 9. 默认状态行不该出现

- 原因：无脑常显。改为仅 `--debug` 显示摘要行。
- Commit：`4e0498a feat(macos): show the pane summary line only with --debug`

## 10. statusbar 模式不能在命令面板切换

- 测试：`StatusBarModeTests`
- Commit：`93b0c5c feat(macos): remove default status line and add statusbar mode to palette`

## 11. 输入/粘贴一直换行（1721）

- 原因：codex 一帧拆成多个 `%output`，逐事件喂 SwiftTerm 中间态漂移。
- 测试：`testPasteRedrawFramesDoNotShiftInputRow`（1721 提取）
- Commit：`a9168f9 fix(macos): coalesce pane feeds so agent redraws stay on the input row`

## 12. cursor agent 输入不可见/上滚无内容（1745）

- 原因：模型默认宽度与 pane 实际宽度不一致，长行折行后 erase-up 行数对不上。
- 测试：`testRedrawStableWhenModelWidthMatchesPaneWidth`（1745 提取）
- Commit：`114637c fix(macos): resize terminal model to pane size before feeding output`

## 13. 后台 tab 刷新影响前台 htop

- 原因：任意 window 的 layout-change 都重建/重绘当前 tab。
- 测试：`testBackgroundTabEventsDoNotReloadCurrentUI`
- Commit：`1911098 fix(macos): ignore background-tab layout events so htop stays stable`

## 14. 老 tmux 不支持 `refresh-client -r`

- 原因：tmux < 3.2 每次颜色上报刷 `unknown flag -r`。
- 测试：`parse_tmux_version_handles_beta_suffix`、`colour_report_requires_tmux_3_2`
- Commit：`16987e7 fix(tmux): skip colour reports on tmux < 3.2`

## 15. Pane 全屏（Cmd+Enter / 命令面板）

- Core：`TogglePaneFullscreen` → tmux `resize-pane -Z`；本地 shell 前端布局全屏。
- 测试：`zoom_pane_toggles_fullscreen`、Cmd+Enter 键位、`PaneFullscreenPolicyTests`、
  `pane_fullscreen_zoom_toggles_real_tmux`（真实 tmux flag 1→0）
- Commit：`a399f7a` / `fb11f51`

## 16. 持续刷新渲染漂移（1740）

- 原因：codex 141 列帧 + 9 行 erase-up，pane 只有 93 列，折行后逐帧漂移。
- 测试：`testRedrawStableWhenFrameWidthMatchesModel1740`（1740 提取）
- Commit：`4b9f766 test(macos): add 1740-derived redraw stability regression`

## 其他

- `340a942`：`tests/logs/` 显式 gitignore，日志永不跟踪/删除。
- `526cc8f`：真实日志 README 索引。
- `110c9cf`：split 回归测试 pane id 修正（新 session 首 pane 是 %0）。
- `6104197` / `76c41c9`：statusbar justify/模式、字体缩放与 TOML 配置。

## 待验证（需要用户实测/新日志）

- Cmd-T 新建 tab 卡住：core 3s 超时回归不卡，疑似前端，缺复现日志。
- 1740/1745 新 build 复测。
- Pane 全屏、主题/statusbar 模式、日志双写实测。
