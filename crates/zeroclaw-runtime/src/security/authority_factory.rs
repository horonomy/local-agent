//! Builds the policy authority from `[governance]` (ADR-019).
//!
//! Not called by production code in this build. Standalone mode builds the
//! local engine with the deny-all default profile (the enforcement slice
//! supplies the real profile via [`build_standalone_authority`]); managed mode
//! fails closed until the `aasm-managed` transport exists.

use std::sync::Arc;

use thiserror::Error;
use zeroclaw_config::governance::{GovernanceConfig, GovernanceMode};

use super::policy_contract::PolicyAuthority;
use super::policy_engine::{PolicyProfile, StandalonePolicyEngine};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AuthorityBuildError {
    /// Managed mode is configured but this build has no AASM transport.
    #[error("managed mode unsupported in this build")]
    ManagedUnsupported,
}

/// Build the authority for a session. Fails closed: there is no fallback
/// from managed to standalone.
pub fn build_authority(
    config: &GovernanceConfig,
) -> Result<Arc<dyn PolicyAuthority>, AuthorityBuildError> {
    match config.mode {
        GovernanceMode::Managed => Err(AuthorityBuildError::ManagedUnsupported),
        GovernanceMode::Standalone => Ok(build_standalone_authority(PolicyProfile::default())),
    }
}

/// Standalone authority over an explicit profile.
pub fn build_standalone_authority(profile: PolicyProfile) -> Arc<dyn PolicyAuthority> {
    Arc::new(StandalonePolicyEngine::new(profile))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_fails_closed() {
        let cfg = GovernanceConfig {
            mode: GovernanceMode::Managed,
            ..GovernanceConfig::default()
        };
        assert!(matches!(
            build_authority(&cfg),
            Err(AuthorityBuildError::ManagedUnsupported)
        ));
    }

    #[tokio::test]
    async fn standalone_builds_a_deny_all_default_engine() {
        use crate::security::policy_contract::{PolicyRequest, Verdict, args_sha256};
        use zeroclaw_api::policy::ToolCapability;
        use zeroclaw_config::autonomy::AutonomyLevel;

        let authority = build_authority(&GovernanceConfig::default()).expect("standalone builds");
        let request = PolicyRequest {
            contract_version: 1,
            session_id: "s".into(),
            turn_id: "t".into(),
            call_id: "c".into(),
            agent_alias: "main".into(),
            parent_agent_alias: None,
            capability: ToolCapability::FsRead,
            tool_name: "file_read".into(),
            via_skill: None,
            mcp_server: None,
            args_sha256: args_sha256(&serde_json::json!({})),
            autonomy: AutonomyLevel::ReadOnly,
            interactive: true,
        };
        assert_eq!(authority.decide(&request).await.verdict, Verdict::Deny);
    }
}
