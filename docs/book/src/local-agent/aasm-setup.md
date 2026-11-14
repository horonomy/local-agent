# AASM: set up, disable and troubleshoot

Read [AASM (optional integration)](./aasm.md) first for the lifecycle states. Nothing on this page asks you to run a privileged command, open a port or start a listener. Local Agent only ever connects out to a Unix socket you name. If your AASM deployment needs elevated rights or a network listener, that is AASM's requirement, outside this page, and needs your administrator's approval.

None of the commands below was run against a built binary for this page, and managed mode has never been run against a live AASM runtime (see [what is not claimed](./aasm.md#what-is-and-is-not-claimed)). The configuration keys and rules are taken from `crates/zeroclaw-config/src/governance.rs`; the expected reason codes are those asserted by tests named in each section.

## Standalone quickstart (no AASM)

This is the default and the supported path. There is nothing to install or enable.

1. Build and configure as in [Getting Started](./getting-started.md). The `local-agent-minimal` build does not contain the managed transport.
2. Leave `[governance]` out of your config. An absent section is standalone (test `absent_section_deserializes_to_standalone`); the default mode is `standalone`.
3. Run an agent: `zeroclaw agent -a <alias> -m "hello"`.

Tool calls are decided by the local engine from the agent's risk profile. Policy decisions are written to a local log unless you turn it off (see [audit settings](#audit-settings)). No AASM software, socket or environment variable is involved.

To state the default explicitly, which changes nothing:

```toml
[governance]
mode = "standalone"
```

## Enable managed mode

Managed mode is for a machine where an AASM runtime already runs under **your own user account** and listens on a socket you can name. This page does not explain how to install or configure AASM, and it is not verified that a stock AASM runtime can listen where Local Agent accepts (see [socket requirements](#socket-requirements)).

1. **Build with the feature.** The managed transport is the Cargo feature `aasm-managed`, outside `default` and outside `local-agent-minimal`. The `src/lib.rs` guard that rejects `local-agent-minimal` combined with ingress features does not list `aasm-managed`, so the combination below is expected to compile. The CI job `Test (AASM managed policy)` only runs `cargo test -p zeroclaw-runtime --features aasm-managed --lib security::` and `--test policy_bypass`. It does **not** build the root binary with `--no-default-features --features local-agent-minimal,aasm-managed`, so this full binary build line is unverified by CI and was not built for this page.

   ```sh
   cargo build --release --locked --bin zeroclaw \
     --no-default-features --features local-agent-minimal,aasm-managed
   ```

   It is Unix only. Without the feature, a managed configuration cannot work (see [Misconfigured](#managed-examples-denial-and-disconnect)).

2. **Choose a socket location you own.** Create a private directory (`0700` is recommended; the code enforces only that it is not group or world writable), and have the AASM runtime place its socket there. Name it `aa-runtime-<agent_id>.sock`, where `<agent_id>` should match what the runtime derives its key from. The adapter accepts any non-empty name; it takes the id from the file name, so a mismatch is not caught by Local Agent.

   ```sh
   mkdir -m 700 "$HOME/aasm-run"
   ```

3. **Set the configuration.** Write absolute paths with no symlink component (on macOS, write `/private/...` rather than `/var/...` or `/tmp/...`). Use your real home path, not `$HOME`: the config file is not shell-expanded.

   ```toml
   [governance]
   mode = "managed"

   [governance.aasm]
   socket_path = "/home/alice/aasm-run/aa-runtime-localagent.sock"
   timeout_ms = 2000
   ```

   `zeroclaw config set governance.mode managed` and `zeroclaw config set governance.aasm.socket_path <path>` write the same keys. `socket_path` has no default: the path must be explicit so another local user cannot pre-create it. Leave `expected_peer_uid` unset (it then means your own uid); `credential_token_file` is accepted only as an absolute path and is never read in this build.

4. **Start the runtime, then Local Agent.** The order does not matter for safety: a missing runtime denies tool calls rather than starting in a weaker mode.

5. **Check a locally allowed call.** Ask the agent to use a tool your risk profile already allows, such as `file_read`. With the runtime allowing it the call runs. With the runtime denying it the agent reports `Tool call not executed: 'file_read' was denied by policy (aasm_deny).` Then look at the [decision log](./aasm-policy-ipc.md#decision-log).

Environment variables: Local Agent reads no AASM environment variable. The general `ZEROCLAW_*` configuration override applies to `[governance]` keys as it does to any other (see [Environment variables](../reference/env-vars.md)), so treat the process environment as part of the configuration.

### Socket requirements

The path is checked on every query, not just at startup:

- absolute, and no symlink at any component, including the socket file itself;
- every ancestor directory is a real directory owned by you or root and not group or world writable. A sticky directory such as `/tmp` is tolerated only as a distant ancestor, never as the immediate parent;
- the socket file is a socket owned by your uid, and the connected peer must be your uid. **A root-owned runtime socket is rejected.** `expected_peer_uid` may only be unset or equal to your own effective uid, because the AASM server accepts only its own uid.

AASM's own default path, `/tmp/aa-runtime-<id>.sock`, is refused by design. Whether a stock AASM runtime can be told to listen on a compliant path is not verified here.

## Disable managed mode

Disabling is a local, unprivileged edit and returns the session to standalone behavior:

1. Set `mode = "standalone"` (or delete the `[governance]` section), for example `zeroclaw config set governance.mode standalone`.
2. Start a new session. The mode is read when a session's tools are assembled, so a running session keeps its mode.
3. Optionally stop the AASM runtime. Local Agent no longer contacts it.

Do **not** "disable" by removing the socket or stopping the runtime while `mode = "managed"`: that is the disconnected state, and every tool call is denied. Removing `aasm-managed` from the build while the config still says managed is the misconfigured state and denies every call too.

## Audit settings

```toml
[governance.audit]
enabled = true
max_size_mb = 8
generations = 3
```

These are the defaults. Validation rejects `max_size_mb * (generations + 1)` above 64 MiB (`generations = 7` at 8 MiB passes; 8 fails) and `max_size_mb = 0`. That is a validation rule only: config load turns a validation error into a warning and the log writer does not clamp, so do not treat 64 MiB as enforced at runtime. With `max_size_mb = 0` every record fails and every call is denied `audit_write_failed`. Disabling the log (`enabled = false`) removes the evidence trail but not the policy. Details: [decision log](./aasm-policy-ipc.md#decision-log).

## Managed examples: denial and disconnect

Each row assumes `mode = "managed"` and a tool the local engine allows. The expected reason code is what the named test asserts; the text the agent sees is always `Tool call not executed: '<tool>' was denied by policy (<code>).`

| Situation | Expected code | Test asserting it |
| --- | --- | --- |
| Runtime not running or socket file missing (disconnected) | `authority_unavailable` | `unreachable_socket_denies_with_no_fallback` |
| Runtime accepts then closes before replying | `authority_unavailable` | `server_closing_mid_exchange_denies` |
| Runtime closes in the middle of a frame | `malformed_response` | `server_closing_mid_exchange_denies` |
| Runtime does not answer within `timeout_ms` | `timeout` | `timeout_denies` |
| Runtime answers deny, pending, redact, unspecified or an unknown code | `aasm_deny` | `deny_verdict_is_aasm_deny_without_reply_text`, `non_allow_verdicts_deny` |
| Runtime sends a bad handshake, unknown tag, undecodable or oversize (over 1 MiB) frame | `malformed_response` | `bad_handshake_denies`, `unknown_tag_denies`, `undecodable_payload_denies`, `oversize_frame_denies_before_reading_payload` |
| Socket directory group or world writable, symlinked path, or wrong owner (no connection is made) | `authority_unavailable` | `group_or_world_writable_directory_is_refused_before_connect`, `symlinked_socket_path_is_refused_before_connect`, `socket_owned_by_another_uid_is_refused` |
| Misconfigured: build lacks `aasm-managed`, no socket path, `expected_peer_uid` not your uid, `timeout_ms` out of range | `authority_unavailable` for every call, no fallback | `managed_mode_fails_closed` (gate), `expected_uid_other_than_own_euid_is_rejected_at_construction` |
| Decision log cannot be written | `audit_write_failed` | `write_failure_denies_and_mints_no_ticket` |

To watch a disconnect, point `socket_path` at an absolute path where nothing listens and ask for a locally allowed tool call: it is denied `authority_unavailable`. Point it at a runtime that is running again and the next call is decided normally; there is no restart step.

An `allow` from the runtime is recorded with reason `local_allow` (there is no separate AASM-allow code); only the decision source and the opaque `aasm_decision_id` show that AASM was consulted.

## Troubleshooting

The adapter deliberately does not say why a socket was refused: a missing socket, a symlink, a writable directory, a wrong owner and a peer uid mismatch all produce `authority_unavailable`. Diagnose with ordinary unprivileged inspection, for example `ls -ld` on each directory from `/` down to the socket and `ls -l` on the socket.

| Symptom | Likely cause and check |
| --- | --- |
| Every tool call is denied `authority_unavailable`, even with the runtime running | Socket path rejected. Check each ancestor for symlinks (macOS `/var`, `/tmp`), group or world write, and owner. Check the socket is owned by your uid, not root. Check `expected_peer_uid` is unset or your uid. Check the file name's agent id matches what the runtime derives its key from (the adapter accepts any non-empty name). |
| Same, but only after switching builds | The build lacks `aasm-managed`. Rebuild with the feature, or set `mode = "standalone"`. |
| `denied by policy (timeout)` | The runtime did not reply within `timeout_ms` (default 2000, allowed 100 to 10000). Check the runtime is responsive before raising the value. |
| `denied by policy (malformed_response)` | The runtime's reply did not match the mirrored protocol. This is the expected symptom if a real runtime disagrees with the unverified wire layout; report the AASM version. |
| `denied by policy (aasm_deny)` | The runtime refused. Local Agent shows no AASM reason text by design; read the decision in AASM. |
| `denied by policy (audit_write_failed)` | The decision log could not be written (disk full, unwritable directory, planted symlink). Fix the directory, or set `[governance.audit] enabled = false` knowingly. Tool use stays blocked until then. |
| `denied by policy (local_deny)`, `excluded`, `network_not_granted`, `unknown_capability` | The local engine denied before AASM was asked. These come from the agent's risk profile, not AASM. See [reason codes](./aasm-policy-ipc.md#reason-codes). |
| `denied by policy (contract_version_mismatch)` | A request carried a contract version other than 1. Not expected from Local Agent itself. |
| A config validation warning mentions `governance` | `Config::load_or_init`, which `zeroclaw agent` uses (read in `src/main.rs`, not run), logs validation errors as a warning and continues so you can repair the config. A bad managed block, including managed in a build without `aasm-managed`, therefore does not stop startup: every tool call is denied `authority_unavailable`, with no standalone fallback. The daemon is not in the minimal profile and its load path was not checked. Fix the key named in the message. |
