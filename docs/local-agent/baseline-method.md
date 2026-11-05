# Upstream baseline assessment method

Ticket: HORO-2377. Baseline: `9dbb57231f48e26fee56243de4f7845924ce4f04`.
This is a reproduction procedure, not a benchmark result or release signoff.

## Build inputs

Use the unmodified upstream checkout, workspace version `0.8.7`, the committed
`Cargo.lock`, and Rust `1.96.0` or a separately recorded compatible toolchain.
The first baseline is the upstream default-feature binary. Do not compare a
modified minimal build to an unspecified upstream build or silently alter the
lockfile to make dependency resolution succeed.

```sh
cargo metadata --offline --locked --format-version 1
cargo build --release --offline --locked --bin zeroclaw -j 2
cargo fmt --all -- --check
```

Redirect raw command output, dependency graphs and benchmark reports to a local
directory outside every Git worktree. Review output locally; cloud reporting is
limited to sanitized summaries. Never run an online fallback or package download
unless that dependency access is independently permitted. Do not use an unrelated
installed executable as the baseline when the source cannot build.

## Current assessment

The local Rust toolchain meets the declared minimum. Initial locked offline
resolution and release-build attempts stopped at uncached `clap-markdown`. The
founder then confirmed configured registry access is permitted; locked target
dependencies were fetched without changing the lockfile. The resumed default
release build succeeded. An initial dirty stamp came from staged method docs;
a subsequent rebuild from the immutable clean checkout reports `mvp-0.0`, the
founder-approved annotated provenance tag. Neither build modified runtime,
build-script, Cargo config or lockfile inputs.

Formatting and isolated CLI help/version checks passed. A controlled loopback
provider journey also verified the actual CLI Agent Loop, workspace-confined
`file_write`, tool-result roundtrip and deterministic final answer without a
provider credential. Focused upstream runtime boundary checks have run separately.
These checks do not establish a complete cumulative suite, repeatable binary
reproducibility, cross-platform support or idle-RSS/agent-startup budgets. Keep
raw measurements and exact outcomes in the local ledger, not source artifacts.

At the pinned commit, upstream Quality Gate succeeded, but Scheduled Platform
Tests failed. Check metadata identifies the macOS failure at `Install cargo-nextest`
and the Windows failure at `Run tests`; it does not establish either root cause.
Preserve these facts; neither this observation nor a formatting pass is a
cumulative baseline pass. Classify failures without treating unavailable local
reproduction or a successful sibling job as a waiver.

## Measurement protocol after the build is available

1. Record the exact source SHA, lockfile identity, toolchain, target triple,
   features, release profile and operating-system version in the local ledger.
2. Use a fresh synthetic configuration and workspace outside Git. Supply no real
   provider key, installed Skills, MCP configuration or corporate environment.
   Inspect the chosen upstream entry point before running it: CLI help/version
   does not prove Agent Loop startup or idle-agent resource use.
3. Measure the actual release executable size. State whether it is stripped and
   whether optional browser/WASM components are included. Do not compare archive
   size against executable size.
4. Measure cold and warm startup separately, at least three samples each, using a
   monotonic clock. Define readiness as a verified observable agent boundary, not
   successful process creation. Do not claim a genuinely cold OS cache without a
   supported, non-privileged method; otherwise label it process-cold only.
5. Measure steady idle RSS, CPU and the complete child-process tree for the same
   verified command/configuration. Distinguish parent RSS from aggregate RSS.
   Record the idle interval, sample count and startup cost separately.
6. Exercise the real agent against controlled synthetic provider/tool services.
   Test plan/act/observe, denial, timeout, cancellation and recovery. A fixture
   self-test is not a passing agent journey.
7. Inspect only the test process tree and its listeners; stop and clean up owned
   processes. Do not inspect unrelated applications or disable networking.
8. Repeat the same method after optimizations. Report the sample range and
   deviations, not a single favorable sample. Missing platforms stay untested.

The MVP 1.0 size/RSS/startup budgets are targets, not acceptance shortcuts.
Maintain capability and permission boundaries even if measurements exceed them.
No fabricated measurements or skipped-as-passed results are permitted.
