# Minimal runtime profile (`local-agent-minimal`)

The minimal profile builds the Local Agent runtime without any inbound surface:
the Agent Loop (plan, act, observe), memory, tools, MCP and CLI chat only. It is the
profile selected by Local Agent MVP 1.0 (HORO-2381) and the one ADR-0001 refers to
as the composed validation profile.

## Build and run

```sh
cargo build --release --locked --bin zeroclaw \
  --no-default-features --features local-agent-minimal
zeroclaw agent -m "hello"
```

`local-agent-minimal` is exactly `agent-runtime`. It is a marker feature: building it
together with an ingress feature fails at compile time, so `--all-features` on the
root crate intentionally does not build.

## What is excluded

| Surface | Behavior in this profile |
| --- | --- |
| `gateway`, `embedded-web`, `acp-bridge`, webhook and ACP-server channels | Not compiled in. Combining any of them with the profile is a `compile_error!`. |
| Messaging channel adapters (`channel-*`, `default-channels`, `channels-full`) | Not compiled in. A config that names one is refused (see below). |
| `daemon`, `service`, `relay`, `gateway`, `acp`, `channel start` | Refused at startup, before config is loaded or any state is written. |
| `[wss]`, `[relay]`, `[enroll]`, `[a2a.server]`, a tunnel provider | A config that enables one is refused instead of silently ignored. |

`config`, `doctor`, `status`, `self-test`, `auth`, `memory`, `skills` and `security`
remain available so a non-minimal config can be repaired.

## Limits of the claim

- Provider adapters are still compiled in. A single OpenAI-compatible endpoint is a
  configuration choice protected by the existing default-deny approval, not a feature gate.
- The runtime crate still contains its WSS, enrollment, relay and local-RPC code. It is
  unreachable in this profile because the commands and configuration above are refused;
  it is not removed from the binary. The profile does not claim "no listener code linked".
- Provider OAuth callbacks and OIDC enrollment use short-lived, user-initiated
  loopback listeners. They are not gated.
- `zeroclaw` installer `--minimal` and the xtask `Minimal` selection mean the kernel
  build, whose `agent` command is a plain chat loop without tools or memory. That is a
  different profile and must not be reported as this one.

## Verification

```sh
cargo test --locked -p zeroclaw --no-default-features --features local-agent-minimal \
  --test component -- minimal_profile
cargo test --locked -p zeroclaw --no-default-features --features local-agent-minimal \
  --test integration -- agent:: memory_loop_continuity:: memory_restart::
cargo tree --locked -p zeroclaw --no-default-features --features local-agent-minimal \
  -e features -i zeroclaw-channels
```

The last command must show no `zeroclaw-channels feature "channel-*"`. The cumulative
Golden Journey is run with the minimal binary as described in
[Golden journeys runner](../../../local-agent/golden-journeys-runner.md).
