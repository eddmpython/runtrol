# runtrol

> [!IMPORTANT]
> **Top-level product rule: the Runtrol sidebar alone must show and manage every connected coding-agent CLI on this PC, top-level projects, actual conversation names, runtime state, time, and usage without a click. A release is blocked if any of this is hidden behind another tab, a collapsed view, repeated labels, or a false hierarchy.**

**Run every project, session, and agent instantly from one VS Code window.**

[한국어](README.md) | English | [中文](README_ZH.md) | [日本語](README_JA.md)

> Status: **the Core and primary VS Code extension are implemented, and `Runtrol Studio 0.1.15` is public for six native targets.** The live session index, real Extension Host and 3,000-frame-per-second Webview performance ratchets, full journey with an installed real CLI, clean Marketplace installation, and session-preserving VSIX upgrade and rollback are verified. The standalone desktop GUI code and execution path have been removed, and the VS Code extension is the only PC surface. The public Runtime protocol, Rust and TypeScript SDKs, external packed-consumer gates, and signed six-target standalone Runtime release pipeline are also implemented. Automatic updates for confirmed provider channels now include process exclusion and exact rollback. Fleet Compare sends one reviewed instruction to two through four isolated worktrees and provider sessions, arranges their native conversations as a VS Code grid, compares passing Artifacts in native diffs, and verifies only one selected passing Receipt. A live two-CLI gate and a real Extension Host eye pass verify that flow. For an ordinary Mission, one `Continue Reviewed Mission` action starts the current safe wave, seals finished Tasks with fixed Gates, prepares the next DAG wave, and sends the exact reviewed instructions. A real installed-CLI and Extension Host journey verifies two successive waves. The [Marketplace extension](https://marketplace.visualstudio.com/items?itemName=runtrol.runtrol-studio), [GitHub Pages site](https://eddmpython.github.io/runtrol/), relay-based phone PWA, bodyless Web Push, and Mission supervision surface are implemented. The answering model and the permission mode switch mid-conversation from their own header chips; each pick is relayed through that CLI's own switch surface and the chip shows only what the service answered (the installed real Claude Code journey gate verifies the switch and the restore through the CLI's own announcements). Every folder with conversations is a top-level project heading, with only the actual conversation name, core state, and time beneath it. A paired device's authority is bounded by its approved workspace roots: the session index, every session command, and Mission reads all pass the same live-root verification, and revoking a root takes effect immediately. Provider preparation runs in per-provider lanes, so five cold first meetings finish in 8.7 seconds where full serialization costs 18.1, and a newly opened folder's existing conversations arrive without a refresh. Each conversation opens as its own editor tab, so several stay on screen at once, split like files, and typing or interrupting targets the conversation of that tab. After an extension update the still-running older Core rolls itself to the new build the moment the machine is idle, so updates actually arrive without a restart. Active gates now run the shipped PWA modules through the production daemon and an installed real CLI for session, approval, and remote disconnection recovery journeys. One click on a project heading enables Agent Tools, letting installed coding agents delegate through seven root-bound public Runtime tools; disabling removes provider registrations, Runtime authority, and protected local credentials. Physical iOS installation and Web Push operation remain unverified contributor operator evidence and are excluded from the current completion scope and score. Most scores below are 0 because no gate asserts those axes yet, not because there is
> no code.
>
> `Continue Ready Missions` reviews up to eight exact Mission digests once and advances the current safe wave across
> projects. A real Extension Host started two separate Git projects in one action and moved both to `integrating` in
> the next action.
>
> `Review and Apply Mission Landing` compares every passing Receipt Artifact from an ordinary Mission with the current
> project in one native VS Code multi-diff. One `Apply, run Gates and complete` action rechecks the Mission, Receipts,
> source and target bytes, links, and unsaved editors, applies the exact reviewed bytes to existing and new files, then
> runs fixed Core Gates. A real Extension Host applied four Artifacts across two Git projects, rejected both project
> and Receipt drift, recovered, kept the second Mission waiting while the first completed, and opened the next Landing.
>
> Fleet Compare no longer stops at comparison. Selecting one passing Task opens a native winner multi-diff containing
> only that Task and Receipt. One public apply action writes the exact bytes without mixing another candidate, runs the
> fixed Gates, and completes through Core. Core retains the selected Task and Receipt as durable terminal evidence, so
> response-loss recovery cannot mistake a different candidate for success. A real Extension Host journey applied only
> `attempt-2` from two distinct real CLI results and reached `completed`, with every target screen inspected directly.
>
> `Mission Auto Flight` arms a reviewed ordinary Mission once on the PC, proves each real provider turn through its
> lifecycle generation, seals fixed Gates and Receipts, and starts the next safe DAG wave automatically. It waits for
> person or quota input and pause, and disarms on authority drift, ambiguous delivery, or recovery. A real two-wave
> CLI journey reached `integrating` with zero operator Continue actions, removed its own authority, and had all three
> screen states inspected directly. Final Receipt Landing and integration always remain explicit.
>
> A phone notification now opens the first session that is actually waiting for the operator without carrying
> conversation content. The `Needs you` count and next action include person waits only and keep account limits
> separate. The real CLI approval gate verifies entry while pending and removal after the answer.
>
> Auto Flight person waits, safe stops, and Receipt Landing use the same content-free wake. After authentication, the
> phone reads at most 64 structural Core signals and opens only the exact session or Mission whose root, Mission
> digest, and current state still match. Push contains no Mission ID, instruction, path, or output, and the phone
> retains only an opaque cursor.

The security boundary and default-deny settings are documented in [SECURITY.md](SECURITY.md).

## North Star

**runtrol turns one VS Code window into the control plane for every project, supported installed coding-agent CLI,
and provider-owned session. Each agent changes its bound repository autonomously. runtrol keeps sessions alive,
isolates concurrent work, and connects the selected session to its exact workspace or worktree without interpreting
the conversation. Session and agent counts may grow while renderers, active subscriptions, and Code-hot workspaces
remain bounded. Streaming and background work must never make typing, scrolling, session switching, or file
navigation stutter. Installed CLIs, models, and capabilities are discovered at runtime. Conversations travel only
between the user's PC and the provider. runtrol does not get in between.**

### Immutable core

- **Features and speed are one contract.** More capability never excuses waiting or stutter. Visible latency, frame drops, and input delay block a release.
- **Multisession cost does not scale with session count.** Fifteen sessions are the daily-use baseline and 30 are the release-gate load. More logical sessions may exist, but at most eight own a hot process and there is exactly one active renderer and one full stream. Selection pinning, instant search, stable ordering, and workspace switching use the same interaction at 30 sessions.
- **Multi-agent operation is provider-neutral.** Supported installed CLIs are discovered automatically and use one list and one interaction model. A new provider requires a manifest or driver, never a core edit.
- **Agents change repositories autonomously.** The provider CLI owns the work and conversation. runtrol supervises only session, workspace, worktree, process lifecycle, and collision boundaries.
- **Conversation selection is bound to workspace switching.** Selecting a session changes conversation and file context immediately. The exact workspace or worktree becomes Code-hot only when editing requires it. runtrol never reads conversation text to guess a path.
- **Device connectivity is separate from session ownership.** VS Code and the phone are paired surfaces of the same Core, and neither owns a session. The Core keeps sessions alive across window, device, and network-path changes. An existing private network such as Tailscale may be discovered as a direct route, but pairing, push, and correctness never depend on it.
- **The human is always first.** Typing, scrolling, the editor, and file navigation stay responsive during long streams, multiple agents, builds, and tests.
- **The thin boundary never changes.** runtrol owns no provider account credential, transcript, model API key, or conversation copy.

The current total is **74/140, average 5.3/10**. Thirteen axes have active CI gates.
A 10 means the complete journey has been repeatedly verified in a real environment.
**A score above the manual tier is backed by a gate that actually runs in CI. A path that is not executed automatically cannot pass 3, no matter how implemented it looks.**

| North Star | Score | Today | Target state |
|---|---:|---|---|
| One session list | 5/10 | Hosted CI drives a real VS Code Extension Host through start, two workspace switches, exact selection restoration, reconnect, interrupt, and close. The counterpart is a deterministic loopback model, so this remains the mock tier. | Whether the provider is Claude Code, Codex, or whatever comes next, every session alive on this PC appears in one list, and start, resume, and delete all happen there. |
| Instant response | 5/10 | A real VS Code Extension Host measures the production bundle. One ratchet covers a real 30-session list, at most eight hot ACP processes, provider-native cold resume, 3,000 raw frames per second, session switches through Core watch acknowledgement and Webview paint, and exact selection restoration after the workspace changes. The transport counterpart is a mock, so the axis remains at this tier. | The list appears with no wait, a conversation opens the moment it is tapped, and neither scrolling nor typing stutters when long output pours in. There is no moment where the user perceives loading. |
| Reach my PC sessions from my phone | 5/10 | Hosted CI runs the shipped PWA WebCrypto, Noise, and CoreClient modules in a headless phone process, starts an installed real Claude Code session through the production daemon, sends a prompt, watches output, and closes it. The deterministic loopback model counterpart keeps this at the mock tier. | Pair the phone to the PC once, and from then on, away from the desk, send new instructions into sessions running on that PC and watch the output live. Neither the plan tier nor the auth method of a provider account blocks this. |
| Provider extensibility | 5/10 | Hosted CI checks the public outside-driver contract, a generic ACP fixture on three operating systems, two turns plus native load through an independently distributed ACP implementation, and the hidden approval denial round trip of real Claude Code. The model endpoints are local mocks. Scheduled CI repeats the parser probes and approval journey with current CLIs. It does not claim account-backed model behavior or the complete event surface. | When a new CLI appears, one adapter is added and the PC screen, the phone screen, and the controls stay the same. The user notices a new provider only as a longer list. |
| No conversation passthrough | 6/10 | An exact egress allowlist on real loopback sockets and the production Noise IK and IKpsk1 boundary run in `egressContract`. A prompt sample never appears in plaintext in the relay capture or diagnostics, transport has no disk or logging API, and drivers and storage know no provider transcript path. The ceiling is 6 until a live phone and relay gate exists. | The user's prompts and the model's responses travel only between the PC and the provider, and between the user's own devices. runtrol stores no copy of that content, and no server in between ever receives it in a readable form. |
| Approve from the phone | 5/10 | An active gate receives a real Claude Code hidden Write approval over the PWA watch path, verifies the complete subject, unique `rejectOnce`, and 32-byte digest, then observes the same provider turn resume and end. The deterministic loopback model counterpart keeps this at the mock tier. | When an agent stops in front of a dangerous action, it appears on the phone, and allowing or denying there resumes the PC session immediately. |
| Survive disconnection | 5/10 | Shipped PWA modules and an installed real CLI replay from the exact cursor after a network cut, then continue through an explicit gap and native resume after a Core restart. The model counterpart is a mock. | The PC session remains recoverable through the official resume surface when the phone locks, the network drops, or runtrol restarts. Retained frames continue from an exact cursor; anything outside the bounded window is an explicit gap, never a silent skip. |
| Cost of running | 6/10 | All three hosted operating systems measure the real debug daemon's idle RSS and ten-second idle CPU against one ratchet. There is no second independent evidence kind, so the ceiling remains 6. | Leave it on all day and the user never notices it is there. Not in the battery, not in the fan, not in the task manager. |
| Same method everywhere | 8/10 | Active hosted CI installs the exact native VSIX into clean VS Code, discovers the bundled Core with no configured path, opens Runtrol, and opens then closes a `Runtrol: New chat` composer through the public command. The same gate runs on Windows, macOS, and Linux, while the release matrix repeats it for all six x64 and ARM64 targets. The claim is limited to a static contract, a real journey, and multi-OS evidence. | Install and operation are the same on Windows, macOS, and Linux. A Windows user never needs to know what WSL or tmux is. |
| Current without asking | 5/10 | `vscodeUpgradeRollback` verifies session continuity through VSIX and Core replacement on three operating systems. `cliUpdateRehearsal` verifies confirmed provider update failure, exact restoration, and oscillation prevention with a deterministic fixture. Hosted CI does not mutate account-backed provider installations, so the evidence remains mock tier. | The app and the installed agent CLIs stay current on their own, and if an update breaks a session it has already rolled back before the user touches anything. There is no moment where the user thinks about versions. |
| Automatic model detection | 6/10 | Hosted `modelDetectionSmoke --require-all` installs current real CLIs without credentials, checks Codex `model/list` and a Claude partial catalogue containing an isolated provider-owned option-cache sentinel, and rejects observed identifiers hardcoded in production source. It does not prove availability for a particular account, so one live gate kind caps the score at 6. | The models this account can actually use appear in the list as they are, and a new model appears without runtrol being changed. |
| Sessions do not trample each other | 5/10 | Real Git metadata and the production Core admission path treat subdirectories in one worktree as one writer and atomically reject overlapping opening, live, and closing reservations. Linked worktrees and operator-explicit shared starts remain distinct. The provider is a fixture, so this is the mock tier. | Which session is touching which folder is always distinguishable, starting a second session into the same folder warns before it happens, and when a provider offers isolation (worktrees) it is available right on the start screen. |
| Agents consult each other | 3/10 | The toggle wires, verifies, and restores the two real CLIs through their own commands, and a real mid-turn consultation was received and measured by hand (2026-08-03). `crossConsultSmoke` drives the real subscription CLIs, so it runs on the operator's machine; with no hosted CI gate the tier is manual. | One toggle registers two CLIs with each other over their official surface (MCP), so one agent asks another for an opinion mid-turn and gets it back. The wiring is made only through each CLI's own official commands (no direct config-file writes), conversation bodies still never pass through runtrol, and the user never needs to learn what MCP is. |
| Freedom to leave | 5/10 | `uninstallLeavesNoTrace` completes a turn with provider state outside the runtrol home, removes the entire home, then loads the same native session under a new daemon and completes a second turn. The counterpart is an ACP fixture, so this is the mock tier. | Delete runtrol and the sessions and history remain each CLI's own, continuing the original way. There is no data runtrol holds hostage. |

Which gate backs which axis is defined in [docs/northStarEvidence.md](docs/northStarEvidence.md).

### Scoring rubric

A score is not a rung somebody picks. It is computed as **a base tier plus additives**. The source
is [tests/audit/northStar/board.toml](tests/audit/northStar/board.toml), the
`northStarBoard` gate computes it, and the `readmeParity` gate holds all four README languages to
what it computed.

**Base tier.** Exactly one holds per axis, and it is a ceiling.

| Base tier | Score | What has to be true |
|---|---:|---|
| `none` | 0 | No gate asserts this axis |
| `manual` | 3 | Someone watched it work by hand. No gate is active in hosted CI. Demo videos, screenshots, and "I ran it and it worked" all land here |
| `mock` | 5 | A registered gate runs, but against fakes. Mock CLI, stub provider, simulated phone |
| `realOneKind` | 6 | It runs against the real counterpart, but only one kind of gate exists: static (`contract`) or live (`smoke`, `bench`) |
| `realBothKinds` | 7 | The real counterpart, with a static gate and a live gate both registered |

**Additives.** They attach only at `realBothKinds`, each one needs a gate of a matching kind, and
holding all four lands exactly on 10.

| Additive | Score | What has to be true |
|---|---:|---|
| `multiProvider` | +1 | The same gate is green against two or more providers |
| `multiOs` | +1 | The same gate is green on two or more operating systems, Windows included |
| `faultInjection` | +0.5 | The gate carries fault injection (kill the daemon, cut the network) and stays green |
| `ratchet` | +0.5 | A regression ratchet goes red the moment the measured number gets worse |

Rules that keep the score from inflating:

1. **However finished the implementation looks, without a gate a runner actually invokes the ceiling is `manual` (3).** No exceptions.
2. `operator` gates, the ones needing a real device or a real account, are **excluded from the total**.
3. A pull request that raises a score includes **the gate name and a link to the CI run**, and edits `board.toml` in the same commit. Prose is not a score.
4. Scores move in steps of 0.5. A number like 8.7 is not precision, it is self deception.
5. **Nothing prevents an axis from going down.** If a provider changes its surface and a gate goes red, the score goes down. This table is today's state, not yesterday's boast.
6. **A ceiling is set by a missing kind of gate, not by a missing run.** An axis with only one kind cannot pass 6 however green it runs. Thirteen of the fourteen axes are in that state today, and `northStarBoard` prints each ceiling next to its score.

### What gets a score and what does not

Three layers, never mixed. Mixing them is how a total goes up while the user receives nothing.

| Layer | What goes in it | How it shows |
|---|---|---|
| **Scored axes** | Outcomes a user can feel (the fourteen above) | 0 through 10, summed to /140 |
| **Floor gates** | Modularity, clean code, security, hygiene, budget | **Not a score.** Green or red, and red does not merge |
| **Kill criteria** | Innovation, positioning | **No number.** Decided only by the kill criteria in [docs/positioning.md](docs/positioning.md) |

- **Why modularity and clean code get no partial credit.** They are floor rules. "Clean code 7/10" means "being broken by 3", which is not a score, it is red. They get finer by being split into named gates instead (`dependencyDirection`, `providerIsolation`, `checkSilentFail`, `cargoClippy`, and the rest). The full list is in [docs/northStarEvidence.md](docs/northStarEvidence.md).
- **Why innovation gets no number.** The innovation is the fourteen axes themselves ("manage several AI agents in one place"). A separate score would count the same thing twice, and no gate can assert it, which is rule 3. Whether the innovation still holds is what the kill criteria decide.

## First principle: user convenience

At every fork, take the side that is easier for the user. The test is not taste. It is **the number of actions the user actually performs and the time they spend waiting**.

- If the user must configure something that should just work, that is a failure
- If the user can see themselves waiting, that is a failure
- If the user must learn a concept (tmux, WSL, tunnels, port forwarding, installing certificates), that is a failure
- If the user does the same thing twice, that is a failure
- **Stutter is a bug, not an optimization target**

## Get it

| | |
|---|---|
| **PC (Windows, macOS, Linux)** | Install [`Runtrol Studio` from the VS Code Marketplace](https://marketplace.visualstudio.com/items?itemName=runtrol.runtrol-studio). x64 and ARM64 are supported. No separate desktop application is distributed |
| **Mobile** | [Phone PWA at the permanent GitHub Pages origin](https://eddmpython.github.io/runtrol/app/). Pair first from the one-use QR in VS Code |

Public release `0.1.15` and all six platform VSIX packages are also available from [GitHub Releases](https://github.com/eddmpython/runtrol/releases/tag/vscode-v0.1.15).
VS Code automatically updates Marketplace installations. If an older version was installed directly from a VSIX, reinstall it once from the Marketplace because VS Code disables automatic updates for manual VSIX installations.

## Let agents use Runtrol

Select the sparkle on a project heading and choose **Enable Agent Tools for This Project**. Installed coding agents
can discover providers and models, start project sessions, send instructions, read events, and stop exact sessions.
The project row says `Agent Tools` when ready.

Authority is bound to that one canonical project root. There is no approval answering, conversation deletion, silent
shared start, API key, transcript copy, or Runtrol-owned agent loop. **Disable Agent Tools for This Project** removes
Runtime authority and the OS-protected credential, plus provider registrations when it is the last enabled project.
The exact contract is in [Agent Tools](docs/agentTools.md).

## Who does not need runtrol

**If you use exactly one provider, that provider's own remote control is better. This is stated first.**

For someone who only uses Claude Code, `claude --remote-control` is better. The people who built it built it,
it is bundled free, it has native push, and it is in the app store.
Anthropic, OpenAI, GitHub, and Amp have all shipped their own remote control. If that is enough, use it.

**runtrol is for the person whose list is split across four of them.**
A Codex session will never appear in the Claude app. That is not a feature gap, it is structural, and no provider has a reason to fix it.

## What runtrol is not

- **Not a chat client.** Rendering the conversation is something each CLI already does. runtrol moves that output without interpreting it.
- **Not a model proxy.** It does not call model APIs, read tokens, or relay requests. That is not a design preference, it is a survival condition.
- **Not an IDE.** Showing a diff is the boundary. Editing one is outside it.
- **Not its own agent framework.** Runtrol owns no planner or autonomous loop. It offers bounded Runtime tools to provider-owned agent loops without becoming one.
- **Not a hosted service.** No accounts, no logins, no plans.
- **Not a terminal multiplexer.** The goal is not to replace tmux but to **not require it**.

## Why Rust

Honestly, **Rust by itself is not a differentiator.** More than ten competitors in this space are already Rust.
Rust is not the goal; it is the means to three axes in the table above.

- **Same method everywhere**: handling ConPTY and POSIX behind one abstraction directly makes Windows first class without tmux.
- **Cost of running**: this is a daemon left on all day. A single static binary with no runtime means neither Node nor Python has to be installed.
- **Instant response**: a list and a conversation that open with no wait require the absence of GC pauses and runtime startup.

If those axes are not nailed down by gates, using Rust means nothing.

## Layout

| | | |
|---|---|---|
| `crates/` | The product core (Rust). Daemon, provider adapters, and transport. There is no standalone GUI crate | Implemented |
| [`clients/typescript/`](clients/typescript/) | Public Runtime TypeScript SDK for external products | Packed consumer verified |
| [`extensions/runtrol-vscode/`](extensions/runtrol-vscode/) | The only PC surface, `Runtrol Studio` | 30-session release load verified, 0.1.15 public |
| [`pwa/`](pwa/) | Mobile PWA | Relay connection, session control, approval, and exact `Needs you` and Mission Flight Signals focus implemented |
| [`site/`](site/) | [Dependency-free GitHub Pages landing](https://eddmpython.github.io/runtrol/) | Live |
| [`assets/brand/`](assets/brand/) | The logo. SVG is the source; favicons, icons, and social cards derive from it | |
| [`docs/`](docs/README.md) | Operational documentation, source of truth | |
| [`tests/audit/`](tests/audit/) | Contract gates | |
| [`tests/audit/northStar/`](tests/audit/northStar/) | The scoreboard engine. Computes the numbers in the table above and holds the four languages to them | |

## Development

```bash
python -X utf8 tests/audit/preflight.py          # full local CI
python -X utf8 tests/audit/preflight.py lint     # lint only
python -X utf8 tests/audit/preflight.py --list   # what runs and what is skipped
git config core.hooksPath .githooks              # once per clone
```

A gate is **a defect detector, not a rubber stamp**. When a new gate goes up, before looking at it pass,
plant the defect it is supposed to catch and confirm it goes red
(`python -X utf8 tests/audit/checkSilentFail.py --selftest` is that shape).

See [CONTRIBUTING.md](CONTRIBUTING.md) to contribute. Design-stage contributions are real contributions.

## License

The product is [AGPL-3.0-only](LICENSE). The published client packages
(`runtrol-runtime-protocol`, `runtrol-runtime-client`, `@runtrol/runtime-client`) exist for
other programs to link against, so they are [Apache-2.0](crates/runtrol-runtime-protocol/LICENSE).

Running runtrol places no license obligation on your own work. It supervises agent CLIs as
separate processes and is not linked into anything you write.
