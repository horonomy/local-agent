# Contributing to Local Agent

Local Agent (Sakura Shrimp) is an independent ZeroClaw-derived project at
`horonomy/local-agent`. Keep upstream licenses, notices, contributor attribution
and historical architecture decisions intact. This page changes engineering
workflow, not licensing or legal contribution terms.

## Start with the project contract

Read [AGENTS.md](AGENTS.md) and the
[engineering lifecycle](docs/local-agent/engineering-lifecycle.md). They define
Local Agent workflow where inherited ZeroClaw contribution or team procedures
differ. Use the [architecture map](docs/book/src/contributing/architecture-map.md)
to load relevant technical references rather than the entire documentation tree.

1. Reconcile the existing approved ticket and acceptance criteria before coding.
2. Reuse or create the appropriate isolated ticket worktree from the verified
   milestone branch. Preserve staged changes, other writers and valid worktrees.
3. Implement the smallest complete increment and add relevant regression coverage.
4. Run focused tests, formatting and scoped lint/type checks; fix findings and
   obtain independent technical review.
5. Make small Gitmoji commits, push reviewed source when authorized controls permit,
   and verify the remote SHA. Ticket PRs target `release/mvp-X.0`; release PRs target
   the verified default branch (currently `master`). Never direct-push to it.
6. Integrate through the single-maintainer owner-review workflow only after
   independent technical review, applicable CI, actual configured approvals,
   conflict, branch-currency and thread gates pass. Use merge commits, not squash.

Do not follow inherited instructions to delete a branch, force-push, skip a hook,
write to upstream or deploy through ZeroClaw infrastructure. Such instructions
are not Local Agent setup steps.

## Risk-based verification

| Stage | Evidence |
| --- | --- |
| Implementation | Focused deterministic tests at the changed behavior, a bug regression, relevant formatting and scoped lint/type checks. |
| PR integration | Relevant integration/regression and feature-combination tests, exact-head CI coverage, independent review and required approvals. |
| Release checkpoint | Complete required suite, cumulative Golden Journeys, security/licenses, supported platforms, documentation builds/parity and independent QA at the candidate SHA. |

Avoid workspace-wide rebuilds after each small change. Reuse unchanged evidence
only when its revision and scope still match. Missing, failed or skipped checks
must be reported as such; focused passes never establish release readiness.
Workstation-enforced receipts and permissions remain separate requirements, not
something this repository document can disable.

A scoped Rust example is:

```sh
cargo test --offline --locked -p <owner-crate> --test <target>
cargo clippy --offline --locked -p <owner-crate> --all-targets -- -D warnings
cargo fmt --all -- --check
```

Use the existing actual hook configuration. This checkout has `.githooks/`, not a
repository `.pre-commit-config.yaml`; inspect hooks before trusting or installing
them. No bypass flags, fabricated passing receipts or automatic new dependencies.

## Documentation is part of the change

Assess documentation impact for every behavior change. Update affected source in
the same PR or a clearly linked blocking PR. Validate documented commands,
release-profile claims, navigation, internal links, generated references and real
mdBook builds. The vulnerable third-party Markdown formatter is retired; never
execute its old invocation merely to satisfy inherited instructions.

The [engineering lifecycle](docs/local-agent/engineering-lifecycle.md) owns current
documentation policy. Existing scripts and CI need the separately reviewed tool
retirement change before their old formatter path is safe to run. Meaningful link,
build and product-parity checks remain required. Keep ADRs in
`docs/book/src/architecture/decisions/` and do not rewrite accepted upstream history.
No documentation deployment through ZeroClaw's domain, accounts or credentials.

## Governance and security

Ordinary approved features and bounded fixes do not require a ZeroClaw Core Team
vote. Material security, authority or cross-component ownership changes need a
durable design decision and appropriate review. `@Chisanan232` is the accountable
owner under the single-maintainer contract in the engineering lifecycle. Use
independent Claude Code or Codex technical review, not fictional GitHub human
approval. Actual configured approval rules remain binding and require authorized
administrative reconciliation if they conflict with this model.

Preserve sandboxing, explicit tool approvals, default-deny provider endpoint/model
authorization and closed remote surfaces in the minimal profile. A configured key
is not approval. Standalone Local Agent must not require AASM.

Never commit secrets, real-user fixtures, private workstation configuration,
corporate data, raw execution logs, model/session captures or generated reports.
Use synthetic inputs and permitted text-only evidence. Captured visual media stays
local under workstation rules, including restrictions on model-context loading.
A leak requires credential rotation and an authorized remediation plan; history
rewrites, force-pushes and external deletion are not automatic recovery steps.

## License and attribution

Preserve the upstream license files: [MIT](LICENSE-MIT) and
[Apache 2.0](LICENSE-APACHE), plus [NOTICE](NOTICE). The inherited
[CLA record](docs/book/src/contributing/cla.md) remains attribution/legal context;
this workflow update does not create or change legal contribution terms.
