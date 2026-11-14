# AASM (optional integration)

Local Agent decides every tool call itself. AASM is an optional second authority that can only make a decision stricter. You never need it: the default configuration is **standalone**, needs no AASM software, opens no listener and contacts nothing.

This page covers what the integration is, its six lifecycle states and what it does not claim. The rest of the section:

- [Set up, disable and troubleshoot](./aasm-setup.md): the standalone quickstart, enabling managed mode, switching it off, and a symptom table.
- [Policy decisions, IPC contract and audit](./aasm-policy-ipc.md): allow, ask, deny, audit, reason codes, approval receipts, cancellation and the local socket protocol.
- [Threat model](./aasm-threat-model.md): what managed mode defends against and what it cannot.

The architecture decision is [ADR-019](../architecture/decisions/ADR-019-policy-decision-contract.md). This section describes behavior of the source on `release/mvp-3.0`. It is **not part of the published MVP-1.0 or MVP-2.0 artifacts**, and this book does not claim any published binary includes the managed transport.

## What it is

- **Standalone** (default): the local policy engine alone decides, from the agent's risk profile.
- **Managed** (opt-in): the same local engine, composed with a query to an AASM runtime over a local Unix socket. The stricter answer wins (`deny` over `ask` over `allow`), so AASM can add restrictions but can never grant what local policy withheld. A local deny is final and is not even sent to AASM.

Managed mode needs two things at once: a build with the Cargo feature `aasm-managed` (not in the default build and not in `local-agent-minimal`), and `[governance] mode = "managed"` in the config. Local Agent is only a client: it connects out to a socket you name and never listens.

## The six lifecycle states

| State | What is true | What Local Agent does |
| --- | --- | --- |
| Not installed | No AASM on the machine, or a build without `aasm-managed`. | Standalone. Nothing to configure. This is the supported default. |
| Installed, disabled | AASM exists on the host, but `[governance]` is absent or `mode = "standalone"`. | Standalone. It never opens the AASM socket, even in a build that contains the managed transport. |
| Standalone | `mode = "standalone"` (also what an absent section means). | Local engine only. Decisions carry source `local`. |
| Managed | `mode = "managed"`, build has `aasm-managed`, Unix, valid `[governance.aasm]`, reachable runtime. | Local engine composed with AASM through the stricter-wins rule. Each tool call opens a fresh connection and is checked again. |
| Misconfigured | `mode = "managed"` but the build lacks `aasm-managed`, the platform is not Unix, `socket_path` is missing or relative, `timeout_ms` is out of range, `expected_peer_uid` differs from the process uid, or the socket file name yields no agent id. | The configuration check fails with a warning at config load (not a startup refusal) and the session still starts, but the policy gate is built in a deny-everything state: every tool call is denied `authority_unavailable`. There is no fallback to standalone. |
| Disconnected | Managed and configured correctly, but the runtime is stopped, slow, replaced by an unsafe socket, or sends something malformed. | The session keeps running. Each call that reaches AASM is denied: `authority_unavailable`, `timeout` or `malformed_response`. The next call reconnects, so service resumes without restarting Local Agent. |

Two details that matter:

- The mode is fixed when the session's tool registry is assembled, from configuration only. It does not change mid-session.
- Locally denied calls do not depend on AASM at all, so a disconnected AASM never turns a local deny into anything else.

## Who reads what

**Individual user, no AASM.** Stay on the defaults. Read the [standalone quickstart](./aasm-setup.md#standalone-quickstart-no-aasm) to confirm nothing here applies to you. Nothing on this page needs action.

**Individual user, AASM already running on your machine.** Read [Enable managed mode](./aasm-setup.md#enable-managed-mode) and [Disable managed mode](./aasm-setup.md#disable-managed-mode). You control your own config, so you can also switch it off; see the [threat model](./aasm-threat-model.md#user-and-administrator-boundaries).

**IT or security administrator.** Read the [threat model](./aasm-threat-model.md) first, then [policy decisions and audit](./aasm-policy-ipc.md). Managed mode here is a same-user control with fail-closed behavior, not a tamper-proof enforcement boundary. Decide whether that is enough before relying on it.

## What is and is not claimed

Implemented and covered by tests in the source (cited in the linked pages):

- standalone and managed authority selection, the `[governance]` validation rules and the 64 MiB audit-size validation rule (a warning at config load, not a runtime clamp);
- fail-closed denial for every adapter error, timeout, malformed or oversize frame, and any AASM verdict other than allow;
- the local-only, digest-only decision log and the single-use approval receipts.

Not verified, and not claimed:

- **The AASM wire layout and handshake are not verified against a live `aa-runtime`.** The messages are hand-mirrored from AASM revision `f6b0e2b70` (protocol v0.0.1) and exercised only against an in-process test double. A real runtime may disagree, in which case every call denies (`malformed_response`, `authority_unavailable` or `timeout`).
- Whether a stock AASM runtime can listen on a path Local Agent accepts (see [socket requirements](./aasm-setup.md#socket-requirements)). AASM's own default `/tmp` path is refused by design.
- Whether AASM policy rules can do anything useful with the request: Local Agent sends a digest, not the arguments.
- **Same-user trust.** The socket owner and the peer must be the same user as Local Agent. A root-owned runtime socket is rejected. Another process under your own user is not distinguished from AASM.
- **No emergency-stop claim.** `EstopManager` is not wired into this integration, so nothing here can halt a running agent on AASM's say-so beyond denying the next tool call.
- **Windows.** Managed mode is Unix only. A named-pipe transport does not exist; `mode = "managed"` on a non-Unix platform fails validation.
- Some audit-record fields are present but not populated yet; see [what the audit record does not tell you](./aasm-policy-ipc.md#what-the-record-does-not-tell-you).
