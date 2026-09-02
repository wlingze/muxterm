#!/usr/bin/env python3
"""Mouse-reporting TUI fixture for Muxterm (isolated tmux / Herdr only).

Mimics Grok/htop/vim: CSI ? 1003h + 1006h + 2004h on the primary screen.
Prints tokens for:
  - SGR wheel 64/65 → MOUSE_WHEEL_UP / MOUSE_WHEEL_DOWN
  - SGR hover 35    → MOUSE_HOVER
  - SGR click 0     → MOUSE_CLICK
  - bracketed paste → PASTE_OK:<text>
  - key y           → OSC 52 copy of OSC52_TOKEN

Never talk to the user's default tmux/herdr server.
"""

from __future__ import annotations

import os
import select
import shutil
import sys
import tty
import termios

READY = os.environ.get("MOUSE_TUI_READY", "MOUSE_TUI_READY")
OSC52_TOKEN = os.environ.get("MOUSE_TUI_OSC52", "OSC52_TOKEN")
ENABLE = "\x1b[?1003h\x1b[?1006h\x1b[?2004h"
DISABLE = "\x1b[?1003l\x1b[?1006l\x1b[?2004l"


def draw() -> None:
    cols, rows = shutil.get_terminal_size((80, 24))
    sys.stdout.write("\x1b[H\x1b[2J")
    sys.stdout.write(f"{READY} mouse=1003,1006 paste=2004\r\n")
    sys.stdout.write(f"grid={cols}x{rows}\r\n")
    sys.stdout.write("y=OSC52  wheel/click/hover echoed below\r\n")
    sys.stdout.flush()


def emit_osc52() -> None:
    import base64

    payload = base64.b64encode(OSC52_TOKEN.encode()).decode("ascii")
    sys.stdout.write(f"\x1b]52;c;{payload}\x07")
    sys.stdout.flush()


def main() -> None:
    fd = sys.stdin.fileno()
    old = termios.tcgetattr(fd)
    tty.setraw(fd)
    sys.stdout.write(ENABLE)
    draw()
    buf = b""
    try:
        while True:
            ready, _, _ = select.select([sys.stdin], [], [], 0.25)
            if not ready:
                continue
            chunk = os.read(fd, 256)
            if not chunk:
                break
            buf += chunk
            while buf:
                if buf.startswith(b"\x1b[<"):
                    end = buf.find(b"M")
                    if end < 0:
                        end = buf.find(b"m")
                    if end < 0:
                        break
                    report = buf[: end + 1]
                    buf = buf[end + 1 :]
                    body = report[3:-1].decode("ascii", "replace")
                    parts = body.split(";")
                    btn = parts[0] if parts else ""
                    if btn == "64":
                        sys.stdout.write("MOUSE_WHEEL_UP\r\n")
                    elif btn == "65":
                        sys.stdout.write("MOUSE_WHEEL_DOWN\r\n")
                    elif btn == "35":
                        sys.stdout.write("MOUSE_HOVER\r\n")
                    elif btn in ("0", "2"):
                        sys.stdout.write("MOUSE_CLICK\r\n")
                    sys.stdout.flush()
                elif buf.startswith(b"\x1b[200~"):
                    end = buf.find(b"\x1b[201~")
                    if end < 0:
                        break
                    text = buf[6:end].decode("utf-8", "replace")
                    buf = buf[end + 6 :]
                    sys.stdout.write(f"PASTE_OK:{text}\r\n")
                    sys.stdout.flush()
                elif buf[:1] in (b"y", b"Y"):
                    buf = buf[1:]
                    emit_osc52()
                elif buf[:1] in (b"q", b"Q", b"\x03"):
                    return
                else:
                    buf = buf[1:]
    finally:
        sys.stdout.write(DISABLE)
        sys.stdout.flush()
        termios.tcsetattr(fd, termios.TCSADRAIN, old)


if __name__ == "__main__":
    main()
