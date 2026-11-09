# Local Agent upstream provenance

Local Agent is an independent ZeroClaw-derived project. Its product brand is
undecided. It is not an official ZeroClaw distribution and claims no upstream
trademark rights or endorsement.

## Pinned foundation

- Canonical upstream: <https://github.com/zeroclaw-labs/zeroclaw>
- Baseline branch: `master`
- Exact baseline commit: `9dbb57231f48e26fee56243de4f7845924ce4f04`
- Baseline tree: `0f767028d5166d64f670cc9df14d6f723a0f5ecb`
- Baseline selection timestamp: `2026-11-02T13:42:23+08:00` (local clone ref record).
- Official default branch: `master`; its latest commit matched the pinned SHA
  when selected and again when the provenance tag was verified.
- Local annotated provenance tag: `mvp-0.0`, founder-approved at the original SHA.
- Published annotated provenance tag: `MVP-0.0`; its object and target match the
  preserved local tag. Git ref names are case-sensitive; preserve both spellings.
- Nearest release tag: `v0.8.7`
- Tag commit: `99bd353c756f4e64f381cfe9c3002ebff32f3055`
- Baseline description: `v0.8.7-35-g9dbb57231`
- Workspace version: `0.8.7`; edition: `2024`; minimum Rust: `1.96.0`
- This record describes upstream foundation scope; later derivative behavior is
  tracked separately through reviewed changes.

The clone is non-shallow. Full upstream ancestry is retained locally. The baseline
is a source reference, not a claim that all upstream checks pass or that Local
Agent is ready for release. Package and executable names remain upstream names
until a separately reviewed productization change.

## Licensing and attribution

Preserve [LICENSE-MIT](LICENSE-MIT), [LICENSE-APACHE](LICENSE-APACHE) and
[NOTICE](NOTICE), including existing copyright and contributor attribution.
Upstream declares `MIT OR Apache-2.0`. Dependencies and vendored components need
their own license review before distribution; the root license is not a blanket
license for every dependency or asset.

The pinned NOTICE references root `TRADEMARK.md` and `CLA.md`, which are absent at
this commit. Contributor CLA documentation exists at
`docs/book/src/contributing/cla.md`; that does not establish derivative IP rights
or grant trademark permission. Do not invent the missing files, remove the notice
or describe the distribution review as complete. Track any resolution through the
reviewed source change and existing issue before release.

## Verification

Run from a checkout containing the preserved upstream refs:

```sh
git rev-parse --is-shallow-repository
git cat-file -t 9dbb57231f48e26fee56243de4f7845924ce4f04
git merge-base --is-ancestor 99bd353c756f4e64f381cfe9c3002ebff32f3055 9dbb57231f48e26fee56243de4f7845924ce4f04
git diff 9dbb57231f48e26fee56243de4f7845924ce4f04 -- LICENSE-MIT LICENSE-APACHE NOTICE
```

Expected: non-shallow `false`, object type `commit`, ancestry check exits zero,
and no license/notice changes. Commit counts are observations, not frozen
acceptance criteria. The founder specifically approved the annotated `mvp-0.0`
provenance tag at this original upstream commit. Verify it without moving it:

```sh
git cat-file -t refs/tags/mvp-0.0
git rev-parse 'mvp-0.0^{commit}'
git rev-parse 'mvp-0.0^{tree}'
```

Expected: `tag`, the exact baseline SHA, and the exact baseline tree above. The
founder-published remote `MVP-0.0` was independently verified at annotated object
`4e36ed7232dc44d59afed4a2f4f78e8923709b48`, identical to local `mvp-0.0`, and peels
to the baseline commit. Remote default branch `master` contains that pristine
baseline. Initial baseline/tag delivery is complete; this is not QA signoff.
Never move, recreate or overwrite either existing provenance ref. Version
completion tags and public releases retain their applicable verification and
authorization requirements.

## Repository and export boundaries

The authorized project destination is `horonomy/local-agent`; upstream remains a
separate source. Authentication, authorship and repository ownership are separate
facts. Verify destination and actual write identity before GitHub operations
without exposing credential values; keep upstream push-disabled locally.

Reviewed project source, documentation, tests and necessary project configuration
have standing transfer authorization. Review each new outbound object and its
provenance, not only extensions or ignore rules. Already published upstream
ancestry is not permission to retransmit local visual media or private execution
artifacts. Keep workstation captures, credentials, corporate content, raw logs and
model/session data local and outside Git. Preserve full ancestry and traceability;
never silently rewrite history, publish unapproved artifacts, or use source
permission to bypass execution, review or release controls.

## Divergence ledger

| Ticket | Intended change | Runtime impact |
| --- | --- | --- |
| HORO-2376 | Provenance, contributor instruction adaptation and local-state ignores | None |
| HORO-2377 | Reproducible baseline assessment and measurement method | None |
| HORO-2378 | Synthetic Golden Journey fixtures | Tests only |
| HORO-2379 | Content/provenance export review support and policy | Developer workflow only |
| HORO-2380 | Architecture and upstream synchronization decision | Documentation only |
| HORO-2383 | Gate the relay bridge, enrollment server and local RPC listener in `zeroclaw-runtime` behind the existing `daemon-listeners` feature so the minimal profile compiles none of that listener source; revert the merge to roll back | None in standard builds; the minimal profile no longer contains those listeners |
| HORO-2383 (C1) | Make `tokio-tungstenite` optional in `zeroclaw-runtime` (enabled by `daemon-listeners`), remove it from `zeroclaw-tools` (unused), enable it in the root crate only through `gateway` and `acp-bridge`, and rewrite `relay_server_name` without tungstenite's `Uri` (same accept/reject set, checked against the old implementation on about 1.08 million inputs); measured delta in the minimal profile graph is 10 packages (`tokio-tungstenite`, `tungstenite`, `sha1`, `webpki-roots`, `rand` 0.9, `rand_chacha` 0.9, `rand_core` 0.9, `getrandom` 0.3, `ppv-lite86`, `zerocopy`); binary bytes not measured here; revert the merge to roll back | None in standard builds (default dependency graph identical before and after) |
| HORO-2381 (`841f577ff`, `cdc3b8acf`, `b181010b5`, `0f941dedb`, `605284d76`, `863ab6257`, `eec88a85e`) | Split `agent-core` out of the root crate's `agent-runtime` feature (`agent-runtime` is now `agent-core` plus `daemon-listeners`), add the `local-agent-minimal` marker feature that equals `agent-core`, and make `src/lib.rs` reject it combined with any ingress, bundle or `channel-*` feature at compile time; fix the xtask preset filter so default-listed presets stay out of the alias filter; revert the merge to roll back | None in standard builds: the default feature set resolves to the same dependency set as before. `local-agent-minimal` is opt-in (`--no-default-features --features local-agent-minimal`) and `--all-features` on the root crate now intentionally fails to build |
| HORO-2382 / HORO-2385 (`157c75dba`, `783315c8a`, `372b7b90b`, `44550460f`, `4cfc859e5`, `d2ea0f612`) | In `src/main.rs`, refuse the `daemon`, `relay`, `gateway`, `acp` and `channel start` commands and the `service` commands that start or install a unit before any config load; refuse a config that enables `[wss]`, `[relay]`, `[enroll]`, `[a2a.server]`, a tunnel provider or a configured channel the build does not compile; accept `--model-provider` only when it names a configured `<family>.<alias>` on every command that takes it; compile the daemon and relay arms only with `daemon-listeners`; revert the merge to roll back | Refusals, the config check and the override gate exist only under `local-agent-minimal`; standard builds keep every command and accept any `--model-provider` as before. The daemon and relay arms are compiled out of builds without `daemon-listeners`, which every standard build enables |
| HORO-2382 (`cace93da7`) | `zeroclaw update` in the minimal profile never selects the standard release archive, with or without `--standard`, and prints profile-specific guidance instead of the standard-build message; in `src/commands/update.rs`; revert the commit to roll back | Minimal profile only for the archive selection and message. The `agent-runtime` to `agent-core` cfg renames in the same file select the same code in standard builds |
| MVP 1.0 dependency and archive hardening (`130056bfd`, `86bef3b84`, `e6ff8d40e`, `0741406dc`, `7fb3599bf`) | Raise `zip` to 9.2.0, call `validate_header_consistency` before extracting archives in the updater (`src/commands/update.rs`), the plugin registry (`src/plugin_registry.rs`) and skill discovery (`src/skills/discovery.rs`), patch `pulldown-cmark` to 0.13.6 and `vodozemac` to 0.11.2 in `Cargo.lock`; revert the commits to roll back | Affects standard builds: archives with inconsistent ZIP headers are now rejected at those three extraction boundaries, and the locked dependency versions changed. These are security fixes, not minimal-profile gating |
| Test hardening and lint fixes for MVP 1.0 CI (`33efc56a1`, `a21b4d35a`, `87067f579`, `31395e335`, `3a3e0f224`, `e13d891e5`, `6a436cfc0`, `7fbc5a4e1`, `8b23c2b13`, `b924bce6d`) | Make scheduler-dependent tests deterministic in `zeroclaw-runtime` tunnel/tailscale, `zeroclaw-hardware` Arduino upload, `apps/zerocode`, `zeroclaw-providers` OpenAI wire fixtures, `zeroclaw-channels` Telegram, `zeroclaw-config` policy, the `zeroclaw-gateway` port-release test and the SOP event test in `src/main.rs`; simplify one empty-batch predicate in `zeroclaw-infra` ACP session store with identical behavior; revert the commits to roll back | Test code and one equivalent lint rewrite only; no standard-build behavior change intended. The one exception is the filesystem entry below |
| Filesystem channel readiness (`0789b5901`, `9d7050738`) | Add `FilesystemChannel::listen_with_ready`, a one-shot that fires after every configured path has an active watcher, in `crates/zeroclaw-channels/src/filesystem.rs`; the regular `listen` path passes no signal; revert the commits to roll back | Adds a public method to standard builds; `listen` behavior, admission policy and shutdown are unchanged and covered by new failure and dropped-receiver tests |
| Windows rename error classification (`254bc84d2`) | Decode only `HRESULT_FROM_WIN32` failures into raw OS errors in `crates/zeroclaw-plugins/src/host/recovery.rs` so a missing package directory reaches the existing `NotFound` recovery path; atomic no-replace moves are unchanged; revert the commit to roll back | Windows builds only (the production helper is `cfg(windows)`); no change on other platforms |
| MVP 1.0 docs, build and repository metadata (`c98278b8d`, `0eebf0d46`, `3922e1819`, `78d2bf47b`, `d88cff247`, `8e56aaa6e`, `ce9dd3c42`) | Rebuild the root README and mdBook around Local Agent, mark inherited listener pages as not Local Agent behavior, drop the generated README installer block in xtask, stop writing the upstream `CNAME` in the docs deploy and delete the root `CNAME`, replace `.github/CODEOWNERS`, and align `AGENTS.md`, `CLAUDE.md` and `CONTRIBUTING.md`; revert the commits to roll back | No runtime impact. The xtask change alters generated documentation surfaces for all builds |
| MVP 1.0 CI and workflow definitions (`.github/workflows/`, `.github/actions/`, `scripts/ci/`, `.cargo/dependency-risk.toml`, `deny.toml`) | Standard hosted runners, pinned Action versions, bounded test and artifact budgets, a dependency-risk guard, PR title check and the minimal-profile artifact, tree and binary checks; revert the relevant commits to roll back | CI and developer workflow only; no change to shipped binaries |

Rows describe ticket scope, not completion or merged state. Exact local commit,
review, PR, validation and blocker status belong in the local engineering ledger.
The MVP 1.0 rows above record the deviations from the pinned baseline that
change runtime behavior, a shipped dependency, the build graph, generated docs or
CI, with their gating and rollback. A deviation without a row here is a ledger gap
to fix, not an authorization. Commit lists were checked against
`git log 9dbb57231f48e26fee56243de4f7845924ce4f04..release/mvp-1.0` at `aefcfa5a7`.
