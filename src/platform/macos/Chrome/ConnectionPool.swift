import Foundation

/// Warm connection pool 的纯逻辑（无 AppKit 依赖）。
///
/// 连接池持有多个后台连接；前台同一时刻至多一个 active slot。
/// 切换目标时优先复用已有连接（不 shutdown），只在容量超限 / TTL 到期 /
/// memory pressure 时才淘汰（tmux 用 detach，保留 server/session）。
public struct ConnectionKey: Hashable, Sendable {
    public let transport: String // "local" / "ssh"
    public let alias: String?    // SSH host alias；local 为 nil
    public let session: String
    public let runtime: String   // "tmux" / "shell"
    public let path: String

    public init(
        transport: String,
        alias: String?,
        session: String,
        runtime: String,
        path: String
    ) {
        self.transport = transport
        self.alias = alias
        self.session = session
        self.runtime = runtime
        self.path = path
    }
}

public enum ConnectionLifecycle: Equatable, Sendable {
    case active
    case background
    case evicting
}

public enum ConnectionEvictionReason: Equatable, Sendable {
    case capacity
    case ttl
    case memoryPressure
}

/// 连接池中一个连接的抽象：真实实现持有 CoreBridge / TerminalManager。
public protocol ConnectionSlotProtocol: AnyObject {
    var key: ConnectionKey { get }
    var lifecycle: ConnectionLifecycle { get set }
    var lastUsedAt: UInt64 { get set }
    /// 后台继续 poll 事件、维护 warm 状态；不得同步 displayIfNeeded。
    func pollBackground()
    /// 淘汰：tmux 用 detach 保留 session；local shell 按实现策略处理。
    func evict(reason: ConnectionEvictionReason)
    /// 窗口/应用关闭：直接回收 handle，不再保留后台连接。
    func shutdown()
}

public struct ConnectionPoolPolicy: Sendable {
    public var maxSlots: Int
    public var ttlNanoseconds: UInt64?

    public init(maxSlots: Int, ttlNanoseconds: UInt64? = nil) {
        self.maxSlots = maxSlots
        self.ttlNanoseconds = ttlNanoseconds
    }
}

public final class ConnectionPool<Slot: ConnectionSlotProtocol> {
    public private(set) var slots: [ConnectionKey: Slot] = [:]
    public private(set) var activeKey: ConnectionKey?
    public var policy: ConnectionPoolPolicy
    private let nowProvider: () -> UInt64

    public init(
        policy: ConnectionPoolPolicy,
        nowProvider: @escaping () -> UInt64 = { DispatchTime.now().uptimeNanoseconds }
    ) {
        self.policy = policy
        self.nowProvider = nowProvider
    }

    public var slotCount: Int { slots.count }

    /// 获取目标连接：已存在则复用并提升为 active；不存在则用 `create` 新建。
    /// 切换时旧 active 自动降为 background，不立即 shutdown。
    @discardableResult
    public func acquire(
        key: ConnectionKey,
        create: (ConnectionKey) -> Slot
    ) -> (Slot, Bool) {
        let now = nowProvider()

        if let existing = slots[key] {
            // 把当前 active 降为 background（如果不是同一个 key）
            if let activeKey, activeKey != key, let active = slots[activeKey] {
                active.lifecycle = .background
            }
            existing.lastUsedAt = now
            existing.lifecycle = .active
            activeKey = key
            return (existing, false)
        }

        // 切走旧 active
        if let activeKey, activeKey != key, let active = slots[activeKey] {
            active.lifecycle = .background
        }

        let slot = create(key)
        slot.lastUsedAt = now
        slot.lifecycle = .active
        slots[key] = slot
        activeKey = key
        evictForCapacity()
        return (slot, true)
    }

    /// 把 active 连接降为 background，不 shutdown（warm）。
    public func release(key: ConnectionKey) {
        guard activeKey == key else { return }
        slots[key]?.lifecycle = .background
        activeKey = nil
    }

    /// 淘汰超过 maxSlots 的 background（LRU：lastUsedAt 升序）。
    public func evictForCapacity() {
        let maxSlots = max(1, policy.maxSlots)
        guard slots.count > maxSlots else { return }
        let background = slots.values
            .filter { $0.lifecycle == .background }
            .sorted { $0.lastUsedAt < $1.lastUsedAt }
        var overflow = slots.count - maxSlots
        for slot in background where overflow > 0 {
            evict(slot, reason: .capacity)
            overflow -= 1
        }
    }

    /// TTL 到期：淘汰超时的 background 连接。
    public func evictExpired() {
        guard let ttl = policy.ttlNanoseconds else { return }
        let now = nowProvider()
        let expired = slots.values.filter { slot in
            slot.lifecycle == .background && now >= slot.lastUsedAt && now - slot.lastUsedAt > ttl
        }
        for slot in expired {
            evict(slot, reason: .ttl)
        }
    }

    /// memory pressure：淘汰全部 background 连接。
    public func evictUnderMemoryPressure() {
        let background = slots.values.filter { $0.lifecycle == .background }
        for slot in background {
            evict(slot, reason: .memoryPressure)
        }
    }

    /// 后台连接继续 poll，保持 warm；不得同步 displayIfNeeded。
    public func pollBackgroundSlots() {
        for slot in slots.values where slot.lifecycle == .background {
            slot.pollBackground()
        }
    }

    /// 窗口/应用关闭：回收全部连接（不保留后台）。
    public func shutdownAll() {
        for slot in slots.values {
            slot.shutdown()
        }
        slots.removeAll()
        activeKey = nil
    }

    private func evict(_ slot: Slot, reason: ConnectionEvictionReason) {
        slot.lifecycle = .evicting
        slot.evict(reason: reason)
        slots.removeValue(forKey: slot.key)
        if activeKey == slot.key {
            activeKey = nil
        }
    }
}
