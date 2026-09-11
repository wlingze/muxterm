import Foundation

#if canImport(CoreGraphics)
import CoreGraphics
#else
public typealias CGFloat = Double
#endif

/// OSC 133 命令刻度：滚动条一侧的覆盖层，不占 tmux 字符格。
public struct CommandMarkTick: Equatable, Sendable {
    public enum Kind: Equatable, Sendable {
        case ok
        case fail
        case pending
    }

    public let seq: UInt64
    public let command: String
    public let exitCode: Int?
    public let offset: UInt32

    public init(seq: UInt64, command: String, exitCode: Int?, offset: UInt32) {
        self.seq = seq
        self.command = command
        self.exitCode = exitCode
        self.offset = offset
    }

    public var kind: Kind {
        guard let code = exitCode else { return .pending }
        return code == 0 ? .ok : .fail
    }
}

public struct PlacedCommandMark: Equatable, Sendable {
    public let mark: CommandMarkTick
    /// AppKit 坐标：0 在底部。offset 0（最新）靠近底部，越大越靠上。
    public let y: CGFloat

    public init(mark: CommandMarkTick, y: CGFloat) {
        self.mark = mark
        self.y = y
    }
}

public enum CommandMarkRailLayout {
    /// 默认近乎不可见；超过一个字符格宽算违规。
    public static let collapsedWidth: CGFloat = 8
    public static let expandedWidth: CGFloat = 14
    public static let tickHeight: CGFloat = 3
    public static let inset: CGFloat = 10

    public static func isVisible(markCount: Int) -> Bool {
        markCount > 0
    }

    public static func width(expanded: Bool) -> CGFloat {
        expanded ? expandedWidth : collapsedWidth
    }

    /// `maxOffset` 是当前 scrollback 从尾到顶的距离；0 表示全部挤在底部。
    public static func place(
        marks: [CommandMarkTick],
        maxOffset: UInt32,
        height: CGFloat,
        inset: CGFloat = inset
    ) -> [PlacedCommandMark] {
        guard height > 0, !marks.isEmpty else { return [] }
        let span = max(maxOffset, marks.map(\.offset).max() ?? 0)
        let usable = max(height - inset * 2, 1)
        return marks.map { mark in
            let ratio = span == 0 ? 0 : CGFloat(mark.offset) / CGFloat(span)
            let y = inset + ratio * usable
            return PlacedCommandMark(mark: mark, y: y)
        }
    }

    public static func hitTest(
        ticks: [PlacedCommandMark],
        y: CGFloat,
        slop: CGFloat = 6
    ) -> PlacedCommandMark? {
        let close = ticks.filter { abs($0.y - y) <= slop }
        guard !close.isEmpty else { return nil }
        return close.min { a, b in
            let da = abs(a.y - y)
            let db = abs(b.y - y)
            if da != db { return da < db }
            if a.mark.kind == .fail && b.mark.kind != .fail { return true }
            if a.mark.kind != .fail && b.mark.kind == .fail { return false }
            return a.mark.seq > b.mark.seq
        }
    }

    /// 点在轨底部附近等于回底；否则按比例跳到历史 offset。
    public static func trackOffset(
        y: CGFloat,
        height: CGFloat,
        maxOffset: UInt32,
        inset: CGFloat = inset
    ) -> UInt32 {
        guard height > 0 else { return 0 }
        let usable = max(height - inset * 2, 1)
        let clamped = min(max(y - inset, 0), usable)
        let ratio = Double(clamped / usable)
        if ratio <= 0.08 { return 0 }
        let raw = (Double(maxOffset) * ratio).rounded()
        return UInt32(min(max(raw, 0), Double(maxOffset)))
    }
}

/// 「↓ 最新 · +N」胶囊文案与滚到底吸附。
public enum JumpLatestCaption {
    /// 接近底部时继续下拉就吸附到实时尾部，避免在最后几行里细挪。
    public static let snapScrollPosition: Double = 0.92

    public static func title(unseenLines: UInt32, latestWord: String) -> String {
        if unseenLines == 0 {
            return "↓ \(latestWord)"
        }
        return "↓ \(latestWord) · +\(unseenLines)"
    }

    public static func shouldSnapToLatest(scrollPosition: Double) -> Bool {
        scrollPosition >= snapScrollPosition && scrollPosition < 0.999
    }
}
