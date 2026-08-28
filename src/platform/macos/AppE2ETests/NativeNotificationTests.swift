import UserNotifications
import XCTest
@testable import MuxtermAppLib

final class NativeNotificationTests: XCTestCase {
    func testAuthorizationPolicyRequestsOnlyForPendingNotification() {
        XCTAssertFalse(
            NativeNotificationAuthorizationPolicy.shouldRequest(
                .notDetermined,
                hasPendingNotification: false
            ),
            "应用启动时不得脱离通知上下文主动请求权限"
        )
        XCTAssertTrue(
            NativeNotificationAuthorizationPolicy.shouldRequest(
                .notDetermined,
                hasPendingNotification: true
            )
        )
        XCTAssertFalse(
            NativeNotificationAuthorizationPolicy.shouldRequest(
                .denied,
                hasPendingNotification: true
            )
        )
        XCTAssertFalse(
            NativeNotificationAuthorizationPolicy.shouldRequest(
                .authorized,
                hasPendingNotification: true
            )
        )
    }

    func testAuthorizationPolicyDeliversForEveryGrantedState() {
        XCTAssertTrue(NativeNotificationAuthorizationPolicy.canDeliver(.authorized))
        XCTAssertTrue(NativeNotificationAuthorizationPolicy.canDeliver(.provisional))
        XCTAssertFalse(NativeNotificationAuthorizationPolicy.canDeliver(.notDetermined))
        XCTAssertFalse(NativeNotificationAuthorizationPolicy.canDeliver(.denied))
    }

    func testDeniedAuthorizationIsSilentButSystemErrorIsLoggedOnce() {
        XCTAssertFalse(
            NativeNotificationLogPolicy.shouldLogAuthorizationError(
                previouslyLogged: false,
                hasSystemError: false
            ),
            "用户拒绝通知是正常授权状态，不得伪装成运行错误"
        )
        XCTAssertTrue(
            NativeNotificationLogPolicy.shouldLogAuthorizationError(
                previouslyLogged: false,
                hasSystemError: true
            )
        )
        XCTAssertFalse(
            NativeNotificationLogPolicy.shouldLogAuthorizationError(
                previouslyLogged: true,
                hasSystemError: true
            )
        )
    }

    func testNotificationServiceIsSafeAndSilentUnderXCTest() {
        XCTAssertTrue(NativeNotificationService.isSuppressedProcess)
        NativeNotificationService.shared.configure {}
        NativeNotificationService.shared.post(title: "workspace", body: "done")
    }
}
