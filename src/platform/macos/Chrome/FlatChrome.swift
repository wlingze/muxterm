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
    /// Core 事件轮询间隔；约 60Hz，避免分割/tab/输入在 100ms 定时器下产生明显滞后。
    public static let eventPollInterval: TimeInterval = 1.0 / 60.0
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
        activePane: UInt32,
        tabsLabel: String,
        panesLabel: String,
        paneLabel: String
    ) -> String {
        "\(status)  \(tabsLabel): \(tabCount)  \(panesLabel): \(paneCount)  \(paneLabel): @\(activePane)"
    }
}

/// tmux 控制模式的终端应答策略。
///
/// tmux / daemon 代理拥有 pane 的 PTY 与终端协议，前端只是渲染镜像：
/// SwiftTerm 在 feed 远端 pane 输出期间生成的查询应答（OSC 10/11/12、
/// CSI DA/DSR、DCS 等）必须丢弃，否则经 `send-keys -l` 回写会被 pane
/// 回显并执行，泄漏成 `git lg` 的 `10;rgb:...` / `65;...c` 字面命令。
///
/// 用户输入（键盘/kitty/粘贴）与鼠标上报不在 feed 窗口内，不受影响；
/// 仅 local 模式（前端就是该 PTY 的终端模拟器）保持转发。
public enum TerminalMirrorPolicy {
    public static func shouldForwardParserResponse(
        duringRemoteOutputFeed: Bool,
        isTmuxMirror: Bool
    ) -> Bool {
        !(isTmuxMirror && duringRemoteOutputFeed)
    }
}


/// PaneOutput 事件的喂入策略。
///
/// 参考成熟终端设计：后端 `%output` 事件流是权威增量，累计快照只用于
/// 新建视图的播种。视图首次创建时，播种快照（后端最近 256KB）已覆盖
/// 后端已入队但尚未派发的事件；这些事件必须跳过，否则同一批字节会双写
/// （输入/回显重复）。快照为空（新 pane 首批字节）时没有任何覆盖，事件
/// 必须原样喂入。视图已存在时事件就是纯增量，直接喂入。
public enum PaneOutputFeedPolicy {
    public static func shouldFeedEvent(
        viewExistedBeforeEvent: Bool,
        seedCoveredEvent: Bool
    ) -> Bool {
        viewExistedBeforeEvent || !seedCoveredEvent
    }
}
