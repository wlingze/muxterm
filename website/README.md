# Muxterm Website

Official static website for Muxterm. Plain HTML, CSS, and JavaScript: no build
step, no package manager, and no runtime requests to third-party services.

## Files

```text
website/
├── index.html                  landing page (English)
├── docs.html                   quickstart guide (English)
├── 404.html                    GitHub Pages fallback
├── .nojekyll                   serve files as-is (no Jekyll)
├── zh/
│   ├── index.html              landing page (Chinese)
│   └── docs.html               quickstart guide (Chinese)
├── assets/
│   ├── style.css               page styles (Hallmark stamp on line 1)
│   ├── tokens.css              design tokens (colour, type, spacing)
│   ├── base.css                reset and accessibility baseline
│   ├── theme-init.js           applies the saved theme before first paint
│   ├── app.js                  theme, install tabs, copy, port demo, surface
│   ├── fonts/
│   │   ├── martian-mono-latin.woff2
│   │   └── OFL.txt
│   ├── img/
│   │   ├── app-workspace-{dark,light}.png
│   │   ├── app-agents-{dark,light}.png
│   │   ├── app-shells-{dark,light}.png
│   │   └── app-ports-{dark,light}.png
│   └── muxterm-logo*.svg       brand assets copied from ../assets/
└── README.md
```

## Local preview

Run a plain HTTP server from the repository root. Binding to `0.0.0.0` lets
another device on the LAN open the preview too.

```bash
python3 -m http.server 8000 --bind 0.0.0.0 --directory website
```

Open <http://127.0.0.1:8000/>. The Chinese site is at
<http://127.0.0.1:8000/zh/>. The site is also safe to open through the
`file://` protocol, though the copy button may be limited by browser security
rules.

## Deployment

The site is deployed by [`.github/workflows/pages.yml`](../.github/workflows/pages.yml)
on every push to `main` that touches `website/`, or manually through
**Actions → Website → Run workflow**.

One-time repository setup (already done for this repository): open
**Settings → Pages** and set **Build and deployment → Source** to
**GitHub Actions**. The Pages site is created with that source; the workflow
then publishes on every push to `main`.

The project site URL is:

```text
https://wlingze.github.io/muxterm/
https://wlingze.github.io/muxterm/zh/
```

That is the GitHub *project site* form: `https://<owner>.github.io/<repo>/`.
A `muxterm.github.io` address would need an account named `muxterm`, and a user
site needs a repository named `wlingze.github.io`. The GitHub account `muxterm`
already exists as somebody else's user account, so the organization route cannot
produce that host either. A custom domain is the only way to drop the owner
prefix: add it under **Settings → Pages → Custom domain**, point DNS at Pages,
and keep the workflow as is (a `CNAME` file is ignored when publishing through
Actions).

All internal links are relative (`./assets/...`, `./docs.html`, `../assets/...`
from `zh/`), so the site works both at that subpath and at a custom domain
without changes.

## Pages

- **Landing page:** hero promise, a three-up screenshot gallery (each frame opens
  the full-size PNG), the port detection and forwarding section, the runtime
  matrix, and downloads.
- **Docs page:** a short quickstart (install, first workspace, connect, ports,
  config, CLI, build). Repository documents are **not** rendered on the site;
  the long-form reference stays in the repository and on GitHub, and the docs
  page links there.

## Themes and languages

- **Theme:** dark by default; the navigation toggle switches to light. Both the
  page and every product screenshot follow the selected theme. The choice is
  stored in `localStorage` under `muxterm-theme`; `?theme=dark` or `?theme=light`
  forces a theme for screenshots and review.
- **Language:** English lives at the site root, Chinese under `zh/`. Each page
  links to its counterpart with `hreflang`.
- **Screenshots:** the gallery has its own dark/light switch. Both switches
  drive the same theme, so the gallery images, their full-size links, the port
  figure, and the surrounding page never disagree. Without JavaScript the dark
  images and the dark theme are the default.

## Screenshots

`assets/img/` holds real captures of the macOS app, each in dark and light:

| File | Scene | Size |
| --- | --- | --- |
| `app-workspace-*` | workbench: workspace sidebar, two tabs, four tmux panes | 1400 × 831 |
| `app-agents-*` | Agents aggregate: Codex and Claude panes with state | 1400 × 831 |
| `app-shells-*` | local Shells aggregate: single pane, plain shell | 1400 × 831 |
| `app-ports-*` | port picker popover behind the connection dot, eight remote ports | 1000 × 616 |

The window scenes come from
`src/frontend/macos/AppE2ETests/WebsiteScreenshotE2ETests.swift` and
`WebsiteGalleryE2ETests.swift`; the port popover comes from
`SSHPortPickerE2ETests.testWebsitePortPickerSnapshot()`. The tests use isolated
tmux sockets (`muxterm-test-*`), an isolated `ZDOTDIR`, and mock terminal
output; they never read or write the user's sessions.

Images are rendered at an explicit pixel width through
`AppE2E.snapshotPNG(of:maxWidth:)` (1400 for windows, 1000 for the popover), so
the committed files stay small and keep a stable ratio. On the page they are
displayed small in a three-up gallery; clicking a shot opens the full-size PNG
in a new tab.

To regenerate after a UI change:

```bash
# Build the Rust static library and patch SwiftTerm once.
./scripts/build-macos.sh

cd src/frontend/macos
for mode in dark light; do
  MUXTERM_SCREENSHOT_APPEARANCE="$mode" \
  MUXTERM_UI_SNAPSHOT_DIR="$PWD/../../../website/assets/img" \
  DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer \
  swift test --disable-swift-testing \
    --filter 'WebsiteScreenshotE2ETests|WebsiteGalleryE2ETests|SSHPortPickerE2ETests/testWebsitePortPickerSnapshot'
done
```

`MUXTERM_UI_SNAPSHOT_DIR` is required; without it the tests still check their
views, but write no PNG. The tests force the matching app appearance, disables
the update check, and restores the process appearance in `tearDown`. The port
picker snapshot switches `NSApplication.shared.appearance` **before** building
the view: label colours resolve at construction, so setting `view.appearance`
afterwards leaves `secondaryLabelColor` text invisible on dark.

## Design notes

- Design system: Hallmark `Terminal` theme (Martian Mono, dark paper, one
  phosphor-green accent). The stamp at the top of `assets/style.css` records the
  theme, draw, posture, and axes.
- The hero product surface is a live reconstruction of the Muxterm
  workspace/sidebar/tab/pane layout. The workspace buttons switch runtimes so
  the three-lane model is visible without leaving the page.
- The screenshot section is a three-up gallery: equal frames at a third of the
  content width, an index label, a short title, and one sentence per scene.
  Each frame is a link to the full-size PNG in a new tab. The gallery collapses
  to two columns at 68rem and one column at 44rem. Below it, the ports section
  pairs the real port-picker capture with an interactive forwarding demo built
  from the same Core/FFI capability.
- The runtime matrix is a real `<table>`. Below 44rem the header row is hidden
  and each row stacks into a labelled card (`td::before` reads `data-label`),
  so nothing overflows the viewport.
- No analytics, tracking pixels, cookies, or third-party requests. The font is
  self-hosted.
- Content rules: no invented metrics, testimonials, logos, or quotes. Release
  requirements and download names come from `README.md` and
  `.github/workflows/release.yml`.

## Updating

- Landing copy: `index.html`, `zh/index.html`
- Quickstart: `docs.html`, `zh/docs.html`
- Shared layout and tokens: `assets/style.css`, `assets/tokens.css`,
  `assets/base.css`
- Install snippet commands: `commands` in `assets/app.js`
- Brand mark: replace `assets/muxterm-logo.svg` and keep the same dimensions.
- Run the Hallmark lint before shipping:

```bash
node ~/.agents/skills/hallmark/scripts/sloplint.mjs \
  --genre atmospheric --scope page \
  website/index.html website/docs.html \
  website/zh/index.html website/zh/docs.html \
  website/404.html \
  website/assets/style.css website/assets/tokens.css website/assets/base.css
```
