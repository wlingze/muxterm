import Foundation

/// 终端查询应答构建器。
///
/// 终端在收到 OSC 10/11（动态颜色查询）、CSI DA（设备属性查询）等查询时，
/// 会向宿主程序请求答案；宿主必须以正确的转义序列回复，否则引导字节（ESC）
/// 丢失会把 `10;rgb:...` 之类当普通文本/命令，出现 `zsh: command not found: 10`。
///
/// 参考 wezterm 的 `set_or_query!`（term/src/terminalstate/performer.rs）：
/// OSC 颜色查询回复为 `ESC ] <code> ; rgb:rrggbb/... ESC \`。
/// 这里提供可单测的纯函数，字节序列必须逐字节保留。
public enum TerminalQueryReply {
    public static let ESC: UInt8 = 0x1b

    /// OSC 动态颜色查询回复：`ESC ] 10 ; rgb:RR/GG/BB ESC \`
    /// - Parameters:
    ///   - code: 10=前景色, 11=背景色, 12=光标色
    ///   - hex: 6 位十六进制颜色，如 `"000000"`
    public static func oscDynamicColor(code: Int, hex: String) -> [UInt8] {
        let rgb = xtermRgb(fromHex: hex)
        var out: [UInt8] = [ESC, UInt8(ascii: "]")]
        out.append(contentsOf: Array("\(code);rgb:\(rgb)".utf8))
        out.append(ESC)
        out.append(UInt8(ascii: "\\"))
        return out
    }

    /// 把 6 位 hex 转成 xterm 的 `RRRR/GGGG/BBBB` 形式（每分量 4 位十六进制）。
    static func xtermRgb(fromHex hex: String) -> String {
        // 兼容 #rrggbb / rrggbb / #rgb / rgb
        let clean = hex.hasPrefix("#") ? String(hex.dropFirst()) : hex
        var r: String, g: String, b: String
        if clean.count == 3 {
            r = String(clean[clean.startIndex])
            g = String(clean[clean.index(clean.startIndex, offsetBy: 1)])
            b = String(clean[clean.index(clean.startIndex, offsetBy: 2)])
            r += r; g += g; b += b
        } else {
            r = String(clean.prefix(2))
            g = String(clean.dropFirst(2).prefix(2))
            b = String(clean.dropFirst(4).prefix(2))
        }
        return [r, g, b].map { $0 + $0 }.joined(separator: "/")
    }

    /// CSI 设备属性（Primary DA）查询回复：`ESC [ ? 65 ; ... c`
    /// 这里用通用 DA1 回复（带 `65` 终端标识符 + 若干属性）。
    public static func csiDeviceAttributes(attrs: [Int]) -> [UInt8] {
        let body = "?" + attrs.map(String.init).joined(separator: ";") + "c"
        var out: [UInt8] = [ESC, UInt8(ascii: "[")]
        out.append(contentsOf: Array(body.utf8))
        return out
    }

    /// 转成 Data，便于直接走 sendInput。
    public static func data(_ bytes: [UInt8]) -> Data {
        Data(bytes)
    }
}
