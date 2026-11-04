# Muxterm Website

Official static website for Muxterm. Plain HTML, CSS, and JavaScript: no build
step, no package manager, and no runtime requests to third-party services.

## Files

```text
website/
├── index.html                  landing page (English)
├── docs.html                   quickstart plus repository reference (English)
├── 404.html                    GitHub Pages fallback
├── .nojekyll                   serve files as-is (no Jekyll)
├── zh/
│   ├── index.html              landing page (Chinese)
│   └── docs.html               quickstart plus repository reference (Chinese)
├── assets/
│   ├── style.css               page styles (Hallmark stamp on line 1)
│   ├── tokens.css              design tokens (colour, type, spacing)
│   ├── base.css                reset and accessibility baseline
│   ├── theme-init.js           applies the saved theme before first paint
│   ├── app.js                  theme, install tabs, copy, demos, docs loader
│   ├── fonts/
│   │   ├── martian-mono-latin.woff2
│   │   └── OFL.txt
│   ├── img/
│   │   ├── app-workspace-{dark,light}.png
│   │   ├── app-agents-{dark,light}.png
│   │   └── app-shells-{dark,light}.png
│   ├── vendor/
│   │   ├── marked.min.js       Markdown renderer for the docs reference
│   │   └── marked-LICENSE.txt
│   └── muxterm-logo*.svg       brand assets copied from ../assets/
├── docs/                       rendered copies of the repository reference docs
├── scripts/
│   └── sync-docs.sh            copy docs/ and PRODUCT.md into website/docs/
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
`file://` protocol, though the copy button and `fetch`-based docs reference may
be limited by browser security rules.

## Deployment

The site is deployed by [`.github/workflows/pages.yml`](../.github/workflows/pages.yml)
on every push to `main` that touches `website/`, or manually through
**Actions → Website → Run workflow**.

One-time repository setup:

1. Open **Settings → Pages**.
2. Under **Build and deployment**, set **Source** to **GitHub Actions**.
3. Push to `main` (or run the workflow manually).

The project site URL is:

```text
https://wlingze.github.io/muxterm/
```

All internal links are relative (`./assets/...`, `./docs.html`, `../assets/...`
from `zh/`), so the site works both at that subpath and at a custom domain
without changes.

## Themes and languages

- **Theme:** dark by default; the navigation toggle switches to light. Both the
  page and the product screenshot follow the selected theme. The choice is
  stored in `localStorage` under `muxterm-theme`; `?theme=dark` or `?theme=light`
  forces a theme for screenshots and review.
- **Language:** English lives at the site root, Chinese under `zh/`. Each page
  links to its counterpart with `hreflang`.
- **Product screenshot:** the dark/light buttons in the screenshot frame set the
  page theme, so the image and the surrounding page never disagree. Without
  JavaScript the dark image and dark theme are the default.

## Screenshots

`assets/img/` contains real captures of the macOS app: the four-pane
workbench, the Agents aggregate, and the local Shells aggregate. They are taken
by `src/frontend/macos/AppE2ETests/WebsiteScreenshotE2ETests.swift` and
`WebsiteGalleryE2ETests.swift`. The tests use isolated tmux sockets
(`muxterm-test-*`), an isolated `ZDOTDIR`, and mock terminal output; they never
read or write the user's sessions. Captures use the 1280 × 760 window and the
display's backing scale; the committed PNGs are 1280 × 760.

To regenerate after a UI change:

```bash
# Build the Rust static library and patch SwiftTerm once.
./scripts/build-macos.sh

cd src/frontend/macos
for mode in dark light; do
  MUXTERM_SCREENSHOT_APPEARANCE="$mode" \
  MUXTERM_UI_SNAPSHOT_DIR="$PWD/../../../website/assets/img" \
  swift test --filter 'WebsiteScreenshotE2ETests|WebsiteGalleryE2ETests'
done
```

`MUXTERM_UI_SNAPSHOT_DIR` is required; without it the test still checks the
window, but writes no PNG. The test forces the matching app appearance and
theme, disables the update check, and restores the process appearance in
`tearDown`.

## Documentation reference

`docs.html` and `zh/docs.html` start with a curated quickstart. The sidebar's
**reference** group lists the long-form repository documents; selecting one
replaces the quickstart with that document in the same reading column. The
document is rendered in the browser with
[marked](https://github.com/markedjs/marked) (vendored, MIT). There is no nested
reader or tab strip: the selected document is the page. The same document can
also be opened on GitHub.

Source files are copied into `website/docs/` so the site remains static and has
no build step. After editing `PRODUCT.md` or a file in `docs/`, sync the copies:

```bash
./website/scripts/sync-docs.sh
```

The copies are committed. Edit the originals, never `website/docs/`, and re-run
the sync script.

## Design notes

- Design system: Hallmark `Terminal` theme (Martian Mono, dark paper, one
  phosphor-green accent). The stamp at the top of `assets/style.css` records the
  theme, draw, posture, and axes.
- The hero product surface is a live reconstruction of the Muxterm
  workspace/sidebar/tab/pane layout. The workspace buttons switch runtimes so
  the three-lane model is visible without leaving the page.
- The gallery below the hero is a horizontal, snap-scrolling slider with three
  real app captures: the workbench, the Agents aggregate, and the Shells
  aggregate. Both themes are stored as separate PNGs; the theme toggle swaps
  every slide and updates `aria-pressed` on both the navigation and screenshot
  controls. The arrow buttons and `1 / 3` counter expose the same scroll state.
- No analytics, tracking pixels, cookies, or third-party requests. The font and
  Markdown renderer are self-hosted.
- Content rules: no invented metrics, testimonials, logos, or quotes. Release
  requirements and download names come from `README.md` and
  `.github/workflows/release.yml`.

## Updating

- Landing copy: `index.html`, `zh/index.html`
- Quickstart: `docs.html`, `zh/docs.html`
- Long-form reference: the repository `docs/` files, then
  `./website/scripts/sync-docs.sh`
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
