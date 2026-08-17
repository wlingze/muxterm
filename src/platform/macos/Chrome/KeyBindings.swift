import Foundation

/// macOS 客户端快捷键动作（与菜单 / Cmd 映射一致）。
public enum KeyAction: Equatable, Sendable {
    case newTab
    case splitHorizontal
    case splitVertical
    case closeWindow
    case closePane
    case switchTab(Int) // 1-based
    case nextPane
    case prevPane
    case commandPalette
    case quickConnect
    case searchPanes
    case quit
    case increaseFontSize
    case decreaseFontSize
    case resetFontSize
    case togglePaneFullscreen
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
    public static func action(for chord: KeyChord, custom: [KeyChord: KeyAction]? = nil) -> KeyAction? {
        if let custom, let action = custom[chord] {
            return action
        }
        return action(for: chord)
    }

    public static func action(for chord: KeyChord) -> KeyAction? {
        let key = chord.key

        // Cmd+T 新建 tab
        if chord.command, !chord.shift, !chord.option, key == "t" {
            return .newTab
        }
        // Cmd+D 上下（竖直）/ Cmd+Shift+D 水平
        if chord.command, !chord.option, key == "d" {
            return chord.shift ? .splitHorizontal : .splitVertical
        }
        // Cmd+W 关窗口
        if chord.command, !chord.shift, !chord.option, key == "w" {
            return .closeWindow
        }
        // Cmd+1..9 切 tab
        if chord.command, !chord.option, !chord.shift,
           let n = Int(key), (1...9).contains(n)
        {
            return .switchTab(n)
        }
        // Cmd+[ / Cmd+]：上一个 / 下一个 pane（焦点跟随）
        if chord.command, !chord.shift, !chord.option, key == "[" {
            return .prevPane
        }
        if chord.command, !chord.shift, !chord.option, key == "]" {
            return .nextPane
        }

        // Cmd+P：QuickConnect 面板（Recent + Project）。
        if chord.command, !chord.shift, !chord.option, key == "p" {
            return .quickConnect
        }
        // Cmd+Shift+P：旧命令面板（保留）。
        if chord.command, chord.shift, !chord.option, key == "p" {
            return .commandPalette
        }
        // Cmd+Shift+F：搜索 pane 文本。
        if chord.command, chord.shift, !chord.option, key == "f" {
            return .searchPanes
        }
        // Cmd+= / Cmd++ 增大字体，Cmd+- 减小，Cmd+0 重置。
        if chord.command, !chord.option {
            if key == "=" || key == "+" {
                return .increaseFontSize
            }
            if key == "-" {
                return .decreaseFontSize
            }
            if key == "0" {
                return .resetFontSize
            }
        }
        // Cmd+Enter / Alt+Enter：当前 pane 全屏切换（tmux `resize-pane -Z` / 本地布局）。
        if !chord.shift, key == "\r" || key == "\n" {
            if chord.command, !chord.option {
                return .togglePaneFullscreen
            }
            if chord.option, !chord.command {
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
            if let n = Int(key), (1...9).contains(n) {
                return .switchTab(n)
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
