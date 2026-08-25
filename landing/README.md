# landing

The next public landing page for runtrol. **Local development only.** It is not wired into the GitHub
Pages workflow on purpose: [`site/`](../site/) is what `pages.yml` deploys on every push, and this folder
stays out of that trigger until the page is promoted.

## Run locally

```text
npm --prefix landing test    # contract test with eight red-path mutations
npm --prefix landing run dev # http://127.0.0.1:4173/  (PORT=... to change)
npm --prefix landing run build
```

No dependencies. The dev server serves this folder, the shared `site/release-assets.mjs`, and the brand
SSOT under `assets/brand/`. The phone app route `app/` is not served locally; the Pages job copies it
from `pwa/dist` at deploy time.

## What is different from `site/`

- Accent is `#F56565` (coral) instead of the brand orange. The mark is two-tone: two arms coral, two arms
  `currentColor`, so the ink arms are white on dark and graphite on light. `assets/brand/` is unchanged;
  the two-tone mark lives inline in `index.html` until the brand SSOT adopts it.
- Icons are Lucide (ISC). Only the used paths are vendored in `icons.js`; the page places them with
  `<i data-icon="name">` and `app.js` inlines the SVG. The footer carries the attribution.
- The hero is an animated Runtrol Studio window (`scene.js`): the sidebar tree fills in, conversations
  open as editor tabs, the running agent icon spins, usage ticks, an approval asks for the user, and the
  phone toast arrives. One keyframe list drives it from a single clock, it pauses off screen, and reduced
  motion shows the finished state.

## Promotion

When the page goes public: copy `index.html`, `styles.css`, `app.js`, `icons.js`, `scene.js`, and
`test.mjs` over the matching files in `site/`, add `icons.js` and `scene.js` to `sourceFiles` in
`site/build.mjs`, run `npm --prefix site test && npm --prefix site run build`, then delete this folder
and its entry in `tests/audit/workspaceHygiene.py`. The push deploys through `pages.yml`.
