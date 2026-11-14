# AASM: threat model

This is the threat model for the optional managed mode. Lifecycle states are in [AASM (optional integration)](./aasm.md), behavior in [policy decisions, IPC contract and audit](./aasm-policy-ipc.md). Standalone mode has no AASM-related attack surface: it opens no socket and reads no AASM input.

The short version: managed mode is **fail-closed policy tightening for one user's agent**. It stops the model and a misbehaving AASM from doing more than local policy allows. It is **not a boundary against the person or software running as the same user**, and it is not tamper-evident.

## Assets and trust

| Party or asset | Trust |
| --- | --- |
| The model and the tool output it reads | Untrusted. Prompt injection is assumed. |
| Local Agent process and its config file | Trusted. Anyone who can edit them under the same user can change the mode. |
| AASM runtime | A peer whose answers can only restrict. It is not trusted to grant. Its identity rests on the socket checks, not the handshake. |
| Other local users | Untrusted. They are kept out by the socket and log permission checks. |
| Same-user processes | **Not distinguished** from Local Agent or from AASM. |
| The network | No listener exists, so there is no inbound network path. Managed mode makes no network connection of its own. |
| Decision log | Evidence of decisions, for the user. Not tamper-evident. |

## User and administrator boundaries

Local Agent has **no administrator enrollment mechanism** for managed mode. The `local-agent-minimal` profile refuses relay and enrollment sections. Managed mode is controlled entirely by the `[governance]` config, which is a file in the user's own configuration directory (plus the general `ZEROCLAW_*` override), read by a process running as the user.

What that means in practice:

- An administrator can install a build with `aasm-managed`, place a config and run an AASM runtime as the user's account. From then on every tool call is checked, and a missing or unsafe runtime blocks tool use.
- The user can set `mode = "standalone"`, or run another build, and the AASM check is gone. Nothing here prevents it, and Local Agent does not report that it happened to AASM.
- The user can also edit `[governance.audit]`, delete the decision log or turn it off.
- A root-owned runtime socket is rejected, so an AASM runtime run by a different account does not work.

If you need a control that the user cannot remove, managed mode alone does not provide it. Such a control would have to come from outside this product, and none is documented or tested here.

## Threats and mitigations

| Threat | Mitigation in the source | Residual risk |
| --- | --- | --- |
| The model calls a tool the policy does not allow, or one disguised by case, whitespace, a wrapper or an MCP-shaped name | Names are normalized or denied; wrappers are judged as their target; exclusions win; unknown capability is denied | Only as good as the risk profile. Standalone policy is the main control, AASM is an extra. |
| Arguments or tool identity swapped between decision and execution | A single-use ticket binds the resolved tool name and arguments digest; no ticket, no run | None known beyond bugs. |
| AASM, a buggy runtime or a fake answers `allow` to widen access | Stricter-wins: a local `deny` is final and never sent; AASM `allow` cannot upgrade an `ask` or `deny` | A fake can still deny, which is an availability problem only. |
| AASM unreachable, slow or confused | Every failure is a deny with a closed code; no fallback to standalone; a fresh connection each time | Availability: tool use stops. Deliberate. |
| Another local user plants or swaps the socket | No default path; absolute path required; no symlink at any component; ancestors owned by you or root and not group or world writable; socket and peer must be your uid; the check repeats on every query | A narrow window between the path check and `connect`; the peer uid check after connect covers it. |
| A process under your own uid impersonates AASM | None. It can bind the path in your private directory and answer. | **Accepted.** It can only add denials, but it can also feed a false sense that AASM is in the loop. |
| Handshake used as proof of identity | Not used that way: the key derives from the public agent id | The proof shows protocol participation only. |
| Eavesdropping or tampering on the socket | Local Unix socket, `0700` directory | Same-user processes can still read it. No encryption on the socket. |
| Leakage of arguments to AASM | Only a SHA-256 digest, the capability, the tool, skill and server names, the alias and ids are sent. The credential token field is always empty. | Names and ids are visible to the runtime. A digest of a low-entropy argument can be confirmed by guessing. |
| Leakage through the decision log | Digest only; no raw arguments or output; `0600` file in a `0700` directory | Same caveat about guessable arguments. Treat the log as sensitive. |
| Decision log tampered with or removed | Not prevented | Same-user write access means it can be edited or deleted. No hash chain or signature. |
| Filling the disk to stop recording | Rotation bounds the log by `max_size_mb` and `generations`; the 64 MiB ceiling is a validation rule that load only warns about, not a runtime clamp. A write failure denies the call, including `max_size_mb = 0`. | An unwritable log blocks tool use. This trades availability for evidence. |
| Secret exposure through governance config | `credential_token_file` is a path only and is never read in this build; no secret is stored in `[governance]` | Another process's use of the token is outside this build. |
| Mode changed by environment | The general `ZEROCLAW_*` override reaches `[governance]` keys. No AASM-specific variable is read. | Anything that controls the process environment controls the mode. |
| Managed configured in an unsuitable build or platform | Every call is denied; the validation error is logged; no fallback | The session still starts, so you notice by denials, not by a refusal to start. |
| An agent approves its own call | An `ask` needs an operator answer; the receipt is written before the ticket becomes redeemable. A non-interactive step cannot answer an `ask`, so it is refused. | Depends on the approval path being a human one. |
| Halting a running agent from AASM | **Not provided.** `EstopManager` is not wired into this integration. | AASM can only deny the next tool call, not interrupt a running one. |

## Out of scope and unverified

- Compromise of the user account, the Local Agent binary or the build.
- Kernel or sandbox escapes. See [Sandboxing](../security/sandboxing.md).
- The behavior, security or release status of AASM itself.
- Compatibility with a live AASM runtime: the wire layout and handshake are hand-mirrored and unverified.
- Windows: managed mode is not supported.
- AASM's own default socket path is refused by design; whether AASM can be configured to a compliant path is not verified.

## Reader checklists

**Individual user.**

1. If you do not run AASM, change nothing. You are standalone and nothing listens or connects.
2. If you do, follow [Enable managed mode](./aasm-setup.md#enable-managed-mode), then deliberately cause a denial and a disconnect using the [examples](./aasm-setup.md#managed-examples-denial-and-disconnect).
3. Know that you can turn it off, and that doing so is invisible to AASM.

**IT or security administrator.**

1. Decide whether a same-user, user-removable, fail-closed check is enough for your requirement. If not, managed mode is not the control; treat it as one layer.
2. Provision an `aasm-managed` build and the config only where an AASM runtime runs as the same user, in a private directory, on Unix. Do not request root-owned sockets, open ports or a listener: they are neither needed nor accepted.
3. Verify against your real AASM version before rollout, since the wire layout has not been checked against a live runtime. Expect `malformed_response` or `authority_unavailable` if it disagrees.
4. Plan for the availability trade: a stopped runtime or a full log directory stops tool use.
5. Do not rely on the audit `outcome` and `decision.aasm` fields; see [what the record does not tell you](./aasm-policy-ipc.md#what-the-record-does-not-tell-you).
