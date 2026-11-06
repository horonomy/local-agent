# Local golden-journey runner

`scripts/local_agent/golden_journey.py` exercises a supplied, already-built ZeroClaw binary against a loopback-only scripted OpenAI-compatible provider and the supplied stdio MCP fixture. It uses Python's standard library only. It never calls an external provider or network service.

This runner is POSIX-only because cleanup is enforced with an owned process group. It does not build Rust or provision the binary/fixture. Paths are supplied at invocation; relative paths resolve from the current directory. If `--fixture` is omitted, the runner looks for `mcp_fixture.py` beside itself.

```sh
python3 scripts/local_agent/golden_journey.py \
  --binary /absolute/path/to/zeroclaw \
  --fixture /absolute/path/to/mcp_fixture.py \
  --case all \
  --deadline 40
```

Each case gets an isolated temporary HOME, workspace, config directory, and stdout/stderr files. The child receives a fixed environment without inherited API keys or proxy variables. Temporary files are deleted after the case; raw CLI output, configuration, and provider payloads are not printed. The sole output is sanitized JSON with case status, booleans, exit codes, and counts. A failing case makes the runner exit nonzero. `--deadline` is a positive finite number of seconds per case.

The runner observes a 1 MiB threshold separately for stdout and stderr in its process-monitor loop and after owned-process cleanup. Either stream exceeding it fails the case with `output_limit` and triggers cleanup. This is a polled file-size limit, not a strict disk-write ceiling: a fast child can overshoot between observations. Final-marker inspection reads at most 1 MiB plus one byte and rejects oversized output. Cleanup failures remain failures even after an output-limit event.

## Cases

- `granted`: requires two provider requests, advertised `fixture__echo`, the exact echoed marker returned through the real stdio fixture, and the normal final marker.
- `ungranted`: configures no MCP bundle and requires one provider request, no advertised echo and no returned tool result. The binary may end successfully without a normal final-answer marker when it declines an ungranted tool call; this case does not treat that missing marker as failure or claim a user-facing denial answer.
- `recovery`: explicitly grants both fixture tools, requests a correlated synthetic failure, issues a corrected echo call only after that exact failure result, then requires the exact echo result and final marker across three provider requests. An unrelated failure or success without the preceding failure cannot satisfy the case. Results are observed in the actual binary's provider-request history; fixture PID observation proves process startup, not an independent fixture-side record of both invocations.
- `provider_timeout`: grants the fixture, completes a correlated echo call, then holds the second provider request. Requires a naturally observed positive failure exit after the configured provider-timeout interval but before the outer deadline, plus an observed child PID and clean process group. Timing alone does not prove the runtime's diagnostic cause.
- `cancellation`: grants the fixture and waits for the exact correlated echo result on the second provider request and the fixture's self-recorded PID in the owned process group, then sends TERM with KILL fallback.

The timeout and cancellation cases are bounded failure/cleanup probes, not successful user journeys. Cancellation tests harness process termination, not graceful in-product cancellation. The runner has unit coverage only; updated actual-binary runs remain blocked by tool-scope permission. Earlier execution of a different runner revision is not proof that this version passes.

## Unit tests

```sh
python3 -m unittest discover -s scripts/local_agent -p 'test_golden_journey.py' -v
```

These are synthetic unit checks for config escaping, environment isolation, ordered error/recovery result correlation, timeout verdict boundaries, bounded marker reads, stdout/stderr overflow, and owned process-group cleanup after a parent exits. They neither import a sibling fixture nor execute the product binary, and are not actual-binary evidence. Fourteen unit tests passed locally; this does not establish the five integration cases or Windows compatibility.
