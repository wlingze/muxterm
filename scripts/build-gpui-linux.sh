#!/usr/bin/env bash
# 编译 GPUI 跨平台前端 → ./build/linux/muxterm（+ 桌面集成资源）
#
# 产物结构（tar.gz 里就是这一层，与 GTK 包同形）：
#   muxterm                              gpui feature 编出的单一二进制
#   README.txt
#   share/applications/dev.muxterm.app.desktop
#   share/icons/hicolor/512x512/apps/dev.muxterm.app.png
#
# 系统依赖（Debian/Ubuntu，`scripts/ci/setup-gpui-linux.sh` 会装齐）：
#   build-essential pkg-config libxkbcommon-dev libwayland-dev libx11-dev
#   libxkbcommon-x11-dev libfontconfig1-dev libfreetype-dev
# gpui 0.2.2 在 Linux 上通过 blade/wgpu + xkbcommon + fontconfig 工作；
# 缺库时 cargo 的构建脚本会报 `pkg-config` 找不到包，构建直接失败。
#
# 用法: ./scripts/build-gpui-linux.sh [--release]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
source "$ROOT/scripts/build-common.sh"

RELEASE="$(parse_release "${1:-}")"
PROFILE="debug"
[[ -n "$RELEASE" ]] && PROFILE="release"

# 缺系统库时先给一条人能看懂的话，而不是让 pkg-config 的原始报错淹掉。
missing=()
command -v pkg-config >/dev/null 2>&1 || missing+=("pkg-config")
for pkg in xkbcommon wayland-client x11 fontconfig; do
  if command -v pkg-config >/dev/null 2>&1 && ! pkg-config --exists "$pkg"; then
    missing+=("$pkg")
  fi
done
if [[ ${#missing[@]} -gt 0 ]]; then
  cat >&2 <<EOF
ERROR: 缺少 GPUI/Linux 构建依赖: ${missing[*]}

安装（Debian/Ubuntu）:
  sudo apt-get install -y build-essential pkg-config \\
    libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev \\
    libx11-dev libfontconfig1-dev libfreetype-dev

其它发行版按包名对应安装（xkbcommon / wayland / libX11 / fontconfig / freetype）。
EOF
  exit 1
fi

export CARGO_REGISTRIES_CRATES_IO_PROTOCOL="${CARGO_REGISTRIES_CRATES_IO_PROTOCOL:-sparse}"

echo "==> cargo build gpui $RELEASE"
cargo build --no-default-features --features gpui $RELEASE

BIN="$(cargo_bin_path "$PROFILE")"
if [[ ! -x "$BIN" ]]; then
  echo "ERROR: $BIN not found after cargo build" >&2
  exit 1
fi
smoke_cli_help "$BIN"

OUT_DIR="$(build_os_dir)"
mkdir -p "$OUT_DIR"
echo "==> install $OUT_DIR/muxterm"
install_executable "$BIN" "$OUT_DIR/muxterm"

# ── 桌面集成资源（打包与本地试用都用它）──────────────────────────
ICON_SRC="$ROOT/src/frontend/macos/Resources/MuxtermLogo.png"
ICON_DIR="$OUT_DIR/share/icons/hicolor/512x512/apps"
DESKTOP_DIR="$OUT_DIR/share/applications"
mkdir -p "$ICON_DIR" "$DESKTOP_DIR"
if [[ -f "$ICON_SRC" ]]; then
  cp -f "$ICON_SRC" "$ICON_DIR/dev.muxterm.app.png"
else
  echo "WARN: $ICON_SRC 缺失，包内将没有图标" >&2
fi

cat > "$DESKTOP_DIR/dev.muxterm.app.desktop" <<'DESKTOP'
[Desktop Entry]
Type=Application
Name=Muxterm
Comment=Terminal for tmux sessions and coding agents
Exec=muxterm gpui
Icon=dev.muxterm.app
Terminal=false
Categories=System;TerminalEmulator;
StartupWMClass=dev.muxterm.app
DESKTOP

# ── 安装器：把 .desktop + 图标装进 XDG data 目录 ──────────────────
# 不做这一步，GNOME 顶栏 / 应用列表只能显示通用图标：图标是按 app_id
# （dev.muxterm.app）去 ~/.local/share/applications 里找 .desktop 的。
cat > "$OUT_DIR/install.sh" <<'INSTALL'
#!/bin/sh
# 安装 Muxterm 的桌面入口与图标（per-user，不需要 root）。
set -eu
here=$(cd "$(dirname "$0")" && pwd)
data="${XDG_DATA_HOME:-$HOME/.local/share}"
app_dir="$data/applications"
icon_dir="$data/icons/hicolor/512x512/apps"
bin="$here/muxterm"

if [ ! -x "$bin" ]; then
  echo "ERROR: $bin 不存在或不可执行" >&2
  exit 1
fi

install -d "$app_dir" "$icon_dir"
if [ -f "$here/share/icons/hicolor/512x512/apps/dev.muxterm.app.png" ]; then
  install -m 644 "$here/share/icons/hicolor/512x512/apps/dev.muxterm.app.png" \
    "$icon_dir/dev.muxterm.app.png"
fi

# Exec 用绝对路径：不要求用户把 muxterm 加进 PATH。
cat > "$app_dir/dev.muxterm.app.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Muxterm
Comment=Terminal for tmux sessions and coding agents
Exec=$bin gpui
Icon=dev.muxterm.app
Terminal=false
Categories=System;TerminalEmulator;
StartupWMClass=dev.muxterm.app
DESKTOP

if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "$app_dir" >/dev/null 2>&1 || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -q -t -f "$data/icons/hicolor" >/dev/null 2>&1 || true
fi

echo "已安装桌面入口: $app_dir/dev.muxterm.app.desktop"
echo "现在可以在应用列表里搜 \"Muxterm\" 打开，图标也会出现在顶栏。"
INSTALL
chmod +x "$OUT_DIR/install.sh"

cat > "$OUT_DIR/README.txt" <<'README'
muxterm (GPUI) for Linux x86_64

运行:
  ./muxterm gpui            打开窗口
  ./muxterm                 双击/桌面启动自动进 gpui；终端里跑是 TUI
  ./muxterm tui             强制 TUI
  ./muxterm gpui -L <sock>  指定 tmux socket

桌面图标 / 应用列表:
  ./install.sh              复制 .desktop 与图标到 ~/.local/share
                            （不做这步，GNOME 顶栏只能显示通用图标）

依赖:
  Wayland 或 X11、Vulkan 或 GL、fontconfig + 等宽字体（内置 JetBrains Mono）、tmux 3.x

Checksums: 见同目录 .sha256 文件。
README
chmod 644 "$OUT_DIR/README.txt"

echo "==> done: $OUT_DIR/muxterm"
file "$OUT_DIR/muxterm" || true
"$OUT_DIR/muxterm" --version || true