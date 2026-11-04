# Terminal and CoreBridge parity inventory

Scope: `src/frontend/macos/Terminal/` (`TerminalManager.swift` 1,779 lines,
`TerminalView.swift` 1,475 lines) and `src/frontend/macos/CoreBridge/`
(`CoreBridge.swift` 2,622 lines plus `shim.c` and `include/`).

Citations without a directory prefix are relative to `src/frontend/macos/`.

Inspected directly: the file and function inventory of both Terminal files,
the IME path, the wheel path, the paste path, the query reply builder, the grid
size path, and the seed bookkeeping fields. Not read line by line: the whole of
`CoreBridge.swift`; the FFI entry table below comes from the symbols the Swift
frontend actually calls, cross-checked against `src/frontend/utils/corebridge.rs`.

## 1. Who owns what

| Layer | Owns | Source |
| --- | --- | --- |
| `TerminalManager` | Pane surface registry, seed scheduling, output queuing, size sync, history capacity, traffic stats | `Terminal/TerminalManager.swift:11-193` |
| `MuxTerminalView` | One pane's VT and its rendering, input encoding, mouse, IME, selection, clipboard | `Terminal/TerminalView.swift:99-1199` |
| SwiftTerm | VT parsing and glyph shaping | external dependency |
| `CoreBridge` | C ABI wrapper, owned DTOs, event decoding | `CoreBridge/CoreBridge.swift` |
| Main window | The single UI-to-Core command boundary | `TerminalManager.enqueueCoreCommand`, `TerminalManager.swift:190` |

The manager already enforces the project rule that terminal input and resize
callbacks never call `CoreBridge` directly. `enqueueCoreCommand` is the only
write path (`TerminalManager.swift:190`).

## 2. Surface lifecycle rules

| Rule | Detail | Source |
| --- | --- | --- |
| Resident per pane | One `MuxTerminalView` per pane id; hidden workspaces keep the view and keep feeding it | `TerminalManager.swift:524-592` |
| Seed before display | A pane that has not shown its first frame hides the host until seeding finishes, so the user never sees a blank terminal; a pane that has shown before stays visible during a re-seed to avoid a white flash | `TerminalManager.swift:117-121` |
| Seed chunking | Seeds are flushed in 32 KiB chunks inside a 4 ms budget per tick | `TerminalManager.swift:125-127`, `476-515` |
| Stashed output | Background output is buffered behind a 256 KiB cap; past the cap the pane is marked as needing an authoritative snapshot and incremental bytes are dropped until Runtime sends one | `TerminalManager.swift:102-111`, `780-795` |
| Snapshot vs frame | A stashed frame must replace the screen when the scene becomes visible; it may not degrade into an append | `TerminalManager.swift:104-106` |
| Feed ordering | Output is flushed before a resize is applied | `TerminalManager.swift:671-684` |
| One grid writer | tmux pane cells are the truth; the model is resized to them and never the other way round | `TerminalView.swift:937-962`, `1070-1105` |
| Model floor | Model size never goes below 2x1 | `TerminalView.swift:929-932` |

## 3. Input path

| Capability | Behaviour | Source |
| --- | --- | --- |
| Plain text | AppKit `interpretKeyEvents` to `insertText`, one path | `TerminalView.swift:122-158` |
| IME placeholder filter | A key event whose characters are entirely U+FFFF or U+FFFE is dropped before SwiftTerm can encode it as `EF BF BF`. Committed text is filtered scalar by scalar, and a placeholder-only commit suppresses SwiftTerm's synthetic key | `TerminalView.swift:123-127`, `158-186` |
| Modifier arrows | Before AppKit turns them into selection movement, Shift/Ctrl/Option with arrow keycodes 123-126 is encoded as `ESC [ 1 ; <modifier> <A|B|C|D>`, where modifier is 1 + shift + 2 option + 4 control. Skipped while `hasMarkedText()` | `TerminalView.swift:131-152` |
| Candidate window | `firstRect` anchors on the caret, and when the cursor sits in the last column it wraps to `x=0, y-=height` so the candidate window is not clipped at the pane edge | `TerminalView.swift:106-120` |
| Bracketed paste | Clipboard text is wrapped in `ESC[200~` / `ESC[201~` when the terminal has bracketed paste on | `TerminalView.swift:681-694` |
| Query replies | OSC 10/11/12 answer `ESC ] <code> ; rgb:RRRR/GGGG/BBBB ESC \`. Primary DA answers `ESC [ ? 65 ; ... c`. Getting these wrong makes zsh run `10;rgb:...` as a command | `Chrome/TerminalQueryReply.swift:1-59` |
| External text | `insertExternalText` for the composer write path | `TerminalView.swift:658-680` |

## 4. Wheel routing

`WheelPassthroughPolicy.route` (`Chrome/FlatChrome.swift:1388-1413`) decides
between four routes, in this precedence.

| Route | Condition | Wire form |
| --- | --- | --- |
| `serverScroll` | Runtime owns the viewport (Herdr) | `ScrollPane` task, with cell position and modifier byte when available |
| `applicationMouse` | Terminal has mouse reporting on and Shift is not held | SGR wheel `ESC [ < 64|65 ; col+1 ; row+1 M` |
| `applicationArrows` | Alternate screen without mouse reporting | `ESC [ A` or `ESC [ B` repeated per line |
| `localHistory` | Everything else | Local scrollback |

Precise touchpad deltas are accumulated into whole cells before sending, and
the line count is capped, because one gesture otherwise emits dozens of
`send-keys` calls (`TerminalView.swift:492-543`).

## 5. Grid size and cell metrics

| Behaviour | Source |
| --- | --- |
| Pixel allocation to columns and rows uses a shared cell size so every pane in a workspace agrees | `TerminalManager.swift:516-523` |
| A resize keeps the tail visible when the pane was already at the tail, and keeps the user's scroll position when it was not | `TerminalView.swift:937-962` |
| PTY resize is skipped for allocations below 40x24 pixels, which would otherwise produce a zero-column grid | `TerminalView.swift:1074-1076` |
| Cell size is derived twice, once from SwiftTerm and once from the drawn glyph, and the drawn value wins | `TerminalView.swift:1161-1198` |
| Pane colours are read from the theme and reported to Core so search and activity know the pane's light or dark background | `TerminalView.swift:1039-1069` |

## 6. History and scrollback

| Behaviour | Source |
| --- | --- |
| History lines are prepended as lines, not replayed as VT bytes | `TerminalView.swift:872-887` |
| Scroll position is expressed as a history offset with a max offset supplied by Core | `TerminalView.swift:888-906` |
| `ensureHistoryCapacity` grows local capacity to at least what Core reported | `TerminalView.swift:923-928` |
| Scroll lines are clamped and drive `setPaneViewport` through the command queue | `TerminalView.swift:907-922` |

## 7. Rendering resilience

| Behaviour | Source |
| --- | --- |
| Committed paint keeps the last complete frame in a fixed-size bitmap, so an expose or selection redraw cannot reveal an intermediate state of a DEC 2026 synchronized frame. Memory scales with pane size, not frame count | `TerminalView.swift:700-760` |
| Unreadable frames restore the previous rendition rather than showing garbage | `TerminalView.swift:844-871` |
| Accessibility output is updated from the visible screen text | `TerminalView.swift:969-987` |

## 8. CoreBridge entry points the Swift frontend actually calls

62 distinct bridge calls, extracted from `bridge.<name>` occurrences across
`App/`, `Chrome/`, `UI/`, and `Terminal/`.

| Purpose | Calls |
| --- | --- |
| Workspace lifecycle | `openWorkspace`, `closeWorkspace`, `reconnectWorkspace`, `selectWorkspace`, `detach`, `shutdown`, `workspaceList`, `flushWorkspaceSelection` |
| Topology | `getTabs`, `getPanes`, `getLayout`, `pollWorkspaceEvents` |
| Pane data | `getPaneOutput`, `paneViewport`, `setPaneViewport`, `paneHistoryMaxOffset`, `paneCommandMarks`, `paneLatestLineSeq`, `paneViewportOffsetForSeq` |
| Input | `sendInput`, `sendInputQuiet`, `sendNamedKey`, `resizePane`, `resizePaneAxis`, `resizeClient` |
| Events and snapshots | `snapshot`, `takeError`, `statusSubscriptionActive`, `statusBarSnapshotJSON`, `structuredAgentSnapshot` |
| Attention | `attentionSnapshotJSON`, `attentionAcknowledge`, `attentionMute`, `attentionOnBecameVisible` |
| Config | `configDescribeJSON`, `configBegin`, `configPatch`, `configCommit`, `configCancel`, `directConfigTransactionHandler` |
| Search | `searchAllJSON` |
| Capabilities | `runtimeSupports`, `runtimeUpdates`, `resolvedTargetConfig` |
| SSH | `forwardSSHPort`, `ignoreSSHPort`, `stopSSHPort`, `workspaceSSHPorts`, `workspaceTrafficBytes` |
| Clipboard images | `startImagePaste`, `startImageStore` |
| Updates | `updateCheck`, `updateInstall`, `updateStatus`, `startRuntimeUpdate` |
| Colours | `reportPaneColours`, `reportAllPaneColours` |
| Session facts | `session`, `socket`, `sshAlias`, `startDirectory`, `execute` |

`src/frontend/utils/corebridge.rs` exposes 113 public functions. Most Swift
calls have a Rust counterpart under a workspace-prefixed name, for example
`selectWorkspace` against `activate_workspace`, `setPaneViewport` against
`set_workspace_pane_viewport`, `resizePaneAxis` against
`resize_workspace_pane_axis` (`corebridge.rs:1683`), and
`statusBarSnapshotJSON` against `status_snapshot_json` (`corebridge.rs:2750`).

Five have no counterpart at all, which is net-new FFI surface rather than a
rename: `resolvedTargetConfig`, `sendNamedKey`, `startRuntimeUpdate` with
`runtimeUpdates`, `structuredAgentSnapshot`, and `takeError`.

## 9. gpui Surface gaps

This is the actionable table. Each row is a capability the macOS frontend has
today and `src/frontend/gpui/terminal/` does not. The symptom column is what a
user would actually notice.

| Gap | Symptom without it | Parity source | Size |
| --- | --- | --- | --- |
| IME marked text and placeholder filtering | Typing Chinese shows `<ffff>` in the shell, and a placeholder commit corrupts the command line | `TerminalView.swift:123-186` | medium |
| Candidate window anchoring | The IME candidate window is clipped at the pane's right edge when the cursor is on the last column | `TerminalView.swift:106-120` | small |
| Modifier arrow encoding before IME resolution | Shift+arrow and Ctrl+arrow move a selection instead of reaching the program | `TerminalView.swift:131-152` | small |
| Query replies (OSC 10/11/12, DA1) | zsh prints `command not found: 10;rgb:...` when a program queries colours | `Chrome/TerminalQueryReply.swift` | small |
| Bracketed paste wrapping | Pasting multi-line text executes each line instead of inserting it | `TerminalView.swift:681-694` | small |
| Mouse reporting, SGR and X10 | vim and htop ignore the mouse; scrollbar and click-to-position do nothing | `TerminalView.swift:544-612` | medium |
| Wheel routing policy and coalescing | One touchpad gesture floods the pane with dozens of sends, or scrolling cannot reach the program at all | `TerminalView.swift:451-543` | medium |
| Local scrollback and viewport offsets | Cannot scroll back through output; Core history max offset is never used | `TerminalView.swift:888-928` | medium |
| Wide and combining characters | CJK text and emoji occupy one cell instead of two, so every line after the first drifts | `TerminalView.swift:141-160` | medium |
| Progressive word selection | Double and triple click select nothing; drag selection does not exist | `TerminalView.swift:315-450` | medium |
| Clipboard image paste | Pasting a screenshot inserts nothing | `TerminalView.swift:8-31`, `614-657` | medium |
| Committed paint | Resizing under a synchronized-output program briefly shows a torn frame | `TerminalView.swift:700-760` | large |
| Per-pane colour reporting | Search results and activity badges do not know the pane background, so highlight contrast is wrong | `TerminalView.swift:1039-1069` | small |
| Accessibility output | Screen readers report nothing for terminal content | `TerminalView.swift:969-987` | small |
| Seed chunking and stashed output cap | A background pane with heavy output either blocks the UI or loses its baseline | `TerminalManager.swift:102-127`, `780-795` | medium |

Two of these are structural rather than incremental. Committed paint depends on
retaining a rendered frame, which gpui does not expose, so the honest options
are to skip it or to draw pane content into an owned bitmap element. Wide
characters need a width table, which `unicode-width` provides, plus a grid that
stores a wide cell and its continuation.

## 10. What this means for the port order

The Surface gaps split into three groups by cost.

Group one is pure encoding with no rendering dependency and can land as unit
tests today: query replies, bracketed paste, modifier arrows, wheel routing
decision, and wheel coalescing. These are exactly the rules that already live in
pure Swift and would live in pure Rust.

Group two needs grid and event work: mouse reporting, wide characters,
scrollback offsets, progressive selection.

Group three needs rendering control: committed paint, and anything that depends
on a real frame.

Group one should go first because it is the cheapest proof that the parity
approach works at all.
