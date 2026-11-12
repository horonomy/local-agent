# Security and Privacy

Everything here is stated by [Minimal runtime profile](../setup/local-agent-minimal.md); read its [limits](../setup/local-agent-minimal.md#limits-of-the-claim) before relying on it.

## No inbound surface

- The gateway, ACP bridge, daemon listeners, messaging channels, relay, embedded web UI and tunnels are not compiled in. Combining the profile with one of them fails at compile time.
- The commands that would start them (`daemon`, `relay`, `gateway`, `acp`, `channel start` and the `service` commands that start or install a unit) are refused at startup, before config is loaded or any state is written.
- A config that enables `[wss]`, `[relay]`, `[enroll]`, `[a2a.server]` or a tunnel provider is refused instead of silently ignored.

## Where data can leave the machine

- Inference requests go only to a provider you configured and referenced from the agent. `--model-provider` must name a configured `<family>.<alias>`; a bare family or a URL is refused.
- The minimal dependency closure contains no Prometheus exporter. That is the only metrics or telemetry crate the check looks for.
- Not covered by the evidence: model catalog lookups, `auth login`, `auth paste-redirect` and `auth refresh`, update checks, the tool sandbox's own DNS allowance, LLM-invoked network tools such as `web_fetch` (approval-gated), the `model_switch` catalog fallback and the open-skills environment override.
- Provider OAuth sign-in uses a short-lived, user-initiated loopback callback listener that is not gated.

## What the agent may do

Tools run under the agent's autonomy level and risk profile, `supervised` by default. The inherited pages below describe the standard build; their channel, gateway and approval-routing sections do not apply.

- [Security overview](../security/overview.md)
- [The security model](../security/model.md)
- [Autonomy levels](../security/autonomy.md)
- [Sandboxing](../security/sandboxing.md)
- [Tool receipts](../security/tool-receipts.md)

Controls specific to MCP, skills and plugins: [MCP tool scopes](./mcp-stdio-setup.md#tool-scopes), [skill trust boundaries](./skills-trust.md#trust-boundaries) and [plugin security controls](./wasm-plugins.md#security-controls).
