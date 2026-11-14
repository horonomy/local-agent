//! Opt-in local screenshot and screen-recording lifecycle (HORO-2401).
//!
//! Captures are local-only by construction: they live under
//! `<state dir>/captures/` (never the workspace), the tool reports opaque ids
//! and sizes rather than paths, and no image is attached to a model request
//! unless the operator opts in. The recorder is a child process in its own
//! process group that is killed on stop, cancellation, error, drop and
//! session end. The platform command and the permission preflight sit behind
//! [`CaptureBackend`] so tests drive the real lifecycle with harmless
//! commands and never capture a screen.

use async_trait::async_trait;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant, SystemTime};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use zeroclaw_api::media::{MarkerKind, RenderedMarker};
use zeroclaw_api::tool::{Tool, ToolOutput, ToolResult};
use zeroclaw_config::policy::SecurityPolicy;
use zeroclaw_config::scattered_types::CaptureConfig;

/// Maximum time to wait for a one-shot screenshot command.
const SCREENSHOT_TIMEOUT: Duration = Duration::from_secs(15);
/// How long a graceful stop (SIGINT, lets the recorder finalize the movie)
/// may take before the group is killed.
const STOP_GRACE: Duration = Duration::from_secs(3);
const MAX_ID_LEN: usize = 48;
const MAX_REGION_EDGE: u32 = 16_384;

// ── Errors ──────────────────────────────────────────────────────────────

/// Failure messages never contain a filesystem path or capture content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureError {
    PermissionDenied,
    Unsupported,
    InvalidTarget(&'static str),
    InvalidId,
    AlreadyRecording,
    NotRecording,
    NotFound,
    ActiveRecording,
    Storage(String),
    Backend(String),
}

impl std::fmt::Display for CaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PermissionDenied => f.write_str(
                "Screen Recording permission is not granted to the host process. Nothing was \
                 captured. Grant it in System Settings > Privacy & Security > Screen & System \
                 Audio Recording for the host binary (consent is tied to its code signature, so \
                 a rebuilt or re-signed binary must be granted again), then restart it. The \
                 agent never prompts for or grants this permission.",
            ),
            Self::Unsupported => f.write_str("Screen capture is not supported on this platform"),
            Self::InvalidTarget(why) => write!(f, "Invalid capture target: {why}"),
            Self::InvalidId => f.write_str("Invalid capture id"),
            Self::AlreadyRecording => {
                f.write_str("A recording is already in progress; stop it before starting another")
            }
            Self::NotRecording => f.write_str("No matching recording is in progress"),
            Self::NotFound => f.write_str("No such capture"),
            Self::ActiveRecording => f.write_str("That recording is still running; stop it first"),
            Self::Storage(why) => write!(f, "Capture storage error: {why}"),
            Self::Backend(why) => write!(f, "Capture failed: {why}"),
        }
    }
}

impl std::error::Error for CaptureError {}

fn storage_error(error: &std::io::Error) -> CaptureError {
    CaptureError::Storage(format!("{:?}", error.kind()))
}

// ── Seam ────────────────────────────────────────────────────────────────

/// What to capture. A target is always required: there is no full-screen capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureTarget {
    /// Native window id (macOS `screencapture -l`).
    Window(u32),
    Region {
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    },
}

/// A platform command to spawn. The lifecycle owns the process; the backend
/// only describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureCommand {
    pub program: String,
    pub args: Vec<String>,
}

/// Platform seam: builds the capture commands and checks permission without
/// prompting. Tests substitute harmless commands.
pub trait CaptureBackend: Send + Sync {
    /// Non-prompting check that the host process may capture the screen.
    fn preflight(&self) -> Result<(), CaptureError>;
    fn screenshot_command(
        &self,
        target: &CaptureTarget,
        out: &Path,
    ) -> Result<CaptureCommand, CaptureError>;
    fn record_command(
        &self,
        target: &CaptureTarget,
        out: &Path,
        max_secs: u64,
    ) -> Result<CaptureCommand, CaptureError>;
}

/// macOS `screencapture`. The argv is covered by unit tests; it has not been run
/// against a real display in this repository's tests.
pub struct ScreencaptureBackend;

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    /// Reports Screen Recording access without prompting the user.
    fn CGPreflightScreenCaptureAccess() -> bool;
}

fn target_args(target: &CaptureTarget) -> Vec<String> {
    match target {
        CaptureTarget::Window(id) => vec!["-l".into(), id.to_string()],
        CaptureTarget::Region {
            x,
            y,
            width,
            height,
        } => vec!["-R".into(), format!("{x},{y},{width},{height}")],
    }
}

impl CaptureBackend for ScreencaptureBackend {
    fn preflight(&self) -> Result<(), CaptureError> {
        #[cfg(target_os = "macos")]
        {
            // SAFETY: argument-free query with no memory-safety preconditions;
            // unlike the Request variant it never shows a prompt.
            if unsafe { CGPreflightScreenCaptureAccess() } {
                Ok(())
            } else {
                Err(CaptureError::PermissionDenied)
            }
        }
        #[cfg(not(target_os = "macos"))]
        Err(CaptureError::Unsupported)
    }

    fn screenshot_command(
        &self,
        target: &CaptureTarget,
        out: &Path,
    ) -> Result<CaptureCommand, CaptureError> {
        let mut args = vec!["-x".to_string()];
        args.extend(target_args(target));
        args.push(out.to_string_lossy().into_owned());
        Ok(CaptureCommand {
            program: "screencapture".into(),
            args,
        })
    }

    fn record_command(
        &self,
        target: &CaptureTarget,
        out: &Path,
        max_secs: u64,
    ) -> Result<CaptureCommand, CaptureError> {
        // `-V` also bounds the recording inside the recorder itself, so a
        // killed agent cannot leave an unbounded recording behind.
        let mut args = vec![
            "-x".to_string(),
            "-v".into(),
            "-V".into(),
            max_secs.to_string(),
        ];
        args.extend(target_args(target));
        args.push(out.to_string_lossy().into_owned());
        Ok(CaptureCommand {
            program: "screencapture".into(),
            args,
        })
    }
}

// ── Storage ─────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Shot,
    Rec,
}

impl Kind {
    fn prefix(self) -> &'static str {
        match self {
            Self::Shot => "shot",
            Self::Rec => "rec",
        }
    }
    fn ext(self) -> &'static str {
        match self {
            Self::Shot => "png",
            Self::Rec => "mov",
        }
    }
    fn of_id(id: &str) -> Option<Self> {
        let valid = !id.is_empty()
            && id.len() <= MAX_ID_LEN
            && id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if !valid {
            None
        } else if id.starts_with("shot-") {
            Some(Self::Shot)
        } else if id.starts_with("rec-") {
            Some(Self::Rec)
        } else {
            None
        }
    }
}

struct Entry {
    id: String,
    path: PathBuf,
    bytes: u64,
    modified: SystemTime,
}

/// The captures directory: private (0700 dir, 0600 files), canonical, outside
/// the workspace, and addressed only through sanitized ids.
pub struct CaptureStore {
    dir: PathBuf,
}

impl CaptureStore {
    /// Create `<state_dir>/captures`, refusing symlinks, foreign owners and
    /// any location that overlaps `workspace_dir`.
    pub fn open(state_dir: &Path, workspace_dir: &Path) -> Result<Self, CaptureError> {
        if !state_dir.is_absolute()
            || state_dir
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(CaptureError::Storage(
                "state directory must be absolute without '..'".into(),
            ));
        }
        std::fs::create_dir_all(state_dir).map_err(|e| storage_error(&e))?;
        let state = std::fs::canonicalize(state_dir).map_err(|e| storage_error(&e))?;
        let dir = state.join("captures");
        if let Ok(meta) = std::fs::symlink_metadata(&dir)
            && meta.file_type().is_symlink()
        {
            return Err(CaptureError::Storage(
                "captures directory must not be a symlink".into(),
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&dir)
                .map_err(|e| storage_error(&e))?;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
                .map_err(|e| storage_error(&e))?;
        }
        #[cfg(not(unix))]
        std::fs::create_dir_all(&dir).map_err(|e| storage_error(&e))?;
        let canonical = std::fs::canonicalize(&dir).map_err(|e| storage_error(&e))?;
        if canonical.parent() != Some(state.as_path()) {
            return Err(CaptureError::Storage(
                "captures directory escapes the state directory".into(),
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let meta = std::fs::metadata(&canonical).map_err(|e| storage_error(&e))?;
            // SAFETY: geteuid has no preconditions.
            if meta.uid() != unsafe { libc::geteuid() } {
                return Err(CaptureError::Storage(
                    "captures directory is owned by another user".into(),
                ));
            }
        }
        let workspace =
            std::fs::canonicalize(workspace_dir).unwrap_or_else(|_| workspace_dir.to_path_buf());
        if canonical.starts_with(&workspace) || workspace.starts_with(&canonical) {
            return Err(CaptureError::Storage(
                "captures must not be stored inside the workspace".into(),
            ));
        }
        Ok(Self { dir: canonical })
    }

    fn path_for(&self, id: &str) -> Result<PathBuf, CaptureError> {
        let kind = Kind::of_id(id).ok_or(CaptureError::InvalidId)?;
        Ok(self.dir.join(format!("{id}.{}", kind.ext())))
    }

    fn new_id(kind: Kind) -> String {
        let stamp = chrono::Utc::now().format("%Y%m%d%H%M%S");
        let nonce = uuid::Uuid::new_v4().simple().to_string();
        format!("{}-{stamp}-{}", kind.prefix(), &nonce[..8])
    }

    /// Create the empty output file as 0600 before any process writes to it.
    fn create_private(&self, path: &Path) -> Result<(), CaptureError> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options.open(path).map(drop).map_err(|e| storage_error(&e))
    }

    fn restrict(path: &Path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        #[cfg(not(unix))]
        let _ = path;
    }

    fn entries(&self) -> Vec<Entry> {
        let Ok(read) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        read.flatten()
            .filter_map(|item| {
                let path = item.path();
                let id = path.file_stem()?.to_str()?.to_owned();
                let kind = Kind::of_id(&id)?;
                if path.extension()?.to_str()? != kind.ext() {
                    return None;
                }
                // `symlink_metadata`: a planted symlink is never followed or counted.
                let meta = std::fs::symlink_metadata(&path).ok()?;
                if !meta.file_type().is_file() {
                    return None;
                }
                Some(Entry {
                    id,
                    path,
                    bytes: meta.len(),
                    modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                })
            })
            .collect()
    }

    /// Remove captures older than `ttl`, then the oldest until the directory fits
    /// `max_total`. `keep` (an in-progress recording) is never removed.
    pub fn purge(&self, ttl: Duration, max_total: u64, keep: Option<&str>) -> usize {
        let now = SystemTime::now();
        let mut removed = 0;
        let mut live = Vec::new();
        for entry in self.entries() {
            let age = now.duration_since(entry.modified).unwrap_or_default();
            if keep != Some(entry.id.as_str()) && age >= ttl {
                if std::fs::remove_file(&entry.path).is_ok() {
                    removed += 1;
                }
            } else {
                live.push(entry);
            }
        }
        live.sort_by_key(|e| e.modified);
        let mut total: u64 = live.iter().map(|e| e.bytes).sum();
        for entry in &live {
            if total <= max_total {
                break;
            }
            if keep == Some(entry.id.as_str()) {
                continue;
            }
            if std::fs::remove_file(&entry.path).is_ok() {
                total = total.saturating_sub(entry.bytes);
                removed += 1;
            }
        }
        removed
    }

    fn delete(&self, id: &str) -> Result<(), CaptureError> {
        let path = self.path_for(id)?;
        match std::fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_file() => {
                std::fs::remove_file(&path).map_err(|e| storage_error(&e))
            }
            _ => Err(CaptureError::NotFound),
        }
    }

    fn delete_all(&self) -> usize {
        self.entries()
            .into_iter()
            .filter(|e| std::fs::remove_file(&e.path).is_ok())
            .count()
    }

    fn size_of(&self, id: &str) -> u64 {
        self.path_for(id)
            .ok()
            .and_then(|p| std::fs::symlink_metadata(p).ok())
            .map_or(0, |m| m.len())
    }
}

// ── Process ownership ───────────────────────────────────────────────────

#[cfg(unix)]
fn signal_group(pgid: i32, signal: i32) {
    if pgid > 1 {
        // SAFETY: killpg has no memory-safety preconditions; pgid > 1 keeps it
        // from signalling the caller's own group or "all processes".
        unsafe {
            libc::killpg(pgid, signal);
        }
    }
}

#[cfg(unix)]
const SIGINT: i32 = libc::SIGINT;
#[cfg(unix)]
const SIGKILL: i32 = libc::SIGKILL;
#[cfg(not(unix))]
fn signal_group(_pgid: i32, _signal: i32) {}
#[cfg(not(unix))]
const SIGINT: i32 = 2;
#[cfg(not(unix))]
const SIGKILL: i32 = 9;

/// A capture child in its own process group. Dropping it before it has been
/// reaped kills the whole group, so a cancelled or failed call cannot leave a
/// recorder behind.
struct RunningChild {
    child: Child,
    /// Zero once reaped, so a recycled pid is never signalled.
    pgid: i32,
}

impl RunningChild {
    fn spawn(command: &CaptureCommand) -> Result<Self, CaptureError> {
        let mut cmd = Command::new(&command.program);
        cmd.args(&command.args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(unix)]
        cmd.process_group(0);
        let child = cmd.spawn().map_err(|e| {
            CaptureError::Backend(format!(
                "could not start the capture command ({:?})",
                e.kind()
            ))
        })?;
        let pgid = child.id().and_then(|p| i32::try_from(p).ok()).unwrap_or(0);
        Ok(Self { child, pgid })
    }

    fn try_exited(&mut self) -> Option<std::process::ExitStatus> {
        match self.child.try_wait() {
            Ok(Some(status)) => {
                self.pgid = 0;
                Some(status)
            }
            _ => None,
        }
    }

    async fn wait(&mut self) -> Option<std::process::ExitStatus> {
        let status = self.child.wait().await.ok();
        self.pgid = 0;
        status
    }

    /// SIGINT so the recorder can finalize the movie, SIGKILL after the grace period.
    async fn finish(mut self) -> Option<std::process::ExitStatus> {
        signal_group(self.pgid, SIGINT);
        match tokio::time::timeout(STOP_GRACE, self.child.wait()).await {
            Ok(Ok(status)) => {
                // The leader is gone; sweep any helper it left in the group.
                signal_group(self.pgid, SIGKILL);
                self.pgid = 0;
                Some(status)
            }
            _ => {
                signal_group(self.pgid, SIGKILL);
                let _ = self.child.wait().await;
                self.pgid = 0;
                None
            }
        }
    }

    async fn kill(mut self) {
        signal_group(self.pgid, SIGKILL);
        let _ = self.child.wait().await;
        self.pgid = 0;
    }
}

impl Drop for RunningChild {
    fn drop(&mut self) {
        signal_group(self.pgid, SIGKILL);
    }
}

/// Removes a not-yet-finished capture file when dropped, including when the
/// owning future is cancelled.
struct PartialFile(Option<PathBuf>);

impl Drop for PartialFile {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

// ── Lifecycle ───────────────────────────────────────────────────────────

/// Lifecycle limits resolved from `[capture]`.
#[derive(Debug, Clone)]
pub struct CaptureLimits {
    pub retention_ttl: Duration,
    pub max_recording: Duration,
    pub max_total_bytes: u64,
    pub delete_on_session_end: bool,
    pub attach_images: bool,
}

impl CaptureLimits {
    pub fn from_config(config: &CaptureConfig) -> Self {
        Self {
            retention_ttl: Duration::from_secs(config.retention_ttl_secs),
            max_recording: Duration::from_secs(config.max_recording_secs),
            max_total_bytes: config.max_total_bytes,
            delete_on_session_end: config.delete_on_session_end,
            attach_images: config.attach_images_to_provider,
        }
    }
}

struct Active {
    id: String,
    running: RunningChild,
    timer: JoinHandle<()>,
    started: Instant,
}

struct Shared {
    store: CaptureStore,
    backend: Arc<dyn CaptureBackend>,
    limits: CaptureLimits,
    active: Mutex<Option<Active>>,
    /// Mirrors the live recorder's group so `Drop` can kill it without the lock.
    pgid: AtomicI32,
    last_event: std::sync::Mutex<Option<String>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StopMode {
    /// Keep the recording (SIGINT first, so the movie is finalized).
    Keep,
    /// Cancel: kill immediately and delete the partial file.
    Discard,
}

impl Shared {
    fn note(&self, text: String) {
        if let Ok(mut slot) = self.last_event.lock() {
            *slot = Some(text);
        }
    }

    fn purge(&self, keep: Option<&str>) {
        self.store
            .purge(self.limits.retention_ttl, self.limits.max_total_bytes, keep);
    }

    /// Finalize a recorder that already exited by itself (finished or crashed).
    /// Must be called with the `active` lock held.
    fn reap_exited(&self, slot: &mut Option<Active>) {
        let Some(active) = slot.as_mut() else { return };
        let Some(status) = active.running.try_exited() else {
            return;
        };
        let Some(done) = slot.take() else { return };
        done.timer.abort();
        self.pgid.store(0, Ordering::Release);
        if status.success() && self.store.size_of(&done.id) > 0 {
            self.note(format!("recording {} finished on its own", done.id));
        } else {
            let _ = self.store.delete(&done.id);
            self.note(format!(
                "recorder for {} exited unexpectedly; the partial file was removed",
                done.id
            ));
        }
    }

    /// Stop the current recording. `expected` guards the timer path against
    /// stopping a newer recording; `abort_timer` is false when the timer itself
    /// is the caller. The lock is held throughout, so a second recording can
    /// never overlap one that is shutting down.
    async fn stop(
        &self,
        expected: Option<&str>,
        mode: StopMode,
        abort_timer: bool,
        reason: &str,
    ) -> Result<(String, u64), CaptureError> {
        let mut slot = self.active.lock().await;
        self.reap_exited(&mut slot);
        let matches = slot
            .as_ref()
            .is_some_and(|a| expected.is_none_or(|id| a.id == id));
        if !matches {
            return Err(CaptureError::NotRecording);
        }
        let Some(active) = slot.take() else {
            return Err(CaptureError::NotRecording);
        };
        if abort_timer {
            active.timer.abort();
        }
        let id = active.id.clone();
        match mode {
            StopMode::Keep => {
                let status = active.running.finish().await;
                self.pgid.store(0, Ordering::Release);
                let bytes = self.store.size_of(&id);
                if bytes == 0 {
                    let _ = self.store.delete(&id);
                    self.note(format!("recording {id} produced no data and was removed"));
                    return Err(CaptureError::Backend(
                        "the recorder produced no data; check Screen Recording permission".into(),
                    ));
                }
                if let Ok(path) = self.store.path_for(&id) {
                    CaptureStore::restrict(&path);
                }
                let _ = status;
                self.note(format!("recording {id} stopped ({reason})"));
                self.purge(Some(&id));
                Ok((id, bytes))
            }
            StopMode::Discard => {
                active.running.kill().await;
                self.pgid.store(0, Ordering::Release);
                let _ = self.store.delete(&id);
                self.note(format!("recording {id} cancelled and removed"));
                Ok((id, 0))
            }
        }
    }
}

impl Drop for Shared {
    /// Session end: no orphan recorder, no partial file, and optionally no captures.
    fn drop(&mut self) {
        signal_group(self.pgid.swap(0, Ordering::AcqRel), SIGKILL);
        if let Some(active) = self.active.get_mut().take() {
            active.timer.abort();
            let _ = self.store.delete(&active.id);
        }
        if self.limits.delete_on_session_end {
            self.store.delete_all();
        }
    }
}

// ── Tool ────────────────────────────────────────────────────────────────

/// `capture`: explicit screenshot / start / stop / status / list / delete.
pub struct CaptureTool {
    security: Arc<SecurityPolicy>,
    shared: Arc<Shared>,
}

impl CaptureTool {
    pub fn new(
        security: Arc<SecurityPolicy>,
        store: CaptureStore,
        backend: Arc<dyn CaptureBackend>,
        limits: CaptureLimits,
    ) -> Self {
        let shared = Arc::new(Shared {
            store,
            backend,
            limits,
            active: Mutex::new(None),
            pgid: AtomicI32::new(0),
            last_event: std::sync::Mutex::new(None),
        });
        Self { security, shared }
    }

    /// Build from `[capture]`: captures go under `<state_dir>/captures`.
    pub fn from_config(
        security: Arc<SecurityPolicy>,
        config: &CaptureConfig,
        state_dir: &Path,
        backend: Arc<dyn CaptureBackend>,
    ) -> Result<Self, CaptureError> {
        let store = CaptureStore::open(state_dir, &security.workspace_dir)?;
        Ok(Self::new(
            security,
            store,
            backend,
            CaptureLimits::from_config(config),
        ))
    }

    fn parse_target(args: &serde_json::Value) -> Result<CaptureTarget, CaptureError> {
        let window = args.get("window_id");
        let region = args.get("region");
        match (window, region) {
            (Some(_), Some(_)) => Err(CaptureError::InvalidTarget(
                "give either window_id or region, not both",
            )),
            (None, None) => Err(CaptureError::InvalidTarget(
                "window_id or region is required; full-screen capture is not available",
            )),
            (Some(window), None) => window
                .as_u64()
                .and_then(|id| u32::try_from(id).ok())
                .filter(|id| *id > 0)
                .map(CaptureTarget::Window)
                .ok_or(CaptureError::InvalidTarget(
                    "window_id must be a positive native window id",
                )),
            (None, Some(region)) => {
                let text = region.as_str().ok_or(CaptureError::InvalidTarget(
                    "region must be the string \"x,y,width,height\"",
                ))?;
                let parts: Vec<u32> = text
                    .split(',')
                    .map(|p| p.trim().parse::<u32>())
                    .collect::<Result<_, _>>()
                    .map_err(|_| {
                        CaptureError::InvalidTarget(
                            "region must be four non-negative integers: x,y,width,height",
                        )
                    })?;
                match parts.as_slice() {
                    [x, y, width, height]
                        if *width > 0
                            && *height > 0
                            && [x, y, width, height].iter().all(|v| **v <= MAX_REGION_EDGE) =>
                    {
                        Ok(CaptureTarget::Region {
                            x: *x,
                            y: *y,
                            width: *width,
                            height: *height,
                        })
                    }
                    _ => Err(CaptureError::InvalidTarget(
                        "region needs positive width/height and edges of at most 16384",
                    )),
                }
            }
        }
    }

    async fn screenshot(&self, args: &serde_json::Value) -> Result<ToolResult, CaptureError> {
        let target = Self::parse_target(args)?;
        let shared = &self.shared;
        shared.backend.preflight()?;
        shared.purge(None);
        let id = CaptureStore::new_id(Kind::Shot);
        let path = shared.store.path_for(&id)?;
        shared.store.create_private(&path)?;
        let mut partial = PartialFile(Some(path.clone()));
        let outcome = async {
            let command = shared.backend.screenshot_command(&target, &path)?;
            let mut child = RunningChild::spawn(&command)?;
            // On timeout the guard drops here and kills the group.
            let status = tokio::time::timeout(SCREENSHOT_TIMEOUT, child.wait())
                .await
                .map_err(|_| CaptureError::Backend("the screenshot command timed out".into()))?;
            if status.is_some_and(|s| s.success()) && shared.store.size_of(&id) > 0 {
                Ok(())
            } else {
                Err(CaptureError::Backend(
                    "the screenshot command produced no image; check Screen Recording permission \
                     and that the window id exists"
                        .into(),
                ))
            }
        }
        .await;
        outcome?;
        partial.0 = None;
        CaptureStore::restrict(&path);
        shared.purge(Some(&id));
        let bytes = shared.store.size_of(&id);
        let text = format!(
            "Captured screenshot id={id} size={bytes} bytes. Stored locally in the agent state \
             directory; it is not sent anywhere. Remove it with action=delete."
        );
        let mut output = ToolOutput::text(text);
        // The marker carries the real path because the provider loader must read the
        // file. It exists only when the operator opted in via
        // `[capture] attach_images_to_provider`, which can send pixels off-device.
        if shared.limits.attach_images {
            output = output.with_attachment(RenderedMarker {
                target: path.display().to_string(),
                kind: MarkerKind::Image,
            });
        }
        Ok(ToolResult {
            success: true,
            output,
            error: None,
        })
    }

    async fn record_start(&self, args: &serde_json::Value) -> Result<ToolResult, CaptureError> {
        let target = Self::parse_target(args)?;
        let shared = &self.shared;
        let mut slot = shared.active.lock().await;
        shared.reap_exited(&mut slot);
        if slot.is_some() {
            return Err(CaptureError::AlreadyRecording);
        }
        shared.backend.preflight()?;
        shared.purge(None);
        let id = CaptureStore::new_id(Kind::Rec);
        let path = shared.store.path_for(&id)?;
        shared.store.create_private(&path)?;
        let max = shared.limits.max_recording;
        let spawned = shared
            .backend
            .record_command(&target, &path, max.as_secs().max(1))
            .and_then(|command| RunningChild::spawn(&command));
        let running = match spawned {
            Ok(running) => running,
            Err(error) => {
                let _ = shared.store.delete(&id);
                return Err(error);
            }
        };
        shared.pgid.store(running.pgid, Ordering::Release);
        let weak: Weak<Shared> = Arc::downgrade(shared);
        let timer_id = id.clone();
        let timer = tokio::spawn(async move {
            tokio::time::sleep(max).await;
            if let Some(shared) = weak.upgrade() {
                let _ = shared
                    .stop(
                        Some(&timer_id),
                        StopMode::Keep,
                        false,
                        "maximum duration reached",
                    )
                    .await;
            }
        });
        *slot = Some(Active {
            id: id.clone(),
            running,
            timer,
            started: Instant::now(),
        });
        Ok(Self::text_ok(format!(
            "Recording started id={id}. It stops automatically after at most {} s; stop it with \
             action=record_stop. Recordings stay local in the agent state directory.",
            max.as_secs()
        )))
    }

    async fn record_stop(&self, args: &serde_json::Value) -> Result<ToolResult, CaptureError> {
        let expected = match args.get("id").and_then(|v| v.as_str()) {
            Some(id) => {
                Kind::of_id(id).ok_or(CaptureError::InvalidId)?;
                Some(id)
            }
            None => None,
        };
        let discard = args
            .get("discard")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let mode = if discard {
            StopMode::Discard
        } else {
            StopMode::Keep
        };
        let (id, bytes) = self.shared.stop(expected, mode, true, "requested").await?;
        Ok(Self::text_ok(if discard {
            format!("Recording {id} cancelled; the partial file was removed.")
        } else {
            format!("Recording stopped id={id} size={bytes} bytes.")
        }))
    }

    async fn status(&self) -> Result<ToolResult, CaptureError> {
        let shared = &self.shared;
        let recording = {
            let mut slot = shared.active.lock().await;
            shared.reap_exited(&mut slot);
            slot.as_ref().map(|a| (a.id.clone(), a.started.elapsed()))
        };
        shared.purge(recording.as_ref().map(|(id, _)| id.as_str()));
        let entries = shared.store.entries();
        let total: u64 = entries.iter().map(|e| e.bytes).sum();
        let mut lines = vec![match &recording {
            Some((id, elapsed)) => format!(
                "recording: yes id={id} elapsed={}s max={}s",
                elapsed.as_secs(),
                shared.limits.max_recording.as_secs()
            ),
            None => "recording: no".to_string(),
        }];
        lines.push(format!(
            "stored: {} captures, {total} of {} bytes; retention {} s; delete on session end: {}; \
             provider image attachment: {}",
            entries.len(),
            shared.limits.max_total_bytes,
            shared.limits.retention_ttl.as_secs(),
            shared.limits.delete_on_session_end,
            shared.limits.attach_images,
        ));
        if let Ok(last) = shared.last_event.lock()
            && let Some(event) = last.as_ref()
        {
            lines.push(format!("last event: {event}"));
        }
        Ok(Self::text_ok(lines.join("\n")))
    }

    async fn list(&self) -> Result<ToolResult, CaptureError> {
        let shared = &self.shared;
        let keep = shared.active.lock().await.as_ref().map(|a| a.id.clone());
        shared.purge(keep.as_deref());
        let now = SystemTime::now();
        let mut entries = shared.store.entries();
        entries.sort_by_key(|e| e.modified);
        let lines: Vec<String> = entries
            .iter()
            .map(|e| {
                format!(
                    "{} size={} age={}s",
                    e.id,
                    e.bytes,
                    now.duration_since(e.modified).unwrap_or_default().as_secs()
                )
            })
            .collect();
        Ok(Self::text_ok(if lines.is_empty() {
            "No captures stored.".to_string()
        } else {
            lines.join("\n")
        }))
    }

    async fn delete(&self, args: &serde_json::Value) -> Result<ToolResult, CaptureError> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or(CaptureError::InvalidId)?;
        Kind::of_id(id).ok_or(CaptureError::InvalidId)?;
        if self
            .shared
            .active
            .lock()
            .await
            .as_ref()
            .is_some_and(|a| a.id == id)
        {
            return Err(CaptureError::ActiveRecording);
        }
        self.shared.store.delete(id)?;
        Ok(Self::text_ok(format!("Deleted capture {id}.")))
    }

    fn text_ok(text: String) -> ToolResult {
        ToolResult {
            success: true,
            output: ToolOutput::text(text),
            error: None,
        }
    }
}

#[async_trait]
impl Tool for CaptureTool {
    zeroclaw_api::declare_capability!(Desktop);

    fn name(&self) -> &str {
        "capture"
    }

    fn description(&self) -> &str {
        "Local screenshot and screen recording of one selected window or region. Actions: \
         screenshot, record_start, record_stop, status, list, delete. Captures stay on this \
         machine and are returned as opaque ids with sizes, never as file contents. Every \
         capture action needs operator approval. Only one recording runs at a time and each \
         has a hard maximum length."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["screenshot", "record_start", "record_stop", "status", "list", "delete"]
                },
                "window_id": {
                    "type": "integer",
                    "description": "Native window id to capture (screenshot, record_start). Give this or region."
                },
                "region": {
                    "type": "string",
                    "description": "Screen region \"x,y,width,height\" (screenshot, record_start). Give this or window_id."
                },
                "id": {
                    "type": "string",
                    "description": "Capture id (delete; optional for record_stop)."
                },
                "discard": {
                    "type": "boolean",
                    "description": "record_stop only: cancel and delete the recording instead of keeping it."
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        if !self.security.can_act() {
            return Ok(ToolResult {
                success: false,
                output: ToolOutput::default(),
                error: Some("Action blocked: autonomy is read-only".into()),
            });
        }
        let result = match args.get("action").and_then(|v| v.as_str()) {
            Some("screenshot") => self.screenshot(&args).await,
            Some("record_start") => self.record_start(&args).await,
            Some("record_stop") => self.record_stop(&args).await,
            Some("status") => self.status().await,
            Some("list") => self.list().await,
            Some("delete") => self.delete(&args).await,
            _ => Err(CaptureError::InvalidTarget(
                "action must be one of screenshot, record_start, record_stop, status, list, delete",
            )),
        };
        Ok(result.unwrap_or_else(|error| ToolResult {
            success: false,
            output: ToolOutput::default(),
            error: Some(error.to_string()),
        }))
    }
}

#[cfg(test)]
mod tests;
