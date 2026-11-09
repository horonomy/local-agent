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

Rows describe ticket scope, not completion or merged state. Exact local commit,
review, PR, validation and blocker status belong in the local engineering ledger.
No MVP 1.0 runtime behavior changes are authorized by this foundation record.
