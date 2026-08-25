#!/usr/bin/env bash
# 本地测试入口：一次编译、一起跑，与 CI 各 job 的命令一一对应。
#
#   scripts/test.sh run core    # core/tui + ffi 全部测试（含 four-mode 矩阵）
#   scripts/test.sh run linux   # GTK e2e 全部测试（xvfb）
#   scripts/test.sh run macos   # macOS ffi + Swift 测试（需 macOS）
#
# 纪律：tmux/herdr 测试全部走隔离 named session（-L muxterm-test-* /
# --session muxterm-test-*），绝不碰用户默认 server/session。
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

THREADS="${MUXTERM_TEST_THREADS:-1}"

usage() {
    sed -n '2,8p' "${BASH_SOURCE[0]}"
    exit 1
}

run_core() {
    cargo fmt --all -- --check
    cargo clippy --no-default-features --features tui -- -D warnings

    # 一次编译、一起跑：lib/bins + 全部集成测试 target。
    cargo test --no-default-features --features tui --lib --bins \
        -- --test-threads="$THREADS"
    cargo test --no-default-features --features tui \
        --test cli_integration \
        --test tmux_backend_integration \
        --test tui_integration \
        --test herdr_session_contract \
        --test herdr_feature_contract \
        --test herdr_authority_contract \
        --test herdr_stability_contract \
        --test herdr_multi_workspace_contract \
        --test herdr_worktree_contract \
        --test existing_ssh_contract \
        --test runtime_transport_matrix_contract \
        -- --test-threads="$THREADS"

    cargo test --no-default-features --features ffi \
        --test sendkeys_regression \
        --test split_regression \
        --test tui_split_ffi_regression \
        --test tui_wizard_ffi_regression \
        --test tui_wizard_ssh_ffi_regression \
        --test ffi_tmux_discovery \
        --test ssh_no_fallback \
        --test ssh_transport_unit \
        --test four_mode_integration \
        -- --test-threads="$THREADS"

    # four-mode SSH 两个 case 是 #[ignore]（需 sshd）。
    cargo test --no-default-features --features ffi \
        --test four_mode_integration -- --ignored --test-threads="$THREADS"
}

run_linux() {
    cargo clippy --features gtk -- -D warnings
    cargo check --features gtk

    # 一次编译、一起跑：全部 GTK e2e target（xvfb 下 --test-threads=1）。
    # GDK_DISABLE：禁用 GDK GL API，规避 xvfb/Mesa 下连续建窗销毁的
    # GL context double-free（SIGABRT；见 docs/HERDR-RUNTIME-STABILITY.md §GL）。
    # --jobs 1：多个 GTK target 共享同一个 Xvfb，串行跑避免窗口/GL 资源竞争。
    # xvfb-run 自带 set -e，其 kill 已退出的 Xvfb 会误报失败；必须捕获
    # cargo test 的真实退出码后再退出（否则全绿也可能 EXIT=1）。
    # 首选 -d/--auto-display（displayfd 探测空闲 display）：残留 Xvfb
    # 进程的 lock 可能已清理，-a 会撞上被占用的 display 导致新 Xvfb 立即
    # 退出（kill 失败 + EXIT=1）。但 Ubuntu 24.04 的 xvfb 21.1.12 包
    # 不支持 -d（CI 上直接报 invalid option）；探测能力后回退 -a（CI 是
    # 干净 runner，无残留 lock 问题）。
    local xvfb_opts=-d
    if ! xvfb-run --help 2>&1 | grep -q -- '--auto-display'; then
        xvfb_opts=-a
    fi
    set +e
    xvfb-run "$xvfb_opts" env GDK_DISABLE=gl-api,gles-api cargo test --features gtk --jobs 1 \
        --test linux_gtk_integration \
        --test linux_herdr_e2e \
        --test linux_herdr_switch_e2e \
        --test linux_herdr_worktree_e2e \
        --test linux_herdr_ssh_e2e \
        --test linux_herdr_agent_e2e \
        --test herdr_direct_reattach \
        --test linux_herdr_authority_e2e \
        --test linux_catalog_ssh_e2e \
        --test linux_existing_e2e \
        --test linux_fault_e2e \
        --test linux_scroll_wheel_e2e \
        --test linux_zoom_input_e2e \
        --test linux_panel_e2e \
        --test linux_render_e2e \
        --test linux_attach_history_e2e \
        --test linux_attention_semantics_e2e \
        --test linux_search_e2e \
        --test linux_runtime_transport_matrix_e2e \
        -- --test-threads="$THREADS"
    RETVAL=$?
    set -e
    echo "run linux: cargo test exit=$RETVAL" >&2
    return "$RETVAL"
}

run_macos() {
    cargo build --release --no-default-features --features ffi
    cargo test --no-default-features --features ffi \
        --test macos_integration -- --test-threads="$THREADS"
    cargo test --no-default-features --features ffi \
        --test macos_e2e -- --test-threads="$THREADS"

    # Swift 侧（headless unit tests），需 macOS + Xcode。
    if [[ "$(uname -s)" == "Darwin" ]]; then
        mkdir -p src/platform/macos/Vendor
        ln -sfn "$PWD/target/release/libmuxterm.a" src/platform/macos/Vendor/libmuxterm.a
        (cd src/platform/macos && ../../../scripts/patch-swiftterm.sh)
        (cd src/platform/macos && swift test --disable-swift-testing)
    else
        echo "run macos 需要 macOS；跳过 Swift 测试" >&2
    fi
}

doctor() {
    # 只读环境/版本/fixture capability 预检（§13.1/§13.2），不安装任何工具。
    echo "== 工具版本 =="
    echo "rustc: $(rustc --version 2>/dev/null || echo MISSING)"
    echo "cargo: $(cargo --version 2>/dev/null || echo MISSING)"
    echo "tmux: $(tmux -V 2>/dev/null || echo MISSING)"
    echo "herdr: $(herdr --version 2>/dev/null | head -1 || echo MISSING)"
    echo "sshd: $(command -v sshd 2>/dev/null || echo MISSING)"
    echo "== locale =="
    echo "LANG=$LANG LC_ALL=${LC_ALL:-unset}"
    echo "== DISPLAY / Xvfb =="
    echo "DISPLAY=${DISPLAY:-unset}"
    echo "xvfb: $(command -v xvfb-run 2>/dev/null || echo MISSING)"
    echo "== 用户默认 server 只读快照（绝不写/杀） =="
    echo "tmux sessions: $(tmux ls 2>/dev/null | wc -l)"
    echo "herdr default: $(herdr status 2>/dev/null | head -1 || echo none)"

    # required 版本（与 §13.1 一致；CI 必须精确匹配，开发机只读提示）。
    local fail=0
    if ! rustc --version 2>/dev/null | grep -q "1.97.1"; then
        echo "WARN: rustc 应为 1.97.1" >&2
        fail=1
    fi
    if ! cargo --version 2>/dev/null | grep -q "1.97.1"; then
        echo "WARN: cargo 应为 1.97.1" >&2
        fail=1
    fi
    if ! tmux -V 2>/dev/null | grep -q "3.7c"; then
        echo "WARN: tmux 应为 3.7c" >&2
        fail=1
    fi
    if ! herdr --version 2>/dev/null | grep -q "0.8.0"; then
        echo "WARN: herdr 应为 0.8.0" >&2
        fail=1
    fi
    if ! command -v sshd >/dev/null 2>&1; then
        echo "WARN: 缺 sshd（loopback SSH 格会 skip/失败）" >&2
        fail=1
    fi
    if ! command -v xvfb-run >/dev/null 2>&1; then
        echo "WARN: 缺 xvfb-run（Linux GTK e2e 需要）" >&2
        fail=1
    fi
    [ "$fail" -eq 0 ] && echo "doctor: OK" || echo "doctor: 有 WARN（见上）" >&2
}

case "${1:-}" in
doctor)
    doctor
    ;;
run)
    case "${2:-}" in
    core) run_core ;;
    linux) run_linux ;;
    macos) run_macos ;;
    *) usage ;;
    esac
    ;;
*) usage ;;
esac
