# Architecture

## Compile-time feature graph

From the root `Cargo.toml`:

| Feature | Meaning |
| --- | --- |
| `agent-core` | The agent runtime dependencies: agent loop, tools, persistence. Without it you get the kernel (config, providers, memory, CLI chat). |
| `daemon-listeners` | The inbound listener code: the daemon and relay commands and the mTLS WebSocket server. |
| `agent-runtime` | `agent-core` plus `daemon-listeners`. The standard runtime. |
| `local-agent-minimal` | Exactly `agent-core`. A marker feature: combining it with an ingress, bundle or `channel-*` feature is a `compile_error!`. |

The standard `default` set also includes `default-channels`, `acp-bridge`, `gateway` and `observability-prometheus`; the minimal profile selects none of them. Details and the verification commands are in [Minimal runtime profile](../setup/local-agent-minimal.md).

The runtime crate still contains relay, enrollment and local RPC listener source that nothing reachable from this profile calls. The linker drops their entry points, and the profile page names the check that asserts it.

## Decisions

[Local Agent ADR-0001: Upstream foundation (proposal)](../architecture/decisions/ADR-0001-upstream-foundation.md) records why the derivative is built on upstream. The upstream ADRs are kept under Upstream with their attribution.

## Inherited architecture pages

These describe the standard build, including channels, gateway and daemon. Where they differ from the profile page, the profile page wins.
