import Foundation

/// 常驻 Workspace scene 的纯逻辑（无 AppKit 依赖）。
///
/// SceneStack 持有所有已打开 scene；同一时刻只有一个 scene 可见。
/// 切换只改变可见 scene，关闭/容量策略才销毁 scene。
public struct SceneKey: Hashable, Sendable {
    public let transport: String // "local" / "ssh"
    public let alias: String?    // SSH host alias；local 为 nil
    public let session: String
    public let runtime: String   // "tmux" / "shell" / "herdr"
    public let path: String
    /// Target-side runtime socket; nil means the runtime default.
    public let socket: String?
    /// Herdr workspace id；tmux/shell 为 nil，不能复用 `path`。
    public let workspaceID: String?

    public init(
        transport: String,
        alias: String?,
        session: String,
        runtime: String,
        path: String,
        socket: String? = nil,
        workspaceID: String? = nil
    ) {
        self.transport = transport
        self.alias = alias
        self.session = session
        self.runtime = runtime
        self.path = path
        self.socket = socket
        self.workspaceID = workspaceID
    }

    /// 完整 runtime identity 已经能唯一定位 Workspace，Project path 只剩元数据；
    /// identity 尚不完整时 path 仍是 provisional key 的必要部分。
    private var hasResolvedWorkspaceIdentity: Bool {
        !session.isEmpty
            && !(socket?.isEmpty ?? true)
            && !(workspaceID?.isEmpty ?? true)
    }

    public static func == (lhs: SceneKey, rhs: SceneKey) -> Bool {
        guard lhs.transport == rhs.transport,
              lhs.alias == rhs.alias,
              lhs.session == rhs.session,
              lhs.runtime == rhs.runtime,
              lhs.socket == rhs.socket,
              lhs.workspaceID == rhs.workspaceID,
              lhs.hasResolvedWorkspaceIdentity == rhs.hasResolvedWorkspaceIdentity
        else {
            return false
        }
        return lhs.hasResolvedWorkspaceIdentity || lhs.path == rhs.path
    }

    public func hash(into hasher: inout Hasher) {
        hasher.combine(transport)
        hasher.combine(alias)
        hasher.combine(session)
        hasher.combine(runtime)
        hasher.combine(socket)
        hasher.combine(workspaceID)
        hasher.combine(hasResolvedWorkspaceIdentity)
        if !hasResolvedWorkspaceIdentity {
            hasher.combine(path)
        }
    }
}

/// Scene key → QuickConnect 目标：tmux 用 session 名，shell 用路径目录名。
public extension SceneKey {
    var targetConfig: TargetConfig {
        let name = session.isEmpty ? QuickConnect.defaultName(for: path) : session
        let runtime = TargetRuntime(rawValue: runtime) ?? .tmux
        let transport: TargetTransport
        if self.transport == "ssh", let alias {
            transport = .ssh(name: alias)
        } else {
            transport = .local
        }
        return TargetConfig(
            name: name,
            runtime: runtime,
            transport: transport,
            path: path,
            session: session.isEmpty ? nil : session,
            socket: socket,
            workspaceID: workspaceID
        )
    }
}

public enum SceneVisibility: Equatable, Sendable {
    case visible
    case hidden
    case closed
}

public enum SceneEvictionReason: Equatable, Sendable {
    case capacity
    case ttl
    case memoryPressure
    case closed
}

/// 隐藏 scene 容量提醒显示给用户的摘要。
public struct SceneCapacityCandidate: Equatable, Sendable {
    public let key: SceneKey
    public let targetConfig: TargetConfig
    public let lastUsedAt: UInt64

    public init(key: SceneKey, targetConfig: TargetConfig, lastUsedAt: UInt64) {
        self.key = key
        self.targetConfig = targetConfig
        self.lastUsedAt = lastUsedAt
    }
}

/// SceneStack 中一个 scene 的抽象：真实实现持有 CoreBridge / TerminalManager。
public protocol SceneProtocol: AnyObject {
    var key: SceneKey { get set }
    var targetConfig: TargetConfig { get set }
    var visibility: SceneVisibility { get set }
    var lastUsedAt: UInt64 { get set }
    /// 关闭 scene；共享 Core handle 的生命周期由窗口统一管理。
    func evict(reason: SceneEvictionReason)
    /// 窗口/应用关闭：回收所有 scene；共享 Core handle 由窗口统一关闭。
    func shutdown()
}

public struct SceneStackPolicy: Sendable {
    public var maxScenes: Int
    public var ttlNanoseconds: UInt64?

    public init(maxScenes: Int, ttlNanoseconds: UInt64? = nil) {
        self.maxScenes = maxScenes
        self.ttlNanoseconds = ttlNanoseconds
    }
}

public final class SceneStack<Slot: SceneProtocol> {
    public private(set) var scenes: [SceneKey: Slot] = [:]
    public private(set) var activeKey: SceneKey?
    public var policy: SceneStackPolicy
    private let nowProvider: () -> UInt64

    public init(
        policy: SceneStackPolicy,
        nowProvider: @escaping () -> UInt64 = { DispatchTime.now().uptimeNanoseconds }
    ) {
        self.policy = policy
        self.nowProvider = nowProvider
    }

    public var sceneCount: Int { scenes.count }

    public var maxScenes: Int { max(1, policy.maxScenes) }

    /// 容量是软提醒阈值；新连接不会因此静默淘汰旧 Workspace。
    public var isOverCapacity: Bool { sceneCount > maxScenes }

    /// 返回最久未使用的隐藏 scene，供 UI 让用户选择性关闭。
    public func oldestHiddenCandidates(limit: Int) -> [SceneCapacityCandidate] {
        guard limit > 0 else { return [] }
        return scenes.values
            .filter { $0.visibility == .hidden }
            .sorted {
                if $0.lastUsedAt != $1.lastUsedAt {
                    return $0.lastUsedAt < $1.lastUsedAt
                }
                return $0.key.session < $1.key.session
            }
            .prefix(limit)
            .map {
                SceneCapacityCandidate(
                    key: $0.key,
                    targetConfig: $0.targetConfig,
                    lastUsedAt: $0.lastUsedAt
                )
            }
    }

    /// 最近使用的目标（按 lastUsedAt 倒序），供 QuickConnect 的 Recent 列表。
    public func recentTargetConfigs(limit: Int = 5) -> [TargetConfig] {
        guard limit > 0 else { return [] }
        let active = activeKey.flatMap { key in
            scenes[key].flatMap { slot in
                slot.visibility == .closed ? nil : slot
            }
        }
        let activeKey = active.map(\.key)
        var ordered: [Slot] = []
        if let active {
            // 当前 Workspace 必须稳定出现在 Recent 首位，即使它刚创建时
            // 的时间戳比历史连接旧（例如启动时登记的 local workspace）。
            ordered.append(active)
        }
        ordered.append(contentsOf: scenes.values
            .filter { $0.visibility != .closed && $0.key != activeKey }
            .sorted {
                if $0.lastUsedAt != $1.lastUsedAt {
                    return $0.lastUsedAt > $1.lastUsedAt
                }
                return $0.key.session < $1.key.session
            })
        return ordered.prefix(limit).map(\.targetConfig)
    }

    /// SceneStack 中的完整 Workspace 快照，按最近使用顺序返回。
    /// 与紧凑 Recent 展示不同，搜索需要包含容量阈值之后仍保留的 scene。
    public func allRecentTargetConfigs() -> [TargetConfig] {
        recentTargetConfigs(limit: Int.max)
    }

    /// 当前可见 scene 对应的目标（用于 QuickConnect 行高亮）。
    public var currentTargetConfig: TargetConfig? {
        activeKey.flatMap { scenes[$0]?.targetConfig }
    }

    /// 更新当前 Workspace 的展示名。tmux rename 会改变后续 attach 使用的
    /// session 名，因此同时重建 scene key；本地 shell 只改展示名。
    public func renameActiveTarget(to name: String, rekeySession: Bool) {
        guard let oldKey = activeKey, let slot = scenes[oldKey] else { return }
        var config = slot.targetConfig
        config.name = name
        if rekeySession {
            config.session = name
        }
        slot.targetConfig = config
        guard rekeySession else { return }

        let newKey = SceneKey(
            transport: oldKey.transport,
            alias: oldKey.alias,
            session: name,
            runtime: oldKey.runtime,
            path: oldKey.path,
            socket: oldKey.socket,
            workspaceID: oldKey.workspaceID
        )
        guard newKey != oldKey, scenes[newKey] == nil else { return }
        scenes.removeValue(forKey: oldKey)
        slot.key = newKey
        scenes[newKey] = slot
        activeKey = newKey
    }

    /// 激活目标 scene：已存在则复用；不存在则用 `create` 新建。
    /// 切换时旧 scene 隐藏但不销毁其视图树。
    @discardableResult
    public func activate(
        key: SceneKey,
        create: (SceneKey) -> Slot
    ) -> (Slot, Bool) {
        let now = nowProvider()

        if let existing = scenes[key] {
            // 把当前 scene 隐藏（如果不是同一个 key）
            if let activeKey, activeKey != key, let active = scenes[activeKey] {
                active.visibility = .hidden
            }
            existing.lastUsedAt = now
            existing.visibility = .visible
            activeKey = key
            return (existing, false)
        }

        // 切走旧 active
        if let activeKey, activeKey != key, let active = scenes[activeKey] {
            active.visibility = .hidden
        }

        let slot = create(key)
        slot.lastUsedAt = now
        slot.visibility = .visible
        scenes[key] = slot
        activeKey = key
        return (slot, true)
    }

    /// 把当前 scene 标为隐藏，不销毁其视图树。
    public func hide(key: SceneKey) {
        guard activeKey == key else { return }
        scenes[key]?.visibility = .hidden
        activeKey = nil
    }

    /// 显式容量清理：按 LRU（lastUsedAt 升序）关闭超过 maxScenes 的隐藏 scene。
    /// activate 本身不会调用此方法。
    public func evictForCapacity() {
        let maxScenes = self.maxScenes
        guard scenes.count > maxScenes else { return }
        let hidden = scenes.values
            .filter { $0.visibility == .hidden }
            .sorted { $0.lastUsedAt < $1.lastUsedAt }
        var overflow = scenes.count - maxScenes
        for slot in hidden where overflow > 0 {
            evict(slot, reason: .capacity)
            overflow -= 1
        }
    }

    /// TTL 到期：关闭超时的隐藏 scene。
    public func evictExpired() {
        guard let ttl = policy.ttlNanoseconds else { return }
        let now = nowProvider()
        let expired = scenes.values.filter { slot in
            slot.visibility == .hidden && now >= slot.lastUsedAt && now - slot.lastUsedAt > ttl
        }
        for slot in expired {
            evict(slot, reason: .ttl)
        }
    }

    /// memory pressure：关闭全部隐藏 scene。
    public func evictUnderMemoryPressure() {
        let hidden = scenes.values.filter { $0.visibility == .hidden }
        for slot in hidden {
            evict(slot, reason: .memoryPressure)
        }
    }

    /// 关闭并移除指定 scene。返回 false 表示 key 不存在。
    @discardableResult
    public func close(key: SceneKey) -> Bool {
        guard let slot = scenes[key] else { return false }
        evict(slot, reason: .closed)
        return true
    }

    /// 窗口/应用关闭：回收全部 scene。
    public func shutdownAll() {
        for slot in scenes.values {
            slot.shutdown()
        }
        scenes.removeAll()
        activeKey = nil
    }

    private func evict(_ slot: Slot, reason: SceneEvictionReason) {
        slot.visibility = .closed
        slot.evict(reason: reason)
        scenes.removeValue(forKey: slot.key)
        if activeKey == slot.key {
            activeKey = nil
        }
    }
}
