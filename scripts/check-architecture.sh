#!/usr/bin/env bash
# Muxterm 结构门禁。只检查已经收口的边界；迁移中的历史注释不作为失败条件。
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

failures=0
checks=0

check_absent() {
    local label="$1"
    local pattern="$2"
    shift 2
    local paths=()
    local path
    for path in "$@"; do
        if [[ -e "$path" ]]; then
            paths+=("$path")
        fi
    done
    if [[ "${#paths[@]}" -eq 0 ]]; then
        return
    fi

    checks=$((checks + 1))
    local matches
    if matches="$(rg -n --no-heading "$pattern" "${paths[@]}" 2>/dev/null)"; then
        echo "architecture: FAIL: $label" >&2
        printf '%s\n' "$matches" | head -50 >&2 || true
        failures=$((failures + 1))
    else
        local rg_status=$?
        if [[ "$rg_status" -gt 1 ]]; then
            echo "architecture: ERROR: unable to check $label (rg status $rg_status)" >&2
            failures=$((failures + 1))
        fi
    fi
}

check_absent \
    "root main.rs must not declare core/platform modules" \
    '^[[:space:]]*mod (core|platform)[[:space:]]*;' \
    src/main.rs
check_absent \
    "frontend must not import Core internals directly" \
    'crate::(core|muxterm_core)' \
    src/platform src/bin
check_absent \
    "legacy runtime mode/factory names" \
    'RuntimeMode|create_runtime|build_runtime' \
    crates/muxterm-core/src
check_absent \
    "legacy catalog builtin path" \
    'catalog/builtin' \
    crates/muxterm-core/src src
check_absent \
    "frontend must use the shared ffi_client instead of ffi_bridge" \
    'ffi_bridge' \
    src crates/muxterm-core/src
check_absent \
    "config service must not depend on runtime/workspace/projects domains" \
    'crate::(runtime|workspace|projects)' \
    crates/muxterm-core/src/config_service
check_absent \
    "FFI function modules must not depend on the api facade" \
    'super::super::api|crate::protocol::ffi::api' \
    crates/muxterm-core/src/protocol/ffi/functions
check_absent \
    "frontend must not expose the removed visible-grid FFI" \
    'muxterm_(workspace_)?pane_visible_ansi|paneVisibleANSI|get_workspace_pane_visible_ansi' \
    src crates/muxterm-core/src

if [[ "$failures" -ne 0 ]]; then
    echo "architecture: $failures check(s) failed" >&2
    exit 1
fi

echo "architecture: OK ($checks checks)"
