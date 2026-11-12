# Local Agent

Local Agent is a local-first personal AI agent that runs from a single Rust
binary on your own machine. It is an independent derivative of
[ZeroClaw](https://github.com/zeroclaw-labs/zeroclaw) and is not an official
ZeroClaw distribution. See [Provenance and attribution](#provenance-and-attribution).

> **Status: MVP 2.0 is released as a pre-release; MVP 3.0 is in development.**
> The `MVP-2.0` tag is a GitHub pre-release with a macOS aarch64 tarball and
> checksums for the `local-agent-minimal` profile (`MVP-1.0` remains available).
> The tarball is not code-signed or notarized. There is no installer or
> package-manager distribution. The product brand is undecided; "Local Agent" is
> a working name, and the executable, crates and config directory still use
> upstream names (`zeroclaw`, `~/.zeroclaw`). Only what is listed under
> [What works in MVP 1.0](#what-works-in-mvp-10) is claimed; MVP 2.0 adds the
> MCP, Skills and plugin compatibility hardening described in its release notes.

## North star

You run the agent on your machine, against a model provider you chose. It plans,
acts through governed tools, observes the result and remembers. You start every
interaction yourself from the command line. This project does not offer, document
or promise an inbound endpoint, a chat-platform listener or remote control.

## What works in MVP 1.0

The `local-agent-minimal` build profile
([profile page](docs/book/src/setup/local-agent-minimal.md)) contains:

- the Agent Loop (plan, act, observe) through `zeroclaw agent`, interactive or
  single-message
- tools under the configured autonomy and risk profile (default `supervised`)
- agent memory, managed with `zeroclaw memory`
- MCP servers over stdio, granted per agent
- Skills (`zeroclaw skills`)
- diagnostics and configuration commands (`config`, `doctor`, `status`,
  `self-test`, `auth`, `security`)

It is standalone. An optional integration with AASM is planned for a later
milestone and is not implemented; nothing here depends on it.

### Not available in this profile

The gateway, ACP bridge, messaging channels, the daemon and its listeners, relay
and enrollment, the embedded web UI, voice wake, tunnels, and the `service`
commands that install or start a unit are not compiled in or are refused at
startup. There are **no install or quickstart instructions for them here**, by
design. The exact list and behavior is in the
[minimal profile page](docs/book/src/setup/local-agent-minimal.md#what-is-excluded).

## User journeys

1. **Chat with a model you configured.** Run `zeroclaw agent -a <alias>`
   (interactive) or add `-m "<message>"` (one message). The alias must exist as
   `[agents.<alias>]` and its `model_provider` must resolve to a configured
   `[providers.models.<family>.<alias>]`; otherwise the command exits before any
   network activity.
2. **Let the agent use tools under your approval policy.** The default
   `supervised` level runs low-risk tools, asks before medium-risk ones and
   blocks high-risk ones. See [Autonomy levels](docs/book/src/security/autonomy.md).
3. **Grant an MCP tool server.** An agent with no `mcp_bundles` connects to no
   MCP server. See [Set up a stdio MCP server](docs/book/src/local-agent/mcp-stdio-setup.md)
   and [MCP](docs/book/src/tools/mcp.md).
4. **Check the claims yourself, offline.** The golden-journey runner drives a
   built binary against a loopback-only scripted provider and a stdio MCP
   fixture, with no real model or credential. See
   [golden-journeys-runner.md](docs/local-agent/golden-journeys-runner.md).

## Supported platforms, providers and build

- **Build:** from source with Rust 1.96.0 or newer (edition 2024), or the
  MVP-2.0 pre-release tarball (macOS aarch64).
- **OS:** exercised on macOS (`aarch64-apple-darwin`) only. The golden-journey
  runner is POSIX-only and the egress probe uses macOS `sandbox-exec`. Linux and
  Windows are **not verified** for this profile.
- **Providers:** `zeroclaw quickstart` offers Anthropic, OpenAI, OpenRouter and
  Ollama, and OpenAI-compatible endpoints can be configured. `zeroclaw providers
  list` lists the provider aliases you have configured, not what the binary
  supports. Which providers were exercised end to end for MVP 1.0 is **not
  established**: the verified journeys use a synthetic local provider, and
  provider sign-in flows (OAuth) are not covered by the egress evidence.

## Install and first run

Build the minimal profile from a checkout:

```sh
cargo build --release --locked --bin zeroclaw \
  --no-default-features --features local-agent-minimal
```

The binary is `zeroclaw` in your Cargo target directory. Then:

```sh
zeroclaw quickstart                  # interactive, needs a terminal; writes a provider and an agent
zeroclaw agent -a <alias> -m "hello"
```

Configuration lives in `~/.zeroclaw/config.toml` unless you pass `--config-dir`;
`zeroclaw config set <path> <value>` configures headlessly. A plain
`cargo build --release` builds the **standard** profile, which includes the
surfaces excluded above, and the upstream installer scripts target upstream
releases. Neither is a Local Agent install path.

## Security and data egress

- Inference requests go only to a provider you configured. `--model-provider`
  accepts only a configured `<family>.<alias>`, never a bare family or a URL.
- Everything the model is sent, including tool results and recalled memory, goes
  to that provider. Use a local provider such as Ollama if that is not acceptable.
- Tools act on your machine with your privileges, within the workspace and
  command policy of the agent's risk profile. Read the
  [security overview](docs/book/src/security/overview.md) before raising autonomy.
- The egress evidence is a sandboxed probe on macOS with loopback recorders; see
  [Provider approval and egress](docs/book/src/setup/local-agent-minimal.md#provider-approval-and-egress).
  It is not proof that no byte ever leaves the machine.

## Known limits

Details are in the
[minimal profile page](docs/book/src/setup/local-agent-minimal.md#limits-of-the-claim).

- Not covered by the egress claim: model-catalog lookups, provider OAuth sign-in
  (a short-lived loopback callback listener that is not gated), update checks,
  the tool sandbox's DNS allowance, and approval-gated network tools such as
  `web_fetch`.
- Provider adapters, and some relay and enrollment source, are still in the source
  tree; the minimal binary is checked not to link the relay and enrollment entry
  points.
- Distribution license review is not complete (see [UPSTREAM.md](UPSTREAM.md)).
  The inherited [SECURITY.md](SECURITY.md) describes upstream's reporting
  channels; no Local Agent vulnerability-reporting channel is established yet.
- The journey evidence is one synthetic-provider run on macOS, not release
  validation.

## Provenance and attribution

Local Agent is derived from [ZeroClaw](https://github.com/zeroclaw-labs/zeroclaw),
created by its own community and maintainers. This repository keeps the upstream
license terms and attribution, and claims no upstream trademark rights or
endorsement.

- Pinned baseline: upstream `master` at commit
  `9dbb57231f48e26fee56243de4f7845924ce4f04` (workspace version `0.8.7`).
  Verification commands and the divergence ledger are in [UPSTREAM.md](UPSTREAM.md).
- License: [MIT](LICENSE-MIT) OR [Apache 2.0](LICENSE-APACHE); attribution in
  [NOTICE](NOTICE). Dependencies need their own license review.
- The ZeroClaw name and logo belong to upstream. Upstream documentation, ADRs and
  RFCs elsewhere in this repository are kept as historical record and may describe
  ZeroClaw channels and features that Local Agent excludes.
- Decision record for this derivative:
  [ADR-0001](docs/book/src/architecture/decisions/ADR-0001-upstream-foundation.md).

## Documentation and contributing

- Documentation source: [`docs/book/src`](docs/book/src/introduction.md)
- Compatibility guides: [stdio MCP setup](docs/book/src/local-agent/mcp-stdio-setup.md),
  [skill origins and trust](docs/book/src/local-agent/skills-trust.md),
  [optional WASM plugin host](docs/book/src/local-agent/wasm-plugins.md) and the
  [compatibility matrix](docs/book/src/local-agent/compatibility-matrix.md)
- Contributor workflow: [AGENTS.md](AGENTS.md) and the
  [engineering lifecycle](docs/local-agent/engineering-lifecycle.md)
- Baseline measurement method: [baseline-method.md](docs/local-agent/baseline-method.md)
- Source export review aid: [export-policy.md](docs/local-agent/export-policy.md)

## Support Development

This project is free and open source. If it is useful to you and you would like to, you can voluntarily support its development on [Ko-fi](https://ko-fi.com/chisanan232). Horonom is not yet an incorporated company: contributions go to the individual maintainer's personal Ko-fi account, not to a registered company. Support is optional, is not a charitable donation, and does not come with priority support or other benefits.
