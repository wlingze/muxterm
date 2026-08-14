import Foundation

/// status 快照查询的目标参数（纯逻辑，便于单测）。
///
/// SSH 连接的 alias 必须放在 `sshAlias`：之前它被误存进 `socket`，导致
/// 查询时执行本地 `tmux -L <alias>` 而不是 `ssh <alias> tmux ...`。
public struct StatusQueryTarget: Equatable, Sendable {
    public let socket: String?
    public let sshAlias: String?

    public static func resolve(
        backendType: String,
        socket: String?,
        sshAlias: String?
    ) -> StatusQueryTarget {
        let normalized = backendType.lowercased()
        if normalized == "ssh" {
            return StatusQueryTarget(socket: nil, sshAlias: sshAlias ?? socket)
        }
        return StatusQueryTarget(socket: socket, sshAlias: sshAlias)
    }
}

/// status bar 模式。
///
/// - `tmux`：连接 tmux 时完全采用 tmux 的 status 配置与颜色（默认，
///   有 tmux 就跟 tmux 一致）；
/// - `theme`：只用 muxterm 主题的黑/白默认色，忽略 tmux 的彩色样式。
public enum StatusBarMode: String, Sendable {
    case tmux
    case theme

    /// 从 config.toml 的 `[statusbar] mode = "tmux"|"theme"` 解析；
    /// 兼容旧名 `color_mode = "gui"`，缺省/未知回退 tmux（有 tmux 就跟 tmux）。
    public static func from(toml: String?) -> StatusBarMode {
        guard let toml else { return .tmux }
        var inStatusBar = false
        for rawLine in toml.split(separator: "\n") {
            let line = rawLine.trimmingCharacters(in: .whitespaces)
            if line.hasPrefix("[") && line.hasSuffix("]") {
                inStatusBar = line == "[statusbar]"
                continue
            }
            guard inStatusBar, let eq = line.firstIndex(of: "=") else { continue }
            let key = line[..<eq].trimmingCharacters(in: .whitespaces)
            guard key == "mode" || key == "color_mode" else { continue }
            let value = line[line.index(after: eq)...]
                .trimmingCharacters(in: .whitespaces)
                .trimmingCharacters(in: CharacterSet(charactersIn: "\""))
                .lowercased()
            switch value {
            case "theme", "muxterm", "muxterm_theme", "gui":
                return .theme
            default:
                return .tmux
            }
        }
        return .tmux
    }
}

/// status bar 横向空间预算（纯逻辑，便于单测）。
///
/// tmux 自己按 `status-left-length` / `status-right-length` 截断左右段，
/// 窗口列表始终拥有剩余空间。原生条用比例预算实现同一语义：左右各封顶、
/// 窗口列表至少保留一块可见宽度，避免长窗口名把整条 bar 撑出窗口。
public struct StatusBarWidthBudget: Equatable, Sendable {
    public let leftMax: CGFloat
    public let rightMax: CGFloat
    public let windowMin: CGFloat

    public init(leftMax: CGFloat, rightMax: CGFloat, windowMin: CGFloat) {
        self.leftMax = leftMax
        self.rightMax = rightMax
        self.windowMin = windowMin
    }
}

public enum StatusBarLayoutPolicy {
    /// left / right 段最多占整条 bar 的比例（各 36%，合 72%）。
    public static let sideMaxFraction: CGFloat = 0.36
    /// 窗口列表最少占整条 bar 的比例（28%）。
    public static let windowMinFraction: CGFloat = 0.28

    /// 按整条 bar 宽度计算左右段封顶与窗口列表最小宽度。
    public static func budget(totalWidth: CGFloat) -> StatusBarWidthBudget {
        let side = max(0, totalWidth * sideMaxFraction)
        let window = max(0, totalWidth * windowMinFraction)
        return StatusBarWidthBudget(leftMax: side, rightMax: side, windowMin: window)
    }
}

/// 提醒位（文档 §B.1）：状态栏上一个常驻位置，面积趋近于零；
/// count > 0 时变红点，表示「我是瓶颈」的工作区数量（绝不因新输出点亮）。
/// 消息弹窗 / 通知列表后续复用这个位置，这里先预留。
public struct StatusBarAttention: Equatable, Sendable {
    public let count: Int

    public init(count: Int) {
        self.count = max(0, count)
    }

    public var isActive: Bool { count > 0 }
}

/// muxterm status bar 快照（对应 Rust `StatusSnapshot` 的 JSON；连接控制模式
/// 会话时读取兼容的 status 配置，概念上属于 muxterm 自己的 status bar）。
public struct StatusBarSnapshot: Equatable, Decodable, Sendable {
    public let enabled: Bool
    public let position: String
    public let justify: String
    public let interval: UInt64
    /// 订阅推送会原地更新（`refresh-client -B` → `%subscription-changed`）。
    public var left: String
    public var right: String
    public let leftLength: Int
    public let rightLength: Int
    public let statusStyle: String
    public let leftStyle: String
    public let rightStyle: String
    public let separator: String
    public let windowFormat: String
    public let windowCurrentFormat: String
    public let windowStyle: String
    public let windowCurrentStyle: String
    public let windows: [StatusBarWindow]
    public let error: String?

    enum CodingKeys: String, CodingKey {
        case enabled, position, justify, interval, left, right
        case leftLength = "left_length"
        case rightLength = "right_length"
        case statusStyle = "status_style"
        case leftStyle = "left_style"
        case rightStyle = "right_style"
        case separator
        case windowFormat = "window_format"
        case windowCurrentFormat = "window_current_format"
        case windowStyle = "window_style"
        case windowCurrentStyle = "window_current_style"
        case windows, error
    }
}

/// status bar 里的一个窗口条目。
public struct StatusBarWindow: Equatable, Decodable, Sendable {
    public let windowId: UInt32
    public let index: UInt32
    public let name: String
    public let flags: String
    public let current: Bool
    public let text: String

    enum CodingKeys: String, CodingKey {
        case windowId = "window_id"
        case index, name, flags, current, text
    }
}

/// FFI 返回的外层响应。
public struct StatusBarResponse: Decodable, Sendable {
    public let ok: Bool
    public let error: String?
    public let status: StatusBarSnapshot?
}

/// sRGB 颜色（0…1）。
public struct StatusBarColor: Equatable, Sendable {
    public let red: Double
    public let green: Double
    public let blue: Double

    public init(red: Double, green: Double, blue: Double) {
        self.red = red
        self.green = green
        self.blue = blue
    }
}

/// 一段文本的 status bar 样式。
public struct StatusBarTextStyle: Equatable, Sendable {
    public var fg: StatusBarColor?
    public var bg: StatusBarColor?
    public var bold: Bool
    public var reverse: Bool

    public init(fg: StatusBarColor? = nil, bg: StatusBarColor? = nil, bold: Bool = false, reverse: Bool = false) {
        self.fg = fg
        self.bg = bg
        self.bold = bold
        self.reverse = reverse
    }

    public static let `default` = StatusBarTextStyle()
}

/// 解析后的文本片段：文字 + 样式。
public struct StatusBarStyledSegment: Equatable, Sendable {
    public let text: String
    public let style: StatusBarTextStyle

    public init(text: String, style: StatusBarTextStyle) {
        self.text = text
        self.style = style
    }
}

/// status bar 样式解析（纯逻辑，便于单测）。
///
/// 支持：
/// - 颜色名：`black`/`red`/…/`white`、`bright*`/`grey`；
/// - `colour0…colour255`（xterm 256 色板）；
/// - `#rgb` / `#rrggbb`；
/// - 属性：`bold`/`nobold`、`reverse`/`noreverse`、`default`；
/// - 内联指令 `#[fg=…,bg=…,bold]`（`align`/`range`/`list`/`push-default`
///   /`pop-default` 等仅用于布局的指令忽略）。
public enum StatusBarStyleParser {
    /// 解析一个 style 字符串（如 `bg=green,fg=black,bold`）。
    public static func parse(style: String) -> StatusBarTextStyle {
        var result = StatusBarTextStyle.default
        for part in style.split(separator: ",") {
            let token = part.trimmingCharacters(in: .whitespaces)
            if token.isEmpty { continue }
            if token == "default" || token == "none" {
                result = .default
                continue
            }
            if token == "bold" { result.bold = true; continue }
            if token == "nobold" { result.bold = false; continue }
            if token == "reverse" { result.reverse = true; continue }
            if token == "noreverse" { result.reverse = false; continue }
            if let eq = token.firstIndex(of: "=") {
                let key = token[..<eq].lowercased()
                let value = String(token[token.index(after: eq)...])
                if key == "fg" {
                    result.fg = color(value)
                } else if key == "bg" {
                    result.bg = color(value)
                }
            }
        }
        return result
    }

    /// 解析内联样式文本（status-left/right/window 格式），返回带样式的片段。
    public static func parseInline(text: String, base: StatusBarTextStyle = .default) -> [StatusBarStyledSegment] {
        var segments: [StatusBarStyledSegment] = []
        var current = base
        var plain = ""

        func flush() {
            if !plain.isEmpty {
                segments.append(StatusBarStyledSegment(text: plain, style: current))
                plain = ""
            }
        }

        let chars = Array(text)
        var i = 0
        while i < chars.count {
            if chars[i] == "#", i + 1 < chars.count, chars[i + 1] == "[" {
                if let end = text.range(of: "]", range: text.index(text.startIndex, offsetBy: i + 2)..<text.endIndex) {
                    flush()
                    let directiveStart = text.index(text.startIndex, offsetBy: i + 2)
                    let directive = String(text[directiveStart..<end.lowerBound])
                    current = apply(directive: directive, to: current, base: base)
                    i = text.distance(from: text.startIndex, to: end.upperBound)
                    continue
                }
            }
            plain.append(chars[i])
            i += 1
        }
        flush()
        return segments
    }

    /// 把 `#[...]` 指令作用到当前样式。
    public static func apply(directive: String, to style: StatusBarTextStyle, base: StatusBarTextStyle) -> StatusBarTextStyle {
        var result = style
        for part in directive.split(separator: ",") {
            let token = part.trimmingCharacters(in: .whitespaces)
            if token.isEmpty { continue }
            if token == "default" {
                result = base
                continue
            }
            if token == "push-default" || token == "pop-default" {
                // v1：忽略 push/pop，不维护栈
                continue
            }
            if token == "bold" { result.bold = true; continue }
            if token == "nobold" { result.bold = false; continue }
            if token == "reverse" { result.reverse = true; continue }
            if token == "noreverse" { result.reverse = false; continue }
            if let eq = token.firstIndex(of: "=") {
                let key = token[..<eq].lowercased()
                let value = String(token[token.index(after: eq)...])
                if key == "fg" {
                    result.fg = color(value)
                } else if key == "bg" {
                    result.bg = color(value)
                }
                // align/range/list/norange/nolist 等仅布局，忽略
            }
        }
        return result
    }

    /// 颜色名 → sRGB。
    public static func color(_ name: String) -> StatusBarColor? {
        let v = name.trimmingCharacters(in: .whitespaces).lowercased()
        if v == "default" { return nil }
        if v.hasPrefix("#") {
            return hexColor(String(v.dropFirst()))
        }
        // muxterm 主题色是不带 # 的 `rrggbb`。
        if v.count == 6, UInt32(v, radix: 16) != nil {
            return hexColor(v)
        }
        if v.hasPrefix("colour"), let n = Int(v.dropFirst("colour".count)) {
            return xterm256(n)
        }
        return namedColor(v)
    }

    /// xterm 256 色板。
    public static func xterm256(_ n: Int) -> StatusBarColor? {
        guard n >= 0, n <= 255 else { return nil }
        if n < 16 {
            let base: [(Double, Double, Double)] = [
                (0, 0, 0), (205, 49, 49), (13, 188, 121), (229, 229, 16),
                (36, 114, 200), (188, 63, 188), (17, 168, 205), (229, 229, 229),
                (102, 102, 102), (241, 76, 76), (35, 209, 139), (245, 245, 67),
                (59, 142, 234), (214, 112, 214), (41, 184, 219), (255, 255, 255),
            ]
            let c = base[n]
            return StatusBarColor(red: c.0 / 255, green: c.1 / 255, blue: c.2 / 255)
        }
        if n < 232 {
            let m = n - 16
            let r = (m / 36) % 6
            let g = (m / 6) % 6
            let b = m % 6
            func level(_ x: Int) -> Double {
                x == 0 ? 0 : Double(40 * x + 55) / 255
            }
            return StatusBarColor(red: level(r), green: level(g), blue: level(b))
        }
        let gray = Double(8 + (n - 232) * 10) / 255
        return StatusBarColor(red: gray, green: gray, blue: gray)
    }

    private static func namedColor(_ name: String) -> StatusBarColor? {
        switch name {
        case "black": return StatusBarColor(red: 0, green: 0, blue: 0)
        case "red": return StatusBarColor(red: 205 / 255, green: 49 / 255, blue: 49 / 255)
        case "green": return StatusBarColor(red: 13 / 255, green: 188 / 255, blue: 121 / 255)
        case "yellow": return StatusBarColor(red: 229 / 255, green: 229 / 255, blue: 16 / 255)
        case "blue": return StatusBarColor(red: 36 / 255, green: 114 / 255, blue: 200 / 255)
        case "magenta": return StatusBarColor(red: 188 / 255, green: 63 / 255, blue: 188 / 255)
        case "cyan": return StatusBarColor(red: 17 / 255, green: 168 / 255, blue: 205 / 255)
        case "white": return StatusBarColor(red: 229 / 255, green: 229 / 255, blue: 229 / 255)
        case "grey", "gray", "brightblack": return StatusBarColor(red: 102 / 255, green: 102 / 255, blue: 102 / 255)
        case "brightred": return StatusBarColor(red: 241 / 255, green: 76 / 255, blue: 76 / 255)
        case "brightgreen": return StatusBarColor(red: 35 / 255, green: 209 / 255, blue: 139 / 255)
        case "brightyellow": return StatusBarColor(red: 245 / 255, green: 245 / 255, blue: 67 / 255)
        case "brightblue": return StatusBarColor(red: 59 / 255, green: 142 / 255, blue: 234 / 255)
        case "brightmagenta": return StatusBarColor(red: 214 / 255, green: 112 / 255, blue: 214 / 255)
        case "brightcyan": return StatusBarColor(red: 41 / 255, green: 184 / 255, blue: 219 / 255)
        case "brightwhite": return StatusBarColor(red: 1, green: 1, blue: 1)
        default: return nil
        }
    }

    private static func hexColor(_ hex: String) -> StatusBarColor? {
        var h = hex
        if h.count == 3 {
            h = h.map { "\($0)\($0)" }.joined()
        }
        guard h.count == 6, let value = UInt32(h, radix: 16) else { return nil }
        return StatusBarColor(
            red: Double((value >> 16) & 0xff) / 255,
            green: Double((value >> 8) & 0xff) / 255,
            blue: Double(value & 0xff) / 255
        )
    }
}
