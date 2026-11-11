# MCP and Skills

## MCP

Local Agent is an MCP client. This page documents servers it spawns over stdio; other transports exist in the code but are not covered here. Access is granted per agent and nothing is granted by default.

- A server is defined under `mcp.servers` (its `name`, `command` and `args` for stdio).
- A bundle `[mcp_bundles.<alias>]` lists the servers it grants in `servers`, with an optional `exclude` list. Deny wins: a name in any referenced bundle's `exclude` is removed.
- An agent connects to the servers granted by the bundles named in its `agents.<alias>.mcp_bundles`.
- Granted tools are namespaced `<server>__<tool>`.

A stdio server runs as a child process with a reduced environment: only `PATH`, `HOME`, `USER`, `LOGNAME`, `SHELL`, `TERM`, `LANG`, `LC_ALL`, `LC_CTYPE`, `TMPDIR`, `TZ` and the Windows runtime variables (`SystemRoot`, `USERPROFILE`, `APPDATA`, `LOCALAPPDATA`, `TEMP`, `TMP`, `PATHEXT`, `COMSPEC`) are inherited. Any other variable, including API keys in the parent environment, reaches a server only when it is set in that server's `env`.

Failure behavior for a stdio server, covered by local synthetic-server tests: non-JSON, stale-id and non-response lines are ignored; a server that exits during the handshake fails the connection; a child that crashes during a call fails that call without waiting for its deadline and the call is not replayed; a call that exceeds its deadline fails and the unresponsive child is reaped; a tool-level `isError` result fails the call without disturbing the connection. No listener is opened.

An agent with no bundle is not given the tools of a configured server. The golden-journey cases `granted` and `ungranted` in `docs/local-agent/golden-journeys-runner.md` observe this against a loopback fixture, on a debug build of the minimal profile.

The inherited [MCP](../tools/mcp.md) page has the full field reference. It documents the standard build, including configuration through the gateway and transports other than stdio; neither is documented here for Local Agent.

## Skills

`zeroclaw skills` provides `list`, `add`, `edit`, `bundle`, `audit`, `install`, `remove` and `test`. `skills` remains available so that a non-minimal config can be repaired.

The egress evidence on the minimal profile page does not cover installing a skill from a URL; treat that as network access you initiate.

Inherited reference: [Skills](../tools/skills.md) · [Skill bundles](../tools/skill-bundles.md).
