//! Builds the policy authority from `[governance]` (ADR-019).
//!
//! Not called by production code in this build. HORO-2394 replaces the
//! standalone `Unimplemented` branch with the local engine; managed mode
//! fails closed until the `aasm-managed` transport exists.

use std::sync::Arc;

use thiserror::Error;
use zeroclaw_config::governance::{GovernanceConfig, GovernanceMode};

use super::policy_contract::PolicyAuthority;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AuthorityBuildError {
    /// Managed mode is configured but this build has no AASM transport.
    #[error("managed mode unsupported in this build")]
    ManagedUnsupported,
    /// The standalone engine lands in HORO-2394.
    #[error("standalone policy authority not implemented (HORO-2394)")]
    Unimplemented,
}

/// Build the authority for a session. Fails closed: there is no fallback
/// from managed to standalone.
pub fn build_authority(
    config: &GovernanceConfig,
) -> Result<Arc<dyn PolicyAuthority>, AuthorityBuildError> {
    match config.mode {
        GovernanceMode::Managed => Err(AuthorityBuildError::ManagedUnsupported),
        GovernanceMode::Standalone => Err(AuthorityBuildError::Unimplemented),
    }
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

    #[test]
    fn standalone_is_explicitly_unimplemented() {
        assert!(matches!(
            build_authority(&GovernanceConfig::default()),
            Err(AuthorityBuildError::Unimplemented)
        ));
    }
}
