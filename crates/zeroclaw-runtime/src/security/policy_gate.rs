//! Authoritative tool-call gate (ADR-019 D2).
//!
//! [`PolicyGate`] sits between the tool name the model emitted and the tool
//! that runs. `prepare_tool_calls` asks it to [`decide`](PolicyGate::decide)
//! once per call, after hooks have rewritten the call and before the approval
//! prompt. An `Allow` or `Ask` mints a [`DecisionTicket`]; the executor
//! [`redeem`](PolicyGate::redeem)s it for exactly the resolved tool and
//! arguments it is about to run, so nothing between the decision and the
//! execution can swap either. A tool without a redeemable ticket does not run.
//!
//! The gate lives inside [`ScopedToolRegistry`](crate::tools::scoped), which
//! only `assemble` can mint, so no turn loop can exist without one.
//!
//! # Name rule
//!
//! The standalone engine accepts only canonical names. Every name that reaches
//! it, and every entry of the profile lists it is compared against, goes
//! through [`fold_name`]: trim, then ASCII-lowercase. A name that is empty or
//! contains a non-ASCII or control character cannot be folded and is denied
//! (list entries that cannot fold are dropped, which can only narrow). Two
//! registered tools whose names differ but fold to the same string are
//! ambiguous and denied. Tickets bind the exact registry name, never the
//! folded one.
//!
//! # Wrappers
//!
//! A tool that reports a [`DelegationTarget`] is evaluated as that target:
//! `tool_name` and capability come from the target and the wrapper's own
//! name travels as `via_skill`. A skill shell tool therefore evaluates as
//! `shell` and a skill HTTP tool as `http_request`, so excluding the target
//! excludes the wrapper. Delegation deeper than [`MAX_DELEGATION_DEPTH`] is
//! denied.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;
use zeroclaw_api::policy::ToolCapability;
use zeroclaw_api::tool::{DelegationTarget, Tool};
use zeroclaw_config::autonomy::AutonomyLevel;
use zeroclaw_config::governance::{GovernanceConfig, GovernanceMode};
use zeroclaw_config::schema::Config;

use super::policy_contract::{
    CONTRACT_VERSION, PolicyAuthority, PolicyRequest, ReasonCode, Verdict, args_sha256,
};
use super::policy_engine::{PolicyProfile, StandalonePolicyEngine};

/// Wrapper chains longer than this are denied.
pub const MAX_DELEGATION_DEPTH: usize = 2;
/// Upper bound on live tickets; the oldest are dropped past it.
const MAX_LIVE_TICKETS: usize = 4096;
const WILDCARD: &str = "*";

/// Fold a tool, skill or MCP server name to the canonical form the engine
/// requires: trimmed ASCII lowercase. `None` when it cannot be folded.
pub fn fold_name(name: &str) -> Option<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() || !trimmed.is_ascii() || trimmed.bytes().any(|b| b.is_ascii_control()) {
        return None;
    }
    Some(trimmed.to_ascii_lowercase())
}

fn fold_set<'a>(items: impl IntoIterator<Item = &'a String>) -> BTreeSet<String> {
    items
        .into_iter()
        .filter_map(|item| {
            if item.trim() == WILDCARD {
                Some(WILDCARD.to_owned())
            } else {
                fold_name(item)
            }
        })
        .collect()
}

/// Raw policy inputs for one agent, as configured.
pub struct AgentPolicyInputs<'a> {
    pub autonomy: AutonomyLevel,
    pub allowed_tools: Option<&'a [String]>,
    pub excluded_tools: Option<&'a [String]>,
    pub always_ask: &'a [String],
    /// Servers granted through the agent's MCP bundles.
    pub mcp_servers: &'a [String],
    /// Per-run caller allow-list; it only removes names, never grants.
    pub caller_allowed: Option<&'a [String]>,
}

/// Build the engine profile from configured inputs.
pub fn profile_from_inputs(inputs: &AgentPolicyInputs<'_>) -> PolicyProfile {
    // The caller list is deliberately not merged in: it may only remove
    // names (see `Enforcing::caller_allowed`), never become a source of the
    // explicit grants the engine requires.
    let allowed_tools = inputs.allowed_tools.map(|list| fold_set(list));
    PolicyProfile {
        allowed_tools,
        excluded_tools: fold_set(inputs.excluded_tools.unwrap_or_default()),
        mcp_grants: fold_set(inputs.mcp_servers),
        ask_tools: fold_set(inputs.always_ask),
        autonomy_ceiling: inputs.autonomy,
    }
}

/// Standalone authority with parent profiles registered. Kept here rather
/// than in `authority_factory` so the factory stays the one place HORO-2395
/// extends for managed mode.
fn standalone_authority_with_parents(
    profile: PolicyProfile,
    parents: Vec<(String, PolicyProfile)>,
) -> Arc<dyn PolicyAuthority> {
    let engine = parents.into_iter().fold(
        StandalonePolicyEngine::new(profile),
        |engine, (alias, p)| engine.with_parent(alias, p),
    );
    Arc::new(engine)
}

/// Proof that one call was decided. Redeemable once, for exactly this
/// resolved tool and these arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionTicket {
    pub call_id: String,
    pub resolved_name: String,
    pub args_sha256: String,
    pub decision_id: String,
}

#[derive(Debug)]
struct LiveTicket {
    ticket: DecisionTicket,
    /// `Allow` or `Ask`; a `Deny` never mints a ticket.
    verdict: Verdict,
    approved: bool,
}

/// A call that did not resolve to a tool at preparation time (a deferred
/// tool activated by an earlier call of the same batch). It is decided when
/// the executor resolves it.
#[derive(Debug)]
struct LateMarker {
    call_id: String,
    args_sha256: String,
}

#[derive(Default)]
struct TicketBook {
    live: Vec<LiveTicket>,
    late: Vec<LateMarker>,
}

enum Mode {
    Enforce(Box<Enforcing>),
    /// Managed mode is configured but the transport does not exist in this
    /// build: every call is denied `authority_unavailable`.
    Unavailable,
    /// Test-only: admits everything and demands no ticket.
    #[cfg(any(test, feature = "test-util"))]
    PermitAll,
}

struct Enforcing {
    authority: Arc<dyn PolicyAuthority>,
    agent_alias: String,
    autonomy: AutonomyLevel,
    /// Folded per-run caller allow-list. Checked in addition to the engine,
    /// so it can narrow but never widen.
    caller_allowed: Option<BTreeSet<String>>,
    /// Parent of a bounded child: every decision is also narrowed by this
    /// parent's profile, registered with the authority.
    bounded_parent: Option<String>,
}

/// Facts about the call being decided or redeemed.
#[derive(Clone, Copy)]
pub struct CallFacts<'a> {
    pub tool: &'a dyn Tool,
    /// The registry the tool resolved from, for the name-collision check.
    pub registry: &'a [Box<dyn Tool>],
    pub args: &'a serde_json::Value,
    pub call_id: &'a str,
    pub turn_id: &'a str,
    pub agent_alias: Option<&'a str>,
    pub parent_agent_alias: Option<&'a str>,
    /// An operator can answer an approval prompt.
    pub interactive: bool,
}

/// The gate's verdict for one call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateOutcome {
    /// Run it; `ask` means the approval prompt is mandatory.
    Proceed {
        ask: bool,
    },
    Deny {
        reason: ReasonCode,
    },
}

/// Why a ticket could not be redeemed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedeemError {
    /// No ticket for exactly this call, or an Ask that was never approved.
    NoTicket,
    /// A deferred call was decided at execution and did not come out Allow.
    Denied(ReasonCode),
}

pub struct PolicyGate {
    mode: Mode,
    tickets: Mutex<TicketBook>,
    next_decision: AtomicU64,
}

impl PolicyGate {
    const fn with_mode(mode: Mode) -> Self {
        Self {
            mode,
            tickets: Mutex::new(TicketBook {
                live: Vec::new(),
                late: Vec::new(),
            }),
            next_decision: AtomicU64::new(1),
        }
    }

    /// Gate for one agent, from its configured policy. `governance` selects
    /// the authority: standalone builds the local engine; managed has no
    /// transport yet and fails closed with no fallback to standalone.
    pub fn for_agent(config: &Config, agent_alias: &str, inputs: &AgentPolicyInputs<'_>) -> Self {
        Self::for_governance(&config.governance, agent_alias, inputs)
    }

    fn for_governance(
        governance: &GovernanceConfig,
        agent_alias: &str,
        inputs: &AgentPolicyInputs<'_>,
    ) -> Self {
        if governance.mode == GovernanceMode::Managed {
            return Self::with_mode(Mode::Unavailable);
        }
        // Parents are not registered here: an independent delegate or a
        // cross-agent SOP step is a separate authority that runs under its own
        // risk profile (its own registry, its own gate). `parent_agent_alias`
        // on the turn is correlation, not a narrowing signal.
        let own = profile_from_inputs(inputs);
        Self::standalone(agent_alias, inputs.autonomy, own, Vec::new())
            .with_caller_filter(inputs.caller_allowed)
    }

    /// Standalone gate over explicit profiles.
    pub fn standalone(
        agent_alias: &str,
        autonomy: AutonomyLevel,
        profile: PolicyProfile,
        parents: Vec<(String, PolicyProfile)>,
    ) -> Self {
        Self::with_mode(Mode::Enforce(Box::new(Enforcing {
            authority: standalone_authority_with_parents(profile, parents),
            agent_alias: agent_alias.to_owned(),
            autonomy,
            caller_allowed: None,
            bounded_parent: None,
        })))
    }

    /// Gate for a bounded delegate: a child that runs the parent's tools under
    /// a target profile. Each call is decided against the child's profile and
    /// the parent's, so the strictest wins and the child cannot escalate. Its
    /// autonomy may not exceed the parent's.
    pub fn bounded_child(
        governance: &GovernanceConfig,
        child_alias: &str,
        child: &AgentPolicyInputs<'_>,
        parent_alias: &str,
        parent: &AgentPolicyInputs<'_>,
    ) -> Self {
        if governance.mode == GovernanceMode::Managed {
            return Self::with_mode(Mode::Unavailable);
        }
        let mut gate = Self::standalone(
            child_alias,
            child.autonomy,
            profile_from_inputs(child),
            vec![(parent_alias.to_owned(), profile_from_inputs(parent))],
        )
        .with_caller_filter(child.caller_allowed);
        if let Mode::Enforce(enforcing) = &mut gate.mode {
            enforcing.bounded_parent = Some(parent_alias.to_owned());
        }
        gate
    }

    /// Restrict the gate to a per-run caller list. It can only remove names.
    pub fn with_caller_filter(mut self, caller: Option<&[String]>) -> Self {
        if let (Mode::Enforce(enforcing), Some(list)) = (&mut self.mode, caller) {
            enforcing.caller_allowed = Some(fold_set(list));
        }
        self
    }

    /// Test-only gate that admits everything. It exists so the many fixtures
    /// that build a registry from raw tools keep working; production code
    /// can only obtain a gate from configuration.
    #[cfg(any(test, feature = "test-util"))]
    pub const fn permit_all_for_test() -> Self {
        Self::with_mode(Mode::PermitAll)
    }

    /// Decide one call. Mints a ticket for `Allow` and `Ask`.
    pub async fn decide(&self, facts: &CallFacts<'_>) -> GateOutcome {
        let enforcing = match &self.mode {
            Mode::Enforce(enforcing) => enforcing,
            Mode::Unavailable => {
                return GateOutcome::Deny {
                    reason: ReasonCode::AuthorityUnavailable,
                };
            }
            #[cfg(any(test, feature = "test-util"))]
            Mode::PermitAll => return GateOutcome::Proceed { ask: false },
        };
        let request = match build_request(enforcing, facts) {
            Ok(request) => request,
            Err(reason) => return GateOutcome::Deny { reason },
        };
        let decision = enforcing.authority.decide(&request).await;
        match decision.verdict {
            Verdict::Deny => GateOutcome::Deny {
                reason: decision.reason,
            },
            verdict @ (Verdict::Allow | Verdict::Ask) => {
                self.mint(facts, &request.args_sha256, verdict);
                GateOutcome::Proceed {
                    ask: verdict == Verdict::Ask,
                }
            }
        }
    }

    /// Record a call that does not resolve yet; the executor decides it.
    pub fn note_unresolved(&self, call_id: &str, args: &serde_json::Value) {
        if !matches!(self.mode, Mode::Enforce(_)) {
            return;
        }
        let mut book = self.tickets.lock();
        book.late.push(LateMarker {
            call_id: call_id.to_owned(),
            args_sha256: args_sha256(args),
        });
        let overflow = book.late.len().saturating_sub(MAX_LIVE_TICKETS);
        book.late.drain(..overflow);
    }

    fn requires_ticket(&self) -> bool {
        match &self.mode {
            Mode::Enforce(_) | Mode::Unavailable => true,
            #[cfg(any(test, feature = "test-util"))]
            Mode::PermitAll => false,
        }
    }

    fn mint(&self, facts: &CallFacts<'_>, digest: &str, verdict: Verdict) {
        let decision_id = self
            .next_decision
            .fetch_add(1, Ordering::Relaxed)
            .to_string();
        let mut book = self.tickets.lock();
        book.live.push(LiveTicket {
            ticket: DecisionTicket {
                call_id: facts.call_id.to_owned(),
                resolved_name: facts.tool.name().to_owned(),
                args_sha256: digest.to_owned(),
                decision_id,
            },
            verdict,
            approved: verdict == Verdict::Allow,
        });
        let overflow = book.live.len().saturating_sub(MAX_LIVE_TICKETS);
        book.live.drain(..overflow);
    }

    /// An operator approved this Ask call; its ticket becomes redeemable.
    pub fn approve(&self, call_id: &str, resolved_name: &str, args: &serde_json::Value) {
        let digest = args_sha256(args);
        let mut book = self.tickets.lock();
        if let Some(live) = book.live.iter_mut().find(|t| {
            t.ticket.call_id == call_id
                && t.ticket.resolved_name == resolved_name
                && t.ticket.args_sha256 == digest
        }) {
            live.approved = true;
        }
    }

    /// Redeem the ticket for the call the executor is about to run.
    pub async fn redeem(&self, facts: &CallFacts<'_>) -> Result<(), RedeemError> {
        if !self.requires_ticket() {
            return Ok(());
        }
        let digest = args_sha256(facts.args);
        let late = {
            let mut book = self.tickets.lock();
            let exact = book.live.iter().position(|t| {
                t.ticket.call_id == facts.call_id
                    && t.ticket.resolved_name == facts.tool.name()
                    && t.ticket.args_sha256 == digest
            });
            if let Some(index) = exact {
                if book.live[index].verdict == Verdict::Ask && !book.live[index].approved {
                    return Err(RedeemError::NoTicket);
                }
                book.live.remove(index);
                return Ok(());
            }
            book.late
                .iter()
                .position(|m| m.call_id == facts.call_id && m.args_sha256 == digest)
                .map(|index| book.late.remove(index))
        };
        if late.is_none() {
            return Err(RedeemError::NoTicket);
        }
        // Deferred call: decide now. No prompt is possible at this point, so
        // only an outright Allow passes.
        let facts = CallFacts {
            interactive: false,
            ..*facts
        };
        match self.decide_without_ticket(&facts).await {
            GateOutcome::Proceed { ask: false } => Ok(()),
            GateOutcome::Proceed { ask: true } => Err(RedeemError::Denied(ReasonCode::LocalDeny)),
            GateOutcome::Deny { reason } => Err(RedeemError::Denied(reason)),
        }
    }

    async fn decide_without_ticket(&self, facts: &CallFacts<'_>) -> GateOutcome {
        let Mode::Enforce(enforcing) = &self.mode else {
            return GateOutcome::Deny {
                reason: ReasonCode::AuthorityUnavailable,
            };
        };
        match build_request(enforcing, facts) {
            Err(reason) => GateOutcome::Deny { reason },
            Ok(request) => {
                let decision = enforcing.authority.decide(&request).await;
                match decision.verdict {
                    Verdict::Deny => GateOutcome::Deny {
                        reason: decision.reason,
                    },
                    verdict => GateOutcome::Proceed {
                        ask: verdict == Verdict::Ask,
                    },
                }
            }
        }
    }
}

/// What a call evaluates as after wrapper resolution.
struct Identity {
    tool_name: String,
    capability: ToolCapability,
    mcp_server: Option<String>,
    via_skill: Option<String>,
}

fn fold_or_deny(name: &str) -> Result<String, ReasonCode> {
    fold_name(name).ok_or(ReasonCode::LocalDeny)
}

/// Two registered tools that fold to the same name are ambiguous.
fn collides(registry: &[Box<dyn Tool>], exact: &str, folded: &str) -> bool {
    registry
        .iter()
        .any(|t| t.name() != exact && fold_name(t.name()).as_deref() == Some(folded))
}

fn resolve_identity(tool: &dyn Tool, registry: &[Box<dyn Tool>]) -> Result<Identity, ReasonCode> {
    let mut via_skill: Option<String> = None;
    let mut current = tool;
    for _ in 0..=MAX_DELEGATION_DEPTH {
        let Some(target) = current.delegation_target() else {
            let name = fold_or_deny(current.name())?;
            if collides(registry, current.name(), &name) {
                return Err(ReasonCode::LocalDeny);
            }
            return Ok(Identity {
                tool_name: name,
                capability: current.capability(),
                mcp_server: current.mcp_server().map(fold_or_deny).transpose()?,
                via_skill,
            });
        };
        if via_skill.is_none() {
            let wrapper = fold_or_deny(current.name())?;
            if collides(registry, current.name(), &wrapper) {
                return Err(ReasonCode::LocalDeny);
            }
            via_skill = Some(wrapper);
        }
        match target {
            DelegationTarget::Builtin { name, capability } => {
                return Ok(Identity {
                    tool_name: fold_or_deny(name)?,
                    capability,
                    mcp_server: None,
                    via_skill,
                });
            }
            DelegationTarget::Tool(next) => current = next,
        }
    }
    Err(ReasonCode::LocalDeny)
}

fn build_request(
    enforcing: &Enforcing,
    facts: &CallFacts<'_>,
) -> Result<PolicyRequest, ReasonCode> {
    let identity = resolve_identity(facts.tool, facts.registry)?;
    if let Some(caller) = &enforcing.caller_allowed {
        let registered = fold_or_deny(facts.tool.name())?;
        if !caller.contains(&registered) && !caller.contains(WILDCARD) {
            return Err(ReasonCode::LocalDeny);
        }
    }
    Ok(PolicyRequest {
        contract_version: CONTRACT_VERSION,
        session_id: zeroclaw_api::TOOL_LOOP_SESSION_KEY
            .try_with(Clone::clone)
            .ok()
            .flatten()
            .filter(|key| !key.is_empty())
            .unwrap_or_else(|| facts.turn_id.to_owned()),
        turn_id: facts.turn_id.to_owned(),
        call_id: facts.call_id.to_owned(),
        agent_alias: facts
            .agent_alias
            .unwrap_or(enforcing.agent_alias.as_str())
            .to_owned(),
        parent_agent_alias: facts
            .parent_agent_alias
            .map(str::to_owned)
            .or_else(|| enforcing.bounded_parent.clone()),
        capability: identity.capability,
        tool_name: identity.tool_name,
        via_skill: identity.via_skill,
        mcp_server: identity.mcp_server,
        args_sha256: args_sha256(facts.args),
        autonomy: enforcing.autonomy,
        interactive: facts.interactive,
    })
}

/// Model-visible text for a denied call. The reason is a closed code, never
/// free text, and the message does not suggest how to widen the policy.
pub fn denial_message(tool_name: &str, reason: ReasonCode) -> String {
    let code = serde_json::to_value(reason)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "denied".to_owned());
    format!("Tool call not executed: '{tool_name}' was denied by policy ({code}).")
}

#[cfg(any(test, feature = "test-util"))]
static PERMIT_ALL_FOR_TEST: PolicyGate = PolicyGate::permit_all_for_test();

/// Shared permit-all gate for fixtures that build a turn context by hand.
#[cfg(any(test, feature = "test-util"))]
pub fn test_permit_all() -> &'static PolicyGate {
    &PERMIT_ALL_FOR_TEST
}

#[cfg(test)]
mod tests;
