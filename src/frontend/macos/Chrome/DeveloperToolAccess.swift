import Foundation

/// macOS「隐私与安全性 → 开发者工具」状态。对应 ExecutionPolicy 的
/// `EPDeveloperToolStatus`，但不依赖该框架，便于纯逻辑测试。
public enum DeveloperToolAccessStatus: String, Equatable, Sendable {
    case notDetermined
    case restricted
    case denied
    case authorized
}

/// 检查/请求 Developer Tools 的纯策略：系统 API 不会弹框，只把 App
/// 登记进设置列表，未授权时再打开对应面板让用户打开开关。
public enum DeveloperToolAccessPolicy {
    public static let settingsURLs = [
        "x-apple.systempreferences:com.apple.preference.security?Privacy_DevTools",
        "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_DevTools",
    ]

    public static func isAuthorized(_ status: DeveloperToolAccessStatus) -> Bool {
        status == .authorized
    }

    public static func shouldOpenSettings(afterRequestGranted granted: Bool) -> Bool {
        !granted
    }

    public static func actionTitle(authorized: Bool) -> String {
        authorized ? "Open System Settings" : "Request Access…"
    }
}
