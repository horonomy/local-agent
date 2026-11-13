//! Governance (`[governance]`): selects the tool-call policy authority.
//!
//! Standalone is the default and the only mode that needs no external peer.
//! Managed mode names an explicit AASM Unix socket. See ADR-019.
//!
//! Local Agent never reads AASM environment variables; every managed-mode
//! input lives in this section.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use zeroclaw_macros::Configurable;

use crate::validation_bail;

/// Accepted `timeout_ms` range for an AASM decision query.
pub const AASM_TIMEOUT_MS_RANGE: std::ops::RangeInclusive<u64> = 100..=10_000;
/// Hard ceiling on total policy-audit bytes (`max_size_mb * (generations + 1)`).
pub const AUDIT_TOTAL_MAX_MB: u64 = 64;

/// Which authority decides tool calls.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, zeroclaw_macros::ConfigEnum,
)]
#[cfg_attr(feature = "schema-export", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum GovernanceMode {
    /// Local rules only; no external peer.
    #[default]
    Standalone,
    /// Local rules composed with an AASM runtime over a Unix socket.
    Managed,
}

/// AASM peer settings (`[governance.aasm]`). Unused in standalone mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Configurable)]
#[cfg_attr(feature = "schema-export", derive(schemars::JsonSchema))]
#[prefix = "governance.aasm"]
pub struct AasmConfig {
    /// Absolute Unix socket path of the AASM runtime. No default: the path
    /// must be explicit so a hostile local user cannot pre-create it.
    #[serde(default)]
    pub socket_path: Option<String>,
    /// Peer uid the socket must belong to. Unset means the current uid.
    #[serde(default)]
    pub expected_peer_uid: Option<u32>,
    /// Per-query timeout in milliseconds (100..=10000). A timeout is a deny.
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    /// Optional file holding a credential token. Presence-only: this build
    /// never reads it.
    #[serde(default)]
    #[credential_class = "path_only_reference"]
    pub credential_token_file: Option<String>,
}

fn default_timeout_ms() -> u64 {
    2000
}

impl Default for AasmConfig {
    fn default() -> Self {
        Self {
            socket_path: None,
            expected_peer_uid: None,
            timeout_ms: default_timeout_ms(),
            credential_token_file: None,
        }
    }
}

/// Policy decision audit settings (`[governance.audit]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Configurable)]
#[cfg_attr(feature = "schema-export", derive(schemars::JsonSchema))]
#[prefix = "governance.audit"]
pub struct GovernanceAuditConfig {
    /// Record one decision line per tool call (never raw arguments/output).
    #[serde(default = "default_audit_enabled")]
    pub enabled: bool,
    /// Rotate the active file at this size in MiB.
    #[serde(default = "default_audit_max_size_mb")]
    pub max_size_mb: u64,
    /// Rotated generations kept besides the active file.
    #[serde(default = "default_audit_generations")]
    pub generations: u32,
}

fn default_audit_enabled() -> bool {
    true
}
fn default_audit_max_size_mb() -> u64 {
    8
}
fn default_audit_generations() -> u32 {
    3
}

impl Default for GovernanceAuditConfig {
    fn default() -> Self {
        Self {
            enabled: default_audit_enabled(),
            max_size_mb: default_audit_max_size_mb(),
            generations: default_audit_generations(),
        }
    }
}

/// Top-level governance settings (`[governance]`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Configurable)]
#[cfg_attr(feature = "schema-export", derive(schemars::JsonSchema))]
#[prefix = "governance"]
pub struct GovernanceConfig {
    /// Policy authority. Pinned at session start; never changes mid-session.
    #[serde(default)]
    pub mode: GovernanceMode,
    /// AASM peer settings.
    #[serde(default)]
    #[nested]
    pub aasm: AasmConfig,
    /// Decision audit settings.
    #[serde(default)]
    #[nested]
    pub audit: GovernanceAuditConfig,
}

/// Whether this build contains the AASM managed transport: true only with the
/// `aasm-managed` feature (forwarded from `zeroclaw-runtime`). Otherwise a
/// managed configuration is refused at validation, so a session can never
/// start in a mode this binary cannot honour.
pub const MANAGED_TRANSPORT_COMPILED: bool = cfg!(feature = "aasm-managed");

impl GovernanceConfig {
    /// Validate against the current platform and build.
    pub fn validate(&self) -> Result<()> {
        self.validate_for_platform(cfg!(unix))?;
        self.validate_for_build(MANAGED_TRANSPORT_COMPILED)
    }

    /// Pure form of the build check so both outcomes are testable.
    pub fn validate_for_build(&self, managed_transport_compiled: bool) -> Result<()> {
        if self.mode == GovernanceMode::Managed && !managed_transport_compiled {
            validation_bail!(
                ValidationFailed,
                "governance.mode".to_string(),
                "governance.mode = \"managed\" needs a build with the aasm-managed feature enabled; this build was compiled without it, and there is no fallback to standalone"
            );
        }
        Ok(())
    }

    /// Pure form of [`Self::validate`] so non-unix rules are testable anywhere.
    pub fn validate_for_platform(&self, is_unix: bool) -> Result<()> {
        let audit = &self.audit;
        if audit.max_size_mb == 0 {
            validation_bail!(
                InvalidNumericRange,
                "governance.audit.max_size_mb".to_string(),
                "governance.audit.max_size_mb must be at least 1"
            );
        }
        let total = audit
            .max_size_mb
            .saturating_mul(u64::from(audit.generations).saturating_add(1));
        if total > AUDIT_TOTAL_MAX_MB {
            validation_bail!(
                InvalidNumericRange,
                "governance.audit.generations".to_string(),
                "governance.audit total size (max_size_mb * (generations + 1) = {total} MiB) exceeds the {AUDIT_TOTAL_MAX_MB} MiB limit"
            );
        }

        let aasm = &self.aasm;
        if !AASM_TIMEOUT_MS_RANGE.contains(&aasm.timeout_ms) {
            let (lo, hi) = (AASM_TIMEOUT_MS_RANGE.start(), AASM_TIMEOUT_MS_RANGE.end());
            validation_bail!(
                InvalidNumericRange,
                "governance.aasm.timeout_ms".to_string(),
                "governance.aasm.timeout_ms must be between {lo} and {hi}"
            );
        }
        if let Some(path) = aasm.credential_token_file.as_deref()
            && !path.starts_with('/')
        {
            validation_bail!(
                ValidationFailed,
                "governance.aasm.credential_token_file".to_string(),
                "governance.aasm.credential_token_file must be an absolute path"
            );
        }

        if self.mode != GovernanceMode::Managed {
            return Ok(());
        }
        if !is_unix {
            validation_bail!(
                ValidationFailed,
                "governance.mode".to_string(),
                "governance.mode = \"managed\" is supported on Unix only"
            );
        }
        match aasm.socket_path.as_deref() {
            None | Some("") => validation_bail!(
                ValidationFailed,
                "governance.aasm.socket_path".to_string(),
                "governance.aasm.socket_path is required in managed mode"
            ),
            Some(path) if !path.starts_with('/') => validation_bail!(
                ValidationFailed,
                "governance.aasm.socket_path".to_string(),
                "governance.aasm.socket_path must be an absolute path"
            ),
            Some(_) => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn managed() -> GovernanceConfig {
        GovernanceConfig {
            mode: GovernanceMode::Managed,
            aasm: AasmConfig {
                socket_path: Some("/run/aasm/runtime.sock".into()),
                ..AasmConfig::default()
            },
            ..GovernanceConfig::default()
        }
    }

    #[test]
    fn partial_aasm_section_keeps_defaults() {
        let cfg: GovernanceConfig =
            toml::from_str("[aasm]\nsocket_path = \"/run/aasm/runtime.sock\"\n").unwrap();
        assert_eq!(cfg.aasm.timeout_ms, 2000);
        assert_eq!(cfg.aasm.credential_token_file, None);
        assert_eq!(cfg.mode, GovernanceMode::Standalone);
    }

    #[test]
    fn timeout_and_token_path_are_validated_in_standalone_too() {
        let mut cfg = GovernanceConfig::default();
        cfg.aasm.timeout_ms = 0;
        assert!(cfg.validate_for_platform(true).is_err());
        cfg.aasm.timeout_ms = 2000;
        cfg.aasm.credential_token_file = Some("relative/token".into());
        assert!(cfg.validate_for_platform(true).is_err());
        cfg.aasm.credential_token_file = Some("/etc/aasm/token".into());
        assert!(cfg.validate_for_platform(true).is_ok());
    }

    #[test]
    fn default_is_standalone_and_valid() {
        let cfg = GovernanceConfig::default();
        assert_eq!(cfg.mode, GovernanceMode::Standalone);
        assert_eq!(cfg.aasm.timeout_ms, 2000);
        assert!(cfg.validate_for_platform(true).is_ok());
        assert!(cfg.validate_for_platform(false).is_ok());
        assert!(crate::schema::Config::default().validate().is_ok());
        assert_eq!(
            crate::schema::Config::default().governance.mode,
            GovernanceMode::Standalone
        );
    }

    #[test]
    fn absent_section_deserializes_to_standalone() {
        let cfg: crate::schema::Config = toml::from_str("").unwrap();
        assert_eq!(cfg.governance, GovernanceConfig::default());
    }

    #[test]
    fn managed_is_refused_when_the_build_lacks_the_transport() {
        assert!(managed().validate_for_build(false).is_err());
        assert!(managed().validate_for_build(true).is_ok());
        assert!(
            GovernanceConfig::default()
                .validate_for_build(false)
                .is_ok()
        );
        assert_eq!(MANAGED_TRANSPORT_COMPILED, cfg!(feature = "aasm-managed"));
        assert_eq!(
            managed().validate().is_ok(),
            cfg!(all(unix, feature = "aasm-managed"))
        );
    }

    #[test]
    fn managed_requires_absolute_socket() {
        assert!(managed().validate_for_platform(true).is_ok());
        let mut relative = managed();
        relative.aasm.socket_path = Some("aasm.sock".into());
        assert!(relative.validate_for_platform(true).is_err());
        let mut missing = managed();
        missing.aasm.socket_path = None;
        assert!(missing.validate_for_platform(true).is_err());
    }

    #[test]
    fn timeout_bounds_enforced_in_managed_mode() {
        for (ms, ok) in [(99, false), (100, true), (10_000, true), (10_001, false)] {
            let mut cfg = managed();
            cfg.aasm.timeout_ms = ms;
            assert_eq!(cfg.validate_for_platform(true).is_ok(), ok, "{ms}");
        }
    }

    #[test]
    fn managed_rejected_on_non_unix() {
        assert!(managed().validate_for_platform(false).is_err());
    }

    #[test]
    fn audit_bounds_enforced() {
        let mut cfg = GovernanceConfig::default();
        cfg.audit.generations = 7; // 8 MiB * 8 = 64 MiB: allowed
        assert!(cfg.validate_for_platform(true).is_ok());
        cfg.audit.generations = 8; // 72 MiB: rejected
        assert!(cfg.validate_for_platform(true).is_err());
        cfg.audit.generations = 3;
        cfg.audit.max_size_mb = 0;
        assert!(cfg.validate_for_platform(true).is_err());
    }
}
