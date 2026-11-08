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

`local-agent-minimal` is exactly `agent-core`: the standard runtime's dependencies
without `daemon-listeners`, the feature that carries the daemon's inbound servers.
The standard `agent-runtime` is `agent-core` plus `daemon-listeners`. The profile is a
marker feature: building it together with a listener, bundle or any `channel-*` feature
fails at compile time, so `--all-features` on the root crate intentionally does not build.

## What is excluded

| Surface | Behavior in this profile |
| --- | --- |
| `daemon-listeners`, `gateway`, `embedded-web`, `acp-bridge` | Not compiled in. Combining any of them with the profile is a `compile_error!`. The mTLS WebSocket server (`rpc::wss`) and the daemon and relay command bodies exist only with `daemon-listeners`. |
| Every `channel-*` feature and the bundles `default-channels`, `channels-full` | Not compiled in. Adding one by hand is a `compile_error!`; a feature-graph test fails if a new `channel-*` feature is missing from the guard. A config that names a channel the build does not compile is refused (see below). |
| `daemon`, `relay`, `gateway`, `acp`, `channel start` and the `service` commands that start or install a unit | Refused at startup, before config is loaded or any state is written. `service stop`, `uninstall`, `status` and `logs` stay available to remove a unit installed by a standard build. |
| `zeroclaw update` | Never replaces this binary with a standard release archive, with or without `--standard`. |
| `[wss]`, `[relay]`, `[enroll]`, `[a2a.server]`, a tunnel provider | A config that enables one is refused instead of silently ignored. |

`config`, `doctor`, `status`, `self-test`, `auth`, `memory`, `skills` and `security`
remain available so a non-minimal config can be repaired.

## Limits of the claim

- Provider adapters are still compiled in. A single OpenAI-compatible endpoint is a
  configuration choice protected by the existing default-deny approval, not a feature gate.
- The runtime crate still contains the relay bridge, enrollment server and local RPC
  listener source, because they share types and helpers with code other commands use.
  Nothing reachable from this profile calls them, so the linker drops their entry points:
  `scripts/ci/minimal_profile_artifact.sh binary` asserts they are absent from an unstripped
  build, with positive controls. Removing that source is separate work (HORO-2383).
- Provider OAuth sign-in (for example the Gemini and OpenAI flows) uses a short-lived,
  user-initiated loopback callback listener. It is not gated, and `listener_probe.py`
  does not exercise it.
- `zeroclaw` installer `--minimal` and the xtask `Minimal` selection mean the kernel
  build, whose `agent` command is a plain chat loop without tools or memory. That is a
  different profile and must not be reported as this one.

## Verification

```sh
F="--no-default-features --features local-agent-minimal"
cargo test --locked -p zeroclaw $F --test component -- minimal_profile
cargo test --locked -p zeroclaw $F --test integration -- agent:: memory_loop_continuity:: memory_restart::
scripts/ci/minimal_profile_artifact.sh tree
cargo build --locked -p zeroclaw --bin zeroclaw $F
scripts/ci/minimal_profile_artifact.sh binary target/debug/zeroclaw
python3.13 scripts/local_agent/listener_probe.py --binary target/debug/zeroclaw
```

`minimal_profile_artifact.sh tree` fails if the minimal feature closure contains the
gateway, `daemon-listeners` or any channel feature, and fails with exit 2 if the default
graph does not show those markers (so it cannot pass vacuously). `listener_probe.py`
runs the agent against a stalling local provider and asserts that no process in its tree
holds a listening TCP or Unix socket, after confirming that `lsof` sees a synthetic
listener. The cumulative Golden Journey is run with the minimal binary as described in
`docs/local-agent/golden-journeys-runner.md` in the repository. Use an unstripped (dev)
binary for the symbol check; release builds strip symbols.
