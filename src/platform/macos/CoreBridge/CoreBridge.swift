import Foundation
import CMuxterm
import MuxtermChrome

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

/// core SSH discovery 返回的 owned 条目。
struct CoreSSHHost: Decodable, Equatable {
    let alias: String
    let hostname: String
    let port: UInt16
    let user: String
}

/// core tmux discovery 返回的 owned session 摘要。
struct CoreTmuxSession: Decodable, Equatable {
    let name: String
    let windows: UInt32
    let attached: Bool
    let created: UInt64
}

/// core 目录列表返回的条目（名字 + 是否目录）。
struct CoreFsEntry: Decodable, Equatable {
    let name: String
    let is_dir: Bool
}

private struct SSHHostsResponse: Decodable {
    let ok: Bool
    let error: String?
    let hosts: [CoreSSHHost]?
}

private struct TmuxSessionsResponse: Decodable {
    let ok: Bool
    let error: String?
    let sessions: [CoreTmuxSession]?
    let workspaces: [CoreTmuxSession]?

    /// core 返回 `workspaces`（W7 改名），旧字段 `sessions` 兼容。
    var resolved: [CoreTmuxSession] {
        sessions ?? workspaces ?? []
    }
}

private struct CreatedSessionResponse: Decodable {
    let ok: Bool
    let error: String?
    let session: String?
}

private struct ListDirResponse: Decodable {
    let ok: Bool
    let error: String?
    let entries: [CoreFsEntry]?
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

    static func detach() -> MuxTask {
        MuxTask(type: TASK_DETACH, targetPane: 0, targetTab: 0, dir: 0, name: nil)
    }

    static func switchPane(_ paneId: UInt32) -> MuxTask {
        MuxTask(type: TASK_SWITCH_PANE, targetPane: paneId, targetTab: 0, dir: 0, name: nil)
    }

    static func togglePaneFullscreen(_ paneId: UInt32) -> MuxTask {
        MuxTask(
            type: TASK_TOGGLE_PANE_FULLSCREEN,
            targetPane: paneId,
            targetTab: 0,
            dir: 0,
            name: nil
        )
    }

    /// 重排 window/tab：`move-window -s :from -t :toIndex`。
    static func moveWindow(from fromTabId: UInt32, toIndex: UInt32) -> MuxTask {
        MuxTask(
            type: TASK_MOVE_WINDOW,
            targetPane: toIndex,
            targetTab: fromTabId,
            dir: 0,
            name: nil
        )
    }

    /// 把 pane 拆成新 tab：`break-pane -s %pane`。
    static func breakPane(_ paneId: UInt32) -> MuxTask {
        MuxTask(
            type: TASK_BREAK_PANE,
            targetPane: paneId,
            targetTab: 0,
            dir: 0,
            name: nil
        )
    }

    /// 重新查询 window/pane 列表（外部 tmux 变更后同步 GUI）。
    static func refreshTabs() -> MuxTask {
        MuxTask(
            type: TASK_REFRESH_TABS,
            targetPane: 0,
            targetTab: 0,
            dir: 0,
            name: nil
        )
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
    /// tmux `-L` socket 名（可选）。
    var socket: String?
    /// tmux session 名（可选）。
    var session: String?
    /// SSH `~/.ssh/config` alias（可选；用于 status 快照的只读查询）。
    var sshAlias: String?
    /// 最近一次 BackendStatus（pane_id 字段复用状态码）。
    private(set) var lastStatus: UInt32 = 2 // Connected
    private var pendingError: String?
    private var pollFailureReported = false

    // MARK: - 日志

    /// 初始化 core 的 tracing 日志（debug 级别 + 可选文件）。
    /// 由 CLI `muxterm gui --debug --log-file` 转发参数后调用；返回 0=成功。
    static func initLogging(debug: Bool, logFile: String?) -> Int32 {
        let level = debug ? "debug" : "info"
        return withOptionalCString(logFile) { filePtr in
            level.withCString { levelPtr in
                muxterm_init_logging(filePtr, levelPtr)
            }
        }
    }

    // MARK: - Core discovery

    /// 读取 core 解析出的用户 SSH alias，不在 macOS 侧读取或解释 ssh config。
    static func discoverSSHHosts(configPath: String? = nil) throws -> [CoreSSHHost] {
        let pointer = withOptionalCString(configPath) { path in
            muxterm_discover_ssh_hosts_json(path)
        }
        let response: SSHHostsResponse = try decodeDiscoveryJSON(pointer)
        guard response.ok else {
            throw CoreBridgeDiscoveryError.message(
                response.error ?? MuxtermI18n.shared.tr(.errorSshHostDiscovery)
            )
        }
        return response.hosts ?? []
    }

    /// 列出本地或 SSH 远端目录条目，供 QuickConnect 配置逐步选目录。
    /// - `backendType`: `local` / `ssh`；ssh 时 `target` 为 ~/.ssh/config alias。
    /// - `path`: 起始路径；空/`~` 时分别取本地 HOME 或远端 `~`。
    static func listDir(
        backendType: String,
        target: String? = nil,
        configPath: String? = nil,
        path: String,
        timeoutMs: UInt32 = 10_000
    ) throws -> [CoreFsEntry] {
        let pointer = backendType.withCString { backend in
            withOptionalCString(target) { target in
                withOptionalCString(configPath) { pathPtr in
                    path.withCString { path in
                        muxterm_list_dir_json(backend, target, pathPtr, path, timeoutMs)
                    }
                }
            }
        }
        let response: ListDirResponse = try decodeDiscoveryJSON(pointer)
        guard response.ok else {
            throw CoreBridgeDiscoveryError.message(
                response.error ?? MuxtermI18n.shared.tr(.errorCoreDiscoveryNoResponse)
            )
        }
        return response.entries ?? []
    }

    /// 通过 core 查询 local 或 SSH tmux session。
    static func discoverTmuxSessions(
        backendType: String,
        target: String? = nil,
        socket: String? = nil,
        configPath: String? = nil,
        timeoutMs: UInt32 = 10_000
    ) throws -> [CoreTmuxSession] {
        let pointer = backendType.withCString { backend in
            withOptionalCString(target) { target in
                withOptionalCString(socket) { socket in
                    withOptionalCString(configPath) { path in
                        muxterm_discover_tmux_sessions_json(
                            backend,
                            target,
                            socket,
                            path,
                            timeoutMs
                        )
                    }
                }
            }
        }
        let response: TmuxSessionsResponse = try decodeDiscoveryJSON(pointer)
        guard response.ok else {
            throw CoreBridgeDiscoveryError.message(
                response.error ?? MuxtermI18n.shared.tr(.errorTmuxSessionDiscovery)
            )
        }
        return response.resolved
    }

    /// 通过 core 创建 detached tmux session。
    static func createTmuxSession(
        backendType: String,
        target: String? = nil,
        socket: String? = nil,
        configPath: String? = nil,
        session: String,
        directory: String,
        timeoutMs: UInt32 = 10_000
    ) throws -> String {
        let pointer = backendType.withCString { backend in
            withOptionalCString(target) { target in
                withOptionalCString(socket) { socket in
                    withOptionalCString(configPath) { path in
                        session.withCString { session in
                            directory.withCString { directory in
                                muxterm_create_tmux_session_json(
                                    backend,
                                    target,
                                    socket,
                                    path,
                                    session,
                                    directory,
                                    timeoutMs
                                )
                            }
                        }
                    }
                }
            }
        }
        let response: CreatedSessionResponse = try decodeDiscoveryJSON(pointer)
        guard response.ok else {
            throw CoreBridgeDiscoveryError.message(
                response.error ?? MuxtermI18n.shared.tr(.errorTmuxSessionCreation)
            )
        }
        return response.session ?? session
    }

    private init(
        handle: OpaquePointer,
        backendType: String,
        socket: String?,
        session: String?,
        sshAlias: String?
    ) {
        self.handle = handle
        self.backendType = backendType.lowercased()
        // SSH：alias 必须放 sshAlias；socket 只表示真正的远端 `-L`。
        let query = StatusQueryTarget.resolve(
            backendType: self.backendType,
            socket: socket,
            sshAlias: sshAlias
        )
        self.socket = query.socket
        self.sshAlias = query.sshAlias
        self.session = session
    }

    /// 创建 handle 并 connect。
    /// - Parameters:
    ///   - backendType: `"local"` / `"tmux"` / `"ssh"` / `"daemon"`
    ///   - socket: tmux `-L` socket 名（可选）。SSH 时这里若传入 alias，FFI 不得再把它当 `-L`。
    ///   - session: session 名（可选）
    convenience init(backendType: String = "local", socket: String? = nil, session: String? = nil) throws {
        let normalized = backendType.lowercased()
        let created: OpaquePointer? = normalized.withCString { btPtr in
            Self.withOptionalCString(socket) { sockPtr in
                Self.withOptionalCString(session) { sessPtr in
                    muxterm_new(btPtr, sockPtr, sessPtr)
                }
            }
        }
        guard let handle = created else {
            throw BridgeError.createFailed
        }
        let rc = muxterm_connect(handle)
        guard rc == 0 else {
            muxterm_free(handle)
            throw BridgeError.connectFailed(rc)
        }
        self.init(
            handle: handle,
            backendType: normalized,
            socket: socket,
            session: session,
            sshAlias: nil
        )
    }

    /// 一步建连：支持指定起始目录（本地/远程 shell）与 tmux-ssh alias。
    /// - `backendType`: `"local"` / `"tmux"` / `"ssh"` / `"tmux-ssh"` / `"daemon"`
    /// - `socket`: tmux `-L` socket 名（可选；SSH 时不是 Host alias）
    /// - `session`: session 名（可选；tmux 有 name → attach，无 → 新建）
    /// - `sshAlias`: `~/.ssh/config` Host 名（SSH 必须走这里，禁止塞进 socket）
    /// - `startDirectory`: 起始工作目录（本地 shell / tmux 新建用）
    static func connect(
        backendType: String,
        socket: String? = nil,
        session: String? = nil,
        sshAlias: String? = nil,
        startDirectory: String? = nil
    ) throws -> CoreBridge {
        let normalized = backendType.lowercased()
        let created: OpaquePointer? = normalized.withCString { btPtr in
            Self.withOptionalCString(socket) { sockPtr in
                Self.withOptionalCString(session) { sessPtr in
                    Self.withOptionalCString(sshAlias) { aliasPtr in
                        Self.withOptionalCString(startDirectory) { dirPtr in
                            muxterm_new_connect(btPtr, sockPtr, sessPtr, aliasPtr, dirPtr)
                        }
                    }
                }
            }
        }
        guard let handle = created else {
            throw BridgeError.createFailed
        }
        let rc = muxterm_connect(handle)
        guard rc == 0 else {
            muxterm_free(handle)
            throw BridgeError.connectFailed(rc)
        }
        return CoreBridge(
            handle: handle,
            backendType: normalized,
            socket: socket,
            session: session,
            sshAlias: sshAlias
        )
    }

    /// status bar 订阅是否已启用（tmux ≥3.2 `refresh-client -B`）。
    /// 已启用时前端关闭轮询定时器，由 `%subscription-changed` 推送驱动。
    func statusSubscriptionActive() -> Bool {
        guard let handle else { return false }
        return muxterm_status_subscription_active(handle) != 0
    }

    /// 抓取 status bar 快照（只读查询，tmux 兼容），返回 JSON 文本。
    func statusBarSnapshotJSON() -> String? {
        guard handle != nil else { return nil }
        return backendType.withCString { bt in
            Self.withOptionalCString(sshAlias) { alias in
                Self.withOptionalCString(socket) { sock in
                    Self.withOptionalCString(session) { sess in
                        guard let p = muxterm_status_snapshot_json(bt, alias, sock, sess) else {
                            return nil
                        }
                        defer { muxterm_free_string(p) }
                        return String(cString: p)
                    }
                }
            }
        }
    }

    // MARK: - 搜索 / 注意力 / 历史（W14/W16 跨平台契约）

    /// 跨全部工作区搜索 pane 文本，返回 JSON 文本（`muxterm_search_all`）。
    func searchAllJSON(query: String) -> String? {
        guard let handle else { return nil }
        return query.withCString { q in
            guard let p = muxterm_search_all(handle, q) else { return nil }
            defer { muxterm_free_string(p) }
            return String(cString: p)
        }
    }

    /// 注意力引擎快照 JSON（`muxterm_attention_snapshot`）。
    func attentionSnapshotJSON() -> String? {
        guard let handle else { return nil }
        guard let p = muxterm_attention_snapshot(handle) else { return nil }
        defer { muxterm_free_string(p) }
        return String(cString: p)
    }

    /// 取走本轮新进入 blocked / done 的通知 JSON（`muxterm_attention_take_notifications`）。
    func attentionTakeNotificationsJSON() -> String? {
        guard let handle else { return nil }
        guard let p = muxterm_attention_take_notifications(handle) else { return nil }
        defer { muxterm_free_string(p) }
        return String(cString: p)
    }

    /// 标记某 pane 成为前台可见。
    @discardableResult
    func attentionOnBecameVisible(paneId: UInt32) -> Int32 {
        guard let handle else { return -1 }
        return muxterm_attention_on_became_visible(handle, paneId)
    }

    /// 更新某 pane 的进程名。
    @discardableResult
    func attentionSetProcessName(paneId: UInt32, name: String?) -> Int32 {
        guard let handle else { return -1 }
        return Self.withOptionalCString(name) { namePtr in
            muxterm_attention_set_process_name(handle, paneId, namePtr)
        }
    }

    /// 静音某 pane 一段时间（秒）。
    @discardableResult
    func attentionMute(paneId: UInt32, seconds: UInt64) -> Int32 {
        guard let handle else { return -1 }
        return muxterm_attention_mute(handle, paneId, seconds)
    }

    /// 读取某 pane 的滚动窗口 ANSI 字节（历史查看用）。
    func paneScrollANSI(paneId: UInt32, offset: UInt32, rows: UInt32) -> Data {
        guard let handle else { return Data() }
        var buf = [UInt8](repeating: 0, count: 64 * 1024)
        let n = buf.withUnsafeMutableBytes { raw in
            muxterm_pane_scroll_ansi(
                handle,
                paneId,
                offset,
                rows,
                raw.bindMemory(to: UInt8.self).baseAddress,
                raw.count
            )
        }
        guard n > 0 else { return Data() }
        return Data(buf.prefix(Int(n)))
    }

    /// 读取某 pane 的 viewport 滚动偏移（0 = 底部/最新）。
    func paneViewport(paneId: UInt32) -> Int32 {
        guard let handle else { return -1 }
        return muxterm_pane_viewport(handle, paneId)
    }

    /// 设置某 pane 的 viewport 滚动偏移（跳转历史后恢复）。
    @discardableResult
    func setPaneViewport(paneId: UInt32, offset: UInt32) -> Int32 {
        guard let handle else { return -1 }
        return muxterm_set_pane_viewport(handle, paneId, offset)
    }

    /// 读取某 pane 最近 n 行文本 JSON（`muxterm_pane_last_n_lines`）。
    func paneLastNLinesJSON(paneId: UInt32, n: UInt32) -> String? {
        guard let handle else { return nil }
        guard let p = muxterm_pane_last_n_lines(handle, paneId, n) else { return nil }
        defer { muxterm_free_string(p) }
        return String(cString: p)
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
                pendingError = MuxtermI18n.shared.tr(.errorCorePoll)
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

    /// 发送原始输入但不触发注意力 on_user_input（W19-E overlay 快速回复）。
    @discardableResult
    func sendInputQuiet(paneId: UInt32, data: Data) -> Int32 {
        guard let handle, !data.isEmpty else { return 0 }
        return data.withUnsafeBytes { raw in
            let ptr = raw.bindMemory(to: UInt8.self).baseAddress
            return muxterm_send_input_quiet(handle, paneId, ptr, raw.count)
        }
    }

    /// 向 tmux 上报 pane 的前景/背景色，供 OSC 10/11 查询代答。
    @discardableResult
    func reportPaneColours(paneId: UInt32, fgHex: String, bgHex: String) -> Int32 {
        guard let handle else { return -1 }
        return fgHex.withCString { fg in
            bgHex.withCString { bg in
                muxterm_report_pane_colours(handle, paneId, fg, bg)
            }
        }
    }

    /// 向 tmux 上报**所有** pane 的前景/背景色（主题切换/连接建立后整段对齐）。
    @discardableResult
    func reportAllPaneColours(fgHex: String, bgHex: String) -> Int32 {
        guard let handle else { return -1 }
        return fgHex.withCString { fg in
            bgHex.withCString { bg in
                muxterm_report_all_pane_colours(handle, fg, bg)
            }
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

    /// 显式分离 tmux/daemon client；成功后由调用方关闭窗口并清理 bridge。
    @discardableResult
    func detach() -> Int32 {
        guard let handle else { return -1 }
        return muxterm_detach(handle)
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

    private static func decodeDiscoveryJSON<T: Decodable>(
        _ pointer: UnsafeMutablePointer<CChar>?
    ) throws -> T {
        guard let pointer else {
            throw CoreBridgeDiscoveryError.message(
                MuxtermI18n.shared.tr(.errorCoreDiscoveryNoResponse)
            )
        }
        let text = String(cString: UnsafePointer(pointer))
        muxterm_free_string(pointer)
        guard let data = text.data(using: .utf8) else {
            throw CoreBridgeDiscoveryError.message(
                MuxtermI18n.shared.tr(.errorCoreDiscoveryInvalidUtf8)
            )
        }
        do {
            return try JSONDecoder().decode(T.self, from: data)
        } catch {
            throw CoreBridgeDiscoveryError.message(
                MuxtermI18n.shared.tr(
                    .errorCoreDiscoveryInvalidJson,
                    arguments: ["error": "\(error)"]
                )
            )
        }
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

enum CoreBridgeDiscoveryError: Error, LocalizedError {
    case message(String)

    var errorDescription: String? {
        switch self {
        case .message(let message):
            return message
        }
    }
}

enum BridgeError: Error, LocalizedError {
    case createFailed
    case connectFailed(Int32)

    var errorDescription: String? {
        switch self {
        case .createFailed:
            return MuxtermI18n.shared.tr(.errorBridgeCreate)
        case .connectFailed(let code):
            return MuxtermI18n.shared.tr(.errorBridgeConnect, arguments: ["code": "\(code)"])
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
