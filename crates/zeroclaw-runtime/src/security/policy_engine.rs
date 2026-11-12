//! Standalone policy engine (ADR-019 D3).
//!
//! Pure and side-effect free: [`StandalonePolicyEngine::evaluate`] reads only
//! the request and the engine's immutable profile, and never panics. Any input
//! it cannot classify is a `Deny`. The engine decides; it does not widen. A
//! request can only be allowed when every applicable rule admits it, and the
//! strictest verdict (`Deny > Ask > Allow`) wins.
//!
//! Not wired into `prepare_tool_calls` yet; that is the enforcement slice.
//! Decisions carry `DecisionSource::Local`, the contract's source for the
//! standalone authority.

use std::collections::{BTreeMap, BTreeSet};

use async_trait::async_trait;
use zeroclaw_api::policy::ToolCapability;
use zeroclaw_config::autonomy::AutonomyLevel;

use super::policy_contract::{
    DecisionSource, PolicyAuthority, PolicyDecision, PolicyRequest, ReasonCode, Verdict,
    reject_unsupported_version,
};

/// Wildcard entry in `allowed_tools` and `ask_tools`.
const WILDCARD: &str = "*";
/// Separator of composed MCP tool names (`<server>__<tool>`).
const COMPOSE_SEPARATOR: &str = "__";

/// Immutable policy inputs for one agent.
///
/// `Default` is deny-all (`allowed_tools = Some(empty)`), so an engine built
/// without an explicit profile fails closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyProfile {
    /// `None`: no allow-list restriction. `Some(empty)`: nothing is allowed.
    /// A `"*"` entry admits any tool except those needing an explicit listing.
    pub allowed_tools: Option<BTreeSet<String>>,
    /// Tool names, skill names or MCP server names that are always denied.
    pub excluded_tools: BTreeSet<String>,
    /// MCP servers granted through a bundle.
    pub mcp_grants: BTreeSet<String>,
    /// Tools whose use always needs a prompt; `"*"` covers every tool.
    pub ask_tools: BTreeSet<String>,
    /// Highest autonomy a request may claim under this profile.
    pub autonomy_ceiling: AutonomyLevel,
}

impl Default for PolicyProfile {
    fn default() -> Self {
        Self {
            allowed_tools: Some(BTreeSet::new()),
            excluded_tools: BTreeSet::new(),
            mcp_grants: BTreeSet::new(),
            ask_tools: BTreeSet::new(),
            autonomy_ceiling: AutonomyLevel::ReadOnly,
        }
    }
}

/// The standalone [`PolicyAuthority`].
///
/// `parents` holds the profiles of parent agents by alias. A subagent request
/// (`parent_agent_alias` set) is decided against its own profile and its
/// parent's, so it can only narrow; an unknown parent is a `Deny`.
#[derive(Debug, Clone, Default)]
pub struct StandalonePolicyEngine {
    profile: PolicyProfile,
    parents: BTreeMap<String, PolicyProfile>,
}

fn deny(reason: ReasonCode) -> PolicyDecision {
    PolicyDecision::deny(reason, DecisionSource::Local)
}

fn decision(verdict: Verdict, reason: ReasonCode) -> PolicyDecision {
    PolicyDecision::new(verdict, reason, DecisionSource::Local)
}

impl PolicyProfile {
    fn lists(&self, name: &str) -> bool {
        self.allowed_tools
            .as_ref()
            .is_some_and(|list| list.contains(name))
    }

    fn admits_by_list(&self, name: &str) -> bool {
        match &self.allowed_tools {
            None => true,
            Some(list) => list.contains(name) || list.contains(WILDCARD),
        }
    }

    fn is_excluded(&self, request: &PolicyRequest) -> bool {
        let hit = |n: &str| self.excluded_tools.contains(n);
        hit(&request.tool_name)
            || request.via_skill.as_deref().is_some_and(hit)
            || request.mcp_server.as_deref().is_some_and(hit)
    }

    fn evaluate(&self, request: &PolicyRequest) -> PolicyDecision {
        if request.tool_name.is_empty() {
            return deny(ReasonCode::LocalDeny);
        }
        if self.is_excluded(request) {
            return deny(ReasonCode::Excluded);
        }
        if request.autonomy > self.autonomy_ceiling {
            return deny(ReasonCode::LocalDeny);
        }
        if request.mcp_server.is_some() && request.capability != ToolCapability::Mcp {
            return deny(ReasonCode::LocalDeny);
        }

        match request.capability {
            ToolCapability::Unknown | ToolCapability::Plugin => {
                return deny(ReasonCode::UnknownCapability);
            }
            ToolCapability::Network | ToolCapability::Browser | ToolCapability::Desktop => {
                if !self.lists(&request.tool_name) {
                    return deny(ReasonCode::NetworkNotGranted);
                }
            }
            ToolCapability::Mcp => {
                if let Some(denied) = self.mcp_denial(request) {
                    return denied;
                }
            }
            ToolCapability::FsWrite | ToolCapability::Shell
                if request.autonomy == AutonomyLevel::ReadOnly =>
            {
                return deny(ReasonCode::LocalDeny);
            }
            ToolCapability::FsRead
            | ToolCapability::FsWrite
            | ToolCapability::Shell
            | ToolCapability::Memory
            | ToolCapability::Subagent
            | ToolCapability::AgentInternal => {
                if !self.admits_by_list(&request.tool_name) {
                    return deny(ReasonCode::LocalDeny);
                }
            }
            // `ToolCapability` is non_exhaustive: a class this engine does not
            // know is unclassified.
            _ => return deny(ReasonCode::UnknownCapability),
        }

        if self.ask_tools.contains(WILDCARD) || self.ask_tools.contains(&request.tool_name) {
            return decision(Verdict::Ask, ReasonCode::LocalAllow);
        }
        decision(Verdict::Allow, ReasonCode::LocalAllow)
    }

    /// MCP needs a well-formed, unambiguous composed name, a bundle grant and,
    /// under an allow-list, an explicit listing of the full composed name.
    fn mcp_denial(&self, request: &PolicyRequest) -> Option<PolicyDecision> {
        let Some(server) = request.mcp_server.as_deref() else {
            return Some(deny(ReasonCode::LocalDeny));
        };
        let well_formed = !server.is_empty() && !server.contains(COMPOSE_SEPARATOR);
        let tool = request
            .tool_name
            .strip_prefix(server)
            .and_then(|rest| rest.strip_prefix(COMPOSE_SEPARATOR));
        // A tool part that itself contains `__` could also parse as a longer
        // server name: ambiguous, so rejected.
        let unambiguous = tool.is_some_and(|t| !t.is_empty() && !t.contains(COMPOSE_SEPARATOR));
        if !well_formed || !unambiguous || !self.mcp_grants.contains(server) {
            return Some(deny(ReasonCode::LocalDeny));
        }
        if self.allowed_tools.is_some() && !self.lists(&request.tool_name) {
            return Some(deny(ReasonCode::LocalDeny));
        }
        None
    }
}

impl StandalonePolicyEngine {
    pub fn new(profile: PolicyProfile) -> Self {
        Self {
            profile,
            parents: BTreeMap::new(),
        }
    }

    /// Register a parent agent's profile so its subagents are narrowed by it.
    pub fn with_parent(mut self, alias: impl Into<String>, profile: PolicyProfile) -> Self {
        self.parents.insert(alias.into(), profile);
        self
    }

    /// Decide a request. Total and side-effect free.
    pub fn evaluate(&self, request: &PolicyRequest) -> PolicyDecision {
        if let Some(rejected) = reject_unsupported_version(request) {
            return rejected;
        }
        let own = self.profile.evaluate(request);
        let Some(parent_alias) = request.parent_agent_alias.as_deref() else {
            return own;
        };
        let Some(parent) = self.parents.get(parent_alias) else {
            return deny(ReasonCode::LocalDeny);
        };
        let inherited = parent.evaluate(request);
        if inherited.verdict > own.verdict {
            inherited
        } else {
            own
        }
    }
}

#[async_trait]
impl PolicyAuthority for StandalonePolicyEngine {
    async fn decide(&self, request: &PolicyRequest) -> PolicyDecision {
        self.evaluate(request)
    }
}
