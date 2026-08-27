# Runtime operations

## Artifact set

A Runtime release contains standalone Runtime ZIPs for Windows, macOS, and Linux on x64 and arm64. It also contains
the public Rust protocol crate, Rust client crate, TypeScript client package, six CPython 3.11 stable-ABI Python
wheels, and a release manifest. Python source distributions are refused so installation never falls back to an
unreviewed local native build.

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

The public locator is `runtime.locator.json` below the Runtime state directory. It lists every daemon generation
serving the home; `runtrol status` prints that list and whether each generation still answers. Its normal states are:

| State | Action |
|---|---|
| Missing | Install Runtime or run an explicit installed Runtime command |
| One current generation | Connect through the SDK and complete instance proof |
| Two generations, one draining | An update is taking over; the draining one finishes its running turns and exits by itself |
| An entry that no longer answers | Left by a crash; the next generation to start removes it |
| Unsafe owner or permissions | Refuse SDK connection and repair locally before replacement |
| Incompatible revision | Install a signed compatible Runtime or roll back the consumer SDK |

Never copy a locator between machines or users. Never edit an endpoint or the instance ID. Each generation writes
only its own entry, atomically, under the home's lock.

## Integration administration

Runtime administration does not require Studio. Use the installed `runtrol` executable in an attached local terminal:

| Command | Purpose |
|---|---|
| `runtrol integrations list [--json]` | List active and revoked integrations with exact scopes, roots, and generations |
| `runtrol integrations review <pending-id>` | Approve a narrowed scope and root subset, deny, or cancel an enrollment |
| `runtrol integrations revoke <integration-id>` | Revoke one exact installed consumer grant |
| `runtrol requests review <pending-id>` | Confirm or deny an exact session-forget, key-rotation, or shared-writer request |
| `runtrol providers help <provider-id>` | Show the provider's discovered installation or repair guidance |

Authority-changing commands refuse piped input and command-line approval flags. They display the complete bounded
subject and require an interactive decision plus exact identifier retyping. Studio exposes equivalent optional GUI
commands named **Review Integration Requests**, **Manage Runtime Integrations**, and **Review Runtime Requests**.

Revocation stops future Runtime access and retires subscriptions. It does not stop or delete provider-native sessions.
After a consumer key is lost or a grant is revoked, create a new identity and enroll again. Do not silently reuse the
old integration ID.

## Update and rollback

Install a newer attested archive with the same script. The installer writes a versioned executable and atomically
switches the per-user launcher. Previous versioned executables remain available for rollback. A running daemon keeps
using its current verified bytes until it stops, so launcher replacement does not mutate a live process.

Nothing has to be finished before the new build serves. The first command run from it starts a new generation
beside the running daemon; the running daemon hands over the store, stops taking new conversations, keeps serving
the turns already running, and exits by itself when none is left. `runtrol status` shows both while both live.
`runtrol panic` still stops every supervised provider process and the daemon at once; use it only after reviewing
active work, because an active turn is interrupted.

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
