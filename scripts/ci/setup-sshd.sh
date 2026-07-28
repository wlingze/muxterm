#!/usr/bin/env bash
# CI 共享 sshd setup：启动一次 daemonized sshd，输出 KEY=VALUE 供 CI 使用。
#
# 设计约束：
# - 成功返回后 sshd 必须仍存活（不在 EXIT trap 中 kill）。
# - stdout 只输出 shell-safe 的 `KEY=VALUE` 行，不输出诊断/日志。
# - 诊断/smoke 结果写 stderr 或 log 文件。
# - 支持 MUXTERM_ENV_FILE：如果设置，追加 KEY=VALUE 到该文件（stdout 为空）。
# - 失败时清理并退出非零。
set -euo pipefail

TMP_DIR="$(mktemp -d /tmp/muxterm-sshd-ci-XXXXXX)"

# ── 失败时清理 ──
cleanup_on_error() {
    cat "$TMP_DIR/sshd.log" 2>/dev/null >&2 || true
    kill "$(cat "$TMP_DIR/sshd.pid" 2>/dev/null)" 2>/dev/null || true
    rm -rf "$TMP_DIR"
}
trap 'cleanup_on_error' ERR

# ── 生成密钥 ──
ssh-keygen -t rsa -b 2048 -f "$TMP_DIR/host_rsa" -N "" -q
ssh-keygen -t ed25519 -f "$TMP_DIR/host_ed25519" -N "" -q
ssh-keygen -t ed25519 -f "$TMP_DIR/client_ed25519" -N "" -q
cp "$TMP_DIR/client_ed25519.pub" "$TMP_DIR/authorized_keys"
chmod 600 "$TMP_DIR/authorized_keys"

# ── 找空闲端口 ──
PORT=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')

# ── sshd config ──
cat > "$TMP_DIR/sshd_config" << SSHCFG
Port $PORT
ListenAddress 127.0.0.1
HostKey $TMP_DIR/host_rsa
HostKey $TMP_DIR/host_ed25519
PidFile $TMP_DIR/sshd.pid
AuthorizedKeysFile $TMP_DIR/authorized_keys
PasswordAuthentication no
PubkeyAuthentication yes
PermitRootLogin no
UsePAM no
StrictModes no
Subsystem sftp internal-sftp
SSHCFG

# ── 启动 sshd（daemonize，不 -D）──
SSHD_BIN="$(command -v sshd || echo /usr/sbin/sshd)"
"$SSHD_BIN" -f "$TMP_DIR/sshd_config" -E "$TMP_DIR/sshd.log"

# ── 等待端口可用 ──
for i in $(seq 1 50); do
    if python3 -c "import socket; socket.socket().connect(('127.0.0.1', $PORT))" 2>/dev/null; then
        break
    fi
    sleep 0.1
done

if ! python3 -c "import socket; socket.socket().connect(('127.0.0.1', $PORT))" 2>/dev/null; then
    echo "ERROR: sshd 未在端口 $PORT 监听" >&2
    cat "$TMP_DIR/sshd.log" >&2
    exit 1
fi

# ── smoke test ──
export HOME="$TMP_DIR"
mkdir -p "$TMP_DIR/.ssh"
cat > "$TMP_DIR/.ssh/config" << SMOKECFG
Host test-smoke
    HostName 127.0.0.1
    Port $PORT
    User $(whoami)
    IdentityFile $TMP_DIR/client_ed25519
    IdentitiesOnly yes
    BatchMode yes
    StrictHostKeyChecking no
    UserKnownHostsFile /dev/null
    LogLevel ERROR
SMOKECFG
chmod 600 "$TMP_DIR/.ssh/config"

if ! timeout 10 ssh -F "$TMP_DIR/.ssh/config" test-smoke "echo ok" >/dev/null 2>&1; then
    echo "ERROR: sshd smoke test 失败" >&2
    cat "$TMP_DIR/sshd.log" >&2
    exit 1
fi

# ── 移除 ERR trap（成功返回不清理）──
trap - ERR

# ── 输出 KEY=VALUE ──
ENV_LINES="MUXTERM_TEST_SSH_HOST=127.0.0.1
MUXTERM_TEST_SSH_PORT=$PORT
MUXTERM_TEST_SSH_USER=$(whoami)
MUXTERM_TEST_SSH_KEY=$TMP_DIR/client_ed25519
MUXTERM_SSHD_TMP_DIR=$TMP_DIR
MUXTERM_SSHD_LOG=$TMP_DIR/sshd.log"

if [ -n "${MUXTERM_ENV_FILE:-}" ]; then
    echo "$ENV_LINES" >> "$MUXTERM_ENV_FILE"
else
    echo "$ENV_LINES"
fi

echo "SSH smoke test passed on port $PORT" >&2
