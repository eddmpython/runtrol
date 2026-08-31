# Automatic updates

## Ownership

The PC product is the Runtrol Studio VS Code extension. There is no standalone desktop updater or GUI process.

VS Code owns extension delivery. Each platform VSIX contains one matching Core binary. On activation the extension
streams that binary into one extension-global managed location. A changed binary replaces the stable name by content,
while an already running daemon and its provider children keep their existing process identities. The next daemon
start uses the new bytes. Reinstalling an earlier VSIX applies the same mechanism in reverse.

## Studio Marketplace delivery

Studio release facts have separate executable owners:

| Fact | Owner |
|---|---|
| Studio version and tag prefix | [`release-policy.json`](../extensions/runtrol-vscode/release-policy.json) |
| native target and runner matrix | [`release-targets.json`](../extensions/runtrol-vscode/release-targets.json) |
| derived package identity, archive names, current and predecessor tags | [`extension-manifest.mjs`](../extensions/runtrol-vscode/tooling/extension-manifest.mjs) |
| release body projection | [`releaseNotes.py`](../.github/scripts/release/releaseNotes.py) |
| archive and workflow contract | [`vscodePackage.py`](../tests/audit/vscodePackage.py) |
| executable release sequence and recovery behavior | [`vscode-release.yml`](../.github/workflows/vscode-release.yml) |

The automatic route accepts only a successful `gates` run for the exact same-repository `main` push SHA. A release
commit changes only `CHANGELOG.md` and the Studio release policy and must remain in current main history. The workflow
generates and validates one release-message file, uses that same file for the annotated tag, and creates or repairs a
tag-bound draft GitHub Release as durable staging. Prepare records the direct annotated-tag object SHA; automatic,
manual, and final runners require both the local and remote tag object to remain that exact object and its peeled
commit to remain the release SHA. One fixed release concurrency group uses the workflow's bounded FIFO queue instead
of cancellation, so a later Gates success cannot erase a release event that waited behind a long publication. Every
dequeued event still has to pass the exact-SHA and current-main ancestry checks.

Each valid staged VSIX is labeled with the exact release commit SHA. A failed-jobs rerun preserves assets whose name,
label, archive contract, and bytes still agree and rebuilds only missing or invalid targets. Before Marketplace
publication, the workflow downloads the complete draft asset set and audits it again. It then publishes and verifies
the Marketplace packages, installs the public version on every release runner, and only then changes the GitHub
Release from draft to public. Immediately before publication, the final job compares the remote draft bytes and every
live Marketplace `VsixSha256` with the locally audited target archives, then rechecks the release message, state,
asset set, commit labels, and bytes after publication. Failed-jobs retries also require the current tag to remain the
greatest remote semantic Studio release before Marketplace publication and before the final GitHub transition, so an
older event cannot regress `make_latest`. The create route refetches `origin/main` and rechecks release-SHA ancestry
in the same step that makes the GitHub Release public. `publishExisting` performs the same time-of-use ancestry check
and revalidates the GitHub Release body, title, tag, public state, asset-name set, and remote bytes against its local
audited set in the Marketplace publisher step. These final checks remain mandatory even when repository branch
protection is missing or changes during a long run.

VS Code's documented
[extension update mechanism](https://code.visualstudio.com/docs/configure/extensions/extension-marketplace)
checks for updates and installs them automatically for enabled extensions on its own cadence. A user can request an
immediate check with the extension's Update action or the Check for Extension Updates command. A package installed
manually from a VSIX has per-extension automatic updates disabled by VS Code. Such an installation must enable Auto
Update from the extension Manage menu or be reinstalled once from the
[Marketplace listing](https://marketplace.visualstudio.com/items?itemName=runtrol.runtrol-studio). Exact-ID search uses
`@id:runtrol.runtrol-studio`.

Studio does not implement a second updater, contact the Marketplace itself during activation, or modify VS Code's
global update settings. Delivery, signing, update policy, and restart prompts remain owned by VS Code.

The daemon owns provider package updates. A surface can request status or an explicit update, but it never constructs
package-manager arguments and never changes provider files itself.

## Studio release operation

### Prepare and start

1. Finish implementation, documentation, local package inspection, and product journeys before changing the Studio
   version. Release policy is not a progress marker.
2. Promote the current changelog entries into a heading for the exact next Studio patch and leave the Unreleased
   heading ready for later work.
3. Change the Studio release policy to that same patch. Commit only the changelog and release policy, then push the
   exact commit to `main`.
4. Follow the Gates run for that SHA. The successful run triggers `vscode-release` automatically. Do not manually
   upload a package or create a second tag while it is active.

### Observe completion

The release is complete only when the release workflow concludes successfully and both public surfaces agree. The
Marketplace must expose the current Studio version for the entire target catalogue, and the GitHub Release must be
public under the governed annotated tag with its exact asset set. A draft Release is staging, not a published release.
The workflow's public install jobs are the executable proof that Marketplace packages activate with their bundled
Runtime; a package merely appearing in a listing is not sufficient.

### Recover a partial run

- Rerun failed jobs on the same workflow run. Durable draft assets let a retry continue from verified targets even if
  `main` has advanced. The selected release SHA must still be an ancestor of current `main`.
- Do not delete or move the annotated tag, rewrite release history, edit the generated release body, or replace draft
  assets by hand. The next run reconciles invalid or incomplete assets from the executable canons.
- A Marketplace target that was already published is safe to encounter again. Publication and public verification
  converge on the exact same version and bytes.
- Use manual `publishExisting` only when the tagged GitHub Release is already public and its body, state, and complete
  asset set are correct. That route treats the GitHub Release as read-only and retries Marketplace publication without
  rebuilding. It is not a way to repair a draft, invent an asset, or bypass the gated release commit.
- If publication credentials are missing or rejected, repair the Marketplace-only Actions secret and rerun the failed
  jobs. Do not move that credential into the repository or a local release command.

### Roll back a bad public Studio release

Never retarget an existing tag or replace a published asset. Prepare a new patch that restores the last known-good
behavior, pass the same gates, and publish through the same release path. An operator may install a previous VSIX as
local incident containment, but that does not change the public channel and VS Code may keep that manual installation
outside automatic updates until Marketplace installation is restored. Runtime generation pinning keeps existing
provider terminals attached to their exact owner while Studio is upgraded or locally rolled back.

## Provider channel confirmation

An update is executable only when all of these facts agree:

1. The provider manifest compiled into the product names a closed update channel.
2. The resolved provider invocation leads to an exact package entry point.
3. The live global package root contains a bounded package manifest that names that entry point and a semantic version.
4. The registry still publishes the exact installed release and a greater plain semantic release.

Operator manifests cannot claim update authority. Provider identifiers, package names, versions, and installation
paths remain runtime observations. The npm adapter owns the fixed argument shape and accepts only the validated package
name and exact semantic version as data.

An unconfirmed copy is status-only. A provider-owned channel is observe-only. Neither state reaches a package command.

## Scheduling and exclusion

The daemon delays its first automatic check until after extension activation and idle measurement windows, then uses
one bounded repeat cadence. [`serve.rs`](../crates/runtrol-daemon/src/serve.rs) owns those executable intervals and the
defer-warning threshold; this document owns why the work stays off activation and interactive paths.

Before changing a package tree, the single session owner proves that the provider has no live, opening, closing, or
temporarily detached process. The same opaque reservation then blocks new sessions for that provider. A shared
discovery gate also excludes short-lived version, flag, and model probes. Other providers remain available.

If provider processes keep an available update deferred past the owned threshold, the daemon publishes one named
warning to the VS Code session surface. It does not terminate a session to obtain the update window.

## Verification and rollback

The installed release is probed before mutation. A successful target must then satisfy both independent package
ownership and the provider's bounded local version and flag probe. A package-manager success code alone is not health.

Every update transaction is closed:

1. Install the exact greater target.
2. Verify its ownership and local provider probe.
3. On either failure, reinstall the exact release that was installed before step 1.
4. Verify the restored ownership and local provider probe before reporting rollback success.

`RUNTROL_HOME/provider-updates.json` is a bounded, atomically replaced safety journal. It contains only provider
identifiers, semantic version floors, and rollback pins. It cannot contain conversation data. The highest verified
release never moves down. An automatic rollback pins the failed target so the timer cannot oscillate. A later explicit
VS Code update is the only retry path and clears the pin only after success.

Package installation and query operations have separate bounded process deadlines and captured-output limits in
[`provider_update.rs`](../crates/runtrol-daemon/src/provider_update.rs). Cancellation drops the exact child guard and
the provider reservation.

## VS Code surface

`Runtrol: Check Provider Updates` shows every provider as current, available, observe-only, not installed, or
unconfirmed. Selecting an available release requires a modal confirmation. A missing exact rollback blocks the action.
Updated, restored, and failed outcomes are rendered as named VS Code messages. Automatic outcomes enter the same
session warning stream and repeated identical warnings are deduplicated by the extension.

Provider update, rollback, and install authorities are local-only scope variants. No remote device scope can express
them. The phone surface cannot trigger package mutation.

## Verification

| Gate | Contract |
|---|---|
| `versionSsot` | Cargo members and Studio each have one version source, and Studio advances only one `0.1.x` patch at a time |
| `vscodeUpgradeRollback` | official VSIX upgrade and rollback preserve the daemon, provider process, session, and workspace |
| `vscode-release` | exact-SHA annotated tag, recoverable draft staging, Marketplace publication, public install journeys, and final GitHub Release convergence |
| `channelVerdict` | package ownership, safe package identifiers, closed channel arguments, and operator-manifest denial |
| `cliUpdateRehearsal` | a broken fixture target restores the exact starting tree, while an unrestorable target fails closed |
| `scopeWall` | provider update requests are local-only and every request has a boundary rule |
| `configReadOnly` | the safety journal is the reviewed runtrol-owned writer and provider files change only through npm |

The provider rehearsal uses a deterministic fixture in hosted CI. It does not mutate a developer's installed global
packages or claim account-backed provider behavior. That is why the North Star axis remains at the mock evidence tier.
