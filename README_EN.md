# runtrol

**Manage every AI from one place.**

[한국어](README.md) | English | [中文](README_ZH.md) | [日本語](README_JA.md)

> Status: **the core is up.** Sessions from both providers appear in one list, and start, close and resume run
> against the real CLIs. Most scores below are 0 because no gate asserts those axes yet, not because there is
> no code.

## North Star

**runtrol helps developers who run several coding agent CLIs, such as Claude Code and Codex,
open, continue, and approve all of them from a single list.
At the desk it is an app; away from the desk it is a phone. The same session, the same way.
However many providers there are, there is one list. Whatever the operating system, the method is the same.
The conversation travels only between the user's PC and the provider. runtrol does not get in between.**

The current total is **19/140, average 1.4/10**. Three axes have gates standing behind them so far.
A 10 means the complete journey has been repeatedly verified in a real environment.
**A score is backed by a gate that actually runs in CI. A path that is not executed automatically does not count, no matter how implemented it looks.**

| North Star | Score | Today | Target state |
|---|---:|---|---|
| One session list | 6/10 | Sessions from two real CLIs appear in one list, start and close work, and the listing carries the name a resume takes (`sessionLifecycleSmoke`, no tokens spent). A resume that succeeds is still outside the gate: it needs a conversation with a turn in it. The ceiling is 6 because no static contract gate backs this axis. | Whether the provider is Claude Code, Codex, or whatever comes next, every session alive on this PC appears in one list, and start, resume, and delete all happen there. |
| Instant response | 0/10 | Not built. | The list appears with no wait, a conversation opens the moment it is tapped, and neither scrolling nor typing stutters when long output pours in. There is no moment where the user perceives loading. |
| Reach my PC sessions from my phone | 0/10 | Not built. | Pair the phone to the PC once, and from then on, away from the desk, send new instructions into sessions running on that PC and watch the output live. Neither the plan tier nor the auth method of a provider account blocks this. |
| Provider extensibility | 7/10 | An outside driver compiles against the public traits alone (`providerContract`), and the current schemas and argument parsers of both installed CLIs are checked against the binding lists (`agentSurfaceDrift`). A live proof that one TOML file adds a generic ACP provider remains. | When a new CLI appears, one adapter is added and the PC screen, the phone screen, and the controls stay the same. The user notices a new provider only as a longer list. |
| No conversation passthrough | 0/10 | Not built. | The user's prompts and the model's responses travel only between the PC and the provider, and between the user's own devices. runtrol stores no copy of that content, and no server in between ever receives it in a readable form. |
| Approve from the phone | 0/10 | Not built. | When an agent stops in front of a dangerous action, it appears on the phone, and allowing or denying there resumes the PC session immediately. |
| Survive disconnection | 0/10 | Not built. | The PC session does not die when the phone locks, the network drops, or runtrol restarts, and on return the output from that interval continues without a gap. |
| Cost of running | 0/10 | Not built. | Leave it on all day and the user never notices it is there. Not in the battery, not in the fan, not in the task manager. |
| Same method everywhere | 0/10 | Not built. | Install and operation are the same on Windows, macOS, and Linux. A Windows user never needs to know what WSL or tmux is. |
| Current without asking | 0/10 | Not built. | The app and the installed agent CLIs stay current on their own, and if an update breaks a session it has already rolled back before the user touches anything. There is no moment where the user thinks about versions. |
| Automatic model detection | 6/10 | Every preflight asks two real CLIs. An enumerable CLI returns the current account's models, while a CLI that cannot enumerate returns stable manifest aliases and states that limitation (`modelDetectionSmoke`, no tokens spent). The gate also checks that discovered model identifiers are not source literals outside drivers. The ceiling is 6 because no separate static contract gate backs this axis. | The models this account can actually use appear in the list as they are, and a new model appears without runtrol being changed. |
| Sessions do not trample each other | 0/10 | Not built. | Which session is touching which folder is always distinguishable, starting a second session into the same folder warns before it happens, and when a provider offers isolation (worktrees) it is available right on the start screen. |
| Agents consult each other | 0/10 | Not built. | One toggle registers two CLIs with each other over their official surface (MCP), so one agent asks another for an opinion mid-turn and gets it back. The wiring is made only through each CLI's own official commands (no direct config-file writes), conversation bodies still never pass through runtrol, and the user never needs to learn what MCP is. |
| Freedom to leave | 0/10 | Not built. | Delete runtrol and the sessions and history remain each CLI's own, continuing the original way. There is no data runtrol holds hostage. |

Which gate backs which axis is defined in [docs/northStarEvidence.md](docs/northStarEvidence.md).

### Scoring rubric

A score is not a rung somebody picks. It is computed as **a base tier plus additives, pushed down by
caps**. The source is [tests/audit/northStar/board.toml](tests/audit/northStar/board.toml), the
`northStarBoard` gate computes it, and the `readmeParity` gate holds all four README languages to
what it computed.

**Base tier.** Exactly one holds per axis, and it is a ceiling.

| Base tier | Score | What has to be true |
|---|---:|---|
| `none` | 0 | No gate asserts this axis |
| `manual` | 3 | Someone watched it work by hand. No gate is registered in a runner. Demo videos, screenshots, and "I ran it and it worked" all land here |
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

**Caps.** They push down over anything claimed above them.

| Cap | Result | What has to be true |
|---|---:|---|
| `skipped` | 5 | The gate skipped in the last run. A skip is not a pass |
| `flaky` | 5 | The gate passed on a retry |
| `unregistered` | 0 | The gate file exists and no runner invokes it. Identical to having no gate |

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
| **PC (Windows)** | Installer with a launcher. Stays current on its own after install. GitHub Releases is the source of truth |
| **PC (macOS, Linux)** | In preparation |
| **Mobile** | PWA. Open it in a browser and add it to the home screen. No app store needed |

There is no release yet. This is the design stage.

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
- **Not an agent framework.** No planner, no subagent orchestration, no autonomous loops. Each CLI already has that and does it better.
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
| `crates/` | The product (Rust). Daemon, provider adapters, transport, desktop app | Not created |
| `pwa/` | Mobile PWA | Not created |
| `site/` | GitHub Pages landing | Not created |
| [`assets/brand/`](assets/brand/) | The logo. SVG is the source; favicons, icons, and social cards derive from it | |
| [`docs/`](docs/README.md) | Operational documentation, source of truth | |
| [`mainPlan/`](mainPlan/README.md) | What is to be built (initiatives; on completion the knowledge is promoted to `docs/` and the folder is deleted) | |
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

[MIT](LICENSE)
