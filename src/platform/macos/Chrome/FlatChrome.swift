import Foundation

#if canImport(CoreGraphics)
import CoreGraphics
#else
public typealias CGFloat = Double
#endif

/// macOS 极简 chrome 尺寸与文案（对齐 `assets/style.css` / iTerm2 Minimal）。
///
/// 纯数值与格式化，无 AppKit 依赖，便于 `swift test` 回归。
public enum FlatChrome {
    /// Core 事件轮询间隔；约 60Hz，避免分割/tab/输入在 100ms 定时器下产生明显滞后。
    public static let eventPollInterval: TimeInterval = 1.0 / 60.0
    /// Tab 栏高度（ARCHITECTURE：≤ 24px）
    public static let tabBarHeight: CGFloat = 24
    /// 状态栏高度（一行小字，不抢终端空间）
    public static let statusBarHeight: CGFloat = 18
    /// Pane 分割线厚度（细分隔，非卡片边框）
    public static let splitDividerThickness: CGFloat = 1
    /// 活跃 pane 指示边框（1px，避免 2px「卡片」感）
    public static let activePaneBorderWidth: CGFloat = 1
    /// 活跃 tab 底边指示线
    public static let activeTabUnderlineHeight: CGFloat = 2
    /// 「+」新建 tab 按钮宽度
    public static let newTabButtonWidth: CGFloat = 28
    /// 状态栏左右内边距
    public static let statusHorizontalInset: CGFloat = 6
    /// Tab 单元左右内边距（文字扫描用）
    public static let tabCellHorizontalInset: CGFloat = 8

    /// 紧凑状态栏文案。保留 XCUITest 解析 token：`connected` / `tabs: N` / `panes: N` / `pane: @N`。
    public static func statusText(
        status: String,
        tabCount: Int,
        paneCount: Int,
        activePane: UInt32,
        tabsLabel: String,
        panesLabel: String,
        paneLabel: String
    ) -> String {
        "\(status)  \(tabsLabel): \(tabCount)  \(panesLabel): \(paneCount)  \(paneLabel): @\(activePane)"
    }
}

/// Muxterm 终端默认外观颜色（深色主题，与 configs/themes/dark.toml 一致）。
///
/// codex / cursor agent 的输入框固定用深色背景，并把输入文字画成「默认前景色」；
/// 终端必须上报浅色前景，文字才能在灰/黑输入框上清晰可见（与 iTerm 深色下的
/// 渲染一致）。这套颜色同时作为 OSC 10/11 上报给 tmux 代答。
public enum MuxtermTerminalColors {
    /// 前景（默认文字）`#cdd6f4`。
    public static let foregroundHex = "cdd6f4"
    /// 背景 `#1e1e2e`。
    public static let backgroundHex = "1e1e2e"
    /// 浅色主题前景/背景（默认）。
    public static let lightForegroundHex = "000000"
    public static let lightBackgroundHex = "ffffff"
    /// 当前生效调色板（默认浅色；可在 config.toml `[theme] name` 切换）。
    public static var activePalette: (fg: String, bg: String) = (lightForegroundHex, lightBackgroundHex)

    /// 根据 `[theme] name` 返回调色板；dark 用浅字深底，其它/缺省用黑字白底。
    public static func palette(forThemeName name: String?) -> (fg: String, bg: String) {
        switch name?.trimmingCharacters(in: .whitespacesAndNewlines).lowercased() {
        case "dark":
            return (foregroundHex, backgroundHex)
        default:
            return (lightForegroundHex, lightBackgroundHex)
        }
    }

    /// 从 config.toml 文本中读取 `[theme]` 下的 `name = "..."`。
    public static func themeName(from toml: String) -> String? {
        var inTheme = false
        for rawLine in toml.split(separator: "\n") {
            let line = rawLine.trimmingCharacters(in: .whitespaces)
            if line.hasPrefix("[") && line.hasSuffix("]") {
                inTheme = line == "[theme]"
                continue
            }
            guard inTheme, let eq = line.firstIndex(of: "=") else { continue }
            let key = line[..<eq].trimmingCharacters(in: .whitespaces)
            guard key == "name" else { continue }
            let value = line[line.index(after: eq)...]
                .trimmingCharacters(in: .whitespaces)
                .trimmingCharacters(in: CharacterSet(charactersIn: "\""))
            return value
        }
        return nil
    }
}

/// muxterm 主题（纯逻辑，便于单测）：浅色是默认，深色用于 codex/agent。
public enum MuxtermTheme: String, CaseIterable, Sendable {
    case light
    case dark

    public var displayName: String {
        switch self {
        case .light: return "Light"
        case .dark: return "Dark"
        }
    }

    public var palette: (fg: String, bg: String) {
        MuxtermTerminalColors.palette(forThemeName: rawValue)
    }

    /// 从 config.toml `[theme] name` 或 UserDefaults 值解析；缺省/未知回退浅色。
    public static func from(name: String?) -> MuxtermTheme {
        switch name?.trimmingCharacters(in: .whitespacesAndNewlines).lowercased() {
        case "dark", "dark.toml": return .dark
        default: return .light
        }
    }
}

/// 终端字体配置与缩放（纯逻辑，便于单测）。
///
/// 默认参考 alacritty 配置：Menlo 18pt。`[font]` 段来自
/// `~/.config/muxterm/config.toml`，Cmd +/- / Cmd 0 在运行期缩放字号。
public enum MuxtermTerminalFont {
    public struct Settings: Equatable, Sendable {
        public var family: String
        public var size: CGFloat

        public init(family: String = MuxtermTerminalFont.defaultFamily,
                    size: CGFloat = MuxtermTerminalFont.defaultSize) {
            self.family = family
            self.size = size
        }
    }

    public static let defaultFamily = "Menlo"
    public static let defaultSize: CGFloat = 18
    public static let minSize: CGFloat = 9
    public static let maxSize: CGFloat = 36
    public static let zoomStep: CGFloat = 1

    public static func settings(from toml: String?) -> Settings {
        guard let toml else { return Settings() }
        var inFont = false
        var family = defaultFamily
        var size = defaultSize
        for rawLine in toml.split(separator: "\n") {
            let line = rawLine.trimmingCharacters(in: .whitespaces)
            if line.hasPrefix("[") && line.hasSuffix("]") {
                inFont = line == "[font]"
                continue
            }
            guard inFont, let eq = line.firstIndex(of: "=") else { continue }
            let key = line[..<eq].trimmingCharacters(in: .whitespaces)
            let rawValue = line[line.index(after: eq)...]
                .trimmingCharacters(in: .whitespaces)
            switch key {
            case "family":
                family = unquote(rawValue)
            case "size":
                if let v = Double(unquote(rawValue)) {
                    size = CGFloat(v)
                }
            default:
                break
            }
        }
        return Settings(family: family.isEmpty ? defaultFamily : family, size: clamp(size))
    }

    /// 从当前字号按方向缩放（+1 增大 / -1 减小），并夹在合法区间。
    public static func zoomed(_ size: CGFloat, direction: Int) -> CGFloat {
        clamp(size + CGFloat(direction) * zoomStep)
    }

    public static func clamp(_ size: CGFloat) -> CGFloat {
        min(max(size, minSize), maxSize)
    }

    private static func unquote(_ raw: String) -> String {
        var v = raw.trimmingCharacters(in: .whitespaces)
        if v.hasPrefix("\""), v.hasSuffix("\""), v.count >= 2 {
            v.removeFirst()
            v.removeLast()
        }
        return v
    }
}

/// 主配置 `~/.config/muxterm/config.toml` 的轻量读取（纯逻辑，便于单测）。
public enum MuxtermConfig {
    /// 连接池 warm slot 上限；与 QuickConnect 的 recent 展示上限对齐。
    public static let defaultPoolMaxSlots = 5

    /// 读取 `[pool] max_slots`；缺省 / 非法回退默认值。
    public static func poolMaxSlots(from toml: String?) -> Int {
        guard let toml else { return defaultPoolMaxSlots }
        var inPool = false
        for rawLine in toml.split(separator: "\n") {
            let line = rawLine.trimmingCharacters(in: .whitespaces)
            if line.hasPrefix("[") && line.hasSuffix("]") {
                inPool = line == "[pool]"
                continue
            }
            guard inPool, let eq = line.firstIndex(of: "=") else { continue }
            let key = line[..<eq].trimmingCharacters(in: .whitespaces)
            guard key == "max_slots" else { continue }
            let rawValue = line[line.index(after: eq)...]
                .trimmingCharacters(in: .whitespaces)
            if let v = Int(rawValue), v >= 1 {
                return v
            }
        }
        return defaultPoolMaxSlots
    }
}

/// tmux 控制模式的终端应答策略。
///
/// tmux / daemon 代理拥有 pane 的 PTY 与终端协议，且 tmux 自己会代答
/// OSC 10/11/12、CSI DA/DSR 等查询（颜色来自客户端用 `refresh-client -r`
/// 上报的 `control_fg`/`control_bg`）。前端只是渲染镜像：SwiftTerm 在
/// feed 远端 pane 输出期间生成的查询应答**一律丢弃**，否则经
/// `send-keys -l` 回写会被 pane 回显并执行，泄漏成 `git lg` 的
/// `10;rgb:...` / `65;...c` 字面命令。
///
/// 用户输入（键盘/kitty/粘贴）与鼠标上报不在 feed 窗口内，不受影响；
/// 仅 local 模式（前端就是该 PTY 的终端模拟器）保持转发。
public enum TerminalMirrorPolicy {
    public static func shouldForwardParserResponse(
        duringRemoteOutputFeed: Bool,
        isTmuxMirror: Bool
    ) -> Bool {
        // tmux 镜像在 feed 远端输出期间，解析器应答一律丢弃：tmux 自己
        // 代答查询，前端回写只会被 pane 回显（git lg 字面乱码）。
        !isTmuxMirror || !duringRemoteOutputFeed
    }
}

/// 检测一段 pane 输出里是否包含「终端查询」序列。
///
/// 目前仅用于诊断/测试：tmux 控制模式下查询由 tmux 自己代答，前端不再
/// 据此放行 SwiftTerm 的解析器应答。
public enum TerminalQueryDetector {
    /// 支持的前缀。
    public enum QueryKind: Equatable {
        case oscDynamicColor(Int)   // OSC 10/11/12 ?
        case csiDeviceAttributes    // CSI c / CSI ? ... c
        case csiDeviceStatus        // CSI n / CSI 5 n / CSI 6 n
        case kittyKeyboard          // CSI ? u / CSI > 4;... u
    }

    /// 扫描字节流，返回找到的查询类型（去重，保序）。
    public static func queries(in bytes: [UInt8]) -> [QueryKind] {
        guard !bytes.isEmpty else { return [] }
        var found: [QueryKind] = []
        var i = 0
        while i < bytes.count {
            guard bytes[i] == 0x1b else {
                i += 1
                continue
            }
            guard i + 1 < bytes.count else { break }
            let next = bytes[i + 1]
            if next == UInt8(ascii: "]") {
                // OSC：ESC ] <code> ; ? <ST|BEL>
                if let (kind, consumed) = parseOSCQuery(bytes, from: i) {
                    if !found.contains(where: { label($0) == label(kind) }) {
                        found.append(kind)
                    }
                    i += consumed
                    continue
                }
            } else if next == UInt8(ascii: "[") {
                // CSI
                if let (kind, consumed) = parseCSIQuery(bytes, from: i) {
                    if !found.contains(where: { label($0) == label(kind) }) {
                        found.append(kind)
                    }
                    i += consumed
                    continue
                }
            }
            i += 2
        }
        return found
    }

    /// 是否存在任意查询（供 feed 门禁用）。
    public static func containsQuery(in bytes: [UInt8]) -> Bool {
        !queries(in: bytes).isEmpty
    }

    private static func label(_ kind: QueryKind) -> String {
        switch kind {
        case .oscDynamicColor(let code): return "osc\(code)"
        case .csiDeviceAttributes: return "da"
        case .csiDeviceStatus: return "dsr"
        case .kittyKeyboard: return "kitty"
        }
    }

    /// 解析 OSC 查询：ESC ] 10 ; ? 或 ESC ] 11;?，以 BEL/ST 结尾。
    private static func parseOSCQuery(_ bytes: [UInt8], from start: Int) -> (QueryKind, Int)? {
        var i = start + 2
        // 数字 code（10/11/12）
        let codeStart = i
        while i < bytes.count, bytes[i].isASCIIDigit {
            i += 1
        }
        guard i > codeStart, i < bytes.count else { return nil }
        let code = Int(String(bytes: Array(bytes[codeStart..<i]), encoding: .ascii) ?? "")
        guard let code, code == 10 || code == 11 || code == 12 else { return nil }
        // 允许可选空格
        while i < bytes.count, bytes[i] == UInt8(ascii: " ") { i += 1 }
        guard i < bytes.count, bytes[i] == UInt8(ascii: ";") else { return nil }
        i += 1
        while i < bytes.count, bytes[i] == UInt8(ascii: " ") { i += 1 }
        guard i < bytes.count, bytes[i] == UInt8(ascii: "?") else { return nil }
        i += 1
        // 跳过直到 BEL / ST
        while i < bytes.count, bytes[i] != 0x07 {
            if bytes[i] == 0x1b, i + 1 < bytes.count, bytes[i + 1] == UInt8(ascii: "\\") {
                i += 2
                return (.oscDynamicColor(code), i - start)
            }
            i += 1
        }
        guard i < bytes.count else { return nil }
        return (.oscDynamicColor(code), i - start + 1)
    }

    /// 解析 CSI 查询：ESC [ c / ESC [ ? ... c / ESC [ n / ESC [ ? ... u。
    private static func parseCSIQuery(_ bytes: [UInt8], from start: Int) -> (QueryKind, Int)? {
        var i = start + 2
        var sawQuestion = false
        if i < bytes.count, bytes[i] == UInt8(ascii: "?") {
            sawQuestion = true
            i += 1
        }
        // kitty 能力查询形如 `CSI > 4;0 u`：`>` 后跟数字/分号。
        var sawGreater = false
        if i < bytes.count, bytes[i] == UInt8(ascii: ">") {
            sawGreater = true
            i += 1
        }
        let paramsStart = i
        while i < bytes.count, bytes[i].isASCIIDigit || bytes[i] == UInt8(ascii: ";") {
            i += 1
        }
        guard i < bytes.count else { return nil }
        let final = bytes[i]
        let params = Array(bytes[paramsStart..<i])
        switch final {
        case UInt8(ascii: "c"):
            return (.csiDeviceAttributes, i - start + 1)
        case UInt8(ascii: "n"):
            return (.csiDeviceStatus, i - start + 1)
        case UInt8(ascii: "u"):
            // kitty keyboard 查询：CSI ? u 或 CSI > 4;... u
            if sawQuestion || sawGreater {
                return (.kittyKeyboard, i - start + 1)
            }
            return nil
        default:
            return nil
        }
    }
}

private extension UInt8 {
    var isASCIIDigit: Bool {
        self >= UInt8(ascii: "0") && self <= UInt8(ascii: "9")
    }
}


/// PaneOutput 事件的喂入策略。
///
/// 参考成熟终端设计：后端 `%output` 事件流是权威增量，累计快照只用于
/// 新建视图的播种。视图首次创建时，播种快照（后端最近 256KB）已覆盖
/// 后端已入队但尚未派发的事件；这些事件必须跳过，否则同一批字节会双写
/// （输入/回显重复）。快照为空（新 pane 首批字节）时没有任何覆盖，事件
/// 必须原样喂入。视图已存在时事件就是纯增量，直接喂入。
public enum PaneOutputFeedPolicy {
    public static func shouldFeedEvent(
        viewExistedBeforeEvent: Bool,
        seedCoveredEvent: Bool
    ) -> Bool {
        viewExistedBeforeEvent || !seedCoveredEvent
    }
}

/// 从终端模型生成「当前屏幕文本」（AX / UI 测试用）。
///
/// 之前把每次 feed 的原始字节累积成 AX 文本，输入/状态区的每一帧中间状态
/// 都会留在里面，看起来像逐帧堆叠的历史；正确做法是只反映当前屏幕。
public enum ScreenText {
    /// 按行列读取器生成屏幕行：逐格取字符、行尾去空白、去掉末尾空行。
    /// `characterAt` 的列/行均为 0 基。
    public static func lines(
        cols: Int,
        rows: Int,
        characterAt: (Int, Int) -> Character
    ) -> [String] {
        guard cols > 0, rows > 0 else { return [] }
        var out: [String] = []
        out.reserveCapacity(rows)
        for y in 0..<rows {
            var line = ""
            line.reserveCapacity(cols)
            for x in 0..<cols {
                line.append(characterAt(x, y))
            }
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            out.append(trimmed)
        }
        while out.last?.isEmpty == true {
            out.removeLast()
        }
        return out
    }
}
