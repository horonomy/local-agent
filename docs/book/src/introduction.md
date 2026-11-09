# Local Agent

Local Agent is a local-first personal AI agent that runs from a single Rust binary on your own machine. It is an independent derivative of ZeroClaw and is not an official ZeroClaw distribution (see [Provenance and attribution](#provenance-and-attribution)).

MVP 1.0 is in development. There is no published release or installer, the product brand is undecided, and the executable and config directory still use upstream names (`zeroclaw`, `~/.zeroclaw`).

## North star

You run the agent on your machine, against a model provider you chose. It plans, acts through governed tools, observes the result and remembers. You start every interaction yourself from the command line. This project does not offer, document or promise an inbound endpoint, a chat-platform listener or remote control.

## What works in MVP 1.0

The [minimal runtime profile](./setup/local-agent-minimal.md) (`local-agent-minimal`) contains:

- the Agent Loop through `zeroclaw agent`, interactive or single-message
- tools under the configured autonomy and risk profile, `supervised` by default
- agent memory, MCP servers over stdio granted per agent, and Skills
- diagnostics and configuration commands

It is standalone. An optional integration with AASM is planned for a later milestone and is not implemented.

The gateway, ACP bridge, messaging channels, the daemon and its listeners, relay and enrollment, the embedded web UI, voice wake, tunnels and the `service` commands that install or start a unit are not part of this profile. This site has no install or quickstart instructions for them as Local Agent features; see [what is excluded](./setup/local-agent-minimal.md#what-is-excluded).

## Where to start

- Building and running the profile: [Minimal runtime profile](./setup/local-agent-minimal.md)
- Where data can leave the machine: [Provider approval and egress](./setup/local-agent-minimal.md#provider-approval-and-egress) and its [limits](./setup/local-agent-minimal.md#limits-of-the-claim)
- Controlling what the agent may do: [Security overview](./security/overview.md) · [Autonomy levels](./security/autonomy.md)
- Granting an MCP tool server: [MCP](./tools/mcp.md)
- Configuring a provider: [Model Providers](./providers/overview.md)
- Why the derivative looks like this: [ADR-0001](./architecture/decisions/ADR-0001-upstream-foundation.md)

Supported platform: the profile has been exercised on macOS (`aarch64-apple-darwin`) only. Linux and Windows are not verified for it.

## Provenance and attribution

Local Agent is derived from [ZeroClaw](https://github.com/zeroclaw-labs/zeroclaw), created by its own community and maintainers. The repository keeps the upstream license terms (MIT OR Apache 2.0) and attribution, and claims no upstream trademark rights or endorsement. The pinned baseline commit, verification steps and divergence ledger are in `UPSTREAM.md` at the repository root.

Most other pages in this book are inherited upstream ZeroClaw documentation, kept as historical record. They can describe channels, gateway, daemon and other features that Local Agent excludes, and their commands, installers and links refer to upstream ZeroClaw. Treat the minimal runtime profile page as authoritative for what Local Agent includes.

`Reference → CLI` and `Reference → Config` are generated from the code at build time, so they list every command of the standard build, including those the minimal profile refuses.
