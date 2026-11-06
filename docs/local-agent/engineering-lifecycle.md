# Local Agent engineering lifecycle

Ticket: HORO-2420; engineering metadata alignment: HORO-2430. This is the Local
Agent repository workflow referenced by root `AGENTS.md`. It replaces inherited
ZeroClaw team procedures where they differ, not workstation security rules,
license obligations, technical boundaries or actual repository protection.

## Local governance and decision scope

Local Agent is independent of ZeroClaw. Upstream Core Team names, contribution
tiers, vote quorums, fixed RFC timers and squash automation do not appoint Local
Agent reviewers or authorize operations in this repository. Preserve the upstream
records as historical context rather than rewriting their decisions.

Use the approved roadmap and an ordinary ticket/PR for routine functionality,
configuration or bounded refactoring. Record a durable design decision before
materially changing the security model, project authority, subsystem ownership or
cross-component contracts. An existing explicit founder decision can supply the
product authorization; implementation still needs independent technical review
and the applicable required checks.

### Single-maintainer review and integration

Local Agent currently uses a founder-approved single-maintainer model.
`@Chisanan232` owns the repository and accountable integration decisions.
`.github/CODEOWNERS` routes consultation to that actual owner; it does not appoint
ZeroClaw maintainers, require another GitHub account, or create a human approval.
Upstream Core Team votes, reviewer counts and exception timers do not apply here.

Every substantive change requires independent technical review by a Claude Code
or Codex reviewer distinct from the implementation session. Record the reviewer,
exact candidate commit, review scope, findings and their resolution. Security,
regression and compatibility consequences determine review depth. An AI review is
not a GitHub human approval; do not submit it as an approval by a separate human,
impersonate a reviewer or fabricate a review state.

For an owner-authored PR, the owner-review and integration workflow is:

1. Reconcile the approved acceptance criteria, diff, provenance and rollback.
2. Obtain independent technical review and resolve all substantive findings.
3. Verify the exact candidate's applicable CI, security, integration and documented
   validation gates, mergeability, branch currency and unresolved review threads.
4. Inspect actual branch protection and repository rules before merging. Honor
   every configured approval requirement. Do not bypass an active requirement:
   if a rule requires a separate non-author human approval, reconcile that policy
   through explicitly authorized repository administration before integration;
   neither ownership nor this document makes it passed.
5. Record the accountable owner integration decision and the evidence above,
   merge using a normal merge commit, and verify the resulting commit. Routine
   integration may be executed under the owner's standing authorization without
   inventing a separate human review or repeated routine founder decisions.

Inspect live approval rules at integration and keep administrative inspection
evidence local. A prior inspection is not a permanent exemption, approval or
passing CI evidence. Repository instructions do not change workstation controls
or publishing permission.

The existing [architecture map](../book/src/contributing/architecture-map.md) routes
technical references. Its linked ZeroClaw governance pages remain useful upstream
context, but this lifecycle owns Local Agent branch, review and validation flow.
Scoped crate instructions still govern dependency direction and local invariants;
they do not appoint an upstream team to approve unrelated derivative workflow.

## Release branches and provenance

Keep the verified default branch stable. The current Local Agent default is
`master`; resolve the remote default instead of assuming `main`. Each milestone
uses one integration branch: `release/mvp-0.0` through `release/mvp-6.0`, then
`release/v0.0.1`. For normal milestones, fetch the authorized origin, verify the
current default-branch commit, and record the exact base SHA and creation time
before branching. Inspect and reuse an existing valid release branch; never
overwrite its progress.

The initial MVP 0.0 bootstrap has one exception: when the approved destination
has no default-branch commit, a local `release/mvp-0.0` may start at the pristine
upstream commit `9dbb57231f48e26fee56243de4f7845924ce4f04`, tree
`0f767028d5166d64f670cc9df14d6f723a0f5ecb`. Independently verify the commit and the
absent remote base. This exception does not authorize import, bypass denied
controls, fabricate a remote base, or promote a milestone.

The annotated `MVP-0.0` remote tag is immutable provenance at that pristine commit,
not MVP 0.0 QA approval. Preserve the existing local `mvp-0.0` tag and its identical
annotated object; tag names are case-sensitive, so do not rename, recreate or move
either ref. Preserve full upstream ancestry, licenses and attribution. Keep the
original upstream as `upstream` and the independent project as `origin`.
Version tags require the applicable verified release commit and authorization;
source-transfer permission and a provenance tag do not authorize a GitHub Release,
installer, package or deployment.

## Ticket work and promotion

Create each independent task branch and worktree from the current milestone
release branch. A suitable name is `mvp-0.0/HORO-XXXX/fix/short_summary`.
Preserve dirty worktrees, reviewed changes and existing commits. A migration must
check conflicts and copy only individually reviewed source changes, never private
configuration, evidence, Git metadata or unrelated modifications.

Use small, single-purpose commits named `<gitmoji> <scope>: <summary>`.
Ticket PR titles use `[HORO-XXXX] <gitmoji> <scope>: <summary>`; their base is the
milestone release branch. The title validator also retains scoped Conventional
Commits compatibility for existing automation and dependency updates.
Include purpose, scope, acceptance criteria, actual tests, risks and limitations.
Independent technical review and accountable owner integration follow the
single-maintainer contract above. They never substitute for a separate human
approval actually required by live repository rules. Never bypass mandatory
checks or unresolved reviews.

After all mandatory acceptance, cumulative Golden Journeys, security, privacy,
licensing and supported-platform gates pass against an exact release-branch SHA,
prepare a separate release PR targeting the latest verified default-branch commit.
Revalidate if that branch moved. Use merge commits through the required reviewed
process and verify the resulting commit.
Do not start the next milestone from an unpromoted or broken predecessor.

## Persistent problem budgets

Track each underlying problem outside version control with a stable identifier,
related ticket, first observation, active investigation time, remediation attempts,
hypotheses, results, release impact and disposition. Deduplicate across sessions,
subagents, worktrees and superficial changes of wording.

The circuit opens at five failed remediation attempts or 60 cumulative active
investigation/remediation minutes, whichever occurs first. There is no sixth
attempt. Record actual fix-and-verification cycles; separately record diagnostic
probes and availability failures. Compilation or passive waiting is not active
engineering time. Unknown historical time is not zero or proof of exhaustion:
bounded factual reconstruction is bookkeeping, not a fresh approval gate. Do not
fabricate a remaining allowance; retain known counters and seek an applicable
decision if reconstruction cannot establish a usable budget. A new session never
resets the budget.

An enforced policy denial stops that operation immediately, even below the time
or attempt limit. Do not retry it through another API, browser, agent or indirect
path. Explicit authorization does not override an enforced security boundary.

Choose a recorded disposition when a circuit opens:

- **Outcome A:** retain the mandatory acceptance, security, licensing, review or
  release blocker and wait for a legitimate decision or control resolution.
  Do not promote the affected release branch.
- **Outcome B:** retain an optional or isolated blocker, mark its criteria BLOCKED,
  and continue genuinely independent authorized work. This does not waive a
  mandatory criterion or make the blocked milestone releasable.

The problem ledger is the accounting source of truth. A helper, reminder or
scheduler must honor it but is not proof of installed enforcement. Verify
restart, cross-agent persistence, attempt/time limits, blocked dependencies and
successful recovery before claiming automated policy enforcement.

## Validation and reporting

Use this risk-based evidence ladder; do not equate an ordinary source commit with
release acceptance:

| Boundary | Required evidence |
| --- | --- |
| Implementation increment | Focused deterministic tests at the changed behavior, relevant formatting and scoped lint/type checks; regression for a bug. |
| PR integration | Relevant integration/regression and feature-enabled/disabled checks, independent technical review, accountable owner integration, all applicable exact-head CI and actual configured approval rules. |
| Release checkpoint | Complete required suite, cumulative Golden Journeys, security/license and supported-platform checks, documentation build/parity, and independent QA tied to the candidate SHA. |

Do not compile the whole workspace after each small change or duplicate expensive
unchanged evidence without a named gap. Risk can justify broader checks sooner,
especially for cross-component, compatibility, dependency or security changes.
Retain valid evidence only when its source, target, feature set and environment
still match; a changed candidate invalidates the affected portion.

Permission-sensitive changes need negative paths and cleanup/cancellation coverage.
Verify actual exit status and nonzero executed test counts; compilation alone is
not a runtime pass. Preserve inherited failures separately from derivative results.
Never suppress, skip or weaken a required test to turn a gate green.

Examples, scoped to the actual owning crate or target:

```sh
cargo test --offline --locked -p <owner-crate> --test <target>
cargo clippy --offline --locked -p <owner-crate> --all-targets -- -D warnings
cargo fmt --all -- --check
```

At an applicable workspace/release checkpoint, the existing non-desktop boundary is:

```sh
cargo test --offline --locked --workspace --exclude zeroclaw-desktop
```

This repository contains `.githooks/`, not a `.pre-commit-config.yaml` contract.
Inspect the active hook configuration and run the applicable real hook; do not
invent a pre-commit configuration or install/trust hooks merely because a generic
workflow mentions them. A missing optional scanner is NOT a scanner pass. Docker
parity and Rustdoc builds are used when their surface or release matrix requires
them, not for every prose or isolated fixture edit.

Repository testing policy is distinct from enforced workstation execution. A host
full-test receipt, source-authorization denial or broader pre-push hook still binds
while active even if this repository policy is risk-based. Do not change host
settings, forge receipts, use bypass flags or publish through an alternate API or
agent. Record the exact mismatch and pursue its legitimate control mechanism.

Use PASS, FAIL, BLOCKED, NOT RUN and NOT APPLICABLE accurately. Record concise
sanitized Jira updates with test counts, source or merge identifiers, limitations
and acceptance decisions. Close work only when its evidence and required workflow
support completion. A partial gate is not release readiness.

## Dependency risk governance

The founder-approved dependency strategy (HORO-2447, 2026-11-06) is risk-based,
not a zero-warning requirement. Keep vulnerability detection, Cargo Deny,
lockfile integrity, license compliance and dependency-source restrictions. Do not
silence all advisories, disable scanners, or fork/vendor an upstream library merely
to remove a maintenance warning.

| Priority | Finding | Required disposition |
| --- | --- | --- |
| P0 | Confirmed critical vulnerability, reachable memory unsoundness, serious security exposure or exploitation path. | Remediate or demonstrate effective isolation before releasing the affected profile. |
| P1 | Confirmed vulnerability with limited exposure or specific exploitation prerequisites. | Assess the actual prerequisites and supported profiles; applicable exploitable or memory-safety defects still block release until remediated or effectively isolated. |
| P2 | Unmaintained, unsupported or end-of-life dependency without a known exploitable vulnerability. | Prefer sustainable upstream remediation or profile isolation; consider an exact, documented, expiring maintenance-risk exception. |
| P3 | Routine update or noncritical compatibility/maintenance work. | Validate affected consumers and integrate at normal gates without manufacturing a security emergency. |

`Unmaintained`, `Vulnerable` and `Unsound` are not equivalent. An informational
unsoundness advisory still requires memory-safety analysis; a low attack likelihood
is not proof of isolation. Assess source/provenance, the locked dependency path,
actual feature selection, reachable APIs, attacker influence and ongoing remediation
cost. Separate Minimal Runtime, supported optional extensions, build/test tools and
legacy ZeroClaw channels. A package in Cargo.lock need not ship in every profile;
a disabled channel does not remove an independent tools dependency. Preserve
practical ZeroClaw Skills, Plugins, Tools and MCP interoperability and keep AASM
optional. The existing HORO-2371/HORO-2391 compatibility contract remains binding.

### Maintenance exceptions

Every new maintenance exception must identify:

- Exact advisory, crate, version, source and dependency origin.
- Affected supported profiles, actual exposure, security justification and
  compensating controls; state what has not been verified.
- Accountable owner, approval, expiry and review dates.
- Upstream tracking, replacement/removal conditions and the existing Jira task.

Validate the actual scanner version and configuration schema before editing.
Release CI currently pins Cargo Deny 0.19.9; local verification used 0.20.2.
Their tagged advisory configuration supports an ignore ID and reason, not a native
expiry or package-version scope. Newer online documentation is not evidence that
those versions accept additional fields. An ID-wide ignore therefore needs a
separately tested fail-closed date and locked-graph scope check, enforced before
both Cargo Deny and cargo-audit. An expiry written only in prose is not enforcement.
Keep the scanners and existing license, source, integrity and vulnerability checks.
Do not treat an exception as active before its implementation, review, checks and
normal integration qualify. Policy approval cannot clear a host denial.

Confirmed exploitable vulnerabilities or applicable memory-safety defects require
remediation or demonstrated effective isolation before release. Significant
residual security-risk acceptance needs accountable authorization; routine
maintenance approval does not authorize it. Inherited advisory ignores and wrapper
exceptions are existing debt to reassess, not evidence of a completed assessment,
a new approval or automatic renewal.

### Current dependency risk and remediation matrix

This is the treatment decision, not a scanner pass or release receipt. All rows
remain subject to actual selected-profile evidence and exact-head integration.
The accountable owner is `@Chisanan232`; listed Jira tasks retain delivery state.

| Finding / product exposure | Risk and treatment | Tracking / review or expiry | Release-blocking status |
| --- | --- | --- | --- |
| `pin-utils 0.1.0`, RUSTSEC-2026-0441, via `async-imap 0.11.3`; default tools dependency and optional email consumer. | P2 maintenance-only. Select a short scoped exception rather than require a fork solely to remove this warning; preserve the reviewed local backport as an alternative. | HORO-2447. Proposed review 2026-11-20 and exclusive UTC expiry 2026-12-06; not active or enforced. | Current Security gate remains FAIL. Exception implementation is host-blocked; no waiver or merge clearance. |
| `lru 0.16.4`, RUSTSEC-2026-0253, optional Nostr cache; default runtime/channels resolve patched `0.18.4`. | P1 pending reachability analysis of panic/unwind prerequisites; this is unsoundness, not maintenance. The inherited ban permits listed Nostr direct wrappers; it is not a blanket rejection of vulnerable use. Preserve its scope checks, but expected parents alone do not prove safe use. | Existing Nostr upstream tracking #9602; reassess before enabling an affected supported profile. No new expiry/exception granted here. | Blocks an affected supported profile until remediated or effectively isolated. Default exclusion is not an all-features pass. |
| `anymap2 0.13.0`, RUSTSEC-2026-0319, optional Matrix path included by wider distribution features. | P2 unmaintained-only. Prefer upstream migration; reassess inherited exact-ID ignore without adding a fork automatically. | Existing upstream tracking #11429; review before the next distribution candidate. No new or renewed exception here. | Existing ignore is not completion evidence; actual profile/security gates still apply. |
| `rustls-pemfile 2.2.0`, RUSTSEC-2025-0134; direct TLS/default-runtime consumers as well as transitive/optional/dev edges. | P2 unmaintained-only. Plan migration to `rustls-pki-types`; the inherited rationale saying only transitive is incomplete. | Existing upstream migration tracking; review before the next release candidate. No new or renewed exception here. | Not a demonstrated exploit, but exposure must not be dismissed as optional-only. Existing checks still apply. |
| ZIP critical dual-header advisory versus later ZIP 9.2 compatibility work. | P0 fix is already integrated through PR #15: ZIP 9.0.5 plus admission guards. Treat 9.2 as P3 unless new security evidence changes that. Preserve its test draft. | HORO-2437 Done; HORO-2453 owns the later candidate. No exception. | Verify the selected candidate and three extraction boundaries. A pristine-default-branch alert is not the release branch's locked version. |
| Crypto API migration, notify 8, GitHub Action pins and remaining Dependabot updates. | P3 unless actual advisory/exposure evidence raises priority. Preserve candidates and validate final consumers; baseline tests do not prove an upgrade. | HORO-2449, HORO-2452, HORO-2448 / PR #25, original PRs #1–8. No exception. | Final execution/CI and specific host restrictions remain binding. Close an original PR only after verified integrated supersession. |
| Web advisory backlog and lockfile-only legacy `glib` unsoundness. | Keep actual vulnerability/unsoundness assessment separate from P2 warnings. Build/test-only does not imply harmless; prove profile exclusion or remediate applicable exposure. | Existing Dependabot/security backlog and inherited GTK tracking #8519; no new waiver. | Confirmed applicable defects block affected release profiles. Do not claim all security findings resolved by the pin-utils decision. |

For HORO-2447, compare alternatives independently of sunk effort:

- **A: isolation/pruning:** appropriate for the approved Minimal Runtime where
  verified, but removing the email feature alone leaves unconditional IMAP in
  `zeroclaw-tools`. Do not delete compatible extension interfaces as a shortcut.
- **B: bounded backport:** existing source, provenance, licenses, pinning review
  and focused tests remain preserved. It adds a local maintenance obligation and
  still needs its actual delivery/host gates; prior investment does not require
  shipping it.
- **C: expiring maintenance exception:** lowest current sustainable cost for this
  maintenance-only finding. It permits neither a new vulnerability ignore nor
  supported-profile expansion. Implementation remains blocked; retain the failing
  gate until an exact tested and reviewed exception qualifies. Retire any eventual
  exception when a verified upstream release or approved profile migration removes
  the dependency, or when new security evidence invalidates its justification.

## Documentation is a product gate

Every behavior-changing ticket must assess documentation impact and update the
affected source documentation in the same PR or identify a clearly linked blocking
PR. A milestone cannot pass QA while required documentation is incomplete or
contradicts the candidate's actual behavior.

Starting with MVP 1.0, README, mdBook landing and navigation, Getting Started,
architecture, configuration examples and engineering guidance must identify Local
Agent as the product and describe only supported release-profile capabilities.
Preserve ZeroClaw licenses, provenance, attribution and historical ADRs. Use the
existing mdBook pipeline and `docs/book/src/architecture/decisions/` ADR location;
do not invent a parallel documentation tree or hand-edit generated references.

Verify documented CLI commands against the candidate, generated snippets against
the selected feature profile, and navigation, internal links and documentation
builds. Distinguish planned, unsupported and unverified behavior from available
features. The founder retired the vulnerable third-party Markdown formatting
execution; do not install or run it to satisfy a release prerequisite. Preserve
repository-owned source checks, changed/internal-link validation, real mdBook
builds, profile parity and independent QA. Unavailable meaningful checks remain
BLOCKED, not passed. The separate reviewed CI/tool-retirement change must be
integrated before an older checkout's script is safe to invoke; never execute its
old formatter path just because this policy names the script.

Documentation hosting stays disabled until a Local Agent-specific destination and
deployment authorization are established. Never deploy through ZeroClaw's domain,
account or publishing credentials. Source-documentation collaboration does not
authorize uploading captures, execution reports, logs or other restricted data.

## Source and data boundaries

Review content and provenance, not just extensions or ignore rules. Approved
public source, non-sensitive templates and generic project documentation are
separate from private runtime dumps and machine-specific agent configuration.
Review the complete outbound Git object set before transfer, including history
and metadata. A staged scanner pass is not initial-history export approval.

Keep workstation configuration, credentials, corporate content, logs, transcripts,
execution traces, captured media, runtime databases, generated dumps and benchmark
artifacts local and outside Git. Native Jira metadata and sanitized text summaries
are not file attachments. Do not upload or encode restricted artifacts through
issues, PRs, CI, models or another service. Existing public upstream media does
not grant blanket authorization to retransmit it from a restricted workstation.

Capture tools may declare image attachments, and provider preparation may inline
them into model requests. Path confinement, approval to capture, and disabled
remote image fetching do not prove local-only provider egress. Keep capture
verification blocked until outgoing image and encoded tool-result paths satisfy
the applicable policy. Never change host networking to simulate that guarantee.

## Skills, continuity and resource discipline

Inherited `.claude/skills/` can contain hardcoded upstream repositories, owners,
master-only PR routes, public tracker writes, squash commands and gateway setup.
Inspect applicability before invocation. Do not run those actions for Local Agent
unless the procedure has been reconciled with current destination, actor, scope
and this lifecycle. A skill is not authorization to publish, merge, install an
external source or access a configured user's runtime credentials.

Use bounded independent readers/reviewers and one writer per worktree. Keep raw
logs and detailed evidence in the existing approved local evidence area, outside
commits; return concise findings and exact source/test references. Preserve a
handoff before context pressure and never recover by repeatedly reading entire
agent transcripts or restarting an exhausted request. Consolidate exact Git state,
PR heads/checks, verified evidence, host restrictions and next actions by roughly
55–60% context usage. At roughly 65–70%, stop large new assignments and prepare a
bounded handoff without interrupting active Git operations or discarding work.
Never rely on compaction near exhaustion or claim automatic fresh-session migration
unless the host actually provides it.

Before compiling, identify the affected package, feature set, target triple and
required tests. Never default to a full Rust workspace build for a small change.
Measure free capacity and expected footprint before expensive compilation; reuse
valid artifacts for the same source/profile instead of rebuilding unchanged proof.

Coordinate target-directory reuse by toolchain, target triple, features and active
ownership. Do not create a large target for every short-lived worktree or blindly
share one among concurrent writers. Bound heavyweight build concurrency and reduce
it under CPU, memory or disk pressure. Pause new heavy work when headroom is unsafe,
then reclaim only authorized inactive, regenerable outputs and resume when actual
measurements permit. Keep concise records of unusual artifact growth.

Preserve Local Agent and other active project resources, worktrees, required
binaries and QA evidence. A denied cleanup is not permission to use another route. Push
reviewed source regularly when actual controls permit and verify the remote
commit; local commits alone are not a remote backup.

No instruction in this document automatically trusts inherited hooks, grants OS
permissions, changes branch protection, authorizes public publication, or clears
an enforced denial.
