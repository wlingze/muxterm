import AppKit
import ExecutionPolicy
import MuxtermChrome

/// 包装 `EPDeveloperTool`：检查当前进程是否在「开发者工具」名单中，
/// 并在需要时登记后打开系统设置。
enum DeveloperToolAccess {
    static func status() -> DeveloperToolAccessStatus {
        map(EPDeveloperTool().authorizationStatus)
    }

    static func request(_ completion: @escaping (Bool) -> Void) {
        EPDeveloperTool().requestAccess { granted in
            completion(granted)
        }
    }

    @discardableResult
    static func openSystemSettings() -> Bool {
        for raw in DeveloperToolAccessPolicy.settingsURLs {
            if let url = URL(string: raw), NSWorkspace.shared.open(url) {
                return true
            }
        }
        return false
    }

    static func requestThenOpenSettingsIfNeeded() {
        request { granted in
            DispatchQueue.main.async {
                if DeveloperToolAccessPolicy.shouldOpenSettings(afterRequestGranted: granted) {
                    openSystemSettings()
                }
            }
        }
    }

    private static func map(_ status: EPDeveloperToolStatus) -> DeveloperToolAccessStatus {
        switch status {
        case .authorized: return .authorized
        case .denied: return .denied
        case .restricted: return .restricted
        case .notDetermined: return .notDetermined
        @unknown default: return .notDetermined
        }
    }
}
