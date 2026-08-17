import Foundation

/// Cmd-Enter 按当前表面分流，避免注意力缩放覆盖主窗口 tmux zoom。
///
/// - 主窗口：tmux `resize-pane -Z`（`CmdEnterKeyE2ETests` 必须保持绿）
/// - 注意力面板：打开独立 replica overlay（`muxterm.replyOverlay`）
/// - overlay 已打开：再按一次关掉
public enum CmdEnterSurface: Equatable, Sendable {
    case mainWindow
    case attentionPanel
    case replyOverlay
}

public enum CmdEnterAction: Equatable, Sendable {
    case toggleTmuxZoom
    case openReplyOverlay
    case closeReplyOverlay
}

public enum CmdEnterRouting {
    public static let overlayIdentifier = "muxterm.replyOverlay"

    public static func action(on surface: CmdEnterSurface) -> CmdEnterAction {
        switch surface {
        case .mainWindow:
            return .toggleTmuxZoom
        case .attentionPanel:
            return .openReplyOverlay
        case .replyOverlay:
            return .closeReplyOverlay
        }
    }
}
