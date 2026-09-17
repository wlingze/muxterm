import Foundation

/// SSH/本地 attach 的 Workspace 页面进度。
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
    public static let spinnerIdentifier = "muxterm.connectProgress.spinner"

    public static func accessibilityValue(stage: ConnectProgressStage) -> String {
        stage.rawValue
    }
}
