# MCP and Skills

## MCP

Local Agent is an MCP client. This page documents servers it spawns over stdio; other transports exist in the code but are not covered here. Access is granted per agent and nothing is granted by default.

- A server is defined under `mcp.servers` (its `name`, `command` and `args` for stdio).
- A bundle `[mcp_bundles.<alias>]` lists the servers it grants in `servers`, with an optional `exclude` list. Deny wins: a name in any referenced bundle's `exclude` is removed.
- An agent connects to the servers granted by the bundles named in its `agents.<alias>.mcp_bundles`.
- Granted tools are namespaced `<server>__<tool>`.

An agent with no bundle is not given the tools of a configured server. The golden-journey cases `granted` and `ungranted` in `docs/local-agent/golden-journeys-runner.md` observe this against a loopback fixture, on a debug build of the minimal profile.

The inherited [MCP](../tools/mcp.md) page has the full field reference. It documents the standard build, including configuration through the gateway and transports other than stdio; neither is documented here for Local Agent.

## Skills

`zeroclaw skills` provides `list`, `add`, `edit`, `bundle`, `audit`, `install`, `remove` and `test`. `skills` remains available so that a non-minimal config can be repaired.

The egress evidence on the minimal profile page does not cover installing a skill from a URL; treat that as network access you initiate.

Inherited reference: [Skills](../tools/skills.md) · [Skill bundles](../tools/skill-bundles.md).
