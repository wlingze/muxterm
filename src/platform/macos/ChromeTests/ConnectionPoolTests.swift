import XCTest
@testable import MuxtermChrome

// MARK: - Test doubles

private final class FakeSlot: ConnectionSlotProtocol {
    var key: ConnectionKey
    var targetConfig: TargetConfig
    var lifecycle: ConnectionLifecycle = .background
    var lastUsedAt: UInt64
    var pollCount = 0
    var evictReasons: [ConnectionEvictionReason] = []

    init(key: ConnectionKey, now: UInt64) {
        self.key = key
        self.targetConfig = key.targetConfig
        self.lastUsedAt = now
    }

    func pollBackground() {
        pollCount += 1
    }

    func evict(reason: ConnectionEvictionReason) {
        lifecycle = .evicting
        evictReasons.append(reason)
    }

    func shutdown() {
        lifecycle = .evicting
    }
}

// MARK: - ConnectionKey

final class ConnectionKeyTests: XCTestCase {
    func testKeyIncludesTransportAliasSessionRuntimePath() {
        let a = ConnectionKey(
            transport: "ssh",
            alias: "ryzen",
            session: "yaklang",
            runtime: "tmux",
            path: "/home/wlz/Developer/yaklang-workspace"
        )
        let b = ConnectionKey(
            transport: "ssh",
            alias: "ryzen",
            session: "yaklang",
            runtime: "tmux",
            path: "/home/wlz/Developer/yaklang-workspace"
        )
        XCTAssertEqual(a, b)
        XCTAssertEqual(a.hashValue, b.hashValue)
    }

    func testKeyDistinguishesAlias() {
        let local = ConnectionKey(
            transport: "local", alias: nil, session: "s", runtime: "tmux", path: "/x"
        )
        let ssh = ConnectionKey(
            transport: "ssh", alias: "ryzen", session: "s", runtime: "tmux", path: "/x"
        )
        XCTAssertNotEqual(local, ssh)
    }

    func testKeyDistinguishesPath() {
        let a = ConnectionKey(
            transport: "local", alias: nil, session: "s", runtime: "tmux", path: "/a"
        )
        let b = ConnectionKey(
            transport: "local", alias: nil, session: "s", runtime: "tmux", path: "/b"
        )
        XCTAssertNotEqual(a, b)
    }
}

// MARK: - ConnectionPool

final class ConnectionPoolTests: XCTestCase {
    private var now: UInt64 = 0

    private func makeKey(
        session: String = "s",
        path: String = "/x",
        transport: String = "local",
        alias: String? = nil,
        runtime: String = "tmux"
    ) -> ConnectionKey {
        ConnectionKey(
            transport: transport,
            alias: alias,
            session: session,
            runtime: runtime,
            path: path
        )
    }

    private func makePool(
        maxSlots: Int = 2,
        ttlNanoseconds: UInt64? = nil
    ) -> ConnectionPool<FakeSlot> {
        ConnectionPool(
            policy: ConnectionPoolPolicy(
                maxSlots: maxSlots,
                ttlNanoseconds: ttlNanoseconds
            ),
            nowProvider: { [weak self] in self?.now ?? 0 }
        )
    }

    private func createSlot(_ key: ConnectionKey) -> FakeSlot {
        FakeSlot(key: key, now: now)
    }

    func testAcquireCreatesNewSlot() {
        let pool = makePool()
        let key = makeKey()
        let (slot, created) = pool.acquire(key: key) { [self] _ in createSlot(key) }
        XCTAssertTrue(created)
        XCTAssertEqual(slot.key, key)
        XCTAssertEqual(slot.lifecycle, .active)
        XCTAssertEqual(pool.activeKey, key)
    }

    func testAcquireReusesActiveSlot() {
        let pool = makePool()
        let key = makeKey()
        let (_, created) = pool.acquire(key: key) { [self] _ in createSlot(key) }
        XCTAssertTrue(created)

        let (reused, createdAgain) = pool.acquire(key: key) { [self] _ in createSlot(key) }
        XCTAssertFalse(createdAgain)
        XCTAssertEqual(reused.key, key)
        XCTAssertEqual(pool.slotCount, 1)
    }

    func testAcquirePromotesBackgroundSlot() {
        let pool = makePool(maxSlots: 3)
        let keyA = makeKey(session: "a")
        let keyB = makeKey(session: "b")
        let (slotA, _) = pool.acquire(key: keyA) { [self] _ in createSlot(keyA) }
        let (_, _) = pool.acquire(key: keyB) { [self] _ in createSlot(keyB) }

        // 切回 A：A 从 background 提升为 active
        pool.release(key: keyB)
        XCTAssertEqual(slotA.lifecycle, .background)
        let (slotA2, created) = pool.acquire(key: keyA) { [self] _ in createSlot(keyA) }
        XCTAssertFalse(created)
        XCTAssertEqual(slotA2.key, keyA)
        XCTAssertEqual(slotA2.lifecycle, .active)
        XCTAssertEqual(pool.activeKey, keyA)
    }

    func testReleaseMovesToBackgroundWithoutEvicting() {
        let pool = makePool(maxSlots: 3)
        let key = makeKey()
        let (slot, _) = pool.acquire(key: key) { [self] _ in createSlot(key) }
        pool.release(key: key)
        XCTAssertEqual(slot.lifecycle, .background)
        XCTAssertTrue(slot.evictReasons.isEmpty)
        XCTAssertNil(pool.activeKey)
        XCTAssertEqual(pool.slotCount, 1)
    }

    func testLRUEvictsOldestBackgroundWhenCapacityExceeded() {
        let pool = makePool(maxSlots: 2)
        now = 100
        let keyA = makeKey(session: "a")
        let (slotA, _) = pool.acquire(key: keyA) { [self] _ in createSlot(keyA) }
        pool.release(key: keyA)

        now = 200
        let keyB = makeKey(session: "b")
        let (slotB, _) = pool.acquire(key: keyB) { [self] _ in createSlot(keyB) }
        pool.release(key: keyB)

        now = 300
        let keyC = makeKey(session: "c")
        let (slotC, created) = pool.acquire(key: keyC) { [self] _ in createSlot(keyC) }
        XCTAssertTrue(created)
        XCTAssertEqual(slotC.lifecycle, .active)

        // maxSlots=2，已有 A/B 两个 background；新 C active 后应淘汰最旧 A
        XCTAssertEqual(slotA.lifecycle, .evicting)
        XCTAssertEqual(slotA.evictReasons, [.capacity])
        XCTAssertEqual(slotB.lifecycle, .background)
        XCTAssertEqual(pool.slotCount, 2)
    }

    func testTTLEvictsExpiredBackgroundSlots() {
        let pool = makePool(maxSlots: 3, ttlNanoseconds: 1_000)
        now = 100
        let keyA = makeKey(session: "a")
        let (slotA, _) = pool.acquire(key: keyA) { [self] _ in createSlot(keyA) }
        pool.release(key: keyA)

        now = 500
        let keyB = makeKey(session: "b")
        let (slotB, _) = pool.acquire(key: keyB) { [self] _ in createSlot(keyB) }
        pool.release(key: keyB)

        now = 2_000
        pool.evictExpired()

        XCTAssertEqual(slotA.lifecycle, .evicting)
        XCTAssertEqual(slotA.evictReasons, [.ttl])
        XCTAssertEqual(slotB.lifecycle, .evicting) // 500+1000=1500 > now=2000 → 应也过期
        XCTAssertEqual(slotB.evictReasons, [.ttl])
        XCTAssertEqual(pool.slotCount, 0)
    }

    func testMemoryPressureEvictsBackgroundSlots() {
        let pool = makePool(maxSlots: 3)
        let keyA = makeKey(session: "a")
        let (slotA, _) = pool.acquire(key: keyA) { [self] _ in createSlot(keyA) }
        pool.release(key: keyA)

        let keyB = makeKey(session: "b")
        let (slotB, _) = pool.acquire(key: keyB) { [self] _ in createSlot(keyB) }
        pool.release(key: keyB)

        pool.evictUnderMemoryPressure()

        XCTAssertEqual(slotA.lifecycle, .evicting)
        XCTAssertEqual(slotA.evictReasons, [.memoryPressure])
        XCTAssertEqual(slotB.lifecycle, .evicting)
        XCTAssertEqual(slotB.evictReasons, [.memoryPressure])
        XCTAssertEqual(pool.slotCount, 0)
    }

    func testActiveSlotNotEvictedByCapacity() {
        let pool = makePool(maxSlots: 1)
        let keyA = makeKey(session: "a")
        let (slotA, _) = pool.acquire(key: keyA) { [self] _ in createSlot(keyA) }

        let keyB = makeKey(session: "b")
        let (slotB, created) = pool.acquire(key: keyB) { [self] _ in createSlot(keyB) }
        XCTAssertTrue(created)
        // maxSlots=1：新 active B 会让 A（background）被淘汰，B 不能淘汰自己
        XCTAssertEqual(slotA.lifecycle, .evicting)
        XCTAssertEqual(slotB.lifecycle, .active)
        XCTAssertEqual(pool.slotCount, 1)
    }

    func testBackgroundPollTouchesAllBackgroundSlots() {
        let pool = makePool(maxSlots: 3)
        let keyA = makeKey(session: "a")
        let (slotA, _) = pool.acquire(key: keyA) { [self] _ in createSlot(keyA) }
        pool.release(key: keyA)

        let keyB = makeKey(session: "b")
        let (slotB, _) = pool.acquire(key: keyB) { [self] _ in createSlot(keyB) }
        pool.release(key: keyB)

        pool.pollBackgroundSlots()

        XCTAssertEqual(slotA.pollCount, 1)
        XCTAssertEqual(slotB.pollCount, 1)
    }

    func testRecentTargetConfigsFromPoolSortedByLastUsed() {
        let pool = makePool(maxSlots: 5)
        now = 100
        let keyA = makeKey(session: "a", path: "/x/a", runtime: "shell")
        _ = pool.acquire(key: keyA) { [self] _ in createSlot(keyA) }
        pool.release(key: keyA)

        now = 200
        let keyB = makeKey(
            session: "b", path: "/x/b", transport: "ssh", alias: "ryzen", runtime: "tmux"
        )
        _ = pool.acquire(key: keyB) { [self] _ in createSlot(keyB) }

        let recents = pool.recentTargetConfigs(limit: 10)
        XCTAssertEqual(recents.map(\.name), ["b", "a"])
        XCTAssertEqual(recents.first?.runtime, .tmux)
        XCTAssertEqual(recents.first?.transport, .ssh(name: "ryzen"))
        XCTAssertEqual(recents.last?.runtime, .shell)
        XCTAssertEqual(recents.last?.path, "/x/a")
    }

    func testRecentAlwaysKeepsActiveTargetFirst() {
        let pool = makePool(maxSlots: 6)
        now = 1
        let initial = makeKey(session: "initial-local", path: "/tmp/local")
        _ = pool.acquire(key: initial) { [self] _ in createSlot(initial) }
        pool.release(key: initial)

        // 模拟历史连接在启动 local workspace 之后被使用，且 Recent 有容量上限。
        for i in 0..<6 {
            now = UInt64(100 + i)
            let key = makeKey(session: "history-\(i)", path: "/tmp/history-\(i)")
            _ = pool.acquire(key: key) { [self] _ in createSlot(key) }
            pool.release(key: key)
        }
        now = 1_000
        _ = pool.acquire(key: initial) { [self] _ in createSlot(initial) }
        pool.slots[initial]?.lastUsedAt = 1

        let recent = pool.recentTargetConfigs(limit: 3)
        XCTAssertEqual(recent.first?.name, "initial-local")
        XCTAssertEqual(recent.count, 3)
    }

    func testCurrentTargetConfigMapsActiveKey() {
        let pool = makePool(maxSlots: 3)
        let key = makeKey(
            session: "yak", path: "/x/yak", transport: "ssh", alias: "ryzen", runtime: "tmux"
        )
        _ = pool.acquire(key: key) { [self] _ in createSlot(key) }
        XCTAssertEqual(pool.currentTargetConfig?.name, "yak")
        XCTAssertEqual(pool.currentTargetConfig?.transport, .ssh(name: "ryzen"))

        pool.release(key: key)
        XCTAssertNil(pool.currentTargetConfig)
    }

    func testRenameActiveTargetUpdatesRecentAndTmuxIdentity() {
        let pool = makePool(maxSlots: 3)
        let key = makeKey(session: "before", path: "/x")
        _ = pool.acquire(key: key) { [self] _ in createSlot(key) }

        pool.renameActiveTarget(to: "after", rekeySession: true)

        XCTAssertEqual(pool.currentTargetConfig?.name, "after")
        XCTAssertEqual(pool.recentTargetConfigs().first?.name, "after")
        XCTAssertEqual(pool.activeKey?.session, "after")
        XCTAssertNil(pool.slots[key])
    }

    func testRenameLocalShellKeepsConnectionIdentity() {
        let pool = makePool(maxSlots: 3)
        let key = makeKey(session: "", path: "/x/project", runtime: "shell")
        _ = pool.acquire(key: key) { [self] _ in createSlot(key) }

        pool.renameActiveTarget(to: "custom", rekeySession: false)

        XCTAssertEqual(pool.currentTargetConfig?.name, "custom")
        XCTAssertEqual(pool.activeKey, key)
        XCTAssertNotNil(pool.slots[key])
    }

    func testConnectionKeyTargetConfigUsesSessionNameOrPathBasename() {
        let tmux = makeKey(session: "sess", path: "/x/y", transport: "ssh", alias: "ryzen")
        let cfg = tmux.targetConfig
        XCTAssertEqual(cfg.name, "sess")
        XCTAssertEqual(cfg.runtime, .tmux)
        XCTAssertEqual(cfg.transport, .ssh(name: "ryzen"))

        let shell = makeKey(session: "", path: "/home/me/proj", runtime: "shell")
        XCTAssertEqual(shell.targetConfig.name, "proj")
        XCTAssertEqual(shell.targetConfig.runtime, .shell)
        XCTAssertEqual(shell.targetConfig.transport, .local)
    }
}
