# Local golden-journey runner

`scripts/local_agent/golden_journey.py` exercises a supplied, already-built ZeroClaw binary against a loopback-only scripted OpenAI-compatible provider and the supplied stdio MCP fixture. It uses Python's standard library only. It never calls an external provider or network service. Use Python 3.10 or newer for both the runner and fixture. The fixture is launched with the same Python interpreter that runs the runner.

This runner is POSIX-only because cleanup is enforced with an owned process group. It does not build Rust or provision the binary/fixture. Paths are supplied at invocation; relative paths resolve from the current directory. If `--fixture` is omitted, the runner looks for `mcp_fixture.py` beside itself.

```sh
python3.13 scripts/local_agent/golden_journey.py \
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

The timeout and cancellation cases are bounded failure/cleanup probes, not successful user journeys. Cancellation tests harness process termination, not graceful in-product cancellation.

All five cases passed locally against the pristine pinned upstream baseline binary, with provider request counts of 2 (granted), 1 (ungranted), 2 (timeout), 2 (cancellation), and 3 (recovery). Every owned process group was cleaned; the recovery case observed the exact correlated model-facing MCP error before the corrected echo and final marker. The initial recovery run failed because the harness expected the raw fixture error rather than the runtime's exact `Error: MCP` envelope; that failed result is retained separately from the corrected run.

This is one synthetic-provider, POSIX/macOS baseline observation, not verification of the derivative release candidate, universal MCP compatibility, all supported platforms, or graceful product cancellation. No external model or real credential was used; raw execution files remain local.

A separate run against candidate binary `8bda0584a2033f6ad36be6a7f9883a6057f398aee8469dadb7ccf101b3bf01e1`, built from source commit `a30ee7231636a898c14b10398eb2a51ddaa95328` for `aarch64-apple-darwin` with Rust 1.98, passed all five cases under Python 3.13.1. Provider request counts were 2 (granted), 1 (ungranted), 2 (timeout), 2 (cancellation), and 3 (recovery). Python 3.9 is unsupported: the fixture requires `typing.TypeAlias`, which is unavailable there. This is one candidate-specific observation, not a universal compatibility claim or full release validation; timeout and cancellation remain bounded harness probes, not graceful product behavior.

A further run against a debug build of the `local-agent-minimal` profile (`--no-default-features --features local-agent-minimal`, source commit `372b7b90b`, `aarch64-apple-darwin`, Rust 1.99.1) passed all five cases under Python 3.13.5 with provider request counts of 2 (granted), 1 (ungranted), 2 (timeout), 2 (cancellation) and 3 (recovery). The granted case echoed the exact tool result, so the exit code alone was not the evidence. This is one synthetic-provider, POSIX/macOS observation of the minimal profile, not release validation.

## Unit tests

```sh
python3.13 -m unittest discover -s scripts/local_agent -p 'test_golden_journey.py' -v
```

These are synthetic unit checks for config escaping, environment isolation, ordered error/recovery result correlation, timeout verdict boundaries, bounded marker reads, stdout/stderr overflow including writes during cleanup, and owned process-group cleanup after a parent exits. They neither import a sibling fixture nor execute the product binary, and are not actual-binary evidence. Sixteen unit tests passed locally; the synthetic child-process tests are separate from the five actual-baseline observations above and do not establish Windows compatibility.
