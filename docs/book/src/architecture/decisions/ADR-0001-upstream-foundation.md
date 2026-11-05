---
id: ADR-0001
title: Preserve ZeroClaw ownership and stage a CLI-only derivative
date: 2026-11-02
status: proposed
relates-to:
  - HORO-2380
  - HORO-2369
  - docs/book/src/foundations/fnd-002-documentation-standards.md
  - crates/zeroclaw-runtime/src/agent/loop_.rs
---

# ADR-0001: Preserve ZeroClaw ownership and stage a CLI-only derivative

- Ticket: HORO-2380; milestone: HORO-2369.
- Status: reviewed foundation proposal; mandatory baseline and release gates remain open.
- Baseline: `9dbb57231f48e26fee56243de4f7845924ce4f04` (ZeroClaw `0.8.7`).
- Runtime modifications: none in this ADR.

## Context

Local Agent is an independent local-first derivative, not a new agent runtime or
an official ZeroClaw distribution. The product excludes messaging-channel
adapters, web gateway/webhook ingress and unsolicited remote control, but retains
interactive CLI use, planning, real tool execution, memory, MCP, Skills, optional
plugins, scheduling, subagents and recovery. Browser/desktop capabilities and
optional AASM governance arrive behind later milestone gates.

Current source is a Rust 2024 workspace requiring Rust 1.96.0. Its default root
features include the agent runtime, gateway, ACP bridge and messaging adapters.
A bare kernel build without `agent-runtime` is not a substitute for the product.

## Source ownership

Paths below are relative to the pinned repository, not a promise that later
upstream commits keep identical internals.

| Capability | Existing owner and inspection entry |
| --- | --- |
| CLI/message entry | `crates/zeroclaw-runtime/src/agent/loop_.rs`, `run` and `process_message` |
| Shared provider/tool turn | `crates/zeroclaw-runtime/src/agent/turn/mod.rs`, `run_tool_call_loop` |
| Execution and approval | `agent/tool_execution.rs` and `agent/turn/approval_gate.rs` in the runtime crate |
| Registry construction and scope | Runtime `tools/mod.rs` and `tools/scoped.rs` |
| MCP lifecycle and calls | `crates/zeroclaw-tools/src/mcp_client.rs` and `mcp_tool.rs` |
| Skills discovery and audit | Runtime `skills/mod.rs` and `skills/audit.rs` |
| WIT/Wasmtime host | `crates/zeroclaw-plugins/src/lib.rs` and `component.rs` |
| Feature selection | Root and owning-crate `Cargo.toml` files |

The upstream architecture map routes request, tool, background-work, state,
config and payload lifecycle documentation. Availability, authorization and
approval are distinct: a registry entry or Skill instruction is not permission.
Do not snapshot live policy into long-lived tool handles or let untrusted tool,
Skill, browser or MCP text override the execution boundary.

## Decision

1. Preserve the existing shared Agent Loop and its provider/tool/memory owners.
   Extend narrow seams rather than implementing another orchestration engine.
2. The original lean experiment used this upstream runtime feature selection:

   ```sh
   cargo build --release --offline --locked --no-default-features --features agent-runtime --bin zeroclaw -j 2
   ```

   It is not the local-only product security boundary. The composed validation
   profile developed under HORO-2385/HORO-2387 selects `local-agent-minimal`, with
   explicit default-deny endpoint/model approval, remote-transport configuration
   refusal and incompatible-feature rejection. Those source commits remain
   separate from this pinned foundation branch and do not pass milestone gates.
   The root graph still includes the channels crate; resolve its feature closure
   before claiming no compiled messaging adapters. CLI chat remains. Runtime tool
   selection, such as an empty optional-tool list, does not remove compiled
   dependencies or separately governed MCP/Skill/plugin activation paths.
3. Prefer existing feature gates for excluded adapters. Delete code only where
   gating cannot enforce product exclusions, with owner/caller review and
   regression tests. Do not churn upstream files solely to pursue size targets.
4. Keep WASM optional and inactive by default. Pin upstream WIT/WASI/Wasmtime
   contracts; separately verify component/backend/platform compatibility. No
   claim of universal plugin compatibility; messaging plugins remain excluded.
5. Preserve standalone operation. Define a versioned policy interface for later
   AASM integration, using existing AASM contracts when inspected and suitable.
   Do not duplicate AASM, silently downgrade managed mode, or pre-approve actions
   solely in prompts. No AASM implementation is introduced in this ADR.
6. Keep browser, desktop, scheduler and worker processes lazy and bounded.
   Screenshot/recording bytes stay local and must not enter external model
   requests. They require explicit product lifecycle and permissions tests.

## Falsifiable preservation and optimization checks

- The resolved graph retains `agent-runtime`, providers, memory and tool parsing;
  excludes gateway, ACP bridge, messaging adapters and unrequested WASM backends.
- CLI requests traverse the existing shared turn loop, round-trip a synthetic tool
  result and finish. Exercise native and text tool-call protocols where claimed.
- Denied tools do not execute. Approval-required calls fail closed without an
  approver. Shell/workspace/path restrictions survive feature changes.
- No configured/granted MCP server means no MCP tool. Explicitly granted stdio
  MCP tools remain scoped, authorized, cancellable and bounded by timeouts.
- Instruction-only Skills retain documented precedence. Script-bearing or
  malicious Skills do not gain execution authority; prompt compaction is not an
  isolation boundary.
- No messaging/gateway startup listener appears for the supported CLI profile;
  inspect owned process trees and verify deterministic cleanup.
- Compare size, idle/aggregate RSS, CPU and startup using the same recorded
  baseline method. The initial 25 MB, 60 MB and 500 ms targets are not findings.
  Preserve functionality if targets need an evidence-backed deviation.

These are acceptance checks, not a complete passing gate. Initial locked offline
resolution stopped at unavailable `clap-markdown`; permitted dependency access
subsequently enabled a clean default baseline build and focused real CLI/runtime
verification. The cumulative runtime suite remains red on two inherited relay
fixture setup failures, and original baseline Clippy also fails. No focused pass,
helper self-test, source graph or approved provenance tag overrides those results.
Resource measurements distinguish provider-waiting from idle; unsupported targets
remain unverified until actually exercised.

## Alternatives and pivot rules

A kernel-only build is rejected because it omits the real Agent Loop. Runtime-only
tool catalog narrowing is useful policy but is not a binary-size optimization.
A new runtime or full governance engine is rejected as duplicated ownership.

NullClaw remains a contingency, not a selected base. Its canonical source,
licenses, maintenance state, security boundaries and compatibility have not been
verified here. A pivot requires verified provenance, comparable real measurements,
all cumulative capability/denial journeys, maintenance analysis and founder
approval. Build-cache absence or a missed size target alone does not justify it.

## Upstream synchronization and divergence

Keep the exact baseline and full local ancestry. Make ticket-sized commits on
isolated branches; preserve licenses, notices and attribution. Track each changed
upstream seam and its rationale through source commits and existing Jira tickets.
Evaluate upstream security fixes first, then review candidate commits and rerun affected journeys
before proposing a cumulative sync PR. Do not merge arbitrary moving upstream
HEAD or silently regenerate lockfiles/features.

The project destination is `horonomy/local-agent`; upstream remains a separate
read-only source. Publication must preserve ancestry and satisfy data-handling
and inherited-workflow safety requirements. A source-transfer approval does not
authorize restricted evidence or execution artifacts. Keep operational access,
transfer status and authorization records outside the public architecture record.
No automatic release, package publication or deployment is authorized by this
ADR. The `mvp-0.0` tag records pristine upstream provenance, not milestone signoff.

## Consequences

Preserving upstream ownership limits duplicate runtime code and keeps future
security fixes attributable. Feature-gated reuse also retains shared crate
coupling and maintenance costs; it does not itself prove resource savings or
permission isolation. The preservation checks and unresolved gates above remain
required before promotion.

### Licensing and rollback

Preserve MIT/Apache license files and NOTICE. The root NOTICE references missing
root `TRADEMARK.md` and `CLA.md`; contributor CLA documentation exists under
`docs/book/src/contributing/cla.md`. Its existence does not settle derivative
IP ownership or grant trademark rights. Review third-party/vendored licenses and
remove misleading official branding through a separately scoped change before
distribution, without discarding attribution.

This documentation-only decision can be superseded by a reviewed ADR. Later
runtime changes must be independently reversible without destructive history
rewrites and must not replace a mandatory failed gate with a waiver.

## References

- [Architecture and contribution map](../../contributing/architecture-map.md)
- [Documentation and ADR standards](../../foundations/fnd-002-documentation-standards.md#6-adr-standards)
- Source: `crates/zeroclaw-runtime/src/agent/loop_.rs`, shared CLI Agent Loop.
- Source: `crates/zeroclaw-runtime/src/agent/turn/mod.rs`, shared turn engine.
- Source: `crates/zeroclaw-tools/src/mcp_client.rs`, MCP transport owner.
- Source: `crates/zeroclaw-runtime/src/skills/mod.rs`, Skills owner.
- Source: `crates/zeroclaw-plugins/src/lib.rs`, optional plugin host.
- Source: `NOTICE`, upstream license and attribution notice.
- [Contributor CLA documentation](../../contributing/cla.md)
