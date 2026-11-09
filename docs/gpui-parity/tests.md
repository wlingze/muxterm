# macOS test inventory for GPUI parity

## Scope and counting

This inventory covers the Swift files in `src/frontend/macos/AppE2ETests`, `ChromeTests`, and `MuxtermAppUITests`. Paths in the tables are relative to `src/frontend/macos/`. The slice contains 90 Swift files, including the zero-test `AppE2ETests/Harness.swift`, 852 `test...` methods, and 23,482 lines. `MuxtermAppUITests/Info.plist` is configuration, not a test file.

The count is source methods, not executed test cases. It does not prove that a test was discovered, ran, or passed. A file receives one tier and one portability value as requested. The tier identifies its predominant test subject, not necessarily every method: several files mix mocked views, real FFI, and isolated runtimes. Consequently the tier totals below are **file-assigned counts**, not a method-level measurement of runtime use. The row notes name important exceptions. Filesystem and resource-loading tests have no dedicated requested tier; `pure-logic` is the nearest available label and does not mean that those methods are free of I/O.

The assertion descriptions report what the bodies check. Named scenarios that do not prove their advertised outcome are called out below. A Rust port must test behavior through the current product boundary; it must not reproduce obsolete Swift frontend connection pools, concrete runtime branching, or virtual windows. For GUI rows, `needs-gpui-headless-harness` means that Muxterm-specific GPUI mounting, input, or observation support is missing, not that GPUI's generic test framework is absent. `tests/gpui_harness_smoke.rs` now constructs a test view and simulates resize and keystrokes, but has no behavioral assertions and does not mount Muxterm. When a row also needs a real runtime or clipboard, that dependency is stated separately. `needs-real-gui-driver` means input or OS window integration cannot be established by calling a model alone.

## Per-file inventory

### AppE2ETests

| File | Tests | Tier | What the tests actually assert | Portability |
|---|---:|---|---|---|
| `AppE2ETests/AgentRenderE2ETests.swift` | 40 | pixel-or-layout | The tests bound SwiftTerm color caches and reuse glyph measurements; compare fractional-row partial and full paints; and sample readable foreground/background pixels after OSC color changes. Other cases check stable synchronized-output frames, history, wheel routing, SGR mouse input, wide-character columns, selection during live feed, and wrapped URL opening. Some use real tmux or Herdr; several inspect SwiftTerm-specific caches rather than observable behavior. | needs-gpui-headless-harness. Add controlled paint, font, input, and pixel sampling; the runtime cases also need isolated servers. Port cache-size assertions only if GPUI has an equivalent contract. |
| `AppE2ETests/AggregateWorkspaceE2ETests.swift` | 6 | real-runtime-e2e | Local shell input remains one UTF-8 command line without doubled text, and output remains visible after a workspace round trip. Other cases check shell replacement after the last local pane exits, fixed Shells navigation, borrowed agent tabs, and restoration of the selected aggregate tab without closing its source. | needs-gpui-headless-harness. Exercise a local shell and multiple workspace projections through the real FFI. |
| `AppE2ETests/AttachE2ETests.swift` | 12 | real-runtime-e2e | Catalog attach with and without initial client size controls when a surface snapshot arrives. Existing multi-tab and multi-pane sessions must paint their first frame without an extra window resize; recorded agent history and input are also checked after attach. Some cases assert the FFI snapshot before constructing a window. | needs-gpui-headless-harness. Use isolated tmux sessions, preseeded agent frames, and a controllable initial window size. |
| `AppE2ETests/AttentionBadgeE2ETests.swift` | 1 | real-runtime-e2e | A background pane BEL raises the blocked count, after which the test manually sets the status bar from that count and checks its value and bell symbol. It does not prove automatic propagation from the event pump to the status bar. | needs-gpui-headless-harness. Retain the BEL fixture but assert the production event-to-badge path without the manual setter. |
| `AppE2ETests/AttentionNavE2ETests.swift` | 1 | real-runtime-e2e | Two background BELs create two rows and Down changes selection. Command-Enter opens a replica containing the selected pane's token, sends input to that pane without changing the main layout, and toggles closed; Enter selects that pane and closes the panel. | needs-gpui-headless-harness. Requires a real attention-producing pane and keyboard dispatch. |
| `AppE2ETests/AttentionSemanticsE2ETests.swift` | 1 | real-runtime-e2e | Viewing the BEL-blocked pane clears the blocked count; subsequent input keeps it clear. After switching away, `NEED_INPUT` matching the configured regex raises blocked again without another BEL. The test explicitly calls the visibility hook. | needs-gpui-headless-harness. Seed repeated events in an isolated runtime and observe production visibility acknowledgement. |
| `AppE2ETests/BreakPaneE2ETests.swift` | 7 | pixel-or-layout | A floating pane title does not shrink the terminal grid, appears only for splits, and exposes split, close, move, and drag-to-swap actions. The break-pane case checks that a new tab appears without an extra layout level against tmux. | needs-gpui-headless-harness. The geometry cases need hit testing; the break action also needs isolated tmux. |
| `AppE2ETests/ChromeE2ETests.swift` | 22 | pixel-or-layout | Status bar regions, button hit targets, attention state, tab order and widths, and overflow are inspected in AppKit views. Connection popovers expose resolved transport and errors, refresh without reopening, and do not invent SSH upload metrics; a rebuild must not leave orphan or overlapping tab buttons. The file also checks traffic formatting and interval sampling. | needs-gpui-headless-harness. Preserve the observable content, interactions, and non-overlap rather than AppKit subview counts or native icon names. |
| `AppE2ETests/CmdEnterKeyE2ETests.swift` | 10 | pixel-or-layout | IME preedit does not reach the pane, while committed Chinese UTF-8 is sent once. Arrow, page, shifted-arrow, and Enter events reach the intended terminal path; Command-W prioritizes a panel and Command-Enter toggles pane zoom. Some cases use real runtimes, others inject AppKit events into test views. | needs-real-gui-driver. IME composition and actual platform key routing need an OS event source; combine it with isolated shell and tmux fixtures. |
| `AppE2ETests/CommandPaletteParityE2ETests.swift` | 4 | real-runtime-e2e | The palette lists existing navigation, search, font, and timeline actions; choosing search changes the unified panel tab. Detach removes only the current workspace, and palette tab/font actions use the production handlers and persist font size. | needs-gpui-headless-harness. Use isolated tmux and an isolated config directory. |
| `AppE2ETests/CommandTimelineE2ETests.swift` | 1 | real-runtime-e2e | Success and failure marks both become visible. Two previous shortcuts move the viewport into history and two next shortcuts return it to zero at the live tail. | needs-gpui-headless-harness. The terminal fixture must emit command marks. |
| `AppE2ETests/ComposerPanelE2ETests.swift` | 19 | pixel-or-layout | The editor accepts PNG-only clipboard data as an image, retains plain Markdown source and raw image paths, refuses whitespace sends, clears a successfully delivered draft, and preserves a failed one. Other cases check per-pane drafts and undo, focus, menu and shortcut ownership, target badges, and upload feedback. The send callback is a test double, not a real shell. | needs-gpui-headless-harness. Real clipboard and IME routing also need a platform driver. |
| `AppE2ETests/ConnectProgressE2ETests.swift` | 1 | real-runtime-e2e | Starting an unreachable SSH target shows a connecting workspace while an existing workspace remains switchable. | needs-gpui-headless-harness. Use an isolated connection fixture and a deterministic blocked endpoint. |
| `AppE2ETests/ConnectTimeoutE2ETests.swift` | 1 | real-runtime-e2e | The connect request returns in under 0.5 seconds and the window remains visible. The test deliberately ignores the result of waiting for an error, so it does not establish that a timeout or error is eventually displayed. | needs-gpui-headless-harness. Add an independently controlled stalled SSH endpoint and explicitly assert eventual failure. |
| `AppE2ETests/ConnectionDiscoverySocketPolicyTests.swift` | 2 | pure-logic | The current local socket is reused only for a local connection, and a remote socket is reused only for the same SSH host. | port directly-to-rust-unit-test. Keep the host-isolation behavior at the current Catalog boundary rather than copying a Swift-only policy type. |
| `AppE2ETests/DisconnectE2ETests.swift` | 1 | real-runtime-e2e | Disconnect leaves the last terminal frame visible and shows a disconnect watermark. | needs-gpui-headless-harness. Kill only the isolated test server and observe the retained frame. |
| `AppE2ETests/FeatureE2ETests.swift` | 1 | real-runtime-e2e | A seeded multi-pane session yields a search hit, navigates and dismisses search, and exposes an attention row after background command activity. It also checks the recorded terminal/agent tail. | needs-gpui-headless-harness. Use the isolated multi-pane fixture and explicit search and activity events. |
| `AppE2ETests/Harness.swift` | 0 | ffi-integration | This is support code, not a test. It creates isolated config roots, tmux-backed fixtures, app windows, event pumping, and terminal polling. | port to-rust-integration-test. Reuse the fixture design, not the AppKit implementation; every tmux operation and cleanup must retain its isolated socket. |
| `AppE2ETests/HerdrZoomE2ETests.swift` | 1 | real-runtime-e2e | Command-Enter changes the Herdr zoom state and leaves the selected painted pane visible. | needs-gpui-headless-harness. A named, isolated Herdr fixture and rendered-surface observation are required. |
| `AppE2ETests/HistoryE2ETests.swift` | 6 | real-runtime-e2e | Attach history can be scrolled and Jump to Latest returns to the tail; live bytes still reach a history-filled pane. Background-pane scroll state must not control the active pane's button. One case sends an AppKit wheel event; another tests static scroll policies only. | needs-gpui-headless-harness. A platform wheel driver is needed for the OS event case; use isolated history fixtures for the rest. |
| `AppE2ETests/I18nResourceBundleTests.swift` | 7 | pure-logic | A missing resource returns the key ID, three macOS bundle layouts load a fixture string, and every typed key exists in both shipping catalogs. Two cases inspect search roots and development catalog lookup. | port to-rust-integration-test. Preserve missing-key and shipping-catalog checks; the three SPM/`.app` bundle lookup layouts are macOS packaging details, not cross-platform GPUI requirements. |
| `AppE2ETests/ImagePasteBannerTests.swift` | 12 | pixel-or-layout | Progress is visible and not dismissible; success shows a filename with the full path on hover; failure shows and copies the complete error and can be dismissed. The tests also check position, accessibility stage names, localization refresh, and that repeating a stage does not restart its spinner. | needs-gpui-headless-harness. Model the clock, accessibility values, and layout; avoid prescribing AppKit controls. |
| `AppE2ETests/ImagePasteE2ETests.swift` | 5 | pixel-or-layout | TIFF clipboard data is converted to PNG without sending text, and invalid image data reports failure while text retains bracketed-paste behavior. Upload completion targets the source workspace after a switch; the banner progresses, clears, and permits dismissal on error. | needs-gpui-headless-harness. Add an image clipboard adapter, controllable upload completion, and a real-FFI case; actual OS clipboard delivery needs a GUI driver. |
| `AppE2ETests/InactivePaneDimmingE2ETests.swift` | 4 | pixel-or-layout | Only inactive split panes receive dimming; the overlay covers the terminal behind its floating title. Fullscreen clears the dim and a stale snapshot does not dim the currently selected pane. | needs-gpui-headless-harness. Inspect rendered bounds and focus state instead of AppKit overlay objects. |
| `AppE2ETests/LastSeenE2ETests.swift` | 1 | real-runtime-e2e | After switching away and producing more than one screen of lines, returning exposes a last-seen button at the pane's upper right. Clicking it moves the Core viewport and terminal away from the live tail and hides the button; the test does not compare the displayed line with the original marker. | needs-gpui-headless-harness. The fixture must emit enough indexed terminal output to scroll. |
| `AppE2ETests/LiveE2ETests.swift` | 2 | real-runtime-e2e | Command-0 selects the last tab, and live echo plus cursor-positioning output remains visible across status-tab switching. | needs-gpui-headless-harness. Use isolated tmux with terminal input and event pumping. |
| `AppE2ETests/MouseModeAttachE2ETests.swift` | 1 | real-runtime-e2e | Reattach clears stale shell mouse mode without disabling the other pane's TUI mouse mode. | needs-gpui-headless-harness. Seed different mouse modes in two isolated panes. |
| `AppE2ETests/MuxtermInfoWindowTests.swift` | 2 | pixel-or-layout | The palette retains the info controller after the action returns, and copy buttons dispatch and show feedback using a private pasteboard. | needs-gpui-headless-harness. Preserve the user-facing info and copy feedback, not Swift controller ownership. |
| `AppE2ETests/NativeNotificationTests.swift` | 4 | pure-logic | Authorization is requested only for a pending notification, granted states allow delivery, and denial is not logged as a system error. The XCTest-suppression case checks a process flag and calls the service; it does not observe OS notification delivery. | port directly-to-rust-unit-test. The UserNotifications authorization enum and XCTest process-suppression assertion are macOS-specific; real delivery needs a separate platform test. |
| `AppE2ETests/NativePanelLayoutE2ETests.swift` | 5 | pixel-or-layout | Arrow and Emacs keys move panel selection, compact geometry stays inside the owner, and empty states are visible. One row checks the attention color block and normal text. | needs-gpui-headless-harness. Test visible selection and clipping rather than exact AppKit metrics. |
| `AppE2ETests/NotifyBackgroundCommandE2ETests.swift` | 1 | real-runtime-e2e | With foreground and background sleeps running, the test waits for any recorded completion notification; if attention rows exist it checks title content. It never attributes the notification to the background pane, never asserts absence of a foreground notification, and conditionally skips the title assertions when there are no rows. | needs-gpui-headless-harness. Record notification source identity and assert both the background positive and foreground negative cases. |
| `AppE2ETests/PaletteSessionListE2ETests.swift` | 2 | real-runtime-e2e | Local discovery refreshes its session list; the second case checks loopback SSH discovery when its prerequisites are available and otherwise skips. | needs-gpui-headless-harness. Use isolated tmux and an opt-in local SSH fixture with explicit skip reporting. |
| `AppE2ETests/PaneCmdE2ETests.swift` | 1 | real-runtime-e2e | A pane-command subscription supplies the process name used by attention. | needs-gpui-headless-harness. Produce a real pane process change in an isolated session. |
| `AppE2ETests/PanelE2ETests.swift` | 1 | real-runtime-e2e | Search and attention panels open and expose input and list identifiers; Quick Connect must expose either its input or its list. It is an existence check, not a search or connect workflow. | needs-gpui-headless-harness. Preserve accessible controls and test actual workflows separately. |
| `AppE2ETests/PrefsE2ETests.swift` | 1 | real-runtime-e2e | Command-equals increases terminal font size and the change is persisted. | needs-gpui-headless-harness. Use isolated configuration and dispatch a real frontend shortcut. |
| `AppE2ETests/QuickConnectE2ETests.swift` | 5 | pixel-or-layout | A startup local workspace appears in the fixed Shells item, and a tmux status snapshot can be zoomed. The remaining cases check that project names and badge dots fit and that a scroll view fills its available width. | needs-gpui-headless-harness. The runtime case additionally needs isolated tmux. |
| `AppE2ETests/RemoteDirectoryPickerE2ETests.swift` | 1 | pixel-or-layout | Constructing the picker places its confirm button inside the window's content hierarchy; it prevents an AppKit constraint crash. It does not list a remote directory or confirm a selection. | needs-gpui-headless-harness. Replace the AppKit hierarchy assertion with a usable picker and a separate remote listing test. |
| `AppE2ETests/RenameE2ETests.swift` | 1 | real-runtime-e2e | Renaming a tab and workspace reaches an isolated tmux session and appears in Quick Connect. | needs-gpui-headless-harness. Observe both the runtime and the frontend list. |
| `AppE2ETests/RenderE2ETests.swift` | 1 | model-projection | After a cursor-positioning storm, the terminal text contains `frame-19` and not `frame-0`. The test feeds a terminal view; it is not a runtime or screenshot test. | port directly-to-rust-unit-test. Feed the same bytes into the GPUI terminal grid. |
| `AppE2ETests/SSHPortPickerE2ETests.swift` | 7 | pixel-or-layout | Remote ports have Open actions before forwarding; URLs use the resolved target IP and bracket IPv6, and unresolved aliases disable opening. Copy, LAN, forwarding, and opening dispatch distinct actions; columns and controls stay inside the popover. | needs-gpui-headless-harness. Supply fake resolution and forwarding callbacks, then add a real forwarding integration fixture. |
| `AppE2ETests/SearchJumpE2ETests.swift` | 3 | real-runtime-e2e | Search hits in another tab, including tab ID zero, activate the right tab and close the panel. A hit outside the visible viewport is scrolled into view. | needs-gpui-headless-harness. Seed distinct tabs and offscreen indexed text in isolated tmux. |
| `AppE2ETests/SettingsWindowE2ETests.swift` | 14 | pixel-or-layout | Manifest categories drive the sidebar; search changes category without discarding a draft. The shortcut editor searches, disables, saves, and cancels; project editing persists entries and retains an SSH host. Other checks cover control widths, translated text, font rows, and developer-tools access. | needs-gpui-headless-harness. Use a real FFI manifest and temporary config for persistence; avoid hard-coding AppKit's sidebar width. |
| `AppE2ETests/SshAttachE2ETests.swift` | 1 | real-runtime-e2e | When loopback SSH is available, a created SSH session attaches and displays a token while retaining the selected alias. The case explicitly skips when its prerequisites are absent. | needs-gpui-headless-harness. Provide an opt-in loopback SSH server and isolated remote tmux socket. |
| `AppE2ETests/StatusBarOverflowE2ETests.swift` | 1 | real-runtime-e2e | With many real tmux tabs, status-bar overflow retains usable space for the right-hand status area. | needs-gpui-headless-harness. Combine a many-tab fixture with bounds and hit-test observations. |
| `AppE2ETests/SurfaceVisibilityE2ETests.swift` | 41 | pixel-or-layout | Hidden panes retain and receive output, queued frames preserve source grid size and order, and seed completion gates visibility. The tests check overflow recovery, independent pane queues, history backfill, cached hosts, clipboard and large/bracketed paste; one overflow case explicitly round-trips through FFI and tmux. Most assertions use test surfaces and internal counters rather than a real GUI. | needs-gpui-headless-harness. Add controllable event scheduling, terminal grid inspection, fake clipboard, and one isolated-tmux FFI round trip. |
| `AppE2ETests/TabCreationE2ETests.swift` | 4 | real-runtime-e2e | A new tab appears and its surface becomes usable even with busy background tabs; creation activates the runtime's new tab. Visiting and closing tabs retains their surfaces, and newly created panes and tabs receive input focus. | needs-gpui-headless-harness. Generate background output in an isolated multi-tab runtime. |
| `AppE2ETests/TabReorderE2ETests.swift` | 1 | real-runtime-e2e | Dragging a tab changes both the displayed order and the corresponding tmux window order. | needs-gpui-headless-harness. Use pointer drag dispatch and an isolated tmux session. |
| `AppE2ETests/ThemeToggleE2ETests.swift` | 2 | pixel-or-layout | Toggling theme changes chrome and terminal palette; explicit hex colors are reflected in terminal colors. | needs-gpui-headless-harness. Inspect actual foreground/background colors rather than NSColor types. |
| `AppE2ETests/UnifiedPanelActionsE2ETests.swift` | 5 | real-runtime-e2e | Pane and workspace search scopes filter results. Attention preview, mute, and open actions route to the correct background workspace, with mute not activating it. | needs-gpui-headless-harness. Seed attention across isolated workspaces and check selected workspace identity. |
| `AppE2ETests/UnifiedPanelE2ETests.swift` | 8 | real-runtime-e2e | One panel exposes workspace, attention, and search tabs; Tab and Shift-Tab cycle them and shortcuts change tabs without dismissing the panel. The current workspace and its shortcut index follow sidebar order; aggregate and shell shortcuts navigate and dismiss the panel. | needs-gpui-headless-harness. Include multi-workspace fixtures and keyboard routing. |
| `AppE2ETests/UnifiedPanelExistingConnectionsE2ETests.swift` | 9 | pixel-or-layout | Existing rows retain typed Herdr identity and call attach rather than project-connect; stale discovery cannot replace a root after navigating back. Connected workspace search covers path, runtime, alias, session, and identity before discovery finishes. One case discovers and attaches an isolated local session. | needs-gpui-headless-harness. Inject delayed discovery and use isolated tmux for the real attach case. |
| `AppE2ETests/UnifiedPanelLauncherE2ETests.swift` | 4 | pixel-or-layout | Open filters machines before listing their sessions, directory completion confirms a new path, and the root orders its available actions. Searching for Open selects that action. | needs-gpui-headless-harness. Provide deterministic machine, session, and directory results. |
| `AppE2ETests/UnifiedPanelRuntimeUpdateE2ETests.swift` | 8 | pixel-or-layout | Runtime-update buttons appear only when supported, target the chosen workspace without activating or closing it, disable while busy, and allow retry on failure. Mouse hit tests and error routing are checked. `testWorkspacePanelSnapshot` merely writes optional PNGs when `MUXTERM_UI_SNAPSHOT_DIR` is set; it makes no image comparison and otherwise performs no visual assertion. | needs-gpui-headless-harness. Use synthetic capability/status updates plus one real FFI error; replace the snapshot method with an actual visual or geometry oracle. |
| `AppE2ETests/UpdateBannerLiveE2ETests.swift` | 2 | ffi-integration | An isolated config points Core to a local release feed and a sandbox install target; one case drives the available-to-installed banner, and the other exposes a manual-check failure. The tests skip without the configured feed or a matching asset. | port to-rust-integration-test. Supply a deterministic loopback feed, test package, and sandbox target; add GPUI rendering on top. |
| `AppE2ETests/UpdateBannerTests.swift` | 12 | pixel-or-layout | Idle and up-to-date are hidden; available exposes install; checking/installing disable the action; installed requests restart; failure shows a reason and retry. View cases check dismissal per version and callback dispatch. | needs-gpui-headless-harness. The presentation mapping can also be a Rust unit test. |
| `AppE2ETests/WindowFrameStabilityE2ETests.swift` | 5 | real-runtime-e2e | A compact window retains its requested frame while switching tabs and workspaces and opening the sidebar. | needs-real-gui-driver. Window-manager frame changes require an actual OS window and an isolated runtime. |
| `AppE2ETests/WindowFullscreenTests.swift` | 3 | pixel-or-layout | Keyboard, palette, and status-bar actions target the native window rather than pane zoom; maximize/restore returns to the saved frame, and the pane and window shortcuts differ. | needs-real-gui-driver. Fullscreen and maximize semantics belong to the platform window manager. |
| `AppE2ETests/WorkspaceSidebarE2ETests.swift` | 20 | pixel-or-layout | Sidebar rows activate the intended workspace, track selected agent/command pane, hide and reveal commands, and retain selection across metadata refresh. Runtime-update capability controls the button; section collapse and resizing preserve usable geometry. Several real-runtime cases check close/fall-forward and fast, latest-wins workspace switching. | needs-gpui-headless-harness. Combine controllable sidebar events with real isolated workspaces and an event-pump stall fixture. |
| `AppE2ETests/ZoomE2ETests.swift` | 8 | real-runtime-e2e | Zoom projects a split to one visible leaf and pane navigation keeps the next pane zoomed; mouse activation selects the zoom target. Cached tab hosts keep their grid and contents across reveal, and repeated snapshots preserve the host. | needs-gpui-headless-harness. Use an isolated multi-pane runtime; preserve visible continuity rather than Swift object identity. |

### ChromeTests

| File | Tests | Tier | What the tests actually assert | Portability |
|---|---:|---|---|---|
| `ChromeTests/AgentInputRenderRegressionTests.swift` | 7 | model-projection | SwiftTerm's model keeps agent input and cursor rows stable across repeated erase/redraw frames and after a snapshot; an escape fence recovers from a partial OSC. Three paste/width cases only compare the number of distinct row values, so an invisible input that returns row -1 every time passes. They inspect model cells and cursor positions, not GPUI pixels. | port directly-to-rust-unit-test. Replay the fixture bytes into the GPUI terminal model and assert both visibility and row stability. |
| `ChromeTests/AggregateWorkspaceModelTests.swift` | 5 | model-projection | Local tabs precede remote ones, agent tabs use stable workspace/topology order rather than status order, and duplicate tab IDs remain distinct across workspaces. The tests also cover aggregate shortcuts and reject a deferred activation for a changed projection. | port directly-to-rust-unit-test. Test the frontend's public projection, not Swift object identity. |
| `ChromeTests/AttentionModelTests.swift` | 19 | model-projection | Decoding preserves blocked/done notifications and workspace metadata; invalid input returns nil. Read done items disappear, queries filter workspace/process/line, and process titles normalize wrappers, ANSI, and OSC sequences. | port directly-to-rust-unit-test. Reuse the same product event fixtures and expected displayed text. |
| `ChromeTests/CmdEnterRoutingTests.swift` | 3 | pure-logic | The main window maps Command-Enter to tmux pane zoom, an attention panel opens the replica overlay, and the overlay toggles closed. | port directly-to-rust-unit-test. A GPUI event test must separately prove actual focus routing. |
| `ChromeTests/CommandMarkRailTests.swift` | 7 | pixel-or-layout | Failure marks are placed above newer success marks, the latest mark hugs the bottom, and hit testing selects a nearby failure. Track endpoints map to oldest/latest, no marks hides the rail, and the latest caption matches expected text. | port directly-to-rust-unit-test. Geometry and hit-testing math can be tested without an OS window; paint needs a separate view test. |
| `ChromeTests/CommandQueueTests.swift` | 20 | model-projection | Queued input bytes and click press/release stay ordered, while consecutive hover, resize, viewport, color, and visibility updates coalesce only for the same target. Cross-workspace commands and config/search/output request identities remain ordered. These are queue-state assertions, not an FFI delivery test. | port directly-to-rust-unit-test. Rust already has a shared command queue; test the externally significant ordering without requiring an identical queue representation. |
| `ChromeTests/ComposerDraftTests.swift` | 14 | pure-logic | Target badges distinguish SSH and local, whitespace is not sent, and an inserted image path replaces the selection without extra separators. Markdown highlight ranges leave the draft text unchanged, and the composer shortcut does not steal Shift-modified input. | port directly-to-rust-unit-test. Verify both text bytes and ranges. |
| `ChromeTests/ComposerDraftsTests.swift` | 7 | model-projection | Draft pages are scoped to panes, reactivation refreshes the target without losing text, and clearing one page does not clear another. Blank pages are omitted from the tab strip, an unbound page stays usable, and a size policy chooses a portrait window. | port directly-to-rust-unit-test. Window geometry needs a separate view test. |
| `ChromeTests/ConnectProgressTests.swift` | 2 | model-projection | SSH attach log stages map to progress labels and the overlay identifier is stable. No SSH connection is made. | port directly-to-rust-unit-test. Preserve progress semantics; adopt an accessibility identifier appropriate to GPUI. |
| `ChromeTests/DirectorySuggestionControllerTests.swift` | 15 | model-projection | Typed prefixes, trailing slashes, tilde, and root choose the right listing base; selecting a candidate replaces only the appropriate path component. Request generation and transport keys prevent late SSH/local results crossing into a newer query. | port directly-to-rust-unit-test. Use deterministic completions and delayed responses. |
| `ChromeTests/FlatChromeTests.swift` | 199 | pure-logic | This is a collection of many test classes, not one feature. Concrete checks include paste chunk reassembly; keyboard mappings and control bytes; bounded pane resize math; color/OSC query and reply bytes; no loss or reordering of live frames; history seeding; pane focus and zoom; project identity and recent deduplication; keybinding overrides; and TOML escaping. It also tests implementation-specific feed scheduling and five `TabSwitchGateTests` that wait for Core confirmation, which conflict with the current zero-wait switch contract. | port directly-to-rust-unit-test. Split by behavior, preserve byte and state invariants, and do not port SwiftTerm implementation details, the old pool-size setting, or the confirmation gate as frontend requirements. |
| `ChromeTests/GuiLogFilePolicyTests.swift` | 2 | pure-logic | A nonempty log-file path requests stderr redirection; nil, empty, and whitespace-only paths do not. No log file is opened. | port directly-to-rust-unit-test. Keep only if the GPUI launcher exposes the same logging option. |
| `ChromeTests/ImagePasteFeedbackTests.swift` | 18 | model-projection | Encoding/uploading stay visible until a result, a fast result waits for a minimum duration, success disappears before failure, and progress cannot be dismissed. A new paste replaces the previous result; clear or dismiss also removes pending results. | port directly-to-rust-unit-test. Use a fake clock so timing boundaries are deterministic. |
| `ChromeTests/PaneNavigationTests.swift` | 5 | model-projection | Next and previous wrap within the current tab, missing active panes fall back to the first, and zoom navigation uses all panes rather than only the visible leaf. | port directly-to-rust-unit-test. Use duplicate pane IDs in different workspaces in an additional integration case. |
| `ChromeTests/PanelModelTests.swift` | 4 | model-projection | Panel tabs wrap, queries survive tab changes, and search scope defaults to workspace and filters pane/workspace results. | port directly-to-rust-unit-test. The actual panel interaction needs a separate view test. |
| `ChromeTests/PerEventModelRegressionTests.swift` | 3 | model-projection | Repeated erase/up/redraw leaves one input region rather than stacked frames; chunked htop-like bytes preserve `CPU` without replacement glyphs. A Nerd Font/Unicode fixture retains text and places the cursor after a two-cell emoji. The assertions read SwiftTerm cells, not painted pixels. | port directly-to-rust-unit-test. Replay byte chunks in the GPUI terminal model; test font fallback separately. |
| `ChromeTests/ProgressiveWordSelectionTests.swift` | 19 | pure-logic | Repeated double-click expands a path component to a word and then a path; underscores remain in identifiers and git-branch hyphens/stars expand as specified. Token drags include whole words and intervening whitespace. The bracket case only checks delegation to SwiftTerm, not the final bracket selection. | port directly-to-rust-unit-test. Replace the SwiftTerm delegation assertion with the actual selected text in GPUI. |
| `ChromeTests/ProjectConnectFlowTests.swift` | 8 | model-projection | An explicit session name wins over a path basename; attach failure leads to detached creation and then attach, with failures distinguished by stage. `testSSHAndLocalFollowSameProjectSemantics` compares session and state, but compares `local.directory` to itself, so it never compares the SSH directory. | port to-rust-integration-test. Exercise Catalog/Core open behavior for local and SSH without rebuilding the legacy Swift attach/create coordinator; fix the directory oracle. |
| `ChromeTests/QuickPanelActionsTests.swift` | 10 | pure-logic | Contextual workspace actions precede global commands, fixed workspaces omit close/update, hidden attention offers Show, and Escape returns to the prior query level. Popover placement, pixel rounding, and fitting-count math are also checked. | port directly-to-rust-unit-test. Verify ordering and geometry math; pointer hit testing requires a GPUI view test. |
| `ChromeTests/ResizeRedrawRegressionTests.swift` | 3 | pixel-or-layout | After a resize, successive erase/redraw frames leave only `STATUS-D`. Two AppKit/SwiftTerm cases assert grid dimensions with hidden versus visible native scrollers; the latter documents a SwiftTerm-specific defect. | needs-gpui-headless-harness. Port the redraw bytes to the terminal model and test the actual GPUI grid allocation; do not require a SwiftTerm scroller. |
| `ChromeTests/SceneStackTests.swift` | 23 | model-projection | Scene keys distinguish transport, alias, path, session, and socket; the fake stack retains and reselects hidden scenes and orders recents. The capacity, TTL, and memory-pressure tests explicitly evict background slots, and rename/identity cases inspect the old Swift scene pool. | port directly-to-rust-unit-test. Retain identity and user-visible scene continuity as behavior; do not port frontend pool eviction, which conflicts with the current resident-scene contract. |
| `ChromeTests/SearchModelTests.swift` | 4 | model-projection | Search hits decode, tab ID zero is retained, malformed data returns nil, and filtering uses line text and workspace. | port directly-to-rust-unit-test. Feed real FFI-shaped search fixtures. |
| `ChromeTests/ShortcutEditorTests.swift` | 7 | model-projection | Binding adds a chord, disabling removes it, duplicate chords are rejected, and an empty override suppresses a built-in binding. Preset style and duplicate override cases retain valid catalog behavior. | port directly-to-rust-unit-test. The interactive save/cancel flow still needs a view test. |
| `ChromeTests/SidebarSectionSplitLayoutTests.swift` | 5 | pixel-or-layout | Zero or degenerate bounds do not create invalid frames; expanded and collapsed sections fill the sidebar in arranged order while retaining headers. | port directly-to-rust-unit-test. Test layout arithmetic without requiring AppKit. |
| `ChromeTests/StatusBarTests.swift` | 28 | model-projection | Named and xterm colors, style segments, snapshots, and runtime/transport fields decode into status data. Width budgets preserve the right side under overflow, activity counts clamp at zero and follow sidebar priority, and tab lookup uses stable window IDs. | port directly-to-rust-unit-test. Paint and physical hit targets require separate GPUI tests. |
| `ChromeTests/SwiftTermQueryReplyTests.swift` | 5 | model-projection | Feeding OSC 10/11/12 queries into SwiftTerm emits exact foreground, background, and cursor RGB reply bytes; CSI device attributes and multiple replies preserve order. This tests SwiftTerm's delegate, not an FFI round trip. | port directly-to-rust-unit-test. Assert the GPUI terminal's own compatible reply bytes and routing policy. |
| `ChromeTests/TargetOptionSelectionTests.swift` | 5 | model-projection | Default target selection is local tmux; selecting a runtime or transport leaves exactly one selected; Herdr is present in the supplied catalog. Accessibility identifiers expose the selected state. | port directly-to-rust-unit-test. The GPUI accessible tree needs an additional UI test. |
| `ChromeTests/WorkspaceQueryTests.swift` | 4 | pure-logic | Case-insensitive fuzzy search combines runtime and SSH filters, includes attach identity fields, and completes `@` tokens without duplicate aliases. Completion replaces only the current token. | port directly-to-rust-unit-test. Use typed Catalog identities rather than parsing runtime-specific IDs. |
| `ChromeTests/WorkspaceReplicaIDTests.swift` | 5 | model-projection | SSH session identities retain project path, same-named sessions at different paths stay distinct, and Herdr replica identity uses workspace ID rather than project path. Empty sessions fall back to the path basename. | port directly-to-rust-unit-test. Test equality and routing through product IDs, not a frontend-owned concrete runtime type. |
| `ChromeTests/WorkspaceSidebarModelTests.swift` | 22 | model-projection | Disconnected workspaces stay listed but leave activity aggregates; agent identity includes workspace, rejects older versions, and survives read state until pane closure. Agent/command projections use stable tab and pane IDs, classify wrappers correctly, sort by status, and route attention jumps to the right workspace. Runtime-update success has a distinct button state. | port directly-to-rust-unit-test. Use structured activity and attention fixtures, including duplicate pane IDs in different workspaces. |

### MuxtermAppUITests

| File | Tests | Tier | What the tests actually assert | Portability |
|---|---:|---|---|---|
| `MuxtermAppUITests/MuxtermAppUITests.swift` | 22 | xcuitest-drives-real-app | XCUITest launches the app and uses keyboard, pointer, accessibility values, and isolated tmux sessions to check command echo, split and tab layout, per-pane input, search, BEL, disconnect retention, attach history, zoom, and the status popover. It checks split/tab feedback under two seconds and several Ctrl-D close cases. The suite skips without an Aqua GUI session, when explicitly disabled, or when required tmux fixtures cannot be created. `testVerticalDividerCanBeDraggedWithoutBreakingLayout` checks that the post-drag layout is reasonable but does not compare divider position before and after the drag. | needs-real-gui-driver. A cross-platform driver must inspect the GPUI accessible tree, send native input, and observe window frames; run it in a logged-in graphical session with isolated tmux. |

### Method-level evidence in the largest files

These cases explain the grouped rows above. Each selector names an existing method; the observations are from its assertions, not its title.

| File | Concrete assertions |
|---|---|
| `AppE2ETests/AgentRenderE2ETests.swift` | `testSelectionDuringSynchronizedOutputKeepsLastCompletePixels` compares the bitmap before and during an incomplete synchronized frame, then expects changed pixels and `COMPLETE_NEW_FRAME` after it completes. `testPartialPaintMatchesFullPaintAtFractionalRowBoundary` compares the complete bitmap bytes from partial and full paints. `testServerScrollUsesWheelDirectionWithoutLocalHistory` expects wheel deltas `[4, -3]`, no focus activations, and no local scrollback. `testHerdrWheelPassesItsPaneCellToServer` checks a nonzero in-bounds cell and no ordinary input bytes. `testWrappedImplicitUrlOpensWholeLinkFromContinuationRow` checks the opened URL equals the complete wrapped link. `testPrependHistoryUsesCJKColumnWidth` checks adjacent Chinese glyphs at columns 0 and 2. |
| `AppE2ETests/SurfaceVisibilityE2ETests.swift` | `testResizeBatchKeepsFrameAndDiffOrder` requires `LATEST_FRAME` and `NEW_INPUT` while rejecting `STALE_DIFF`. `testHostStaysHiddenUntilSeedAndLiveCatchupFinish` keeps the host hidden until a 96 KiB seed and `LIVE_CATCHUP` are applied. `testActivePaneOutputOverflowFencesOnlyThatSurfaceQueue` requests one replacement snapshot after a 3 MiB flood and excludes the dropped bytes while admitting the fresh frame and live suffix. `testProductionOverflowRequestRoundTripsThroughFfiAndTmux` checks that a real tmux snapshot recovers the token and excludes the injected `z` flood. `testBracketedLargePasteWrapsTheWholePayloadOnce` checks one begin/end wrapper around all chunks. |
| `ChromeTests/FlatChromeTests.swift` | `testLargePasteChunksReassembleWithoutDroppingBytes` checks three chunks and exact reassembly, including the seven-byte tail. `testFairPaneFeedSchedulerDoesNotLetChattyPaneStarveQuietPane` checks that a one-byte quiet pane runs between two 16 KiB chunks from a busy pane; that is a scheduler assertion, not a GUI latency measurement. `testUnexpectedSingleFFIsRejectedWithoutChangingMouseOrBackspaceBytes` rejects a lone `0xff` but accepts backspace, UTF-8 Chinese and a mouse report containing `0xff`. `testSshAttachBackendDoesNotPutAliasInSocket` checks that alias `ryzen` is not passed as the tmux socket. `testTomlEscapesQuotesAndBackslashes` encodes and decodes a quoted name and path. `testRequestBlocksUntilConfirmation` instead asserts an obsolete confirmation gate and is excluded below. |
| `ChromeTests/WorkspaceSidebarModelTests.swift` | `testStructuredAgentRegistryRejectsOlderVersion` keeps the newer working status until an accepted update changes it to idle. `testStructuredAgentRegistryIsolatesSamePaneIdAcrossWorkspaces` checks that pane 1's agent does not leak to another workspace's pane 1. `testCoreClassifiedWrapperAgentGoesToAgentsNotCommands` expects one cursor agent and no command rows. `testCursorProcessIsNotLabeledCodexAndIdleShellStaysOutOfAgents` checks cursor remains named cursor and the idle shell is absent. `testPanelJumpRoutingRequiresMatchingWorkspaceForSharedPaneId` accepts the matching workspace/pane pair and rejects a different pane. |
| `MuxtermAppUITests/MuxtermAppUITests.swift` | `testTypedCommandIsEchoedAndPrintedExactlyOnce` counts exactly two occurrences of a marker, one expected in command echo and one in output. `testCtrlDMultiTabClosesOnlyCurrentTab` keeps the window and waits for the tab count to fall from two to one. `testCtrlDClosesOnlyCurrentPane` waits for the pane count to fall from two to one. `testDisconnectKeepsLastFrameAndShowsOverlay` waits for the previous marker after disconnect and for an overlay. The helper used by several other tests can accept a marker from command echo, as noted below. |

## Summary 1: tests per tier

Each method is counted once in its file's assigned tier, including mixed files. The zero-test support file contributes one file and no methods.

| Tier | Files | Test methods |
|---|---:|---:|
| pure-logic | 10 | 264 |
| model-projection | 21 | 213 |
| ffi-integration | 2 | 2 |
| real-runtime-e2e | 31 | 84 |
| xcuitest-drives-real-app | 1 | 22 |
| pixel-or-layout | 25 | 267 |
| **Total** | **90** | **852** |

## Summary 2: GPUI acceptance suite

These selectors identify the source tests that supply a GPUI acceptance scenario. `A/`, `C/`, and `U/` mean `AppE2ETests/`, `ChromeTests/`, and `MuxtermAppUITests/`; append `.swift` to each listed basename. `*` means every test method in that file, except the explicitly named removals in Summary 3. A class selector means every test method in that class. The acceptance requirement is the user-observable outcome described in the last column, not Swift class identity, a private call count, or a particular widget implementation. Where a method combines both, retain the scenario and replace its internal assertion. Weaker source tests remain listed for the behavior they actually prove; the coverage gaps below are additional requirements, not claims that the source test already proves them.

| Source tests | Required observable outcome |
|---|---|
| `A/AgentRenderE2ETests::*`, `A/RenderE2ETests::*`, `C/AgentInputRenderRegressionTests::*`, `C/PerEventModelRegressionTests::*`, `C/ResizeRedrawRegressionTests::*` | Redraws preserve the latest frame, cursor, cell widths, readable colors, selection, and response bytes across attach, resize, frame boundaries, and synchronized output. Do not substitute model cells for paint or paint for model-cell checks. |
| `A/SurfaceVisibilityE2ETests::*`, `A/AttachE2ETests::*`, `A/HistoryE2ETests::*`, `A/MouseModeAttachE2ETests::*` | Initial and hidden surfaces retain ordered output, snapshots and live bytes; reattach displays the right grid, history and mouse modes without an extra resize. Copy, paste and backfill must not lose data. |
| `A/AggregateWorkspaceE2ETests::*`, `C/AggregateWorkspaceModelTests::*` | Shells and agent projections show the right source tabs, retain identity across workspaces, restore selection, and remain usable after switching or closing a projection. |
| `A/AttentionBadgeE2ETests::*`, `A/AttentionNavE2ETests::*`, `A/AttentionSemanticsE2ETests::*`, `A/NotifyBackgroundCommandE2ETests::*`, `C/AttentionModelTests::*` | BEL, blocked and done activity appear with the correct title and navigation behavior. Attribute notifications to their source and explicitly check foreground suppression; the existing background notification and badge tests have weaker oracles. |
| `A/BreakPaneE2ETests::*`, `A/InactivePaneDimmingE2ETests::*`, `A/ZoomE2ETests::*`, `C/PaneNavigationTests::*` | Split, move, swap, close, focus, dim, zoom and navigate panes without losing terminal contents or selecting a pane in another tab. |
| `A/TabCreationE2ETests::*`, `A/TabReorderE2ETests::*`, `A/LiveE2ETests::*`, `A/LastSeenE2ETests::*` | Create, close, switch and reorder tabs while output and focus stay correct; return to recorded scroll positions and retain visible feedback under load. |
| `A/ChromeE2ETests::*`, `A/StatusBarOverflowE2ETests::*`, `C/StatusBarTests::*`, `C/CommandMarkRailTests::*` | Tab/status controls fit, remain clickable, display the correct connection and activity state, and navigate command marks. SSH metrics must not be fabricated. |
| `A/CmdEnterKeyE2ETests::*`, `C/CmdEnterRoutingTests::*` | Route shortcuts by focus, forward terminal key bytes exactly once, preserve IME preedit versus committed text, and distinguish pane zoom from overlay behavior. |
| `A/CommandPaletteParityE2ETests::*`, `A/PanelE2ETests::*`, `A/NativePanelLayoutE2ETests::*`, `C/PanelModelTests::*`, `C/QuickPanelActionsTests::*` | Discover and invoke existing commands; keep panel selection, query, scope, accessible controls, and contextual actions usable. |
| `A/UnifiedPanelE2ETests::*`, `A/UnifiedPanelActionsE2ETests::*`, `A/UnifiedPanelExistingConnectionsE2ETests::*`, `A/UnifiedPanelLauncherE2ETests::*`, `A/QuickConnectE2ETests::*`, `C/WorkspaceQueryTests::*` | Unified search, attention and workspace actions route to the right workspace; existing sessions attach, stale discovery cannot overwrite navigation, and machine/path selection works. Names and badge dots remain visible as the panel resizes. |
| `A/SearchJumpE2ETests::*`, `A/CommandTimelineE2ETests::*`, `C/SearchModelTests::*` | Search results, including tab zero and offscreen hits, select the correct tab and scroll position; command marks navigate to the selected output. |
| `A/ComposerPanelE2ETests::*`, `C/ComposerDraftTests::*`, `C/ComposerDraftsTests::*` | Drafts remain scoped to panes, preserve source and undo, retain failed sends, clear successful sends, and send exact nonblank text to the intended target. |
| `A/ImagePasteE2ETests::*`, `A/ImagePasteBannerTests::*`, `C/ImagePasteFeedbackTests::*` | Image data, errors, original workspace routing, progress, dismissal and accessibility work through clipboard and upload; ordinary text remains a text paste. |
| `A/ConnectProgressE2ETests::*`, `A/ConnectTimeoutE2ETests::*`, `A/SshAttachE2ETests::*`, `A/PaletteSessionListE2ETests::*`, `A/ConnectionDiscoverySocketPolicyTests::*`, `C/ConnectProgressTests::*`, `C/DirectorySuggestionControllerTests::*` | Connecting and discovering remain responsive and scoped to the selected host; local and SSH sessions attach correctly, late directory replies do not overwrite a newer target, and errors are visible. |
| `A/DisconnectE2ETests::*`, `A/FeatureE2ETests::*`, `A/PaneCmdE2ETests::*`, `A/HerdrZoomE2ETests::*` | Disconnect retains the last frame; search, command activity, process identity and supported runtime zoom reach the visible frontend. |
| `A/PrefsE2ETests::*`, `A/SettingsWindowE2ETests::*`, `A/ThemeToggleE2ETests::*`, `C/ShortcutEditorTests::*`, `C/TargetOptionSelectionTests::*` | Font, theme, shortcut and target changes take effect; settings search retains drafts and project edits persist, including SSH host identity. |
| `A/RemoteDirectoryPickerE2ETests::*`, `A/SSHPortPickerE2ETests::*` | Directory selection stays usable; port open, forwarding, copy and LAN actions route separately and use the resolved host, including IPv6. The picker hierarchy test alone does not prove remote listing. |
| `A/UnifiedPanelRuntimeUpdateE2ETests::*`, `A/UpdateBannerTests::*`, `A/UpdateBannerLiveE2ETests::*` | Capability-scoped update actions target the chosen workspace; busy, failure, retry, dismissal and install results are visible. The source snapshot-only method is excluded below. |
| `A/WorkspaceSidebarE2ETests::*`, `C/WorkspaceSidebarModelTests::*`, `C/SidebarSectionSplitLayoutTests::*`, `C/WorkspaceReplicaIDTests::*` | Sidebar selection and activity match the active workspace/pane, identities do not collide, sections remain usable, and switching is immediate even when background events are delayed. |
| `A/RenameE2ETests::*`, `C/ProjectConnectFlowTests::*` | Renamed items remain discoverable and project opening follows the selected target. Test attach/create outcomes through Catalog/Core, not the obsolete Swift coordinator, and fix the SSH directory oracle. |
| `A/WindowFrameStabilityE2ETests::*`, `A/WindowFullscreenTests::*`, `A/MuxtermInfoWindowTests::*` | Workspace/tab navigation does not unexpectedly move the window; maximize/fullscreen are distinct from pane zoom; info copy actions show feedback. |
| `A/I18nResourceBundleTests::testMissingResourceBundleFallsBackToKeyId`, `A/I18nResourceBundleTests::testEveryTypedKeyExistsInBothShippingCatalogs` | Missing translations have a deterministic fallback and both shipping catalogs contain the frontend's typed keys. |
| `A/NativeNotificationTests::testAuthorizationPolicyRequestsOnlyForPendingNotification`, `A/NativeNotificationTests::testAuthorizationPolicyDeliversForEveryGrantedState`, `A/NativeNotificationTests::testDeniedAuthorizationIsSilentButSystemErrorIsLoggedOnce` | Permission is requested in notification context, granted delivery is allowed, and ordinary denial is not reported as a system error. Real platform delivery requires another test. |
| `C/CommandQueueTests::*`, `C/ProgressiveWordSelectionTests::*`, `C/SwiftTermQueryReplyTests::*` | Input and output are ordered and target-scoped; selection expands by word/path; terminal queries produce and route correct replies. Queue implementation and SwiftTerm delegation are not prescribed. |
| `C/FlatChromeTests::FlatChromeTests::*`, `LargePastePlanTests::*`, `KeyBindingsTests::*`, `TerminalMirrorPolicyTests::*`, `MuxtermTerminalColorsTests::*`, `MuxtermThemeTests::*`, `MuxtermTerminalFontTests::*`, `MuxtermIssueReportTests::*`, `TerminalQueryDetectorTests::*` | Preserve the user-visible last-seen, paste, shortcut, terminal color/font, diagnostics and query behavior in this multi-class file. The FlatChrome test of an internal architecture budget is excluded below. |
| `C/FlatChromeTests::PaneOutputFeedPolicyTests::*`, `PanePaintPolicyTests::*`, `WheelPassthroughPolicyTests::*`, `KeyPassthroughPolicyTests::*`, `PaneHistoryScrollPolicyTests::*`, `ColorContrastTests::*`, `DefaultColorGuardTests::*`, `ScreenTextTests::*` | Retain the visible result of ordered live/seed feeds, history, mouse/keyboard routing, readable colors, and screen text. Rewrite scheduler, coalescing and SwiftTerm-specific mechanics against the current frontend boundary. |
| `C/FlatChromeTests::WorkspaceDisconnectOverlayPolicyTests::*`, `PaneLayoutProjectionTests::*`, `PaneHistorySeedPolicyTests::*`, `PaneFullscreenPolicyTests::*`, `InactivePaneDimmingPolicyTests::*`, `PaneResizeMathTests::*`, `TerminalInputEncodingTests::*`, `TerminalQueryReplyTests::*` | Preserve disconnect overlay, layout/focus, history, pane zoom, dimming, resize, terminal input and reply behavior. Object reuse and internal reload flags alone are not acceptance oracles. |
| `C/FlatChromeTests::QuickConnectModelTests::*`, `QuickConnectStoreTests::*`, `KeyBindingsConfigTests::*`, `DeveloperToolAccessPolicyTests::*` | Preserve project/target identity and visible recent lists, shortcut configuration, and developer-tool access through the current Core and frontend interfaces. |
| `C/SceneStackTests::testKeyIncludesTransportAliasSessionRuntimePath`, `testKeyDistinguishesAlias`, `testKeyDistinguishesPath`, `testKeyDistinguishesTargetSocket`, `testRecentTargetConfigsFromPoolSortedByLastUsed`, `testAllRecentTargetConfigsIncludesWorkspacesBeyondSoftLimit`, `testRecentAlwaysKeepsActiveTargetFirst`, `testHerdrSceneKeyKeepsWorkspaceIdentitySeparateFromProjectPath`, `testResolvedRuntimeIdentityDoesNotDependOnProjectPath` | Distinct targets and recents must remain distinguishable and correctly ordered. The Swift pool itself is not the acceptance interface. |
| `U/MuxtermAppUITests::*` | Exercise the actual GPUI application with native input and an accessible tree: window launch, command input, split and tab layout, per-pane I/O, Ctrl-D, search, attention, disconnect, history, zoom and status popover. Strengthen the divider and command-output checks described below. |

### Coverage gaps in the source tests

| Source | What the body proves | Required additional oracle |
|---|---|---|
| `ChromeTests/ProjectConnectFlowTests.swift:81` | `XCTAssertEqual(local.directory, local.directory)` is tautological; only session and state are compared with SSH. | Compare each selected local and SSH path against the expected path, then observe the target through Catalog/Core. |
| `AppE2ETests/NotifyBackgroundCommandE2ETests.swift:8` | At least one completion notification is recorded; the test does not establish which pane caused it or forbid a foreground notification. | Record source workspace and pane. Assert one background completion and no notification for the foreground command. |
| `AppE2ETests/AttentionBadgeE2ETests.swift:8` | BEL changes the Core count, then the test manually calls `setAttention`. | Observe the visible badge update through the production event path without manually setting it. |
| `AppE2ETests/ConnectTimeoutE2ETests.swift:8` | The initial call returns quickly and the window remains visible; the eventual-error wait is ignored. | Control the stalled connection and assert an eventual error or timeout state while other workspace input still works. |
| `ChromeTests/AgentInputRenderRegressionTests.swift:150,219,252` | Three redraw tests assert only that `Set(rows).count == 1`; `inputRow` returns -1 if neither expected marker is visible, so all frames can disappear and still pass. | Assert a nonnegative row and the expected visible input on every frame, then assert that the row is stable. |
| `AppE2ETests/LastSeenE2ETests.swift:7` | After clicking last-seen, the test checks only that the viewport is away from the tail and the button is hidden. | Assert the actual recorded line or an independently anchored position, not just a nonzero offset. |
| `AppE2ETests/PanelE2ETests.swift:7` and `RemoteDirectoryPickerE2ETests.swift:6` | The panels or confirm control exist; neither test completes the named search/connect/directory flow. | Drive an actual query and a remote listing/selection, then verify the selected target. |
| `MuxtermAppUITests/MuxtermAppUITests.swift:218` | A drag leaves three panes in a reasonable layout; no before/after divider position is compared. | Move the divider by a measurable amount and compare pane bounds before and after while verifying input remains usable. |
| `MuxtermAppUITests/MuxtermAppUITests.swift:281,730` | `testBackspaceDeletesCharacterInShell` and `echoAndVerifyVisible` search for a marker that may already be present in the echoed command. | Assert a distinct output-only marker or a controlled command response, not merely text also present in input. |
| `AppE2ETests/NativeNotificationTests.swift:63` | The test checks the XCTest suppression flag and calls the service; it does not inspect delivered OS notifications. | Use an isolated notification adapter or platform driver to observe actual delivery and suppression. |

## Summary 5: GPUI 覆盖现状（2026-11-09）

上面的清单回答「Swift 有哪些测试」；这一节回答「gpui 现在有没有对应的测试」。
统计方式：

```sh
rg -c '#\[(gpui::)?test\]' src/frontend/gpui -g '*.rs' | awk -F: '{s+=$2} END {print s}'
rg -c '#\[(gpui::)?test\]' tests/gpui_*.rs | awk -F: '{s+=$2} END {print s}'
```

当前（2026-11-09 实测）：gpui 模块内 **477** 个单测（按文件拆，不照抄 Swift 的巨型
`FlatChromeTests`），集成测试 **108** 个：`gpui_chrome` 74、`gpui_workspace_picker` 13、
`gpui_acceptance` 11、`gpui_acceptance_herdr` 2、`gpui_surface_events` 6、
`gpui_surface_e2e` 1、`gpui_harness_smoke` 1。加上 lib 里其它前端共享模块，`--lib`
总计 1919 个测试。

### 5.1 纯逻辑层（ChromeTests 对应）

| Swift 文件（方法数） | gpui 对应 | 状态 |
|---|---|---|
| AttentionModelTests (19) | model/attention.rs (23)，含 Core 活动快照映射与 replica id join | 已覆盖 |
| AggregateWorkspaceModelTests (5) | model/aggregate.rs (7) | 已覆盖 |
| ComposerDraftTests (14) / ComposerDraftsTests (7) | model/composer.rs (23) | 已覆盖 |
| DirectorySuggestionControllerTests (15) | model/directory.rs (13) + state/picker.rs | 部分（目录建议合并逻辑在 state 层，未逐条对照） |
| ImagePasteFeedbackTests (18) | model/image_paste.rs (18) | 已覆盖 |
| PaneNavigationTests (5) | model/pane_navigation.rs (7，逐条对照五个用例) + state.rs `move_pane_focus` + gpui_chrome（布局叶子 / zoom）+ gpui_acceptance | 已覆盖（2026-11-09 起 Cmd-[ / Cmd-] 走显式 `SwitchPane`，不再交给 Core 推断） |
| PanelModelTests (4) / QuickPanelActionsTests (10) | model/panel.rs (5) + chrome/panel_view.rs (7) + gpui_chrome 面板组 | 部分（行内动作集已覆盖，面板窗口布局未做像素对照） |
| ProgressiveWordSelectionTests (19) | model/word_selection.rs (22) | 已覆盖 |
| ProjectConnectFlowTests (8) | model/workspace_picker.rs + model/quick_connect.rs (21) + gpui_workspace_picker (13) | 部分（源测试自带假断言，已按文档要求改成观察 Core 结果） |
| ResizeRedrawRegressionTests (3) | model/resize_math.rs (7) + chrome/pane_grid.rs (5) | 部分（重绘时序没有逐帧对照） |
| SearchModelTests (4) | model/search.rs (7) | 已覆盖 |
| ShortcutEditorTests (7) | model/shortcuts.rs (20) + state/palette_actions.rs | 已覆盖 |
| SidebarSectionSplitLayoutTests (5) | chrome/sidebar_view.rs (8) | 已覆盖 |
| StatusBarTests (28) | model/status_bar.rs (30) | 已覆盖 |
| SwiftTermQueryReplyTests (5) | model/query_reply.rs (8) + model/query_detector.rs (9) | 已覆盖 |
| SwiftTerm 1.15 属性绘制（`getAttributes` / `mapColor`，无同名 Swift 测试） | chrome/pane_view.rs (8，切段与颜色) + theme.rs（提亮 / dim 数学）+ gpui_chrome（`pane-bold` 等渲染断言） | 已覆盖（粗体提亮 0–6、dim 对半混合、反显后再提亮、隐藏格保宽度） |
| TargetOptionSelectionTests (5) | model/target.rs (7) + chrome/target_editor.rs | 已覆盖 |
| WorkspaceQueryTests (4) | model/quick_connect.rs (21) | 已覆盖 |
| WorkspaceReplicaIDTests (5) | model/replica_id.rs (11) + 验收里的 join 用例 | 已覆盖 |
| WorkspaceSidebarModelTests (22) | model/sidebar.rs (21) | 已覆盖 |
| CommandQueueTests (20) | command_queue.rs (4) + gpui_chrome 命令队列断言 | 部分（KeyAction 编解码在 model/shortcuts，队列本身用例较少） |
| CmdEnterRoutingTests (3) | model/shortcuts.rs（Cmd-Enter / Alt-Enter）+ state.run_action | 部分 |
| AgentInputRenderRegressionTests (7) | terminal/parser.rs (19) + model/surface_feed.rs (14) + gpui_surface_events (6) | 部分 |
| PerEventModelRegressionTests (3) | model/surface_feed.rs (14) | 已覆盖 |
| ConnectProgressTests (2) | model/existing.rs (6) + state/picker.rs | 部分 |
| GuiLogFilePolicyTests (2) | chrome/info.rs (3，日志开/关两条路径) | 部分 |
| CommandMarkRailTests (7) | 无 | **缺口**（命令刻度轨还没移植） |
| FlatChromeTests (199) | 拆到 chrome/* 与 model/* 各文件 | 部分（它是 12 个类的合集，逐类对应见上；剩余差异在调度/合并等内部机制） |
| SceneStackTests (23) | 不移植 | 差异（池/场景栈归 Core，见 Summary 3） |

### 5.2 真实运行层（AppE2ETests 对应）

gpui 用 `tests/gpui_acceptance*.rs` 跑隔离 tmux / herdr，覆盖：

- 启动即有 workspace、输入进 shell；
- Cmd-T 新 tab、Cmd-D 分屏、Cmd-[ / Cmd-] 换 pane（焦点跟随）；
- Rename Tab、Move Tab 走 Core 往返；
- 命令面板 Open 流程真的建出 workspace（tmux / herdr 各一条）；
- 活动快照 join 到前端 workspace id（Core 有 agent 时侧栏 Agents 有行）。

其余 AppE2ETests（约 400 个方法）里，凡是需要真实 AppKit 窗口、原生通知、系统剪贴板、
SSH 远端或 GPU 绘制的，gpui 侧目前用「模型断言 + 无头 harness」替代；剩余未覆盖项：

| 缺口 | 说明 |
|---|---|
| 终端选区复制 / Select All | gpui 还没有鼠标拖选；菜单 `Copy` 仍是禁用项 |
| 命令刻度轨 / CommandTimeline | 没有对应的模型与断言 |
| 原生通知 | gpui 未接通知通道 |
| SSH / 端口转发 / 更新横幅 | 属于 Core 能力，gpui 只做了面板入口 |
| 逐帧像素对照（InactivePaneDimming / Zoom 等） | 用状态断言替代，没有做像素级对比 |

## Summary 3: source tests that can be dropped or replaced

Dropping the Swift-specific test is not permission to drop the user-visible outcome. The table states whether a behavioral replacement is required.

| Source tests or assertions | Reason and replacement |
|---|---|
| `AppE2ETests/Harness.swift` | Zero test methods. Replace the fixture with Rust support rather than counting it as coverage. |
| `AppE2ETests/UnifiedPanelRuntimeUpdateE2ETests.swift::testWorkspacePanelSnapshot` | Optional PNG export, no comparison, and no work when the snapshot directory is unset. Replace with explicit bounds/pixel assertions or a reviewed golden-image comparison. |
| `AppE2ETests/I18nResourceBundleTests.swift::testLoadsFlatJsonFromSpmBundleBesideApp`, `testLoadsFlatJsonFromSpmBundleInContentsResources`, `testLoadsJsonFromContentsResourcesI18n`, `testSearchRootsNeverRequireBundleModule`, `testSharedTrFindsDevCatalogWithoutBundleModule` | These test SPM/macOS bundle and development lookup locations. Keep translation fallback and catalog completeness; test GPUI's actual packaging paths separately. |
| `AppE2ETests/NativeNotificationTests.swift::testNotificationServiceIsSafeAndSilentUnderXCTest` | XCTest-specific suppression is not an OS delivery assertion. Replace it with a platform-independent fake and a real notification adapter test. |
| `AppE2ETests/AgentRenderE2ETests.swift::testTrueColorRenderCachesStayBoundedAcrossFrames`, `testWideGlyphMetricsAreReusedAcrossRedraws`, `testCommittedPaintUsesFixedSizeBitmapAcrossManyFrames`, `testTerminalPointMetricsMatchSwiftTermBackingPixelGrid` | These prescribe SwiftTerm cache, bitmap or metric implementation. Retain readable output, aligned glyphs and bounded-resource requirements using GPUI-specific measurements. |
| `ChromeTests/ResizeRedrawRegressionTests.swift::testHiddenScrollerKeepsModelEqualToTmuxPaneColumns`, `testVisibleScrollerShrinksModelColumns` | These depend on AppKit's `NSScroller` and a documented SwiftTerm-specific width loss. Replace with a GPUI test that its allocated grid matches the reported client size. |
| `ChromeTests/ProgressiveWordSelectionTests.swift::testBracketsDeferToSwiftTerm` | A nil result only proves delegation to SwiftTerm, not what a user can select. Replace with an actual bracket-selection test. |
| `ChromeTests/FlatChromeTests.swift::MuxtermConfigTests::testPoolMaxSlotsDefaultAndParse` | A frontend pool slot setting conflicts with the current resident-scene and Core-owned workspace architecture. Do not add it to GPUI. |
| `ChromeTests/FlatChromeTests.swift::TabSwitchGateTests::*` | All five tests require a frontend gate to wait for Core confirmation or timeout. Current workspace/tab switching is a zero-Core-call, zero-wait local scene operation. Replace with immediate-switch and latest-selection acceptance tests. |
| `ChromeTests/FlatChromeTests.swift::EventBatchPlanTests::*` and `FlatChromeTests::testFlatMetricsMatchArchitectureBudget` | These pin private Swift event-plan or architecture metrics, not user-visible results. Preserve input/output order, no starvation and responsiveness through behavior tests. |
| `ChromeTests/SceneStackTests.swift::testCapacityDoesNotEvictUntilExplicitCleanup`, `testTTLEvictsExpiredBackgroundSlots`, `testMemoryPressureEvictsBackgroundSlots`, `testActiveSlotNotEvictedByCapacity` | These require frontend-managed background-slot eviction. Do not implement eviction to satisfy legacy tests; keep open workspace surfaces resident and test visible continuity. Other tests of `SceneStack` pool acquisition and eviction internals are not selected in Summary 2. |
| `ChromeTests/GuiLogFilePolicyTests.swift::*` | These inspect only a Swift stderr-redirection predicate, not a GPUI-visible contract. If GPUI exposes a log-file option, test that its actual output reaches the requested file. |

## Summary 4: Rust infrastructure required by tier

The table names gaps in this repository, not capabilities that GPUI can never provide. Existing Rust unit tests already cover parts of the GPUI keymap, grid, parser, theme, and pane layout. `tests/gpui_harness_smoke.rs` uses `#[gpui::test]` and `TestAppContext` to construct a generic view and invoke resize and keystroke simulation; it currently has no assertions, does not mount Muxterm, and does not exercise its claimed clipboard behavior. `tests/gpui_surface_e2e.rs` already starts isolated tmux and carries shell output through FFI, `EventPump`, and the GPUI terminal `Surface`; it does not launch or drive a GPUI window. GTK test support and Swift `Harness.swift` are useful examples, not a GPUI parity harness.

| Tier | Existing support | Missing Rust support for this acceptance suite |
|---|---|---|
| pure-logic | Rust's unit-test runner and GPUI terminal/keymap tests exist. | Shared macOS-derived byte, selection, color, shortcut and localization fixtures, plus fake clocks and deterministic expected values for the new GPUI models. No GUI is necessary for these cases. |
| model-projection | Shared Rust `ViewStore`, `CommandQueue`, `EventPump`, and some GPUI model tests exist. | A reusable FFI-shaped event/snapshot builder with duplicate pane IDs, delayed responses, capability updates, agent state, search results and deterministic queue drains. Assert public projections rather than copying Swift models. |
| ffi-integration | `FfiClient`, Rust integration tests and an isolated tmux-to-Surface example exist. | A GPUI-facing product fixture that creates isolated config/cache/install roots and an ephemeral loopback release feed with a test package, drives update actions, and observes FFI results in the mounted frontend. |
| real-runtime-e2e | `tests/gpui_surface_e2e.rs` has one real tmux-to-terminal-grid path. Existing Rust Linux tests and Swift fixtures show additional scenarios. | Reusable Rust fixtures for isolated multi-tab/multi-pane tmux, named Herdr, controllable SSH/loopback failures, history, command marks, output floods, background attention and safe cleanup; connect them to a mounted GPUI frontend and observable event pump. Never use a default tmux socket or unnamed Herdr stop. |
| xcuitest-drives-real-app | Swift XCUITest is a real macOS driver; it does not drive the Rust GPUI application. | A GPUI app launcher plus native keyboard, mouse, IME, window-manager and accessibility driver for each supported OS, with a logged-in graphical session and explicit skip reporting. It must observe panes and terminal text without relying on Swift accessibility objects. |
| pixel-or-layout | Generic GPUI test context and resize/input smoke now exist; Swift tests have AppKit bitmap and hit-test helpers. | Muxterm-specific GPUI view mounting, deterministic fonts/scale/theme and fake clock, accessible-tree and bounds/hit-test probes, clipboard and URL adapters, pixel capture/comparison, and a documented golden-image tolerance. Real clipboard, IME, fullscreen and notifications also need the OS driver. |

## Recount and validate

The following commands enumerate every test method with its source line and print each file's method and line counts. `Harness.swift` is listed with zero methods.

```sh
for dir in AppE2ETests ChromeTests MuxtermAppUITests; do
    find "src/frontend/macos/$dir" -type f -name '*.swift' | sort | while IFS= read -r file; do
        printf '\n%s: ' "$file"
        printf '%s tests, ' "$(grep -Ec '^[[:space:]]*func[[:space:]]+test[[:alnum:]_]+[[:space:]]*\(' "$file" || true)"
        wc -l < "$file"
        grep -nE '^[[:space:]]*func[[:space:]]+test[[:alnum:]_]+[[:space:]]*\(' "$file" || true
    done
done
find src/frontend/macos/AppE2ETests src/frontend/macos/ChromeTests src/frontend/macos/MuxtermAppUITests -type f -name '*.swift' -print0 | xargs -0 wc -l | tail -1
```

This check compares the actual source files and method names with the inventory and tier table. It does not validate the qualitative classifications or execute Swift or GPUI tests.

```sh
python3 - <<'PY'
from collections import Counter
from pathlib import Path
import re

root = Path('src/frontend/macos')
doc = Path('docs/gpui-parity/tests.md').read_text()
dirs = ('AppE2ETests', 'ChromeTests', 'MuxtermAppUITests')
files = {str(p.relative_to(root)): p for d in dirs for p in (root / d).glob('*.swift')}
rows = re.findall(
    r'^\| `((?:AppE2ETests|ChromeTests|MuxtermAppUITests)/[^`]+\.swift)` '
    r'\| (\d+) \| ([a-z0-9-]+) \| .*? \| (.*?) \|$', doc, re.M,
)
ports = {
    'port directly-to-rust-unit-test', 'port to-rust-integration-test',
    'needs-gpui-headless-harness', 'needs-real-gui-driver', 'cannot-port-meaningfully',
}
method_counts, file_counts, seen, names = Counter(), Counter(), Counter(), set()
for path, count, tier, portability in rows:
    assert path in files, path
    seen[path] += 1
    methods = re.findall(r'(?m)^\s*func\s+(test\w+)\s*\(', files[path].read_text())
    assert len(methods) == int(count), (path, count, len(methods))
    assert any(portability == p or portability.startswith(p + '.') for p in ports), path
    names.update(methods)
    method_counts[tier] += int(count)
    file_counts[tier] += 1
assert set(seen) == set(files)
assert all(count == 1 for count in seen.values())
summary = re.findall(r'^\| ([a-z0-9-]+) \| (\d+) \| (\d+) \|$', doc, re.M)
assert {tier: int(n) for tier, _, n in summary} == dict(method_counts)
assert {tier: int(n) for tier, n, _ in summary} == dict(file_counts)
assert not set(re.findall(r'\btest[A-Z]\w+', doc)) - names
acceptance = doc.split('## Summary 2: GPUI acceptance suite', 1)[1].split('### Coverage gaps', 1)[0]
aliases = {'A': 'AppE2ETests', 'C': 'ChromeTests', 'U': 'MuxtermAppUITests'}
selected = {f'{aliases[a]}/{name}.swift' for a, name in re.findall(r'\b([ACU])/([A-Z]\w+)(?=::)', acceptance)}
assert selected <= set(files)
assert set(files) - selected == {'AppE2ETests/Harness.swift', 'ChromeTests/GuiLogFilePolicyTests.swift'}
assert len(files) == 90 and sum(method_counts.values()) == 852
print(f'{len(files)} files, {sum(method_counts.values())} methods, {len(summary)} tiers: OK')
PY
```
