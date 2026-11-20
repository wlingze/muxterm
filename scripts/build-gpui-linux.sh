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

echo "==> done: $OUT_DIR/muxterm"
file "$OUT_DIR/muxterm" || true
"$OUT_DIR/muxterm" --version || true