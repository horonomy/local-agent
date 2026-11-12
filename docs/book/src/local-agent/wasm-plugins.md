# Optional WASM plugin host

WASM plugins are **optional and off in the default Local Agent build**. The `local-agent-minimal` profile contains no plugin host and no `wasmtime`; the `zeroclaw plugin` command does not exist in it. A plugin host is an opt-in build, and even then plugins do nothing until you enable them. This page states what that opt-in build accepts, what it refuses, and which claims were actually run. It does not promise that ZeroClaw plugins work on Local Agent. For per-sample results see the [compatibility matrix](./compatibility-matrix.md#wasm-plugins); for authoring see the inherited [Plugin protocol](../developing/plugin-protocol.md).

## What was run

Two debug builds from this tree (`0.8.7`, macOS aarch64, Rust under the `la-cargo` wrapper):

```sh
la-cargo build --no-default-features --features local-agent-minimal --bin zeroclaw
la-cargo build --no-default-features --features local-agent-minimal,plugins-wasm-cranelift --bin zeroclaw
```

The plugin commands below ran on the second binary against two in-repo test fixtures, built with `la-cargo build -p zeroclaw-tool-plugin-fixture -p zeroclaw-channel-plugin-fixture --target wasm32-wasip2` and placed in package directories with the fixture's `plugin-manifest.toml` renamed to `manifest.toml`. No third-party plugin and no registry package was installed or run.

## Build profiles

| Build | Plugin host | `zeroclaw plugin` |
| --- | --- | --- |
| `local-agent-minimal` (default for Local Agent) | not compiled | `error: unrecognized subcommand 'plugin'` (verified) |
| `local-agent-minimal` + `plugins-wasm-cranelift` | compiled, JIT | present (verified) |
| `local-agent-minimal` + `plugins-wasm-runtime-only` | compiled, precompiled `.cwasm` only | **unverified**; no `.cwasm` sample was produced |
| standard upstream build | compiled where the artifact carries it | outside Local Agent's promise |

The feature combination with `plugins-wasm-cranelift` compiles because the minimal compile guard does not list the plugin host features. That is a build fact, not a supported-product statement: it is the only configuration in which plugin commands were run.

## Versions and ABI

| Item | Value | Source |
| --- | --- | --- |
| Plugin interface | WIT package `zeroclaw:plugin@0.1.0`, directory `wit/v0`, **experimental** (no `.frozen` file) | `wit/VERSIONING.md`, `HOST_WIT_PACKAGE` / `HOST_WIT_VERSION` in `crates/zeroclaw-plugins/src/component.rs` |
| WASI | preview 2, `wasi:*@0.2.x`, target `wasm32-wasip2` | `check_abi_pin` in the same file |
| Component form | WebAssembly component only; core modules and `wasm32-wasip1` are refused | matrix P8 |

Because `wit/v0` is experimental, rebuild a component against the WIT shipped in the exact tree that runs it. A prebuilt component from an earlier experimental world is not a conformance target. The host rejects a component whose `zeroclaw:plugin/*` interfaces carry another version or none, or whose `wasi:*` interfaces are not `0.2.x`, at install, `plugin info` and load, naming the interface and the pinned version. A unit test ties the pin to every `wit/v0/*.wit` package line. A wrong-version component was **not** built for this guide, so that refusal is cited from the tests (`abi_pin_*`, `install_gate_*`), not from a command run here. The manifest carries no host-version field, so a mismatch shows up only at the load check.

## Install and enable

```sh
zeroclaw --config-dir ./cfg plugin install ./tool-package
zeroclaw --config-dir ./cfg plugin list --verify
zeroclaw --config-dir ./cfg plugin info tool-fixture
zeroclaw --config-dir ./cfg plugin remove tool-fixture
```

Verified with the tool fixture (`capabilities = ["tool"]`, `permissions = ["config_read"]`): install seeds a `[[plugins.entries]]` row, `plugin list --verify` prints `[loads]`, `plugin info` prints `Loads: yes. The component instantiates against this host's WIT world.`, and `remove` deletes it. A package directory holds `manifest.toml` and the `.wasm` named by its `wasm_path`.

Install constraints, as observed:

- A package that does not load is refused: for non-component bytes the error is `does not load against this host: failed to load WASM component`.
- `plugin install --no-verify` skips only that load check. Run on the same bad bytes it installed them and warned that the plugin "will be skipped at startup". It does **not** bypass the channel refusal below. Avoid `--no-verify`.
- An unknown capability or permission fails manifest parsing and lists the accepted values. Capabilities: `tool`, `channel`, `memory`, `observer`, `skill`. There is no listener, server, webhook, exec or shell permission to request.
- Installing grants nothing at runtime. `plugins.enabled`, `plugins.auto_discover` and `plugins.security.signature_mode` read `false`, `false` and `disabled` on a fresh config (verified). Network reach exists only through the operator's `plugins.entries.<key>.egress_hosts`; a manifest `[egress]` table is an attestation, not a grant.
- Registry search and install-by-name, `plugin update`, and `plugin migrate` fetch or move third-party content and were **not run**.

## Messaging-channel plugins are unsupported

A package declaring the `channel` capability is refused at `plugin install`, `plugin update` and `plugin bind` in a `local-agent-minimal` build, even with the plugin host compiled in:

```text
Error: plugin `channel-fixture` declares the `channel` capability, which the local-agent-minimal profile does not provide (no channel runtime, daemon or listener). Install a tool-only plugin, or use the standard build.
```

Verified, with and without `--no-verify`. A config that contains `[channels.plugin.<alias>]` is refused when the binary starts: `this config enables surfaces the local-agent-minimal profile does not provide ([channels.plugin] configured)` (verified on the minimal build with `zeroclaw agents list`). `daemon`, `gateway start` and `channel start` print `is not available in the local-agent-minimal profile` (verified). Tool plugins still install. The component-level channel ABI works in the test suite, which is why the refusal is a policy of this profile and not an ABI limit.

## Security controls

| Control | Behavior | Evidence |
| --- | --- | --- |
| Opt-in host | not compiled in the minimal profile; off by default when compiled | verified above |
| Runtime off by default | `plugins.enabled` and `plugins.auto_discover` default `false` | verified above |
| Signatures | `signature_mode` is `strict` (reject unsigned or untrusted), `permissive` (warn) or `disabled` (default). Official registry packages are unsigned, so none install under `strict` | defaults verified; enforcement from tests (`admit_source_rejects_unsigned_plugin_before_load_verification`), not re-run |
| Egress | deny by default; private, link-local and cloud-metadata destinations and redirects to them refused; operator allow-list only | tests (`egress_plugin_e2e`), not re-run |
| No inbound surface | sockets and WebSockets are outbound and host-mediated; no server permission exists | tests (`socket_plugin_e2e`, `websocket_plugin_e2e`), not re-run |
| Resource limits | fuel, timeout and memory limits trap the guest and fail the call | tests (`tool_plugin_timeout_e2e`, `reference_plugin`), not re-run |
| Secrets | a guest reads only top-level string properties marked `x-secret: true` in its manifest schema. The plaintext does reach plugin code, so a plugin you install is trusted with the secrets you give it | documented in the plugin protocol page; not run here |

## Known limitations

- `wit/v0` is unstable: expect to rebuild plugins on any WIT change.
- Every tool call recompiles the component; there is no compilation cache.
- Hardened environments that forbid executable memory (for example systemd `MemoryDenyWriteExecute=yes`, SELinux `execmem` denial) refuse every plugin load.
- Unknown manifest keys (`listen`, `allowed_hosts`, `[network]`) are ignored, not rejected, and grant nothing.
- The `observer` capability is admitted by the manifest but nothing activates it.
- A precompiled `.cwasm` is bound to the exact wasmtime build.
- `zeroclaw --help` on the minimal build still lists `gateway`, `daemon`, `channel`, `relay` and `service`; they are refused when run. `security status` prints a gateway line even though no gateway is compiled in. Treat both as inherited text, not as capability.

## Tested versus planned

| Capability | Status |
| --- | --- |
| Plugin host absent from the minimal profile | tested (command output above) |
| Tool plugin install, load check, info, remove on the opt-in build | tested (commands above, one fixture) |
| Channel plugin refusal at install and config | tested (commands above and `component::minimal_profile_plugins::*`) |
| Agent loop calling a plugin tool; egress, socket, memory, timeout fixtures | tested in `cargo test -p zeroclaw-plugins --features plugins-wasm-cranelift` (existing evidence, not re-run here) |
| ABI mismatch refusal, signature enforcement | tested in unit tests, not run as commands here |
| Precompiled `.cwasm` | unverified |
| Registry and community plugins, any third-party component | unverified; not run |
| Compatibility with arbitrary ZeroClaw plugins | **not claimed**; only the listed fixtures were exercised |
| A stable, frozen plugin ABI | planned; `wit/v0` is experimental today |
