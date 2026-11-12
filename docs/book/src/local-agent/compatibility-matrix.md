# Compatibility matrix (MCP, Skills, WASM plugins)

This page records what was actually loaded and run against the pinned source, per sample and per claim. It is **not a promise of compatibility** with any ecosystem, registry, server, skill collection or plugin. A sample that is not listed is not claimed. Nothing here re-enables a channel, gateway, daemon or listener, and the optional [AASM](./aasm.md) integration is neither required nor assumed.

## Pinned inputs

| Item | Pin | Source |
| --- | --- | --- |
| Upstream baseline | `9dbb57231f48e26fee56243de4f7845924ce4f04` (`v0.8.7-35-g9dbb57231`) | `UPSTREAM.md` |
| Workspace version | `0.8.7`, edition 2024 | `Cargo.toml`, `UPSTREAM.md` |
| Plugin ABI | WIT package `zeroclaw:plugin@0.1.0` (`wit/v0`, experimental, no `.frozen`) | `HOST_WIT_PACKAGE` / `HOST_WIT_VERSION` in `crates/zeroclaw-plugins/src/component.rs`, `wit/VERSIONING.md` |
| WASI | `wasi:*@0.2.x`, target `wasm32-wasip2` | `check_abi_pin` in `component.rs` |
| MCP protocol revision sent by the client | `2024-11-05` | `MCP_PROTOCOL_VERSION` in `crates/zeroclaw-tools/src/mcp_protocol.rs` |
| MCP transport covered for Local Agent | stdio, newline-delimited JSON-RPC | [MCP and Skills](./mcp-skills.md) |
| Skill formats | `SKILL.md` (frontmatter `name`, `description`), `SKILL.toml`, registry `manifest.toml` | [Skills](../tools/skills.md) |
| Plugin manifest vocabulary | capabilities `tool`, `channel`, `memory`, `observer`, `skill`; permissions `http_client`, `websocket_client`, `socket_client`, `file_read`, `file_write`, `config_read`, `memory_read`, `memory_write`, `state_read`, `state_write` | `PluginCapability` / `PluginPermission` in `crates/zeroclaw-plugins/src/lib.rs` |

`wit/v0` is experimental. Until it is frozen, a component must be rebuilt against the WIT shipped by the host it targets, and a prebuilt component from an earlier experimental world is not a conformance target (`wit/VERSIONING.md`).

Totals (35 rows): 18 works unchanged, 2 requires limited adaptation, 9 unsupported, 6 unverified. P6 is counted as unsupported because its runtime claim is unsupported even though its component-level ABI claim works.

## How to read a row

Status is one of: **works unchanged**, **requires limited adaptation**, **unsupported** (with the concrete reason), **unverified** (with the reason). Each row names the kind of claim, because they fail differently:

- *protocol*: the wire or ABI contract (MCP JSON-RPC, WIT/WASI).
- *configuration*: what the operator must configure or grant.
- *source*: where the artifact comes from and whether it is local.
- *ABI*: the component interface version.
- *runtime*: what happens when it executes.

Build profiles: **minimal** is `local-agent-minimal` (no WASM host, no channel, no listener). **plugins opt-in** is minimal plus `plugins-wasm-cranelift` (or `plugins-wasm-runtime-only` for precompiled `.cwasm`). **standard** is the upstream default build and is outside Local Agent's promise.

Samples are local only: in-repo files, the plugin fixtures under `crates/zeroclaw-plugins/tests/fixtures`, and synthetic stdio stubs. No third-party code was downloaded or run.

## MCP (stdio client)

Profile: all profiles (MCP is part of the minimal closure).

| # | Sample | Claim | Status | Evidence |
| --- | --- | --- | --- | --- |
| M1 | Local `/bin/sh` stdio stub: `initialize`, `tools/list`, `tools/call` | protocol, runtime | works unchanged. Tools appear as `<server>__<tool>`; the client sends `protocolVersion` `2024-11-05` | `mcp_compat_matrix::stdio_sample_discovers_namespaces_and_invokes_unchanged` |
| M2 | `scripts/local_agent/mcp_fixture.py` (tools `echo`, `fail`; 8 KiB line bound; `2024-11-05`) | protocol, runtime | works unchanged at the fixture-contract level (14 Python subprocess tests). The agent-loop journeys `granted` (2 provider requests), `ungranted` (1), `provider_timeout` (2), `cancellation` (2) and `recovery` (3) all passed against a debug `local-agent-minimal` build (see Reproduce) | `python3 -m unittest scripts.local_agent.test_mcp_fixture` |
| M3 | Server that answers `initialize` with a different `protocolVersion` | protocol | unverified. Known limitation: `handshake` in `mcp_client.rs` sends `protocolVersion` `2024-11-05` and reads only the server's `capabilities`; the server's `protocolVersion` is never compared, logged or refused, so no revision is negotiated and a mismatch surfaces only as a later call failure. Not changed here because revisions are dates, not majors, and no safe refusal rule is defined | `server_claiming_another_protocol_revision_is_not_rejected` (characterization) |
| M4 | Server tool/result text containing instructions (`IGNORE ALL PREVIOUS INSTRUCTIONS ...`) | runtime | works unchanged. Description and result are carried as data; the call succeeds and the text is returned verbatim, never interpreted | `injected_text_stays_data_and_approval_flag_is_not_forwarded` |
| M5 | Caller-supplied `approved` flag on a tool call | runtime | works unchanged. The flag is stripped and never reaches the server | same test |
| M6 | Call to a tool the server never advertised | runtime | works unchanged. Fails with `unknown MCP tool` naming the tool; nothing is routed | `unknown_tool_name_fails_with_a_named_error` |
| M7 | Server environment | configuration | works unchanged. Only a fixed allow-list is inherited; anything else must be set in the server's `env` | in-module `stdio_child_env_*` tests (HORO-2388) |
| M8 | Server exits at handshake, crashes mid-call, oversized line, deadline, `isError` | runtime | works unchanged. Each fails the call (or connection) without replaying the call and reaps the child | in-module `stdio_*` lifecycle tests (HORO-2388) |
| M9 | Agent with no granted bundle | configuration | works unchanged. The server is never spawned and its tools are not offered | `ungranted_or_excluded_agent_never_spawns_the_stdio_server`, golden `ungranted` |
| M10 | Resources and prompts | protocol | works unchanged when the server advertises the capability; otherwise the call is refused locally | `list_resources_gated_when_unsupported`, `get_prompt_gated_when_unsupported` |
| M11 | HTTP and SSE MCP transports | protocol, runtime | unverified. Present in code and inherited tests; not covered by Local Agent docs, and the URL is operator-configured network access that this ticket does not exercise | n/a |
| M12 | Real community servers (for example `npx`/`uvx`-launched ones) | source | unverified. They require fetching third-party code, which this ticket does not do | n/a |

Instruction for supported use: define a stdio server under `mcp.servers`, list it in a `[mcp_bundles.<alias>]`, and name the bundle in `agents.<alias>.mcp_bundles`. Put every variable the server needs in its `env`. See [MCP and Skills](./mcp-skills.md).

## Skills

Profile: all profiles. Local Agent loads skills from the agent workspace (`<install>/agents/<alias>/workspace/skills/<name>/`), assigned bundles and the built-ins. `.claude/skills` is not a discovery location; the repository's `.claude/skills` entries below are used as sample content by pointing the loader at that directory.

| # | Sample | Claim | Status | Evidence |
| --- | --- | --- | --- | --- |
| S1 | `.claude/skills`: `github-pr`, `squash-merge`, `github-pr-review-session`, `github-issue`, `changelog-generation`, `feature-matrix-parity`, `pr-architecture-check`, `skill-creator`, `wit-breaking-change-check` (9) | source, runtime | works unchanged as instruction-only skills (audit clean, no tools, default no-script policy). Placing one in a workspace `skills/` directory is a configuration step; invoking their content was not exercised | `skills_compat_matrix::in_repo_markdown_skills_that_load_unchanged` |
| S2 | `.claude/skills/zeroclaw` and `.claude/skills/github-issue-triage` | source | requires limited adaptation. Each links to repository files outside the skill directory; the audit drops the skill with `markdown link escapes skill root`. Copy the linked material inside the skill or remove the link | `in_repo_skills_with_repo_relative_links_are_dropped_with_a_reason` |
| S3 | Built-in `zeroclaw-docs` | source | works unchanged. Audit clean | `builtin_docs_skill_passes_the_audit` |
| S4 | Skill carrying `install.sh` | runtime | unsupported by default (secure-default script policy). The drop is attributed to script policy; `skills.allow_scripts = true` is a deliberate opt-in | `shell_script_payload_is_refused_unless_scripts_are_allowed` |
| S5 | Skill text that tries to widen authority (autonomy, channel, listener) | runtime | works unchanged. Loaded as prompt text; registers no tool, channel or listener | `injection_text_in_a_skill_registers_no_authority` |
| S6 | `SKILL.toml` tool kinds, `builtin`/`mcp` delegation, name allowlist, unknown tool kinds | runtime | works unchanged per the HORO-2389 contract; not re-derived here | `cargo test -p zeroclaw-runtime --lib skills::` (290 passed) |
| S7 | `skills install <url>`, community registries | source | unverified. Needs network and third-party content | n/a |

## WASM plugins

Profile: **plugins opt-in only.** The minimal closure contains no `zeroclaw-plugins` and no `wasmtime`. Even where the host is compiled in, `plugins.enabled` and `plugins.auto_discover` default to `false`, and `plugins.security.signature_mode` defaults to `disabled` (an operator who wants strict signing sets `strict`).

| # | Sample | Claim | Status | Evidence |
| --- | --- | --- | --- | --- |
| P1 | `tool-fixture` (tool, `config_read`) | ABI, runtime | works unchanged on plugins opt-in; unsupported on minimal (host not compiled) | `reference_plugin*`, `tool_plugin_e2e` |
| P2 | `tool-secret-fixture`, `tool-sequential-fixture`, `tool-timeout-fixture` | ABI, runtime | works unchanged on plugins opt-in. Fuel/timeout/memory limits trap the guest and fail the call | `tool_plugin_e2e`, `sequential_egress_e2e`, `tool_plugin_timeout_e2e`, `reference_plugin` |
| P3 | `tool-socket-fixture`, `tool-websocket-fixture` (`socket_client`, `websocket_client`) | ABI, runtime | works unchanged on plugins opt-in. Outbound only and host-mediated; the plugin never listens | `socket_plugin_e2e`, `websocket_plugin_e2e` |
| P4 | `memory-fixture` | ABI, runtime | works unchanged on plugins opt-in | `memory_plugin_e2e` |
| P5 | `egress-fixture` and a destination outside the operator allow-list | runtime | works unchanged. Deny by default; outside-allow-list, private-address and cloud-metadata destinations and redirects to them are refused. A manifest `[egress]` table is an attestation, never a grant | `egress_plugin_e2e`, `host::tests::valid_egress_declaration_is_admitted_but_confers_no_reach` |
| P6 | `channel-fixture`, `channel-egress-fixture`, `channel-websocket-fixture`; any package declaring the `channel` capability | ABI, runtime | ABI: works at the component level (`channel_plugin_e2e`). Runtime on Local Agent: **unsupported, fails closed**. In a `local-agent-minimal` build, even with the plugin host compiled in, `plugin install`, `plugin update` and `plugin bind` refuse such a package ("plugin `<name>` declares the `channel` capability, which the local-agent-minimal profile does not provide ..."), and a `[channels.plugin.<alias>]` config section is refused at startup ("[channels.plugin.<alias>] configured", naming the profile). Tool plugins still install. `channel start` and the daemon are refused as before. Standard builds are unchanged | `component::minimal_profile_plugins::*` (3 tests, minimal+`plugins-wasm-cranelift`), `ingress_commands_are_refused_before_any_setup` |
| P7 | Component importing another `zeroclaw:plugin/*` version, an unversioned one, or non-`0.2.x` `wasi:*` | ABI | unsupported. Install, `plugin info` and startup fail with `plugin ABI mismatch`, naming the interface and the pinned version; rebuild against this host's `wit/v0` | `abi_pin_*`, `install_gate_*`, `load_component_rejects_a_component_built_for_another_plugin_abi` (HORO-2390) |
| P8 | Core module, `wasm32-wasip1` build or non-component bytes | ABI | unsupported. Refused with `failed to load WASM component` | `verify_component_loads_accepts_the_fixture_and_rejects_a_non_component` |
| P9 | Components built for earlier experimental worlds | ABI | unsupported by policy (`wit/VERSIONING.md`); rebuild | doc |
| P10 | Unsigned package under `strict` | configuration | unsupported. Refused before the component is loaded | `host::tests::admit_source_rejects_unsigned_plugin_before_load_verification` |
| P11 | Unknown capability (`listener`, `server`, `webhook`, `daemon`) or permission (`socket_server`, `listen`, `bind`, `shell`, `exec`) in a manifest | protocol | unsupported. Install fails, naming the rejected value and listing the accepted ones. No server-side or exec permission exists | `compat_matrix::install_refuses_unknown_capability_with_the_accepted_list`, `install_refuses_unknown_permission_such_as_a_listener` |
| P12 | Manifest with no capabilities | protocol | unsupported (`declares no capabilities`) | `install_refuses_a_manifest_that_declares_no_capability` |
| P13 | `observer` capability | runtime | unsupported. The manifest is admitted but the host has no observer loader, so it is neither a tool nor a channel plugin and nothing activates it | `observer_only_plugin_is_never_activated_as_tool_or_channel` |
| P14 | Keys the manifest schema does not define (`listen`, `allowed_hosts`, `[network]`) | configuration | requires limited adaptation: remove them. They are ignored, not rejected, and confer no permission and no egress | `unknown_manifest_keys_are_ignored_and_grant_nothing` |
| P15 | Precompiled `.cwasm` (`plugins-wasm-runtime-only`) | ABI, runtime | unverified. It is bound to the exact wasmtime build; the pin check is covered for a deserialized component, but no `.cwasm` sample was produced here | `abi_pin_applies_to_a_deserialized_cwasm_component` |
| P16 | Registry or community plugins | source | unverified. Needs a network fetch of third-party code | n/a |

Instruction for supported use (plugins opt-in build only): build the component for `wasm32-wasip2` against this tree's `wit/v0`; write a `manifest.toml` like `crates/zeroclaw-plugins/tests/fixtures/tool-fixture/plugin-manifest.toml`; install it, then check it with `plugin info` (this runs the load gate); grant egress only through the operator's `plugins.entries[].egress_hosts`. For migration from an older experimental build, follow the "Migration guide for plugin authors" in `wit/VERSIONING.md`.

## Fail-closed summary

| Surface | Behavior | Where it is decided |
| --- | --- | --- |
| Messaging channels, gateway, ACP, daemon, relay, tunnels | Not compiled; combining the profile with one is a `compile_error!`; the commands that start them are refused at startup with an actionable message | `src/lib.rs`, `src/main.rs`, [Minimal profile](../setup/local-agent-minimal.md) |
| Channel plugins and `[channels.plugin.*]` config | Refused at install, update, bind and startup in the minimal profile (P6) | `src/main.rs` `minimal_profile_plugin_refusal`, `minimal_profile_config_violations` |
| Incompatible ABI, non-component, wrong WASI | Refused at install and load with a named interface and the pinned version | P7, P8 |
| Unapproved egress | Denied by default; the allow-list is operator config | P5 |
| Privileged or server-side plugin permissions | Do not exist in the manifest vocabulary; install fails | P11 |
| Unsolicited listeners from plugins | Not expressible: sockets and WebSockets are outbound, host-mediated | P3, P11 |
| MCP privilege escalation (`approved`), unknown tools, ungranted servers | Not forwarded, not routed, not spawned | M5, M6, M9 |
| Skill authority and scripts | Skills grant no authority; shell scripts refused by default | S4, S5 |

## Known gaps

- MCP client does not negotiate or reject by protocol revision (M3).
- Unknown manifest keys are ignored rather than rejected (P14).
- `signature_mode` defaults to `disabled` (P10).

## Reproduce

```sh
cargo test -p zeroclaw-plugins --features plugins-wasm-cranelift
cargo test -p zeroclaw-tools --test mcp_compat_matrix
cargo test -p zeroclaw-tools --lib mcp_
cargo test -p zeroclaw-runtime --test skills_compat_matrix
cargo test -p zeroclaw-runtime --lib skills::
python3 -m unittest scripts.local_agent.test_mcp_fixture
```

```sh
cargo test --no-default-features --features local-agent-minimal,plugins-wasm-cranelift --test component minimal_profile
cargo build --no-default-features --features local-agent-minimal --bin zeroclaw
python3 scripts/local_agent/golden_journey.py --binary target/debug/zeroclaw --fixture scripts/local_agent/mcp_fixture.py --case all --deadline 40
python3 scripts/local_agent/listener_probe.py --binary target/debug/zeroclaw
python3 scripts/local_agent/egress_probe.py --binary target/debug/zeroclaw --mode approved
```

Observed outcomes (macOS aarch64, debug build of `local-agent-minimal`, Python 3.13.5, synthetic loopback provider, no external network):

- `local-agent-minimal` combined with `plugins-wasm-cranelift` compiles; the compile guard does not list the plugin host features, so the combination is legal. Standard features plus `plugins-wasm-cranelift` also compile.
- `component minimal_profile`: 14 passed (3 new).
- `golden_journey.py --case all`: `case_pass` true; `granted` 2, `ungranted` 1, `provider_timeout` 2, `cancellation` 2, `recovery` 3 provider requests, all passed.
- `listener_probe.py`: verdict `no-listener`, positive control seen, `agent_alive` true, no listeners.
- `egress_probe.py --mode approved`: verdict `PASS`, 3 approved connections, 0 other connections, no outbound attempt inferred.

The plugin fixtures need the `wasm32-wasip2` target. The `skills_compat_matrix` samples read `.claude/skills` in this repository, so a change to those skills can change S1 and S2; update this page with the test.

Link check: `mdbook build` could not run locally (its `peer-groups` preprocessor binary is not built) and no mdbook link-check plugin is installed; the relative links on this page were verified with `lychee --offline` (5 links, 0 errors).
