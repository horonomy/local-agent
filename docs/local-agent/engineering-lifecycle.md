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

Local Agent uses the founder-approved single-maintainer model.
Zero human PR reviews are required by project policy. There is no non-author
human approval, upstream reviewer quorum or human-review exception timer.
`@Chisanan232` owns accountable integration; CODEOWNERS identifies ownership,
not an additional reviewer or approval prerequisite. Do not ask the founder to nominate
reviewers, recruit collaborators or reconfirm this settled review policy.

Every approved development PR requires independent technical review by a
separate Claude Code or Codex review agent, distinct from its implementation
session. Record the exact candidate, review scope, findings and resolution.
Security, regression and compatibility consequences determine review depth.
An AI technical review is not a GitHub human approval: never impersonate a human
reviewer or fabricate a GitHub approval state.

The autonomous owner-authorized integration workflow is:

1. Reconcile acceptance criteria, source changes, provenance and rollback.
2. Obtain independent technical review and fix all blocking findings. Repeat
   focused tests and independent review until the current revision is acceptable.
3. Verify actual exact-candidate CI, security, product and release checks, branch
   currency, mergeability and unresolved findings or review threads.
4. Inspect live branch protection and rulesets. Do not bypass an active requirement.
   If a rule requires human approval, use the authorized repository-owner
   administration process to set that requirement to zero human approvals while
   preserving required status checks and other applicable security controls.
   Verify the resulting rules before merging; never claim an approval was obtained.
   If administration is denied, retain the real external blocker without inventing
   a project human-review requirement or using an administrator bypass.
5. Record accountable integration evidence, merge normally with a merge commit,
   verify the resulting commit, and continue the next approved task. Routine
   integration requires no additional founder or human-review decision.

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
Independent technical review and accountable integration follow the zero-human
review contract above. Reconcile any active GitHub human-approval rule through
owner administration as specified there; no approval may be bypassed or fabricated.
Never bypass mandatory checks or unresolved blocking findings.

After all mandatory acceptance, cumulative Golden Journeys, security, privacy,
licensing and supported-platform gates pass against an exact release-branch SHA,
prepare a separate release PR targeting the latest verified default-branch commit.
Revalidate if that branch moved. Use merge commits through the required reviewed
process and verify the resulting commit.
Do not start the next milestone from an unpromoted or broken predecessor.

### Pull-request triage

PR triage is a manual routing aid; it does not change acceptance criteria, risk
classification, required checks, review depth, or release readiness. Preserve all
existing category, size, priority, dependency labels and discussion history. Exactly
one of these mutually exclusive triage labels belongs on each open PR:

| Label | Meaning |
| --- | --- |
| `triage:active` | Selected for current implementation, validation, or review; not a claim that checks are green or the PR is merge-ready. |
| `triage:deferred` | Valid work intentionally waiting for a later review or capacity window. |
| `triage:blocked` | Delivery is waiting on a named external, CI, authorization, compatibility, security-review, or prerequisite condition. |
| `triage:needs-assessment` | Scope, fit, or risk needs a bounded assessment before choosing active or parked delivery. |

Deferred and blocked PRs also carry one or more reason labels, and no other triage
state does. The supported values are `reason:low-priority`, `reason:out-of-scope`,
`reason:upstream`, `reason:prerequisite`, `reason:ci`, `reason:authorization`,
`reason:compatibility`, and `reason:security-review`. Reason labels classify the
current decision; they are not risk acceptance, a waiver, or a replacement for
canonical security/risk policy. A P2 classification alone is not an indefinite
blocker. Keep a genuine vulnerability or unsoundness blocked while it remains
unresolved.

For `triage:deferred` and `triage:blocked`, keep one updatable structured decision
in a comment authored by `@Chisanan232`, current for the exact head SHA. Add the
marker and heading below; update this decision in place rather than posting
competing current decisions. Preserve other comments and history. Use all fields
shown; comma-separated reasons must exactly match the PR's `reason:*` labels:

```text
<!-- local-agent-triage -->
## Triage Decision
- Status: triage:deferred
- Reason: reason:low-priority
- Evidence: Current evidence and why this disposition fits.
- Resume condition: Concrete event or condition that returns this to delivery.
- Next review date: 2026-11-20
- Related Jira / PR dependencies: Jira keys and PR numbers, or None.
- Next action: The next owner action, or None.
- Head: <full 40-character head commit SHA>
```

The status in the comment must match the sole triage status label. Review dates use
ISO `YYYY-MM-DD`; ordinary deferrals start with a 14-day review window. Set an
earlier date for a security concern when its facts warrant it, and state that basis
in the evidence. A missing review date, a changed head, or an elapsed review date
requires a fresh decision, not silent indefinite parking.

Reassess when CI changes, a prerequisite merges, an upstream fix lands, an advisory
changes, a risk review expires, milestone scope needs the work, or the scheduled
review date arrives. Remove stale triage and reason labels and supersede the old
current decision; return resumed work to `triage:active` and ordinary delivery
without requiring founder approval. If a blocker still exists, update its evidence,
resume condition, next action, review date and exact head. Do not use a triage label
or elapsed time to clear a real security, compatibility, or required-review finding.

`scripts/github/pr_triage.py` is an optional, manual validator for a JSON array of
PR metadata on stdin. It checks exclusive status, reason labels, the marked owner
comment, structured fields, head and review date; overdue and changed-head decisions
are warnings for human reassessment. It neither fetches nor mutates GitHub state.
It is not a CI, merge, acceptance, or release gate and does not monitor or schedule
reviews. No recurring monitor is implied; only if requested, a session-bound Cron
reminder may be used for at most seven days, and it is not persistent enforcement.

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
agent transcripts or restarting an exhausted request.

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
