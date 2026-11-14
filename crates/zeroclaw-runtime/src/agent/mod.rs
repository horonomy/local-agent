#[allow(clippy::module_inception)]
pub mod agent;
pub(crate) mod approval_bridge;
pub mod classifier;
pub mod context_analyzer;
pub mod cost;
pub mod dispatcher;
pub mod eval;
pub mod execution_tree_budget;
pub mod history;
pub mod history_pruner;
pub mod history_trim;
pub mod loop_;
pub mod loop_detector;
pub mod memory_inject;
pub mod memory_strategy;
pub mod personality;
pub mod personality_templates;
pub mod pricing_catalog;
pub mod prompt;
pub mod system_prompt;
pub mod thinking;
pub(crate) mod tool_call_format;
pub mod tool_execution;
pub mod tool_receipts;
pub(crate) mod turn;

pub use turn::context::TurnMeta;
pub use turn::{
    ContextWindowExceeded, append_safeguard_fallback_notice, context_window_exceeded_from_error,
    is_semantic_empty_terminal_completion,
    media_degrade::{
        degrade_media_in_message, degrade_media_in_messages, is_turn_opening_user_message,
    },
    redact::{is_credential_key, scrub_credentials_value},
    semantic_empty_terminal_completion_message, terminal_completion_error_message,
};
pub use turn::{SteeringAdmission, SteeringAdmit, SteeringInput, SteeringPosture};

pub(crate) fn is_runtime_approved_arg_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "shell" | "schedule" | "cron_add" | "cron_update" | "cron_run"
    )
}

/// Nodes are surfaced as `node:<node_id>:<capability>` tools whose `execute`
/// honours an `approved` argument for sensitive capabilities.
const NODE_TOOL_PREFIX: &str = "node:";

/// True when a call to `tool_name` reaches an executor that honours the
/// model-visible `approved` argument: a built-in in the runtime-approved set,
/// a node tool, or a wrapper (elevated skill tool) delegating to either.
pub(crate) fn takes_runtime_approved_arg(
    tool_name: &str,
    tool: Option<&dyn zeroclaw_api::tool::Tool>,
) -> bool {
    fn target_takes(tool: &dyn zeroclaw_api::tool::Tool, depth: usize) -> bool {
        if is_runtime_approved_arg_tool(tool.name()) || tool.name().starts_with(NODE_TOOL_PREFIX) {
            return true;
        }
        if depth == 0 {
            // Unbounded wrapper chain: treat as sensitive.
            return true;
        }
        match tool.delegation_target() {
            Some(zeroclaw_api::tool::DelegationTarget::Tool(inner)) => {
                target_takes(inner, depth - 1)
            }
            Some(zeroclaw_api::tool::DelegationTarget::Builtin { name, .. }) => {
                is_runtime_approved_arg_tool(name)
            }
            None => false,
        }
    }
    is_runtime_approved_arg_tool(tool_name)
        || tool_name.starts_with(NODE_TOOL_PREFIX)
        || tool.is_some_and(|tool| target_takes(tool, 8))
}

/// Overwrite any model-supplied `approved` with the runtime decision for every
/// call that reaches an `approved`-honouring executor.
pub(crate) fn set_runtime_approved_arg(
    tool_name: &str,
    tool: Option<&dyn zeroclaw_api::tool::Tool>,
    args: &mut serde_json::Value,
    approved: bool,
) {
    if takes_runtime_approved_arg(tool_name, tool)
        && let Some(args) = args.as_object_mut()
    {
        args.insert("approved".to_string(), serde_json::Value::Bool(approved));
    }
}

/// Borrow-only Attributable holding an agent alias.
/// Used by entry points (loop_::run, process_message, cron dispatch)
/// that don't construct a full `Agent` but still need to open an
/// `attribution_span!` carrying the agent's role + alias.
pub struct AgentAttribution<'a>(pub &'a str);

impl ::zeroclaw_api::attribution::Attributable for AgentAttribution<'_> {
    fn role(&self) -> ::zeroclaw_api::attribution::Role {
        ::zeroclaw_api::attribution::Role::Agent
    }
    fn alias(&self) -> &str {
        self.0
    }
}

#[allow(unused_imports)]
pub use agent::{Agent, AgentBuilder, TurnEvent};
#[allow(unused_imports)]
pub use loop_::{process_message, process_message_with_live_config, run};

#[cfg(test)]
mod tests;
