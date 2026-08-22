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
    cargo test --no-default-features --features tui --lib --bins
    cargo test --no-default-features --features tui \
        --test cli_integration \
        --test tmux_backend_integration \
        --test tui_integration \
        --test herdr_session_contract \
        --test herdr_feature_contract \
        --test herdr_authority_contract \
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
    xvfb-run -a cargo test --features gtk \
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

case "${1:-}" in
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
