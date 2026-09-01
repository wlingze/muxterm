# Muxterm Config Contract

> 状态：已实现契约（`config_version = 1`）。本文描述的字段、Schema/Manifest、事务、FFI/CLI 和 Linux/macOS 设置窗口均已落地；macOS 侧仍需在 macOS runner 上编译与 XCTest。
>
> 本文是 Muxterm 配置系统的权威文档。代码、CLI、FFI、Linux GTK 和 macOS AppKit
> 都必须以本文的字段语义和事务行为为准；其他架构文档只保留摘要和链接。

## 1. 设计目标

Muxterm 的配置语义属于 Core。平台层只负责把 Core 返回的 Schema、Settings
Manifest 和值渲染为原生控件，然后把用户操作提交回 Core。

```text
TOML files -> SettingsService -> resolved Config + JSON Schema + Manifest
                                      |
                         JSON/FFI or direct Core API
                                      |
                    GTK4/libadwaita  |  AppKit  |  CLI/TUI
```

配置系统必须满足：

- 一个配置事实源：`config.toml`；Project、快捷键、字体和主题不再分散保存。
- 默认值、校验、迁移、事务和变更通知只在 `src/core/config/` 定义。
- GUI 编辑使用草稿事务：编辑期间不写盘，Apply 才原子提交，Cancel 可回滚预览。
- 外部编辑、CLI 和多个设置窗口并发时使用字段级三方合并，不静默覆盖。
- 正式配置分组严格拒绝未知字段；只有 `extensions.<vendor>` 可以保存扩展数据。
- 配置错误必须保留文件现场，并报告路径、行、列、字段和修复建议。

## 2. 文件布局和优先级

默认目录为 `$XDG_CONFIG_HOME/muxterm`；没有 `XDG_CONFIG_HOME` 时使用
`~/.config/muxterm`。macOS 也使用该路径，以保证 CLI、Rust Core 和原生前端共享
同一事实源。

| 路径 | 用途 | 是否运行时事实源 |
| --- | --- | --- |
| `~/.config/muxterm/config.toml` | 全局设置、Projects、快捷键 | 是 |
| `~/.config/muxterm/themes/<name>.toml` | 用户主题 | 是 |
| `configs/themes/{black,white}.toml` | 随应用发布的内置主题 | 只读默认值 |
| `~/.config/muxterm/quickconnect.toml` | 旧版本 Project 文件，迁移输入/备份 | 否 |
| `~/.config/muxterm/preferences.toml` | 旧 Linux 运行期偏好，仅用于一次迁移 | 否 |
| macOS UserDefaults | 旧 macOS 运行期偏好，仅用于一次迁移 | 否 |

解析优先级从低到高为：

1. Core 内置默认值。
2. `config.toml` 中用户明确写出的值。
3. `[platform.linux]` 或 `[platform.macos]` 中当前平台专属值。
4. 设置窗口当前事务的内存草稿（只在预览期间存在）。
5. 当前进程的显式 CLI/runtime 参数（只影响本次运行，不自动落盘）。

Project 只覆盖一次 Workspace 启动所需的 runtime、transport 和启动参数；它不能
覆盖全局字体、主题、快捷键或布局。

## 3. `ConfigDocument` 字段目录

`ConfigDocument` 是完整、已补齐默认值的 Rust 类型。磁盘文件可以是稀疏的；
`SettingsService` 同时保存原始 TOML AST，提交单个字段时尽量保留注释和未修改的顺序。

### 3.1 顶层

```toml
config_version = 1

[font]
[theme]
[statusbar]
[pool]
[tmux]
[ssh]
[scrollback]
[attention]
[ui]
[pane]
[behavior]
[platform.linux]
[platform.macos]

[[projects]]

[shortcuts]
[[shortcuts.overrides]]

[extensions]
```

正式分组采用 `deny_unknown_fields`。未知数据必须放在
`[extensions.<vendor>]`，并由对应厂商自行解释。

### 3.2 外观和终端

| 路径 | 类型 | 默认值 | 生效 |
| --- | --- | --- | --- |
| `font.family` | string | `JetBrains Mono` | 立即预览 |
| `font.size` | number，9–72 | `13.0` | 立即预览 |
| `font.fallback` | string array | `[]` | 立即预览 |
| `theme.name` | string | `system` | 立即预览 |
| `theme.light` | string | `white` | 立即预览 |
| `theme.dark` | string | `black` | 立即预览 |
| `statusbar.mode` | `tmux`/`theme` | `tmux` | 立即预览 |
| `scrollback.lines` | integer，100–1,000,000 | `10000` | 新建 Pane |
| `pool.max_slots` | integer，≥1 | `20` | 新建/切换 Workspace |

`theme.name = "system"` 根据系统外观在 `theme.light` 和 `theme.dark` 之间选择；
填写其他主题名时固定使用该主题。首期只提供 Muxterm 自有 TOML 主题格式，不导入
其他终端的主题文件。

字体选择遵循：用户选中的可用字体、应用捆绑的 JetBrains Mono、系统 monospace。
缺少用户字体只产生 warning，不修改用户配置，也不安装系统字体。

`pool.max_slots` 是 warm Workspace 的软提醒阈值，不是硬上限。超过阈值时连接池保留全部
Workspace，界面只列出最久未使用的后台 Workspace，让用户选择性关闭；“全部保留”不会触发
静默 LRU 淘汰。TTL、内存压力和用户明确关闭仍可按各自策略回收资源。

### 3.3 Runtime 和行为

现有运行时字段保留原名，便于迁移：

| 分组 | 字段 |
| --- | --- |
| `pool` | `max_slots` |
| `tmux` | `auto_mouse`、`default_session`、`socket` |
| `ssh` | `host`、`port`、`user`、`key_path` |
| `pane` | `default_command`、`workdir` |
| `behavior` | `on_last_pane_exit`、`on_program_exit_abnormal` |
| `attention` | `enabled`、`blocked_regex`、`debounce_ms` |
| `ui` | `tab_bar_position`、`tab_bar_height`、`show_title_bar`、`borderless` |

`tmux`、`herdr`、`shell` 必须通过 Catalog 的 runtime descriptor 和 capability
描述。platform 不直接检查 runtime 字符串，也不拼接 tmux 或 Herdr 命令。

## 4. Project / Workspace 模型

Project 是可重复使用的 Workspace 启动规格，不是产品层 Session：

```toml
[[projects]]
id = "muxterm"
name = "Muxterm"
path = "~/Developer/self/muxterm"
command = ["$SHELL", "-l"]
env = { RUST_LOG = "info" }

[projects.runtime]
id = "tmux"
session = "muxterm"
socket = ""

[projects.transport]
id = "local"
target = ""
```

Project 的稳定 `id` 在重命名时保持不变。`runtime` 和 `transport` 的 options 由
Core descriptor 提供 Schema；首期实现 shell、tmux、Herdr、local 和 SSH。

Recent 连接是运行时池派生数据，不落盘。旧 `quickconnect.toml` 会由 Core 读取并
合并到主文档；当前实现保留原文件作为可恢复备份，不会在迁移过程中静默删除。
其中的 `socket`、`session`、command 和 env 均映射到 `ProjectDocument`，重复 `id`
时主文档优先。

## 5. 快捷键

快捷键按 Action ID 保存，不直接保存 GTK 或 AppKit 类型：

```toml
[shortcuts]
preset = "qwerty"
primary_key = "auto"

[[shortcuts.overrides]]
action = "quick_connect"
bindings = [{ key = "KeyP", modifiers = ["primary"] }]
```

- `primary_key = "auto"` 在 Linux 解析为 Alt，在 macOS 解析为 Command。
- `key` 使用稳定物理键码；QWERTY 和 Colemak preset 保持相同物理键位意图。
- override 替换对应 Action 的 preset 绑定；空数组表示禁用；删除 override 恢复默认。
- 同一作用域内的重复 chord 是错误；跨作用域或系统保留组合是 warning。
- 菜单、命令面板和快捷键共享同一 Action Dispatcher，因此显示名称和执行行为一致。

Core 提供 Action Catalog：Action ID、标题 key、帮助 key、作用域、可用平台、
是否允许重复触发和默认 bindings。前端只渲染和派发，不自行维护另一份动作表。

## 6. ThemeDocument 和字体

主题文件版本化且只使用 Muxterm 格式：

```toml
theme_version = 1
name = "black"

[terminal]
foreground = "#e6e6e6"
background = "#0b0b0b"
cursor = "#ffffff"
cursor_text = "#0b0b0b"
selection_foreground = "#0b0b0b"
selection_background = "#d8d8d8"
ansi = ["#000000", "#cc0000", "#00aa00", "#aa5500", "#0000cc", "#aa00aa", "#00aaaa", "#aaaaaa",
        "#555555", "#ff0000", "#00ff00", "#ffff00", "#5555ff", "#ff55ff", "#55ffff", "#ffffff"]

[chrome]
surface = "#0b0b0b"
surface_alt = "#151515"
text = "#f5f5f5"
muted_text = "#9b9b9b"
border = "#303030"
accent = "#ffffff"
```

Linux 用 Fontconfig application-font API 注册 bundled font；macOS 用 CoreText
process-scope 注册。应用退出后不留下系统字体安装痕迹。

## 7. JSON Schema 和 Settings Manifest

Schema 使用 JSON Schema Draft 2020-12；Manifest 是独立的、平台无关的 UI 描述：

```json
{
  "manifest_version": 1,
  "schema_id": "muxterm.config.v1",
  "groups": [
    {
      "id": "appearance",
      "title_key": "settings.appearance",
      "fields": [
        {
          "path": "/font/family",
          "control": "font_picker",
          "apply": "immediate",
          "title_key": "settings.font.family",
          "description_key": "settings.font.family.description"
        }
      ]
    }
  ]
}
```

Schema 提供类型、枚举、范围、默认值和正式字段；Manifest 提供分组、顺序、控件、
i18n key、平台/capability、预览和生效模式。新增字段只需由 Core 发布二者，通用
设置页即可显示。

`SettingsBundle` 至少包含：`revision`、`values`、`defaults`、`schema`、`manifest`、
`diagnostics`、`action_catalog` 和 runtime/transport descriptors。

## 8. SettingsService 事务和热加载

Core API 的语义如下：

```rust
describe(platform) -> SettingsBundle
begin() -> SettingsTransaction
patch(transaction, json_patch) -> DraftResult
commit(transaction, expected_revision) -> CommitResult
cancel(transaction)
reload_from_disk() -> ReloadResult
subscribe() -> SettingsEvent
```

编辑过程：

1. `begin` 保存 baseline revision 和 resolved document。
2. `patch` 使用 RFC 6902 JSON Patch 修改 draft，并立即执行 Schema/语义校验。
3. 有效字段产生预览事件；不可立即应用的字段标记为“新建 Pane”或“重启需要”。
4. Cancel 发布 rollback；Apply 重新读取磁盘并执行三方合并。
5. 无冲突时使用原子 TOML 提交，递增 revision，发布 committed/reloaded 事件。
6. 有冲突时不写文件，返回每个冲突字段的 base/mine/disk 值。

三方合并规则：字段未被 draft 改动时采用 disk；disk 未改动时采用 draft；两边相同
时直接通过；两边不同且都偏离 baseline 时产生冲突。Project 按 `id`、快捷键按
Action ID 合并，不按数组下标合并。

文件监听必须防抖。外部文件解析失败时继续使用最后一份有效配置，设置窗口展示
错误路径和修复建议，绝不用空默认值覆盖磁盘文件。

## 9. FFI 和 CLI

FFI 函数使用现有字符串分配/释放约定，所有结果均返回统一 JSON envelope：

- `muxterm_config_describe_json`
- `muxterm_config_begin_json`
- `muxterm_config_patch_json`
- `muxterm_config_commit_json`
- `muxterm_config_cancel_json`
- `muxterm_config_reload_json`
- `muxterm_config_events_json`

CLI 语法：

```text
muxterm config path
muxterm config show [--resolved] [--format text|json|toml]
muxterm config schema [--manifest]
muxterm config validate [PATH]
muxterm config doctor
muxterm config get PATH
muxterm config set PATH VALUE [--string]
muxterm config unset PATH
muxterm config project list|add|edit|remove
muxterm config shortcut list|bind|unbind|reset|preset
```

CLI 写操作和 GUI 写操作必须调用同一个 SettingsService，不得另写 TOML parser。

## 10. 迁移和错误处理

`config_version` 缺失表示旧 v0。迁移步骤按顺序执行：

1. 映射当前 config 字段并将 `light`/`dark` 转为 `white`/`black`。
2. 将 `quickconnect.toml` 的 Project 合并进 `[[projects]]`。
3. 将 Linux preferences 和 macOS UserDefaults 的运行期覆盖合并进主配置。
4. 生成 `config_version = 1`，重新解析并校验。
5. 成功写入主文档后保留旧来源作为备份；任一步失败都不删除、不覆盖。

正式字段中的未知键、类型错误、范围错误、无效颜色、找不到主题、快捷键冲突和
Project runtime/transport 不支持都必须产生结构化诊断。诊断包含 error code、
JSON Pointer、文件、行、列、用户可读消息和修复建议。

## 11. 平台边界和 UI 要求

Core 负责：配置模型、默认值、Schema、Manifest、Action Catalog、Projects、主题、
字体语义、事务、迁移、持久化、合并、通知。

Linux/macOS 负责：原生窗口、原生字体选择器、平台字体枚举、Manifest 控件、键盘
事件转换、Accessibility、i18n 和视觉布局。

平台禁止：直接读取/写入 config 文件、复制 Config 默认值、维护 QuickConnect
持久化、解析 runtime 协议、实现第二套快捷键动作表。

## 12. 验收矩阵

- Core：默认值、Schema、Manifest、严格校验、Theme、Font fallback、Project、Shortcut。
- 存储：注释保留、稀疏配置、权限、原子失败、revision、迁移不丢字段。
- 事务：预览、Cancel、Apply、外部编辑、三方合并、冲突解决、热加载。
- CLI/FFI：命令完整、JSON/text 输出一致、错误 envelope、字符串释放、失败不改盘。
- GTK/Xvfb：动态表单、键盘导航、字体/主题预览、Project/Shortcut editor、脏关闭确认。
- macOS CI/XCTest：AppKit renderer、NSFontPanel、UserDefaults migration、Action Dispatcher。
- Runtime：Project 启动测试使用隔离 tmux socket 和命名 Herdr session，不触碰默认服务。
- 代码门禁：`cargo fmt`、`cargo check --features gtk`、`cargo test`、`cargo clippy -- -D warnings`。

## 13. 非目标

- 首期不直接兼容 Alacritty、Ghostty、iTerm2、Kitty 或其他主题格式。
- 不实现主题插件系统、主题市场或 YAML parser。
- 不允许 Project 覆盖全局外观，不实现布局 DSL。
- 不引入产品层 Session 或虚拟 Window。
- 不把 JetBrains Mono 安装到系统字体目录。
