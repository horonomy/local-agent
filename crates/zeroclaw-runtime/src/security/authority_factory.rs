//! Builds the policy authority from `[governance]` (ADR-019).
//!
//! Standalone mode builds the local engine with the deny-all default profile
//! ([`build_standalone_authority`] takes an explicit one); managed mode
//! composes the local engine with the AASM adapter under the `aasm-managed`
//! feature and otherwise refuses startup.

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
    /// Managed mode is configured but `[governance]` is not usable.
    #[error("managed mode configuration invalid: {0}")]
    ManagedConfigInvalid(String),
}

/// Build the authority for a session. Fails closed: there is no fallback
/// from managed to standalone.
pub fn build_authority(
    config: &GovernanceConfig,
) -> Result<Arc<dyn PolicyAuthority>, AuthorityBuildError> {
    match config.mode {
        GovernanceMode::Managed => build_managed(config),
        GovernanceMode::Standalone => Ok(build_standalone_authority(PolicyProfile::default())),
    }
}

/// Managed authority: the local engine composed with the AASM adapter.
#[cfg(all(feature = "aasm-managed", unix))]
fn build_managed(
    config: &GovernanceConfig,
) -> Result<Arc<dyn PolicyAuthority>, AuthorityBuildError> {
    let local = build_standalone_authority(PolicyProfile::default());
    super::aasm_adapter::build_managed_authority(config, local)
        .map_err(|e| AuthorityBuildError::ManagedConfigInvalid(e.to_string()))
}

/// Without the `aasm-managed` feature, managed mode refuses startup.
#[cfg(not(all(feature = "aasm-managed", unix)))]
fn build_managed(
    _config: &GovernanceConfig,
) -> Result<Arc<dyn PolicyAuthority>, AuthorityBuildError> {
    Err(AuthorityBuildError::ManagedUnsupported)
}

/// Standalone authority over an explicit profile.
pub fn build_standalone_authority(profile: PolicyProfile) -> Arc<dyn PolicyAuthority> {
    Arc::new(StandalonePolicyEngine::new(profile))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(all(feature = "aasm-managed", unix)))]
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
