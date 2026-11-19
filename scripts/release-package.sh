#!/usr/bin/env bash
# 发布打包：把 CI 产物整理成稳定文件名 + 生成更新清单 latest.json。
#
# 用法:
#   scripts/release-package.sh <dist-dir>
#
# 环境变量:
#   VERSION      版本号（tag 名，如 v1.2.3 / v1.2.3-beta.1 / v1.2.3-alpha.1）
#   FS_VERSION   文件名安全版本号（默认由 VERSION 推导）
#   CHANNEL      发布通道：stable / beta / alpha / prerelease / manual
#   PRERELEASE   "true" 表示预发布
#   SOURCE_SHA   对应提交
#   REPO         owner/name（默认 wlingze/muxterm）
#
# 约定：清单里的资产 URL 用**相对路径**，由 Core 按清单地址所在目录解析。
# 这样 stable（releases/latest/download）、beta（固定 tag）与本地 alpha 测试
# 服务可以共用同一份清单。
set -euo pipefail

DIST="${1:?usage: release-package.sh <dist-dir>}"
REPO="${REPO:-wlingze/muxterm}"
VERSION="${VERSION:?VERSION is required}"
FS_VERSION="${FS_VERSION:-$(printf '%s' "$VERSION" | sed 's/[^a-zA-Z0-9._-]/-/g')}"
CHANNEL="${CHANNEL:-stable}"
PRERELEASE="${PRERELEASE:-false}"
SOURCE_SHA="${SOURCE_SHA:-}"

MACOS_SRC="muxterm-macos-arm64-${FS_VERSION}.dmg"
LINUX_SRC="muxterm-linux-x86_64-${FS_VERSION}.tar.gz"
WINDOWS_SRC="muxterm-windows-x86_64-${FS_VERSION}.zip"
MACOS_STABLE="muxterm-macos-arm64.dmg"
LINUX_STABLE="muxterm-linux-x86_64.tar.gz"
WINDOWS_STABLE="muxterm-windows-x86_64.zip"

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

size_of() {
  if stat -c%s "$1" >/dev/null 2>&1; then
    stat -c%s "$1"
  else
    stat -f%z "$1"
  fi
}

# 稳定文件名（不含版本号）：更新清单与 /releases/latest/download 直达链接都靠它。
cp "$DIST/$MACOS_SRC" "$DIST/$MACOS_STABLE"
cp "$DIST/$MACOS_SRC.sha256" "$DIST/$MACOS_STABLE.sha256"
cp "$DIST/$LINUX_SRC" "$DIST/$LINUX_STABLE"
cp "$DIST/$LINUX_SRC.sha256" "$DIST/$LINUX_STABLE.sha256"
# Windows 也在扫描表里，但当前客户端没有 Windows 自更新目标
# （`core/update/platform.rs` 返回 None）；缺包时跳过，不阻断 macOS/Linux 发布。
WINDOWS_JSON=""
if [[ -f "$DIST/$WINDOWS_SRC" ]]; then
  cp "$DIST/$WINDOWS_SRC" "$DIST/$WINDOWS_STABLE"
  if [[ -f "$DIST/$WINDOWS_SRC.sha256" ]]; then
    cp "$DIST/$WINDOWS_SRC.sha256" "$DIST/$WINDOWS_STABLE.sha256"
  fi
  windows_sha="$(sha256_of "$DIST/$WINDOWS_STABLE")"
  windows_size="$(size_of "$DIST/$WINDOWS_STABLE")"
  WINDOWS_JSON=",
    \"windows-x86_64\": {
      \"name\": \"${WINDOWS_STABLE}\",
      \"url\": \"${WINDOWS_STABLE}\",
      \"sha256\": \"${windows_sha}\",
      \"size\": ${windows_size}
    }"
fi

macos_sha="$(sha256_of "$DIST/$MACOS_STABLE")"
linux_sha="$(sha256_of "$DIST/$LINUX_STABLE")"
macos_size="$(size_of "$DIST/$MACOS_STABLE")"
linux_size="$(size_of "$DIST/$LINUX_STABLE")"
prerelease_json=false
[[ "$PRERELEASE" == "true" ]] && prerelease_json=true

cat > "$DIST/latest.json" <<EOF
{
  "version": "${VERSION}",
  "tag": "${VERSION}",
  "channel": "${CHANNEL}",
  "prerelease": ${prerelease_json},
  "source_sha": "${SOURCE_SHA}",
  "published_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "assets": {
    "macos-arm64": {
      "name": "${MACOS_STABLE}",
      "url": "${MACOS_STABLE}",
      "sha256": "${macos_sha}",
      "size": ${macos_size}
    },
    "linux-gui-x86_64": {
      "name": "${LINUX_STABLE}",
      "url": "${LINUX_STABLE}",
      "sha256": "${linux_sha}",
      "size": ${linux_size}
    }${WINDOWS_JSON}
  }
}
EOF

echo "==> release metadata for ${VERSION} (channel=${CHANNEL}, repo=${REPO})"
ls -la "$DIST"
