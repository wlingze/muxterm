import XCTest
@testable import MuxtermChrome

// MARK: - Test doubles

private final class FakeSlot: SceneProtocol {
    var key: SceneKey
    var targetConfig: TargetConfig
    var visibility: SceneVisibility = .hidden
    var lastUsedAt: UInt64
    var evictReasons: [SceneEvictionReason] = []

    init(key: SceneKey, now: UInt64) {
        self.key = key
        self.targetConfig = key.targetConfig
        self.lastUsedAt = now
    }

    func evict(reason: SceneEvictionReason) {
        visibility = .closed
        evictReasons.append(reason)
    }

    func shutdown() {
        visibility = .closed
    }
}

// MARK: - SceneKey

final class SceneKeyTests: XCTestCase {
    func testKeyIncludesTransportAliasSessionRuntimePath() {
        let a = SceneKey(
            transport: "ssh",
            alias: "ryzen",
            session: "yaklang",
            runtime: "tmux",
            path: "/home/wlz/Developer/yaklang-workspace"
        )
        let b = SceneKey(
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
        let local = SceneKey(
            transport: "local", alias: nil, session: "s", runtime: "tmux", path: "/x"
        )
        let ssh = SceneKey(
            transport: "ssh", alias: "ryzen", session: "s", runtime: "tmux", path: "/x"
        )
        XCTAssertNotEqual(local, ssh)
    }

    func testKeyDistinguishesPath() {
        let a = SceneKey(
            transport: "local", alias: nil, session: "s", runtime: "tmux", path: "/a"
        )
        let b = SceneKey(
            transport: "local", alias: nil, session: "s", runtime: "tmux", path: "/b"
        )
        XCTAssertNotEqual(a, b)
    }

    func testKeyDistinguishesTargetSocket() {
        let a = SceneKey(
            transport: "local", alias: nil, session: "s", runtime: "tmux", path: "/x",
            socket: "muxterm-a"
        )
        let b = SceneKey(
            transport: "local", alias: nil, session: "s", runtime: "tmux", path: "/x",
            socket: "muxterm-b"
        )
        XCTAssertNotEqual(a, b)
    }
}

// MARK: - SceneStack

final class SceneStackTests: XCTestCase {
    private var now: UInt64 = 0

    private func makeKey(
        session: String = "s",
        path: String = "/x",
        transport: String = "local",
        alias: String? = nil,
        runtime: String = "tmux",
        socket: String? = nil
    ) -> SceneKey {
        SceneKey(
            transport: transport,
            alias: alias,
            session: session,
            runtime: runtime,
            path: path,
            socket: socket
        )
    }

    private func makePool(
        maxScenes: Int = 2,
        ttlNanoseconds: UInt64? = nil
    ) -> SceneStack<FakeSlot> {
        SceneStack(
            policy: SceneStackPolicy(
                maxScenes: maxScenes,
                ttlNanoseconds: ttlNanoseconds
            ),
            nowProvider: { [weak self] in self?.now ?? 0 }
        )
    }

    private func createSlot(_ key: SceneKey) -> FakeSlot {
        FakeSlot(key: key, now: now)
    }

    func testAcquireCreatesNewSlot() {
        let pool = makePool()
        let key = makeKey()
        let (slot, created) = pool.activate(key: key) { [self] _ in createSlot(key) }
        XCTAssertTrue(created)
        XCTAssertEqual(slot.key, key)
        XCTAssertEqual(slot.visibility, .visible)
        XCTAssertEqual(pool.activeKey, key)
    }

    func testAcquireReusesActiveSlot() {
        let pool = makePool()
        let key = makeKey()
        let (_, created) = pool.activate(key: key) { [self] _ in createSlot(key) }
        XCTAssertTrue(created)

        let (reused, createdAgain) = pool.activate(key: key) { [self] _ in createSlot(key) }
        XCTAssertFalse(createdAgain)
        XCTAssertEqual(reused.key, key)
        XCTAssertEqual(pool.sceneCount, 1)
    }

    func testAcquirePromotesBackgroundSlot() {
        let pool = makePool(maxScenes: 3)
        let keyA = makeKey(session: "a")
        let keyB = makeKey(session: "b")
        let (slotA, _) = pool.activate(key: keyA) { [self] _ in createSlot(keyA) }
        let (_, _) = pool.activate(key: keyB) { [self] _ in createSlot(keyB) }

        // 切回 A：A 从 background 提升为 active
        pool.hide(key: keyB)
        XCTAssertEqual(slotA.visibility, .hidden)
        let (slotA2, created) = pool.activate(key: keyA) { [self] _ in createSlot(keyA) }
        XCTAssertFalse(created)
        XCTAssertEqual(slotA2.key, keyA)
        XCTAssertEqual(slotA2.visibility, .visible)
        XCTAssertEqual(pool.activeKey, keyA)
    }

    func testReleaseMovesToBackgroundWithoutEvicting() {
        let pool = makePool(maxScenes: 3)
        let key = makeKey()
        let (slot, _) = pool.activate(key: key) { [self] _ in createSlot(key) }
        pool.hide(key: key)
        XCTAssertEqual(slot.visibility, .hidden)
        XCTAssertTrue(slot.evictReasons.isEmpty)
        XCTAssertNil(pool.activeKey)
        XCTAssertEqual(pool.sceneCount, 1)
    }

    func testCapacityDoesNotEvictUntilExplicitCleanup() {
        let pool = makePool(maxScenes: 2)
        now = 100
        let keyA = makeKey(session: "a")
        let (slotA, _) = pool.activate(key: keyA) { [self] _ in createSlot(keyA) }
        pool.hide(key: keyA)

        now = 200
        let keyB = makeKey(session: "b")
        let (slotB, _) = pool.activate(key: keyB) { [self] _ in createSlot(keyB) }
        pool.hide(key: keyB)

        now = 300
        let keyC = makeKey(session: "c")
        let (slotC, created) = pool.activate(key: keyC) { [self] _ in createSlot(keyC) }
        XCTAssertTrue(created)
        XCTAssertEqual(slotC.visibility, .visible)

        // maxScenes=2，超过阈值只产生提醒候选；新 C active 后不能静默淘汰 A。
        XCTAssertEqual(slotA.visibility, .hidden)
        XCTAssertTrue(slotA.evictReasons.isEmpty)
        XCTAssertEqual(slotB.visibility, .hidden)
        XCTAssertEqual(pool.sceneCount, 3)
        XCTAssertTrue(pool.isOverCapacity)
        XCTAssertEqual(
            pool.oldestHiddenCandidates(limit: 2).map { $0.key.session },
            ["a", "b"]
        )

        pool.evictForCapacity()
        XCTAssertEqual(slotA.visibility, .closed)
        XCTAssertEqual(slotA.evictReasons, [.capacity])
        XCTAssertEqual(pool.sceneCount, 2)
    }

    func testTTLEvictsExpiredBackgroundSlots() {
        let pool = makePool(maxScenes: 3, ttlNanoseconds: 1_000)
        now = 100
        let keyA = makeKey(session: "a")
        let (slotA, _) = pool.activate(key: keyA) { [self] _ in createSlot(keyA) }
        pool.hide(key: keyA)

        now = 500
        let keyB = makeKey(session: "b")
        let (slotB, _) = pool.activate(key: keyB) { [self] _ in createSlot(keyB) }
        pool.hide(key: keyB)

        now = 2_000
        pool.evictExpired()

        XCTAssertEqual(slotA.visibility, .closed)
        XCTAssertEqual(slotA.evictReasons, [.ttl])
        XCTAssertEqual(slotB.visibility, .closed) // 500+1000=1500 > now=2000 → 应也过期
        XCTAssertEqual(slotB.evictReasons, [.ttl])
        XCTAssertEqual(pool.sceneCount, 0)
    }

    func testMemoryPressureEvictsBackgroundSlots() {
        let pool = makePool(maxScenes: 3)
        let keyA = makeKey(session: "a")
        let (slotA, _) = pool.activate(key: keyA) { [self] _ in createSlot(keyA) }
        pool.hide(key: keyA)

        let keyB = makeKey(session: "b")
        let (slotB, _) = pool.activate(key: keyB) { [self] _ in createSlot(keyB) }
        pool.hide(key: keyB)

        pool.evictUnderMemoryPressure()

        XCTAssertEqual(slotA.visibility, .closed)
        XCTAssertEqual(slotA.evictReasons, [.memoryPressure])
        XCTAssertEqual(slotB.visibility, .closed)
        XCTAssertEqual(slotB.evictReasons, [.memoryPressure])
        XCTAssertEqual(pool.sceneCount, 0)
    }

    func testActiveSlotNotEvictedByCapacity() {
        let pool = makePool(maxScenes: 1)
        let keyA = makeKey(session: "a")
        let (slotA, _) = pool.activate(key: keyA) { [self] _ in createSlot(keyA) }

        let keyB = makeKey(session: "b")
        let (slotB, created) = pool.activate(key: keyB) { [self] _ in createSlot(keyB) }
        XCTAssertTrue(created)
        // maxScenes=1：新 active B 只让 A 保持后台，B 不能因容量被淘汰。
        XCTAssertEqual(slotA.visibility, .hidden)
        XCTAssertEqual(slotB.visibility, .visible)
        XCTAssertEqual(pool.sceneCount, 2)
        XCTAssertTrue(pool.isOverCapacity)
        pool.evictForCapacity()
        XCTAssertEqual(slotA.visibility, .closed)
        XCTAssertEqual(pool.sceneCount, 1)
    }

    func testRecentTargetConfigsFromPoolSortedByLastUsed() {
        let pool = makePool(maxScenes: 5)
        now = 100
        let keyA = makeKey(session: "a", path: "/x/a", runtime: "shell")
        _ = pool.activate(key: keyA) { [self] _ in createSlot(keyA) }
        pool.hide(key: keyA)

        now = 200
        let keyB = makeKey(
            session: "b", path: "/x/b", transport: "ssh", alias: "ryzen", runtime: "tmux"
        )
        _ = pool.activate(key: keyB) { [self] _ in createSlot(keyB) }

        let recents = pool.recentTargetConfigs(limit: 10)
        XCTAssertEqual(recents.map(\.name), ["b", "a"])
        XCTAssertEqual(recents.first?.runtime, .tmux)
        XCTAssertEqual(recents.first?.transport, .ssh(name: "ryzen"))
        XCTAssertEqual(recents.last?.runtime, .shell)
        XCTAssertEqual(recents.last?.path, "/x/a")
    }

    func testAllRecentTargetConfigsIncludesWorkspacesBeyondSoftLimit() {
        let pool = makePool(maxScenes: 20)
        for index in 0..<25 {
            let key = makeKey(session: "workspace-\(index)", path: "/x/workspace-\(index)")
            _ = pool.activate(key: key) { [self] _ in createSlot(key) }
            pool.hide(key: key)
        }

        let all = pool.allRecentTargetConfigs()

        XCTAssertEqual(all.count, 25)
        XCTAssertTrue(all.contains { $0.name == "workspace-24" })
        XCTAssertTrue(pool.isOverCapacity)
    }

    func testRecentAlwaysKeepsActiveTargetFirst() {
        let pool = makePool(maxScenes: 6)
        now = 1
        let initial = makeKey(session: "initial-local", path: "/tmp/local")
        _ = pool.activate(key: initial) { [self] _ in createSlot(initial) }
        pool.hide(key: initial)

        // 模拟历史连接在启动 local workspace 之后被使用，且 Recent 有容量上限。
        for i in 0..<6 {
            now = UInt64(100 + i)
            let key = makeKey(session: "history-\(i)", path: "/tmp/history-\(i)")
            _ = pool.activate(key: key) { [self] _ in createSlot(key) }
            pool.hide(key: key)
        }
        now = 1_000
        _ = pool.activate(key: initial) { [self] _ in createSlot(initial) }
        pool.scenes[initial]?.lastUsedAt = 1

        let recent = pool.recentTargetConfigs(limit: 3)
        XCTAssertEqual(recent.first?.name, "initial-local")
        XCTAssertEqual(recent.count, 3)
    }

    func testCurrentTargetConfigMapsActiveKey() {
        let pool = makePool(maxScenes: 3)
        let key = makeKey(
            session: "yak", path: "/x/yak", transport: "ssh", alias: "ryzen", runtime: "tmux"
        )
        _ = pool.activate(key: key) { [self] _ in createSlot(key) }
        XCTAssertEqual(pool.currentTargetConfig?.name, "yak")
        XCTAssertEqual(pool.currentTargetConfig?.transport, .ssh(name: "ryzen"))

        pool.hide(key: key)
        XCTAssertNil(pool.currentTargetConfig)
    }

    func testRenameActiveTargetUpdatesRecentAndTmuxIdentity() {
        let pool = makePool(maxScenes: 3)
        let key = makeKey(session: "before", path: "/x")
        _ = pool.activate(key: key) { [self] _ in createSlot(key) }

        pool.renameActiveTarget(to: "after", rekeySession: true)

        XCTAssertEqual(pool.currentTargetConfig?.name, "after")
        XCTAssertEqual(pool.recentTargetConfigs().first?.name, "after")
        XCTAssertEqual(pool.activeKey?.session, "after")
        XCTAssertNil(pool.scenes[key])
    }

    func testRenameTmuxTargetPreservesSocketIdentity() {
        let pool = makePool(maxScenes: 3)
        let key = makeKey(session: "before", path: "/x", socket: "muxterm-isolated")
        _ = pool.activate(key: key) { [self] _ in createSlot(key) }

        pool.renameActiveTarget(to: "after", rekeySession: true)

        XCTAssertEqual(pool.activeKey?.socket, "muxterm-isolated")
        XCTAssertEqual(pool.activeKey?.session, "after")
    }

    func testRenameLocalShellKeepsConnectionIdentity() {
        let pool = makePool(maxScenes: 3)
        let key = makeKey(session: "", path: "/x/project", runtime: "shell")
        _ = pool.activate(key: key) { [self] _ in createSlot(key) }

        pool.renameActiveTarget(to: "custom", rekeySession: false)

        XCTAssertEqual(pool.currentTargetConfig?.name, "custom")
        XCTAssertEqual(pool.activeKey, key)
        XCTAssertNotNil(pool.scenes[key])
    }

    func testSceneKeyTargetConfigUsesSessionNameOrPathBasename() {
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

    func testHerdrSceneKeyKeepsWorkspaceIdentitySeparateFromProjectPath() {
        let key = SceneKey(
            transport: "local",
            alias: nil,
            session: "agents",
            runtime: "herdr",
            path: "/Users/me/Developer/muxterm",
            socket: "/Users/me/.config/herdr/sessions/agents/herdr.sock",
            workspaceID: "w7"
        )

        XCTAssertEqual(key.targetConfig.runtime, .herdr)
        XCTAssertEqual(key.targetConfig.path, "/Users/me/Developer/muxterm")
        XCTAssertEqual(key.targetConfig.session, "agents")
        XCTAssertEqual(key.targetConfig.socket, "/Users/me/.config/herdr/sessions/agents/herdr.sock")
        XCTAssertEqual(key.targetConfig.workspaceID, "w7")
    }

    func testResolvedRuntimeIdentityDoesNotDependOnProjectPath() {
        let project = SceneKey(
            transport: "local",
            alias: nil,
            session: "agents",
            runtime: "herdr",
            path: "/Users/me/Developer/muxterm",
            socket: "/Users/me/.config/herdr/sessions/agents/herdr.sock",
            workspaceID: "w7"
        )
        let existing = SceneKey(
            transport: "local",
            alias: nil,
            session: "agents",
            runtime: "herdr",
            path: "",
            socket: "/Users/me/.config/herdr/sessions/agents/herdr.sock",
            workspaceID: "w7"
        )

        XCTAssertEqual(project, existing)
        XCTAssertEqual(Set([project, existing]).count, 1)

        let provisionalA = SceneKey(
            transport: "local",
            alias: nil,
            session: "",
            runtime: "herdr",
            path: "/Users/me/Developer/a"
        )
        let provisionalB = SceneKey(
            transport: "local",
            alias: nil,
            session: "",
            runtime: "herdr",
            path: "/Users/me/Developer/b"
        )
        XCTAssertNotEqual(provisionalA, provisionalB)
    }
}
