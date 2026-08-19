import UserNotifications
import XCTest
@testable import MuxtermAppLib

final class NativeNotificationTests: XCTestCase {
    func testAuthorizationPolicyRequestsOnlyWhenUndetermined() {
        XCTAssertTrue(NativeNotificationAuthorizationPolicy.shouldRequest(.notDetermined))
        XCTAssertFalse(NativeNotificationAuthorizationPolicy.shouldRequest(.denied))
        XCTAssertFalse(NativeNotificationAuthorizationPolicy.shouldRequest(.authorized))
    }

    func testAuthorizationPolicyDeliversForEveryGrantedState() {
        XCTAssertTrue(NativeNotificationAuthorizationPolicy.canDeliver(.authorized))
        XCTAssertTrue(NativeNotificationAuthorizationPolicy.canDeliver(.provisional))
        XCTAssertFalse(NativeNotificationAuthorizationPolicy.canDeliver(.notDetermined))
        XCTAssertFalse(NativeNotificationAuthorizationPolicy.canDeliver(.denied))
    }

    func testNotificationServiceIsSafeAndSilentUnderXCTest() {
        XCTAssertTrue(NativeNotificationService.isSuppressedProcess)
        NativeNotificationService.shared.configure {}
        NativeNotificationService.shared.post(title: "workspace", body: "done")
    }
}
