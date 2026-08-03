import Foundation

#if canImport(CoreGraphics)
import CoreGraphics
#else
public typealias CGFloat = Double
#endif

/// macOS 极简 chrome 尺寸与文案（对齐 `assets/style.css` / iTerm2 Minimal）。
///
/// 纯数值与格式化，无 AppKit 依赖，便于 `swift test` 回归。
public enum FlatChrome {
    /// Tab 栏高度（ARCHITECTURE：≤ 24px）
    public static let tabBarHeight: CGFloat = 24
    /// 状态栏高度（一行小字，不抢终端空间）
    public static let statusBarHeight: CGFloat = 18
    /// Pane 分割线厚度（细分隔，非卡片边框）
    public static let splitDividerThickness: CGFloat = 1
    /// 活跃 pane 指示边框（1px，避免 2px「卡片」感）
    public static let activePaneBorderWidth: CGFloat = 1
    /// 活跃 tab 底边指示线
    public static let activeTabUnderlineHeight: CGFloat = 2
    /// 「+」新建 tab 按钮宽度
    public static let newTabButtonWidth: CGFloat = 28
    /// 状态栏左右内边距
    public static let statusHorizontalInset: CGFloat = 6
    /// Tab 单元左右内边距（文字扫描用）
    public static let tabCellHorizontalInset: CGFloat = 8

    /// 紧凑状态栏文案。保留 XCUITest 解析 token：`connected` / `tabs: N` / `panes: N` / `pane: @N`。
    public static func statusText(
        status: String,
        tabCount: Int,
        paneCount: Int,
        activePane: UInt32
    ) -> String {
        "\(status)  tabs: \(tabCount)  panes: \(paneCount)  pane: @\(activePane)"
    }
}


/// Tracks how much cumulative pane output has already been rendered.
///
/// The first PaneOutput event can arrive before the view exists. Creating the
/// view reads the cumulative snapshot, which already includes that event; using
/// the cursor's unseen suffix prevents feeding the same bytes a second time.
public struct PaneOutputCursor {
    private var fedLength = 0

    public init() {}

    public mutating func initial(snapshot: Data) -> Data {
        guard snapshot.count > fedLength else { return Data() }
        let unseen = snapshot.dropFirst(fedLength)
        fedLength = snapshot.count
        return Data(unseen)
    }

    public mutating func incremental(event: Data, snapshot: Data) -> Data {
        if snapshot.count > fedLength {
            let unseen = snapshot.dropFirst(fedLength)
            fedLength = snapshot.count
            return Data(unseen)
        }
        // The Rust core appends every PaneOutput event to the cumulative
        // snapshot BEFORE dispatching it, so the snapshot is authoritative.
        // If it shrank (bounded buffer trimmed its head), reset and re-feed
        // the current tail rather than silently dropping it.
        if snapshot.count < fedLength {
            fedLength = 0
            return initial(snapshot: snapshot)
        }
        // snapshot.count == fedLength: this tick's bytes were already
        // consumed by a prior initial()/incremental(). Never re-feed the
        // raw event — that is what caused the prompt/echo to double.
        return Data()
    }
}
