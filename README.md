# Muxterm

Linux 桌面端的 **tmux control mode (`-CC`) 原生 UI 终端**。

用 Rust + GTK4 把本地或远程 tmux 的 session / window / pane 渲染成原生 tab 与分割布局，而不是「黑框终端 + `Ctrl+B`」。体验上接近 iTerm2 的 tmux 集成，但面向 Linux。

> 跨平台终端：Linux (GTK4 + TUI) / macOS (SwiftUI) / Windows (TUI-CLI)。详见 [PRODUCT.md](PRODUCT.md)。

## 功能概览

- **tmux `-CC` 原生集成**：pane 输出进原生终端视图，布局变化同步到 UI
- **Tab / Pane**：每个 pane 可对应 tab；支持水平 / 垂直分割
- **本地与远程**：本地 `tmux -CC`，以及通过 SSH 连接远程 tmux session
- **命令面板**：VSCode 风格（默认 `Alt+P`），含 `ssh: connect` 等命令
- **可配置**：TOML 配置（字体、主题、快捷键、SSH、滚动缓冲等）

## 截图

截图请放在 [`assets/screenshots/`](assets/screenshots/)（当前仓库占位说明见该目录）。

建议文件名：

| 文件 | 说明 |
|------|------|
| `overview.png` | 主界面：tab 栏 + pane 分割 |
| `command-palette.png` | 命令面板 |
| `ssh-connect.png` | SSH 远程连接流程 |

添加截图后，可在本节引用，例如：

```markdown
![主界面](assets/screenshots/overview.png)
```

## 依赖环境

| 组件 | 说明 |
|------|------|
| Linux | 桌面环境（GTK4） |
| Rust | 建议 stable（开发机实测 `rustc 1.97.1`） |
| tmux | 建议 3.x（实测 `3.7b`） |
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

GitHub Release 自动构建四种产物（打 tag `v*.*.*` 触发，或手动 dispatch）：

| 产物 | 平台 | 类型 | 运行时依赖 |
|------|------|------|-----------|
| `muxterm-cli-linux-x86_64-*` | Linux x86_64 | CLI/TUI 命令行工具 | glibc (ubuntu-latest), tmux for tmux ops |
| `muxterm-gtk-linux-x86_64-*` | Linux x86_64 | GTK4 GUI 应用 | glibc, libgtk-4-1, libvte-2.91-gtk4, libssl3, tmux |
| `muxterm-macos-arm64-*.zip` | macOS ARM64 | SwiftUI .app 包 | macOS 13+ |
| `muxterm-cli-windows-x86_64-*.exe` | Windows x86_64 | CLI/TUI（无 GUI） | tmux-dependent ops require usable tmux env |

每个产物附带 `.sha256` 校验文件。

> **Windows 限制**：当前 Windows 构建仅支持 TUI/CLI 模式（crossterm），尚无 GUI 前端。

### 版本命名

- 打 tag `v1.2.3` → Release 版本 `v1.2.3`（正式发布）
- 手动 dispatch → `最近 v* tag-dev.短SHA`；无可达 tag → `短SHA`（预发布）

### 从源码构建

| 脚本 | 产物 |
|------|------|
| `scripts/build-linux.sh [--release]` | Linux GTK4 桌面前端 |
| `scripts/build-tui.sh [--release]` | 纯终端 TUI（CI / 无头 / Windows 兼容） |
| `scripts/build-cli.sh [--release]` | C ABI 库 + CLI 二进制 |
| `scripts/build-macos.sh` | macOS SwiftUI app（libmuxterm.a + SwiftPM） |

`--release` 参数或 `PROFILE=release` 环境变量控制 release/debug 构建。

```bash
git clone https://github.com/wlingze/muxterm.git
cd muxterm
./scripts/build-tui.sh --release
./target/release/muxterm
```

调试运行：

```bash
cargo run
# 或更详细日志
cargo run -- --verbose
# 显式选前端
cargo run --features gtk -- --gtk
cargo run --no-default-features --features tui -- --tui
```

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
- `[tmux]` — 鼠标、默认 session
- `[ssh]` — 远程 host / port / user / key
- `[[keybindings]]` — 快捷键自定义

## 使用方法

启动后，Muxterm 会以 GTK4 窗口呈现本地/远程 tmux 的 pane。

### 默认快捷键

| 快捷键 | 作用 |
|--------|------|
| `Alt+N` | 新窗口（new-window ≈ 新 tab + pane） |
| `Alt+T` | 新本地 shell tab |
| `Alt+D` | 水平分割 pane |
| `Alt+Shift+D` | 垂直分割 pane |
| `Alt+1` … `Alt+9` | 切换到第 N 个 tab |
| `Alt+0` | 最后一个 tab |
| `Alt+[` / `Alt+]` | 上一个 / 下一个 pane |
| `Alt+R` | Pane 切换器（模糊搜索） |
| `Alt+P` | 命令面板 |
| `Alt+Shift+Q` | 退出 |

### 命令面板与 SSH

1. 按 `Alt+P` 打开命令面板  
2. 选择 `ssh: connect`（或输入关键字过滤）  
3. 在 QuickPick 中输入 `user@host`（或依赖 `config.toml` 的 `[ssh]`）  
4. 连接成功后，远程 tmux `-CC` 的 pane 会出现在本地 UI 中  

断开可用命令面板中的 `ssh: disconnect`。

### CLI

```text
muxterm [OPTIONS]

Options:
  -v, --verbose              启用详细日志（也可用 RUST_LOG）
  -L, --socket <SOCKET>      tmux socket 名（传给 `tmux -L`，隔离独立 server）
  -h, --help
  -V, --version
```

示例：用独立 socket，不影响默认 tmux 会话：

```bash
muxterm -L muxterm
```

## 项目结构

```text
muxterm/
├── src/
│   ├── lib.rs                  # 库根（pub mod core + platform）
│   ├── main.rs                 # 薄入口（arg 解析 → 委托 platform::cli/tui/linux）
│   ├── core/                   # 非 GUI 核心，平台无关
│   │   ├── model/              # Session→Window→Tab→Pane 模型 + Backend trait
│   │   ├── protocol/
│   │   │   ├── terminal/       # 输入编码 / 进程查询 / scrollback
│   │   │   └── ffi/            # C ABI 导出（macOS/Linux TUI 经此调用）
│   │   ├── runtime/
│   │   │   ├── shell/          # 本地 shell 后端（LocalBackend）
│   │   │   ├── tmux/           # tmux -CC 后端 + 协议解析 + pty
│   │   │   └── daemon.rs       # daemon IPC 后端（DaemonBackend）
│   │   ├── transport/          # local + ssh 字节流传输
│   │   ├── config.rs           # TOML 配置 + 主题
│   │   ├── discovery.rs        # SSH session 发现
│   │   ├── types.rs            # PaneId / WindowId / TabId / SessionId
│   │   └── buffer_cap.rs       # 输出/事件有界缓冲
│   └── platform/              # 前端
│       ├── cli/               # CLI 命令（解析 + 路由 + daemon + 格式化）
│       ├── tui/               # crossterm TUI（feature = "tui"）
│       ├── linux/             # GTK4 + vte4（feature = "gtk"）
│       └── macos/             # SwiftUI + SwiftPM（C ABI via CoreBridge）
├── scripts/                    # 构建脚本
│   ├── build-cli.sh            # cargo build --features ffi
│   ├── build-tui.sh            # cargo build --features tui
│   ├── build-linux.sh          # cargo build --features gtk
│   └── build-macos.sh          # cargo build ffi release + swift build
├── configs/                    # 示例配置与主题
├── tests/                      # 集成 / 回归测试
├── PRODUCT.md                  # 产品规划
├── ARCHITECTURE.md             # 架构与交互规范
└── AGENTS.md                   # 给 coding agent 的开发约定
```

更细的模块职责见 [`ARCHITECTURE.md`](ARCHITECTURE.md)。

## 开发指南

开始前建议阅读：

1. [`PRODUCT.md`](PRODUCT.md) — 产品目标与路线图  
2. [`ARCHITECTURE.md`](ARCHITECTURE.md) — 交互与模块边界  
3. [`AGENTS.md`](AGENTS.md) — commit / 测试约定  

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

CI（push / PR 到 `main`）会跑 check、fmt、clippy、test；详见 [`.github/workflows/ci.yml`](.github/workflows/ci.yml)。

## 许可证

MIT（见 `Cargo.toml` 中的 `license` 字段）。
