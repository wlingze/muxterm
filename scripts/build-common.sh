#!/usr/bin/env bash
# 共享构建工具：统一产物输出目录 ./build/<os>/，保证各系统打包结果一致。
#
# 约定：
#   - 产物统一放到 <repo>/build/<os>/ 下
#   - 二进制统一命名：muxterm（macOS/Linux）、muxterm.exe（Windows）
#   - macOS 额外产出 Muxterm.app（`muxterm gui` 用）
#
# 用法：`source scripts/build-common.sh` 后调用：
#   build_os_dir()      -> 输出 ROOT/build/<os> 路径
#   install_executable <src> <dst>
#       把可执行文件装到 dst。macOS 必须换 inode 再 ad-hoc 签名，
#       禁止对已有 Mach-O 原地 `cp -f`（内核缓存旧签名 → `zsh: killed`）。
set -euo pipefail

# 解析 release 标志（支持 --release 或 PROFILE=release）
parse_release() {
  RELEASE=""
  if [[ "${1:-}" == "--release" || "${PROFILE:-}" == "release" ]]; then
    RELEASE="--release"
  fi
  printf '%s' "$RELEASE"
}

# 探测操作系统短名
detect_os() {
  local uname_out
  uname_out="$(uname -s)"
  case "$uname_out" in
    Darwin) echo "macos" ;;
    MINGW*|MSYS*|CYGWIN*) echo "windows" ;;
    *) echo "linux" ;;
  esac
}

# 输出 OS 下二进制文件名
binary_name() {
  if [[ "$(detect_os)" == "windows" ]]; then
    echo "muxterm.exe"
  else
    echo "muxterm"
  fi
}

# 输出 build 根目录（repo 根）
build_root() {
  local root
  root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
  echo "$root/build"
}

# 输出当前 OS 的产物目录 <repo>/build/<os>
build_os_dir() {
  echo "$(build_root)/$(detect_os)"
}

# 解析 cargo 实际 target 目录（优先 CARGO_TARGET_DIR，其次 .cargo/config.toml 的 target-dir）
cargo_target_dir() {
  local root
  root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
  if [[ -n "${CARGO_TARGET_DIR:-}" ]]; then
    echo "$CARGO_TARGET_DIR"
    return
  fi
  local cfg
  cfg="$(cat "$root/.cargo/config.toml" 2>/dev/null || true)"
  local td
  td="$(printf '%s\n' "$cfg" | sed -n 's/^target-dir[[:space:]]*=[[:space:]]*["'"'"']\(.*\)["'"'"']/\1/p' | head -1)"
  if [[ -n "$td" ]]; then
    # 相对路径以仓库根为基准（可能含 ../）。若目录尚不存在（例如 CI 首次 clone
    # 时共享 target 未创建），回退到仓库本地 ./target，避免构建中断。
    if (cd "$root" && cd "$td" 2>/dev/null); then
      (cd "$root" && cd "$td" && pwd)
    else
      echo "$root/target"
    fi
  else
    echo "$root/target"
  fi
}

# 输出指定 profile 的二进制完整路径（debug/release）
cargo_bin_path() {
  local profile="$1"
  local td
  td="$(cargo_target_dir)"
  echo "$td/$profile/$(binary_name)"
}

# 安装可执行文件到 dst。
#
# macOS / Apple Silicon：对已有 Mach-O 原地 `cp -f` 不会换 inode，内核继续
# 用旧代码签名页校验新内容，启动即 SIGKILL（zsh: killed / Code Signature
# Invalid）。必须先写到新文件，ad-hoc 签名，再 `mv` 换上。
install_executable() {
  local src="$1"
  local dst="$2"
  local tmp
  if [[ ! -f "$src" ]]; then
    echo "ERROR: install_executable: source missing: $src" >&2
    return 1
  fi
  mkdir -p "$(dirname "$dst")"
  tmp="${dst}.new.$$"
  rm -f "$tmp"
  cp "$src" "$tmp"
  chmod +x "$tmp"
  if [[ "$(uname -s)" == "Darwin" ]]; then
    xattr -cr "$tmp" 2>/dev/null || true
    # `-` = ad-hoc；debug CLI 不启用 hardened runtime。
    codesign --force --sign - --identifier "dev.muxterm.cli" "$tmp"
  fi
  mv -f "$tmp" "$dst"
  if [[ "$(uname -s)" == "Darwin" ]]; then
    codesign --verify "$dst"
  fi
}

# 构建结束后必须能跑 `--help`。失败（含 SIGKILL=137）则让整个打包失败。
smoke_cli_help() {
  local bin="$1"
  echo "==> smoke: $bin --help"
  if [[ ! -x "$bin" ]]; then
    echo "ERROR: CLI is not executable: $bin" >&2
    return 1
  fi
  if ! "$bin" --help >/dev/null; then
    local status=$?
    echo "ERROR: $bin --help failed (exit $status; 137 = SIGKILL / invalid signature)" >&2
    if [[ "$(uname -s)" == "Darwin" ]]; then
      codesign -dv --verbose=4 "$bin" >&2 || true
      codesign --verify --verbose=4 "$bin" >&2 || true
    fi
    return 1
  fi
}
