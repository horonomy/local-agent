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

A further run against a debug build of the `local-agent-minimal` profile (`--no-default-features --features local-agent-minimal`, source commit `d2ea0f612`, `aarch64-apple-darwin`, Rust 1.99.1) passed all five cases under Python 3.13.5 with provider request counts of 2 (granted), 1 (ungranted), 2 (timeout), 2 (cancellation) and 3 (recovery). The granted case echoed the exact tool result, so the exit code alone was not the evidence. This is one synthetic-provider, POSIX/macOS observation of the minimal profile, not release validation.

## Governance cases (MVP 3.0, not yet run on a built candidate)

`--case all` now also runs four governance cases defined in `scripts/local_agent/governance_journey.py` (ADR-019). **None of these four has been run against any built binary: no result is recorded for them, and none may be reported as a pass until it has been run on the exact release candidate.** They need the release binary; only their builders and verdict logic are unit-tested (below).

Run every case, MCP and governance, on a candidate binary (POSIX, Python 3.10 or newer):

```sh
python3 scripts/local_agent/golden_journey.py \
  --binary /absolute/path/to/zeroclaw \
  --case all \
  --deadline 40
```

A single governance case: `--case policy_deny|policy_ask_denied|audit_record|managed_unavailable`. The JSON output adds a `failure` label and a `fixture_tool_calls` count for these cases. A case passes only if the run is clean (owned process group reaped, no output over 1 MiB, no `Authorization` header, no malformed provider request), the CLI exits 0 after exactly two provider requests with the final marker `GOV_FINAL`, and the case-specific assertion below holds. In every case the scripted provider emits one `fixture__echo` call with the synthetic argument `HORO-2398-GOV-SENTINEL-7f3a9c`. The fixture is launched through a wrapper that appends a bare `call` line to a log for each `tools/call` it dispatches (never the arguments), so non-execution is observed in the fixture process, not inferred. Stdin is `/dev/null`: there is no operator.

- `policy_deny`: `excluded_tools = ["fixture"]` with `fixture__echo` explicitly listed in `allowed_tools`, the bundle granted and the tool in `auto_approve`. Passes only if the tool result the model receives contains `Tool call not executed: 'fixture__echo' was denied by policy (excluded).` (`policy_gate::denial_message`, reason `ReasonCode::Excluded`: an exclusion beats a grant), the fixture received zero `tools/call` requests and the sentinel is not in the result. The exclusion names the server, not the tool, because the registry filter matches exact tool names (`scoped.rs`) and would remove an exactly named tool before the engine decides, which yields `Unknown tool` instead of a policy reason.
- `policy_ask_denied`: `always_ask = ["fixture__echo"]` (an Ask overrides `auto_approve`; ADR-019 D3) with no operator. Passes only if the tool result contains `Tool call not executed: 'fixture__echo' requires approval and no ` or `Denied by user. The operator was asked to approve 'fixture__echo' and declined` (`approval_gate.rs`), with zero fixture `tools/call` requests and no sentinel in the result. Which of the two the CLI produces without a terminal is not known without a run; both mean the call was refused without an approval.
- `audit_record`: standalone default governance, tool allowed. Passes only if the fixture ran exactly once and echoed the sentinel, exactly one live `policy-audit.jsonl` exists under the isolated temp root, no `policy-audit.jsonl*` file contains the sentinel bytes anywhere, and exactly one record has `tool_name` `fixture__echo`, `v` 1, `capability` `mcp`, `decision.reason` `local_allow`, `decision.effective` `allow`, a non-empty `decision_id`, `authority.mode` `standalone`, `args_sha256` equal to the SHA-256 of the canonical JSON `{"text":"HORO-2398-GOV-SENTINEL-7f3a9c"}`, and no key named `args`, `arguments`, `output` or `result` at any depth. The digest equality assumes the runtime does not add arguments before deciding; if a first real run fails only on `audit_field_mismatch`, check that before suspecting the audit writer.
- `managed_unavailable`: `[governance] mode = "managed"` with `socket_path` naming an absolute path whose directory does not exist, in the `local-agent-minimal` build, which has no `aasm-managed` feature. The tool is otherwise allowed, so a standalone fallback would run it. Passes only if the tool result contains `Tool call not executed: 'fixture__echo' was denied by policy (authority_unavailable).` and the fixture received zero `tools/call` requests. If the binary refuses to start (no provider request), the case fails with `no_provider_request`. That would also be fail-closed, but ADR-019 D6 says "Refuse startup" while the AASM setup page and `policy_gate.rs` (`Mode::Unavailable`) say the session runs and denies every call; the discrepancy must be reconciled by the owners, not accepted by this runner. Do not run this case against a build that has `aasm-managed`.

Premises that only a real run can confirm: that each tool stays registered (advertised) so the call reaches the policy gate, that the CLI exits 0 after the denial and a second provider request, and the stdout final marker. If a premise is wrong the case fails; it does not pass.

## Unit tests

```sh
python3.13 -m unittest discover -s scripts/local_agent -p 'test_golden_journey.py' -v
```

These are synthetic unit checks for config escaping, environment isolation, ordered error/recovery result correlation, timeout verdict boundaries, bounded marker reads, stdout/stderr overflow including writes during cleanup, and owned process-group cleanup after a parent exits. They neither import a sibling fixture nor execute the product binary, and are not actual-binary evidence. Sixteen unit tests passed locally; the synthetic child-process tests are separate from the five actual-baseline observations above and do not establish Windows compatibility.

Governance builders and verdicts add sixteen further unit tests (thirty-two in the file): per-case config rendering, the canonical-JSON digest vector, the exact denial strings, the fixture call-log wrapper run over real stdio against the unmodified fixture, and one passing plus several failing observations or audit logs per assertion (tool ran, wrong reason code, sentinel or raw field in the audit log, wrong digest, missing or duplicate record). They run without a binary and are not evidence of product behavior.
