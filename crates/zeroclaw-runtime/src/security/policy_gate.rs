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
    ApprovalReceipt, ApprovalResponse, AuthorityInfo, CONTRACT_VERSION, DecisionOutcome,
    DecisionRecord, DecisionSink, DecisionTrail, PolicyAuthority, PolicyRequest, ReasonCode,
    Verdict, args_sha256,
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
    // `*` is the wildcard in policy lists; a tool literally named `*` must
    // never be able to match it.
    if trimmed == WILDCARD
        || trimmed.is_empty()
        || !trimmed.is_ascii()
        || trimmed.bytes().any(|b| b.is_ascii_control())
    {
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
    let allowed_tools = inputs.allowed_tools.map(fold_set);
    PolicyProfile {
        allowed_tools,
        excluded_tools: fold_set(inputs.excluded_tools.unwrap_or_default()),
        mcp_grants: fold_set(inputs.mcp_servers),
        ask_tools: fold_set(inputs.always_ask),
        autonomy_ceiling: inputs.autonomy,
    }
}

/// Standalone authority with parent profiles registered. Kept here rather
/// than in `authority_factory` because it takes per-agent parent profiles the
/// factory's default-profile builders do not.
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
    /// An `Ask` already claimed by an answer; a second answer is refused.
    answered: bool,
    /// The decision's audit record, reused for the approval receipt.
    record: Option<DecisionRecord>,
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
    /// No usable authority: managed mode in a build without the
    /// `aasm-managed` transport (or unix), or a managed authority that failed
    /// to build. Every call is denied `authority_unavailable`; there is no
    /// fallback to standalone.
    Unavailable,
    /// Test-only: admits everything and demands no ticket.
    #[cfg(any(test, feature = "test-util"))]
    PermitAll,
}

struct Enforcing {
    authority: Arc<dyn PolicyAuthority>,
    /// Which authority composition decides; recorded on every audit line.
    mode: GovernanceMode,
    agent_alias: String,
    autonomy: AutonomyLevel,
    /// Folded per-run caller allow-list. Checked in addition to the engine,
    /// so it can narrow but never widen.
    caller_allowed: Option<BTreeSet<String>>,
    /// The caller list as configured, for children that must inherit it.
    caller_list: Option<Vec<String>>,
    /// Parent of a bounded child: every decision is also narrowed by this
    /// parent's profile, registered with the authority.
    bounded_parent: Option<String>,
    /// Own profile followed by every registered parent profile, for the
    /// per-server checks of tools that name MCP servers in their arguments.
    guards: Vec<PolicyProfile>,
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

/// True when `[governance]` failed to load and was reset to its default, so
/// the configured authority (possibly managed) is unknown.
pub fn governance_degraded(config: &Config) -> bool {
    config.degraded_security.iter().any(|s| s == "governance")
}

pub struct PolicyGate {
    mode: Mode,
    /// Receives one record per decision. A write failure denies the call.
    sink: Option<Arc<dyn DecisionSink>>,
    tickets: Mutex<TicketBook>,
    next_decision: AtomicU64,
}

impl PolicyGate {
    const fn with_mode(mode: Mode) -> Self {
        Self {
            mode,
            sink: None,
            tickets: Mutex::new(TicketBook {
                live: Vec::new(),
                late: Vec::new(),
            }),
            next_decision: AtomicU64::new(1),
        }
    }

    /// Gate for one agent, from its configured policy. `governance` selects
    /// the authority: standalone builds the local engine; managed composes it
    /// with the AASM adapter (feature `aasm-managed`, unix) and otherwise, or
    /// if the adapter cannot be built, fails closed with no fallback to
    /// standalone.
    pub fn for_agent(config: &Config, agent_alias: &str, inputs: &AgentPolicyInputs<'_>) -> Self {
        // A `[governance]` section that failed to load was reset to the
        // standalone default; the operator's intent (possibly managed) is
        // unknown, so deny everything rather than fall back to standalone.
        if governance_degraded(config) {
            return Self::with_mode(Mode::Unavailable);
        }
        Self::for_governance(&config.governance, agent_alias, inputs)
    }

    fn for_governance(
        governance: &GovernanceConfig,
        agent_alias: &str,
        inputs: &AgentPolicyInputs<'_>,
    ) -> Self {
        if governance.mode == GovernanceMode::Managed {
            return Self::managed(
                governance,
                agent_alias,
                inputs.autonomy,
                profile_from_inputs(inputs),
                Vec::new(),
            )
            .with_caller_filter(inputs.caller_allowed);
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
        let authority = standalone_authority_with_parents(profile.clone(), parents.clone());
        Self::enforcing(
            GovernanceMode::Standalone,
            agent_alias,
            autonomy,
            authority,
            profile,
            parents,
        )
    }

    /// Managed gate: the local profiles composed with the AASM adapter, which
    /// can only tighten. A configuration the adapter rejects yields a gate
    /// that denies everything, never a standalone gate.
    #[cfg(all(feature = "aasm-managed", unix))]
    fn managed(
        governance: &GovernanceConfig,
        agent_alias: &str,
        autonomy: AutonomyLevel,
        profile: PolicyProfile,
        parents: Vec<(String, PolicyProfile)>,
    ) -> Self {
        let local = standalone_authority_with_parents(profile.clone(), parents.clone());
        match super::aasm_adapter::build_managed_authority(governance, local) {
            Ok(authority) => Self::enforcing(
                GovernanceMode::Managed,
                agent_alias,
                autonomy,
                authority,
                profile,
                parents,
            ),
            Err(_) => Self::with_mode(Mode::Unavailable),
        }
    }

    /// Without the `aasm-managed` transport, managed mode denies every call.
    #[cfg(not(all(feature = "aasm-managed", unix)))]
    fn managed(
        _governance: &GovernanceConfig,
        _agent_alias: &str,
        _autonomy: AutonomyLevel,
        _profile: PolicyProfile,
        _parents: Vec<(String, PolicyProfile)>,
    ) -> Self {
        Self::with_mode(Mode::Unavailable)
    }

    fn enforcing(
        mode: GovernanceMode,
        agent_alias: &str,
        autonomy: AutonomyLevel,
        authority: Arc<dyn PolicyAuthority>,
        profile: PolicyProfile,
        parents: Vec<(String, PolicyProfile)>,
    ) -> Self {
        let guards = std::iter::once(profile)
            .chain(parents.into_iter().map(|(_, p)| p))
            .collect();
        Self::with_mode(Mode::Enforce(Box::new(Enforcing {
            authority,
            mode,
            agent_alias: agent_alias.to_owned(),
            autonomy,
            caller_allowed: None,
            caller_list: None,
            bounded_parent: None,
            guards,
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
        let parents = vec![(parent_alias.to_owned(), profile_from_inputs(parent))];
        let mut gate = if governance.mode == GovernanceMode::Managed {
            Self::managed(
                governance,
                child_alias,
                child.autonomy,
                profile_from_inputs(child),
                parents,
            )
        } else {
            Self::standalone(
                child_alias,
                child.autonomy,
                profile_from_inputs(child),
                parents,
            )
        }
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
            enforcing.caller_list = Some(list.to_vec());
        }
        self
    }

    /// Record every decision to `sink`. If a record cannot be written the call
    /// is denied `audit_write_failed` (ADR-019 D6).
    pub fn with_sink(mut self, sink: Arc<dyn DecisionSink>) -> Self {
        self.sink = Some(sink);
        self
    }

    /// A gate that denies every call. For a path that cannot build the policy
    /// it needs and must fail closed rather than fall back to a default.
    pub const fn deny_all() -> Self {
        Self::with_mode(Mode::Unavailable)
    }

    /// The per-run caller list this gate enforces, if any.
    pub fn caller_list(&self) -> Option<Vec<String>> {
        match &self.mode {
            Mode::Enforce(enforcing) => enforcing.caller_list.clone(),
            _ => None,
        }
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
        let (outcome, record) = self.evaluate(enforcing, facts).await;
        if let GateOutcome::Proceed { ask } = outcome {
            let verdict = if ask { Verdict::Ask } else { Verdict::Allow };
            self.mint(facts, verdict, record);
        }
        outcome
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

    fn mint(&self, facts: &CallFacts<'_>, verdict: Verdict, record: Option<DecisionRecord>) {
        let digest = args_sha256(facts.args);
        let decision_id = record
            .as_ref()
            .and_then(|r| r.decision_id.clone())
            .unwrap_or_else(|| self.next_decision_id());
        let mut book = self.tickets.lock();
        book.live.push(LiveTicket {
            ticket: DecisionTicket {
                call_id: facts.call_id.to_owned(),
                resolved_name: facts.tool.name().to_owned(),
                args_sha256: digest,
                decision_id,
            },
            verdict,
            approved: verdict == Verdict::Allow,
            answered: verdict == Verdict::Allow,
            record,
        });
        let overflow = book.live.len().saturating_sub(MAX_LIVE_TICKETS);
        book.live.drain(..overflow);
    }

    /// Decide a pipeline step, which runs inside the pipeline tool with no
    /// way to prompt: only an outright `Allow` passes, so an `Ask` (a tool in
    /// `always_ask`) and any `Deny` refuse the step. No ticket is minted; the
    /// pipeline tool itself already holds one. The collision check is skipped
    /// because the step registry is not visible here.
    pub async fn decide_step(
        &self,
        tool: &dyn Tool,
        args: &serde_json::Value,
    ) -> Result<(), String> {
        let facts = CallFacts {
            tool,
            registry: &[],
            args,
            call_id: "pipeline-step",
            turn_id: "pipeline-step",
            agent_alias: None,
            parent_agent_alias: None,
            interactive: false,
        };
        let outcome = match &self.mode {
            Mode::Enforce(_) => self.decide_without_ticket(&facts).await,
            Mode::Unavailable => GateOutcome::Deny {
                reason: ReasonCode::AuthorityUnavailable,
            },
            #[cfg(any(test, feature = "test-util"))]
            Mode::PermitAll => return Ok(()),
        };
        match outcome {
            GateOutcome::Proceed { ask: false } => Ok(()),
            GateOutcome::Proceed { ask: true } => {
                Err(denial_message(tool.name(), ReasonCode::LocalDeny))
            }
            GateOutcome::Deny { reason } => Err(denial_message(tool.name(), reason)),
        }
    }

    /// Admission callback for the pipeline tool, sharing this gate.
    pub fn step_admission(self: &Arc<Self>) -> zeroclaw_tools::pipeline::StepAdmission {
        let gate = Arc::clone(self);
        Arc::new(move |tool, args| {
            let gate = Arc::clone(&gate);
            Box::pin(async move { gate.decide_step(tool.as_ref(), &args).await })
        })
    }

    /// An operator approved this Ask call; its ticket becomes redeemable.
    /// The grant is written as an approval receipt first: `false` means it was
    /// not recorded (no open ask, already answered, or the audit write
    /// failed) and the ticket stays unredeemable.
    pub fn approve(&self, call_id: &str, resolved_name: &str, args: &serde_json::Value) -> bool {
        self.answer_ask(call_id, resolved_name, args, ApprovalResponse::Granted)
    }

    /// An operator rejected this Ask call: the receipt is written and the
    /// ticket is dropped, so nothing can run under it.
    pub fn reject(&self, call_id: &str, resolved_name: &str, args: &serde_json::Value) {
        self.answer_ask(call_id, resolved_name, args, ApprovalResponse::Rejected);
    }

    /// The prompt ended without an answer (turn cancelled or timed out): a
    /// `cancelled` receipt is written and the ticket is dropped.
    pub fn cancel_ask(&self, call_id: &str, resolved_name: &str, args: &serde_json::Value) {
        self.answer_ask(call_id, resolved_name, args, ApprovalResponse::Cancelled);
    }

    /// Claim the newest open ask for this exact call, write its receipt
    /// outside the ticket lock, then settle the ticket.
    fn answer_ask(
        &self,
        call_id: &str,
        resolved_name: &str,
        args: &serde_json::Value,
        response: ApprovalResponse,
    ) -> bool {
        let digest = args_sha256(args);
        let (decision_id, template) = {
            let mut book = self.tickets.lock();
            let Some(live) = book.live.iter_mut().rev().find(|t| {
                !t.answered
                    && t.verdict == Verdict::Ask
                    && t.ticket.call_id == call_id
                    && t.ticket.resolved_name == resolved_name
                    && t.ticket.args_sha256 == digest
            }) else {
                return false;
            };
            live.answered = true;
            (live.ticket.decision_id.clone(), live.record.clone())
        };
        let recorded = match (&self.sink, template) {
            (Some(sink), Some(mut receipt)) => {
                receipt.ts = now_ts();
                receipt.approval = Some(ApprovalReceipt {
                    decision_id: decision_id.clone(),
                    response,
                });
                sink.record(&receipt).is_ok()
            }
            _ => true,
        };
        let mut book = self.tickets.lock();
        let Some(index) = book
            .live
            .iter()
            .position(|t| t.ticket.decision_id == decision_id)
        else {
            return false;
        };
        if response == ApprovalResponse::Granted && recorded {
            book.live[index].approved = true;
        } else if response != ApprovalResponse::Granted {
            book.live.remove(index);
        }
        recorded && response == ApprovalResponse::Granted
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
        self.evaluate(enforcing, facts).await.0
    }

    fn next_decision_id(&self) -> String {
        self.next_decision
            .fetch_add(1, Ordering::Relaxed)
            .to_string()
    }

    /// Decide one call and write its record. The record is returned for
    /// `Allow` and `Ask` so the approval receipt can reuse it.
    async fn evaluate(
        &self,
        enforcing: &Enforcing,
        facts: &CallFacts<'_>,
    ) -> (GateOutcome, Option<DecisionRecord>) {
        let request = build_request(enforcing, facts);
        let decision = match &request {
            Ok(request) => Some(enforcing.authority.decide(request).await),
            Err(_) => None,
        };
        let (verdict, reason) = match (&request, &decision) {
            (Err(reason), _) => (Verdict::Deny, *reason),
            (_, Some(d)) => (d.verdict, d.reason),
            _ => (Verdict::Deny, ReasonCode::LocalDeny),
        };
        let mut record = self.sink.as_ref().map(|_| {
            let mut record = decision_record(
                enforcing.mode,
                facts,
                request.as_ref().ok(),
                verdict,
                reason,
            );
            if verdict != Verdict::Deny {
                record.decision_id = Some(self.next_decision_id());
            }
            record.authority.aasm_decision_id = decision
                .and_then(|d| d.authority_ref)
                .map(|id| bounded_id(&id));
            record
        });
        if let (Some(sink), Some(record)) = (&self.sink, &record)
            && sink.record(record).is_err()
        {
            // The error text can carry paths; only the closed code is kept.
            zeroclaw_log::record!(
                WARN,
                zeroclaw_log::Event::new(module_path!(), zeroclaw_log::Action::Fail),
                "policy audit write failed; call denied"
            );
            return (
                GateOutcome::Deny {
                    reason: ReasonCode::AuditWriteFailed,
                },
                None,
            );
        }
        match verdict {
            Verdict::Deny => (GateOutcome::Deny { reason }, None),
            _ => (
                GateOutcome::Proceed {
                    ask: verdict == Verdict::Ask,
                },
                record.take(),
            ),
        }
    }
}

fn now_ts() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Longest identifier kept verbatim in an audit record.
const MAX_AUDIT_ID_LEN: usize = 128;

/// Identifiers can be model-controlled (`call_id`) or long. One that is not a
/// short run of `[A-Za-z0-9._:-]` is replaced by a truncated digest, so it can
/// neither carry content nor grow a record past the file limit.
fn bounded_id(id: &str) -> String {
    use sha2::{Digest, Sha256};
    let plain = id.len() <= MAX_AUDIT_ID_LEN
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-'));
    if plain {
        return id.to_owned();
    }
    let digest = Sha256::digest(id.as_bytes());
    let hex: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    format!("h-{hex}")
}

fn decision_record(
    mode: GovernanceMode,
    facts: &CallFacts<'_>,
    request: Option<&PolicyRequest>,
    verdict: Verdict,
    reason: ReasonCode,
) -> DecisionRecord {
    let opt = |value: Option<&str>| value.map(bounded_id);
    DecisionRecord {
        v: 1,
        ts: now_ts(),
        session_id: bounded_id(request.map_or(facts.turn_id, |r| r.session_id.as_str())),
        turn_id: bounded_id(facts.turn_id),
        call_id: bounded_id(facts.call_id),
        agent_alias: bounded_id(request.map_or(facts.agent_alias.unwrap_or_default(), |r| {
            r.agent_alias.as_str()
        })),
        parent_agent_alias: opt(request.map_or(facts.parent_agent_alias, |r| {
            r.parent_agent_alias.as_deref()
        })),
        capability: request.map_or(ToolCapability::Unknown, |r| r.capability),
        tool_name: bounded_id(request.map_or(facts.tool.name(), |r| r.tool_name.as_str())),
        via_skill: opt(request.and_then(|r| r.via_skill.as_deref())),
        mcp_server: opt(request.and_then(|r| r.mcp_server.as_deref())),
        args_sha256: args_sha256(facts.args),
        decision: DecisionTrail {
            local: verdict,
            aasm: None,
            effective: verdict,
            reason,
        },
        authority: AuthorityInfo {
            mode: match mode {
                GovernanceMode::Standalone => "standalone",
                GovernanceMode::Managed => "managed",
            }
            .to_owned(),
            contract_version: CONTRACT_VERSION,
            aasm_decision_id: None,
        },
        outcome: DecisionOutcome {
            executed: false,
            success: false,
            cancelled: false,
            duration_ms: 0,
        },
        decision_id: None,
        approval: None,
    }
}

/// What a call evaluates as after wrapper resolution.
struct Identity {
    tool_name: String,
    capability: ToolCapability,
    mcp_server: Option<String>,
    via_skill: Option<String>,
    /// Servers a generic MCP tool addresses through its arguments.
    servers_in_args: Option<Vec<String>>,
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

fn resolve_identity(
    tool: &dyn Tool,
    registry: &[Box<dyn Tool>],
    args: &serde_json::Value,
) -> Result<Identity, ReasonCode> {
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
                capability: current.capability_for_call(args),
                mcp_server: current.mcp_server().map(fold_or_deny).transpose()?,
                servers_in_args: current.mcp_servers_for_call(args),
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
                    servers_in_args: None,
                    via_skill,
                });
            }
            DelegationTarget::Tool(next) => current = next,
        }
    }
    Err(ReasonCode::LocalDeny)
}

fn check_servers(guards: &[PolicyProfile], servers: &[String]) -> Result<(), ReasonCode> {
    for profile in guards {
        if servers.is_empty() {
            if profile
                .mcp_grants
                .iter()
                .any(|g| profile.excluded_tools.contains(g))
            {
                return Err(ReasonCode::Excluded);
            }
            continue;
        }
        for server in servers {
            let folded = fold_or_deny(server)?;
            if profile.excluded_tools.contains(&folded) {
                return Err(ReasonCode::Excluded);
            }
            if !profile.mcp_grants.contains(&folded) {
                return Err(ReasonCode::LocalDeny);
            }
        }
    }
    Ok(())
}

fn build_request(
    enforcing: &Enforcing,
    facts: &CallFacts<'_>,
) -> Result<PolicyRequest, ReasonCode> {
    let mut identity = resolve_identity(facts.tool, facts.registry, facts.args)?;
    // An extension tool cannot be authorised under a built-in's name: an
    // allow-list entry such as `tool_search` means the built-in, and would
    // otherwise also grant a guest that took the same name.
    // The gate folds names to lowercase while the inventory is case-exact
    // (`TodoWrite`), so compare ignoring ASCII case.
    if identity.capability == ToolCapability::Plugin
        && zeroclaw_tools::inventory::BUILTIN_TOOLS
            .iter()
            .any(|spec| spec.name.eq_ignore_ascii_case(&identity.tool_name))
    {
        return Err(ReasonCode::LocalDeny);
    }
    if let Some(servers) = identity.servers_in_args.take() {
        // A generic MCP tool (resources, prompts) is not bound to one server:
        // each server named in its arguments must be granted and not excluded,
        // and an all-server call is refused when any granted server is
        // excluded, since its results cannot be filtered. The call itself is
        // then decided as a local introspection tool.
        check_servers(&enforcing.guards, &servers)?;
        identity.capability = ToolCapability::AgentInternal;
    }
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

tokio::task_local! {
    static ACTIVE_CALLER_LIST: Option<Vec<String>>;
}

/// Run `future` with the calling gate's per-run caller list visible to
/// [`active_caller_list`]. The executor wraps every tool execution in it, so a
/// tool that starts a child agent can hand the list on and the child cannot be
/// wider than its parent.
pub async fn scope_caller_list<F: std::future::Future>(
    list: Option<Vec<String>>,
    future: F,
) -> F::Output {
    ACTIVE_CALLER_LIST.scope(list, future).await
}

/// The caller list of the tool call currently executing, if any.
pub fn active_caller_list() -> Option<Vec<String>> {
    ACTIVE_CALLER_LIST.try_with(Clone::clone).ok().flatten()
}

/// Whether a caller-list scope is active at all. The executor always sets
/// one, so `false` means the call left the executor's task (a spawned task
/// loses task-local state) and a tool that must hand the list on cannot know
/// it: it has to fail closed.
pub fn caller_scope_active() -> bool {
    ACTIVE_CALLER_LIST.try_with(|_| ()).is_ok()
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
