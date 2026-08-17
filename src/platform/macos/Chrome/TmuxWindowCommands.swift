import Foundation

/// 把 iTerm2 的 tab/pane 手势映射到 tmux 命令。不要再发明一层窗口层次。
///
/// Muxterm：tmux window = tab，tmux pane = pane。
///
/// iTerm2 证据（2026-08-17）：
/// - 拖 tab 排序：`PseudoTerminal moveTabAtIndex:toIndex:` /
///   `tabBarControl moveTabAtIndex:toTabBar:atIndex:`
///   https://github.com/gnachman/iTerm2/blob/f243568d/sources/TerminalView/MovePaneController.m
/// - 把 session/pane 拖成新 tab：`MoveSessionToNewTabBuiltInFunction`；
///   tmux 客户端走 `TmuxController breakOutWindowPane:`（`break-pane`）
///   https://github.com/gnachman/iTerm2/blob/ea21d790/sources/TmuxController.h
/// - pane 挪到另一个 split：`movePane:intoPane:`（tmux `move-pane`）
public enum TmuxWindowCommands {
    /// 重排 tab = `move-window`。
    public static let reorderWindows = "move-window"
    /// 把 pane 拆成新 tab = `break-pane`。
    public static let paneToNewWindow = "break-pane"
    /// pane 挪进另一个 split = `move-pane`。
    public static let movePane = "move-pane"

    /// `-s :from -t :to`（tmux window index）。
    public static func moveWindowArgs(fromIndex: Int, toIndex: Int) -> [String] {
        [reorderWindows, "-s", ":\(fromIndex)", "-t", ":\(toIndex)"]
    }

    /// `-s %pane`：该 pane 变成新 window/tab。
    public static func breakPaneArgs(pane: String) -> [String] {
        [paneToNewWindow, "-s", pane]
    }
}
