# Site Deployment

The public site is live at [https://eddmpython.github.io/runtrol/](https://eddmpython.github.io/runtrol/). GitHub Pages is configured with `build_type: workflow` and enforced HTTPS.

## Source and build

[`site/`](../site/) owns the landing source and [`pwa/`](../pwa/) owns the phone application source. Both use static HTML, CSS, and browser JavaScript with no package dependencies and no CDN assets. `npm --prefix site run build` creates the ignored `site/dist` directory from both reviewed surfaces plus canonical files under [`assets/brand/`](../assets/brand/).

The build deletes only generated `site/dist` and `pwa/dist`, creates `.nojekyll`, and rejects a complete output larger than 250,000 bytes. The first public deployment was 77,429 bytes. The landing plus phone application currently remain below that same budget. Main content, product status, and installation instructions remain available without JavaScript.

## Contract test

`npm --prefix site test` checks:

- English is the static default;
- Korean, Chinese, and Japanese dictionaries exist;
- the 30-session North Star is visible in static HTML;
- the live Marketplace and phone PWA routes remain statically available;
- manual installation selects the matching native `.vsix` asset from the latest GitHub Release;
- there is no hardcoded release version or external style, font, or script CDN;
- forbidden dash characters do not enter site source.

The test also injects five defects and requires every mutation to fail before the valid source passes.

## GitHub Pages workflow

[`pages.yml`](../.github/workflows/pages.yml) runs on relevant pushes to `main` and on manual dispatch. The build job has only repository content read and Pages read permissions. The deploy job receives only Pages write and OIDC token permissions. Every external action is pinned to a full commit digest.

The job order is test, build, configure, upload, then deploy into the `github-pages` environment. The first successful public run was `31545875267` at commit `59625bb`. Run `31557512027` deployed the live Marketplace actions and target-aware manual fallback at commit `1bf19d1`.

## Release links

The primary extension install location is the public [Visual Studio Marketplace listing](https://marketplace.visualstudio.com/items?itemName=runtrol.runtrol-studio). Version 0.1.0 is available for all six supported Windows, macOS, and Linux architecture targets. The static English page exposes a working install action without requiring JavaScript.

The browser requests the latest `eddmpython/runtrol` GitHub Release and enables the manual button only when at least one `.vsix` asset exists. Runtime platform and architecture hints select the matching native package. When the browser cannot identify a supported target, the button opens the tagged release so the user can choose explicitly. The page never invents a version or links to a build artifact that does not exist.

The phone application is published at `/runtrol/app/` under the same Pages origin. The Pages job runs its JavaScript contract tests before copying the PWA output into the site artifact. The Rust audit separately proves that the browser WebCrypto implementation and production Rust implementation complete both pairing and later Noise sessions in both directions, including consecutive transport records whose nonces are greater than zero. Separate active gates drive the shipped phone modules through the production daemon and an installed real CLI for session, approval, and remote disconnection recovery journeys.

## Visual direction

The page combines eddmpython's restrained carbon and ivory composition, xlpod's compact channel rail, and pyproc's product-like technical panel. The content-heavy codaro structure is intentionally not used for this single-page open source tool. The canonical runtrol mark is reused without modifications, effects, or alternate geometry.
