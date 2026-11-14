//! Scoped desktop control (`computer_use`, HORO-2400).
//!
//! Mouse and keyboard actions against ONE explicitly allowlisted window. The
//! guards run here, in-process, before anything reaches a [`DesktopBackend`]:
//!
//! - an empty `window_allowlist` denies every action;
//! - the call must name the target `app`, and the frontmost window must be
//!   that app and match an allowlist entry (app name or bundle id, optional
//!   title substring);
//! - click coordinates must fall inside that window's bounds;
//! - a per-session action cap and a sliding per-minute limit apply;
//! - every action has a deadline; dropping the future (timeout or cancel)
//!   must stop the backend, see [`DesktopBackend`].
//!
//! The capability is `Desktop` and the runtime treats the tool as an Ask on
//! every call, so an operator approves each action. This module never asks
//! for, detects-and-grants or works around OS permissions. No backend with
//! native input synthesis is shipped yet: the platform backend answers with an
//! honest error (see [`platform_backend`]). Screenshots are not part of this
//! tool; capture belongs to the screenshot lifecycle.
//!
//! Limits, stated plainly: the window check and the dispatch are two backend
//! calls, so a window change in between is not observable here; coordinates
//! are checked against the bounds the backend reported a moment earlier;
//! typed text is passed to the backend verbatim and only kept out of results,
//! errors and `Debug` output by this module.

use async_trait::async_trait;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};
use zeroclaw_api::tool::{Tool, ToolOutput, ToolResult};
use zeroclaw_config::computer_use::{ComputerUseConfig, WindowRule};

const MAX_TYPED_CHARS: usize = 500;
const WINDOW: Duration = Duration::from_secs(60);
/// Keys that stay inside the focused window. Everything else is refused, so
/// modifiers, chords and system keys (launchers, power, capture) cannot pass.
const NAMED_KEYS: &[&str] = &[
    "enter",
    "return",
    "tab",
    "escape",
    "space",
    "backspace",
    "delete",
    "up",
    "down",
    "left",
    "right",
    "home",
    "end",
    "pageup",
    "pagedown",
];

/// Screen rectangle in the backend's coordinate space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bounds {
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
}

impl Bounds {
    /// Half-open: the right and bottom edges are outside.
    pub fn contains(&self, x: i64, y: i64) -> bool {
        self.width > 0
            && self.height > 0
            && x >= self.x
            && y >= self.y
            && x < self.x.saturating_add(self.width)
            && y < self.y.saturating_add(self.height)
    }
}

/// The window input would currently land in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowInfo {
    pub app_name: String,
    pub bundle_id: Option<String>,
    pub title: String,
    pub bounds: Bounds,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendError {
    /// The OS withheld a permission. Never requested or granted by this code.
    PermissionMissing(String),
    /// No implementation for this OS or build.
    Unsupported(String),
    Failed(String),
}

/// Native seam. Implementations must be cancel-safe: when a returned future is
/// dropped (deadline or cancellation) any helper process or pending event it
/// started must be killed, e.g. via `kill_on_drop`.
#[async_trait]
pub trait DesktopBackend: Send + Sync {
    async fn frontmost_window(&self) -> Result<WindowInfo, BackendError>;
    async fn click(&self, x: i64, y: i64) -> Result<(), BackendError>;
    async fn type_text(&self, text: &str) -> Result<(), BackendError>;
    async fn press_key(&self, key: &str) -> Result<(), BackendError>;
}

/// Backend for builds and systems without native support. Every call fails
/// with the same honest reason.
pub struct UnavailableBackend {
    reason: &'static str,
}

#[cfg(target_os = "macos")]
const PLATFORM_REASON: &str = "the macOS backend requires Accessibility permission and is not enabled in this build; ZeroClaw never requests or grants it";
#[cfg(not(target_os = "macos"))]
const PLATFORM_REASON: &str = "computer_use is unsupported on this OS (only macOS is planned; Windows and Linux have no backend)";

impl UnavailableBackend {
    pub fn new(reason: &'static str) -> Self {
        Self { reason }
    }

    fn error(&self) -> BackendError {
        BackendError::Unsupported(self.reason.to_owned())
    }
}

#[async_trait]
impl DesktopBackend for UnavailableBackend {
    async fn frontmost_window(&self) -> Result<WindowInfo, BackendError> {
        Err(self.error())
    }
    async fn click(&self, _x: i64, _y: i64) -> Result<(), BackendError> {
        Err(self.error())
    }
    async fn type_text(&self, _text: &str) -> Result<(), BackendError> {
        Err(self.error())
    }
    async fn press_key(&self, _key: &str) -> Result<(), BackendError> {
        Err(self.error())
    }
}

/// The backend this build uses on this OS. Today always [`UnavailableBackend`].
pub fn platform_backend() -> Arc<dyn DesktopBackend> {
    Arc::new(UnavailableBackend::new(PLATFORM_REASON))
}

enum DesktopAction {
    Click { x: i64, y: i64 },
    Type { text: String },
    Key { key: String },
}

impl fmt::Debug for DesktopAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Click { x, y } => write!(f, "Click({x}, {y})"),
            Self::Type { text } => write!(f, "Type([redacted {} chars])", text.chars().count()),
            Self::Key { key } => write!(f, "Key({key})"),
        }
    }
}

struct Request {
    app: String,
    action: DesktopAction,
}

#[derive(Default)]
struct Budget {
    used: u32,
    recent: VecDeque<Instant>,
}

pub struct ComputerUseTool {
    backend: Arc<dyn DesktopBackend>,
    rules: Vec<WindowRule>,
    max_per_session: u32,
    max_per_minute: u32,
    deadline: Duration,
    budget: std::sync::Mutex<Budget>,
    /// One action at a time: input is a single shared device.
    serial: tokio::sync::Mutex<()>,
}

fn denied(message: impl Into<String>) -> ToolResult {
    ToolResult {
        success: false,
        output: ToolOutput::default(),
        error: Some(message.into()),
    }
}

fn parse_request(args: &Value) -> Result<Request, String> {
    let action = args
        .get("action")
        .and_then(Value::as_str)
        .ok_or("missing `action`")?;
    let app = args
        .get("app")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .ok_or("`app` is required: name the app or bundle id this action targets")?
        .to_owned();
    let action = match action {
        "click" => {
            let coord = |name: &str| {
                args.get(name)
                    .and_then(Value::as_i64)
                    .ok_or(format!("`{name}` must be an integer"))
            };
            DesktopAction::Click {
                x: coord("x")?,
                y: coord("y")?,
            }
        }
        "type" => {
            let text = args
                .get("text")
                .and_then(Value::as_str)
                .ok_or("`text` is required")?;
            if text.is_empty() || text.chars().count() > MAX_TYPED_CHARS {
                return Err(format!("`text` must be 1..={MAX_TYPED_CHARS} characters"));
            }
            if text.chars().any(is_hostile_text_char) {
                return Err(
                    "`text` must not contain control, private-use, unassigned or invisible formatting characters"
                        .into(),
                );
            }
            DesktopAction::Type {
                text: text.to_owned(),
            }
        }
        "key" => {
            let key = args
                .get("key")
                .and_then(Value::as_str)
                .ok_or("`key` is required")?;
            validate_key(key)?;
            DesktopAction::Key {
                key: key.to_owned(),
            }
        }
        "screenshot" | "screen_capture" => {
            return Err(
                "screen capture is not part of computer_use; use the capture tool, which owns the screenshot lifecycle"
                    .into(),
            );
        }
        other => return Err(format!("unknown action `{other}`; use click, type or key")),
    };
    Ok(Request { app, action })
}

/// Characters that could act as key events (control, AppKit function-key
/// private use), or hide content from the approver (bidi, zero-width,
/// separators, unassigned). `\n` and `\t` are allowed. Std only: Unicode
/// category Cf and Cn are covered by explicit ranges, not a full table.
fn is_hostile_text_char(c: char) -> bool {
    if c == '\n' || c == '\t' {
        return false;
    }
    c.is_control()
        || matches!(c,
            '\u{00AD}' | '\u{061C}' | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{2028}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}'
            | '\u{E000}'..='\u{F8FF}'
            | '\u{FEFF}'
            | '\u{FFF0}'..='\u{FFFF}'
            | '\u{E0000}'..='\u{E0FFF}'
            | '\u{F0000}'..='\u{10FFFF}')
}

fn validate_key(key: &str) -> Result<(), String> {
    let lowered = key.to_ascii_lowercase();
    let single = key.len() == 1 && key.chars().all(|c| c.is_ascii_alphanumeric());
    // Canonical `f1`..`f12` only: no sign, no leading zero.
    let function = matches!(
        lowered.as_str(),
        "f1" | "f2" | "f3" | "f4" | "f5" | "f6" | "f7" | "f8" | "f9" | "f10" | "f11" | "f12"
    );
    if single || function || NAMED_KEYS.contains(&lowered.as_str()) {
        Ok(())
    } else {
        Err("`key` must be one letter or digit, F1-F12, or one of: enter, return, tab, escape, space, backspace, delete, up, down, left, right, home, end, pageup, pagedown; modifiers, chords and system keys are not allowed".into())
    }
}

impl ComputerUseTool {
    pub fn new(
        backend: Arc<dyn DesktopBackend>,
        config: &ComputerUseConfig,
    ) -> anyhow::Result<Self> {
        if let Some((path, message)) = config.validation_error() {
            anyhow::bail!("{path}: {message}");
        }
        Ok(Self {
            backend,
            rules: config.rules().map_err(anyhow::Error::msg)?,
            max_per_session: config.max_actions_per_session,
            max_per_minute: config.max_actions_per_minute,
            deadline: Duration::from_millis(config.action_timeout_ms),
            budget: std::sync::Mutex::new(Budget::default()),
            serial: tokio::sync::Mutex::new(()),
        })
    }

    /// Count one dispatch; `false` when a limit is reached.
    fn charge(&self) -> Result<(), String> {
        let mut budget = self
            .budget
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if budget.used >= self.max_per_session {
            return Err(format!(
                "session action limit ({}) reached; start a new session to continue",
                self.max_per_session
            ));
        }
        let now = Instant::now();
        while budget
            .recent
            .front()
            .is_some_and(|t| now.duration_since(*t) >= WINDOW)
        {
            budget.recent.pop_front();
        }
        if budget.recent.len() >= self.max_per_minute as usize {
            return Err(format!(
                "rate limit ({} actions per minute) reached; wait and retry",
                self.max_per_minute
            ));
        }
        budget.used += 1;
        budget.recent.push_back(now);
        Ok(())
    }

    fn backend_failure(error: BackendError, secret: Option<&str>) -> ToolResult {
        let scrub = |m: String| match secret {
            Some(s) if !s.is_empty() => m.replace(s, "[redacted]"),
            _ => m,
        };
        denied(match error {
            BackendError::PermissionMissing(detail) => scrub(format!(
                "an OS permission is missing: {detail}. ZeroClaw does not request or grant OS permissions; the machine owner must enable it"
            )),
            BackendError::Unsupported(detail) => scrub(detail),
            BackendError::Failed(detail) => scrub(format!("desktop action failed: {detail}")),
        })
    }

    async fn run(&self, request: Request) -> ToolResult {
        let secret = match &request.action {
            DesktopAction::Type { text } => Some(text.as_str()),
            _ => None,
        };
        let window = match self.backend.frontmost_window().await {
            Ok(window) => window,
            Err(e) => return Self::backend_failure(e, secret),
        };
        let named = request.app.eq_ignore_ascii_case(&window.app_name)
            || window
                .bundle_id
                .as_deref()
                .is_some_and(|id| request.app.eq_ignore_ascii_case(id));
        if !named {
            return denied(format!(
                "denied: the frontmost window is not `{}`; bring the target window forward first",
                request.app
            ));
        }
        if !self
            .rules
            .iter()
            .any(|r| r.matches(&window.app_name, window.bundle_id.as_deref(), &window.title))
        {
            return denied(format!(
                "denied: this window of `{}` is not in computer_use.window_allowlist",
                window.app_name
            ));
        }
        if let DesktopAction::Click { x, y } = request.action
            && !window.bounds.contains(x, y)
        {
            return denied("denied: the coordinates are outside the allowlisted window");
        }
        if let Err(message) = self.charge() {
            return denied(format!("denied: {message}"));
        }
        let (outcome, done) = match &request.action {
            DesktopAction::Click { x, y } => (
                self.backend.click(*x, *y).await,
                format!("clicked ({x}, {y}) in {}", window.app_name),
            ),
            DesktopAction::Type { text } => (
                self.backend.type_text(text).await,
                format!(
                    "typed {} characters into {}",
                    text.chars().count(),
                    window.app_name
                ),
            ),
            DesktopAction::Key { key } => (
                self.backend.press_key(key).await,
                format!("pressed {key} in {}", window.app_name),
            ),
        };
        match outcome {
            Ok(()) => ToolResult::ok(done),
            Err(e) => Self::backend_failure(e, secret),
        }
    }
}

#[async_trait]
impl Tool for ComputerUseTool {
    zeroclaw_api::declare_capability!(Desktop);

    fn name(&self) -> &str {
        "computer_use"
    }

    fn description(&self) -> &str {
        "Click, type or press a key in one explicitly allowlisted desktop window. \
         Every action needs operator approval. Name the target `app`; coordinates \
         must lie inside that window. Not a screenshot tool."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["click", "type", "key"]},
                "app": {"type": "string", "description": "App name or bundle id of the target window (must be the frontmost window)"},
                "x": {"type": "integer", "description": "Screen X, inside the window (click)"},
                "y": {"type": "integer", "description": "Screen Y, inside the window (click)"},
                "text": {"type": "string", "description": "Text to type (type)"},
                "key": {"type": "string", "description": "Key name such as Enter or Tab; no modifier chords (key)"}
            },
            "required": ["action", "app"]
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let request = match parse_request(&args) {
            Ok(request) => request,
            Err(message) => return Ok(denied(message)),
        };
        if self.rules.is_empty() {
            return Ok(denied(
                "denied: computer_use.window_allowlist is empty, so no window is accessible",
            ));
        }
        let _one_at_a_time = self.serial.lock().await;
        Ok(
            match tokio::time::timeout(self.deadline, self.run(request)).await {
                Ok(result) => result,
                Err(_) => denied(
                    "denied: the desktop action timed out and was cancelled; its result is unknown",
                ),
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::sync::Notify;

    const SECRET: &str = "hunter2-correct-horse";

    struct DropFlag(Arc<AtomicBool>);
    impl Drop for DropFlag {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    struct Fake {
        window: Result<WindowInfo, BackendError>,
        calls: Mutex<Vec<String>>,
        hang_on_click: bool,
        click_started: Arc<Notify>,
        helper_killed: Arc<AtomicBool>,
        fail_with: Option<BackendError>,
    }

    fn notes_window() -> WindowInfo {
        WindowInfo {
            app_name: "Notes".into(),
            bundle_id: Some("com.apple.Notes".into()),
            title: "Meeting".into(),
            bounds: Bounds {
                x: 100,
                y: 100,
                width: 400,
                height: 300,
            },
        }
    }

    impl Fake {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                window: Ok(notes_window()),
                calls: Mutex::new(Vec::new()),
                hang_on_click: false,
                click_started: Arc::new(Notify::new()),
                helper_killed: Arc::new(AtomicBool::new(false)),
                fail_with: None,
            })
        }
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl DesktopBackend for Fake {
        async fn frontmost_window(&self) -> Result<WindowInfo, BackendError> {
            self.calls.lock().unwrap().push("frontmost".into());
            self.window.clone()
        }
        async fn click(&self, x: i64, y: i64) -> Result<(), BackendError> {
            if self.hang_on_click {
                let _helper = DropFlag(self.helper_killed.clone());
                self.click_started.notify_one();
                std::future::pending::<()>().await;
            }
            self.calls.lock().unwrap().push(format!("click {x},{y}"));
            Ok(())
        }
        async fn type_text(&self, text: &str) -> Result<(), BackendError> {
            if let Some(e) = &self.fail_with {
                return Err(e.clone());
            }
            self.calls.lock().unwrap().push(format!("type {text}"));
            Ok(())
        }
        async fn press_key(&self, key: &str) -> Result<(), BackendError> {
            self.calls.lock().unwrap().push(format!("key {key}"));
            Ok(())
        }
    }

    fn config(allow: &[&str]) -> ComputerUseConfig {
        ComputerUseConfig {
            enabled: true,
            window_allowlist: allow.iter().map(|s| (*s).to_owned()).collect(),
            ..ComputerUseConfig::default()
        }
    }

    fn tool(backend: &Arc<Fake>, cfg: &ComputerUseConfig) -> ComputerUseTool {
        ComputerUseTool::new(backend.clone(), cfg).unwrap()
    }

    fn click(app: &str, x: i64, y: i64) -> Value {
        json!({"action": "click", "app": app, "x": x, "y": y})
    }

    fn error_of(result: &ToolResult) -> String {
        assert!(!result.success, "expected a denial");
        result.error.clone().unwrap_or_default()
    }

    #[tokio::test]
    async fn empty_allowlist_denies_every_action_without_touching_the_backend() {
        let backend = Fake::new();
        let t = tool(&backend, &config(&[]));
        for args in [
            click("Notes", 150, 150),
            json!({"action": "type", "app": "Notes", "text": "hi"}),
            json!({"action": "key", "app": "Notes", "key": "Enter"}),
        ] {
            let r = t.execute(args).await.unwrap();
            assert!(error_of(&r).contains("allowlist is empty"));
        }
        assert!(backend.calls().is_empty());
    }

    #[tokio::test]
    async fn wrong_app_or_unlisted_window_is_denied() {
        let backend = Fake::new();
        // Frontmost is Notes/"Meeting".
        let t = tool(&backend, &config(&["Notes"]));
        let r = t.execute(click("Mail", 150, 150)).await.unwrap();
        assert!(error_of(&r).contains("not `Mail`"));
        let other = tool(&backend, &config(&["Mail"]));
        let r = other.execute(click("Notes", 150, 150)).await.unwrap();
        assert!(error_of(&r).contains("not in computer_use.window_allowlist"));
        let titled = tool(&backend, &config(&["Notes::Groceries"]));
        let r = titled.execute(click("Notes", 150, 150)).await.unwrap();
        assert!(error_of(&r).contains("not in computer_use.window_allowlist"));
        assert!(backend.calls().iter().all(|c| c == "frontmost"));
    }

    #[tokio::test]
    async fn coordinates_outside_the_window_are_denied_edges_included() {
        let backend = Fake::new();
        let t = tool(&backend, &config(&["Notes"]));
        for (x, y) in [(99, 150), (150, 99), (500, 150), (150, 400), (-1, -1)] {
            let r = t.execute(click("Notes", x, y)).await.unwrap();
            assert!(
                error_of(&r).contains("outside the allowlisted window"),
                "{x},{y}"
            );
        }
        assert!(backend.calls().iter().all(|c| c == "frontmost"));
        let r = t.execute(click("Notes", 499, 399)).await.unwrap();
        assert!(r.success);
    }

    #[tokio::test]
    async fn allowed_window_executes_each_action_exactly_once() {
        let backend = Fake::new();
        let t = tool(&backend, &config(&["com.apple.notes::meet"]));
        assert!(t.execute(click("notes", 150, 150)).await.unwrap().success);
        let typed = t
            .execute(json!({"action": "type", "app": "Notes", "text": "hello"}))
            .await
            .unwrap();
        assert!(typed.success);
        assert!(
            t.execute(json!({"action": "key", "app": "Notes", "key": "Enter"}))
                .await
                .unwrap()
                .success
        );
        let acted: Vec<_> = backend
            .calls()
            .into_iter()
            .filter(|c| c != "frontmost")
            .collect();
        assert_eq!(acted, ["click 150,150", "type hello", "key Enter"]);
    }

    #[tokio::test]
    async fn session_and_rate_limits_apply_and_denied_calls_do_not_consume_them() {
        let backend = Fake::new();
        let mut cfg = config(&["Notes"]);
        cfg.max_actions_per_session = 2;
        let t = tool(&backend, &cfg);
        // Scope denials are free.
        for _ in 0..3 {
            let _ = t.execute(click("Notes", 0, 0)).await.unwrap();
        }
        assert!(t.execute(click("Notes", 150, 150)).await.unwrap().success);
        assert!(t.execute(click("Notes", 150, 150)).await.unwrap().success);
        let r = t.execute(click("Notes", 150, 150)).await.unwrap();
        assert!(error_of(&r).contains("session action limit"));

        let mut cfg = config(&["Notes"]);
        cfg.max_actions_per_minute = 1;
        let t = tool(&backend, &cfg);
        assert!(t.execute(click("Notes", 150, 150)).await.unwrap().success);
        let r = t.execute(click("Notes", 150, 150)).await.unwrap();
        assert!(error_of(&r).contains("rate limit"));
    }

    #[tokio::test]
    async fn deadline_cancels_the_backend_action() {
        let backend = Arc::new(Fake {
            hang_on_click: true,
            ..Arc::try_unwrap(Fake::new()).ok().unwrap()
        });
        let mut cfg = config(&["Notes"]);
        cfg.action_timeout_ms = 100;
        let t = ComputerUseTool::new(backend.clone(), &cfg).unwrap();
        let r = t.execute(click("Notes", 150, 150)).await.unwrap();
        assert!(error_of(&r).contains("timed out"));
        assert!(backend.helper_killed.load(Ordering::SeqCst));
    }

    #[tokio::test]
    #[allow(
        clippy::disallowed_methods,
        reason = "test spawns the call so it can be aborted mid-flight"
    )]
    async fn cancelling_the_call_cancels_the_backend_action() {
        let backend = Arc::new(Fake {
            hang_on_click: true,
            ..Arc::try_unwrap(Fake::new()).ok().unwrap()
        });
        let t = Arc::new(ComputerUseTool::new(backend.clone(), &config(&["Notes"])).unwrap());
        let handle = tokio::spawn({
            let t = t.clone();
            async move { t.execute(click("Notes", 150, 150)).await }
        });
        backend.click_started.notified().await;
        handle.abort();
        let _ = handle.await;
        assert!(backend.helper_killed.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn typed_text_never_reaches_results_errors_or_debug_output() {
        let backend = Arc::new(Fake {
            fail_with: Some(BackendError::Failed(format!("rejected {SECRET}"))),
            ..Arc::try_unwrap(Fake::new()).ok().unwrap()
        });
        let t = tool(&backend, &config(&["Notes"]));
        let args = json!({"action": "type", "app": "Notes", "text": SECRET});
        let failed = t.execute(args.clone()).await.unwrap();
        assert!(!error_of(&failed).contains(SECRET));

        let ok_backend = Fake::new();
        let ok = tool(&ok_backend, &config(&["Notes"]))
            .execute(args.clone())
            .await
            .unwrap();
        assert!(ok.success);
        assert!(!format!("{:?}", ok.output).contains(SECRET));
        assert!(ok.output.to_string().contains("typed 21 characters"));

        let parsed = parse_request(&args).unwrap();
        assert!(!format!("{:?}", parsed.action).contains(SECRET));
    }

    #[tokio::test]
    async fn missing_permission_is_reported_not_requested() {
        let backend = Arc::new(Fake {
            window: Err(BackendError::PermissionMissing("Accessibility".into())),
            ..Arc::try_unwrap(Fake::new()).ok().unwrap()
        });
        let r = tool(&backend, &config(&["Notes"]))
            .execute(click("Notes", 150, 150))
            .await
            .unwrap();
        let message = error_of(&r);
        assert!(message.contains("Accessibility"));
        assert!(message.contains("does not request or grant"));
    }

    #[tokio::test]
    async fn platform_backend_is_honest_and_never_acts() {
        let t = ComputerUseTool::new(platform_backend(), &config(&["Notes"])).unwrap();
        let r = t.execute(click("Notes", 150, 150)).await.unwrap();
        let message = error_of(&r);
        assert!(
            message.contains("not enabled in this build")
                || message.contains("unsupported on this OS"),
            "{message}"
        );
    }

    #[tokio::test]
    async fn malformed_dangerous_or_out_of_scope_requests_are_refused() {
        let backend = Fake::new();
        let t = tool(&backend, &config(&["Notes"]));
        for args in [
            json!({"action": "key", "app": "Notes", "key": "cmd+q"}),
            json!({"action": "key", "app": "Notes", "key": "Ctrl-Alt-Delete"}),
            json!({"action": "key", "app": "Notes", "key": "a b"}),
            json!({"action": "key", "app": "Notes", "key": "left_cmd"}),
            json!({"action": "key", "app": "Notes", "key": "lcmd"}),
            json!({"action": "key", "app": "Notes", "key": "Spotlight"}),
            json!({"action": "key", "app": "Notes", "key": "F+5"}),
            json!({"action": "key", "app": "Notes", "key": "F05"}),
            json!({"action": "key", "app": "Notes", "key": "F13"}),
            json!({"action": "type", "app": "Notes", "text": "a\u{2028}b"}),
            json!({"action": "type", "app": "Notes", "text": "a\u{FEFF}b"}),
            json!({"action": "type", "app": "Notes", "text": "a\u{200B}b"}),
            json!({"action": "type", "app": "Notes", "text": "a\u{FFFF}b"}),
            json!({"action": "type", "app": "Notes", "text": "a\u{E0001}b"}),
            json!({"action": "type", "app": "Notes", "text": "a\u{F702}"}),
            json!({"action": "type", "app": "Notes", "text": "a\u{202E}b"}),
            json!({"action": "type", "app": "Notes", "text": "x\u{1b}[2J"}),
            json!({"action": "type", "app": "Notes", "text": "a".repeat(501)}),
            json!({"action": "click", "app": "Notes", "x": 1.5, "y": 2}),
            json!({"action": "click", "x": 150, "y": 150}),
            json!({"action": "mouse_move", "app": "Notes"}),
            json!({"action": "screen_capture", "app": "Notes"}),
        ] {
            let r = t.execute(args.clone()).await.unwrap();
            assert!(!r.success, "{args}");
        }
        let shot = t
            .execute(json!({"action": "screenshot", "app": "Notes"}))
            .await
            .unwrap();
        assert!(error_of(&shot).contains("capture tool"));
        assert!(backend.calls().is_empty());
    }

    #[test]
    fn allowed_keys_and_text_pass_parsing() {
        for key in ["a", "Z", "7", "Enter", "tab", "F1", "f12", "PageDown"] {
            assert!(validate_key(key).is_ok(), "{key}");
        }
        let ok = json!({"action": "type", "app": "Notes", "text": "caf\u{e9} \u{4f60}\n\tok"});
        assert!(parse_request(&ok).is_ok());
    }

    #[test]
    fn declares_desktop_capability_and_rejects_invalid_config() {
        let t = tool(&Fake::new(), &config(&["Notes"]));
        assert_eq!(t.name(), "computer_use");
        assert_eq!(
            t.capability(),
            zeroclaw_api::policy::ToolCapability::Desktop
        );
        let backend: Arc<dyn DesktopBackend> = Fake::new();
        assert!(ComputerUseTool::new(backend, &config(&["*"])).is_err());
    }
}
