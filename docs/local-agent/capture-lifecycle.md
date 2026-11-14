# Local screenshot and screen recording

The `capture` tool takes a screenshot or a short screen recording of **one
selected window or region**, keeps it on this machine, and removes it on a
schedule you control. It is off by default, compiled only into builds with the
`local-desktop` feature (or `tools-external`), and the idle core embeds no video
recorder: recording runs the OS `screencapture` command on demand.

## Turning it on

Build with `--features local-desktop`, then in the config:

```toml
[capture]
enabled = true                  # default: false (tool is not registered)
retention_ttl_secs = 3600       # captures older than this are purged
max_recording_secs = 60         # hard maximum for one recording, 1..=300
max_total_bytes = 536870912     # size cap for the whole captures directory
delete_on_session_end = true    # remove every capture when the session ends
attach_images_to_provider = false   # see "What leaves the machine"
```

The agent must also be granted the tool by name (`allowed_tools` containing
`capture`); a wildcard never grants desktop tools. There is deliberately no
directory key, and unknown keys in `[capture]` are rejected.

## What is stored where

| Item | Location | Permissions |
|---|---|---|
| Directory | `<data_dir>/captures/` | 0700, owned by you, never a symlink |
| Screenshot | `captures/shot-<utc>-<nonce>.png` | 0600 |
| Recording | `captures/rec-<utc>-<nonce>.mov` | 0600 |

Captures are never written into the workspace (which may be git-tracked). The
tool refuses to start if the captures directory would sit inside, or contain,
the workspace. The tool's output carries opaque ids and sizes only, not paths or
contents, and no capture content reaches logs or audit records.

## Actions

| Action | What it does |
|---|---|
| `screenshot` | One image of `window_id` or `region` (`"x,y,width,height"`). A target is required; there is no full-screen capture. |
| `record_start` | Starts a recording of the same kind of target. Only one recording at a time. A timer stops it at `max_recording_secs`; the recorder is also told the same limit. |
| `record_stop` | Stops the recording and keeps it. With `discard: true` it kills the recorder and deletes the partial file. |
| `status` | Recording state, stored count and bytes, limits, last lifecycle event. |
| `list` | Capture ids with sizes and ages. |
| `delete` | Deletes one capture by id. Ids are `[a-z0-9-]` and resolved only inside the captures directory; paths and traversal are rejected. |

Every `capture` call is an **Ask**: the policy gate prompts the operator for each
action, even under Full autonomy and with an empty `always_ask`. In a
non-interactive session an unapproved call does not run, so the recorder is
never started.

## Lifecycle and cleanup

- Expired captures (`retention_ttl_secs`) and the oldest captures beyond
  `max_total_bytes` are purged at `screenshot`, `record_start`, `record_stop`,
  `status` and `list`. An in-progress recording is never purged.
- The recorder runs in its own process group. It is killed (whole group) on
  `discard`, on a failed or cancelled call, when the tool is dropped, and on
  session end. A normal stop sends SIGINT first so the movie is finalized,
  then SIGKILL after 3 seconds. A recorder that crashes is reaped, its partial
  file is deleted, and a new recording may start.
- With `delete_on_session_end = true` every capture is deleted when the tool is
  dropped at session end.

### Delete everything

Stop the agent, then remove the directory contents:

```sh
rm -rf "<data_dir>/captures"
```

Or ask the agent to `list` and `delete` ids. Time Machine local snapshots can
keep deleted blocks until they thin out.

## What leaves the machine

Nothing is sent by this tool. The risk is indirect: if a screenshot is attached
to a model request, its pixels go to the configured provider, which may be a
remote service. So by default **no image is attached**; the tool returns text
only. `attach_images_to_provider = true` is the single switch that lets
screenshots from `capture`, the legacy `screenshot` tool and the `browser` tool
be attached as images. Recordings are never attached. The founder decision on
sending screen content to remote providers is still pending, so the default
stays off and must not be enabled for a remote provider until that decision is
made. Do not point other tools (`file_upload`, `http_request`, `git_operations`)
at the captures directory.

## Honest limits

- **Permission.** On macOS the host binary needs Screen Recording consent. The
  tool checks it with a non-prompting call before capturing and returns an
  actionable error if it is missing. It never prompts for or grants it.
- **Window selection.** Only a numeric native `window_id` or a region is
  accepted. There is no title lookup and no allow-list of windows yet; the
  window allow-list from the computer-use work can feed `window_id` later.
- **Redaction.** The tool bounds what is captured (one window or region) and
  never returns pixels or paths. It cannot blur content inside the selected
  area: do not select a window that shows secrets.
- **Hard kill.** If the agent process is SIGKILLed, no cleanup code runs. The
  recorder stops by itself at the `-V` limit and files are purged by the next
  purge. The tool does not read stale process-group ids from disk to kill them
  (pid reuse would make that unsafe).
- **Unverified on real hardware.** The `screencapture` flags (`-l`, `-R`, `-v`,
  `-V`) and the permission check are covered by argv and fake-backend tests, not
  by a real capture in CI.

## Owner actions

1. Grant Screen Recording to the host binary: System Settings > Privacy &
   Security > Screen & System Audio Recording.
2. macOS ties that consent to the binary's code signature. A rebuilt or
   re-signed binary (including a local ad-hoc build) must be granted again.
3. Decide whether `attach_images_to_provider` may ever be enabled, and for which
   providers.
4. Run one manual end-to-end capture on a non-corporate machine before relying
   on the recording path.
