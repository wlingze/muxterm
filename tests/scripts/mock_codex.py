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
HISTORY_LINES = int(os.environ.get("MOCK_CODEX_HISTORY_LINES", "0"))
HISTORY_TOKEN = os.environ.get("MOCK_CODEX_HISTORY_TOKEN", "TOKEN_HISTORY")
DYNAMIC_SIZE = os.environ.get("MOCK_CODEX_DYNAMIC_SIZE", "0") == "1"


def screen_size() -> tuple[int, int]:
    if DYNAMIC_SIZE:
        size = shutil.get_terminal_size()
        return max(8, size.columns), max(8, size.lines)
    return COLS, ROWS


def draw(frame: int) -> None:
    cols, rows = screen_size()
    out = sys.stdout
    out.write("\x1b[H\x1b[2J")
    out.write(f"TOKEN_HEADER mock-codex frame-{frame}\n")
    out.write("─" * min(48, max(8, cols - 1)) + "\n")
    out.write("TOKEN_BODY agent working\n")
    out.write(f"MOCK_CODEX_FRAME={frame}\n")
    # rows-2：2-pane 布局里 VTE 可见区比 tmux 24 行少 2 行左右，
    # 写最后一行会把 TOKEN_PROMPT 放到可见区之外（Linux e2e 断言可见文本）。
    out.write(f"\x1b[{max(1, rows - 2)};1HTOKEN_PROMPT ▌")
    out.flush()


def draw_done() -> None:
    _, rows = screen_size()
    # 固定位置写 DONE，不追加换行：追加会滚动屏幕，把 row 1 的
    # TOKEN_HEADER 卷出可见区（Linux e2e 断言可见文本）。
    sys.stdout.write(f"\x1b[{max(1, rows - 3)};1HMOCK_CODEX_DONE")
    sys.stdout.flush()


def main() -> None:
    sys.stdout.reconfigure(line_buffering=True) if hasattr(sys.stdout, "reconfigure") else None
    for i in range(HISTORY_LINES):
        marker = HISTORY_TOKEN if i == 0 else f"history-{i:03d}"
        sys.stdout.write(f"{marker} previous agent message\n")
    sys.stdout.flush()

    last = 0
    for i in range(FRAMES):
        last = i
        draw(i)
        time.sleep(SLEEP)
    draw(last)
    sys.stdout.write("\x1b]133;D;0\x07")
    sys.stdout.write("\x07")
    draw_done()
    last_size = screen_size()
    while True:
        time.sleep(0.05 if DYNAMIC_SIZE else 3600)
        if DYNAMIC_SIZE and screen_size() != last_size:
            # Cursor/pi 会在 SIGWINCH 后按新网格重画。这条分支让
            # attach E2E 覆盖“首次进入即 resize”，不依赖测试后改窗口。
            last_size = screen_size()
            draw(last)
            draw_done()


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        sys.exit(0)
