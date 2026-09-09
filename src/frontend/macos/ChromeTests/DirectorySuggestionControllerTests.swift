import XCTest
@testable import MuxtermChrome

final class DirectorySuggestionControllerTests: XCTestCase {
    func testListingBaseUsesParentWhileTypingPrefix() {
        XCTAssertEqual(
            DirectoryPathModel.baseDirectory(for: "/Users/wlz/Developer/mu"),
            "/Users/wlz/Developer"
        )
        XCTAssertEqual(DirectoryPathModel.inputPrefix(for: "/Users/wlz/Developer/mu"), "mu")
    }

    func testTrailingSlashListsThatDirectoryWithoutPrefix() {
        XCTAssertEqual(
            DirectoryPathModel.baseDirectory(for: "/Users/wlz/Developer/"),
            "/Users/wlz/Developer"
        )
        XCTAssertEqual(DirectoryPathModel.inputPrefix(for: "/Users/wlz/Developer/"), "")
    }

    func testTildeAndRootListingBases() {
        XCTAssertEqual(DirectoryPathModel.baseDirectory(for: "~/Dev"), "~")
        XCTAssertEqual(DirectoryPathModel.inputPrefix(for: "~/Dev"), "Dev")
        XCTAssertEqual(DirectoryPathModel.baseDirectory(for: "~/Dev/"), "~/Dev")
        XCTAssertEqual(DirectoryPathModel.baseDirectory(for: "~"), "~")
        XCTAssertEqual(DirectoryPathModel.baseDirectory(for: "/"), "/")
        XCTAssertEqual(DirectoryPathModel.baseDirectory(for: "/home/"), "/home")
        XCTAssertEqual(DirectoryPathModel.baseDirectory(for: ""), "~")
    }

    func testSelectingCandidateEntersDirectoryWithoutDuplicatingBasename() {
        // 已进入目录（尾斜杠）：候选直接追加，不重复拼接 basename。
        XCTAssertEqual(
            DirectoryPathModel.applyingSelection(candidate: "muxterm", to: "/Users/wlz/Developer/"),
            "/Users/wlz/Developer/muxterm/"
        )
        // 无尾斜杠时最后一段是输入前缀，候选替换该前缀（不拼到完整路径）。
        XCTAssertEqual(
            DirectoryPathModel.applyingSelection(candidate: "muxterm", to: "/Users/wlz/Developer"),
            "/Users/wlz/muxterm/"
        )
    }

    func testSelectingCandidateHandlesTildeRootAndEmpty() {
        XCTAssertEqual(DirectoryPathModel.applyingSelection(candidate: "Dev", to: "~"), "~/Dev/")
        XCTAssertEqual(DirectoryPathModel.applyingSelection(candidate: "Dev", to: "~/"), "~/Dev/")
        XCTAssertEqual(DirectoryPathModel.applyingSelection(candidate: "home", to: "/"), "/home/")
        XCTAssertEqual(DirectoryPathModel.applyingSelection(candidate: "foo", to: ""), "foo/")
    }

    func testSelectingFullPathCandidateIsIgnored() {
        var controller = DirectorySuggestionController(path: "/Users/wlz/Developer")
        let request = controller.request
        let response = controller.select(candidate: "/Users/wlz/Developer/muxterm")
        XCTAssertEqual(controller.text, "/Users/wlz/Developer")
        XCTAssertEqual(response, request, "完整路径候选必须被忽略，不能当 basename 拼接")
    }

    func testSelectingCandidateReplacesTypedPrefixNotWholePath() {
        var controller = DirectorySuggestionController(path: "/Users/wlz/Developer/mu")
        let request = controller.select(candidate: "muxterm")
        XCTAssertEqual(controller.text, "/Users/wlz/Developer/muxterm/")
        XCTAssertEqual(request.path, "/Users/wlz/Developer/muxterm")
        XCTAssertEqual(request.generation, 1)
    }

    func testRepeatedSelectionIsIdempotent() {
        var controller = DirectorySuggestionController(path: "/Users/wlz/Developer/")
        _ = controller.select(candidate: "muxterm")
        let before = controller.text
        let request = controller.select(candidate: "muxterm")
        XCTAssertEqual(controller.text, before, "重复选择同一候选不得再次拼接")
        XCTAssertEqual(request.generation, 1, "重复选择不得触发新请求")
    }

    func testGoUpUsesDirectorySemantics() {
        XCTAssertEqual(
            DirectoryPathModel.applyingGoUp(to: "/Users/wlz/Developer/muxterm/"),
            "/Users/wlz/Developer/"
        )
        XCTAssertEqual(
            DirectoryPathModel.applyingGoUp(to: "/Users/wlz/Developer/muxterm"),
            "/Users/wlz/Developer/"
        )
        XCTAssertEqual(DirectoryPathModel.applyingGoUp(to: "~/Dev"), "~/")
        XCTAssertEqual(DirectoryPathModel.applyingGoUp(to: "~"), "~")
        XCTAssertEqual(DirectoryPathModel.applyingGoUp(to: "/"), "/")
        XCTAssertEqual(DirectoryPathModel.applyingGoUp(to: "/a/b/"), "/a/")
        XCTAssertEqual(DirectoryPathModel.applyingGoUp(to: "~/"), "~")
    }

    func testResolvedPathNormalizesDotsAndTrailingSlash() {
        XCTAssertEqual(DirectoryPathModel.resolvedPath(for: "/Users/wlz/Developer/"), "/Users/wlz/Developer")
        XCTAssertEqual(DirectoryPathModel.resolvedPath(for: "~/Dev/"), "~/Dev")
        XCTAssertEqual(DirectoryPathModel.resolvedPath(for: "~/a/../b"), "~/b")
        XCTAssertEqual(DirectoryPathModel.resolvedPath(for: "/a/./b/"), "/a/b")
        XCTAssertEqual(DirectoryPathModel.resolvedPath(for: "/"), "/")
        XCTAssertEqual(DirectoryPathModel.resolvedPath(for: "~"), "~")
        XCTAssertEqual(DirectoryPathModel.resolvedPath(for: ""), "~")
    }

    func testAsyncResponsesAreGuardedByGenerationAndRequestKey() {
        var controller = DirectorySuggestionController(path: "/Users/wlz/Developer")
        let first = controller.request
        let second = controller.updateInput("/Users/wlz/Developer/mu")
        XCTAssertNotEqual(first.generation, second.generation)

        let stale = DirectoryListingResponse(
            request: DirectoryListingRequest(
                generation: first.generation,
                path: first.path,
                isSSH: first.isSSH,
                alias: first.alias
            ),
            directories: ["muxterm", "old"]
        )
        XCTAssertFalse(controller.apply(stale), "旧 generation 响应必须丢弃")
        XCTAssertTrue(controller.candidates.isEmpty)

        let current = DirectoryListingResponse(
            request: second,
            directories: ["muxterm", "legion"]
        )
        XCTAssertTrue(controller.apply(current))
        XCTAssertEqual(controller.candidates, ["muxterm"], "候选必须按当前输入前缀过滤")
    }

    func testSSHAndLocalRequestsDoNotCrossApply() {
        var controller = DirectorySuggestionController(path: "~")
        let local = controller.request
        let remote = controller.setTransport(isSSH: true, alias: "ryzen")
        XCTAssertTrue(controller.isSSH)
        XCTAssertEqual(controller.alias, "ryzen")
        XCTAssertNotEqual(local, remote)

        let staleLocal = DirectoryListingResponse(request: local, directories: ["local-only"])
        XCTAssertFalse(controller.apply(staleLocal))
        let remoteDirs = DirectoryListingResponse(request: remote, directories: ["remote-only"])
        XCTAssertTrue(controller.apply(remoteDirs))
        XCTAssertEqual(controller.candidates, ["remote-only"])
    }

    func testSwitchingTransportClearsOldCandidates() {
        var controller = DirectorySuggestionController(path: "~")
        _ = controller.apply(
            DirectoryListingResponse(
                request: controller.request,
                directories: ["local-a", "local-b"]
            )
        )
        XCTAssertEqual(controller.candidates, ["local-a", "local-b"])
        _ = controller.setTransport(isSSH: true, alias: "ryzen")
        XCTAssertTrue(controller.candidates.isEmpty)
    }

    func testSelectionRequestUsesEnteredDirectoryAndApplyAcceptsIt() {
        var controller = DirectorySuggestionController(path: "/Users/wlz/")
        let request = controller.select(candidate: "Developer")
        XCTAssertEqual(controller.text, "/Users/wlz/Developer/")
        XCTAssertEqual(request.path, "/Users/wlz/Developer")
        XCTAssertTrue(
            controller.apply(DirectoryListingResponse(request: request, directories: ["muxterm"]))
        )
        XCTAssertEqual(controller.candidates, ["muxterm"])
    }

    func testTypingFullPathKeepsItAsTextAndListsItsParent() {
        var controller = DirectorySuggestionController(path: "/Users/wlz")
        let request = controller.updateInput("/Users/wlz/Developer/muxterm")
        XCTAssertEqual(controller.text, "/Users/wlz/Developer/muxterm")
        XCTAssertEqual(request.path, "/Users/wlz/Developer")
        XCTAssertTrue(
            controller.apply(
                DirectoryListingResponse(request: request, directories: ["muxterm", "muxterm-old", "other"])
            )
        )
        XCTAssertEqual(controller.candidates, ["muxterm", "muxterm-old"])
    }
}
