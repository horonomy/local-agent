use super::*;
use crate::security::policy_contract::{
    AuthorityInfo, CONTRACT_VERSION, DecisionOutcome, DecisionTrail, ReasonCode, Verdict,
};
use zeroclaw_api::policy::ToolCapability;

fn record(n: usize) -> DecisionRecord {
    DecisionRecord {
        v: 1,
        ts: "2026-11-13T00:00:00.000Z".into(),
        session_id: "s".into(),
        turn_id: "t".into(),
        call_id: format!("call-{n}"),
        agent_alias: "main".into(),
        parent_agent_alias: None,
        capability: ToolCapability::FsRead,
        tool_name: "file_read".into(),
        via_skill: None,
        mcp_server: None,
        args_sha256: "0".repeat(64),
        decision: DecisionTrail {
            local: Verdict::Allow,
            aasm: None,
            effective: Verdict::Allow,
            reason: ReasonCode::LocalAllow,
        },
        authority: AuthorityInfo {
            mode: "standalone".into(),
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

fn dir_files(dir: &Path) -> Vec<(String, u64)> {
    let mut files: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|e| {
            let e = e.unwrap();
            (
                e.file_name().to_string_lossy().into_owned(),
                e.metadata().unwrap().len(),
            )
        })
        .collect();
    files.sort();
    files
}

#[test]
fn records_are_appended_as_json_lines() {
    let tmp = tempfile::tempdir().unwrap();
    let sink = FileDecisionSink::new(tmp.path().join("nested"), &GovernanceAuditConfig::default());
    for n in 0..3 {
        sink.record(&record(n)).unwrap();
    }
    let text = fs::read_to_string(tmp.path().join("nested").join(POLICY_AUDIT_FILE)).unwrap();
    let parsed: Vec<DecisionRecord> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(parsed, (0..3).map(record).collect::<Vec<_>>());
}

#[cfg(unix)]
#[test]
fn log_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("audit");
    FileDecisionSink::new(&dir, &GovernanceAuditConfig::default())
        .record(&record(0))
        .unwrap();
    let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&dir), 0o700);
    assert_eq!(mode(&dir.join(POLICY_AUDIT_FILE)), 0o600);
}

#[test]
fn rotation_keeps_generations_and_never_exceeds_the_total_bound() {
    let tmp = tempfile::tempdir().unwrap();
    let line_len = serde_json::to_vec(&record(0)).unwrap().len() as u64 + 1;
    let max = line_len * 4;
    let generations = 2;
    let sink = FileDecisionSink::with_max_bytes(tmp.path(), max, generations);
    for n in 0..100 {
        sink.record(&record(n)).unwrap();
        let files = dir_files(tmp.path());
        assert!(files.iter().all(|(_, len)| *len <= max), "{files:?}");
        let total: u64 = files.iter().map(|(_, len)| len).sum();
        assert!(total <= max * (u64::from(generations) + 1), "{files:?}");
    }
    let names: Vec<String> = dir_files(tmp.path()).into_iter().map(|(n, _)| n).collect();
    assert_eq!(
        names,
        [
            "policy-audit.jsonl",
            "policy-audit.jsonl.1",
            "policy-audit.jsonl.2"
        ]
    );
    // Newest record is in the live file; older ones moved to generations.
    let live = fs::read_to_string(tmp.path().join(POLICY_AUDIT_FILE)).unwrap();
    assert!(live.contains("call-99"));
}

#[test]
fn zero_generations_keeps_only_the_live_file() {
    let tmp = tempfile::tempdir().unwrap();
    let line_len = serde_json::to_vec(&record(0)).unwrap().len() as u64 + 1;
    let sink = FileDecisionSink::with_max_bytes(tmp.path(), line_len * 2, 0);
    for n in 0..10 {
        sink.record(&record(n)).unwrap();
    }
    assert_eq!(dir_files(tmp.path()).len(), 1);
}

#[test]
fn a_record_larger_than_the_file_limit_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let sink = FileDecisionSink::with_max_bytes(tmp.path(), 16, 1);
    assert!(sink.record(&record(0)).is_err());
    assert!(dir_files(tmp.path()).is_empty());
}

#[test]
fn an_unwritable_location_is_an_error_not_a_silent_drop() {
    let tmp = tempfile::tempdir().unwrap();
    let blocker = tmp.path().join("blocker");
    fs::write(&blocker, b"file").unwrap();
    let sink = FileDecisionSink::new(blocker.join("under"), &GovernanceAuditConfig::default());
    assert!(sink.record(&record(0)).is_err());
}

#[cfg(unix)]
#[test]
fn a_symlinked_log_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("elsewhere");
    fs::write(&target, b"keep").unwrap();
    std::os::unix::fs::symlink(&target, tmp.path().join(POLICY_AUDIT_FILE)).unwrap();
    let sink = FileDecisionSink::new(tmp.path(), &GovernanceAuditConfig::default());
    assert!(sink.record(&record(0)).is_err());
    assert_eq!(fs::read(&target).unwrap(), b"keep");
}

#[test]
fn delete_removes_only_the_audit_files() {
    let tmp = tempfile::tempdir().unwrap();
    let line_len = serde_json::to_vec(&record(0)).unwrap().len() as u64 + 1;
    let sink = FileDecisionSink::with_max_bytes(tmp.path(), line_len * 2, 3);
    for n in 0..12 {
        sink.record(&record(n)).unwrap();
    }
    fs::write(tmp.path().join("policy-audit.jsonl.bak"), b"x").unwrap();
    fs::write(tmp.path().join("other.log"), b"x").unwrap();
    assert_eq!(sink.delete().unwrap(), 4);
    let left: Vec<String> = dir_files(tmp.path()).into_iter().map(|(n, _)| n).collect();
    assert_eq!(left, ["other.log", "policy-audit.jsonl.bak"]);
    assert_eq!(sink.delete().unwrap(), 0);
    // The log can be started again after deletion.
    sink.record(&record(0)).unwrap();
    assert_eq!(delete_policy_audit(tmp.path()).unwrap(), 1);
    assert_eq!(delete_policy_audit(&tmp.path().join("missing")).unwrap(), 0);
}
