import XCTest
@testable import MuxtermAppLib

final class ConnectionDiscoverySocketPolicyTests: XCTestCase {
    private let hostA = SSHHostInfo(alias: "host-a", hostname: "a.example", user: nil, port: nil)
    private let hostB = SSHHostInfo(alias: "host-b", hostname: "b.example", user: nil, port: nil)

    func testLocalSocketIsUsedOnlyFromCurrentLocalConnection() {
        XCTAssertEqual(
            ConnectionDiscoverySocketPolicy.socket(
                for: .local,
                currentSSHHost: nil,
                currentSocket: "muxterm-test-local"
            ),
            "muxterm-test-local"
        )
        XCTAssertNil(ConnectionDiscoverySocketPolicy.socket(
            for: .local,
            currentSSHHost: hostA.alias,
            currentSocket: "muxterm-test-remote"
        ))
    }

    func testRemoteSocketNeverLeaksAcrossHostsOrFromLocal() {
        XCTAssertNil(ConnectionDiscoverySocketPolicy.socket(
            for: .ssh(hostA),
            currentSSHHost: nil,
            currentSocket: "muxterm-test-local"
        ))
        XCTAssertNil(ConnectionDiscoverySocketPolicy.socket(
            for: .ssh(hostA),
            currentSSHHost: hostB.alias,
            currentSocket: "muxterm-test-host-b"
        ))
        XCTAssertEqual(
            ConnectionDiscoverySocketPolicy.socket(
                for: .ssh(hostA),
                currentSSHHost: hostA.alias,
                currentSocket: "muxterm-test-host-a"
            ),
            "muxterm-test-host-a"
        )
    }
}
