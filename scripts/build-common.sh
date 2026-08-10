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
#   copy_binary <src>   -> 把 <src> 复制到 OUT_DIR 并改名
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
