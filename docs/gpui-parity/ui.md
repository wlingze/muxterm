# macOS UI parity inventory

## Scope and evidence

This is a source inventory for a gpui port, not a proposal to remove or redesign macOS behavior. The primary scope is all seven files in `src/frontend/macos/UI/`, totaling 4,773 lines, and every file in `src/frontend/macos/Resources/`. Caller and `MuxtermChrome` references below explain placement, models, and constants that the seven UI files import. They are not a second inventory of the App, Chrome, or Terminal directories.

Bare Swift citations such as `PaneLayout.swift:15` mean `src/frontend/macos/UI/PaneLayout.swift:15`. Citations beginning with `App/`, `Chrome/`, or `Resources/` are relative to `src/frontend/macos/`. Sizes are AppKit logical points unless explicitly called backing pixels. Named `NSColor` values are semantic, appearance-dependent colors, not fixed RGB values.

Source inspection time: **2026-11-04T15:32:22+08:00**, branch `feature/union-frontend`. No screenshots or live window measurements were taken. gpui notes are implementation mappings, not claims that the current gpui frontend already has parity. The local dependency is gpui 0.2.2, declared in `Cargo.toml:62`. Its locally installed source has `div`, `canvas`, `svg`, `img`, `anchored`, `list`, and `uniform_list` elements. No external compatibility or version claim is needed for this inventory.

## Pane layout

### Component: `PaneLayoutView`

- **Where it appears:** The main window's terminal region, inside `ContentView`, adjacent to the status bar. It fills the content width. The bar can be above or below it. The command rail, banners, and other ContentView overlays do not reserve terminal space. Sources: `PaneLayout.swift:41`, `App/ContentView.swift:40-47`, `App/ContentView.swift:103-114`, `App/ContentView.swift:180-225`.
- **Layout rules:** Recursively mounts a binary `LayoutNode` tree. Each leaf is a `PaneHostView`; each split is a `SplitContainerView`. Root edges are pinned to all four container edges. The background is `textBackgroundColor`. An empty pane snapshot yields no tree. With no supplied layout, one pane becomes a single leaf; more than one pane is rejected. A supplied tree must be compatible with the current pane IDs before it replaces anything. The visible leaf set, not the complete snapshot, controls zoom layout, title eligibility, and host reuse. Sources: `PaneLayout.swift:98-104`, `PaneLayout.swift:189-293`, `PaneLayout.swift:333-408`, `PaneLayout.swift:627-720`.
- **Visual states:** Normal tree; no panes; local fullscreen projected to one leaf; core-provided zoom tree; cached tab or workspace reveal; pending seed handled by the leaf host. Local fullscreen toggles the requested existing pane against `baseLayout`, and changing the fullscreen pane preserves fullscreen. Invalid layout snapshots retain the previous display. Active focus is resolved without jumping to a stale snapshot's first pane when a split grows. Sources: `PaneLayout.swift:189-293`, `PaneLayout.swift:473-493`, `Chrome/PaneLayoutProjection.swift:4-35`, `Chrome/FlatChrome.swift:915-939`.
- **Interactions:** Receives activation, title action, swap, move-to-new-tab, divider resize, and surface-ready callbacks. Hit-testing a window point returns the enclosing visible host. Dragging a pane to a different pane requests a swap; dropping on itself cancels; dropping outside all hosts requests a new tab. This includes a divider gap, where no host contains the drop point. It does not implement a separate pane keyboard shortcut handler. Sources: `PaneLayout.swift:79-96`, `PaneLayout.swift:452-470`, `Chrome/PaneLayoutProjection.swift:113-126`.
- **Data:** `TerminalManager`, `LayoutNode`, `[Pane]`, current tab ID, local fullscreen ID, cached trees, active pane ID, readiness callbacks, and capability-fed `allowsPaneBreak`. Existing terminal views remain alive across tab and workspace switches. A parked workspace stores tab trees, original layout, fullscreen selection, and previous layout bounds. Sources: `PaneLayout.swift:42-96`, `PaneLayout.swift:107-176`, `PaneLayout.swift:295-408`.
- **gpui note:** A persistent entity per workspace and terminal, with recursive split containers or an explicit layout pass, expresses this structure. Hidden tabs must not recreate terminal models. Leaf overlays use absolute positioning. The AppKit constraint solver is not a drop-in flex layout; the split equations below are the required result.

### Exact pane sizing and resize math

1. **Titles subtract zero height.** `PaneTitleBarGeometry.height = 22`, `dividerLength = 6`, and `reservedHeight(showsTitles:) = 0` for both values of the argument. Therefore `terminalClientContentSize = (container.width, max(0, container.height))`. Each terminal is pinned to all four host edges. The title is separately pinned to the host's top, left, and right at height 22. It overlays terminal pixels and does not consume rows. The black dim overlay also fills the terminal's exact frame. Sources: `PaneLayout.swift:15-28`, `PaneLayout.swift:429-434`, `PaneLayout.swift:812-837`.
2. **Every split consumes a real 6-point strip.** Let `L` be the parent width for `horizontal == true`, otherwise the parent height. The tree stores a thousandths ratio, so let `d = 6` and `r = clamp(node.ratio / 1000, 0.05, 0.95)`. The constraint multiplier is `r / (1-r)`, applied to first-child extent relative to second-child extent. With required edge constraints and the ratio constraint at `.defaultHigh`, the normal feasible result is `first = (L-d)*r`, `second = (L-d)*(1-r)`. Both children fill the perpendicular dimension. `horizontal == true` means first on the left, divider, second on the right. `false` means first at the top, divider, second below. Nested splits apply the same equation to the child allocation. There is no title, border, or margin deduction in this calculation, and no explicit child minimum size constraint. Extremely small parents rely on AppKit's constraint resolution rather than an explicit minimum-pane algorithm. Sources: `PaneLayout.swift:668-689`, `PaneLayout.swift:1251-1330`, `Chrome/PaneResizeMath.swift:4-22`.
3. **Live divider motion is relative to drag start, not incremental rounding.** On mouse-down, record `startRatio` and the pointer coordinate. For a horizontal split the coordinate is local `x`. For a vertical split it is `parentHeight - localY`, because AppKit's local Y increases upward while the tree is ordered top-to-bottom. During a drag, `usable = max(L-d, 1)`, `startLength = clamp(startRatio)*usable`, and `nextRatio = clamp((startLength + currentPosition - startPosition)/usable, 0.05, 0.95)`. The old ratio constraint is replaced and the subtree is laid out immediately. No cell snapping occurs during the drag. Sources: `PaneLayout.swift:1338-1366`, `Chrome/PaneResizeMath.swift:13-22`.
4. **Mouse-up commits an extent, not a ratio.** The split reports the first child's actual width or height and `edgePaneID(in: first)`. That helper descends through the **second** child of every nested split until it reaches a leaf. It does not choose the first leaf. The caller gets shared cell metrics from `terminalManager.cellSizeInPixels(paneIds: currentPaneIds)`, converts a square rect `(0, 0, extent, extent)` to backing coordinates, and takes its width or height according to the split axis. It divides by the matching cell dimension, floors the result, and sends `(paneID, horizontal, UInt16(count))` only when cell size is positive and `2 <= count < 10000`. Missing cell metrics produce no request. No title height is subtracted. In a nested first subtree this is the subtree's extent addressed through its second-descendant edge leaf, not a sum of separately rounded leaf cells. Sources: `PaneLayout.swift:665-709`, `PaneLayout.swift:1368-1370`, `Chrome/PaneResizeMath.swift:25-30`.
5. **Geometry synchronization happens after layout.** The outer layout compares rounded integer point bounds; unchanged tokens do not schedule window-size work. Host bounds changes schedule tree-change synchronization even if the window did not change size. Work is coalesced on the main queue, discarded when the visible pane set no longer matches, and retried up to ten queue turns if either dimension is below 8. It publishes host geometry before `syncAllVisibleSizes`. A cached reveal does not inherently imply a terminal rebuild. `PaneTitleBarGeometry.shouldSend` returns true only for the first grid or a changed `(cols, rows)` pair. Actual terminal-grid planning beyond these calls belongs to TerminalManager, not this UI slice. Sources: `PaneLayout.swift:30-34`, `PaneLayout.swift:499-519`, `PaneLayout.swift:551-625`, `PaneLayout.swift:982-1001`.

Example: a 1,000-point-wide split at ratio 0.6 allocates 596.4 points to the first child, 6 to the divider, and 397.6 to the second before platform pixel rounding. At a 2× backing scale and a 16-backing-pixel cell width, committing that first extent requests `floor(1192.8/16) = 74` columns. It does not request 75 columns from `1000*0.6`, and hovering a title does not change that number.

Two comments conflict with executable layout. `PaneLayout.swift:1010-1011` describes a title row that reserves height, but the constraints and `reservedHeight` do not. `PaneLayout.swift:1283` describes a 1-point visible divider, but `DividerHandleView` fills its entire 6-point layer with separator color at 0.35 alpha. The port baseline here is the executable behavior, not those comments.

### Component: `PaneHostView`

- **Where it appears:** One host at each visible pane-tree leaf, surrounding the persistent `MuxTerminalView`.
- **Layout rules:** Terminal fills the host without inset. Layer border is square, active width 1, inactive width 0. Dim overlay sits above the terminal; title sits above the dim overlay. Hover eligibility covers the top 22 points, full width, using host-wide enter/move/exit tracking. The host does not become first responder. Sources: `PaneLayout.swift:742-837`, `PaneLayout.swift:874-928`, `Chrome/FlatChrome.swift:1199-1201`.
- **Visual states:** Active accent border; inactive unbordered; inactive multi-pane dim overlay; single-pane or fullscreen without dimming. Title is initially hidden, eligible only with more than one visible pane, shown on entry into the top-strip tracking area, and hidden on exit. Setting title eligibility hides it until a hover event. `setSurfaceReady(false)` hides the entire host while retaining its layout allocation; `true` reveals it and requests display. A newly built host starts from `TerminalManager.isSurfaceReady`. Sources: `PaneLayout.swift:874-940`, `Chrome/FlatChrome.swift:907-910`, `Chrome/FlatChrome.swift:1207-1213`.
- **Interactions:** Host and terminal clicks activate the pane. Opening the title action menu activates it; the selected menu action then forwards a callback. A shared native context menu is installed on both the host and terminal with Copy, Paste, Select All, followed by a separator and Move Pane to New Tab only when allowed. That item activates the pane before moving it. Title drag is allowed only when `allowsPaneBreak` is true and the current tree has more than one pane (`currentPaneIds.count > 1`). Sources: `PaneLayout.swift:85-90`, `PaneLayout.swift:797-811`, `PaneLayout.swift:838-865`, `PaneLayout.swift:931-979`, `PaneLayout.swift:1004-1006`.
- **Data:** Pane ID and title; active state; visible pane count; terminal view; readiness; move capability; title action and drag callbacks. Accessibility value and help expose rounded host width and height, active flag, and dim flag. Sources: `PaneLayout.swift:743-775`, `PaneLayout.swift:982-991`.
- **gpui note:** A relative full-size container, terminal child, non-layout border, absolute dim and title layers, and a top-strip hover target. Focus must go to the terminal rather than the wrapper. Context-menu and accessibility behavior need explicit equivalents; an overlay that captures pointer events changes terminal selection behavior.

### Component: `InactivePaneDimView`

- **Where it appears:** Above terminal pixels inside an inactive split host, below its title.
- **Layout rules:** Exact terminal bounds, black at 0.15 alpha, no independent padding or corner radius.
- **Visual states:** Visible iff `visiblePaneCount > 1 && !isActive`; otherwise hidden.
- **Interactions:** `hitTest` always returns nil. It does not intercept selection, clicks, or terminal drag gestures.
- **Data:** Host active state and visible pane count.
- **gpui note:** A pointer-transparent absolute black overlay. Do not lower the whole host's opacity because the border and title are not dimmed this way. Sources for all fields: `PaneLayout.swift:724-739`, `PaneLayout.swift:824-827`, `PaneLayout.swift:874-890`, `Chrome/FlatChrome.swift:1207-1213`.

### Component: `PaneTitleBarView`

- **Where it appears:** Hover overlay at the top of an eligible pane, including inactive panes.
- **Layout rules:** Height 22. Label uses 11-point medium system font, secondary label color, tail truncation, one line. Label is inset 7 from the left, centered vertically, and ends at least 6 before the action button. Action button is 18×18, centered vertically, 4 from the right. The border is 0.5 points in `separatorColor` at 0.65 alpha. Inactive background is `windowBackgroundColor`. Empty or whitespace-only titles become `Pane @<id>`; nonempty titles are trimmed and also become the tooltip. Sources: `PaneLayout.swift:1025-1072`, `PaneLayout.swift:1102-1111`.
- **Visual states:** Hidden or hover-visible as described above. Active label uses `controlAccentColor` over accent at 0.10 alpha; inactive uses `secondaryLabelColor` over window background. Action button uses `ellipsis.circle` and a localized pane-actions tooltip. During a title drag, the whole title has opacity 0.65, restored to 1 on release. Sources: `PaneLayout.swift:1034-1049`, `PaneLayout.swift:1097-1111`, `PaneLayout.swift:1211-1234`.
- **Interactions:** Mouse-down activates the pane. A drag starts only after Euclidean distance reaches 5 points and `dragEnabled` is true. Release passes its window point to the host's drop policy. The action button opens a native menu at its lower edge after activation. Menu order is Split Horizontal, Split Vertical, separator, Move, separator, Fullscreen, Cycle Layout, separator, Close Pane. Move destinations are rebuilt at open time; nil destination means new tab, other entries carry tab IDs. Move is hidden when there are no destinations. Actions forward `PaneTitleAction`; the view does not assign keyboard equivalents. Sources: `PaneLayout.swift:1051-1095`, `PaneLayout.swift:1113-1208`, `PaneLayout.swift:1211-1234`.
- **Data:** Pane ID, normalized title, active flag, drag-enabled flag, `moveDestinationsProvider`, and action callbacks.
- **gpui note:** An absolute flex row with clipped single-line text, a small icon button, popup menu, and pointer capture. Native `NSMenu` sizing, keyboard navigation, and SF Symbol glyphs need explicit assets or platform handling. The draggable label region is distinct from terminal selection.

### Components: `SplitContainerView` and `DividerHandleView`

- **Where they appear:** Every internal node and its strip between two child pane subtrees.
- **Layout rules:** The equations above are the complete local sizing rule. The 6-point strip fills the cross-axis and uses `separatorColor` at 0.35 alpha. The split itself has no extra padding. Sources: `PaneLayout.swift:1239-1330`, `PaneLayout.swift:1375-1394`.
- **Visual states:** Horizontal or vertical orientation; normal and dragged ratio. No separate hover, selected, or pressed color is implemented.
- **Interactions:** Cursor is left-right resize for side-by-side splits and up-down resize for stacked splits. Mouse-down explicitly captures subsequent left-drag and left-up events until release, even outside the strip. Drag updates constraints live; release sends the first-child extent. The handle is an accessibility splitter identified by the edge leaf pane ID. There is no double-click reset or keyboard resize implementation in this file. Sources: `PaneLayout.swift:1284-1287`, `PaneLayout.swift:1338-1370`, `PaneLayout.swift:1401-1439`.
- **Data:** Orientation, clamped ratio, divider length, child views, edge leaf ID, drag-start point, and resize callback.
- **gpui note:** A split layout with an explicit fixed-width or fixed-height strip and captured pointer movement. Persist ratio state, use point-to-backing conversion only at the grid boundary, and preserve the top-origin vertical calculation. Plain equal-flex children do not express arbitrary ratios or commit behavior.

## Shared tab definitions

`TabBar.swift` is not a view class. It holds three enums that decorate the status bar, sidebar, quick panel, and tabs. A gpui port needs them as theme or config types, not as a missing widget.

### Component: `AggregateWorkspaceAppearance`

- **Where it appears:** Shells/Agents aggregate workspaces. The enum is consumed by the status bar, sidebar, quick panel, and tab buttons; it never changes terminal ANSI colors.
- **Layout rules:** No geometry. It selects an accent and a one-character symbol: Shells uses `systemTeal` and `S`, Agents uses `systemPurple` and `A`.
- **Visual states:** `shells`, `agents`, or nil for an ordinary workspace. It initializes from aggregate workspace IDs or a `StatusBarWorkspacePresentation`, and returns nil for `.workspace` and `.opening`.
- **Interactions:** None directly. Consumers decide which chips, tints, and borders to apply.
- **Data:** `AggregateWorkspaceIdentity.shells/agents`.
- **gpui note:** A small enum with `accent: Hsla` and `symbol: &'static str`. Keep it separate from terminal palette state. Sources: `TabBar.swift:4-34`.

### Components: `TabBarPosition`, `TabBarStyle`

- **Where it appears:** `TabBarPosition` controls whether the status bar is above or below panes. `TabBarStyle` controls tab widths inside that bar.
- **Layout rules:** Position is `top` or `bottom`, default bottom. Style is `equal_width`, `fixed`, or `compact`. Fixed width is 128 points. Compact clamps to 44..280 points. Equal width lets the tab row fill its viewport with a low-priority 44-point minimum.
- **Visual states:** No separate visual state. Values come from config, not UserDefaults.
- **Interactions:** Changing position swaps constraints; changing style rebuilds tab buttons.
- **Data:** `config.toml` UI settings.
- **gpui note:** Model these as `#[derive(Clone, Copy, PartialEq, Eq)]` enums and branch layout explicitly. Do not encode `equal_width` as a numeric width. Sources: `TabBar.swift:36-56`, `App/ContentView.swift:229-243`.

## Status bar

### Component: `StatusBarView`

- **Where it appears:** One bar across the full window width, either below the terminal (default) or above it. `tab bar position` swaps the pane and bar constraints and moves the separator edge. With a top bar, the integrated-titlebar call does not change the 4-point leading inset. Sources: `App/ContentView.swift:40-47`, `App/ContentView.swift:180-225`, `App/ContentView.swift:229-243`.
- **Layout rules:** Height is `FlatChrome.tabBarHeight`, 24 points, even when top-positioned. Horizontal order is sidebar toggle, middle stack, paste spinner, status dot, workspace, window fullscreen, settings, attention, new tab. Leading inset 4; toggle 24×20. Middle stack starts 6 after toggle, ends 4 before spinner, is 20 tall. Trailing order is mirrored with fixed 2-point gaps; settings/fullscreen/workspace/attention are 24×20 when shown, status dot 18×18, new tab 28 wide, spinner 14. Trailing inset 4. Middle stack is framed manually: without tmux segments, tab viewport equals the whole middle bounds. With tmux segments and theme mode `tmux`, left and right text widths are capped at 36% of **bar** width each, right first takes `clamp(max(intrinsic, 64), <=36%)` then left takes `min(intrinsic, 36%, remaining-12)`, left sits flush left, right flush right, and the tab viewport lies between them with 6-point gaps. `StatusBarLayoutPolicy.windowMinFraction` (28%) and `StatusBarTabOverflow` have no call site in the macOS sources, so this path does not enforce a 28% tab floor or a scroll-overflow mode; the viewport simply clips. Sources: `StatusBarView.swift:237-449`, `StatusBarView.swift:452-470`, `StatusBarView.swift:485-521`, `Chrome/FlatChrome.swift:13-28`, `Chrome/StatusBarModel.swift:84-96`.
- **Visual states:** Bar background is `windowBackgroundColor`. A 1-point separator edge sits top when the bar is top-positioned, bottom otherwise. Shells/Agents presentation makes it 2 points in teal/purple at 0.72 alpha and tints the bar at 0.09/0.10 alpha; Opening is 1 point separator with a yellow ellipsis workspace button. In normal workspace mode the workspace button uses `square.grid.2x2`. The status dot is a blue `arrow.up.circle.fill` during image paste; red exclamation on any error; green for local connected; SSH connected color thresholds use green below 256 KiB/s, yellow below 1 MiB/s, red at or above; yellow ellipsis while connecting; red xmark for disconnected or exited; tertiary questionmark otherwise. A 14-point paste spinner appears for text or image paste. Buttons hide and their width constraints go to zero according to trailing-button and capability flags. Sources: `StatusBarView.swift:452-470`, `StatusBarView.swift:557-635`, `StatusBarView.swift:747-815`, `Chrome/StatusBarModel.swift:154-159`, `TabBar.swift:6-33`.
- **Interactions:** Clicking empty bar area drags the window. Sidebar toggle, tab selection, new tab, workspace, connection dot, attention, window fullscreen, and settings each have one callback. Tabs forward click to `onSelectWindow` when tmux status is enabled, else `onSelectTab`. Double-click requests rename when allowed. Close xmark requests close. A tab drag starts at 4-point Euclidean distance, dims to 0.65, and on release uses the nearest tab's midpoint to request move; displacement left requests before, right requests after. Each tab context menu may contain Rename, Move Left, Move Right, separator, Close. The status dot opens a transient 300-point-wide connection popover; in SSH it also opens a 500×292 port picker with 230-point scrolling list. Fullscreen tooltip advertises Shift-Cmd-F; settings advertises Cmd-,. Command-H opens connection details through the status-dot path. Sources: `StatusBarView.swift:523-539`, `StatusBarView.swift:938-1086`, `StatusBarView.swift:1224-1475`, `StatusBarView.swift:1541-1690`, `StatusBarView.swift:1800-1810`.
- **Data:** `[Tab]`, optional tmux `StatusBarSnapshot`, tab activity map, connection summary, receive/send rates and totals, SSH port listing, attention summary, workspace presentation, debug flag, errors, paste flags, capability flags, tab style, and position. Tab topology and order always come from Core tabs, never from the tmux window list. Sources: `StatusBarView.swift:128-235`, `StatusBarView.swift:647-680`, `StatusBarView.swift:1497-1541`.
- **gpui note:** A fixed-height flex row with a manually sized middle region, absolute separator edge, dynamic-width trailing controls, and native popovers. Use `uniform_list` only for genuinely scrolling port rows; tab count is small, and equal-width tabs are flex children with a low-priority 44-point minimum. `anchored` or a popup layer covers popovers. SF Symbols, native button metrics, transient popover behavior, and OS window dragging need explicit equivalents.

### Component: `StatusTabButton`

- **Where it appears:** Inside the clipped tab viewport in the middle of `StatusBarView`.
- **Layout rules:** Corner radius 4, intrinsic height 20. Intrinsic width is `max(44, native title width + 20)`. Equal-width style uses fill-equally, 2-point spacing, trailing pinned to viewport, and only a low-priority 44-point minimum. Fixed style uses 128; compact uses `44..280`; both keep natural width left-aligned with 3-point spacing and no trailing pin. Title is 11-point system, semibold active or regular inactive, left-aligned. Text indents 25 when activity is visible, otherwise 8; tail indent -24 when close is visible, otherwise -8. Tail truncation. Activity indicator is 12×12, 7 from left. Close is a 16×16 borderless `xmark`, 4 from right. Active underline is 2 tall, x 7, width `max(0, width-14)`. Overflow is horizontally clipped, not scrolled, in this UI. Sources: `StatusBarView.swift:1497-1630`, `StatusBarView.swift:2143-2208`, `TabBar.swift:48-56`, `Chrome/StatusBarModel.swift:98-121`.
- **Visual states:** Active underline accent and title label color; inactive secondary label. Without aggregate identity, active background is `selectedControlColor` at 0.14 and inactive clear. Shells tint active 0.20/inactive 0.065 teal and text at full/0.78; Agents does the same in purple. With tmux styling, foreground/background come from tmux, reverse swaps them, underline is hidden and close tint follows foreground. Close tint is tertiary normally or aggregate accent at 0.82. Activity states are hidden for idle/nil; working is a 2.4-point stroked arc of about 315 degrees that rotates once per 0.9 seconds unless Reduce Motion is on; blocked is a filled pink dot; done is a filled teal dot. Working color is `(1.0, 0.72, 0.18)` dark or `(0.70, 0.22, 0.035)` light. Sources: `StatusBarView.swift:2246-2312`, `StatusBarView.swift:2314-2411`, `StatusBarView.swift:2413-2469`.
- **Interactions:** Single click selects. Double click renames and returns before drag handling. Dragging starts at 4-point distance, restores opacity on release, and selects on a non-drag release. Close xmark requests close without selecting first. Context menu actions forward without selecting first. Sources: `StatusBarView.swift:2222-2262`, `StatusBarView.swift:1541-1626`.
- **Data:** Tab ID in `tag`, displayed ordinal + name, optional attributed tmux title and style, active flag, activity indicator, close capability, rename capability, reordering capability, aggregate appearance.
- **gpui note:** Flex child with clipping and explicit text indents, an underlay or 2-point bottom border, optional 12-point animated spinner, SVG close icon, right-click menu, and drag threshold. Equal-width tab fill is not the same as a fixed-width tab row. Reduce Motion must stop rotation without hiding the arc.

### Component: `StatusDotButton`, `AttentionBellButton`

- **Where it appears:** Right-hand chrome group of the status bar, before workspace/fullscreen/settings/new-tab controls.
- **Layout rules:** Status dot 18×18, image-only, symbol point size 11 medium, proportionally scaled down. Bell is 24 wide, 20 tall, 11-point symbol. Badge is at least 11×11 with radius 5.5, 8-point bold monospaced digits, white, top-right. Status and bell override hit-testing so only the button itself, not a transparent excess area, takes clicks.
- **Visual states:** Dot symbol/color combinations are listed in the bar states above. Bell is outlined when count is zero and filled when positive; badge background and bell tint follow dominant attention: teal done, pink blocked, yellow working, tertiary idle, secondary nil. Count text is empty for 0, `1` for exactly one, and `99+` above 99; badge hides according to `AttentionBadgePolicy.showsCount`. **The bell's own tint color is not red**, despite the source comment. Sources: `StatusBarView.swift:1958-2001`, `StatusBarView.swift:2075-2141`, `Chrome/StatusBarModel.swift:123-150`.
- **Interactions:** Clicking dot opens connection details and requests refresh. Clicking bell forwards attention. Both expose tooltips and accessibility labels; there is no hover animation or separate context menu.
- **Data:** Connection summary, rates, totals, error, debug text, SSH ports and host; attention count and dominant `AgentSidebarIndicator`.
- **gpui note:** Small icon buttons with SVG/SF-equivalent assets and a badge overlay. Badge label must remain outside the icon's layout box so count changes do not shift the right-hand button order.

## Command history rail

### Component: `CommandMarkRailView`

- **Where it appears:** A single overlay at the terminal region's right edge, inset 4 vertically. It describes the selected pane's history, not one rail per split host. Expanded width changes nearby jump/latest affordance placement, not pane allocation. Sources: `App/ContentView.swift:96-101`, `App/ContentView.swift:116-153`.
- **Layout rules:** Collapsed width 8; expanded width 14. The view is **not flipped**: offset zero is near the bottom. Set `span = max(maxOffset, max(mark.offset))`, `usable = max(height - 20, 1)`, and `tickY = 10 + (span == 0 ? 0 : mark.offset/span)*usable`. Marks stay in input order; there is no collision avoidance. Track width is 2 collapsed or 3 expanded, centered horizontally, inset 10 vertically, radius 1. Each tick is 6 or 10 wide, centered horizontally, 3 high, radius 1. Drawing stops if hidden, tick list empty, or height <= 8. There is **no viewport thumb**. Sources: `CommandMarkRailView.swift:20`, `CommandMarkRailView.swift:124-157`, `Chrome/CommandMarkRail.swift:47-76`.
- **Visual states:** Hidden iff mark count is zero, not when maxOffset is zero. `exitCode == 0` is green, nonzero is red, nil is pending in tertiary label color. Tick alpha is 0.92 collapsed and 1 expanded; track separator alpha is 0.28 and 0.55. Hover expands the entire rail; it does not change one tick's size or color. Sources: `CommandMarkRailView.swift:59-75`, `CommandMarkRailView.swift:95-104`, `CommandMarkRailView.swift:124-157`.
- **Interactions:** Enter/move tracking works in the key window. Clicking a mark within 6 points calls `onSelectMark`. Ties choose closest, then failure over nonfailure, then larger sequence number. Otherwise click Y maps to `ratio = clamp(y-10, 0, usable)/usable`; ratio <= 0.08 returns offset 0, otherwise round `maxOffset*ratio` and clamp to its range. The no-mark tooltip is hardcoded Chinese `命令时间线`; a hit shows Chinese outcome, command, and exit code or `…`. Exit collapses and clears the tooltip. No drag scrollbar, wheel override, or keyboard handler is defined. Sources: `CommandMarkRailView.swift:83-121`, `CommandMarkRailView.swift:168-184`, `Chrome/CommandMarkRail.swift:78-110`.
- **Data:** `[CommandMarkTick]`, `maxOffset`, derived placed ticks, expansion state, and callbacks. Accessibility exposes a scroll-bar role and two 1-point proxy views at top and bottom for latest success/failure. Proxies hide independently when no mark of their kind exists; their tooltips and values contain the last matching command. Sources: `CommandMarkRailView.swift:10-51`, `CommandMarkRailView.swift:59-75`.
- **gpui note:** Absolute `canvas` drawing, pointer hit tests, and a hover target. Convert the bottom-origin tick Y to gpui's chosen coordinate system explicitly. Marks are positions, not rows, so `uniform_list` is not appropriate. Preserve the accessibility proxy semantics without adding visible ticks or a scrollbar thumb.

## Banners and connection summary

### Component: `ImagePasteBannerView`

- **Where it appears:** Main terminal's bottom-right overlay, 12 from the terminal's bottom and right, width bounded to 220..420, preferred 420 at priority 490, left edge no closer than 12. It is also reused in the composer panel. It reserves no terminal rows. Sources: `App/ContentView.swift:131-136`, `App/ContentView.swift:165-170`, `App/ComposerPanel.swift:235`, `App/ComposerPanel.swift:468-470`.
- **Layout rules:** Height 28, corner radius 6, no border configured. Spinner and icon share a 14×14 slot, 10 from the left, vertically centered. Single message label starts 8 after the slot, uses 12-point medium system font and middle truncation with low horizontal compression resistance. There is **no detail row**. Copy follows the label by at least 8, is at least 38 wide, and ends 4 before Dismiss. Dismiss ends 8 from the right. Both are small inline borderless buttons; Dismiss displays literal `✕`, not an SF Symbol. Sources: `ImagePasteBannerView.swift:25-96`.
- **Visual states:** `.idle` hides and stops spinning. `.encoding` shows localized reading/encoding text; `.uploading` shows upload progress text. Both show the spinner, hide the icon, and hide Copy and Dismiss. `.done(path)` shows localized success with the basename, a green `checkmark.circle.fill`, Copy, and Dismiss. `.failed(message)` shows localized failure prefix plus message, red `exclamationmark.triangle.fill`, Copy, and Dismiss. Background is accent at 0.14 alpha for progress, green at 0.16 for success, red at 0.16 for failure. Done tooltip is the full path; failed tooltip is the raw message; busy stages have none. Accessibility value is the stage string. Same-stage application does not rebuild widgets. Sources: `ImagePasteBannerView.swift:105-214`.
- **Interactions:** Copy uses `onCopy` or the system pasteboard. Done copies the full path; failed copies localized failure prefix plus message. It does **not** change the button to a Copied state. Dismiss forwards `onDismiss`; it is hidden during progress. There are no keyboard shortcuts or context-menu handlers here. Sources: `ImagePasteBannerView.swift:160-183`, `ImagePasteBannerView.swift:227-240`.
- **Data:** `ImagePasteStage` through `ImagePasteFeedback`. The external timeline holds progress for at least 0.4 seconds before presenting a fast result, success for 2.5 seconds, and failure for 10 seconds. Progress does not expire automatically. The composer clears the timeline after inserting a path instead of showing a redundant success result. Sources: `Chrome/ImagePasteFeedback.swift:8-86`, `Chrome/ImagePasteFeedback.swift:91-156`, `App/ComposerPanel.swift:461-470`.
- **gpui note:** Absolute flex row, spinner animation, SVG result icons, middle-truncated text, clipboard action, and externally timed presentation. Constraint priority 490 means the preferred width yields to the user's window size; the separate hard minimum is still 220. Native small-button metrics require an explicit cross-platform baseline.

### Component: `UpdateBannerView` and `UpdateBannerPresentation`

- **Where it appears:** Main ContentView overlay at top +6, left/right +10/-10. It can overlap the terminal or top-positioned bar because it is not a reserved row. Sources: `App/ContentView.swift:103-114`, `App/ContentView.swift:162-164`.
- **Layout rules:** Height 28, radius 6, accent background alpha 0.12, no border configured. Message is 12-point medium system font, label color, tail truncation, low compression resistance, left inset 10. Rounded small action uses 12-point semibold; its left edge is at least 8 past the message. Small inline borderless literal `✕` follows by 6 and ends 8 from the right. All are vertically centered. Sources: `UpdateBannerView.swift:20-69`.
- **Visual states:** Core phase `available` shows version plus Install Now; `failed` shows failure plus Retry. Both show enabled action and Dismiss. `checking`, `installing`, and `installed` show their message but **hide both buttons**. Installed asks the user to restart; there is no restart button in this view. All other phases hide the banner. A dismissed nonempty version stays hidden across later states of that same version. A status with empty version clears this suppression; an identical status short-circuits before rerender. Sources: `UpdateBannerView.swift:78-100`, `UpdateBannerView.swift:139-190`.
- **Interactions:** Primary action calls `onPrimaryAction`. Dismiss remembers the most recently rendered version, hides the banner, then calls `onDismiss`. Refreshing localization updates message and action text. No view-owned keyboard shortcut, hover effect, or context menu is defined. Sources: `UpdateBannerView.swift:103-129`.
- **Data:** `CoreBridge.UpdateStatus` and `UpdateBannerPresentation.make`, plus `dismissedVersion` and `lastRendered`. Version comparison, download, and installation belong to Core, not the view.
- **gpui note:** An absolute flex row with clipped label, native-like buttons, and an explicit phase/version suppression state. Hiding a disabled primary action is part of parity; rendering a disabled button instead changes the layout.

### Component: `ConnectionStatusView`

- **Where it appears:** A standalone optional debug/error row implementation. Production macOS Swift sources contain no construction call, so it is not evidence of an extra mounted row below `StatusBarView`.
- **Layout rules:** Height 0 hidden, otherwise 18. Same `textBackgroundColor` as the terminal, top separator at `y = height - 1` with height 1. Main label is 10-point regular monospaced system font, inset 6 horizontally and vertically centered. No explicit truncation mode is assigned. A 2×2 output probe at top-left has alpha 0.01 and no border or background, keeping it in the accessibility tree. Sources: `ConnectionStatusView.swift:8-68`, `ConnectionStatusView.swift:76-79`, `Chrome/FlatChrome.swift:17-18`, `Chrome/FlatChrome.swift:27-28`.
- **Visual states:** Starts hidden. With `isDebug`, `update(snapshot:)` builds localized connection state, tab count, pane count, and active `@paneID`; otherwise its base text is empty. Nonempty error overrides base text in red. Normal debug text is tertiary label color. Layout-sync is a specific error message; clearing it does not clear a different error. Empty text hides the row and sets height zero. Connected, connecting, disconnected, error, exited, and unknown states are localized. Sources: `ConnectionStatusView.swift:81-114`, `ConnectionStatusView.swift:131-150`.
- **Interactions:** No buttons, pointer tracking, drag, keyboard, or menus. External calls set error, layout-sync error, output probe, or snapshot. There is no self-expiring transient-hint timer. Localization refresh preserves whether a layout-sync message was showing. Sources: `ConnectionStatusView.swift:99-129`.
- **Data:** `FrameSnapshot`, `isDebug`, base/error strings, localized layout-sync message, and AX-only output snippet.
- **gpui note:** A conditionally sized row and a separate accessible diagnostic value. Porting the implementation does not imply mounting it without confirming product use. The almost-transparent probe is a testing/accessibility detail, not user-visible terminal output.

## Resources

### `MuxtermLogo.svg`

- **Form:** 1000×1000 viewBox, no fixed pixel width or height. Artwork is translated by 90 and scaled by 0.82, leaving about 9% transparent margin around the icon, as the source comment states. The background rounded square has radius 238.
- **Colors:** Background gradient runs from `#091625` at top-left to `#1a3044` at bottom-right. The M strokes and bars are flat `#f4f0e6`. The X is a mint gradient from `#70e6b5` at top-left to `#35c99e` at bottom-right. There is no text, shadow, or stroke.
- **Geometry:** M consists of two vertical 136×580 bars at x=200 and x=664 (the right path is drawn from x=800 back to 664), plus two angled tops. X consists of two overlapping mint diagonals. A pixel-exact port should reuse this SVG as an asset rather than redraw paths from the table.
- **gpui note:** Load as `svg()` with `.w_8().h_8()` or an icon size token. The design assumes a square; do not preserve the 1000-unit viewBox as a layout size. Sources: `Resources/MuxtermLogo.svg:1-31`.

### Bitmap and package resources

| Resource | Fact | Consumer |
|---|---|---|
| `Resources/MuxtermLogo.png` | PNG, 1024×1024, 8-bit RGBA, 57,790 bytes | Source artwork for icon packaging and documentation |
| `Resources/MuxtermLogo.icns` | macOS icon, 288,216 bytes, includes `ic12` | `Info.plist` `CFBundleIconFile` and `project.yml` bundle resource |
| `Resources/i18n/en.json` | 522 lines, 28,048 bytes, English catalog | `MuxtermI18n` loader |
| `Resources/i18n/zh-CN.json` | 522 lines, 27,759 bytes, Simplified Chinese catalog | `MuxtermI18n` loader |

The ICNS is the user-visible Dock and Finder icon on macOS. A cross-platform gpui build needs the PNG or SVG for its own window, About, or taskbar icon. Sources: `Info.plist:10`, `project.yml:87`, `project.yml:116`.

### Localization catalog

- **Lookup:** `MuxtermI18n.shared.tr(key, arguments:)` resolves the current language, falls back to English, then to the key id. It replaces `{{name}}` placeholders with the supplied strings. `trDynamic` exists for catalog keys that enter from Core data. Sources: `App/I18n.swift:655-705`.
- **Language selection:** `system`, English, or Simplified Chinese. `system` resolves to Chinese when the first preferred language starts with `zh`, otherwise English. A language preference is stored under `muxterm.language` and a change posts `muxtermLanguageChanged`. Sources: `App/I18n.swift:630-676`.
- **UI-slice keys used by these components:** `image_paste_busy`, `image_paste_invalid`, `image_paste_failed`, `image_paste_progress`, `image_paste_encoding`, `image_paste_done`; `update_available`, `update_available_message`, `update_install_now`, `update_installing`, `update_installed_restart`, `update_checking`, `update_failed`, `update_retry`; `status_connected`, `status_connecting`, `status_disconnected`, `status_error`, `status_exited`, `status_unknown`; `status_attention`, `status_attention_count`, `status_attention_none`; `status_transport`, `status_host`, `status_state`, `status_receive_rate`, `status_received`, `status_send_rate`, `status_sent`, `status_error_details`; `status_ports`, `status_forward`, `status_stop_forward`, `status_forwarding`, `status_ignore`, `status_no_ports`, `status_scanning_ports`, `status_lan_access`, `status_copy_address`, `status_open_address`, `status_open_remote_port`, `status_remote_address_pending`; `status_refresh`, `status_connection_details`, `status_show_connection_details`; `menu_copy`, `menu_paste`, `menu_select_all`, `menu_split_horizontal`, `menu_split_vertical`, `menu_close_pane`, `toggle_pane_fullscreen`, `cycle_layout`, `move_pane`, `move_pane_to_new_tab`, `pane_actions`; `close_tab`, `rename_tab`, `move_tab_left`, `move_tab_right`, `new_tab_tooltip`, `tabs`, `panes`, `pane`, `pane_accessibility`; `layout_syncing`, `terminal_output_snippet`. Sources: `Resources/i18n/en.json:2-6`, `Resources/i18n/en.json:140-188`, `Resources/i18n/en.json:199-223`, `Resources/i18n/en.json:493-502`.
- **Placeholders:** Image success uses `{{name}}` for the basename. Update available uses `{{version}}`; update failure uses `{{message}}`; attention count uses `{{count}}`; pane accessibility uses `{{id}}`; tab switch uses `{{number}}`.
- **gpui note:** Keep the JSON catalog format as the single source and expose a small Rust lookup type with the same precedence: current locale, English, key. Do not hardcode English strings in gpui views. The rail's tooltips and accessibility labels are currently hardcoded Chinese in `CommandMarkRailView` and are exceptions, not the convention.

## Design tokens

The table is one candidate `theme` module. Values are points, ratios, or colors. Semantic AppKit colors are marked as such; gpui should map them to light/dark theme slots, not to fixed RGB values copied from one macOS appearance. Source numbers that conflict with executable behavior are called out in the notes column.

| Rust token | Value | Source | Use and notes |
|---|---|---|---|
| `tab_bar_height` | 24 | `Chrome/FlatChrome.swift:16` | `StatusBarView` fixed height. Also used for the top-positioned bar. |
| `status_row_height` | 18 | `Chrome/FlatChrome.swift:18` | Optional `ConnectionStatusView` height, not the 24-point tab bar. |
| `pane_title_height` | 22 | `PaneLayout.swift:16` | Hover overlay height, not reserved terminal rows. |
| `pane_divider_width` | 6 | `PaneLayout.swift:17` | Actual split strip and hit target. |
| `split_divider_thickness` | 1 | `Chrome/FlatChrome.swift:20` | Legacy/other constant; the visible `DividerHandleView` fills all 6 points. |
| `pane_active_border_width` | 1 | `Chrome/FlatChrome.swift:22` | Active host border; zero inactive. |
| `pane_dim_alpha` | 0.15 | `Chrome/FlatChrome.swift:1201-1213` | Black overlay only for inactive pane when visible pane count > 1. |
| `pane_title_bg_active` | `controlAccentColor` at 0.10 | `PaneLayout.swift:1080-1084` | Active hover title background. |
| `pane_title_bg_inactive` | `windowBackgroundColor` | `PaneLayout.swift:1080-1084` | Inactive hover title background. |
| `pane_title_border_alpha` | 0.65 | `PaneLayout.swift:1029-1033` | Separator color at 0.5-point border. |
| `pane_title_font_size` | 11 medium | `PaneLayout.swift:1039` | System font, tail truncation. |
| `pane_title_action_size` | 18 | `PaneLayout.swift:1063-1068` | `ellipsis.circle` button. |
| `pane_title_drag_threshold` | 5 | `PaneLayout.swift:1224` | Euclidean points before drag. |
| `divider_alpha` | 0.35 | `PaneLayout.swift:1375-1394` | Whole 6-point strip, despite the 1-point comment. |
| `pane_ratio_min` | 0.05 | `Chrome/PaneResizeMath.swift:5` | Clamp lower bound. |
| `pane_ratio_max` | 0.95 | `Chrome/PaneResizeMath.swift:6` | Clamp upper bound. |
| `command_rail_width_collapsed` | 8 | `Chrome/CommandMarkRail.swift:48` | Rail reserves no pane width; it overlays. |
| `command_rail_width_expanded` | 14 | `Chrome/CommandMarkRail.swift:49` | Width while hovered. |
| `command_rail_inset` | 10 | `Chrome/CommandMarkRail.swift:51` | Vertical inset for tick placement and track. |
| `command_rail_tick_height` | 3 | `Chrome/CommandMarkRail.swift:50` | Tick rectangle height. |
| `command_rail_track_width` | 2 collapsed, 3 expanded | `CommandMarkRailView.swift:130` | Centered track. |
| `command_rail_tick_width` | 6 collapsed, 10 expanded | `CommandMarkRailView.swift:146` | Centered tick. |
| `command_rail_tick_alpha` | 0.92, 1 expanded | `CommandMarkRailView.swift:144` | Mark color alpha. |
| `command_rail_track_alpha` | 0.28, 0.55 expanded | `CommandMarkRailView.swift:136` | Separator track alpha. |
| `command_rail_radius` | 1 | `CommandMarkRailView.swift:137` | Track and tick corner radius. |
| `command_rail_hit_slop` | 6 | `Chrome/CommandMarkRail.swift:82` | Points around a tick that select it. |
| `command_rail_jump_threshold` | 0.08 | `Chrome/CommandMarkRail.swift:100-108` | Bottom ratio that maps to scroll offset zero. |
| `banner_height` | 28 | `ImagePasteBannerView.swift:95`, `UpdateBannerView.swift:68` | Both overlays. |
| `banner_radius` | 6 | `ImagePasteBannerView.swift:28`, `UpdateBannerView.swift:19` | Both overlays. |
| `banner_message_font_size` | 12 medium | `ImagePasteBannerView.swift:42`, `UpdateBannerView.swift:28` | Image banner uses middle truncation; update uses tail truncation. |
| `image_banner_preferred_width` | 420 | `App/ContentView.swift:145` | Constraint priority 490. |
| `image_banner_min_width` | 220 | `App/ContentView.swift:167` | Hard minimum. |
| `image_banner_icon_size` | 14 | `ImagePasteBannerView.swift:78-81` | Spinner and icon share the slot. |
| `image_banner_copy_min_width` | 38 | `ImagePasteBannerView.swift:91` | Borderless small Copy. |
| `image_banner_progress_bg` | `controlAccentColor` at 0.14 | `ImagePasteBannerView.swift:196` | Tone progress. |
| `image_banner_success_bg` | `systemGreen` at 0.16 | `ImagePasteBannerView.swift:197` | Tone success. |
| `image_banner_failure_bg` | `systemRed` at 0.16 | `ImagePasteBannerView.swift:198` | Tone failure. |
| `image_paste_progress_min_visible` | 0.4 s | `Chrome/ImagePasteFeedback.swift:55` | Minimum progress display. |
| `image_paste_success_hold` | 2.5 s | `Chrome/ImagePasteFeedback.swift:50` | Result hold. |
| `image_paste_failure_hold` | 10 s | `Chrome/ImagePasteFeedback.swift:52` | Result hold. |
| `update_banner_bg` | `controlAccentColor` at 0.12 | `UpdateBannerView.swift:19` | Full-width overlay. |
| `update_banner_margin_x` | 10 | `App/ContentView.swift:162` | Left and right. |
| `update_banner_margin_top` | 6 | `App/ContentView.swift:163` | Top only. |
| `status_bar_separator` | 1 normal, 2 aggregate | `StatusBarView.swift:561` | Edge line thickness. |
| `status_bar_aggregate_edge_alpha` | 0.72 | `StatusBarView.swift:562-563` | Teal/purple edge. |
| `status_bar_shells_tint` | teal at 0.09 | `StatusBarView.swift:589-592` | Whole-bar tint. |
| `status_bar_agents_tint` | purple at 0.10 | `StatusBarView.swift:599-602` | Whole-bar tint. |
| `status_bar_opening_chip` | yellow at 0.12 | `StatusBarView.swift:605-609` | Workspace chip. |
| `status_bar_control_gap` | 2 | `StatusBarView.swift:403-423` | Between trailing controls. |
| `status_bar_leading_inset` | 4 | `StatusBarView.swift:380-382` | Sidebar toggle leading. |
| `status_bar_trailing_inset` | 4 | `StatusBarView.swift:399` | New-tab trailing. |
| `status_bar_middle_gap` | 6 | `StatusBarView.swift:387` | Toggle to middle stack. |
| `status_bar_middle_height` | 20 | `StatusBarView.swift:389` | Middle content height. |
| `sidebar_toggle_size` | 24×20 | `StatusBarView.swift:383-384` | Icon button. |
| `status_dot_size` | 18×18 | `StatusBarView.swift:2055-2056` | Status dot. |
| `status_control_size` | 24×20 | `StatusBarView.swift:450-452` | Workspace, fullscreen, settings, attention. |
| `new_tab_width` | 28 | `Chrome/FlatChrome.swift:26` | Plus button, hidden width 0. |
| `paste_spinner_size` | 14×14 | `StatusBarView.swift:434` | Width animates 0 or 14. |
| `workspace_chip_radius` | 5 | `StatusBarView.swift:313` | Workspace button layer radius. |
| `workspace_chip_active_alpha` | 0.24 Shells, 0.25 Agents | `StatusBarView.swift:583`, `StatusBarView.swift:597` | Accent chip background. |
| `status_dot_symbol_size` | 11 medium | `StatusBarView.swift:2061-2062` | SF Symbol point size. |
| `attention_badge_height` | 11 | `StatusBarView.swift:2094` | At least 11×11. |
| `attention_badge_radius` | 5.5 | `StatusBarView.swift:2087` | Height / 2. |
| `attention_badge_font` | 8 bold monospaced digits | `StatusBarView.swift:2084` | White count. |
| `popover_width` | 300 | `StatusBarView.swift:1057` | Connection details. |
| `popover_insets` | 14 horizontal, 12 vertical | `StatusBarView.swift:1052-1055` | Root stack inset. |
| `popover_spacing` | 9 | `StatusBarView.swift:967` | Root stack. |
| `popover_grid_row_spacing` | 5 | `StatusBarView.swift:1009` | Detail grid. |
| `popover_grid_column_spacing` | 12 | `StatusBarView.swift:1009` | Detail grid. |
| `popover_title_font` | 13 semibold | `StatusBarView.swift:973` | Connection details title. |
| `popover_key_font` | 11 regular | `StatusBarView.swift:983` | Right-aligned label. |
| `popover_value_font` | 11 regular, 11 monospaced digits for rates/totals | `StatusBarView.swift:985-992` | Selectable values. |
| `port_picker_size` | 500×292 | `StatusBarView.swift:1127`, `StatusBarView.swift:1133` | Port picker. |
| `port_list_height` | 230 | `StatusBarView.swift:1129` | Scroll view. |
| `port_row_spacing` | 6 | `StatusBarView.swift:1194` | Between row controls. |
| `port_remote_width` | 44 | `StatusBarView.swift:1205` | `:port` column. |
| `port_value_min_width` | 80 | `StatusBarView.swift:1212` | Address/value slot. |
| `port_value_height` | 24 | `StatusBarView.swift:1211` | Row control height. |
| `port_forward_controls_width` | 56 | `StatusBarView.swift:1239` | Copy and LAN group. |
| `port_copy_width` | 28 | `StatusBarView.swift:1259` | Copy icon button. |
| `port_lan_width` | 24 | `StatusBarView.swift:1267` | L toggle. |
| `port_action_width` | max(76, fitting) | `StatusBarView.swift:1177-1179` | Forward/Stop. |
| `port_open_width` | max(54, fitting) | `StatusBarView.swift:1184` | Open button. |
| `port_ignore_width` | max(50, fitting) | `StatusBarView.swift:1187` | Ignore slot. |
| `port_value_font` | 10 monospaced digits | `StatusBarView.swift:1274` | Address/error. |
| `tab_height` | 20 intrinsic | `StatusBarView.swift:2192` | Tab button. |
| `tab_radius` | 4 | `StatusBarView.swift:2148` | Tab button. |
| `tab_min_width` | 44 | `StatusBarView.swift:2192` | Low priority equal-width floor; hard floor compact. |
| `tab_fixed_width` | 128 | `TabBar.swift:54` | Fixed style. |
| `tab_compact_max_width` | 280 | `TabBar.swift:55` | Compact style. |
| `tab_spacing_equal` | 2 | `StatusBarView.swift:1512` | Equal-width style. |
| `tab_spacing_fixed_compact` | 3 | `StatusBarView.swift:1512` | Fixed/compact style. |
| `tab_title_font` | 11 system, semibold active / regular inactive | `StatusBarView.swift:2277` | Tail truncation. |
| `tab_text_indent_activity` | 25 | `StatusBarView.swift:2286` | Custom first-line and head indent. |
| `tab_text_indent_plain` | 8 | `StatusBarView.swift:2286` | Custom indent. |
| `tab_text_tail_close` | -24 | `StatusBarView.swift:2288` | Reserved close space. |
| `tab_text_tail_plain` | -8 | `StatusBarView.swift:2288` | No close. |
| `tab_activity_size` | 12×12 | `StatusBarView.swift:2176` | Activity mark. |
| `tab_close_size` | 16×16 | `StatusBarView.swift:2180` | Close xmark. |
| `tab_active_underline_height` | 2 | `Chrome/FlatChrome.swift:24` | Bottom underline. |
| `tab_active_underline_inset` | 7 | `StatusBarView.swift:2202-2206` | Each side; width minus 14. |
| `tab_active_bg` | `selectedControlColor` at 0.14 | `StatusBarView.swift:2322` | Non-aggregate active tab. |
| `tab_aggregate_active_bg` | accent at 0.20 | `StatusBarView.swift:2317` | Shells/Agents. |
| `tab_aggregate_inactive_bg` | accent at 0.065 | `StatusBarView.swift:2317` | Shells/Agents. |
| `tab_aggregate_inactive_text_alpha` | 0.78 | `StatusBarView.swift:2281` | Accent text. |
| `tab_close_tint_alpha` | 0.82 | `StatusBarView.swift:2327` | Aggregate close xmark. |
| `tab_drag_threshold` | 4 | `StatusBarView.swift:2248` | Euclidean points. |
| `tab_drag_dim_alpha` | 0.65 | `StatusBarView.swift:2252` | Source tab during drag. |
| `activity_working_dark` | #FFB82E | `StatusBarView.swift:2416-2418` | srgb 1.0, 0.72, 0.18. |
| `activity_working_light` | #B33809 | `StatusBarView.swift:2419` | srgb 0.70, 0.22, 0.035. |
| `activity_rotation_duration` | 0.9 s | `StatusBarView.swift:2458` | Linear infinite rotation, stopped for Reduce Motion. |
| `jump_latest_height` | 26 | `App/ContentView.swift:149` | Not in the UI slice but visible over the terminal. |
| `jump_latest_radius` | 13 | `App/ContentView.swift:75` | Pill radius. |
| `jump_latest_min_width` | 72 | `App/ContentView.swift:151` | Pill minimum. |
| `jump_latest_font` | 12 semibold | `App/ContentView.swift:79` | System font. |
| `last_seen_height` | 24 | `App/ContentView.swift:146` | Not in the UI slice. |
| `last_seen_radius` | 12 | `App/ContentView.swift:87` | Pill radius. |
| `last_seen_font` | 11 medium | `App/ContentView.swift:90` | System font. |
| `logo_bg_start` | #091625 | `Resources/MuxtermLogo.svg:6` | Application icon. |
| `logo_bg_end` | #1a3044 | `Resources/MuxtermLogo.svg:7` | Application icon. |
| `logo_mark` | #f4f0e6 | `Resources/MuxtermLogo.svg:19-22` | M strokes. |
| `logo_mint_start` | #70e6b5 | `Resources/MuxtermLogo.svg:10` | X gradient. |
| `logo_mint_end` | #35c99e | `Resources/MuxtermLogo.svg:11` | X gradient. |
| `logo_corner_radius` | 238/1000 | `Resources/MuxtermLogo.svg:18` | Rounded background in viewBox units. |
| `logo_art_scale` | 0.82 with 90-unit translation | `Resources/MuxtermLogo.svg:17` | Transparent margin. |

Some values above come from `Chrome` or `App` callers rather than the seven UI files. They appear because the UI files own the rendered pixels even when a constraint is installed by the caller. The table is deliberately a flat list of both sizes and alpha tokens; a Rust theme module can nest them later without changing the names.
