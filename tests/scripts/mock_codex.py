#!/usr/bin/env python3
"""Deterministic Codex-like TUI for Muxterm e2e (isolated tmux only).

Draws a CUP/alt-style frame with HEADER/BODY/PROMPT tokens, then emits
OSC 133 D + BEL so attention/notify tests can see 'task complete'.
Never talk to a real Codex. Never touch the user's default tmux server.
"""

from __future__ import annotations

import os
import shutil
import sys
import time

COLS = int(os.environ.get("MOCK_CODEX_COLS", "80"))
# 用 pane 实际行数：固定 24 在 2-pane 布局里会写到可见区之外，
# Linux e2e 的 text_format 只返回可见视口，TOKEN_PROMPT 会看不到。
ROWS = int(os.environ.get("MOCK_CODEX_ROWS", "0")) or shutil.get_terminal_size().lines
FRAMES = int(os.environ.get("MOCK_CODEX_FRAMES", "6"))
SLEEP = float(os.environ.get("MOCK_CODEX_SLEEP", "0.04"))


def draw(frame: int) -> None:
    out = sys.stdout
    out.write("\x1b[H\x1b[2J")
    out.write(f"TOKEN_HEADER mock-codex frame-{frame}\n")
    out.write("─" * min(48, max(8, COLS - 1)) + "\n")
    out.write("TOKEN_BODY agent working\n")
    out.write(f"MOCK_CODEX_FRAME={frame}\n")
    # ROWS-2：2-pane 布局里 VTE 可见区比 tmux 24 行少 2 行左右，
    # 写最后一行会把 TOKEN_PROMPT 放到可见区之外（Linux e2e 断言可见文本）。
    out.write(f"\x1b[{max(1, ROWS - 2)};1HTOKEN_PROMPT ▌")
    out.flush()


def main() -> None:
    sys.stdout.reconfigure(line_buffering=True) if hasattr(sys.stdout, "reconfigure") else None
    last = 0
    for i in range(FRAMES):
        last = i
        draw(i)
        time.sleep(SLEEP)
    draw(last)
    sys.stdout.write("\x1b]133;D;0\x07")
    sys.stdout.write("\x07")
    # 固定位置写 DONE，不追加换行：追加会滚动屏幕，把 row 1 的
    # TOKEN_HEADER 卷出 VTE 视口（Linux e2e 断言可见文本）。
    sys.stdout.write(f"\x1b[{max(1, ROWS - 3)};1HMOCK_CODEX_DONE")
    sys.stdout.flush()
    while True:
        time.sleep(3600)


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        sys.exit(0)
