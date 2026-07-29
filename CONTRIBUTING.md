# Contributing to runtrol

Thank you for looking. runtrol is at the **design stage**: what exists today is the North Star,
architecture decisions backed by measurements, and the contract gates. Product code lands
milestone by milestone (see [mainPlan/README.md](mainPlan/README.md)).

Design-stage contributions are real contributions: challenging a decision in `docs/` with
evidence, testing a probe on your machine, or preparing a provider manifest all count.

## The one rule that never bends

**runtrol is thin.** It supervises coding-agent CLIs and transports their events.
A PR is closed regardless of quality if it makes runtrol:

- read, store, index, summarize, or rewrite conversation content
- call model APIs or hold API keys (the child CLIs own their own auth)
- hardcode provider knowledge: model names, flags, session paths (discovery over constants)

If you want a capability that seems to need one of these, open an issue first.
Almost always the provider CLI already exposes a structured surface for it.

## Where truth lives

| | |
|---|---|
| `README.md` | The North Star, honestly scored. A score only counts when a gate runs in CI |
| `docs/` | Decided. The operational source of truth |
| `mainPlan/` | Being designed or built. Initiatives are deleted on completion, knowledge promoted to `docs/` |
| `tests/audit/` | The contract gates |

## Before you code

- For anything non-trivial, open an issue first. [docs/positioning.md](docs/positioning.md)
  defines what fits this product and what does not.
- New capabilities graduate through `tests/_attempts/<category>/` before entering `crates/`:
  prove the concept against the real CLI first, then modularize, then clean, then land.
- **The easiest first contribution is a provider manifest.** A CLI that speaks ACP registers
  with about ten lines of TOML and zero Rust (see `mainPlan/providerAdapter/`).

## Gates

```bash
python -X utf8 tests/audit/preflight.py     # full local CI
git config core.hooksPath .githooks        # once per clone
```

A gate is a defect detector, not a rubber stamp. When you add one, prove it can fail
before trusting that it passes: plant the defect it should catch and watch it go red
(`python -X utf8 tests/audit/checkSilentFail.py --selftest` is the model).

## Style

- Rust idiom. `cargo fmt --all --check` and `cargo clippy --all-targets -- -D warnings` are floor conditions.
- No silent failures. Discarding a `Result` needs an `// ok:` comment explaining why it is
  safe and how progress is guaranteed. Machine-checked by `tests/audit/checkSilentFail.py`.
- No em dash (U+2014) or en dash (U+2013) anywhere, no emoji. Yes, really; it is machine-enforced.
  Use periods, colons, parentheses, or a tilde for ranges.
- Structure: no `utils/`, `helpers/`, `common/`, or `misc/` dumping grounds. One concern,
  one source of truth. If the same value lives in two places, that is a bug.

## Commits and pull requests

- Maintainers keep the history in Korean `성격: 내용` form. **You may write your commits in
  English**; PRs are squash-merged with a normalized message, so do not worry about the format.
- One PR = one intent, tests ride along.
- A PR that raises a North Star score must link the CI run of the gate that backs it.
- Do not credit AI tools in commit metadata or messages: no bot co-author trailers and no
  tool-name attribution lines. The push hook blocks these (`tests/audit/checkNoAiMarkers.py`
  is the engine, and this document intentionally avoids quoting the banned strings so that
  it can pass its own gate).

## License

MIT. By contributing you agree your contributions are licensed under MIT. There is no CLA.
