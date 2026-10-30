# Muxterm 使用手册

面向使用者的安装、打开、切换、配置与排障指南。
产品结构见 [`WORKSPACE.md`](WORKSPACE.md)，Runtime 语义见 [`RUNTIME.md`](RUNTIME.md)，
配置契约见 [`CONFIG.md`](CONFIG.md)，施工/开发约定见 [`AGENTS.md`](../AGENTS.md)。

官网（中英文、黑白主题）：<https://wlingze.github.io/muxterm/>；中文入口
<https://wlingze.github.io/muxterm/zh/>。官网含真实 macOS 应用截图与完整文档参考；
本地预览与部署见 [`website/README.md`](../website/README.md)。

> **一句话：** Muxterm 是工作区工作台：把 tmux / Herdr / 本地 shell 收成同一套原生
> Workspace / Tab / Pane。`Workspace = Runtime(Transport) + path`；GUI 窗口只是某个
> Workspace 的体现。

---

## 1. 安装

### 1.1 系统要求

| 目标 | 要求 |
| --- | --- |
| macOS | Apple silicon，macOS 13+；DMG 内含 `Muxterm.app` |
| Linux 桌面 | x86_64，GTK4、vte4（GTK4 版）、OpenSSL、tmux 3.x |
| TUI / CLI | 能运行 Rust 二进制即可；tmux 可选，但 tmux runtime 需要 |
| 源码构建 | Rust 1.97.1（仓库 `rust-toolchain.toml` 固定）；macOS 还需 Xcode / Swift |

官方稳定版只发布 macOS ARM64 与 Linux GTK4 两个桌面包；TUI/CLI 从源码构建，
或在手动 dispatch 的 prerelease 中获取 Linux CLI 包。

### 1.2 macOS

1. 从 [GitHub Releases](https://github.com/wlingze/muxterm/releases/latest) 下载
   `muxterm-macos-arm64.dmg`。
2. 打开 DMG，把 `Muxterm.app` 拖到 `/Applications`。
3. 首次启动如果被 Gatekeeper 拦截，打开 **系统设置 → 隐私与安全性**，允许该应用。
   当前发布包使用 ad-hoc 签名，未经过 Apple 公证。

也可以把 CLI 安装到 PATH：`cargo install --path .`（在仓库根目录执行）。

### 1.3 Linux 桌面

安装依赖（Debian / Ubuntu）：

```bash
sudo apt-get install -y build-essential pkg-config \
  libgtk-4-dev libvte-2.91-gtk4-dev libssl-dev tmux
```

Arch：

```bash
sudo pacman -S rust gtk4 vte4 openssl pkgconf tmux
```

从 Releases 下载 `muxterm-gtk-linux-x86_64.tar.gz`，解压后直接运行：

```bash
tar xzf muxterm-gtk-linux-x86_64.tar.gz
./muxterm          # 默认 GUI
./muxterm tui      # TUI
./muxterm --help   # CLI 子命令
```

发布包在 ubuntu-latest 上构建。如果目标机器的 glibc 较旧，请从源码构建。

### 1.4 从源码构建

产物统一进入 `./build/<os>/`（`linux` / `macos` / `windows`），不跨 worktree 共享：

```bash
git clone https://github.com/wlingze/muxterm.git
cd muxterm

./scripts/build-linux.sh --release   # Linux GTK4
./scripts/build-tui.sh --release     # TUI / CLI
./scripts/build-macos.sh --release   # macOS .app
./scripts/build-cli.sh --release     # C ABI + CLI
```

`--release` 也可用环境变量 `PROFILE=release`。需要覆盖 cargo 目标目录时设置
`CARGO_TARGET_DIR`。

### 1.5 验证安装

```bash
muxterm --version
muxterm --help
```

`--help` 会列出全部 CLI 子命令；`muxterm gui --help` / `muxterm tui --help`
显示前端入口参数。

---

## 2. 第一次启动

### 2.1 入口

```bash
muxterm gui          # Linux → GTK4；macOS → .app
muxterm tui          # 终端里的 TUI
muxterm              # 无 subcommand = CLI 模式
muxterm --help
```

兼容期内 `--gtk` / `--tui` 仍然可用，但新脚本请用 subcommand。
macOS 也可以直接双击 `Muxterm.app`。

### 2.2 QuickConnect

按 `Alt+P`（Linux）或 `Cmd+P`（macOS）打开 QuickConnect。列表中的 **Candidate**
来自 Core Catalog：

- **Project**：`config.toml` 里持久化的项目；
- **Worktree**：Project 下的 git checkout；
- **Existing**：正在运行的 tmux / Herdr 会话；
- **Recent**：最近打开过的目标。

选中后 Core 解析成 `WorkspaceSpec` 并打开。前端不构造 spec，也不直接拼 tmux 命令。

### 2.3 本地 shell 与已有 tmux

- 选择本地 shell 目标：每个 pane 一条 PTY，不依赖 tmux。
- 选择已有 tmux 会话：Muxterm 通过 `tmux -CC` 控制模式 attach，把 tmux window
  映射为 Tab、tmux pane 映射为 Pane。
- 在命名 socket 上测试，不影响默认 tmux server：

```bash
tmux -L muxterm-test new-session -d -s demo
muxterm -L muxterm-test
```

清理也只能针对这个 socket：

```bash
tmux -L muxterm-test kill-server
```

> **安全红线：** 不要对用户默认 tmux server 执行 `kill-server` / `kill-session` /
> `kill-pane`。需要销毁会话时，必须显式带 `-L <你自己的 socket>`。

### 2.4 SSH

QuickConnect 里选择 SSH host，或输入新的 `user@host:port`。Muxterm 使用系统 SSH
配置（含 `~/.ssh/config`、跳板机设置），并把多个 pane 复用同一条 TargetConnection。

`config.toml` 也提供 `[ssh]` 默认值：

```toml
[ssh]
host = "example.com"
port = 22
user = "app"
key_path = "~/.ssh/id_ed25519"
```

SSH 远端 tmux 仍可用 `-CC` attach；Herdr 通过转发的 unix socket 连接。

### 2.5 端口感知与转发

SSH 工作区的状态栏会扫描目标机上的 TCP 监听端口。扫描通过现有 SSH 连接执行，
使用远端 `ss` 或 `lsof`，不在前端线程里跑。macOS 前端目前提供端口选择器：

- **转发**：把一个远端端口映射到本地端口，并在状态栏显示 `localhost:<port>`；
- **LAN 访问**：对已转发的端口显式开启 LAN 暴露，默认只绑本地；
- **打开**：直接打开本地或远端地址；
- **忽略 / 停止**：忽略一个发现的端口，或停止已有转发。

Core/FFI 已暴露端口扫描与转发接口；当前端口选择器 UI 在 macOS 前端。

### 2.6 Herdr

Herdr 是可选 runtime。启一个**命名** server：

```bash
herdr --session work server
```

然后在 QuickConnect 里选择该 server 的 workspace。一个 Herdr socket 共享一条
TargetConnection；每个 Herdr workspace 仍是池里独立的一格。

> **安全红线：** 不要执行无名字的 `herdr server stop`。测试或清理只操作自己创建的
> named session。

---

## 3. 核心概念

| 词 | 含义 |
| --- | --- |
| **Workspace** | 可连接实例：`Runtime(Transport) + path`。池里的一格。 |
| **Runtime** | `shell` / `tmux` / `herdr`。负责填 Tab/Pane、收字节、执行 Task。 |
| **Transport** | `local` / `ssh`。只决定字节怎么到对端，不是 Runtime。 |
| **Tab / Pane** | Workspace 内部拓扑；所有 Runtime 都必须给出同一套，包括没有 tmux 的 shell。 |
| **Project** | 持久化项目记录：路径、目标环境、默认 Runtime / Transport、模板。 |
| **Worktree** | Project 下的 git checkout；打开后是池里独立一格，带 provenance。 |
| **WorkspaceTemplate** | 新建会话时应用的 Tab / Pane 初始结构；attach 已有会话不套模板。 |
| **Window** | GUI 窗口，只是某个 Workspace 的体现；不是产品树节点。 |
| **Attention** | Activity lane 的聚合状态：working / blocked / done，供侧栏和面板显示。 |

`Runtime × Transport` 不是手写枚举。兼容性是
`runtime.channel_requirements() ⊆ transport.supported_channels()` 计算出来的：

| Runtime | local | ssh |
| --- | --- | --- |
| `shell` | 支持 | 支持 |
| `tmux` | 支持 | 支持（远端 tmux） |
| `herdr` | 支持 | 支持（socket 转发） |

---

## 4. 打开、切换与关闭

### 4.1 切换 Workspace

- 点击侧栏中的 Workspace；
- QuickConnect 选择 Candidate；
- `Ctrl+Alt+1..5`（默认）直达前五个 Workspace。

已打开的 Workspace 各有一棵常驻 Scene。切换只是换可见场景，**点击路径零 Core 调用、
零锁等待**，不会重连，也不存在 warm/cold slot。

### 4.2 Tab 与 Pane

| 操作 | 默认快捷键 |
| --- | --- |
| 新 Tab | `Alt+T` / `Cmd+T` |
| 水平分割 | `Alt+S` / `Cmd+D` |
| 垂直分割 | `Alt+V` / `Cmd+S` |
| 上一个 / 下一个 Pane | `Alt+[` / `Alt+]` |
| 第 N 个 Tab | `Alt+1..9` / `Cmd+1..9` |
| 最后一个 Tab | `Alt+0` / `Cmd+0` |
| Pane 全屏 | `Ctrl+Return` |
| 关闭 Pane | 命令面板 / 菜单中的 Close Pane |

Linux 侧仍保留兼容绑定：`Alt+D` = 垂直分割，`Alt+Shift+D` = 水平分割。
macOS 的 `Cmd+D` / `Cmd+S` 与 View 菜单一致；`Cmd+Shift+D` / `Cmd+Shift+S`
提供另一方向。

### 4.3 命令面板与搜索

- 命令面板：`Alt+Shift+P`（Linux）/ `Cmd+Shift+P`（macOS）。所有动作和快捷键共用
  同一 Action Dispatcher，面板显示的键位就是实际执行的键位。
- 搜索 Pane 历史：macOS `Cmd+F`；Linux 使用命令面板中的 **Search panes**。
  搜索由 Core Index 提供，不把结果重新编码成 ANSI 灌回 Surface。

### 4.4 Attention

侧栏的 **Attention** 汇总 Activity lane 的状态。某个 pane 中的 agent 正在等待输入、
命令退出或终端发出 BEL/通知时，会出现提醒；可以直接跳转到对应 Workspace/Pane，
也可以隐藏或静音某条提醒。前端只渲染 Core 的结果，不自己解析进程或屏幕。

### 4.5 粘贴图片

- macOS：复制图片后按 `Cmd+V`，或使用菜单粘贴。
- Linux：复制图片内容后按 `Ctrl+Shift+V`，或右键粘贴。

支持 PNG / TIFF，最大 20 MiB / 64 MP。Muxterm 把图片转成 PNG，上传到 Workspace
所在机器，再把文件路径插入终端（不自动回车）。SSH 上传可能耗时，界面会显示编码 /
上传 / 完成 / 失败状态。

---

## 5. 快捷键总表

`primary_key = "auto"` 时，Linux 解析为 `Alt`，macOS 解析为 `Command`。
下表按默认 `qwerty` preset 列出。

| 动作 | Linux | macOS |
| --- | --- | --- |
| QuickConnect | `Alt+P` | `Cmd+P` |
| 命令面板 | `Alt+Shift+P` | `Cmd+Shift+P` |
| 新 Tab | `Alt+T` | `Cmd+T` |
| 水平分割 | `Alt+S` | `Cmd+D` |
| 垂直分割 | `Alt+V` | `Cmd+S` |
| Pane 切换 | `Alt+[` / `Alt+]` | `Cmd+[` / `Cmd+]` |
| Tab 1–9 | `Alt+1..9` | `Cmd+1..9` |
| 最后一个 Tab | `Alt+0` | `Cmd+0` |
| Workspace 1–5 | `Ctrl+Alt+1..5` | `Ctrl+Alt+1..5` |
| 搜索 Pane | 命令面板 → Search panes | `Cmd+F` |
| 复制 | `Ctrl+Shift+C` | `Cmd+C` |
| 粘贴 | `Ctrl+Shift+V` | `Cmd+V` |
| 放大 / 缩小 / 重置字体 | `Ctrl++` / `Ctrl+-` / `Ctrl+0` | 同左 |
| 退出 | `Ctrl+Q` | `Cmd+Q` |

TUI 底部提示使用同一套核心绑定：`Alt+P` 面板、`Alt+T` 新 Tab、`Alt+S` 水平、
`Alt+V` 垂直、`Alt+[ ]` 切换 Pane、`Ctrl+Q` 退出。

Action ID 与完整默认绑定见 [`CONFIG.md`](CONFIG.md) §5 和
`src/core/config/shortcut.rs`。

---

## 6. 配置

### 6.1 文件位置

| 路径 | 用途 |
| --- | --- |
| `~/.config/muxterm/config.toml` | 全局设置、Projects、快捷键 |
| `~/.config/muxterm/themes/<name>.toml` | 用户主题 |
| `configs/themes/black.toml` / `white.toml` | 随应用发布的内置主题 |
| `~/.config/muxterm/quickconnect.toml` | 旧 Project 文件，仅迁移输入 |
| `~/.config/muxterm/preferences.toml` | 旧 Linux 偏好，仅迁移输入 |

macOS 也使用 `~/.config/muxterm`，保证 CLI、Core 和原生前端共享同一事实源。
配置严格校验，未知字段必须放进 `[extensions.<vendor>]`。

复制示例配置：

```bash
mkdir -p ~/.config/muxterm
cp configs/config.example.toml ~/.config/muxterm/config.toml
```

### 6.2 常用字段

```toml
config_version = 1

[font]
family = "JetBrains Mono"
size = 13.0
fallback = ["Noto Sans Mono", "monospace"]

[theme]
name = "system"     # system / black / white
light = "white"
dark = "black"

[statusbar]
mode = "tmux"       # tmux = 跟随 tmux；theme = 跟随 Muxterm 主题

[scrollback]
lines = 10000

[pool]
max_slots = 20      # 软阈值；超过只提醒，不自动淘汰

[tmux]
auto_mouse = true

[update]
auto_check = true
manifest_url = ""   # 空 = 内置 stable 通道
```

完整字段、类型、默认值和生效时机见 [`CONFIG.md`](CONFIG.md) §3。

### 6.3 快捷键覆盖

推荐使用 `[shortcuts.overrides]`，按 Action ID 覆盖，而不是复制整份默认表：

```toml
[shortcuts]
preset = "qwerty"
primary_key = "auto"

[[shortcuts.overrides]]
action = "quick_connect"
bindings = [{ key = "KeyP", modifiers = ["primary"] }]
```

空 `bindings = []` 表示禁用该 Action；删除 override 恢复默认。旧 `[[keybindings]]`
仍可读取并迁移，但新配置优先写 overrides。

### 6.4 Project / 模板

```toml
[[projects]]
id = "muxterm"
name = "Muxterm"
path = "~/Developer/self/muxterm"
command = ["$SHELL", "-l"]
template = "workstation"

[projects.runtime]
id = "tmux"
session = "muxterm"
socket = ""

[projects.transport]
id = "local"
target = ""

[[templates]]
name = "workstation"

[[templates.tabs]]
name = "Monitor"
active = true
```

Project 的 `id` 是稳定 slug，重命名时保持不变。模板只在**新建**会话时应用；
attach 已有 tmux/Herdr 会话以远端拓扑为准。

### 6.5 通过 CLI 检查配置

```bash
muxterm config path                 # 配置文件路径
muxterm config show                 # 磁盘上的原始 TOML
muxterm config show --resolved      # 补齐默认值后的值
muxterm config validate             # 校验当前配置
muxterm config validate ./config.toml
muxterm config doctor               # 路径 / schema / 校验 / 主题检查
muxterm config get font.size
muxterm config set font.size 14
```

`config set` 走 Core 的字段级补丁与原子提交；GUI 设置窗口同样使用草稿事务
（Apply 才写盘，Cancel 回滚预览）。

---

## 7. CLI

无 subcommand 即 CLI 模式。常用命令：

```bash
# 列表
muxterm list-workspaces
muxterm list-tabs
muxterm list-panes

# 在隔离 socket 上列出（不触碰默认 server）
muxterm -L muxterm-test list-workspaces

# 向当前 Pane 发送文字（不自动回车）
muxterm send-keys "cargo test"

# 捕获当前 Pane 的画面
muxterm capture-pane

# 结构化 tmux 命令
muxterm tmux session list
muxterm tmux session new --name demo --cwd .
```

注意：

- `-L <socket>` 是 tmux socket 名，用来隔离 server；
- `-s` 在不同子命令里的含义并不统一，脚本里优先用 `muxterm <cmd> --help`
  确认参数；
- `muxterm --help` 列出全部子命令，`muxterm <cmd> --help` 查看参数；
- `tmux` 的 tab/pane 子命令需要 `--session`，部分还需要 `--pane`；
- CLI 每次调用拥有一个短期 Core handle，不常驻 Scene。

---

## 8. 客户端更新

- `[update] auto_check = true` 时，每个会话启动后检查一次；只提醒，不自动安装。
- macOS 应用菜单 / Linux 命令面板提供「检查更新」。
- 点击「一键更新」后，Core 下载、校验 SHA-256、原子安装：
  - Linux 替换 `muxterm` 可执行文件并保留备份；
  - macOS 挂载 DMG、替换 `Muxterm.app`。
- 只有 stable 通道会写入 `releases/latest/download/latest.json`。要试 beta/alpha，
  把 `[update] manifest_url` 指向对应通道的 `latest.json`。

本地验证更新链路（不需要真实 release）：

```bash
VERSION=v0.2.0-alpha.1 CHANNEL=alpha PRERELEASE=true \
  bash scripts/release-package.sh /tmp/muxterm-feed
cd /tmp/muxterm-feed && python3 -m http.server 8231 &
MUXTERM_UPDATE_TEST_FEED=/tmp/muxterm-feed \
MUXTERM_UPDATE_VERSION=v0.0.1 \
  cargo test --no-default-features --features ffi --test update_flow_integration -- --nocapture
```

---

## 9. 排障

### GUI 起不来

```bash
muxterm gui --debug --log-file /tmp/muxterm.log
# 或
RUST_LOG=debug muxterm gui
```

先看日志里的 tmux 协议 / Herdr 帧 / FFI 调用。Linux 请确认 GTK4 与 vte4 开发/运行库
都已安装；macOS 请确认架构是 arm64。

### tmux 连不上

```bash
tmux ls
tmux -L <socket> ls
```

确认 socket 名、session 名、远端 `tmux -CC` 是否可用。不要用 `kill-server` 排查；
需要重启时只对隔离 socket 操作。

### Herdr 连不上

确认 named server 已启动、socket 路径正确、SSH 转发（远程时）可用。不要无名字地
`herdr server stop`。

### 快捷键没反应

1. `muxterm config doctor`；
2. 检查 `[shortcuts]` preset / primary_key / overrides；
3. 确认没有重复 chord 或系统保留组合；
4. macOS 的菜单快捷键优先于终端输入，必要时在设置窗口重绑。

### 更新失败

检查 `manifest_url`、网络、SHA-256 与安装目录权限。Linux 备份文件就在可执行文件
同目录；macOS 更新失败不会删除原 `Muxterm.app`。

---

## 10. 相关文档

| 文档 | 内容 |
| --- | --- |
| [`WORKSPACE.md`](WORKSPACE.md) | 产品树、Workspace、打开路径、FFI/CLI |
| [`RUNTIME.md`](RUNTIME.md) | Runtime × Transport、provider、能力、适配表 |
| [`SURFACE.md`](SURFACE.md) | 单面、像素定律 |
| [`FRONTEND.md`](FRONTEND.md) | Scene / EventPump / 页面 / 各平台落地 |
| [`CONFIG.md`](CONFIG.md) | `config.toml` 契约 |
| [`CLIPBOARD.md`](CLIPBOARD.md) | 图片粘贴、SSH 上传、独立输入框 |
| [`ACTIVITY-DETECTION.md`](ACTIVITY-DETECTION.md) | Activity / Attention 判定与回归 |
| [`PROJECT-STRUCTURE.md`](PROJECT-STRUCTURE.md) | 目录与 crate 分层 |
| [`../website/README.md`](../website/README.md) | 官网本地预览与 GitHub Pages 部署 |
