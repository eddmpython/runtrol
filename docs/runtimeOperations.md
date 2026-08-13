# Runtime operations

## Artifact set

A Runtime release contains standalone Runtime ZIPs for Windows, macOS, and Linux on x64 and arm64. It also contains
the public Rust protocol crate, Rust client crate, TypeScript client package, and a release manifest.

Every Runtime ZIP contains one headless `runtrol` executable, LICENSE, NOTICE, public schema, manifest, checksums, and
per-user install and uninstall scripts. It contains no provider CLI, provider credential, model, conversation, desktop
window, or consumer application.

Before installation, verify GitHub Sigstore artifact provenance and the release manifest. After extraction, verify
every file against `SHA256SUMS`. Refuse a target mismatch, missing artifact, extra executable, checksum mismatch, or
unverified replacement.

## Install and start

Windows PowerShell from the extracted target directory:

```powershell
.\install.ps1
```

macOS or Linux:

```sh
./install.sh
```

Installation is per-user and requires no administrator account. Windows installs versioned binaries under
`%LOCALAPPDATA%\RuntrolRuntime` and Runtime state under `%LOCALAPPDATA%\runtrol`. Unix installs versioned binaries
under `$HOME/.local/share/runtrol` by default and a launcher under `$HOME/.local/bin`. State uses the platform-standard
per-user location.

Installation does not add a system service or open a window. Running an explicit command such as `runtrol endpoint`
starts the headless singleton when needed and prints the private local administration endpoint. Public SDKs do not
start or download Runtime.

## Locator states and repair

The public locator is `runtime.locator.json` below the Runtime state directory. Its normal states are:

| State | Action |
|---|---|
| Missing | Install Runtime or run an explicit installed Runtime command |
| Valid and running | Connect through the SDK and complete instance proof |
| Stale after a verified crash | Stop all Runtrol processes, verify owner and install record, then remove only the stale locator |
| Unsafe owner or permissions | Refuse SDK connection and repair locally before replacement |
| Incompatible revision | Install a signed compatible Runtime or roll back the consumer SDK |

Never copy a locator between machines or users. Never edit its endpoint or instance ID. The next verified Runtime start
publishes a new atomic locator.

## Integration administration

Use these VS Code commands:

| Command | Purpose |
|---|---|
| `Runtrol: Review Integration Requests` | Approve or deny exact pending identities, scopes, and roots |
| `Runtrol: Manage Runtime Integrations` | Review and revoke installed consumer grants |
| `Runtrol: Review Runtime Requests` | Confirm exact session-forget and key-rotation requests |

Revocation stops future Runtime access and retires subscriptions. It does not stop or delete provider-native sessions.
After a consumer key is lost or a grant is revoked, create a new identity and enroll again. Do not silently reuse the
old integration ID.

## Update and rollback

Install a newer attested archive with the same script. The installer writes a versioned executable and atomically
switches the per-user launcher. Previous versioned executables remain available for rollback. A running daemon keeps
using its current verified bytes until it stops, so launcher replacement does not mutate a live process.

Before changing the active daemon, finish the current turn or explicitly interrupt and cool affected sessions in VS
Code. `runtrol panic` stops every supervised provider process and the daemon. Use it only after reviewing active work.
The provider-owned conversation remains with the provider, but an active turn is interrupted.

Rollback by running the installer from the earlier attested target archive. Verify that its protocol revision
inventory overlaps every installed consumer and that its `rollbackSafeStoreSchema` accepts the current state. Release
0.1.1 uses store rollback floor 1. If either check fails, do not activate the old binary. SDKs reconnect through the
new locator and must not resubmit ambiguous mutations or reacquire control silently.

## Uninstall

First review managed sessions and integrations in VS Code. Revoke consumers that should no longer authenticate.
Finish, interrupt, or cool active work according to operator intent. Stop the daemon with `runtrol panic`, then confirm
that `runtime.locator.json` is absent.

Run the uninstall script from the same verified archive:

```powershell
.\uninstall.ps1
```

```sh
./uninstall.sh
```

The script refuses while a Runtime locator exists. It removes the installed Runtime executable, launcher, locator,
grants, cache, install record, and Runtime-owned metadata. It does not inspect or remove provider installations,
provider authentication, or provider-owned conversations. An external client then receives `runtimeNotInstalled` and
must not recreate Runtime without user intent.

Successful uninstall ends with one JSON object whose `status` is `removed`, `runtimeOwnedStateRemoved` is true, and
`providerStateTouched` is false. Automation must require that exact result and a zero exit code.

After uninstall, verify the product and state roots are absent and resume any retained conversation through its
provider CLI if needed. Reinstallation creates a new Runtime instance and requires honest integration re-enrollment.

## Failure handling

| Symptom | Safe response |
|---|---|
| Installer checksum failure | Stop and obtain the exact attested archive again |
| Locator remains after a crash | Verify no Runtime process owns it before removing only that file |
| SDK says `protocolIncompatible` | Compare revision inventories and choose a signed compatible update or rollback |
| SDK says `integrationRevoked` | Remove obsolete consumer credentials and enroll a new identity with user intent |
| Update leaves old daemon active | Complete work, stop the verified daemon, and issue a new explicit Runtime command |
| Uninstaller reports an existing locator | Do not force deletion. Review sessions, stop Runtime, and retry |
