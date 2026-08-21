#!/usr/bin/env bash
# 给 SwiftTerm 1.15.0 打 Muxterm 补丁：
# 1) 绘制时 Minimum Contrast（黑底黑字）
# 2) doCommand 处理 deleteToBeginningOfLine / noop（不再 Unhandle print）
# 3) 暴露 scrollWheel override，允许 Muxterm 仅对滚轮临时启用 TUI mouse protocol
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MACOS_DIR="$ROOT/src/platform/macos"
CHECKOUT="$MACOS_DIR/.build/checkouts/SwiftTerm"
APPLE="$CHECKOUT/Sources/SwiftTerm/Apple/AppleTerminalView.swift"
MAC="$CHECKOUT/Sources/SwiftTerm/Mac/MacTerminalView.swift"

if [[ ! -f "$APPLE" ]]; then
  echo "==> resolving SwiftTerm (checkout missing)"
  (cd "$MACOS_DIR" && swift package resolve)
fi
if [[ ! -f "$APPLE" || ! -f "$MAC" ]]; then
  echo "ERROR: SwiftTerm sources not found under $CHECKOUT" >&2
  exit 1
fi
chmod u+w "$APPLE" "$MAC"

python3 - "$APPLE" "$MAC" <<'PY'
import pathlib, sys

apple = pathlib.Path(sys.argv[1])
mac = pathlib.Path(sys.argv[2])
text = apple.read_text()
if "MUXTERM_MIN_CONTRAST" not in text:
    anchor = """        var fgColor = mapColor (color: fg, isFg: true, isBold: isBold, useBrightColors: useBrightColors)
        let bgColor = mapColor (color: bg, isFg: false, isBold: false)
        // Apply dim/faint attribute (SGR 2)
        if flags.contains (.dim) {
            fgColor = fgColor.dimmedColor (towards: bgColor)
        }
"""
    insert = anchor + "        fgColor = muxtermMinContrast(fg: fgColor, bg: bgColor) // MUXTERM_MIN_CONTRAST\n"
    if anchor not in text:
        print("ERROR: SwiftTerm getAttributes color mapping changed; update scripts/patch-swiftterm.sh", file=sys.stderr)
        sys.exit(1)
    text = text.replace(anchor, insert, 1)
    helper = '''
#if os(macOS)
/// Muxterm：格子前景相对背景对比不足时抬亮/压暗（iTerm2 Minimum Contrast）。
func muxtermMinContrast(fg: TTColor, bg: TTColor) -> TTColor {
    guard let f = fg.usingColorSpace(.sRGB), let b = bg.usingColorSpace(.sRGB) else {
        return fg
    }
    func lin(_ v: CGFloat) -> CGFloat {
        v <= 0.04045 ? v / 12.92 : pow((v + 0.055) / 1.055, 2.4)
    }
    func lum(_ r: CGFloat, _ g: CGFloat, _ bl: CGFloat) -> CGFloat {
        0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(bl)
    }
    func ratio(_ a: CGFloat, _ b: CGFloat) -> CGFloat {
        let hi = max(a, b)
        let lo = min(a, b)
        return (hi + 0.05) / (lo + 0.05)
    }
    let fr = f.redComponent, fg_ = f.greenComponent, fb = f.blueComponent
    let br = b.redComponent, bg_ = b.greenComponent, bb = b.blueComponent
    if ratio(lum(fr, fg_, fb), lum(br, bg_, bb)) >= 3.0 {
        return fg
    }
    let target: CGFloat = lum(br, bg_, bb) < 0.5 ? 1 : 0
    var lo: CGFloat = 0
    var hi: CGFloat = 1
    var best = (target, target, target)
    for _ in 0..<14 {
        let t = (lo + hi) / 2
        let r = fr + (target - fr) * t
        let g = fg_ + (target - fg_) * t
        let bl = fb + (target - fb) * t
        if ratio(lum(r, g, bl), lum(br, bg_, bb)) >= 3.0 {
            best = (r, g, bl)
            hi = t
        } else {
            lo = t
        }
    }
    return TTColor(srgbRed: best.0, green: best.1, blue: best.2, alpha: 1)
}
#endif
'''
    needle = """    public func selectNone () {
        selection.selectNone()
    }

}
"""
    if needle not in text:
        print("ERROR: SwiftTerm AppleTerminalView class ending changed; update scripts/patch-swiftterm.sh", file=sys.stderr)
        sys.exit(1)
    text = text.replace(needle, needle + helper, 1)
    apple.write_text(text)
    print("==> applied SwiftTerm min-contrast patch")
else:
    print("==> SwiftTerm min-contrast patch already applied")

mac_text = mac.read_text()
if "MUXTERM_DOCOMMAND" not in mac_text:
    old = """        case #selector(moveToRightEndOfLine(_:)):
            send (EscapeSequences.emacsForward)
        default:
            print ("Unhandle selector \\(selector)")
        }
"""
    new = """        case #selector(moveToRightEndOfLine(_:)):
            send (EscapeSequences.emacsForward)
        default:
            // MUXTERM_DOCOMMAND
            switch NSStringFromSelector(selector) {
            case "deleteToBeginningOfLine:":
                send([0x15])
            case "deleteToEndOfLine:":
                send([0x0b])
            default:
                break
            }
        }
"""
    if old not in mac_text:
        print("ERROR: SwiftTerm doCommand default changed; update scripts/patch-swiftterm.sh", file=sys.stderr)
        sys.exit(1)
    mac.write_text(mac_text.replace(old, new, 1))
    print("==> applied SwiftTerm doCommand patch")
else:
    print("==> SwiftTerm doCommand patch already applied")

mac_text = mac.read_text()
if "MUXTERM_SCROLL_WHEEL" not in mac_text:
    old = "    public override func scrollWheel(with event: NSEvent) {\n"
    new = "    open override func scrollWheel(with event: NSEvent) { // MUXTERM_SCROLL_WHEEL\n"
    if old not in mac_text:
        print("ERROR: SwiftTerm scrollWheel declaration changed; update scripts/patch-swiftterm.sh", file=sys.stderr)
        sys.exit(1)
    mac.write_text(mac_text.replace(old, new, 1))
    print("==> applied SwiftTerm scroll-wheel override patch")
else:
    print("==> SwiftTerm scroll-wheel override patch already applied")
PY
