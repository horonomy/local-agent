# Skills: origins and trust boundaries

A skill is data. Loading one adds instructions (and, for `SKILL.toml`, declared tool wrappers) to an agent's prompt; it never changes autonomy, allowed commands, channels or listeners. This page states where skills come from, what stops a bad one, and what Local Agent does not check. Field-level reference stays on the inherited [Skills](../tools/skills.md) page; the sample-by-sample results are in the [compatibility matrix](./compatibility-matrix.md#skills).

## What was run

The commands below were run against a debug build of the minimal profile (`0.8.7`, macOS aarch64) with an isolated config directory and three throwaway skill directories. Items marked **unverified** were not run.

## Origins

| Origin | Location | Loaded for an agent | Trust note |
| --- | --- | --- | --- |
| Built-in | embedded in the binary (`zeroclaw-docs`) | every agent, lowest precedence | read-only, audited clean (S3) |
| Agent workspace | `<install>/agents/<alias>/workspace/skills/<name>/` | that agent | you placed it |
| Assigned bundle | `<install>/shared/skills/<bundle>/<name>/` | agents listing the bundle in `agents.<alias>.skill_bundles` | you installed or copied it |
| Global | `<install>/data/skills/<name>/` | no agent, automatically | fallback install target only |
| Open-skills | `open_skills_dir` or `$HOME/open-skills` | only when enabled in `skills` config | may clone or pull a community repository: network you did not initiate per run. Leave off unless you point it at a reviewed local copy |
| Plugin skills | a plugin package declaring the `skill` capability | qualified names, never shadow built-ins | needs the opt-in plugin host; see [WASM plugins](./wasm-plugins.md) |

`.claude/skills` and similar directories of other tools are **not** discovery locations. Copy a skill into a workspace or bundle to use it.

Names resolve first-wins, case-insensitively, in the order workspace, open-skills, assigned bundles, built-ins. A skill that loses its name is reported as dropped.

## Audit before load

Every install and every load runs the same audit. You can run it yourself on a directory:

```sh
zeroclaw --config-dir ./cfg skills audit ./release-check
```

Verified outcomes on a directory with a `SKILL.md` and nothing else, one with an extra `install.sh`, and one whose Markdown links outside its own directory:

| Skill content | Result |
| --- | --- |
| `SKILL.md` only | `Skill audit passed (2 files scanned)`, exit 0 |
| plus `install.sh` | `install.sh: script-like files are blocked by skill security policy.`, exit non-zero |
| a Markdown link to a path outside the skill directory (for example `../other/SKILL.md`) | `SKILL.md: markdown link escapes skill root`, exit non-zero |

Install runs the audit and refuses on failure:

```sh
zeroclaw --config-dir ./cfg skills bundle add ops
zeroclaw --config-dir ./cfg skills install ./release-check --bundle ops
zeroclaw --config-dir ./cfg skills list --bundle ops
zeroclaw --config-dir ./cfg skills list --agent assistant
zeroclaw --config-dir ./cfg skills remove release-check --bundle ops
```

Verified: the first install succeeds and prints `Installed into bundle 'ops'`; installing the script-bearing directory fails with `Skill security audit failed`. `skills list --agent assistant` showed only the built-in `zeroclaw-docs`, because the agent did not list the `ops` bundle in `skill_bundles`. That listing is the check that a skill is actually loaded. `skills remove` from a bundle archives the directory under `shared/skills/_deleted/`.

## Trust boundaries

- **No authority.** Skill text that asks to widen autonomy, enable a channel or open a listener is loaded as prompt text and registers nothing (matrix S5).
- **Scripts off by default.** Shell, PowerShell and shebang files are blocked. `skills.allow_scripts = true` is an explicit opt-in; leave it off unless you have read the scripts (S4). `skills test` runs a skill's own `TEST.sh`: read it first.
- **Tool delegation is operator-gated.** A `SKILL.toml` tool of kind `builtin` or `mcp` registers only if the operator's `allowed_tools` / `excluded_tools` admit the target, or the operator lists the composed `<skill>__<tool>` name (S6); at call time the policy gate still evaluates it as its target, so the target must be admitted there too.
- **Names are an allowlist** of letters, digits, space and `- _ . :`, so lookalike or invisible characters cannot make two components disagree about a name.
- **Prompt injection is not solved.** Skill content reaches the model. Compact prompt mode shrinks the prompt; it is not isolation.

Skill tools are evaluated as the built-in they wrap: a `shell` or `script` skill tool as `shell`, an `http` skill tool as `http_request`, a `builtin` or `mcp` tool as its target. They are not registered when that built-in is in `excluded_tools`, and the policy gate denies them at call time on the same basis. Under a non-empty `allowed_tools` the operator must list the wrapper's target (`shell`, `http_request` or the target tool), not only the composed skill name; `http_request` additionally needs explicit listing as a network tool. A name containing `__` is not auto-admitted.

Known limitation, unchanged from the inherited page: composed tool names are not injectively encoded (`a/b__c` and `a__b/c` collide).

## Tested and not tested

| Claim | Status |
| --- | --- |
| Audit blocks scripts and escaping links; local install refuses a failing skill | tested: commands above, plus `skills_compat_matrix` |
| In-repo Markdown skills load unchanged (9 samples); two with repo-relative links need adaptation | tested in the matrix (S1, S2); not re-run for this guide |
| Skill text cannot register authority | tested in the matrix (S5), not re-run |
| `skills install <url>`, `--well-known`, git catalogs, community registries | **unverified**: need network and third-party content, and URL install is network access you initiate |
| Open-skills clone/pull behavior | **unverified** |
| Autonomous skill creation (`[skills.skill_creation]`) | off by default; **unverified** for Local Agent |
| Skill-bundle packages signed and distributed through the plugin system | **planned / opt-in**: depends on the plugin host, absent from the minimal profile |
