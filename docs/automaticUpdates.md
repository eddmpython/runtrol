# Automatic updates

## Ownership

The PC product is the Runtrol Studio VS Code extension. There is no standalone desktop updater or GUI process.

VS Code owns extension delivery. Each platform VSIX contains one matching Core binary. On activation the extension
streams that binary into one extension-global managed location. A changed binary replaces the stable name by content,
while an already running daemon and its provider children keep their existing process identities. The next daemon
start uses the new bytes. Reinstalling an earlier VSIX applies the same mechanism in reverse.

The daemon owns provider package updates. A surface can request status or an explicit update, but it never constructs
package-manager arguments and never changes provider files itself.

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

The first automatic check starts five minutes after the daemon begins, outside extension activation and idle
measurement windows. Later checks run hourly.

Before changing a package tree, the single session owner proves that the provider has no live, opening, closing, or
temporarily detached process. The same opaque reservation then blocks new sessions for that provider. A shared
discovery gate also excludes short-lived version, flag, and model probes. Other providers remain available.

If provider processes keep an available update deferred for 24 hours, the daemon publishes one named warning to the
VS Code session surface. It does not terminate a session to obtain the update window.

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

Package installation has a five-minute process deadline. Query operations have a 30-second deadline and bounded
captured output. Cancellation drops the exact child guard and the provider reservation.

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
| `versionSsot` | Cargo and VS Code packages derive one release version |
| `vscodeUpgradeRollback` | official VSIX upgrade and rollback preserve the daemon, provider process, session, and workspace |
| `channelVerdict` | package ownership, safe package identifiers, closed channel arguments, and operator-manifest denial |
| `cliUpdateRehearsal` | a broken fixture target restores the exact starting tree, while an unrestorable target fails closed |
| `scopeWall` | provider update requests are local-only and every request has a boundary rule |
| `configReadOnly` | the safety journal is the reviewed runtrol-owned writer and provider files change only through npm |

The provider rehearsal uses a deterministic fixture in hosted CI. It does not mutate a developer's installed global
packages or claim account-backed provider behavior. That is why the North Star axis remains at the mock evidence tier.
