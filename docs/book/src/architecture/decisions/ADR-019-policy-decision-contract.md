---
id: ADR-019
title: Tool calls are decided through a versioned policy decision contract with an optional AASM adapter
date: 2026-11-12
status: proposed
relates-to:
  - ADR-011
  - ADR-018
  - crates/zeroclaw-api/src/policy.rs
  - crates/zeroclaw-runtime/src/security/policy_contract.rs
  - crates/zeroclaw-config/src/governance.rs
---

# ADR-019: Tool calls are decided through a versioned policy decision contract with an optional AASM adapter

## Context

Local Agent (Local Agent MVP 3.0, epic HORO-2372) must decide every tool call from one place, whether it runs alone or beside an AASM (Agent Assembly) runtime. Today authority is spread across `SecurityPolicy`, `allowed_tools`, the approval gate and per-tool checks, and the tool name the model emits is not always the tool that runs (skill wrappers, MCP composed names). A tool that declares no capability cannot be classified at all.

AASM is optional. It may add restrictions; it must never add permissions that local policy withheld, and an unreachable or confusing AASM must never become an allow. This ADR fixes the contract, the data model and the fail-closed table. It ships types, configuration and an authority factory only (HORO-2393). The standalone engine, the enforcement point and the audit writer are HORO-2394; the managed transport is HORO-2395.

Pinned versions: contract v1. AASM protocol v0.0.1 at rev `f6b0e2b70` (read-only reference `proto/ipc.proto`, `proto/policy.proto`, `aa-sdk-client/src/{codec,decision,keypair,config,client}.rs`).

## Decision

### D1. Data model

- `ToolCapability` (`zeroclaw-api`, `#[non_exhaustive]`, serde snake_case): `fs_read`, `fs_write`, `shell`, `network`, `memory`, `mcp`, `subagent`, `browser`, `desktop`, `plugin`, `agent_internal`, `unknown`. `Tool::capability()` defaults to `Unknown`; an unclassified tool is never granted implicitly.
- `PolicyRequest` (`zeroclaw-runtime::security::policy_contract`): `contract_version` (1), `session_id`, `turn_id`, `call_id`, `agent_alias`, `parent_agent_alias`, `capability`, `tool_name` (resolved canonical name), `via_skill`, `mcp_server`, `args_sha256`, `autonomy`, `interactive`. `args_sha256` is the hex SHA-256 of the canonical JSON (sorted keys, no whitespace) of the arguments without the runtime-injected top-level `approved` key. Raw arguments never leave the process.
- `PolicyDecision`: `verdict` (`allow` | `ask` | `deny`), `reason` (closed `ReasonCode` enum, never model-authored text), `source` (`local` | `aasm` | `composed`), `authority_ref` (opaque id from the authority).
- Reason codes: `local_allow`, `local_deny`, `aasm_deny`, `authority_unavailable`, `unknown_capability`, `excluded`, `network_not_granted`, `contract_version_mismatch`, `audit_write_failed`, `timeout`, `malformed_response`.
- Composition: `strictest(a, b)` with `Deny > Ask > Allow` is the only way decisions combine. AASM can never widen a local decision. On a tie the local decision is kept.
- A request whose `contract_version` is not 1 is rejected as `Deny` / `contract_version_mismatch`.
- Traits: `PolicyAuthority::decide(&PolicyRequest) -> PolicyDecision` (async, total: failing to decide is a deny) and `DecisionSink::record(&DecisionRecord) -> Result<()>`.

### D2. Enforcement points

Decide in `prepare_tool_calls`, after hook rewrite and delivery defaults, before `gate_tool_approval`. Deciding earlier would evaluate arguments a hook can still change; deciding later lets approval prompts run for calls that were never admissible. The decision yields a `DecisionTicket { resolved_name, args_sha256, decision_id }`. The executor re-checks the ticket against the call it is about to run and denies on any mismatch, so nothing between decision and execution can swap the tool or its arguments.

### D3. Standalone engine rules (implemented in HORO-2394)

- `Unknown` and `Plugin` capabilities that are not granted are `Deny`, irrespective of autonomy including `Full`.
- An exclusion always wins over a grant.
- `Network`, `Browser` and `Desktop` are `Deny` unless the tool is named explicitly in `allowed_tools`; a wildcard does not grant them.
- A skill wrapper is evaluated as its target tool.
- `Mcp` needs a bundle grant and, under a non-empty `allowed_tools`, an explicit listing.
- Subagent authority is the parent's authority narrowed; a subagent cannot escalate.
- A policy `Ask` forces a prompt even under `auto_approve` or `Full`.

### D4. Managed mode (AASM adapter)

- Local Agent is the client. It connects to the AASM runtime's Unix socket; it opens no inbound listener. Unix only.
- Framing: `[1-byte tag][varint length][prost payload]`, 1 MiB maximum frame (AASM accepts 8 MiB; Local Agent is stricter and treats a larger frame as malformed). Tags written by the client: `1` PolicyQuery (`CheckActionRequest`), `5` handshake proof (`HandshakeProof`). Tags read: `1` PolicyResponse (`CheckActionResponse`), `5` handshake challenge (`HandshakeChallenge`). Unused in v1: `2` EventReport, `3` ApprovalResponse, `4` heartbeat.
- Handshake: the runtime sends `HandshakeChallenge { nonce = 1 }`; the client replies `HandshakeProof { agent_did = 1, public_key = 2, signature = 3, sdk_version = 4 }` where `signature` is Ed25519 over `nonce || sdk_version`. The transport key derives from the public agent id (`SHA-256(identifier)` as seed), so the handshake proves protocol participation and authenticates no one; server authenticity rests on the socket checks below.
- Messages used: `CheckActionRequest { agent_id = 1, credential_token = 2, trace_id = 3, span_id = 4, action_type = 5, context = 6, caller_agent_id = 7 }` and `CheckActionResponse { decision = 1, reason = 2, policy_rule = 3, approval_id = 4, redact = 5, decision_latency_us = 6, agent_identity_assurance = 7, decision_id = 8 }`. `Decision`: `UNSPECIFIED=0, ALLOW=1, DENY=2, PENDING=3, REDACT=4`. The mapping from `PolicyRequest` to `ActionContext` (the tool-call context carries a digest, not raw arguments) is specified with the transport in HORO-2395.
- Dependencies: do not depend on `aa-sdk-client` or `aa-proto`. Hand-mirror the needed prost field subset behind the non-default `aasm-managed` feature.
- Server authenticity: explicit absolute `socket_path` (no default, no `/tmp` fallback); the parent directory must not be group or world writable and must be owned by the current uid or root; the connected peer uid must equal `expected_peer_uid` (default: current uid).
- Local Agent never reads AASM environment variables. All managed-mode inputs come from `[governance]`.
- `credential_token_file` is presence-only configuration in this ticket; nothing reads it.

### D5. Configuration

`[governance]` (`zeroclaw-config::governance`), default standalone, so existing configurations are unchanged:

| Key | Default | Rule |
| --- | --- | --- |
| `mode` | `standalone` | `standalone` or `managed`; managed is rejected on non-Unix |
| `aasm.socket_path` | none | required in managed mode, absolute |
| `aasm.expected_peer_uid` | current uid | |
| `aasm.timeout_ms` | 2000 | 100..=10000, checked in every mode |
| `aasm.credential_token_file` | none | presence-only |
| `audit.enabled` | true | |
| `audit.max_size_mb` | 8 | at least 1 |
| `audit.generations` | 3 | `max_size_mb * (generations + 1)` at most 64 MiB |

The mode is pinned at session start from configuration only and never changes mid-session.

### D6. Fail-closed table and audit

| Condition | Result |
| --- | --- |
| Managed, authority unreachable at start | Session runs; every tool call is `Deny` / `authority_unavailable` |
| Timeout | `Deny` / `timeout` |
| AASM `DENY`, `PENDING`, `REDACT`, `UNSPECIFIED`, unknown code | `Deny` (`aasm_deny`; `PENDING` and `REDACT` are unsupported in v1) |
| Malformed frame, oversize frame, bad handshake | `Deny` / `malformed_response` |
| Contract version mismatch | `Deny` / `contract_version_mismatch` |
| Managed configured, build lacks `aasm-managed` | Refuse startup |
| Managed unavailable | No fallback to standalone |
| Decision record cannot be written | `Deny` / `audit_write_failed`, both modes |

The last row trades availability for evidence: a full or unwritable disk stops tool use. That is deliberate; an unrecorded decision is not admissible.

`DecisionRecord` (`policy-audit.jsonl`, `v: 1`) holds `ts`, `session_id`, `turn_id`, `call_id`, `agent_alias`, `parent_agent_alias`, `capability`, `tool_name`, `via_skill`, `mcp_server`, `args_sha256`, `decision { local, aasm?, effective, reason }`, `authority { mode, contract_version, aasm_decision_id? }` and `outcome { executed, success, cancelled, duration_ms }`. Raw arguments and tool output are never recorded. The file is separate from other audit logs and bounded by `[governance.audit]`.

In this ticket only the types, configuration and an authority factory exist. The factory fails closed for managed mode ("managed mode unsupported in this build") and returns an explicit `Unimplemented` error for standalone until HORO-2394; no production code calls it, so the shipped binary's behavior is unchanged.

## Known gaps HORO-2394 must close

- `default_auto_approve` includes `web_fetch`, `web_search_tool` and `browser_open`.
- `WebFetch` defaults `allowed_domains` to `["*"]`.
- Skill `shell`/`http`/`script` tools register even when `shell` is excluded.
- Names containing `__` are auto-admitted under a non-empty `allowed_tools`.
- Composed-name collisions (skill and MCP names).
- An absent approval manager maps to `NotRequired`, which fails open.
- MCP grandchild processes survive teardown.
- `cron_run`, `cron_add`, `cron_update` and `schedule` must be classed `Shell`.
- `EstopManager` is not enforced.

### Verified gap evidence (checked against the code at `36f2a323d`)

The standalone engine (`policy_engine.rs`) refuses each of these by construction but is not yet called by the runtime, so the runtime paths below are unchanged and must be closed by the enforcement slice:

| Gap | Evidence |
| --- | --- |
| `default_auto_approve` contains network tools | `zeroclaw-config/src/schema.rs` `default_auto_approve()` lists `web_search_tool`, `web_fetch` and `browser_open` |
| `WebFetch` admits every domain | `default_web_fetch_allowed_domains()` returns `["*"]` (same file) |
| Absent approval manager fails open | `agent/turn/approval_gate.rs` `gate_tool_approval`: `ctx.approval.map(..).unwrap_or(ApprovalRequirement::NotRequired)` |
| `__` names auto-admitted under an allow-list | `tools/delegate.rs` (`list.iter().any(..) \|\| name.contains("__")`) and `zeroclaw-tools/src/tool_search.rs` `is_tool_allowed` |
| Shell-class tools are not all classed as shell | `SHELL_COMMAND_TOOLS` in `zeroclaw-api/src/runtime_traits.rs` names `shell`, `cron_add`, `cron_update`, `schedule` but not `cron_run` (a tool named `cron_run` exists in `tools/cron_run.rs`); no tool overrides `Tool::capability()`, so every built-in is `Unknown` until classified |
| Skill `shell`/`script` tools skip the exclusion check | `skills/mod.rs` registers `"shell" \| "script"` skill tools (`SkillShellTool`) without consulting `excluded_tools`; only elevation targets pass `elevation_target_admitted` |

Not confirmed and therefore not claimed: composed-name collisions between skill and MCP names beyond the `__` auto-admission above.

## Out of scope

Real aa-runtime interoperability (UNVERIFIED: the wire layout above is read from source at the pinned revision and not exercised against a running runtime); AASM approvals and `PENDING`; `REDACT`; event shipping; browser and desktop governance on real surfaces; tamper-proof audit; emergency stop; Windows managed mode; prompt injection.

## Test vectors

Golden hex vectors for `CheckActionRequest`, `CheckActionResponse` and the handshake proof are not recorded here. They must be generated from the mirrored prost types, and the proof needs an Ed25519 implementation. TODO owned by HORO-2395.

## Consequences

- One decision point and one audit shape for both modes; a managed peer can only tighten.
- Every tool needs a capability class; until HORO-2394 classifies built-ins, `Unknown` is denied by the standalone engine.
- The audit-failure deny makes disk health a tool-availability dependency.
- The wire layout is a pinned copy; AASM protocol changes need a new contract or adapter revision.
