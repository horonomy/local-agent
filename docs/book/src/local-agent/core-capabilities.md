# Core Capabilities

What the `local-agent-minimal` profile contains, each item backed by [Minimal runtime profile](../setup/local-agent-minimal.md) or by the source it names.

| Capability | What you get |
| --- | --- |
| Agent Loop | Plan, act, observe, through `zeroclaw agent`, interactive or single-message (`-m`). |
| Tools | Tools run under the agent's configured autonomy and risk profile. The levels are `readonly`, `supervised` and `full`; `supervised` is the default. |
| Memory | Agent memory, inspected with `zeroclaw memory` (`list`, `get`, `stats`, `clear`, `reindex`). |
| Providers | Inference goes only to a provider you configured as `[providers.models.<family>.<alias>]` and referenced from the agent. |
| MCP | MCP servers over stdio, granted per agent. See [MCP and Skills](./mcp-skills.md). |
| Skills | Managed with `zeroclaw skills`. See [MCP and Skills](./mcp-skills.md). |
| Diagnostics | `config`, `doctor`, `status`, `self-test`, `auth` and `security`. |

## Not in this profile

No gateway, ACP bridge, messaging channel, daemon listener, relay or enrollment, embedded web UI, voice wake, tunnel, or `service` command that installs or starts a unit. Browser and computer use are [planned](./browser-computer-use.md); an optional [AASM](./aasm.md) integration is planned.

## Inherited reference pages

These pages are inherited from upstream ZeroClaw and describe the standard build. Where they differ from the minimal profile page, the profile page wins.

- [Agents](../agents/overview.md)
- [Model Providers](../providers/overview.md)
- [Tools overview](../tools/overview.md)
