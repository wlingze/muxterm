import Foundation

/// 切 tab 等待确认的门禁（纯逻辑，便于单测）。
///
/// 外部关闭 tab / 快照里已不存在 / 超时都会放行，避免 UI 一直等一个
/// 永远不会到达的 `STATE_ACTIVE_TAB_CHANGED`。
public struct TabSwitchGate {
    public let timeout: TimeInterval
    public private(set) var pendingTab: UInt32?
    public private(set) var pendingSince: Date?

    public init(timeout: TimeInterval = 1.5) {
        self.timeout = timeout
    }

    /// 发起一次切 tab：记住目标与时刻。
    public mutating func request(tab: UInt32, now: Date = Date()) {
        pendingTab = tab
        pendingSince = now
    }

    /// 收到激活 tab 变更且就是等待的目标：立即放行。
    public mutating func onTabChanged(to tab: UInt32) {
        if pendingTab == tab {
            clear()
        }
    }

    /// 等待中的 tab 被外部关闭：立即放行，不等超时。
    public mutating func onTabClosed(_ tab: UInt32) {
        if pendingTab == tab {
            clear()
        }
    }

    /// 快照更新：等待的目标已不存在（shell 退出等）→ 放行。
    public mutating func onSnapshot(tabs: [UInt32]) {
        if let pending = pendingTab, !tabs.contains(pending) {
            clear()
        }
    }

    /// 门禁是否放行：没有等待目标，或已超过超时。
    public func isReleased(now: Date = Date()) -> Bool {
        guard let pendingSince else { return true }
        return now.timeIntervalSince(pendingSince) > timeout
    }

    private mutating func clear() {
        pendingTab = nil
        pendingSince = nil
    }
}
