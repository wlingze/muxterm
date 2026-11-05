# Muxterm

<p align="center">
  <img src="assets/muxterm-logo-b-wide.svg" alt="Muxterm logo" width="180">
</p>

把 tmux / Herdr / 本地 shell 收成工作区的跨平台终端工作台，原生集成 tmux / Herdr，并可按 Project 组织本地和远程工作。

官网：<https://wlingze.github.io/muxterm/>（源码在 [`website/`](website/)，支持中英文与黑白主题）· 使用手册：[`docs/USER-GUIDE.md`](docs/USER-GUIDE.md)

产品层级是 **Workspace = Runtime(Transport) + path**。Linux（GTK4）、macOS（Swift）、TUI、CLI 都是 frontend，只经共享 Rust Core 的 C FFI。详见 [PRODUCT.md](PRODUCT.md)。目录见 [docs/PROJECT-STRUCTURE.md](docs/PROJECT-STRUCTURE.md)。

后续以编辑框、输出阅读/批量复制、图片投递与呈现、远程文件打开和终端可靠性收口；ACP 使用现成客户端。
功能计划与稳定期边界见 [当前产品方向](docs/PRODUCT-DIRECTION.md)，不代表这些计划已全部交付。

## 功能概览

- **Runtime × Transport**：shell / tmux / herdr 与 local / ssh 独立组合，不是手写 `LocalTmux` 四种 mode
- **复用器即工作区**：每个 tmux session、Herdr workspace 进入 WorkspacePool，保留原生 Tab / Pane 拓扑
- **Tab / Pane**：每个 Workspace 同一套拓扑；tmux window → Tab，tmux pane → Pane
- **本地与远程**：local 或 `ssh <alias>`；同一 SSH host 复用一条 TargetConnection
- **端口感知与转发**：SSH 工作区扫描远端监听端口，一键转发到 localhost，可选 LAN 访问（macOS 前端）
- **Project / Worktree / 模板**：项目清单、git worktree 联动、新建会话时应用 WorkspaceTemplate
- **Activity / Attention**：commands / agents / attention 三条 lane；后台 agent 等待输入时侧栏可见
- **QuickConnect / 命令面板**：默认 `Alt+P` / `Cmd+P` 打开 QuickConnect，`Alt+Shift+P` / `Cmd+Shift+P` 打开命令面板
- **可配置**：单一 `config.toml`（字体、主题、快捷键、Projects、templates）
- **客户端更新**：stable / beta / alpha 通道，内置下载、SHA-256 校验、原子安装

## 官网与界面预览

官网：<https://wlingze.github.io/muxterm/>（中文 <https://wlingze.github.io/muxterm/zh/>）。
源码在 [`website/`](website/)，是纯静态页面，支持中英文与黑白主题，也可用本地 HTTP 预览：

```bash
python3 -m http.server 8000 --bind 0.0.0.0 --directory website
# 打开 http://127.0.0.1:8000/
```

官网首页包含可交互的 Workspace / Tab / Pane 预览，以及真实 macOS 应用截图（工作台、
Agents 聚合、Shells 聚合三张一组，另有端口选择器，各有亮/暗两套，使用模拟会话数据）。
截图在页面以小尺寸三栏展示，点开看大图；一键切换整站主题，黑白两套截图随之同步。
端口扫描与转发在首页单独成节，配真实截图与可交互演示。文档页只保留快速上手，
仓库 `docs/` 的完整参考留在 GitHub，不在网页上渲染。

真实用户会话截图目录 [`assets/screenshots/`](assets/screenshots/) 可能包含个人内容，仍不默认提交；
官网使用的截图由 `src/frontend/macos/AppE2ETests/WebsiteScreenshotE2ETests.swift`、
`WebsiteGalleryE2ETests.swift` 与 `SSHPortPickerE2ETests` 在隔离 tmux socket / `ZDOTDIR` 上生成，
细节见 [`website/README.md`](website/README.md)。

## 依赖环境

| 组件 | 说明 |
|------|------|
| Linux | 桌面环境（GTK4）才能跑 `muxterm gui` |
| Rust | 建议 stable（开发机实测 `rustc 1.97.1`） |
| tmux | 建议 3.x（实测 `3.7b`）；tmux runtime 需要 |
| 系统库 | `gtk4`、`vte`（GTK4 版）、OpenSSL 等开发包 |

Arch 示例：

```bash
sudo pacman -S rust gtk4 vte4 openssl pkgconf tmux
```

Debian / Ubuntu 示例：

```bash
sudo apt-get install -y build-essential pkg-config \
  libgtk-4-dev libvte-2.91-gtk4-dev libssl-dev tmux
```

## 安装

### 从 Release 下载

发布由 tag 触发，产物按 tag 后缀分通道：

| Tag 示例 | 通道 | 行为 |
|----------|------|------|
| `v1.2.3` | stable | 构建 + 发布正式 Release（`/releases/latest` 指向它，内置更新只认它） |
| `v1.2.3-beta.1` | beta | 构建 + 发布 prerelease（不占用 `latest`） |
| `v1.2.3-alpha.1` | alpha | **只构建**，产物上传到 Actions artifacts，不创建 Release |
| 手动 dispatch | manual | 构建 `最近 tag-dev.短SHA` 的 prerelease |

正式目标平台是两个桌面版本：

| 产物 | 平台 | 类型 | 运行时依赖 |
|------|------|------|-----------|
| `muxterm-macos-arm64.dmg` | macOS ARM64 | DMG containing `Muxterm.app` | macOS 13+ |
| `muxterm-gtk-linux-x86_64.tar.gz` | Linux x86_64 | GTK4 GUI app | glibc, libgtk-4-1, libvte-2.91-gtk4, libssl3, tmux |

每个产物附带 `.sha256` 校验文件；Release 同时发布 `latest.json`（内置更新读取的清单）。
资产文件名同时保留带版本号的一份（如 `muxterm-macos-arm64-v1.2.3.dmg`）。

直接下载 stable（`latest`）：

- macOS ARM64：[`muxterm-macos-arm64.dmg`](https://github.com/wlingze/muxterm/releases/latest/download/muxterm-macos-arm64.dmg)
- Linux x86_64：[`muxterm-gtk-linux-x86_64.tar.gz`](https://github.com/wlingze/muxterm/releases/latest/download/muxterm-gtk-linux-x86_64.tar.gz)

macOS 发布包使用 ad-hoc 签名，未经过 Apple 公证；首次启动若被 Gatekeeper 拦截，
请在 **系统设置 → 隐私与安全性** 允许该应用。Linux 包在 ubuntu-latest 上构建；
目标机 glibc 较旧时请从源码构建。

### 客户端更新

Core 负责「提醒 + 一键更新」，前端只渲染提醒条与按钮：

- 启动后按 `[update] auto_check` 自动检查一次；也可在 macOS 菜单 / Linux 命令面板
  手动触发「检查更新」。
- 发现新版本时显示提醒条，点击「一键更新」由 Core 下载、校验 SHA-256、原子安装
  （Linux 替换 `muxterm` 可执行文件并保留备份；macOS 挂载 DMG 替换 `Muxterm.app`）。
- 只有 stable 版本会出现在 `releases/latest/download/latest.json`；要试 beta/alpha，
  把 `[update] manifest_url` 指向对应通道的 `latest.json`。

本地验证整条链路（无需真实 release）：

```bash
VERSION=v0.2.0-alpha.1 CHANNEL=alpha PRERELEASE=true \
  bash scripts/release-package.sh /tmp/muxterm-feed
cd /tmp/muxterm-feed && python3 -m http.server 8231 &
MUXTERM_UPDATE_TEST_FEED=/tmp/muxterm-feed \
MUXTERM_UPDATE_VERSION=v0.0.1 \
  cargo test --no-default-features --features ffi --test update_flow_integration -- --nocapture
```

### 版本命名

- 打 tag `v1.2.3` → Release 版本 `v1.2.3`（正式发布，占 `latest`）
- 打 tag `v1.2.3-beta.1` → prerelease `v1.2.3-beta.1`（不占 `latest`）
- 打 tag `v1.2.3-alpha.1` → 只构建，不发布
- 手动 dispatch → `最近 v* tag-dev.短SHA`；无可达 tag → `短SHA`（预发布）

`MUXTERM_BUILD_VERSION` 会写进二进制，内置更新据此比较版本；CI 打 tag 时由
workflow 注入，本地构建回退 `Cargo.toml` 版本号。

### 从源码构建

| 脚本 | 产物 |
|------|------|
| `scripts/build-linux.sh [--release]` | Linux GTK4 桌面前端 |
| `scripts/build-tui.sh [--release]` | 纯终端 TUI（CI / 无头，跨平台） |
| `scripts/build-cli.sh [--release]` | C ABI 库 + CLI 二进制 |
| `scripts/build-macos.sh [--release]` | macOS SwiftUI app（libmuxterm.a + SwiftPM） |

产物统一输出到 `./build/<os>/`（`os` = `linux` / `macos` / `windows`），
二进制统一命名 `muxterm`（Windows 为 `muxterm.exe`）；macOS 额外产出 `Muxterm.app`。
这样各系统本地构建与 CI 打包结果一致。

`--release` 参数或 `PROFILE=release` 环境变量控制 release/debug 构建。

```bash
git clone https://github.com/wlingze/muxterm.git
cd muxterm
./scripts/build-tui.sh --release
./build/linux/muxterm        # macOS 上是 ./build/macos/muxterm
```

每个仓库 / worktree 使用**本地**编译缓存（`.cargo/config.toml` 不设置 `target-dir`，
cargo 默认 `./target`）与本地产物目录 `./build/<os>/`，不跨 worktree 共享；可用环境变量 `CARGO_TARGET_DIR` 覆盖。

调试运行：

```bash
# 无 subcommand = CLI
cargo run -- --help

# TUI
cargo run --no-default-features --features tui -- tui

# Linux GUI
cargo run --features gtk -- gui

# 详细日志
cargo run --features gtk -- gui --verbose
```

兼容期内 `--tui` / `--gtk` 仍可用；目标入口是 subcommand `tui` / `gui`。

可选：把二进制装到 PATH：

```bash
cargo install --path .
```

## 配置

复制示例配置：

```bash
mkdir -p ~/.config/muxterm
cp configs/config.example.toml ~/.config/muxterm/config.toml
```

常用段落：

- `[font]` / `[theme]` — 字体与主题（主题文件在 `configs/themes/`）
- `[[projects]]` — 项目清单（目标 Runtime / Transport / 路径）
- `[[templates]]` — WorkspaceTemplate（新建会话时的 tab / pane）
- `[shortcuts]` — 快捷键
- `[platform.linux]` / `[platform.macos]` — 平台专属
- `[update]` — 客户端更新（`auto_check`、`manifest_url`）

权威字段见 [`docs/CONFIG.md`](docs/CONFIG.md)。Runtime / Transport **不直接读**配置文件；值经 WorkspaceSpec 或 provider 参数注入。
用户视角的安装、QuickConnect、SSH/Herdr 与排障见 [`docs/USER-GUIDE.md`](docs/USER-GUIDE.md)。

## 使用方法

完整安装、QuickConnect、SSH、Herdr、配置和排障见 [`docs/USER-GUIDE.md`](docs/USER-GUIDE.md)。

`muxterm gui` 以当前系统 GUI 呈现已打开的 Workspace。`muxterm tui` 在终端里做同一件事。无 subcommand 时是 CLI。

### 默认快捷键

`primary_key = "auto"` 时，Linux 解析为 `Alt`，macOS 解析为 `Command`。

| 动作 | Linux | macOS |
|------|-------|-------|
| QuickConnect | `Alt+P` | `Cmd+P` |
| 命令面板 | `Alt+Shift+P` | `Cmd+Shift+P` |
| 新窗口 | `Alt+N` | `Cmd+N` |
| 新本地 shell tab | `Alt+T` | `Cmd+T` |
| 水平分割 pane | `Alt+S` | `Cmd+D` |
| 垂直分割 pane | `Alt+V` | `Cmd+S` |
| 上一个 / 下一个 pane | `Alt+[` / `Alt+]` | `Cmd+[` / `Cmd+]` |
| 第 N 个 tab | `Alt+1` … `Alt+9` | `Cmd+1` … `Cmd+9` |
| 最后一个 tab | `Alt+0` | `Cmd+0` |
| 搜索 Pane | 命令面板 → Search panes | `Cmd+F` |
| 复制 / 粘贴 | `Ctrl+Shift+C` / `Ctrl+Shift+V` | `Cmd+C` / `Cmd+V` |
| 退出 | `Ctrl+Q` | `Cmd+Q` |

Linux 仍保留兼容绑定：`Alt+D` = 垂直分割，`Alt+Shift+D` = 水平分割。
完整 Action ID 与覆盖方式见 [`docs/CONFIG.md`](docs/CONFIG.md) §5。

macOS 窗口隐藏了红黄绿交通灯，窗口操作保留在 **View** 菜单、命令面板和快捷键设置中。默认：
`⌘M` 最小化窗口、`⇧⌘T` 占满当前桌面（再按还原）、`⇧⌘F` 原生全屏、`⇧⌘W` 关闭窗口。

### QuickConnect 与打开

1. `Alt+P` / `Cmd+P` 打开 QuickConnect
2. 从 Candidate 列表选 Project / Worktree / Existing / Recent
3. Core 解析成 WorkspaceSpec 并打开；frontend 不手写 spec
4. 已在池里的 Workspace 点击即切换可见 Scene，不重连

### CLI

```text
muxterm [OPTIONS] [COMMAND]

无 subcommand     CLI（list-workspaces / new-tab / send-keys …）
muxterm tui       TUI 前端
muxterm gui       当前系统 GUI

Options:
  -v, --verbose              启用详细日志（也可用 RUST_LOG）
  -L, --socket <SOCKET>      tmux socket 名（传给 `tmux -L`，隔离独立 server）
  -h, --help
  -V, --version
```

示例：用独立 socket，不影响默认 tmux 会话：

```bash
muxterm -L muxterm list-workspaces
```

`-L` 是 tmux socket 名。其他子命令参数以 `muxterm <cmd> --help` 为准。
不要对用户默认 server 跑 `kill-session` / `kill-server`。

## 项目结构

```text
muxterm/
├── src/
│   ├── lib.rs                 # 唯一 Core library root
│   ├── main.rs                # 唯一 binary：薄入口，无 mod 声明
│   ├── core/
│   └── frontend/              # cli / tui / linux / macos / windows + ffi_client
├── scripts/
├── configs/
├── tests/
├── docs/
├── website/                   # 官网（静态 HTML/CSS/JS，GitHub Pages）
├── PRODUCT.md
├── ARCHITECTURE.md
├── TASKS.md
└── AGENTS.md
```

模块职责见 [`docs/PROJECT-STRUCTURE.md`](docs/PROJECT-STRUCTURE.md)。

## 开发指南

开始前建议阅读：

1. [`docs/USER-GUIDE.md`](docs/USER-GUIDE.md) — 安装、使用、配置与排障
2. [`PRODUCT.md`](PRODUCT.md) — 产品目标
3. [`docs/WORKSPACE.md`](docs/WORKSPACE.md) — 产品树与打开路径
4. [`docs/FRONTEND.md`](docs/FRONTEND.md) — 前端 Scene / 页面
5. [`TASKS.md`](TASKS.md) — 施工顺序
6. [`AGENTS.md`](AGENTS.md) — commit / 测试 / tmux 安全

常用命令：

```bash
cargo fmt
cargo check --features gtk
cargo clippy --features gtk -- -D warnings
cargo test --no-default-features --features tui
cargo check --no-default-features --features ffi
```

约定摘要：

- Rust 2021；应用层 `anyhow`，库层错误用 `thiserror`
- 日志用 `tracing`，不要用 `println!` 调试
- tmux 协议解析保持纯函数，便于单元测试
- 增量提交：`feat:` / `fix:` / `test:` / `refactor:` / `docs:` / `ci:` / `chore:`
- 不要在 commit message 里加 `Co-authored-by`

CI 按职责和路径分流（仅 `main` push / 到 `main` 的 PR，避免 feature branch 的 push + PR 重复运行）：

- 核心共享检查：[`ci.yml`](.github/workflows/ci.yml)
- Linux 平台检查：[`linux.yml`](.github/workflows/linux.yml)
- macOS 平台检查：[`macos.yml`](.github/workflows/macos.yml)
- 官网部署：[`pages.yml`](.github/workflows/pages.yml)（push `main` 且改动 `website/`，或手动触发）
- Release workflow [`release.yml`](.github/workflows/release.yml) 保持 tag / 手动触发，独立于 PR CI

稳定 aggregate check 名称：`core / required`、`four-mode / aggregate`、`linux / required`、`macos / required`。由于 Linux/macOS workflow 使用路径触发器，主分支保护应按仓库的路径规则集/required workflow 能力配置平台检查；不要把未触发路径的 `linux / required` 或 `macos / required` 当作所有 PR 都必须出现的单一全局 status。

「four-mode」矩阵的产品含义是 **Runtime × Transport**（shell/tmux × local/ssh），不是复合 `RuntimeMode` 枚举。Herdr 专项见 [`docs/HERDR-TESTING.md`](docs/HERDR-TESTING.md)。

## 许可证

MIT（见 `Cargo.toml` 中的 `license` 字段）。
