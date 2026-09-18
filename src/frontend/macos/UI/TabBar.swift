import AppKit

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
