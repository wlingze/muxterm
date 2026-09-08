import Foundation

/// `--log-file` 打开时，AppKit / NSLog / IMK 的 stderr 应进同一个文件，
/// 不要刷到 `muxterm gui` 的终端。
public enum GuiLogFilePolicy {
    public static func shouldRedirectStandardError(logFile: String?) -> Bool {
        guard let path = logFile?.trimmingCharacters(in: .whitespacesAndNewlines),
              !path.isEmpty
        else {
            return false
        }
        return true
    }
}
