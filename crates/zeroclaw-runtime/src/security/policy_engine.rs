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

/// Names must already be canonical: the engine does no case or whitespace
/// folding, so anything that could be folded elsewhere is refused.
fn is_canonical(name: &str) -> bool {
    !name.is_empty()
        && name.trim() == name
        && name.is_ascii()
        && !name
            .bytes()
            .any(|b| b.is_ascii_uppercase() || b.is_ascii_control())
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
        // Excluding a server name excludes its composed tools, even when the
        // request does not carry `mcp_server`.
        let composed_prefix = request
            .tool_name
            .split_once(COMPOSE_SEPARATOR)
            .map(|(prefix, _)| prefix);
        hit(&request.tool_name)
            || composed_prefix.is_some_and(hit)
            || request.via_skill.as_deref().is_some_and(hit)
            || request.mcp_server.as_deref().is_some_and(hit)
    }

    fn evaluate(&self, request: &PolicyRequest) -> PolicyDecision {
        let names = [
            Some(request.tool_name.as_str()),
            request.via_skill.as_deref(),
            request.mcp_server.as_deref(),
        ];
        if names.into_iter().flatten().any(|n| !is_canonical(n)) {
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
        // A composed name is only meaningful for MCP; elsewhere it could
        // smuggle a server-shaped name past a listing.
        if request.capability != ToolCapability::Mcp
            && request.tool_name.contains(COMPOSE_SEPARATOR)
        {
            return deny(ReasonCode::LocalDeny);
        }
        // Read-only sessions keep only observation and agent-internal tools.
        if request.autonomy == AutonomyLevel::ReadOnly
            && !matches!(
                request.capability,
                ToolCapability::FsRead | ToolCapability::Memory | ToolCapability::AgentInternal
            )
            && !matches!(
                request.capability,
                ToolCapability::Unknown | ToolCapability::Plugin
            )
        {
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
        // Underscore edges would let `a_` + `__t` collide with `a` + `___t`.
        let well_formed = !server.is_empty()
            && !server.contains(COMPOSE_SEPARATOR)
            && !server.starts_with('_')
            && !server.ends_with('_');
        let tool = request
            .tool_name
            .strip_prefix(server)
            .and_then(|rest| rest.strip_prefix(COMPOSE_SEPARATOR));
        // A tool part that itself contains `__` could also parse as a longer
        // server name: ambiguous, so rejected.
        let unambiguous = tool.is_some_and(|t| {
            !t.is_empty() && !t.contains(COMPOSE_SEPARATOR) && !t.starts_with('_')
        });
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::policy_contract::{CONTRACT_VERSION, args_sha256, strictest};

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    fn profile(allowed: Option<&[&str]>) -> PolicyProfile {
        PolicyProfile {
            allowed_tools: allowed.map(set),
            excluded_tools: BTreeSet::new(),
            mcp_grants: BTreeSet::new(),
            ask_tools: BTreeSet::new(),
            autonomy_ceiling: AutonomyLevel::Full,
        }
    }

    fn req(capability: ToolCapability, tool: &str) -> PolicyRequest {
        PolicyRequest {
            contract_version: CONTRACT_VERSION,
            session_id: "s".into(),
            turn_id: "t".into(),
            call_id: "c".into(),
            agent_alias: "main".into(),
            parent_agent_alias: None,
            capability,
            tool_name: tool.into(),
            via_skill: None,
            mcp_server: None,
            args_sha256: args_sha256(&serde_json::json!({})),
            autonomy: AutonomyLevel::Supervised,
            interactive: true,
        }
    }

    fn mcp_req(server: &str, composed: &str) -> PolicyRequest {
        PolicyRequest {
            mcp_server: Some(server.into()),
            ..req(ToolCapability::Mcp, composed)
        }
    }

    fn eval(p: &PolicyProfile, r: &PolicyRequest) -> PolicyDecision {
        StandalonePolicyEngine::new(p.clone()).evaluate(r)
    }

    #[track_caller]
    fn assert_deny(d: PolicyDecision, reason: ReasonCode) {
        assert_eq!(d.verdict, Verdict::Deny, "{d:?}");
        assert_eq!(d.reason, reason);
        assert_eq!(d.source, DecisionSource::Local);
    }

    #[test]
    fn unknown_and_plugin_are_denied_even_under_full_and_wildcard() {
        let mut p = profile(Some(&["*", "x"]));
        p.autonomy_ceiling = AutonomyLevel::Full;
        for cap in [ToolCapability::Unknown, ToolCapability::Plugin] {
            let mut r = req(cap, "x");
            r.autonomy = AutonomyLevel::Full;
            assert_deny(eval(&p, &r), ReasonCode::UnknownCapability);
            assert_deny(eval(&profile(None), &r), ReasonCode::UnknownCapability);
        }
    }

    #[test]
    fn exclusion_wins_over_every_grant() {
        let mut p = profile(Some(&["*", "shell", "net"]));
        p.excluded_tools = set(&["shell", "net", "gh", "sk"]);
        p.mcp_grants = set(&["gh"]);
        assert_deny(
            eval(&p, &req(ToolCapability::Shell, "shell")),
            ReasonCode::Excluded,
        );
        assert_deny(
            eval(&p, &req(ToolCapability::Network, "net")),
            ReasonCode::Excluded,
        );
        assert_deny(eval(&p, &mcp_req("gh", "gh__search")), ReasonCode::Excluded);
        let mut skill = req(ToolCapability::Shell, "other");
        skill.via_skill = Some("sk".into());
        assert_deny(eval(&p, &skill), ReasonCode::Excluded);
        // Not excluded: unrelated tool is still admitted by the wildcard.
        assert_eq!(
            eval(&p, &req(ToolCapability::FsRead, "file_read")).verdict,
            Verdict::Allow
        );
    }

    #[test]
    fn network_class_needs_explicit_listing_not_wildcard() {
        for cap in [
            ToolCapability::Network,
            ToolCapability::Browser,
            ToolCapability::Desktop,
        ] {
            for p in [
                profile(None),
                profile(Some(&["*"])),
                profile(Some(&["other"])),
            ] {
                assert_deny(
                    eval(&p, &req(cap, "web_fetch")),
                    ReasonCode::NetworkNotGranted,
                );
            }
            assert_eq!(
                eval(&profile(Some(&["web_fetch"])), &req(cap, "web_fetch")).verdict,
                Verdict::Allow
            );
        }
    }

    #[test]
    fn skill_wrapper_is_evaluated_as_its_target() {
        // Wrapper resolved to a network target that is not listed.
        let mut r = req(ToolCapability::Network, "web_fetch");
        r.via_skill = Some("helper".into());
        assert_deny(
            eval(&profile(Some(&["helper__fetch", "*"])), &r),
            ReasonCode::NetworkNotGranted,
        );
        // Listing the target admits it.
        assert_eq!(
            eval(&profile(Some(&["web_fetch"])), &r).verdict,
            Verdict::Allow
        );
        // A shell target under an allow-list lacking shell is denied.
        let mut s = req(ToolCapability::Shell, "shell");
        s.via_skill = Some("helper".into());
        assert_deny(
            eval(&profile(Some(&["helper__run"])), &s),
            ReasonCode::LocalDeny,
        );
    }

    #[test]
    fn mcp_needs_grant_and_explicit_listing() {
        let r = mcp_req("gh", "gh__search");
        let mut p = profile(None);
        assert_deny(eval(&p, &r), ReasonCode::LocalDeny); // no bundle grant
        p.mcp_grants = set(&["gh"]);
        assert_eq!(eval(&p, &r).verdict, Verdict::Allow); // unrestricted list
        p.allowed_tools = Some(set(&["*"]));
        assert_deny(eval(&p, &r), ReasonCode::LocalDeny); // wildcard is not listing
        p.allowed_tools = Some(set(&["gh__other"]));
        assert_deny(eval(&p, &r), ReasonCode::LocalDeny);
        p.allowed_tools = Some(set(&["gh__search"]));
        assert_eq!(eval(&p, &r).verdict, Verdict::Allow);
        p.allowed_tools = Some(BTreeSet::new());
        assert_deny(eval(&p, &r), ReasonCode::LocalDeny);
    }

    #[test]
    fn composed_names_are_not_auto_admitted_or_ambiguous() {
        // A `__` name is not admitted by the allow-list just for containing it.
        let p = profile(Some(&["file_read"]));
        assert_deny(
            eval(&p, &req(ToolCapability::Memory, "github__search")),
            ReasonCode::LocalDeny,
        );
        // MCP name must be exactly `<server>__<tool>`.
        let mut q = profile(None);
        q.mcp_grants = set(&["gh", "gh__x", "a"]);
        for (server, name) in [
            ("gh", "other__search"), // prefix mismatch
            ("gh", "gh__"),          // empty tool part
            ("gh", "gh_search"),     // no separator
            ("gh", "gh__a__b"),      // ambiguous: could be server gh__a
            ("gh__x", "gh__x__y"),   // server name contains separator
            ("", "__t"),             // empty server
            ("a", "ab__t"),          // server is only a string prefix
        ] {
            assert_deny(eval(&q, &mcp_req(server, name)), ReasonCode::LocalDeny);
        }
        // Server set on a non-MCP capability is inconsistent.
        let mut bad = req(ToolCapability::FsRead, "file_read");
        bad.mcp_server = Some("gh".into());
        assert_deny(eval(&profile(None), &bad), ReasonCode::LocalDeny);
        // MCP capability without a server is denied.
        assert_deny(
            eval(&q, &req(ToolCapability::Mcp, "gh__search")),
            ReasonCode::LocalDeny,
        );
    }

    #[test]
    fn allow_list_semantics() {
        let r = req(ToolCapability::FsRead, "file_read");
        assert_eq!(eval(&profile(None), &r).verdict, Verdict::Allow);
        assert_eq!(
            eval(&profile(Some(&["file_read"])), &r).verdict,
            Verdict::Allow
        );
        assert_eq!(eval(&profile(Some(&["*"])), &r).verdict, Verdict::Allow);
        assert_deny(eval(&profile(Some(&[])), &r), ReasonCode::LocalDeny);
        assert_deny(eval(&profile(Some(&["x"])), &r), ReasonCode::LocalDeny);
        assert_deny(
            eval(&profile(None), &req(ToolCapability::FsRead, "")),
            ReasonCode::LocalDeny,
        );
        assert_deny(eval(&PolicyProfile::default(), &r), ReasonCode::LocalDeny);
    }

    #[test]
    fn autonomy_above_ceiling_and_read_only_mutation_are_denied() {
        let mut p = profile(None);
        p.autonomy_ceiling = AutonomyLevel::Supervised;
        let mut full = req(ToolCapability::FsRead, "file_read");
        full.autonomy = AutonomyLevel::Full;
        assert_deny(eval(&p, &full), ReasonCode::LocalDeny);

        for cap in [ToolCapability::Shell, ToolCapability::FsWrite] {
            let mut ro = req(cap, "t");
            ro.autonomy = AutonomyLevel::ReadOnly;
            assert_deny(eval(&profile(None), &ro), ReasonCode::LocalDeny);
        }
        let mut ro_read = req(ToolCapability::FsRead, "file_read");
        ro_read.autonomy = AutonomyLevel::ReadOnly;
        assert_eq!(eval(&profile(None), &ro_read).verdict, Verdict::Allow);
    }

    #[test]
    fn engine_ask_forces_prompt_even_under_full() {
        let mut p = profile(None);
        p.ask_tools = set(&["shell"]);
        let mut r = req(ToolCapability::Shell, "shell");
        r.autonomy = AutonomyLevel::Full;
        assert_eq!(eval(&p, &r).verdict, Verdict::Ask);
        assert_eq!(
            eval(&p, &req(ToolCapability::FsRead, "file_read")).verdict,
            Verdict::Allow
        );
        p.ask_tools = set(&["*"]);
        assert_eq!(eval(&p, &r).verdict, Verdict::Ask);
        // Ask never rescues a denial.
        p.excluded_tools = set(&["shell"]);
        assert_deny(eval(&p, &r), ReasonCode::Excluded);
    }

    #[test]
    fn subagent_can_only_narrow() {
        let parent = {
            let mut p = profile(Some(&["file_read"]));
            p.autonomy_ceiling = AutonomyLevel::Supervised;
            p
        };
        // Child profile is wider than the parent.
        let child = profile(Some(&["file_read", "shell"]));
        let engine = StandalonePolicyEngine::new(child).with_parent("main", parent);
        let mut sub = req(ToolCapability::FsRead, "file_read");
        sub.agent_alias = "sub".into();
        sub.parent_agent_alias = Some("main".into());
        assert_eq!(engine.evaluate(&sub).verdict, Verdict::Allow);

        let mut shell = req(ToolCapability::Shell, "shell");
        shell.parent_agent_alias = Some("main".into());
        assert_deny(engine.evaluate(&shell), ReasonCode::LocalDeny);

        let mut escalate = sub.clone();
        escalate.autonomy = AutonomyLevel::Full;
        assert_deny(engine.evaluate(&escalate), ReasonCode::LocalDeny);

        let mut orphan = sub.clone();
        orphan.parent_agent_alias = Some("ghost".into());
        assert_deny(engine.evaluate(&orphan), ReasonCode::LocalDeny);

        // Parent Ask carries to the child.
        let mut ask_parent = profile(None);
        ask_parent.ask_tools = set(&["file_read"]);
        let engine = StandalonePolicyEngine::new(profile(None)).with_parent("main", ask_parent);
        assert_eq!(engine.evaluate(&sub).verdict, Verdict::Ask);
    }

    #[test]
    fn unsupported_contract_version_is_denied() {
        for v in [0, CONTRACT_VERSION + 1] {
            let mut r = req(ToolCapability::FsRead, "file_read");
            r.contract_version = v;
            assert_deny(
                eval(&profile(None), &r),
                ReasonCode::ContractVersionMismatch,
            );
        }
    }

    #[tokio::test]
    async fn trait_decide_matches_evaluate() {
        let engine = StandalonePolicyEngine::new(profile(None));
        let r = req(ToolCapability::FsRead, "file_read");
        assert_eq!(engine.decide(&r).await, engine.evaluate(&r));
    }

    const CAPS: [ToolCapability; 12] = [
        ToolCapability::FsRead,
        ToolCapability::FsWrite,
        ToolCapability::Shell,
        ToolCapability::Network,
        ToolCapability::Memory,
        ToolCapability::Mcp,
        ToolCapability::Subagent,
        ToolCapability::Browser,
        ToolCapability::Desktop,
        ToolCapability::Plugin,
        ToolCapability::AgentInternal,
        ToolCapability::Unknown,
    ];
    const AUTONOMY: [AutonomyLevel; 3] = [
        AutonomyLevel::ReadOnly,
        AutonomyLevel::Supervised,
        AutonomyLevel::Full,
    ];

    fn grid() -> Vec<PolicyRequest> {
        let mut out = Vec::new();
        for cap in CAPS {
            for tool in ["t", "gh__search", "web_fetch", ""] {
                for autonomy in AUTONOMY {
                    for server in [None, Some("gh")] {
                        for skill in [None, Some("sk")] {
                            let mut r = req(cap, tool);
                            r.autonomy = autonomy;
                            r.mcp_server = server.map(Into::into);
                            r.via_skill = skill.map(Into::into);
                            out.push(r);
                        }
                    }
                }
            }
        }
        out
    }

    fn profiles() -> Vec<PolicyProfile> {
        let mut out = Vec::new();
        for allowed in [
            None,
            Some(vec![]),
            Some(vec!["*"]),
            Some(vec!["t", "web_fetch", "gh__search"]),
        ] {
            for excluded in [vec![], vec!["t"], vec!["gh"]] {
                for grants in [vec![], vec!["gh"]] {
                    for ask in [vec![], vec!["t"], vec!["*"]] {
                        for ceiling in AUTONOMY {
                            out.push(PolicyProfile {
                                allowed_tools: allowed.as_deref().map(set),
                                excluded_tools: set(&excluded),
                                mcp_grants: set(&grants),
                                ask_tools: set(&ask),
                                autonomy_ceiling: ceiling,
                            });
                        }
                    }
                }
            }
        }
        out
    }

    /// Every way of making a profile stricter.
    fn narrowings(p: &PolicyProfile) -> Vec<PolicyProfile> {
        let mut out = Vec::new();
        for name in ["t", "gh", "web_fetch", "gh__search"] {
            let mut q = p.clone();
            q.excluded_tools.insert(name.into());
            out.push(q);
            let mut q = p.clone();
            q.ask_tools.insert(name.into());
            out.push(q);
            if let Some(list) = &p.allowed_tools {
                let mut q = p.clone();
                q.allowed_tools = Some(list.iter().filter(|n| *n != name).cloned().collect());
                out.push(q);
            }
        }
        let mut q = p.clone();
        q.allowed_tools = Some(BTreeSet::new());
        out.push(q);
        let mut q = p.clone();
        q.mcp_grants.clear();
        out.push(q);
        let mut q = p.clone();
        q.ask_tools.insert("*".into());
        out.push(q);
        for lower in AUTONOMY {
            if lower < p.autonomy_ceiling {
                let mut q = p.clone();
                q.autonomy_ceiling = lower;
                out.push(q);
            }
        }
        out
    }

    #[test]
    fn lattice_narrowing_a_profile_never_weakens_a_decision() {
        let requests = grid();
        for p in profiles() {
            for q in narrowings(&p) {
                for r in &requests {
                    let before = eval(&p, r);
                    let after = eval(&q, r);
                    assert!(
                        after.verdict >= before.verdict,
                        "narrowed profile weakened {before:?} -> {after:?} for {r:?}\n{p:?}\n{q:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn lattice_composition_never_weakens_a_prior_deny() {
        let requests = grid();
        let engine = StandalonePolicyEngine::new(profile(Some(&["*"])));
        for r in &requests {
            let local = engine.evaluate(r);
            let prior_deny = PolicyDecision::deny(ReasonCode::AasmDeny, DecisionSource::Aasm);
            assert_eq!(
                strictest(prior_deny.clone(), local.clone()).verdict,
                Verdict::Deny
            );
            assert_eq!(strictest(local, prior_deny).verdict, Verdict::Deny);
        }
    }

    #[test]
    fn total_over_the_whole_grid_and_every_deny_has_local_source() {
        let requests = grid();
        for p in profiles() {
            let engine = StandalonePolicyEngine::new(p);
            for r in &requests {
                let d = engine.evaluate(r);
                assert_eq!(d.source, DecisionSource::Local);
                if d.verdict == Verdict::Deny {
                    assert_ne!(d.reason, ReasonCode::LocalAllow, "{r:?}");
                }
            }
        }
    }
}
