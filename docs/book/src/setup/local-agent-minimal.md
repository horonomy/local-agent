# Minimal runtime profile (`local-agent-minimal`)

The minimal profile builds the Local Agent runtime without any inbound surface:
the Agent Loop (plan, act, observe), memory, tools, MCP and CLI chat only. It is the
profile selected by Local Agent MVP 1.0 (HORO-2381) and the one ADR-0001 refers to
as the composed validation profile.

## Build and run

```sh
cargo build --release --locked --bin zeroclaw \
  --no-default-features --features local-agent-minimal
zeroclaw agent -a <alias> -m "hello"
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

## Provider approval and egress

For inference requests, `zeroclaw agent` contacts only a provider you configured:

- The agent runs only as a configured `[agents.<alias>]`, and its `model_provider` must
  resolve to `[providers.models.<family>.<alias>]`. An unresolved reference, or none, makes
  `zeroclaw agent` exit with an error before any network activity.
- `--model-provider` is accepted only when it names a configured `<family>.<alias>`, on
  `agent`, `models refresh`, `models list`, `doctor models` and
  `doctor update-context-windows`. A bare family or a URL is refused, and the refusal does
  not echo the value because a URL can carry a credential. Without this rule a `custom:` override naming a URL would
  send the request to an endpoint the config never approved.
- The minimal closure contains no Prometheus exporter
  (`minimal_profile_artifact.sh tree` asserts it is absent, with a positive control from
  the default graph). That is the only metrics or telemetry crate the check looks for.

Evidence, none of which reaches a public network (the component tests use unsandboxed loopback recorders):

```sh
cargo test --locked -p zeroclaw --no-default-features --features local-agent-minimal \
  --test component -- minimal_profile_provider_policy
python3.13 scripts/local_agent/egress_probe.py --binary target/debug/zeroclaw --mode approved
python3.13 scripts/local_agent/egress_probe.py --binary target/debug/zeroclaw --mode unapproved-alias
python3.13 scripts/local_agent/egress_probe.py --binary target/debug/zeroclaw --mode keyed-baseline
python3.13 scripts/local_agent/egress_probe.py --binary target/debug/zeroclaw --mode override-custom
python3.13 scripts/local_agent/egress_probe.py --binary target/debug/zeroclaw --mode override-models-refresh
```

`egress_probe.py` runs the agent under macOS `sandbox-exec` with every non-loopback
outbound connection denied, and two loopback recorders for the approved and an
unapproved endpoint. `keyed-baseline` is its control: a keyed provider with no `uri` must
visibly reach for its public endpoint and be blocked, otherwise the probe fails instead of
passing. A sandbox denial is not observable from outside, so an attempt is inferred from
the recorders and the error text. The system resolver stays reachable under this profile,
so a hostname lookup can still happen; no connection to a non-loopback address can succeed.

Not covered here: the model catalog lookups (the `openai` family's `list_models()` always
fetches models.dev, and `models refresh --all` is not probed), `auth login`, `auth paste-redirect` and
`auth refresh` (public OAuth endpoints, ungated), update checks, the tool
sandbox's own DNS allowance, LLM-invoked network tools such as `web_fetch`
(approval-gated, not automatic), the `model_switch` catalog fallback and the open-skills
environment override. Those are tracked as open decisions for HORO-2385.

## Limits of the claim

- Provider adapters are still compiled in. A single OpenAI-compatible endpoint is a
  configuration choice protected by the existing default-deny approval, not a feature gate.
- The relay bridge, enrollment server and local RPC listener are compiled out of this
  profile: their source in the runtime crate sits behind `daemon-listeners`, so the
  compiler, not the linker, keeps it out. The client helpers other commands use
  (`call_local`, `ensure_node_id`, `request_new_paircode` and friends) stay compiled in.
  `scripts/ci/minimal_profile_artifact.sh binary` still asserts the listener entry points
  are absent from an unstripped build, with positive controls, as a second line of
  defence against a gate being lost. `tokio-tungstenite` and `tungstenite`, which only
  those listeners and the gateway used, are no longer in this profile's dependency graph
  (`scripts/ci/minimal_profile_artifact.sh tree` asserts they are absent and that the
  default graph still has them). Other dependencies the listeners use, such as `rustls`,
  remain because the client helpers need them.
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
runs the agent against a stalling local provider and asserts that no process in its
`pgrep -P` descendant tree holds a listening TCP or Unix socket, after confirming that
`lsof` sees a synthetic listener. It covers the idle agent only; the `daemon`, `relay`
and `gateway` commands are covered by the refusal tests, and a standard build's `agent`
would pass the probe too, so it is evidence about this artifact's behavior, not by itself
proof that listener code is absent (the symbol and feature-closure checks are). The cumulative Golden Journey is run with the minimal binary as described in
`docs/local-agent/golden-journeys-runner.md` in the repository. Use an unstripped (dev)
binary for the symbol check; release builds strip symbols.
