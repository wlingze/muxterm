import XCTest
@testable import MuxtermChrome

final class TargetOptionSelectionTests: XCTestCase {
    func testDefaultSelectionIsTMUXLocal() {
        let selection = TargetOptionSelection()
        XCTAssertTrue(selection.isSelected(runtime: .tmux))
        XCTAssertFalse(selection.isSelected(runtime: .shell))
        XCTAssertFalse(selection.isSelected(runtime: .herdr))
        XCTAssertTrue(selection.isSelected(transport: .local))
        XCTAssertFalse(selection.isSelected(transport: .ssh(name: "ryzen")))
    }

    func testSelectingRuntimeKeepsExactlyOneSelected() {
        var selection = TargetOptionSelection()
        selection.selectRuntime(.shell)
        XCTAssertTrue(selection.isSelected(runtime: .shell))
        XCTAssertFalse(selection.isSelected(runtime: .tmux))
        selection.selectRuntime(.tmux)
        XCTAssertTrue(selection.isSelected(runtime: .tmux))
        XCTAssertFalse(selection.isSelected(runtime: .shell))

        selection.selectRuntime(.herdr)
        XCTAssertTrue(selection.isSelected(runtime: .herdr))
        XCTAssertFalse(selection.isSelected(runtime: .tmux))
        XCTAssertFalse(selection.isSelected(runtime: .shell))
    }

    func testRuntimeCatalogModelIncludesHerdr() {
        XCTAssertEqual(TargetRuntime.allCases, [.shell, .tmux, .herdr])
    }

    func testSelectingTransportKeepsExactlyOneSelected() {
        var selection = TargetOptionSelection()
        selection.selectTransport(.ssh(name: "ryzen"))
        XCTAssertTrue(selection.isSelected(transport: .ssh(name: "ryzen")))
        XCTAssertFalse(selection.isSelected(transport: .local))
        selection.selectTransport(.local)
        XCTAssertTrue(selection.isSelected(transport: .local))
        XCTAssertFalse(selection.isSelected(transport: .ssh(name: "ryzen")))
    }

    func testAccessibilityIdentifiersAreStableAndExposeSelection() {
        XCTAssertEqual(
            TargetOptionAccessibility.identifier(kind: "runtime", option: "tmux", selected: true),
            "muxterm.target.runtime.tmux.selected"
        )
        XCTAssertEqual(
            TargetOptionAccessibility.identifier(kind: "runtime", option: "tmux", selected: false),
            "muxterm.target.runtime.tmux.unselected"
        )
        XCTAssertEqual(
            TargetOptionAccessibility.identifier(kind: "transport", option: "local", selected: true),
            "muxterm.target.transport.local.selected"
        )
        XCTAssertEqual(
            TargetOptionAccessibility.identifier(kind: "transport", option: "ssh", selected: false),
            "muxterm.target.transport.ssh.unselected"
        )
    }
}
