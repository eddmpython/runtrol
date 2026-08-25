# Site Deployment

The public site is live at [https://eddmpython.github.io/runtrol/](https://eddmpython.github.io/runtrol/). It is the landing page for the product and the permanent origin of the phone app at `/runtrol/app/`. GitHub Pages is configured with `build_type: workflow` and enforced HTTPS; nothing is served from a branch.

This document is the operating manual: what is deployed, what deploys it, how to change it, how to check it, and how to roll it back.

## What is deployed

| Path on the origin | Source | What it is |
|---|---|---|
| `/runtrol/` | [`site/`](../site/) | The landing page: static HTML, CSS, and browser JavaScript, no dependencies |
| `/runtrol/app/` | [`pwa/`](../pwa/) | The phone app. See [phonePwa.md](phonePwa.md) for its own contract |
| `/runtrol/assets/brand/` | [`assets/brand/`](../assets/brand/) | Favicon, touch icon, and social card, copied from the brand SSOT |

The build never fetches anything. There is no CDN, no web font, no analytics, and no external script; the contract test refuses them.

## What deploys it

[`pages.yml`](../.github/workflows/pages.yml) runs on every push to `main` that touches `site/**`, `pwa/**`, `assets/brand/**`, `assets/event-presentation.json`, or the workflow file itself, and on manual dispatch from the Actions tab. A push that touches only `docs/` or `crates/` does not deploy.

The job order is fixed: site contract test, phone app contract test, dependency-free build, configure Pages, upload artifact, deploy into the `github-pages` environment. A failing test stops the run before anything is uploaded, so the live site only ever changes when both contracts pass.

Permissions are the minimum for each job. The build job has repository read and Pages read. The deploy job has Pages write and an OIDC token. Every external action is pinned to a full commit digest, not a tag.

There is no staging origin. The way to see a change before it is public is the local preview below, and the way to publish is the push.

## Everyday change

1. Edit the source in `site/` (or `pwa/`, or the brand geometry in `assets/brand/render.py`).
2. `npm --prefix site test` and, for phone app changes, `npm --prefix pwa test`.
3. `npm --prefix site run dev` and look at `http://127.0.0.1:4173/` in a browser, in both themes. A layout that passes the tests can still be wrong on screen; the tests do not see pixels.
4. `npm --prefix site run build` to confirm the artifact assembles and stays under budget.
5. Commit with explicit paths and push. The workflow deploys within about a minute.
6. Confirm the run and the origin (see "After a deploy").

Everything the page says must be true at the moment it is pushed. The page does not carry a release version; it asks GitHub Releases at load time, so a release does not require a site change and a site change does not require a release.

## Local preview

`npm --prefix site run dev` serves `site/` and `assets/brand/` on loopback at `http://127.0.0.1:4173/` (set `PORT` to change it) with the same relative paths the build emits, so `index.html` runs unchanged and edits show on reload. The phone app route `app/` is not served by the preview; run the build and open `site/dist/` for that.

The preview is a development surface. It binds only `127.0.0.1` and sends `cache-control: no-store`.

## Build contract

`npm --prefix site run build` deletes only the generated `site/dist` and `pwa/dist`, runs the phone app build, copies the site sources and the listed brand files, copies the phone app output under `app/`, writes `.nojekyll`, and then checks the result:

- every listed brand file and the `app/` route are referenced by the built `index.html`;
- the whole output is at most 250,000 bytes.

The first public deployment was 77,429 bytes. The current landing plus phone app is about 223,000 bytes; the animated hero and the Lucide paths cost roughly 40 KB of that. Raising the budget is a deliberate decision, not a side effect of a large asset.

Main content, product status, install instructions, the Marketplace link, and the phone app link are all present in the static HTML. JavaScript adds language switching, theme switching, the hero animation, and release discovery; without it the page still says everything that matters.

## Contract test

`npm --prefix site test` reads the source files and checks:

- English is the static default;
- Korean, Chinese, and Japanese dictionaries exist;
- the North Star and the 30-conversation gate are visible in static HTML;
- the live Marketplace and phone app routes are statically available;
- manual installation selects the matching native `.vsix` asset from the latest GitHub Release;
- there is no hardcoded release version and no external style, font, or script;
- the mark is two-tone (accent arms and ink arms), the accent token is the coral, and every icon placed in the page is vendored in `icons.js`;
- the channel row links GitHub, support, YouTube, and Threads;
- the hero scene respects reduced motion and pauses off screen;
- client code never persists conversation content;
- forbidden dash characters do not enter site source.

The test then injects nine defects (wrong default language, wrong Marketplace identity, an orange-only mark, a removed phone app claim, an unvendored icon, the legacy accent, a scene that ignores reduced motion, a missing channel, a forbidden dash) and requires each mutation to fail before the valid source passes. A check that cannot fail is not a check.

## Page anatomy

| File | Owns |
|---|---|
| `index.html` | Static content in English, the `data-i18n` keys, the inline two-tone mark, the channel row, and the hero markup tagged with `data-scene` |
| `styles.css` | Tokens (`--accent: #f56565`, ink, lines), light and dark palettes, the Studio window, sections, responsive rules |
| `app.js` | Language and theme preferences, the four copy dictionaries, icon mounting, release discovery |
| `icons.js` | The Lucide subset (ISC, `lucide-static 1.34.0`). Add an icon by pasting the inner markup of its SVG under a new key |
| `scene.js` | The hero storyboard: one keyframe list, one clock, a 15 s loop |
| `release-assets.mjs` | Native target inference and `.vsix` selection, shared with the tests |
| `build.mjs`, `test.mjs`, `dev.mjs` | Build, contract test, preview |

### Copy

Every visible string that changes with language carries `data-i18n="key"` in `index.html` and an entry under `en`, `ko`, `zh`, and `ja` in `app.js`. English in the HTML is the source; the dictionaries are projections. A key missing from a dictionary falls back to English at runtime and is a defect to fix, not a feature.

### Icons

Icons are placed as `<i data-icon="name" data-size="16"></i>` and inlined by `app.js`. Only icons present in `icons.js` may be placed; the test fails on any other name. No icon is ever loaded from the network.

### Hero scene

`scene.js` drives the Runtrol Studio window: the sidebar tree fills in, two conversations open as editor tabs, the running agent's icon spins, usage ticks, an approval asks for the user, and the phone toast arrives. Every animated element is tagged `data-scene` and starts hidden; the keyframe list switches classes on a single `requestAnimationFrame` clock. The scene pauses when the window is hidden or the Studio is off screen, and `prefers-reduced-motion` renders the finished state once with no motion. The sample text is fixed copy that never came from a real transcript.

## Brand assets

The favicon, touch icon, Marketplace icon, and social cards are generated, not drawn. `python -X utf8 assets/brand/render.py` rebuilds every SVG, PNG, and ICO in `assets/brand/` from the geometry table in [`assets/brand/README.md`](../assets/brand/README.md) with no dependencies, and the same inputs produce the same bytes. Change the geometry or a colour in `render.py`, run it, look at the results, and commit the whole folder; a push that touches `assets/brand/**` redeploys the site so the favicon and social card update together.

The mark is two-tone: two arms coral `#F56565`, two arms in the ink of the surface (graphite on light, white on dark). The page inlines the mark so its ink arms follow the theme through `currentColor`; `favicon.svg` follows the tab strip through an embedded media query.

## After a deploy

- `gh run list --workflow=pages.yml --limit 1` shows the run for the push; it should read `completed success` and take under a minute.
- `curl -sI https://eddmpython.github.io/runtrol/ | head -1` returns `HTTP/2 200`.
- Open the origin in a browser. GitHub's CDN caches for a few minutes; a hard reload shows the new build.
- The phone app at `/runtrol/app/` must still load and still pair. A landing change cannot break it, but a `pwa/` change deploys through the same artifact.

Runs of note: `31545875267` was the first public deployment at commit `59625bb`; `31557512027` deployed the live Marketplace actions and target-aware manual fallback at `1bf19d1`; `32801255495` deployed the two-tone mark and the animated Studio landing at `fa44285`.

## Rolling back

The site is exactly what is on `main`. To undo a deploy, `git revert` the commit and push; the workflow redeploys the previous content. To redeploy the current `main` without a change (for example after a Pages outage), dispatch `pages.yml` from the Actions tab. Never force-push `main`, and never edit the `github-pages` environment by hand; there is nothing there to edit.

## Release links

The primary install location is the public [Visual Studio Marketplace listing](https://marketplace.visualstudio.com/items?itemName=runtrol.runtrol-studio), available for all six supported Windows, macOS, and Linux architecture targets. The static English page exposes that link without JavaScript.

The browser requests the latest `eddmpython/runtrol` GitHub Release and enables the manual button only when at least one `.vsix` asset exists. Platform and architecture hints select the matching native package. When the browser cannot identify a supported target, the button opens the tagged release so the user chooses explicitly. When the request fails, the page keeps the Marketplace path and says no manual VSIX is confirmed. The page never invents a version or links to an artifact that does not exist.

The phone app is published at `/runtrol/app/` under the same origin. The Pages job runs its contract tests before copying its output into the artifact. The Rust audit separately proves that the browser WebCrypto implementation and the production Rust implementation complete pairing and later Noise sessions in both directions, and active gates drive the shipped phone modules through the production daemon and an installed real CLI for session, approval, and remote disconnection recovery.

## Visual direction

The page follows lucide.dev's composition: a light-first surface with one coral accent, large type, generous space, and Lucide icons. Dark mode is a token swap, not a second stylesheet. The header carries the same channel row as the author's other sites (GitHub, support, YouTube, Threads). The Studio window in the hero is always dark, like the editor it depicts, in both themes.

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| Workflow did not run after a push | The push touched none of the trigger paths | Dispatch it manually, or push the site change |
| Workflow failed at "Test site contract" | A contract broke, or a mutation stopped failing | Run `npm --prefix site test` locally; read the assertion message |
| Build fails with "exceeds byte budget" | A large asset entered `site/` or `assets/brand/` | Remove or regenerate the asset; raising the budget is a decision, not a default |
| Icons missing on the page | An icon name not in `icons.js`, or `app.js` threw before mounting | The test names the icon; the browser console names the throw |
| Favicon still orange in the tab | Browser favicon cache | Hard reload, or open the SVG directly once |
| Hero window empty | JavaScript disabled or a module failed to load | The finished state still needs `scene.js`; with scripts blocked the window shows its chrome only, and the text above it carries the message |
| Preview shows 404 for `app/` | Expected; the preview serves the landing only | Build and open `site/dist/app/` |
