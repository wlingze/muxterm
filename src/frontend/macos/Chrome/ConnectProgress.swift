import Foundation

/// SSH/本地 attach 的全窗口进度（盖住主内容，不是小对话框）。
///
/// 阶段名对齐生产日志：resolving / ssh / list-sessions / attach / capture。
public enum ConnectProgressStage: String, CaseIterable, Sendable {
    case resolving
    case ssh
    case listSessions = "list-sessions"
    case attach
    case capture
}

public enum ConnectProgress {
    public static let identifier = "muxterm.connectProgress"

    public static func accessibilityValue(stage: ConnectProgressStage) -> String {
        stage.rawValue
    }
}
