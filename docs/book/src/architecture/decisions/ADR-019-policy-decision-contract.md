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

- `Unknown` is `Deny`, irrespective of autonomy including `Full`. `Plugin` (WASM, node and subprocess tools) is `Deny` unless the tool is named explicitly in `allowed_tools`, as for `Network`; a wildcard or unset list does not grant it, and `ReadOnly` never admits it.
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

The standalone engine (`policy_engine.rs`) covers only three of these rows: the allow-list (a `__` name is never admitted for containing `__`), composed-name handling, and explicit listing for network-class tools. It is not called by the runtime yet, so every runtime path below is unchanged. The enforcement slice must close the rest itself: `WebFetch` `allowed_domains = ["*"]`, an absent approval manager mapping to `NotRequired`, `cron_run` classification, and skill `shell`/`script` registration without an exclusion check.

| Gap | Evidence |
| --- | --- |
| `default_auto_approve` contains network tools | `zeroclaw-config/src/schema.rs` `default_auto_approve()` lists `web_search_tool`, `web_fetch` and `browser_open` |
| `WebFetch` admits every domain | `default_web_fetch_allowed_domains()` returns `["*"]` (same file) |
| Absent approval manager fails open | `agent/turn/approval_gate.rs` `gate_tool_approval`: `ctx.approval.map(..).unwrap_or(ApprovalRequirement::NotRequired)` |
| `__` names auto-admitted under an allow-list | `tools/delegate.rs` (`list.iter().any(..) \|\| name.contains("__")`) and `zeroclaw-tools/src/tool_search.rs` `is_tool_allowed` |
| Shell-class tools are not all classed as shell | `SHELL_COMMAND_TOOLS` in `zeroclaw-api/src/runtime_traits.rs` names `shell`, `cron_add`, `cron_update`, `schedule` but not `cron_run` (a tool named `cron_run` exists in `tools/cron_run.rs`); no tool overrides `Tool::capability()`, so every built-in is `Unknown` until classified |
| Skill `shell`/`script` tools skip the exclusion check | `skills/mod.rs` registers `"shell" \| "script"` skill tools (`SkillShellTool`) without consulting `excluded_tools`; only elevation targets pass `elevation_target_admitted` |

Engine preconditions and choices:

- Tool, skill and MCP server names must be canonical when they reach the engine: the engine does no case or whitespace folding, and denies (`local_deny`) any name that is empty, has leading or trailing whitespace, contains an uppercase letter, a control character or a non-ASCII character. The enforcement slice must supply canonical resolved names.
- A `__` name whose capability is not `Mcp` is denied; for `Mcp`, the prefix before the first `__` is checked against exclusions, and server names with `__` or underscore edges, and tool parts that are empty, contain `__` or start with `_`, are rejected as ambiguous.
- Under `ReadOnly` autonomy only `FsRead`, `Memory` and `AgentInternal` can be allowed; `Mcp`, `Browser`, `Desktop`, `Network`, `Subagent`, `FsWrite` and `Shell` are denied.
- `ReasonCode` has no Ask-specific code, so an engine-level Ask carries `local_allow`; the verdict, not the reason, forces the prompt.

Not confirmed and therefore not claimed: composed-name collisions between skill and MCP names beyond the `__` auto-admission above.

## Enforcement (HORO-2394 slice B)

The decision is authoritative at tool execution. This section records what the code does; it claims no AASM interoperability, and managed mode still has no transport.

- **Gate placement.** `ScopedToolRegistry`, which only `assemble` can mint, carries a non-optional `PolicyGate`. `prepare_tool_calls` decides each call after hook rewrite and delivery defaults and before the approval prompt. `Deny` returns a structured tool error and the tool never runs. `Ask` forces the prompt past `auto_approve`, `Full` autonomy, the session allowlist, the non-interactive shell exemption and the back-channel fallback; with no approval manager an `Ask` is a deny.
- **Tickets.** An `Allow` or `Ask` mints a single-use `DecisionTicket` keyed on the final call id, the exact resolved registry name and the argument digest (the runtime-injected `approved` key excluded). `execute_one_tool_with_provenance` redeems it before dispatch, so the parallel, sequential and recovery paths are covered. An `Ask` ticket is redeemable only after an operator approved. The tool is resolved with the executor's own resolver.
- **Same-batch activation.** A call that does not resolve at preparation time (a deferred tool an earlier call of the batch activates) is decided at execution with no prompt available: only `Allow` runs; `Ask` and `Deny` both deny.
- **Name rule.** Tool, skill and MCP server names, and every `allowed_tools`, `excluded_tools`, `always_ask` and MCP grant entry, are folded with trim plus ASCII-lowercase before the engine sees them. Names that are empty, non-ASCII or contain control characters are denied (list entries that cannot fold are dropped, which only narrows). Registered tools whose names collide after folding are denied. Tickets bind the exact name.
- **Wrappers.** A tool reporting a delegation target is evaluated as that target, with its own name as `via_skill`: a skill shell or script tool is `shell`, a skill HTTP tool is `http_request`, a skill builtin or MCP wrapper is its target. Excluding the target excludes the wrapper, and the operator must list the target (not only the composed skill name) under an allow-list. Skill shell and HTTP tools are also not registered when their built-in is excluded.
- **Network, browser, desktop.** Under the default configuration (`allowed_tools` unset) these classes are denied, which closes the `default_auto_approve` gap because the deny precedes any approval. Naming the tool in `allowed_tools` is the explicit operator act; `WebFetch` `allowed_domains = ["*"]` then permits any host.
- **MCP.** A `__` name is no longer admitted for containing `__` in `ToolAccessPolicy` or the delegate admission check; composed names must be listed.
- **Per-run caller list.** The caller allow-list is a separate filter that can only remove names; it never supplies the explicit listing the engine requires.
- **Classification.** Every built-in declares a capability; `cron_run`, `cron_add`, `cron_update`, `schedule`, `pipeline` and `git_operations` are `Shell`. A registry test builds the maximal production registry and fails on any `Unknown` tool outside an explicit allowlist (empty). WASM, node and subprocess tools are `Plugin` and need explicit listing in `allowed_tools` (the plugin artifact smoke test lists them); hardware peripheral tools are `Desktop` and need explicit listing. `SHELL_COMMAND_TOOLS` stays prompt guidance and omits `cron_run`, which authors no command.
- **Bounded versus independent.** A bounded delegate runs the caller's tools under the target profile: its gate is the target's profile narrowed by the caller's (parent registered, autonomy cannot exceed the parent's). `spawn_subagent` children inherit the parent's security policy and alias, so their gate equals the parent's. An independent delegate and a cross-agent SOP step are separate authorities that run under their own risk profile with their own registry and gate; `parent_agent_alias` on those turns is correlation, not narrowing, by existing design.
- **Managed mode.** `Config::validate` refuses `mode = "managed"` while `MANAGED_TRANSPORT_COMPILED` is false (HORO-2395 flips it with the transport). A gate minted for managed mode denies every call `authority_unavailable`; there is no fallback to standalone.

- **Per-call class.** `Tool::capability_for_call` lets a tool's class depend on its arguments. `model_switch` is `AgentInternal` except `list_models`, a live provider request, which is `Network`. `escalate_to_human`, `poll`, `reaction` and the hardware `datasheet` tool send outbound traffic and are `Network`.
- **Generic MCP tools.** `mcp_resources` and `mcp_prompts` are `Mcp` and report the servers their arguments name through `Tool::mcp_servers_for_call`. The gate requires each named server to be granted and not excluded, refuses an all-server call while a granted server is excluded, then decides the call as local introspection.
- **Pipeline.** Each `execute_pipeline` child step is decided by the same gate before any step starts (parallel mode decides every step first). No prompt is possible inside the pipeline, so only an outright `Allow` runs: a step that needs explicit network, browser or desktop listing, or that is in `always_ask`, is refused. A bounded delegate's registry does not carry the pipeline tool (its steps would be decided by the parent's gate, which cannot be re-pointed); the child keeps the tools a pipeline would have invoked.
- **`spawn_subagent`.** The child inherits the parent's security policy and alias, and the executor scopes the gate's per-run caller list around each tool call so the child run receives it: a parent narrowed by a caller list cannot spawn a wider child. The scope is a task-local, so a `spawn_subagent` in a production registry is refused when it runs without it (for example inside a task spawned by a parallel pipeline step) rather than spawning an unbounded child.
- **ReadOnly.** `Memory` and `AgentInternal` tools are admitted under ReadOnly by design (observation, local bookkeeping). The mutators among them (`memory_store`, `memory_forget`, `memory_purge`, `sessions_delete`, `sessions_reset`, `cron_remove`) enforce the autonomy level themselves through `SecurityPolicy::enforce_tool_operation`. `sop_execute`, `sop_approve` and `sop_advance` have no such check and drive runs that execute tools, so they are `Subagent` class and denied under ReadOnly; `sop_workshop` is `FsWrite`.
- **Config-writing tools.** `model_routing_config` and `proxy_config` are `FsWrite` and therefore allowed by default under Supervised and Full autonomy; operators who do not want an agent to edit its own routing or proxy settings must exclude them.
- **Ordering and shared state.** Hooks run before the policy decision, so the gate sees the call a hook produced. The ticket book is shared by all turns using one registry and is capped (the oldest tickets are dropped); a stale ticket can only be redeemed by an identical call id, name and argument digest.
- **Eval and test-util.** The eval runner assembles through the real gate with its own default policy; its echo tool is classified. The permit-all gate and raw registry mint exist only under `cfg(test)` or the `test-util` feature, and `crates/zeroclaw-runtime/tests/test_util_hygiene.rs` fails if any workspace manifest enables `test-util` outside `[dev-dependencies]`. A bounded delegate with no root config is denied; only a test fixture can opt out, explicitly.

Honest limits: the audit writer (`DecisionSink`) and `EstopManager` are not wired; the name-collision check covers the registry, not the activated set. Pipeline steps are decided by the gate (only an outright Allow runs) but without a per-step ticket, a prompt or the registry name-collision check; the pipeline call's own ticket authorizes the call, and the steps are separately checked by `decide_step`. `ask_user` sends model-chosen text to an operator-configured channel and stays `AgentInternal`, unlike `escalate_to_human`, which can reach third-party channels and is `Network`. A bounded delegate's gate has no per-run caller list of its own; it is narrowed by the parent's already filtered registry and profile. `model_switch` reports `AgentInternal` as its static class; policy uses the per-call class, so a consumer that filters on the static class sees `list_models` as local. A production `spawn_subagent` called from outside the executor's scope is refused.

## Out of scope

Real aa-runtime interoperability (UNVERIFIED: the wire layout above is read from source at the pinned revision and not exercised against a running runtime); AASM approvals and `PENDING`; `REDACT`; event shipping; browser and desktop governance on real surfaces; tamper-proof audit; emergency stop; Windows managed mode; prompt injection.

## Test vectors

Generated from the implemented codec (`security/aasm_adapter.rs`) and pinned by the unit test `golden_vectors_are_pinned`. Each vector is a complete frame: tag byte, varint length, prost payload. They show what this adapter writes and reads; they were not compared with bytes produced by AASM.

Fixed inputs: the `PolicyRequest` built by the test helper `request()` (agent alias `main`, parent `parent`, capability `shell`, session `sess-1`, turn `turn-1`, call `call-1`, `args_sha256` of `{"command":"ls"}`); handshake nonce = bytes `00..1f`; socket file name `aa-runtime-test-agent.sock`, so the transport key seed is `SHA-256("test-agent")` and `sdk_version` is `local-agent-aasm-adapter/1`.

| Frame | Hex |
| --- | --- |
| `CheckActionRequest` (tag 1) | `01de010a061a046d61696e1a067475726e2d31220663616c6c2d31280232b70112b4010a057368656c6c120866756e6374696f6e1aa0017b22617267735f736861323536223a2234636632393631316136363933343836326632396163666363383137653330623930356331616237336435653635383331343133656236623435346434396462222c226361706162696c697479223a227368656c6c222c2273657373696f6e5f6964223a22736573732d31222c227669615f736b696c6c223a6e756c6c2c226d63705f736572766572223a6e756c6c7d3a081a06706172656e74` |
| `CheckActionResponse` (tag 1; `decision` ALLOW, `reason` "ok", `decision_id` "01-abc") | `010e080112026f6b420630312d616263` |
| `HandshakeProof` (tag 5) | `05da010a386469643a6b65793a7a364d6b75674466666b646478615576554a6967485332694a4a6b546b786547314e70697250596f73694d36647869701240653233313261363666303761646262656636366161616464333465323262346435306335613935336434393430656466356638383630623430626366616365351a404ef43312301d1d71d7558f5d50761b6928d2a533b0ae43575d49a79301e9efdd4dac22aac06094bd4027321cd366370f25d605645fad53ad3d0a27566d7d7408221a6c6f63616c2d6167656e742d6161736d2d616461707465722f31` |

The Ed25519 signature is deterministic (RFC 8032, `ring`), so the proof vector is reproducible. The test double in the same module verifies each proof with an independent `ring` verification call.

## Implementation notes (HORO-2395)

Verified by tests against an in-process double, not against AASM: protocol mirrored from rev `f6b0e2b70`, not verified against a live daemon.

- One connection per query: connect, read challenge, send proof and query, read one response, close. The socket checks therefore run before every query. The whole exchange (connect, handshake, reply) is bounded by `governance.aasm.timeout_ms`.
- Checks before connecting (run on a blocking thread inside the query deadline): absolute path without `..`; every path component is a real directory or the socket itself, never a symlink (`lstat` walk); every ancestor directory is owned by root, the current uid or `expected_peer_uid` and is not group/world writable, except that a sticky directory owned by root or the current uid is accepted as a distant ancestor (for example `/tmp` in front of a private directory) but never as the immediate parent; the socket is a socket owned by the expected uid. After connecting, the peer uid from the kernel (`peer_cred`) must equal the expected uid. Any failure is `Deny` / `authority_unavailable`. A path swapped between check and connect is covered by the directory permission rule and the peer uid check; the peer uid check itself has no automated test (it needs a second uid).
- Operator requirement: place the socket in a `0700` directory owned by the client uid. AASM's default `/tmp/aa-runtime-<id>.sock` is refused by design (`/tmp` is `1777`, and a symlink on macOS), so the runtime must be configured with a private socket path.
- `expected_peer_uid`: AASM accepts only connections from its own uid, so a managed client works only when the runtime runs as the same uid as Local Agent. An `expected_peer_uid` that differs from the client's effective uid can never work and is rejected when the adapter is built (startup refusal), not per call.
- Reason mapping: connect/IO failure or a peer that closes before sending a frame byte is `authority_unavailable`; deadline is `timeout`; truncated frame, oversize frame, bad varint (including a tenth byte above 1, which would overflow a 64-bit length), unknown tag, undecodable payload or a missing or bad challenge is `malformed_response`; any verdict other than `ALLOW` (including `PENDING`, `REDACT`, `UNSPECIFIED`, unknown codes) is `aasm_deny`. `ReasonCode` has no allow code for AASM, so an AASM `ALLOW` is `local_allow` with `source = aasm`.
- Attribution: the reason is a closed code, so an AASM deny of a locally allowed call keeps `aasm_deny`, `source = composed` and the AASM decision id as `authority_ref`; no free text is added.
- Composition (`ComposedAuthority`): the local engine decides first; a local `Deny` is returned without consulting AASM. Otherwise the stricter verdict wins and the source is `composed`. Because AASM has no Ask verdict, an Ask can only come from the local engine.
- Request mapping (a Local Agent choice, UNVERIFIED against AASM policy semantics): `action_type = TOOL_CALL`, `agent_id.agent_id = agent_alias` with empty `org_id`/`team_id`, `caller_agent_id = parent_agent_alias`, `trace_id = turn_id`, `span_id = call_id`, empty `credential_token`, and `ToolCallContext.args_json` carrying only the digest and classification (`args_sha256`, `capability`, `session_id`, `via_skill`, `mcp_server`). AASM policies that inspect argument fields cannot match them.
- Handshake agent id (UNVERIFIED): AASM derives its expected key from the agent id that is part of its socket file name (`aa-runtime-<agent_id>.sock`, read from `aa-runtime/src/ipc/server.rs`). The adapter derives the same id from the configured `socket_path` file name; a socket named differently uses its stem. If the runtime was configured with another agent id, its handshake will reject the proof and every call is denied. There is no configuration key for the agent id in this build.
- Ed25519 uses `ring`, already a dependency of `zeroclaw-runtime`; `prost` became an optional dependency of the same crate (already in `Cargo.lock` as a dev-dependency). `Cargo.lock` is unchanged.
- Not in this build: reading `credential_token_file`, persistent connections, event reports, approvals, heartbeat.

## Consequences

- One decision point and one audit shape for both modes; a managed peer can only tighten.
- Every tool needs a capability class; until HORO-2394 classifies built-ins, `Unknown` is denied by the standalone engine.
- The audit-failure deny makes disk health a tool-availability dependency.
- The wire layout is a pinned copy; AASM protocol changes need a new contract or adapter revision.
