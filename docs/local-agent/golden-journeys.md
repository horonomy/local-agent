# Synthetic stdio MCP fixture: validation boundaries

`scripts/local_agent/mcp_fixture.py` is a deterministic, stdlib-only JSON-RPC fixture for local test setup. It uses the newline-delimited stdio framing implemented by the upstream MCP transport: one JSON-RPC message per input line, a newline-terminated response for requests, and no response for notifications. The fixture advertises `initialize` protocol version `2024-11-05`, an `echo` tool and a stateless `fail` tool. It rejects echo strings longer than 1 KiB; `fail` accepts only empty arguments and returns the fixed text `synthetic tool failure` with `isError: true`. Calling `echo` afterward in the same session still succeeds. This is an ordinary MCP tool-error result, separate from malformed-request JSON-RPC errors. It bounds each request line to 8 KiB before JSON parsing, returns fixed JSON-RPC errors for malformed/oversized input and unsupported methods, and exits on clean stdin EOF. It does not access the filesystem, shell, network, or spawn subprocesses.

Run the fixture-level subprocess self-tests with Python 3.10 or newer:

```sh
python3.13 -m unittest scripts.local_agent.test_mcp_fixture
```

These tests exercise the actual process over stdin/stdout pipes. They validate the fixture contract only; they do not prove runtime MCP discovery, tool policy, or agent-loop behavior. There are no committed input captures or JSON fixtures. Process-exit and open-pipe checks use POSIX APIs; the local pass is not evidence of Windows compatibility.

## Real-runtime verification scope

The exact existing upstream regression tests below are reusable runtime gates,
separate from fixture self-tests:

- `crates/zeroclaw-runtime/src/tools/scoped.rs::assemble_grants_no_mcp_to_agent_without_bundles`
- `crates/zeroclaw-runtime/src/tools/scoped.rs::assemble_lists_bundled_mcp_tools_in_both_loading_modes`
- `crates/zeroclaw-runtime/src/tools/scoped.rs::assemble_deferred_mcp_excludes_denied_tool_from_prompt_and_search`
- `crates/zeroclaw-runtime/src/agent/loop_.rs::run_tool_call_loop_memory_injection_keys_on_origin`
- `crates/zeroclaw-runtime/src/agent/loop_.rs::run_tool_call_loop_reports_unanswerable_approval_without_blaming_the_user`
- `crates/zeroclaw-runtime/src/agent/loop_.rs::run_tool_call_loop_parallel_cancel_no_double_terminal_for_completed_call`

The runtime test module also has the `ScriptedProvider` / `run_scripted_loop` seam in `crates/zeroclaw-runtime/src/agent/turn/mod.rs`. Initial offline execution was blocked by uncached `clap-markdown`; after permitted registry access and a locked build, these focused boundary tests ran separately. A clean baseline CLI journey also exercised a local synthetic provider, governed workspace `file_write`, tool-result roundtrip and final answer without credentials. Neither result proves this new stdio fixture was composed end-to-end with the runtime. The cumulative runtime library suite remains red on two inherited relay fixture setup failures; no fixture self-test substitutes for that gate. Keep detailed outcomes and raw evidence locally.
