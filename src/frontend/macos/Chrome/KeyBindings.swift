import Foundation

/// macOS 客户端快捷键动作（与菜单 / Cmd 映射一致）。
public enum KeyAction: Equatable, Hashable, Sendable {
    case newTab
    case splitHorizontal
    case splitVertical
    case splitAuto
    case closeWindow
    case closePane
    case switchTab(Int) // 1-based
    case switchLastTab
    /// 当前 tab 的前一个 / 后一个。Cmd-1..9 盖不住第 10 个以后的 tab。
    case previousTab
    case nextTab
    case nextPane
    case prevPane
    /// 在 OSC 133 命令刻度之间向前/向后跳转；不向 pane 发送按键。
    case previousCommand
    case nextCommand
    case commandPalette
    case quickConnect
    case attention
    case searchWorkspace
    case searchGlobal
    case quit
    case increaseFontSize
    case decreaseFontSize
    case resetFontSize
    case togglePaneFullscreen
    /// macOS 原生整个窗口全屏，与 pane zoom 分开。
    case toggleWindowFullscreen
    /// 只在当前桌面里把窗口铺满，不进入全屏 Space。
    case toggleWindowZoom
    case openSettings
    case cycleLayout
    case toggleSidebar
    /// 进入前端 Shells 聚合槽；不会向终端发送清屏字符。
    case openShells
    /// 进入前端 Agents 聚合槽；只投影已有源 Tab。
    case openAgents
    case switchWorkspace(Int) // 1-based opened order; 0 is always last
}

/// 修饰键 + 主键（大小写无关）的纯数据描述。
public struct KeyChord: Equatable, Hashable, Sendable {
    public var command: Bool
    public var shift: Bool
    public var option: Bool
    public var control: Bool
    public var key: String

    public init(
        command: Bool = false,
        shift: Bool = false,
        option: Bool = false,
        control: Bool = false,
        key: String
    ) {
        self.command = command
        self.shift = shift
        self.option = option
        self.control = control
        self.key = key.lowercased()
    }
}

/// 快捷键 → 动作 纯函数表。Cmd+[ / Cmd+] 切 pane 必须保持不变。
public enum KeyBindings {
    /// 解析键位；`custom` 来自 `~/.config/muxterm/config.toml` 的 `[[keybindings]]`，
    /// 命中时优先于内置默认。
    public static func action(
        for chord: KeyChord,
        custom: [KeyChord: KeyAction]? = nil,
        suppressed: Set<KeyAction> = []
    ) -> KeyAction? {
        if let custom, let action = custom[chord] {
            return action
        }
        if let builtin = action(for: chord), suppressed.contains(builtin) {
            return nil
        }
        return action(for: chord)
    }

    public static func action(for chord: KeyChord) -> KeyAction? {
        let key = chord.key

        // Cmd+T 新建 tab
        if chord.command, !chord.shift, !chord.option, key == "t" {
            return .newTab
        }
        // Cmd-D 左右、Cmd-Shift-D 上下；Cmd-S 上下、Cmd-Shift-S 左右。
        // Cmd-V 必须留给粘贴。自动切分不再占用默认键。
        if chord.command, !chord.option, !chord.control {
            if key == "d" {
                return chord.shift ? .splitVertical : .splitHorizontal
            }
            if key == "s" {
                return chord.shift ? .splitHorizontal : .splitVertical
            }
        }
        // Cmd+W 只关当前层；整个窗口显式使用 Cmd+Shift+W。
        if chord.command, !chord.option, !chord.control, key == "w" {
            return chord.shift ? .closeWindow : .closePane
        }
        // Cmd+1..9 切 tab；Cmd+Ctrl+N 留给 Workspace 切换。
        if chord.command, !chord.option, !chord.shift, !chord.control,
           let n = Int(key), (1...9).contains(n)
        {
            return .switchTab(n)
        }
        if chord.command, !chord.option, !chord.shift, !chord.control, key == "0" {
            return .switchLastTab
        }
        // Cmd-Left / Cmd-Right：相邻 tab。第 10 个以后没有数字键。
        if chord.command, !chord.option, !chord.shift, !chord.control {
            if key == "left" {
                return .previousTab
            }
            if key == "right" {
                return .nextTab
            }
        }
        // Cmd+[ / Cmd+]：上一个 / 下一个 pane（焦点跟随）
        if chord.command, !chord.shift, !chord.option, key == "[" {
            return .prevPane
        }
        if chord.command, !chord.shift, !chord.option, key == "]" {
            return .nextPane
        }

        // Cmd+Option+↑/↓：在 shell 命令时间线中前后跳转。保留普通
        // ↑/↓、Cmd+↑/↓ 给 SwiftTerm / shell，避免破坏 TUI 输入语义。
        if chord.command, chord.option, !chord.shift, !chord.control, key == "up" {
            return .previousCommand
        }
        if chord.command, chord.option, !chord.shift, !chord.control, key == "down" {
            return .nextCommand
        }

        // Cmd+Ctrl+A/S：进入 Agents / Shells 前端聚合槽。
        if chord.command, chord.control, !chord.option, !chord.shift {
            if key == "a" { return .openAgents }
            if key == "s" { return .openShells }
        }

        // Cmd+P：QuickConnect 面板。右侧 workspace 图标隐藏，但快捷键保留。
        if chord.command, !chord.shift, !chord.option, !chord.control, key == "p" {
            return .quickConnect
        }

        // Cmd+Shift+P：旧命令面板（保留）。
        if chord.command, chord.shift, !chord.option, key == "p" {
            return .commandPalette
        }
        // Cmd+R：通知 / Attention 面板。
        if chord.command, !chord.shift, !chord.option, !chord.control, key == "r" {
            return .attention
        }
        // Cmd+F：当前 Workspace
        if chord.command, !chord.shift, !chord.option, key == "f" {
            return .searchWorkspace
        }
        // Cmd+= / Cmd++ 增大字体，Cmd+- 减小，Cmd+Shift+0 重置。
        if chord.command, !chord.option {
            if key == "=" || key == "+" {
                return .increaseFontSize
            }
            if key == "-" {
                return .decreaseFontSize
            }
            if key == "0", chord.shift, !chord.control {
                return .resetFontSize
            }
        }
        if chord.control, !chord.command, !chord.shift, !chord.option, key == "0" {
            return .resetFontSize
        }
        // Cmd+Shift+L：布局格式循环（tmux next-layout / 左右变上下）。
        if chord.command, chord.shift, !chord.option, !chord.control, key == "l" {
            return .cycleLayout
        }
        // Cmd-Shift-F/E/Enter：整个 macOS 窗口全屏。
        // Cmd-Shift-T/G：当前桌面内最大化。Colemak 和 QWERTY 是同一物理键的两个字母。
        // Cmd/Alt-Enter 仍只放大 pane。
        if chord.command, !chord.option, !chord.control {
            if chord.shift, ["f", "e", "\r", "\n"].contains(key) {
                return .toggleWindowFullscreen
            }
            if chord.shift, key == "t" || key == "g" {
                return .toggleWindowZoom
            }
            if !chord.shift, key == "," {
                return .openSettings
            }
        }
        if key == "\r" || key == "\n" {
            if !chord.shift, !chord.control,
               (chord.command && !chord.option) || (chord.option && !chord.command) {
                return .togglePaneFullscreen
            }
        }

        // Alt+T / Alt+S / Alt+V / Alt+[ / Alt+] / Alt+1..9（兼容 TUI）
        if chord.option, !chord.command {
            if key == "t" { return .newTab }
            if key == "s" { return .splitHorizontal }
            if key == "v" { return .splitVertical }
            if key == "[" { return .prevPane }
            if key == "]" { return .nextPane }
            if key == "0" { return .switchLastTab }
            if let n = Int(key), (1...9).contains(n) {
                return .switchTab(n)
            }
        }

        // Cmd-B 切换主窗口侧边栏。
        if chord.command, !chord.shift, !chord.option, !chord.control, key == "b" {
            return .toggleSidebar
        }

        // Cmd+Ctrl+1..9 固定打开顺序；Cmd+Ctrl+0 永远最后一个。
        if chord.command, chord.control, !chord.shift, !chord.option {
            if key == "0" {
                return .switchWorkspace(0)
            }
            if let n = Int(key), (1...9).contains(n) {
                return .switchWorkspace(n)
            }
        }

        // Ctrl+Q 退出。Ctrl+D 不属于窗口快捷键：它必须作为 EOF
        // 发送给当前 pane 的前台进程，由 shell/程序决定是否退出。
        if chord.control, !chord.command {
            if key == "q" { return .quit }
        }

        return nil
    }
}

/// 数字键只覆盖前 9 个 tab。第 10 个及以后靠相邻偏移，到头就停。
public enum TabSwitchPolicy {
    /// `activeIndex` 是 0-based。`offset` 为 0 或越界时返回 nil，不环绕。
    public static func adjacentIndex(activeIndex: Int, count: Int, offset: Int) -> Int? {
        guard count > 0, (0..<count).contains(activeIndex), offset != 0 else { return nil }
        let destination = activeIndex + offset
        guard (0..<count).contains(destination) else { return nil }
        return destination
    }
}
