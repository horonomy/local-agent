# Security: Overview

An agent that can execute shell commands, open URLs, and write files is a privileged process. ZeroClaw's security model sits on top of every tool call and every channel message, gating what the agent is actually allowed to do at runtime.

Each agent runs under a risk profile and a runtime profile it references; see [Agents](../agents/overview.md) for how those references attach to an agent.

- [The security model](./model.md): the six enforcement layers, additional gates, failure behavior, and the default posture.
- [Autonomy levels](./autonomy.md): the coarse-grained ReadOnly / Supervised / Full knob.
- [Sandboxing](./sandboxing.md): OS-level isolation backends per platform.
- [Tool receipts](./tool-receipts.md): HMAC evidence for successful tool results, passed back in-band with the conversation.

## Archive integrity

Plugin, skill-discovery and ZIP updater extraction validate local-file headers
against the central directory before writing extracted entries. Inconsistent
filenames, defined header metadata, duplicate names or overlapping entries are
refused. Existing
path confinement, archive size and artifact-approval checks still apply; this
validation does not authorize an archive or claim arbitrary content is safe.
