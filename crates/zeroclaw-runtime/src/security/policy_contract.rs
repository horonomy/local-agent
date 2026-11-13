//! Versioned policy decision contract (ADR-019).
//!
//! Types only: no production caller consumes them yet. HORO-2394 wires the
//! standalone engine and the enforcement point. Decisions combine solely
//! through [`strictest`], so an external authority can never widen a local
//! decision.

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroclaw_api::policy::ToolCapability;
use zeroclaw_config::autonomy::AutonomyLevel;

/// Current contract version carried by every request and audit record.
pub const CONTRACT_VERSION: u32 = 1;

/// Runtime-injected argument key excluded from the argument digest.
const APPROVED_KEY: &str = "approved";

/// Ordered `Allow < Ask < Deny`; the derived `Ord` is the strictness order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Allow,
    Ask,
    Deny,
}

/// Closed set of decision reasons. Never model-authored text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ReasonCode {
    LocalAllow,
    LocalDeny,
    AasmDeny,
    AuthorityUnavailable,
    UnknownCapability,
    Excluded,
    NetworkNotGranted,
    ContractVersionMismatch,
    AuditWriteFailed,
    Timeout,
    MalformedResponse,
}

/// Which authority produced a decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionSource {
    Local,
    Aasm,
    Composed,
}

/// A policy decision. `reason` is a code; `authority_ref` is an opaque
/// identifier assigned by the deciding authority (for example an AASM
/// decision id), never free text from a model or tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyDecision {
    pub verdict: Verdict,
    pub reason: ReasonCode,
    pub source: DecisionSource,
    pub authority_ref: Option<String>,
}

impl PolicyDecision {
    pub fn new(verdict: Verdict, reason: ReasonCode, source: DecisionSource) -> Self {
        Self {
            verdict,
            reason,
            source,
            authority_ref: None,
        }
    }

    pub fn deny(reason: ReasonCode, source: DecisionSource) -> Self {
        Self::new(Verdict::Deny, reason, source)
    }
}

/// Combine two decisions, keeping the stricter. On a tie the first wins,
/// so callers pass the local decision first.
pub fn strictest(a: PolicyDecision, b: PolicyDecision) -> PolicyDecision {
    if b.verdict > a.verdict { b } else { a }
}

/// One tool call to be decided. Carries no raw arguments, only a digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyRequest {
    pub contract_version: u32,
    pub session_id: String,
    pub turn_id: String,
    pub call_id: String,
    pub agent_alias: String,
    pub parent_agent_alias: Option<String>,
    pub capability: ToolCapability,
    /// Resolved canonical tool name (after alias/wrapper resolution).
    pub tool_name: String,
    pub via_skill: Option<String>,
    pub mcp_server: Option<String>,
    pub args_sha256: String,
    pub autonomy: AutonomyLevel,
    pub interactive: bool,
}

/// Reject a request whose contract version this build does not implement.
/// Returns the Deny to apply, or `None` when the version is supported.
pub fn reject_unsupported_version(request: &PolicyRequest) -> Option<PolicyDecision> {
    (request.contract_version != CONTRACT_VERSION)
        .then(|| PolicyDecision::deny(ReasonCode::ContractVersionMismatch, DecisionSource::Local))
}

/// Hex SHA-256 of the canonical JSON of `args` (object keys sorted, no
/// whitespace), excluding the runtime-injected top-level `approved` key.
pub fn args_sha256(args: &serde_json::Value) -> String {
    let mut canonical = String::new();
    write_canonical(args, true, &mut canonical);
    hex::encode(Sha256::digest(canonical.as_bytes()))
}

fn write_canonical(value: &serde_json::Value, top_level: bool, out: &mut String) {
    use serde_json::Value;
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map
                .keys()
                .filter(|k| !(top_level && k.as_str() == APPROVED_KEY))
                .collect();
            keys.sort();
            out.push('{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(key).unwrap_or_default());
                out.push(':');
                write_canonical(&map[key], false, out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(item, false, out);
            }
            out.push(']');
        }
        scalar => out.push_str(&scalar.to_string()),
    }
}

/// Decides a request. Implementations must be total: failure to decide is a
/// `Deny`, never an error the caller could interpret as permission.
#[async_trait]
pub trait PolicyAuthority: Send + Sync {
    async fn decide(&self, request: &PolicyRequest) -> PolicyDecision;
}

/// Durable sink for decision records. A write failure is a Deny at the
/// enforcement point (`ReasonCode::AuditWriteFailed`).
pub trait DecisionSink: Send + Sync {
    fn record(&self, record: &DecisionRecord) -> Result<()>;
}

/// Decisions leading to the effective verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionTrail {
    pub local: Verdict,
    pub aasm: Option<Verdict>,
    pub effective: Verdict,
    pub reason: ReasonCode,
}

/// Authority metadata of the session that produced a record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorityInfo {
    pub mode: String,
    pub contract_version: u32,
    pub aasm_decision_id: Option<String>,
}

/// What happened after the decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionOutcome {
    pub executed: bool,
    pub success: bool,
    pub cancelled: bool,
    pub duration_ms: u64,
}

/// How an operator answered an `Ask`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalResponse {
    Granted,
    Rejected,
}

/// Receipt of an operator's answer, scoped to one decision: `decision_id`
/// names the ticket that the grant makes redeemable (once).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalReceipt {
    pub decision_id: String,
    pub response: ApprovalResponse,
}

/// One audit line (`policy-audit.jsonl`, `v: 1`). Raw arguments and tool
/// output are never recorded: only `args_sha256`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecisionRecord {
    pub v: u32,
    pub ts: String,
    pub session_id: String,
    pub turn_id: String,
    pub call_id: String,
    pub agent_alias: String,
    pub parent_agent_alias: Option<String>,
    pub capability: ToolCapability,
    pub tool_name: String,
    pub via_skill: Option<String>,
    pub mcp_server: Option<String>,
    pub args_sha256: String,
    pub decision: DecisionTrail,
    pub authority: AuthorityInfo,
    pub outcome: DecisionOutcome,
    /// Gate-local id of the ticket minted for an `Allow` or `Ask`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_id: Option<String>,
    /// Present on the record written when an `Ask` is answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<ApprovalReceipt>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const VERDICTS: [Verdict; 3] = [Verdict::Allow, Verdict::Ask, Verdict::Deny];

    fn decision(verdict: Verdict, source: DecisionSource) -> PolicyDecision {
        PolicyDecision::new(verdict, ReasonCode::LocalAllow, source)
    }

    #[test]
    fn strictest_lattice_is_exhaustive() {
        for a in VERDICTS {
            for b in VERDICTS {
                let got = strictest(
                    decision(a, DecisionSource::Local),
                    decision(b, DecisionSource::Aasm),
                );
                assert_eq!(got.verdict, a.max(b), "{a:?} vs {b:?}");
                if a == b {
                    assert_eq!(got.source, DecisionSource::Local, "tie keeps first");
                } else if b > a {
                    assert_eq!(got.source, DecisionSource::Aasm);
                }
            }
        }
        let order = [Verdict::Deny, Verdict::Ask, Verdict::Allow];
        assert!(order.windows(2).all(|w| w[0] > w[1]));
    }

    #[test]
    fn aasm_allow_cannot_widen_local_deny() {
        let local = PolicyDecision::deny(ReasonCode::LocalDeny, DecisionSource::Local);
        let aasm = decision(Verdict::Allow, DecisionSource::Aasm);
        assert_eq!(strictest(local, aasm).verdict, Verdict::Deny);
    }

    fn request(version: u32) -> PolicyRequest {
        PolicyRequest {
            contract_version: version,
            session_id: "s".into(),
            turn_id: "t".into(),
            call_id: "c".into(),
            agent_alias: "main".into(),
            parent_agent_alias: None,
            capability: ToolCapability::Shell,
            tool_name: "shell".into(),
            via_skill: None,
            mcp_server: None,
            args_sha256: args_sha256(&json!({"command": "ls"})),
            autonomy: AutonomyLevel::Supervised,
            interactive: true,
        }
    }

    #[test]
    fn unknown_contract_version_is_denied() {
        assert!(reject_unsupported_version(&request(CONTRACT_VERSION)).is_none());
        for v in [0, CONTRACT_VERSION + 1, u32::MAX] {
            let denied = reject_unsupported_version(&request(v)).expect("rejected");
            assert_eq!(denied.verdict, Verdict::Deny);
            assert_eq!(denied.reason, ReasonCode::ContractVersionMismatch);
        }
    }

    #[test]
    fn args_digest_is_canonical_and_ignores_approved() {
        let a = args_sha256(&json!({"b": 1, "a": {"y": [1, 2], "x": null}}));
        let b = args_sha256(&json!({"a": {"x": null, "y": [1, 2]}, "b": 1, "approved": true}));
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
        assert_ne!(
            a,
            args_sha256(&json!({"b": 2, "a": {"y": [1, 2], "x": null}}))
        );
        // Only the top-level key is runtime-injected.
        assert_ne!(
            args_sha256(&json!({"n": {"approved": true}})),
            args_sha256(&json!({"n": {}}))
        );
    }

    fn record() -> DecisionRecord {
        let req = request(CONTRACT_VERSION);
        DecisionRecord {
            v: 1,
            ts: "2026-11-12T00:00:00Z".into(),
            session_id: req.session_id,
            turn_id: req.turn_id,
            call_id: req.call_id,
            agent_alias: req.agent_alias,
            parent_agent_alias: None,
            capability: req.capability,
            tool_name: req.tool_name,
            via_skill: None,
            mcp_server: None,
            args_sha256: req.args_sha256,
            decision: DecisionTrail {
                local: Verdict::Allow,
                aasm: Some(Verdict::Ask),
                effective: Verdict::Ask,
                reason: ReasonCode::AasmDeny,
            },
            authority: AuthorityInfo {
                mode: "managed".into(),
                contract_version: CONTRACT_VERSION,
                aasm_decision_id: Some("d-1".into()),
            },
            outcome: DecisionOutcome {
                executed: false,
                success: false,
                cancelled: false,
                duration_ms: 3,
            },
            decision_id: None,
            approval: None,
        }
    }

    #[test]
    fn decision_record_round_trips_without_raw_args() {
        let rec = record();
        let line = serde_json::to_string(&rec).unwrap();
        assert_eq!(serde_json::from_str::<DecisionRecord>(&line).unwrap(), rec);

        let value: serde_json::Value = serde_json::from_str(&line).unwrap();
        let keys: Vec<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        for forbidden in ["args", "arguments", "output", "result"] {
            assert!(
                !keys.contains(&forbidden),
                "{forbidden} must not be recorded"
            );
        }
        assert!(keys.contains(&"args_sha256"));
        assert_eq!(value["decision"]["reason"], "aasm_deny");
    }
}
