#!/usr/bin/env python3
"""DECSTBM + 底行 LF：复现 emulate.rs resize 后 soft-wrap 越界。

隔离 tmux 用。窗口变高后 core 若没同步 grid_soft_wrapped，feed 这些字节
会 panic，muxterm_poll_events 丢掉整批 %output，GUI 只剩半截。
"""

from __future__ import annotations

import shutil
import sys
import time


def main() -> None:
    size = shutil.get_terminal_size()
    rows = max(8, size.lines)
    cols = max(20, size.columns)
    out = sys.stdout
    out.write("\x1b[H\x1b[2J")
    out.write("AGENT_TOP\n")
    # 部分滚动区（不是整屏 1;rows），逼出 linefeed 的 insert(bottom) 路径。
    out.write(f"\x1b[2;{rows}r")
    out.write(f"\x1b[{rows};1H")
    for i in range(rows):
        out.write(f"SCROLL-{i}".ljust(min(cols - 1, 20)) + "\n")
    out.write("FULL_AGENT_FRAME\n")
    out.flush()
    while True:
        time.sleep(3600)


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        sys.exit(0)
