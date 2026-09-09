import Foundation

/// macOS NSEvent 的 Ctrl 组合到终端控制字节的纯映射。
///
/// SwiftTerm 通常会做同样的转换，但 Muxterm 的输入必须经过 tmux
/// `WriteRaw`，因此在平台边界保留一个可测试的明确协议。
public enum TerminalInputEncoding {
    public static let backspaceByte: UInt8 = 0x7f

    public static func controlByte(for key: String) -> UInt8? {
        let bytes = Array(key.utf8)
        guard bytes.count == 1 else { return nil }
        let byte = bytes[0]
        switch byte {
        case 0x01...0x1a:
            // 某些输入法/测试工具已经把 charactersIgnoringModifiers
            // 变成控制码，直接保留，避免二次转换。
            return byte
        case 0x41...0x5a:
            return byte &- 0x40
        case 0x61...0x7a:
            return byte &- 0x60
        case 0x00, 0x20, 0x32, 0x60: // NUL / Ctrl+2 / Ctrl+`
            return 0x00
        case 0x5b, 0x33: // ESC / Ctrl+3
            return 0x1b
        case 0x5c, 0x34: // FS / Ctrl+4
            return 0x1c
        case 0x5d, 0x35: // GS / Ctrl+5
            return 0x1d
        case 0x5e, 0x7e, 0x36: // RS / Ctrl+6 / Ctrl+~
            return 0x1e
        case 0x5f, 0x2f, 0x37: // US / Ctrl+/ / Ctrl+7
            return 0x1f
        case 0x3f, 0x38: // DEL / Ctrl+?
            return backspaceByte
        default:
            return nil
        }
    }
}
