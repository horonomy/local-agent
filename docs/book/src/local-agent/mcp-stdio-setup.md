# Set up a stdio MCP server

This guide wires one local stdio MCP server to one agent and bounds what the agent may call. It covers stdio only. Local Agent does not document or promise a remote MCP listener: nothing here opens a port, and the client never accepts inbound connections. HTTP and SSE client transports exist in the code, are unverified for Local Agent, and are out of scope (see the [compatibility matrix](./compatibility-matrix.md), row M11).

Reference pages this guide builds on: [MCP and Skills](./mcp-skills.md) (grant model, child environment, failure behavior) and the inherited [MCP](../tools/mcp.md) field reference.

## What was run

Every command below was run against a debug build of the minimal profile (`la-cargo build --no-default-features --features local-agent-minimal --bin zeroclaw`, version `0.8.7`, macOS aarch64) with an isolated config directory. Commands that were not run are marked **unverified** or **unsupported**.

## 1. Define the server and grant it

Write the server, a bundle that grants it, and an agent that uses the bundle. The synthetic fixture shipped in this repository stands in for a real server.

```toml
schema_version = 3

[[mcp.servers]]
name = "docs"
transport = "stdio"
command = "python3"
args = ["/absolute/path/to/scripts/local_agent/mcp_fixture.py"]
tool_timeout_secs = 5

[mcp_bundles.local_tools]
servers = ["docs"]

[agents.assistant]
risk_profile = "assistant"
mcp_bundles = ["local_tools"]

[risk_profiles.assistant]
level = "supervised"
allowed_tools = ["docs__echo"]
excluded_tools = ["docs__fail"]
auto_approve = ["docs__echo"]
```

Grants are explicit. An agent without `mcp_bundles` never spawns the server. Granted tools are namespaced `<server>__<tool>` (here `docs__echo`).

Check how the config was read:

```sh
zeroclaw --config-dir ./cfg config list --filter mcp
zeroclaw --config-dir ./cfg security status --agent assistant
```

Verified: the first command lists `mcp.servers.docs.*` and `mcp_bundles.local_tools.*`; the second prints the agent's risk profile, supervised autonomy and sandbox. Without `risk_profile` on the agent, `security status` fails with `agents.assistant.risk_profile is empty`.

`zeroclaw config set` edits an existing server's field:

```sh
zeroclaw --config-dir ./cfg config set mcp.servers.docs.tool_timeout_secs 7
```

Verified. It cannot create a server: `config set mcp.servers.docs.command python3` on a config with no `docs` entry fails with `Unknown property`. The inherited MCP page's `config set mcp.servers.filesystem.command npx` example has the same limit in this build. Add the `[[mcp.servers]]` table by editing `config.toml`.

## 2. See the protocol the client speaks

A stdio server reads newline-delimited JSON-RPC from stdin and writes responses to stdout. The client sends `protocolVersion` `2024-11-05`. You can exercise the fixture by hand:

```sh
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"probe","version":"0"}}}' \
  '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' \
  '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"echo","arguments":{"text":"hi"}}}' \
  | python3 scripts/local_agent/mcp_fixture.py
```

Verified: three responses; the `tools/call` result is `{"content":[{"type":"text","text":"hi"}],"isError":false}`. This exercises the fixture, not the Local Agent client.

## 3. Run the agent against it

An agent run needs a model provider. The repository's golden-journey runner supplies a loopback-only scripted provider, so no credential or external network is involved:

```sh
python3 scripts/local_agent/golden_journey.py \
  --binary /absolute/path/to/zeroclaw \
  --fixture scripts/local_agent/mcp_fixture.py \
  --case all --deadline 40
```

Verified on this build (Python 3.14.7): `case_pass` true. `granted` made 2 provider requests and returned the exact echoed result through the real stdio fixture; `ungranted` made 1 and offered no MCP tool; `provider_timeout` 2, `cancellation` 2 and `recovery` 3 all passed. `listener_probe.py` reported `no-listener`. These are synthetic-provider observations on one platform. `zeroclaw agent -a assistant -m hi` with no resolvable provider exits before any network activity (`model_provider does not resolve`); running it against a real provider is **unverified** here.

## Tool scopes

Three independent controls bound what a granted server can do. They are separate on purpose.

| Control | Question it answers | Tested here |
| --- | --- | --- |
| `mcp_bundles` (grant) | Does the agent connect to the server at all? Deny wins through `exclude`. | Golden `granted` / `ungranted`; test `ungranted_or_excluded_agent_never_spawns_the_stdio_server` |
| `allowed_tools` / `excluded_tools` (risk profile) | May the agent call this prefixed tool at all? `excluded_tools` always subtracts. | Config loads (above); the golden `granted` case runs with a one-tool `allowed_tools`; per-name exclusion of an MCP tool is covered by inherited tests, not re-run here |
| `auto_approve` / `always_ask` | Does a call need a human decision? Supervised autonomy asks for medium-risk tools unless the prefixed name is listed. | Same as above; the interactive prompt was not driven here |

Two behaviors to plan around:

- When `allowed_tools` is non-empty, every MCP tool (any name containing `__`) is auto-admitted unless it is in `excluded_tools`. List destructive tools in `excluded_tools`. The `docs__fail` entry above is how this guide blocks the fixture's second tool.
- An empty `allowed_tools` means no restriction, not deny-all. Use `deny_all_tools = true` for deny-all.

The client also strips a caller-supplied `approved` flag before forwarding a call, and treats tool and result text as data, never as instructions (matrix rows M4, M5).

## Child environment and secrets

A stdio server is spawned with a reduced environment. Provider API keys in your shell do not reach it. Put what the server needs in that server's `env`, which is stored as a secret and never printed by `config list` (it shows `🔒`). Proxy variables, toolchain variables such as `NVM_DIR`, and `XDG_*` are not inherited. The full allow-list is on [MCP and Skills](./mcp-skills.md).

## Not covered

- Launching community servers with `npx` or `uvx`: **unverified**. It fetches third-party code, which was not run for this guide.
- Real model providers driving MCP tools: **unverified**; the evidence uses a scripted provider.
- HTTP and SSE MCP transports and any remote MCP listener: **not documented, not promised**.
- The server's own `protocolVersion` is not compared with the client's (matrix row M3).
