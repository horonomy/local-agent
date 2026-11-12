# Getting Started

Local Agent MVP 2.0 is published only as a pre-release tarball (macOS aarch64, not code-signed or notarized) with no installer. You can download it from the `MVP-2.0` GitHub release or build the `local-agent-minimal` profile from source and run the resulting `zeroclaw` executable (the executable and config directory still use upstream names).

## Build

```sh
cargo build --release --locked --bin zeroclaw \
  --no-default-features --features local-agent-minimal
```

The profile is `agent-core` under another name: the Agent Loop, memory, tools, MCP and CLI chat, with no inbound surface. [Minimal runtime profile](../setup/local-agent-minimal.md) is the authoritative description of what it contains and excludes.

## Configure an agent

`zeroclaw agent` runs only as a configured agent alias:

- the alias must exist as `[agents.<alias>]`
- its `model_provider` must resolve to `[providers.models.<family>.<alias>]`

If the reference is missing or unresolved, `zeroclaw agent` exits with an error before any network activity. Only a provider you configured is contacted for inference; see [Provider approval and egress](../setup/local-agent-minimal.md#provider-approval-and-egress).

Settings can be read and changed with `zeroclaw config list`, `get`, `set` and `init`, and `--config-dir` selects a config directory. For a complete configuration, see the inherited [Minimal working example](../providers/configuration.md#minimal-working-example). That page documents the standard build, so ignore its channel, gateway and daemon settings.

## Run

```sh
zeroclaw agent -a <alias>
zeroclaw agent -a <alias> -m "hello"
```

Without `-m` the agent starts an interactive chat session; with `-m` it answers one message and exits. You start every interaction yourself.

## Check the setup

`zeroclaw doctor`, `zeroclaw status` and `zeroclaw self-test` are available in this profile, as are `config`, `auth`, `memory`, `skills` and `security`, so a non-minimal config can be repaired.

## Platform

The profile has been exercised on macOS (`aarch64-apple-darwin`) only. Linux and Windows are not verified for it.

## Next

- [Core Capabilities](./core-capabilities.md)
- [MCP and Skills](./mcp-skills.md)
- [Security and Privacy](./security-privacy.md)
