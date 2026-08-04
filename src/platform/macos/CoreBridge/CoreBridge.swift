import Foundation
import CMuxterm

// MARK: - 模型类型（Swift 侧 owned 快照）

/// Tab 快照。
struct Tab: Equatable {
    let id: UInt32
    let name: String
    let isActive: Bool
}

/// Pane 快照。
struct Pane: Equatable {
    let id: UInt32
    let cols: UInt16
    let rows: UInt16
    let isActive: Bool
}

/// 布局树（owned，对应 CLayoutNode）。
indirect enum LayoutNode: Equatable {
    case leaf(paneId: UInt32)
    case split(horizontal: Bool, ratio: UInt32, first: LayoutNode, second: LayoutNode)

    /// 按布局树顺序返回当前 tab 的 pane 叶子。
    func leafPaneIDs() -> [UInt32] {
        switch self {
        case .leaf(let paneId):
            // tmux 合法的第一个 pane id 就是 0；不能把它当成空值。
            return [paneId]
        case .split(_, _, let first, let second):
            return first.leafPaneIDs() + second.leafPaneIDs()
        }
    }
}

/// 从 FFI 拷贝出的状态变更事件。
struct StateChange: Equatable {
    let type: UInt32
    let paneId: UInt32
    let tabId: UInt32
    let windowId: UInt32
    let data: Data
    let name: String

    var isPaneOutput: Bool { type == STATE_PANE_OUTPUT }
    var isPaneClosed: Bool { type == STATE_PANE_CLOSED }
    var isTabClosed: Bool { type == STATE_TAB_CLOSED }
    var isBackendStatus: Bool { type == STATE_BACKEND_STATUS }
}

/// 平台 → 核心的任务（避免与 Swift Concurrency `Task` 重名）。
struct MuxTask {
    let type: UInt32
    let targetPane: UInt32
    let targetTab: UInt32
    let dir: UInt32
    let name: String?

    static func splitPane(targetPane: UInt32, horizontal: Bool) -> MuxTask {
        MuxTask(
            type: TASK_SPLIT_PANE,
            targetPane: targetPane,
            targetTab: 0,
            dir: horizontal ? DIR_HORIZONTAL : DIR_VERTICAL,
            name: nil
        )
    }

    static func newTab() -> MuxTask {
        MuxTask(type: TASK_NEW_TAB, targetPane: 0, targetTab: 0, dir: 0, name: nil)
    }

    static func switchTab(_ tabId: UInt32) -> MuxTask {
        MuxTask(type: TASK_SWITCH_TAB, targetPane: 0, targetTab: tabId, dir: 0, name: nil)
    }

    static func closePane(_ paneId: UInt32) -> MuxTask {
        MuxTask(type: TASK_CLOSE_PANE, targetPane: paneId, targetTab: 0, dir: 0, name: nil)
    }

    static func closeTab(_ tabId: UInt32) -> MuxTask {
        MuxTask(type: TASK_CLOSE_TAB, targetPane: 0, targetTab: tabId, dir: 0, name: nil)
    }

    static func nextPane() -> MuxTask {
        MuxTask(type: TASK_NEXT_PANE, targetPane: 0, targetTab: 0, dir: 0, name: nil)
    }

    static func prevPane() -> MuxTask {
        MuxTask(type: TASK_PREV_PANE, targetPane: 0, targetTab: 0, dir: 0, name: nil)
    }

    static func switchPane(_ paneId: UInt32) -> MuxTask {
        MuxTask(type: TASK_SWITCH_PANE, targetPane: paneId, targetTab: 0, dir: 0, name: nil)
    }
}

/// 一帧渲染快照。
struct FrameSnapshot {
    var tabs: [Tab] = []
    var panes: [Pane] = []
    var layout: LayoutNode?
    var status: String = "disconnected"
    var activeTab: UInt32 = 0
    var activePane: UInt32 = 0
}

// MARK: - CoreBridge

/// 封装 `muxterm.h` C ABI；生命周期对应 Rust `MuxtermHandle`。
/// 与 TUI `src/platform/tui/ffi_bridge.rs` 逻辑同构。
final class CoreBridge {
    private var handle: OpaquePointer?
    /// 当前连接的后端类型；tmux/ssh 都通过控制 client 同步整体尺寸。
    let backendType: String
    /// 最近一次 BackendStatus（pane_id 字段复用状态码）。
    private(set) var lastStatus: UInt32 = 2 // Connected
    private var pendingError: String?
    private var pollFailureReported = false

    /// 创建 handle 并 connect。
    /// - Parameters:
    ///   - backendType: `"local"` / `"tmux"` / `"ssh"` / `"daemon"`
    ///   - socket: tmux `-L` socket 名（可选）
    ///   - session: session 名（可选）
    init(backendType: String = "local", socket: String? = nil, session: String? = nil) throws {
        let normalizedBackendType = backendType.lowercased()
        self.backendType = normalizedBackendType
        let handle = normalizedBackendType.withCString { btPtr in
            Self.withOptionalCString(socket) { sockPtr in
                Self.withOptionalCString(session) { sessPtr in
                    muxterm_new(btPtr, sockPtr, sessPtr)
                }
            }
        }
        guard let handle else {
            throw BridgeError.createFailed
        }
        let rc = muxterm_connect(handle)
        guard rc == 0 else {
            muxterm_free(handle)
            throw BridgeError.connectFailed(rc)
        }
        self.handle = handle
    }

    deinit {
        shutdownAndFree()
    }

    /// 执行任务。
    @discardableResult
    func execute(task: MuxTask) -> Int32 {
        guard let handle else { return -1 }
        return withCTask(task) { cTask in
            muxterm_execute(handle, cTask)
        }
    }

    /// 非阻塞拉取事件（内部拷贝 data/name，指针在返回后即可失效）。
    func pollEvents() -> [StateChange] {
        guard let handle else { return [] }
        var buf = Array(repeating: CStateChange(), count: 64)
        let n = muxterm_poll_events(handle, &buf, Int32(buf.count))
        if n < 0 {
            if !pollFailureReported {
                pendingError = "核心事件轮询失败，GUI 状态可能暂时无法同步"
                pollFailureReported = true
            }
            return []
        }
        guard n > 0 else { return [] }

        return buf.prefix(Int(n)).map { c in
            if c.type_ == STATE_BACKEND_STATUS {
                lastStatus = c.pane_id
            }
            let data: Data
            if c.data != nil && c.data_len > 0 {
                data = Data(bytes: c.data, count: c.data_len)
            } else {
                data = Data()
            }
            return StateChange(
                type: c.type_,
                paneId: c.pane_id,
                tabId: c.tab_id,
                windowId: c.window_id,
                data: data,
                name: Self.string(from: c.name)
            )
        }
    }

    /// 取出一次待显示的核心错误。
    func takeError() -> String? {
        defer { pendingError = nil }
        return pendingError
    }

    /// 向指定 pane 发送原始输入字节。
    @discardableResult
    func sendInput(paneId: UInt32, data: Data) -> Int32 {
        guard let handle, !data.isEmpty else { return 0 }
        return data.withUnsafeBytes { raw in
            let ptr = raw.bindMemory(to: UInt8.self).baseAddress
            return muxterm_send_input(handle, paneId, ptr, raw.count)
        }
    }

    /// 同步 pty 行列（SwiftTerm sizeChanged → LocalBackend resize）。
    @discardableResult
    func resizePane(paneId: UInt32, cols: UInt16, rows: UInt16) -> Int32 {
        guard let handle, cols > 0, rows > 0 else { return -1 }
        return muxterm_resize_pane(handle, paneId, cols, rows)
    }

    /// 同步 tmux 控制 client 的整体字符格尺寸，避免对每个 pane 逐个 resize 造成反馈环。
    @discardableResult
    func resizeClient(cols: UInt16, rows: UInt16) -> Int32 {
        guard let handle, cols > 0, rows > 0 else { return -1 }
        return muxterm_resize_client(handle, cols, rows)
    }

    /// 调整鼠标拖动对应的 pane 单一轴尺寸。
    @discardableResult
    func resizePaneAxis(paneId: UInt32, horizontal: Bool, size: UInt16) -> Int32 {
        guard let handle, size > 0 else { return -1 }
        let axis = horizontal ? DIR_HORIZONTAL : DIR_VERTICAL
        return muxterm_resize_pane_axis(handle, paneId, axis, size)
    }

    func getTabs() -> [Tab] {
        guard let handle else { return [] }
        var buf = Array(repeating: CTab(), count: 32)
        let n = muxterm_get_tabs(handle, &buf, Int32(buf.count))
        guard n > 0 else { return [] }
        return buf.prefix(Int(n)).map { t in
            Tab(id: t.id, name: Self.string(from: t.name), isActive: t.is_active != 0)
        }
    }

    func getPanes(tabId: UInt32) -> [Pane] {
        guard let handle else { return [] }
        var buf = Array(repeating: CPane(), count: 64)
        let n = muxterm_get_panes(handle, tabId, &buf, Int32(buf.count))
        guard n > 0 else { return [] }
        return buf.prefix(Int(n)).map { p in
            Pane(id: p.id, cols: p.cols, rows: p.rows, isActive: p.is_active != 0)
        }
    }

    func getLayout(tabId: UInt32) -> LayoutNode? {
        guard let handle else { return nil }
        var root = CLayoutNode()
        let rc = muxterm_get_layout(handle, tabId, &root)
        guard rc == 0 else { return nil }
        return Self.cloneLayout(root)
    }

    func getPaneOutput(paneId: UInt32) -> Data {
        guard let handle else { return Data() }
        var buf = [UInt8](repeating: 0, count: 256 * 1024)
        let n = muxterm_get_pane_output(handle, paneId, &buf, buf.count)
        guard n > 0 else { return Data() }
        return Data(buf.prefix(Int(n)))
    }

    /// 拉取完整渲染快照。
    func snapshot() -> FrameSnapshot {
        let tabs = getTabs()
        let activeTab = tabs.first(where: \.isActive)?.id ?? tabs.first?.id ?? 0
        let panes = getPanes(tabId: activeTab)
        let activePane = panes.first(where: \.isActive)?.id ?? panes.first?.id ?? 0
        let layout = getLayout(tabId: activeTab)
        return FrameSnapshot(
            tabs: tabs,
            panes: panes,
            layout: layout,
            status: Self.statusLabel(lastStatus),
            activeTab: activeTab,
            activePane: activePane
        )
    }

    func shutdown() {
        shutdownAndFree()
    }

    // MARK: - Private

    private func shutdownAndFree() {
        guard let handle else { return }
        _ = muxterm_shutdown(handle)
        muxterm_free(handle)
        self.handle = nil
    }

    private func withCTask<T>(_ task: MuxTask, _ body: (UnsafePointer<CTask>) -> T) -> T {
        if let name = task.name {
            return name.withCString { namePtr in
                var c = CTask(
                    type_: task.type,
                    target_pane: task.targetPane,
                    target_tab: task.targetTab,
                    dir: task.dir,
                    name: namePtr
                )
                return body(&c)
            }
        } else {
            var c = CTask(
                type_: task.type,
                target_pane: task.targetPane,
                target_tab: task.targetTab,
                dir: task.dir,
                name: nil
            )
            return body(&c)
        }
    }

    private static func withOptionalCString<T>(
        _ value: String?,
        _ body: (UnsafePointer<CChar>?) -> T
    ) -> T {
        guard let value else { return body(nil) }
        return value.withCString { body($0) }
    }

    private static func string(from ptr: UnsafePointer<CChar>?) -> String {
        guard let ptr else { return "" }
        return String(cString: ptr)
    }

    private static func cloneLayout(_ node: CLayoutNode) -> LayoutNode {
        switch node.type_ {
        case LAYOUT_SPLIT_H, LAYOUT_SPLIT_V:
            let first: LayoutNode
            if let p = node.first {
                first = cloneLayout(p.pointee)
            } else {
                first = .leaf(paneId: 0)
            }
            let second: LayoutNode
            if let p = node.second {
                second = cloneLayout(p.pointee)
            } else {
                second = .leaf(paneId: 0)
            }
            return .split(
                horizontal: node.type_ == LAYOUT_SPLIT_H,
                ratio: node.ratio,
                first: first,
                second: second
            )
        default:
            return .leaf(paneId: node.pane_id)
        }
    }

    private static func statusLabel(_ code: UInt32) -> String {
        switch code {
        case 0: return "disconnected"
        case 1: return "connecting"
        case 2: return "connected"
        case 3: return "error"
        case 4: return "exited"
        default: return "unknown"
        }
    }
}

enum BridgeError: Error, LocalizedError {
    case createFailed
    case connectFailed(Int32)

    var errorDescription: String? {
        switch self {
        case .createFailed:
            return "muxterm_new 失败"
        case .connectFailed(let code):
            return "muxterm_connect 失败: \(code)"
        }
    }
}

// MARK: - C 结构默认值

private extension CStateChange {
    init() {
        self.init(
            type_: STATE_OTHER,
            pane_id: 0,
            tab_id: 0,
            window_id: 0,
            data: nil,
            data_len: 0,
            name: nil
        )
    }
}

private extension CTab {
    init() {
        self.init(id: 0, name: nil, is_active: 0)
    }
}

private extension CPane {
    init() {
        self.init(id: 0, cols: 0, rows: 0, is_active: 0)
    }
}

private extension CLayoutNode {
    init() {
        self.init(type_: LAYOUT_LEAF, pane_id: 0, ratio: 0, first: nil, second: nil)
    }
}
