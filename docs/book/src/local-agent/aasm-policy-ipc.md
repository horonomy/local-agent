# AASM: policy decisions, IPC contract and audit

This page is the reference for what is decided, what crosses the local socket and what is recorded. For lifecycle states see [AASM (optional integration)](./aasm.md); for steps see [set up, disable and troubleshoot](./aasm-setup.md). The design record is [ADR-019](../architecture/decisions/ADR-019-policy-decision-contract.md); where this page and the source differ, the source is right. It was written from `policy_contract.rs`, `policy_engine.rs`, `policy_gate.rs`, `aasm_adapter.rs` and `policy_audit.rs` under `crates/zeroclaw-runtime/src/security/`.

## Decisions: allow, ask, deny

One decision is made per tool call, after hooks have rewritten the call and before the approval prompt. The verdicts are ordered `allow` < `ask` < `deny`, and two decisions combine only by taking the stricter.

| Verdict | Meaning | Where it can come from |
| --- | --- | --- |
| `allow` | The call may run. A single-use ticket is minted for exactly this tool and these arguments. | Local engine; AASM `ALLOW`. |
| `ask` | The call may run only if an operator approves it. The prompt is mandatory, even for an `auto_approve` tool or full autonomy. | **Local engine only**, for a tool named in the risk profile's `always_ask`. AASM never produces `ask`: the `PENDING` verdict is treated as a deny and its approval id is dropped. |
| `deny` | The call does not run. The model sees `Tool call not executed: '<tool>' was denied by policy (<code>).` | Local engine, AASM, or any failure to decide. |

In managed mode the local engine runs first. If it denies, that is final and AASM is not queried. Otherwise AASM is queried and the stricter result wins, so AASM can turn an `allow` into a `deny` but a local `deny` can never be turned into an `allow`. The only way a decision is ever widened is by editing the local configuration.

Anything that cannot be decided is a `deny`, never an error the caller could read as permission. That covers a tool name that cannot be normalized, a tool that shares a normalized name with another, wrapper chains deeper than 2, an unknown capability class and a request with a contract version other than 1.

### What the local engine decides

The standalone engine is pure and decides from the agent's risk profile (autonomy level, `allowed_tools`, `deny_all_tools`, `excluded_tools`, `always_ask`) and the MCP servers granted to the agent. In summary (the full rules are ADR-019 D3):

- an exclusion always wins over a grant;
- a tool of unknown capability is denied under every autonomy level;
- network, browser, desktop and extension (plugin) tools are denied unless the tool is named explicitly in `allowed_tools`; a wildcard does not grant them;
- an MCP tool needs a granted server and, under a non-empty allow-list, an explicit listing;
- a read-only session keeps only read, memory and agent-internal tools;
- a skill wrapper is evaluated as the tool it wraps, and a bounded delegate is narrowed by its parent and cannot escalate;
- pipeline steps and deferred tool calls cannot prompt, so only an outright `allow` passes: an `ask` is refused.

Tool names are compared after trimming and ASCII-lowercasing. See [Tools overview](../tools/overview.md) and [Autonomy levels](../security/autonomy.md) for the profile settings.

## Reason codes

The reason is a closed set of codes, never text written by a model, tool or AASM.

| Code | Meaning |
| --- | --- |
| `local_allow` | Allowed (or asked) by the local engine. Also recorded for an allow that came from AASM; there is no AASM-allow code. |
| `local_deny` | Denied by the local engine: autonomy above the profile ceiling, not in the allow-list, MCP not granted, unparseable or ambiguous name, unknown parent, caller list excludes it. |
| `excluded` | An exclusion matched the tool, its skill wrapper or its MCP server. |
| `network_not_granted` | A network, browser or desktop tool is not named in `allowed_tools`. In a read-only session such a tool gets `local_deny` instead, because the read-only check runs first. |
| `unknown_capability` | The tool's capability is unknown, or an extension tool is not named in `allowed_tools`. |
| `aasm_deny` | AASM answered anything other than allow. |
| `authority_unavailable` | AASM could not be reached or the socket was refused; or managed mode cannot be honored (see [Misconfigured](./aasm.md#the-six-lifecycle-states)). |
| `timeout` | No answer within `timeout_ms`. |
| `malformed_response` | Truncated, oversize, undecodable or unexpected frame, or a bad handshake. |
| `contract_version_mismatch` | The request's contract version is not 1. |
| `audit_write_failed` | The decision, or an approval receipt, could not be recorded. |

## Approval receipts and cancellation

An `allow` or `ask` is recorded with a gate-local `decision_id`. A `deny` has none. For an `ask`:

1. The operator is prompted. Answering **granted** writes an approval receipt first, and only then does the ticket become redeemable. If that write fails the ticket stays unredeemable and the call does not run.
2. **Rejected** writes a receipt and drops the ticket, so nothing can run under it.
3. **Cancelled** (the turn was cancelled or timed out before an answer) writes a `cancelled` receipt and drops the ticket.

Each ticket accepts one answer; a second answer is refused and writes nothing. A ticket is redeemed once, for exactly the resolved tool name and arguments digest it was minted for, so nothing between the decision and the execution can swap either. A call with no redeemable ticket does not run. Receipts are scoped to one decision: they name its `decision_id` and carry the response `granted`, `rejected` or `cancelled`. They are not the HMAC [tool receipts](../security/tool-receipts.md), which prove a tool actually ran.

## Decision log

Each decision is appended as one JSON line to `policy-audit.jsonl` in the instance data directory (`<config dir>/data` by default, or `ZEROCLAW_DATA_DIR`; see [Environment variables](../reference/env-vars.md)). The log is local only and is never sent anywhere by Local Agent.

- **Digest only.** Raw arguments and tool output are never written; the record carries `args_sha256`, the hex SHA-256 of the canonical JSON of the arguments (keys sorted, no whitespace, excluding the runtime-injected top-level `approved` key). Identifiers that are long or not plain `[A-Za-z0-9._:-]` are replaced by `h-` and 16 hex characters of their digest.
- **Bounded.** The live file rotates at `max_size_mb` to `policy-audit.jsonl.1`, `.2` and so on, keeping `generations` files. Rotation keeps the total near `max_size_mb * (generations + 1)`. The 64 MiB ceiling is only a `GovernanceConfig::validate` rule: config load demotes validation errors to a warning and the sink does not clamp, so it is **not a runtime guarantee**. `max_size_mb = 0` is invalid and, as written, makes every record fail, so every call is denied `audit_write_failed`. A single record larger than the file limit is refused.
- **Private.** Created `0600` in a `0700` directory (existing ones you own are tightened), opened without following symlinks; a live path that is not a regular file is refused. A `policy-audit.lock` file in the same directory serializes writers across processes.
- **Blocking.** If a record cannot be written the call is denied `audit_write_failed`. A full or unwritable disk stops tool use by design; an unrecorded decision is not admissible. The error text, which can contain paths, is not shown to the model, only the closed code.
- **Optional.** `[governance.audit] enabled = false` turns recording off.

There is no command to delete or export the log in this build; it is ordinary files you manage yourself. Do not attach it to tickets without review: digests of arguments can still confirm a guess.

An illustrative record (a managed call that AASM denied; the digest is the SHA-256 of `{}`):

```json
{"v":1,"ts":"2026-11-14T10:00:00.000Z","session_id":"turn-1","turn_id":"turn-1","call_id":"call-1","agent_alias":"main","parent_agent_alias":null,"capability":"fs_read","tool_name":"file_read","via_skill":null,"mcp_server":null,"args_sha256":"44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a","decision":{"local":"deny","aasm":null,"effective":"deny","reason":"aasm_deny"},"authority":{"mode":"managed","contract_version":1,"aasm_decision_id":null},"outcome":{"executed":false,"success":false,"cancelled":false,"duration_ms":0}}
```

The `decision_id` and `approval` fields are omitted when absent. The value of `local` above is what the current source writes (it carries the effective verdict); see the next section.

### What the record does not tell you

The record is a **decision log**, not an execution log. Read the following before using it as evidence:

- `outcome` is written with fixed zero values at decision time and is not updated after the tool runs. Do not infer from it whether a call ran, succeeded or was cancelled. Cancellation shows only as a `cancelled` approval receipt.
- `authority.mode` is `standalone` or `managed` according to the session's mode, but `decision.aasm` is always `null`. `decision.local` and `decision.effective` both carry the final verdict. To tell an AASM-involved decision apart, use `decision.reason` (`aasm_deny`, `timeout`, `malformed_response`, `authority_unavailable`) and `authority.aasm_decision_id`, which is filled when AASM returned a well-formed id. An allow from AASM is not distinguishable by reason.
- A call denied `authority_unavailable` because managed mode could not be honored at all (build without `aasm-managed`, rejected `[governance]` section) is refused before any record is built, so it is **not** in the log (ADR-019, M3). Only the model-visible refusal shows it.
- The log cannot show what AASM was told beyond the digest, nor why AASM decided as it did.

`decision.aasm` and `outcome` exist in the record type but are not yet populated by the gate. That is a gap in the current source, not a documented guarantee.

## IPC contract and supported versions

| Item | Value |
| --- | --- |
| Policy contract version | `1`. Any other version in a request is denied `contract_version_mismatch`. |
| AASM protocol mirrored | v0.0.1 at AASM revision `f6b0e2b70`. Hand-mirrored; **verified against the AASM source at that revision by comparison, not exercised against a live `aa-runtime`**. Local tests use an in-process double. |
| Transport | Unix domain socket, **client-initiated**. Local Agent connects out; it has no listener in this build. Unix only: there is no Windows named-pipe transport. |
| Connection | A fresh connection per query, so every socket check is repeated every time. No pooling, no cache of a previous allow. |
| Framing | One tag byte, a varint payload length, then a protobuf payload. Frames over 1 MiB (AASM itself accepts 8 MiB) are malformed. |
| Exchange | Connect, verify peer uid, read the runtime's challenge (tag 5), send the handshake proof (tag 5) and the policy query (tag 1) back to back, read the response (tag 1). **No handshake acknowledgement is waited for or modeled**; the AASM source at `f6b0e2b70` has no ack frame (source comparison, not live). |
| Deadline | The whole exchange, including the socket checks, must finish within `timeout_ms` (default 2000, allowed 100 to 10000). |

What is sent for one call: the agent alias as the agent id (organization and team ids are left empty because Local Agent does not know them), the turn id as trace id, the call id as span id, the action type tool call, the resolved tool name, a source of `mcp` or `function`, and for the parent alias, if any, the caller id. The argument field carries a small JSON object with `args_sha256`, the capability, the session id, the skill name and the MCP server name. **Raw arguments are never sent**, and the credential token field is always empty (the `credential_token_file` setting is never read). It is unverified whether AASM policy rules can match anything useful in a digest.

What is read back: only the verdict and an optional decision id. A verdict equal to the numeric allow value allows; every other value denies. The reason text, rule name and approval id from AASM are ignored. The decision id is kept only if it is at most 128 characters of `[A-Za-z0-9._:-]`, so it cannot carry content.

The handshake proof is an Ed25519 signature over the runtime's nonce plus a fixed client version string, with a key derived from the public agent id (the socket file name). It proves that the client speaks the protocol, **not who the client is**. Server authenticity rests on the socket path and uid checks described in [socket requirements](./aasm-setup.md#socket-requirements).
