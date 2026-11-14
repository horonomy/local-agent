# On-demand MCP children

By default every stdio MCP server in an agent's grant is started when the agent
is assembled and stays up. A server marked `spawn = "on_demand"` is instead
started by the first call to one of its tools, shared by concurrent calls, and
stopped after an idle window. An idle agent then runs none of these children.

## Configuration

Per server, under `[mcp.servers.<name>]`:

```toml
[mcp.servers.docs-browser]
transport = "stdio"
command = "/path/to/server"
spawn = "on_demand"            # default: "eager" (unchanged behavior)
declared_tools = ["open_page", "snapshot"]
idle_timeout_secs = 60         # default 60; 0 = stop right after the last call
```

| Field | Meaning |
|---|---|
| `spawn` | `eager` (default) starts at assembly. `on_demand` defers the start. |
| `declared_tools` | The only tools the server may expose. Required and non-empty for `on_demand`. |
| `idle_timeout_secs` | Time with no in-flight call before the child is stopped. |

An `on_demand` entry that is not stdio, has no `declared_tools`, or sets
`pinned_resources` (which would read from the server at startup) is skipped with
an error log rather than started.

## Behavior

- **Declared tools only.** Tool names come from config, not from the server, so
  the set is deterministic and known without starting it. A tool the server
  later advertises that is not listed is not callable. Declared tools carry a
  permissive input schema (`object`, any properties); the server's own schema is
  not fetched, so it validates arguments itself.
- **Policy first.** Calls reach the child only through the normal tool path, so
  the policy gate decides before anything runs. A denied call never starts the
  child.
- **Ownership.** The first call starts the child and completes the MCP
  handshake; concurrent first calls share that one child. In-flight calls are
  counted and a child is never stopped while any is running.
- **Release.** The child is stopped after the idle window, on transport reset
  (for example after a cancelled or timed-out call), on close, and when the
  registry is dropped. Each child runs in its own process group and the whole
  group is killed, so descendants it forked do not outlive it.
- **Crash.** A child that exits is replaced by the next call; an idle crash is
  not treated as an unhealthy server.
- **Logs.** Start and stop events go to the local agent log with the server
  name and reason only; no arguments, environment or output are recorded.

## Limits

- The handshake runs on first use, so the first call after idle pays process
  start-up latency.
- Non-Unix builds do not isolate a process group; only the direct child is
  killed there.
- Resource and prompt capability tools see no capabilities from an `on_demand`
  server until a child has been started and re-handshaken; they do not start one.
- Idle expiry is timer-based within one agent process. A hard-killed agent
  cannot run cleanup; its children are then left to notice their closed stdin,
  which a server may ignore.

## Measuring

`scripts/local_agent/process_tree_bench.py` counts descendant processes per phase
(assembled, active, after idle, after close) for eager and on-demand policies
using fake children, and fails if an on-demand idle tree is not empty or any child
outlives close. It models the policy; the Rust behavior is covered by the
`mcp_lazy` and `policy_bypass` tests. `--observe PID` prints descendant counts by
executable name for a live process.
