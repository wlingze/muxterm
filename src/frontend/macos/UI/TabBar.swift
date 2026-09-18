import AppKit
import MuxtermChrome

/// Shells / Agents 固定槽共用的 chrome 视觉身份。只装饰侧边栏、快速面板、
/// 状态栏和 Tab，不改变终端 Surface 的 ANSI palette。
enum AggregateWorkspaceAppearance: String {
    case shells
    case agents

    init?(workspaceID: String) {
        switch workspaceID {
        case AggregateWorkspaceIdentity.shells: self = .shells
        case AggregateWorkspaceIdentity.agents: self = .agents
        default: return nil
        }
    }

    init?(presentation: StatusBarWorkspacePresentation) {
        switch presentation {
        case .shells: self = .shells
        case .agents: self = .agents
        case .workspace, .opening: return nil
        }
    }

    var accentColor: NSColor {
        switch self {
        case .shells: .systemTeal
        case .agents: .systemPurple
        }
    }

    var symbol: String {
        switch self {
        case .shells: "S"
        case .agents: "A"
        }
    }
}

/// Tab 栏位置。值来自统一 config.toml 的 `[ui] tab_bar_position`，由
/// MainWindow 在启动和切换时通过 Core 事务读写；这里不再使用 UserDefaults。
enum TabBarPosition: String {
    case top
    case bottom
}

/// Tab 宽度策略。`equal_width` 使用 iTerm2 风格铺满可用区域，`compact`
/// 保留固定宽度并在 Tab 多时截断。
enum TabBarStyle: String {
    case equalWidth = "equal_width"
    case compact
}
