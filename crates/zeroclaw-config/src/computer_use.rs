//! Scoped desktop control (`[computer_use]`, HORO-2400).
//!
//! Disabled by default. The `computer_use` tool refuses every action unless
//! the frontmost window matches an explicit `window_allowlist` entry.

use serde::{Deserialize, Serialize};
use zeroclaw_macros::Configurable;

const MAX_ENTRY_LEN: usize = 200;
const MAX_ENTRIES: usize = 32;
const SEPARATOR: &str = "::";

/// One allowlisted app or window: `"<app or bundle id>"` or
/// `"<app or bundle id>::<title substring>"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowRule {
    pub app: String,
    pub title_contains: Option<String>,
}

impl WindowRule {
    /// Parse one entry. Wildcards and empty halves are refused: a rule must
    /// name something.
    pub fn parse(raw: &str) -> Result<Self, String> {
        if raw.len() > MAX_ENTRY_LEN || raw.chars().any(char::is_control) {
            return Err(format!(
                "entry must be at most {MAX_ENTRY_LEN} characters without control characters"
            ));
        }
        let (app, title) = match raw.split_once(SEPARATOR) {
            Some((app, title)) => (app.trim(), Some(title.trim())),
            None => (raw.trim(), None),
        };
        if app.is_empty() || app.contains('*') {
            return Err("the app or bundle id must be non-empty and contain no `*`".into());
        }
        if let Some(title) = title
            && (title.is_empty() || title.contains('*'))
        {
            return Err("the title part must be non-empty and contain no `*`".into());
        }
        Ok(Self {
            app: app.to_owned(),
            title_contains: title.map(str::to_owned),
        })
    }

    /// Case-insensitive: exact app name or bundle id, then title substring.
    pub fn matches(&self, app_name: &str, bundle_id: Option<&str>, title: &str) -> bool {
        let app_ok = self.app.eq_ignore_ascii_case(app_name)
            || bundle_id.is_some_and(|id| self.app.eq_ignore_ascii_case(id));
        app_ok
            && self
                .title_contains
                .as_ref()
                .is_none_or(|needle| title.to_lowercase().contains(&needle.to_lowercase()))
    }
}

fn default_max_actions_per_session() -> u32 {
    20
}
fn default_max_actions_per_minute() -> u32 {
    10
}
fn default_action_timeout_ms() -> u64 {
    5_000
}

/// `[computer_use]`: scoped, per-action-approved desktop control.
#[derive(Debug, Clone, Serialize, Deserialize, Configurable)]
#[cfg_attr(feature = "schema-export", derive(schemars::JsonSchema))]
#[prefix = "computer_use"]
pub struct ComputerUseConfig {
    /// Register the `computer_use` tool (needs a `local-desktop` build and an
    /// explicit `allowed_tools` listing). Default: false.
    #[serde(default)]
    pub enabled: bool,
    /// Windows the tool may act on: `"App"`, `"bundle.id"` or `"App::title"`.
    /// Empty denies every action.
    #[serde(default)]
    pub window_allowlist: Vec<String>,
    /// Actions allowed per tool instance (one session) before it refuses.
    #[serde(default = "default_max_actions_per_session")]
    pub max_actions_per_session: u32,
    /// Sliding one-minute action limit.
    #[serde(default = "default_max_actions_per_minute")]
    pub max_actions_per_minute: u32,
    /// Per-action deadline in milliseconds.
    #[serde(default = "default_action_timeout_ms")]
    pub action_timeout_ms: u64,
}

impl Default for ComputerUseConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            window_allowlist: Vec::new(),
            max_actions_per_session: default_max_actions_per_session(),
            max_actions_per_minute: default_max_actions_per_minute(),
            action_timeout_ms: default_action_timeout_ms(),
        }
    }
}

impl ComputerUseConfig {
    /// Parsed allowlist; the first bad entry is an error.
    pub fn rules(&self) -> Result<Vec<WindowRule>, String> {
        self.window_allowlist
            .iter()
            .enumerate()
            .map(|(i, raw)| {
                WindowRule::parse(raw)
                    .map_err(|e| format!("computer_use.window_allowlist[{i}]: {e}"))
            })
            .collect()
    }

    /// `(config path, message)` of the first invalid setting.
    pub fn validation_error(&self) -> Option<(&'static str, String)> {
        if self.window_allowlist.len() > MAX_ENTRIES {
            return Some((
                "computer_use.window_allowlist",
                format!("at most {MAX_ENTRIES} entries"),
            ));
        }
        if let Err(message) = self.rules() {
            return Some(("computer_use.window_allowlist", message));
        }
        if !(1..=1_000).contains(&self.max_actions_per_session) {
            return Some((
                "computer_use.max_actions_per_session",
                "must be between 1 and 1000".into(),
            ));
        }
        if !(1..=600).contains(&self.max_actions_per_minute) {
            return Some((
                "computer_use.max_actions_per_minute",
                "must be between 1 and 600".into(),
            ));
        }
        if !(100..=60_000).contains(&self.action_timeout_ms) {
            return Some((
                "computer_use.action_timeout_ms",
                "must be between 100 and 60000".into(),
            ));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_disabled_and_valid() {
        let c = ComputerUseConfig::default();
        assert!(!c.enabled);
        assert!(c.window_allowlist.is_empty());
        assert!(c.validation_error().is_none());
    }

    #[test]
    fn rules_reject_wildcards_and_empty_halves() {
        for bad in ["", "  ", "*", "App*", "App::", "App::*", "::title", "a\nb"] {
            assert!(WindowRule::parse(bad).is_err(), "{bad:?}");
        }
        assert!(WindowRule::parse(&"a".repeat(201)).is_err());
    }

    #[test]
    fn rule_matches_app_or_bundle_and_title() {
        let rule = WindowRule::parse("Notes::meeting").unwrap();
        assert!(rule.matches("notes", None, "Team Meeting"));
        assert!(!rule.matches("notes", None, "Groceries"));
        assert!(!rule.matches("Other", None, "meeting"));
        let bundle = WindowRule::parse("com.apple.Notes").unwrap();
        assert!(bundle.matches("Notes", Some("com.apple.notes"), "x"));
        assert!(!bundle.matches("Notes", None, "x"));
    }

    #[test]
    fn validation_flags_bounds_and_bad_entries() {
        let mut c = ComputerUseConfig::default();
        c.window_allowlist = vec!["*".into()];
        assert_eq!(
            c.validation_error().unwrap().0,
            "computer_use.window_allowlist"
        );
        c.window_allowlist = vec!["Notes".into(); MAX_ENTRIES + 1];
        assert!(c.validation_error().is_some());
        c.window_allowlist = vec!["Notes".into()];
        let mut bad = c.clone();
        bad.max_actions_per_session = 0;
        assert_eq!(
            bad.validation_error().unwrap().0,
            "computer_use.max_actions_per_session"
        );
        let mut bad = c.clone();
        bad.max_actions_per_minute = 601;
        assert_eq!(
            bad.validation_error().unwrap().0,
            "computer_use.max_actions_per_minute"
        );
        let mut bad = c.clone();
        bad.action_timeout_ms = 99;
        assert_eq!(
            bad.validation_error().unwrap().0,
            "computer_use.action_timeout_ms"
        );
        assert!(c.validation_error().is_none());
    }
}
