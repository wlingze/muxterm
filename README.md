# Muxterm

跨平台终端：把 tmux / Herdr / 本地 shell 收成原生 Tab / Pane UI，而不是黑框 + `Ctrl+B`。

产品层级是 **Workspace = Runtime(Transport) + path**。Linux（GTK4）、macOS（Swift）、TUI、CLI 都是 frontend，只经共享 Rust Core 的 C FFI。详见 [PRODUCT.md](PRODUCT.md)。目录见 [docs/PROJECT-STRUCTURE.md](docs/PROJECT-STRUCTURE.md)。

## 功能概览

- **Runtime × Transport**：shell / tmux / herdr 与 local / ssh 独立组合，不是手写 `LocalTmux` 四种 mode
- **Tab / Pane**：每个 Workspace 同一套拓扑；tmux window → Tab，tmux pane → Pane
- **本地与远程**：local 或 `ssh <alias>`；同一 SSH host 复用一条 TargetConnection
- **Project / Worktree / 模板**：项目清单、git worktree 联动、新建会话时应用 WorkspaceTemplate
- **命令面板**：VSCode 风格（默认 `Alt+P` / macOS Command+P）
- **可配置**：单一 `config.toml`（字体、主题、快捷键、Projects、templates）

## 截图

截图请放在 [`assets/screenshots/`](assets/screenshots/)（当前仓库占位说明见该目录）。

建议文件名：

| 文件 | 说明 |
|------|------|
| `overview.png` | 主界面：侧栏 + tab 栏 + pane 分割 |
| `command-palette.png` | 命令面板 |
| `ssh-connect.png` | SSH 远程连接流程 |

添加截图后，可在本节引用，例如：

```markdown
![主界面](assets/screenshots/overview.png)
```

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

GitHub Release 自动构建产物（打 tag `v*.*.*` 触发，或手动 dispatch）：

| 产物 | 平台 | 类型 | 运行时依赖 |
|------|------|------|-----------|
| `muxterm-cli-linux-x86_64-*` | Linux x86_64 | CLI/TUI | glibc (ubuntu-latest), tmux for tmux ops |
| `muxterm-gtk-linux-x86_64-*` | Linux x86_64 | GTK4 GUI | glibc, libgtk-4-1, libvte-2.91-gtk4, libssl3, tmux |
| `muxterm-macos-arm64-*.zip` | macOS ARM64 | SwiftUI .app 包 | macOS 13+ |

每个产物附带 `.sha256` 校验文件。

### 版本命名

- 打 tag `v1.2.3` → Release 版本 `v1.2.3`（正式发布）
- 手动 dispatch → `最近 v* tag-dev.短SHA`；无可达 tag → `短SHA`（预发布）

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

权威字段见 [`docs/CONFIG.md`](docs/CONFIG.md)。Runtime / Transport **不直接读**配置文件；值经 WorkspaceSpec 或 provider 参数注入。

## 使用方法

`muxterm gui` 以当前系统 GUI 呈现已打开的 Workspace。`muxterm tui` 在终端里做同一件事。无 subcommand 时是 CLI。

### 默认快捷键

| 快捷键 | 作用 |
|--------|------|
| `Alt+N` | 新 Tab |
| `Alt+T` | 新本地 shell tab |
| `Alt+D` | 水平分割 pane |
| `Alt+Shift+D` | 垂直分割 pane |
| `Alt+1` … `Alt+9` | 切换到第 N 个 tab |
| `Alt+0` | 最后一个 tab |
| `Alt+[` / `Alt+]` | 上一个 / 下一个 pane |
| `Alt+R` | Pane 切换器（模糊搜索） |
| `Alt+P` | 命令面板（macOS 为 Command+P） |
| `Alt+Shift+Q` | 退出 |

Linux 的 `primary_key = "auto"` 解析为 Alt；macOS 解析为 Command。见 [`docs/CONFIG.md`](docs/CONFIG.md) §5。

### 命令面板与打开

1. 打开命令面板
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

`-s` 是工作区名，不是 tmux `$N`。不要对用户默认 server 跑 `kill-session`。

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
├── PRODUCT.md
├── ARCHITECTURE.md
├── TASKS.md
└── AGENTS.md
```

模块职责见 [`docs/PROJECT-STRUCTURE.md`](docs/PROJECT-STRUCTURE.md)。

## 开发指南

开始前建议阅读：

1. [`PRODUCT.md`](PRODUCT.md) — 产品目标
2. [`docs/WORKSPACE.md`](docs/WORKSPACE.md) — 产品树与打开路径
3. [`docs/FRONTEND.md`](docs/FRONTEND.md) — 前端 Scene / 页面
4. [`TASKS.md`](TASKS.md) — 施工顺序
5. [`AGENTS.md`](AGENTS.md) — commit / 测试 / tmux 安全

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
- Release workflow [`release.yml`](.github/workflows/release.yml) 保持 tag / 手动触发，独立于 PR CI

稳定 aggregate check 名称：`core / required`、`four-mode / aggregate`、`linux / required`、`macos / required`。由于 Linux/macOS workflow 使用路径触发器，主分支保护应按仓库的路径规则集/required workflow 能力配置平台检查；不要把未触发路径的 `linux / required` 或 `macos / required` 当作所有 PR 都必须出现的单一全局 status。

「four-mode」矩阵的产品含义是 **Runtime × Transport**（shell/tmux × local/ssh），不是复合 `RuntimeMode` 枚举。Herdr 专项见 [`docs/HERDR-TESTING.md`](docs/HERDR-TESTING.md)。

## 许可证

MIT（见 `Cargo.toml` 中的 `license` 字段）。
