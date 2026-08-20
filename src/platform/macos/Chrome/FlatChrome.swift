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

/// 一套完整终端调色板：默认前景/背景/光标 + ANSI 16 色（与 `configs/themes/*.toml` 对齐）。
///
/// 主题与终端内所有颜色绑定：浅色 chrome 必须配浅色终端（白底黑字），
/// 不能只改边框而把 SwiftTerm 留在深色默认上。
public struct MuxtermPalette: Equatable, Sendable {
    public let fg: String
    public let bg: String
    public let cursor: String
    /// 16 个 ANSI 色，hex 不含 `#`。
    public let ansi: [String]

    public init(fg: String, bg: String, cursor: String, ansi: [String]) {
        self.fg = fg
        self.bg = bg
        self.cursor = cursor
        self.ansi = ansi
    }

    /// 浅色：黑字白底 + Catppuccin Latte ANSI 16 色。
    public static let light = MuxtermPalette(
        fg: MuxtermTerminalColors.lightForegroundHex,
        bg: MuxtermTerminalColors.lightBackgroundHex,
        cursor: "dc8a78",
        ansi: [
            "bcc0cc", "d20f39", "40a02b", "df8e1d",
            "1e66f5", "ea76cb", "179299", "5c5f77",
            "6c6f85", "d20f39", "40a02b", "df8e1d",
            "1e66f5", "ea76cb", "179299", "acb0be",
        ]
    )

    /// 深色：Catppuccin Mocha（`configs/themes/dark.toml`）。
    public static let dark = MuxtermPalette(
        fg: MuxtermTerminalColors.foregroundHex,
        bg: MuxtermTerminalColors.backgroundHex,
        cursor: "f5e0dc",
        ansi: [
            "45475a", "f38ba8", "a6e3a1", "f9e2af",
            "89b4fa", "f5c2e7", "94e2d5", "bac2de",
            "585b70", "f38ba8", "a6e3a1", "f9e2af",
            "89b4fa", "f5c2e7", "94e2d5", "a6adc8",
        ]
    )
}

/// Muxterm 终端默认外观颜色。
///
/// 浅色是默认：白底黑字，跟 light chrome 绑定。深色用 Catppuccin Mocha，
/// 同时作为 OSC 10/11 上报给 tmux 代答。
public enum MuxtermTerminalColors {
    /// 深色前景（默认文字）`#cdd6f4`。
    public static let foregroundHex = "cdd6f4"
    /// 深色背景 `#1e1e2e`。
    public static let backgroundHex = "1e1e2e"
    /// 浅色主题前景/背景（默认）。
    public static let lightForegroundHex = "000000"
    public static let lightBackgroundHex = "ffffff"
    /// 当前生效调色板（默认浅色；可在 config.toml `[theme] name` 切换）。
    public static var activePalette: MuxtermPalette = .light

    /// 根据 `[theme] name` 返回调色板；dark 用浅字深底，其它/缺省用黑字白底。
    public static func palette(forThemeName name: String?) -> MuxtermPalette {
        switch name?.trimmingCharacters(in: .whitespacesAndNewlines).lowercased() {
        case "dark":
            return .dark
        default:
            return .light
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

    public var palette: MuxtermPalette {
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
/// 上报的 `control_fg`/`control_bg`）。前端只是渲染镜像：SwiftTerm 的
/// `send(source: Terminal)` **只有解析器应答**（没有用户按键），tmux
/// 镜像下必须一律丢弃。否则 OSC/DA 经 `send-keys` 写进 pane，zsh 会
/// 把 `10;rgb:...` 当命令执行，prompt_subst / Pure 模板也会被打乱。
///
/// 用户输入走 `send(source: TerminalView)`，不受这条策略影响；
/// 仅 local 模式（前端就是该 PTY 的终端模拟器）保持转发。
public enum TerminalMirrorPolicy {
    public static func shouldForwardParserResponse(
        duringRemoteOutputFeed: Bool,
        isTmuxMirror: Bool
    ) -> Bool {
        _ = duringRemoteOutputFeed
        return !isTmuxMirror
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
        while i < bytes.count, bytes[i].isASCIIDigit || bytes[i] == UInt8(ascii: ";") {
            i += 1
        }
        guard i < bytes.count else { return nil }
        let final = bytes[i]
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

    /// native SwiftTerm scrollback 在 `userScrolling` 状态下仍须接收 live。
    /// viewport 只决定显示 yDisp，不能成为丢弃 `%output` 的门禁。
    public static func shouldFeedLive(viewport: UInt32) -> Bool {
        _ = viewport
        return true
    }
}

/// 历史查看策略：触控板/PageUp 交给 SwiftTerm 或前台 TUI；应用自身不拦截
/// 事件，也不通过 PaneBuf dump 替换 live 屏幕。
public enum PaneHistoryScrollPolicy {
    /// 禁止从 live TUI 手里抢触控板/滚轮。
    public static let stealsLiveTrackpad = false

    /// 禁止 PageUp/PageDown 改 viewport（htop/Cursor 自己要这些键）。
    public static let stealsLivePageKeys = false

    /// 搜索跳转也只移动 native scroll position；不能把 core 历史帧重播到
    /// 正在工作的 VT，否则会清掉 live 光标/alternate-screen 状态。
    public static func shouldReplaceLiveScreen(isSearchJump: Bool) -> Bool {
        _ = isSearchJump
        return false
    }

    /// `deltaLines > 0` 往历史上滚（macOS `scrollingDeltaY > 0`）。
    public static func nextOffset(
        current: UInt32,
        deltaLines: Int,
        maxOffset: UInt32
    ) -> UInt32 {
        let next = Int64(current) + Int64(deltaLines)
        if next <= 0 {
            return 0
        }
        if next >= Int64(maxOffset) {
            return maxOffset
        }
        return UInt32(next)
    }

    /// 把滚轮/触控板 delta 收成整数行，余数留在 accumulator。
    public static func lines(
        deltaY: CGFloat,
        precise: Bool,
        cellHeight: CGFloat,
        accumulator: inout CGFloat
    ) -> Int {
        if deltaY == 0 {
            return 0
        }
        let cell = max(cellHeight, 1)
        if precise {
            accumulator += deltaY
            let n = Int(accumulator / cell)
            accumulator -= CGFloat(n) * cell
            return n
        }
        accumulator = 0
        let rounded = Int(deltaY.rounded())
        if rounded != 0 {
            return rounded
        }
        return deltaY > 0 ? 1 : -1
    }
}

/// last-seen 跳转的纯状态判定。
///
/// `rawOffset == -1` 表示 core 已经淘汰了旧 seq；此时必须清掉按钮，
/// 不能沿用上一轮缓存的 offset 误跳到历史或 live 尾部。
public enum LastSeenNavigation {
    public static func targetOffset(
        latest: Int64,
        seen: UInt64?,
        rawOffset: Int32
    ) -> UInt32? {
        guard latest >= 0,
              let seen,
              UInt64(latest) > seen,
              rawOffset >= 0
        else {
            return nil
        }
        return UInt32(rawOffset)
    }
}

/// 首屏 / 直播喂给 SwiftTerm 的字节：内置 VT 只交出可见缓冲，禁止把
/// `capture-pane -S -10000` 或 256KB 环当历史重放（iTerm2 也不会这么做）。
///
/// Cursor/Codex 每帧 `CSI H`+`CSI 2J`；一整段重放就是「从很早刷到现在」。
public enum PanePaintPolicy {
    /// 优先用 PaneBuf 的可见网格 ANSI；没有再从原始字节抽末帧 / 末 N 行。
    public static func firstPaint(visible: Data, raw: Data, rows: Int) -> Data {
        if !visible.isEmpty {
            return visible
        }
        return lastScreen(raw, visibleRows: max(rows, 1))
    }

    /// 已有视图收到事件：只走 live。禁止把 Codex/htop 的增量当 capture
    /// 再 `RIS`+可见网格替换，否则正在刷的 GitHub 地址 / htop 画面会被擦掉。
    public static func paint(
        seeded: Bool,
        visible: Data,
        incoming: Data,
        rows: Int
    ) -> Data {
        if !seeded {
            return firstPaint(visible: visible, raw: incoming, rows: rows)
        }
        return live(incoming, visibleRows: max(rows, 1))
    }

    /// `capture-pane -S -10000` 一类录像：行数远超一屏。仅用于首屏策略测试。
    public static func looksLikeHistoryDump(_ data: Data, rows: Int) -> Bool {
        if data.isEmpty {
            return false
        }
        if frameCount(data) >= 2 {
            return false
        }
        let rowCount = max(rows, 1)
        let lines = splitLines(data)
        if lines.count > rowCount * 2 {
            return true
        }
        return data.count > 64 * 1024 && lines.count > rowCount
    }

    /// 直播增量必须保持 tmux `%output` 的原始字节顺序。
    ///
    /// 一个事件可能从 alternate-screen 进入、光标定位、清屏和绘制正文
    /// 中间任意位置开始；按 `CSI H`/`CSI 2J`/`?1049h` 找“最后一帧”会
    /// 丢掉仍然属于同一 VT 状态机的前缀，Cursor/Codex 就会跳屏或把输入
    /// 区逐行堆到 scrollback。首屏历史裁剪只允许发生在 `firstPaint`。
    public static func live(_ data: Data, visibleRows: Int = 24) -> Data {
        _ = visibleRows
        return data
    }

    /// 首屏且没有可见网格时：丢掉 capture 历史，只留末屏。
    public static func lastScreen(_ data: Data, visibleRows: Int) -> Data {
        if data.isEmpty {
            return data
        }
        if frameCount(data) >= 2 {
            return lastVisibleFrame(data)
        }
        let rows = max(visibleRows, 1)
        let lines = splitLines(data)
        if lines.count > rows {
            return joinLines(Array(lines.suffix(rows)))
        }
        return data
    }

    /// 从最后一个帧起点（CSI H / CSI 2J / alt-screen）切到末尾。
    public static func lastVisibleFrame(_ data: Data) -> Data {
        let bytes = [UInt8](data)
        var last: Int?
        var i = 0
        while i < bytes.count {
            if isFrameStart(bytes, i) {
                last = i
            }
            i += 1
        }
        guard let start = last else { return data }
        return Data(bytes[start...])
    }

    static func frameCount(_ data: Data) -> Int {
        let bytes = [UInt8](data)
        var starts = 0
        var seenContent = false
        var i = 0
        while i < bytes.count {
            if isFrameStart(bytes, i) {
                if starts == 0 || seenContent {
                    starts += 1
                }
                seenContent = false
                i += 2
                let rest = bytes[i...]
                for (j, b) in rest.enumerated() {
                    if b == UInt8(ascii: "H") || b == UInt8(ascii: "J")
                        || b == UInt8(ascii: "h") || b == UInt8(ascii: "l")
                    {
                        i += j + 1
                        break
                    }
                }
                continue
            }
            if bytes[i] != 0x1b {
                seenContent = true
            }
            i += 1
        }
        return starts
    }

    private static func isFrameStart(_ bytes: [UInt8], _ i: Int) -> Bool {
        guard i + 1 < bytes.count, bytes[i] == 0x1b, bytes[i + 1] == UInt8(ascii: "[") else {
            return false
        }
        let rest = bytes[(i + 2)...]
        if rest.starts(with: [UInt8(ascii: "H")]) { return true }
        if rest.starts(with: [UInt8(ascii: "1"), UInt8(ascii: ";"), UInt8(ascii: "1"), UInt8(ascii: "H")]) {
            return true
        }
        if rest.starts(with: [UInt8(ascii: "2"), UInt8(ascii: "J")]) { return true }
        if rest.starts(with: Array("?1049h".utf8)) { return true }
        if rest.starts(with: Array("?1049l".utf8)) { return true }
        return false
    }

    private static func splitLines(_ data: Data) -> [Data] {
        var lines: [Data] = []
        var current = Data()
        for b in data {
            current.append(b)
            if b == 0x0a {
                lines.append(current)
                current = Data()
            }
        }
        if !current.isEmpty {
            lines.append(current)
        }
        return lines
    }

    private static func joinLines(_ lines: [Data]) -> Data {
        var out = Data()
        out.reserveCapacity(lines.reduce(0) { $0 + $1.count })
        for line in lines {
            out.append(line)
        }
        return out
    }
}

/// 前景相对背景对比度不够时，把前景往黑或往白推（对标 iTerm2 Minimum Contrast）。
///
/// 浅色主题 000000/ffffff 本身对比充足，不会改。Cursor 输入框常用黑底 + 默认前景：
/// 若把 OSC 10 报成纯黑，字就叠在黑底上；上报前再对黑底做一次保证。
public enum ColorContrast {
    public static let minimumRatio: Double = 3.0

    public struct RGB: Equatable {
        public var r: Double
        public var g: Double
        public var b: Double
    }

    public static func parse(_ hex: String) -> RGB? {
        let value = hex.trimmingCharacters(in: CharacterSet.alphanumerics.inverted).lowercased()
        guard value.count == 6, let n = UInt32(value, radix: 16) else { return nil }
        return RGB(
            r: Double((n >> 16) & 0xff) / 255.0,
            g: Double((n >> 8) & 0xff) / 255.0,
            b: Double(n & 0xff) / 255.0
        )
    }

    public static func hex(_ rgb: RGB) -> String {
        let r = Int((rgb.r * 255.0).rounded())
        let g = Int((rgb.g * 255.0).rounded())
        let b = Int((rgb.b * 255.0).rounded())
        return String(format: "%02x%02x%02x", max(0, min(255, r)), max(0, min(255, g)), max(0, min(255, b)))
    }

    public static func relativeLuminance(_ c: RGB) -> Double {
        func lin(_ v: Double) -> Double {
            v <= 0.04045 ? v / 12.92 : pow((v + 0.055) / 1.055, 2.4)
        }
        return 0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
    }

    public static func contrastRatio(fg: String, bg: String) -> Double {
        guard let a = parse(fg), let b = parse(bg) else { return 1 }
        return contrastRatio(a, b)
    }

    public static func contrastRatio(_ a: RGB, _ b: RGB) -> Double {
        let l1 = relativeLuminance(a)
        let l2 = relativeLuminance(b)
        let hi = max(l1, l2)
        let lo = min(l1, l2)
        return (hi + 0.05) / (lo + 0.05)
    }

    public static func ensureReadable(
        fg: String,
        bg: String,
        minRatio: Double = minimumRatio
    ) -> String {
        guard let f = parse(fg), let b = parse(bg) else { return fg }
        return hex(ensureReadable(fg: f, bg: b, minRatio: minRatio))
    }

    public static func ensureReadable(
        fg: RGB,
        bg: RGB,
        minRatio: Double = minimumRatio
    ) -> RGB {
        if contrastRatio(fg, bg) >= minRatio {
            return fg
        }
        let target = relativeLuminance(bg) < 0.5
            ? RGB(r: 1, g: 1, b: 1)
            : RGB(r: 0, g: 0, b: 0)
        var lo = 0.0
        var hi = 1.0
        var best = target
        for _ in 0..<14 {
            let t = (lo + hi) / 2
            let mixed = RGB(
                r: fg.r + (target.r - fg.r) * t,
                g: fg.g + (target.g - fg.g) * t,
                b: fg.b + (target.b - fg.b) * t
            )
            if contrastRatio(mixed, bg) >= minRatio {
                best = mixed
                hi = t
            } else {
                lo = t
            }
        }
        return best
    }

    /// 终端默认色：只保证相对主题背景可读（浅色仍是黑字白底）。
    public static func themeColors(fg: String, bg: String) -> (fg: String, bg: String) {
        (ensureReadable(fg: fg, bg: bg), bg)
    }

    /// 上报给 tmux 的 OSC 10/11：必须是主题色本身。
    ///
    /// 不能为了 Cursor 黑底输入框把前景改成灰色：`refresh-client -r`
    /// 会写进 session，普通 `tmux attach` 里默认字也会变成白/灰。
    /// 黑底黑字只在 SwiftTerm 绘制时做 Minimum Contrast。
    public static func oscColors(fg: String, bg: String) -> (fg: String, bg: String) {
        themeColors(fg: fg, bg: bg)
    }
}

extension MuxtermPalette {
    /// 主题色若前景/背景过近，只推前景。
    public func contrasted() -> MuxtermPalette {
        let pair = ColorContrast.themeColors(fg: fg, bg: bg)
        guard pair.fg != fg else { return self }
        return MuxtermPalette(fg: pair.fg, bg: pair.bg, cursor: cursor, ansi: ansi)
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
