# Scoped desktop control (`computer_use`)

Status: guards, policy and config are implemented and tested with a fake
backend. **No native input backend ships yet**: on every OS the tool answers
with an explicit error and performs no input. Nothing here is verified against
a real desktop.

## What it is

A separate tool, `computer_use`, for click, type and key actions in one
allowlisted window. It is built only with the `local-desktop` feature and
registered only when `[computer_use] enabled = true` and `[tools].optional`
includes `computer_use` or `*`.

It is **not** the `browser` tool. The `browser` tool's legacy `computer_use`
backend (an HTTP sidecar on 127.0.0.1:8787 that is not in this repository) is
still refused by the strict local browser profile; this tool is the only
desktop-input surface for the local profile.

## Guards (enforced in-process, before any backend call)

- Empty `window_allowlist` denies every action.
- Each call names the target `app` (name or bundle id). The frontmost window
  must be that app and match an allowlist entry; a title substring is optional
  (`"Notes"`, `"com.apple.Notes"`, `"Notes::Meeting"`). `*` is rejected and a title part needs at least 3 characters. Prefer bundle ids: an app name or short title substring can match more windows than intended.
- Click coordinates must be inside that window's bounds.
- `max_actions_per_session` and `max_actions_per_minute` limit dispatches;
  each action has an `action_timeout_ms` deadline. A timeout or a cancelled
  call drops the backend future, which a backend must turn into killing any
  helper it started.
- Only a letter/digit, F1-F12 and a short list of named keys are accepted (no modifiers, chords or system keys); typed text
  is limited to 500 characters, rejects control, private-use, invisible
  formatting and noncharacter code points (other unassigned code points are not
  screened), and never appears in results, errors or debug
  output.
- Screenshots are out of scope here and answer "use the capture tool".

## Policy

Capability `Desktop`: `computer_use` needs its own entry in `allowed_tools`; a
wildcard or a listed `browser` grants nothing. The policy gate treats it as an
Ask on **every** call regardless of `always_ask`, and the approval ticket is
bound to that call's arguments and is single use. Read-only autonomy denies it.

## Config

```toml
[computer_use]
enabled = false            # default
window_allowlist = []      # empty = deny everything
max_actions_per_session = 20
max_actions_per_minute = 10
action_timeout_ms = 5000
```

## Known limits

- The session and rate budgets live in the tool instance; every tool-registry
  rebuild (each agent build and each channel turn) starts a fresh budget.
- Typed text is kept out of this tool's results and errors and out of the
  turn-loop argument logs in `call_prep.rs`. It is still part of the call
  arguments, which approval prompts, transcripts and any other logging path may
  carry; this is not verified exhaustively.

- The window check and the dispatch are separate backend calls; a window change
  between them is not detected.
- The Ask is enforced by the local policy engine. A remote policy authority is
  not verified to preserve it.

## Owner-only actions

- macOS needs Accessibility consent (System Settings, Privacy & Security) for
  any future native backend. ZeroClaw never requests, detects-and-grants or
  scripts it; the machine owner enables it.
- Decide whether to ship a native macOS backend (new dependency review under
  `deny.toml`) and run a manual check on a non-corporate machine.
- Windows and Linux have no backend and report "unsupported on this OS".
