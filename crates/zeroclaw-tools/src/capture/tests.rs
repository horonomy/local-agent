//! Lifecycle tests. The backend is a fake that returns harmless shell
//! commands, so the real process-group, timer and storage code runs while no
//! screen is ever captured.

// Helpers for the unix-only process-group tests are unused on other targets.
#![cfg_attr(not(unix), allow(dead_code))]

use super::*;
use std::sync::atomic::AtomicUsize;
use zeroclaw_config::autonomy::AutonomyLevel;

type Builder = Box<dyn Fn(&Path) -> CaptureCommand + Send + Sync>;

struct FakeBackend {
    preflight: Result<(), CaptureError>,
    shot: Builder,
    rec: Builder,
    shot_calls: AtomicUsize,
    rec_calls: AtomicUsize,
}

impl CaptureBackend for FakeBackend {
    fn preflight(&self) -> Result<(), CaptureError> {
        self.preflight.clone()
    }
    fn screenshot_command(
        &self,
        _target: &CaptureTarget,
        out: &Path,
    ) -> Result<CaptureCommand, CaptureError> {
        self.shot_calls.fetch_add(1, Ordering::SeqCst);
        Ok((self.shot)(out))
    }
    fn record_command(
        &self,
        _target: &CaptureTarget,
        out: &Path,
        _max_secs: u64,
    ) -> Result<CaptureCommand, CaptureError> {
        self.rec_calls.fetch_add(1, Ordering::SeqCst);
        Ok((self.rec)(out))
    }
}

fn sh(script: &str, out: &Path, extra: &[&Path]) -> CaptureCommand {
    let mut args = vec![
        "-c".to_string(),
        script.to_string(),
        "sh".to_string(),
        out.to_string_lossy().into_owned(),
    ];
    args.extend(extra.iter().map(|p| p.to_string_lossy().into_owned()));
    CaptureCommand {
        program: "sh".into(),
        args,
    }
}

fn fake(pidfile: Option<PathBuf>, rec_script: &'static str) -> Arc<FakeBackend> {
    Arc::new(FakeBackend {
        preflight: Ok(()),
        shot: Box::new(|out| sh("printf PNGDATA > \"$1\"", out, &[])),
        rec: Box::new(move |out| match &pidfile {
            Some(p) => sh(rec_script, out, &[p.as_path()]),
            None => sh(rec_script, out, &[]),
        }),
        shot_calls: AtomicUsize::new(0),
        rec_calls: AtomicUsize::new(0),
    })
}

/// Writes data, records its own pid in $2 and sleeps as a child process of
/// the group leader (a helper the group sweep must also kill).
#[cfg(unix)]
const REC_WITH_HELPER: &str = "printf MOVDATA > \"$1\"; sleep 30 & echo $! > \"$2\"; wait";
const REC_SIMPLE: &str = "printf MOVDATA > \"$1\"; echo $$ > \"$2\"; exec sleep 30";

struct Fixture {
    _root: tempfile::TempDir,
    workspace: PathBuf,
    state: PathBuf,
}

fn fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    let state = root.path().join("state");
    std::fs::create_dir_all(&workspace).unwrap();
    Fixture {
        workspace,
        state,
        _root: root,
    }
}

fn limits() -> CaptureLimits {
    CaptureLimits {
        retention_ttl: Duration::from_secs(3600),
        max_recording: Duration::from_secs(30),
        max_total_bytes: 1 << 20,
        delete_on_session_end: true,
        attach_images: false,
    }
}

fn tool_with(fx: &Fixture, backend: Arc<FakeBackend>, limits: CaptureLimits) -> CaptureTool {
    let security = Arc::new(SecurityPolicy {
        autonomy: AutonomyLevel::Full,
        workspace_dir: fx.workspace.clone(),
        ..SecurityPolicy::default()
    });
    let store = CaptureStore::open(&fx.state, &fx.workspace).unwrap();
    CaptureTool::new(security, store, backend, limits)
}

fn captures_dir(fx: &Fixture) -> PathBuf {
    std::fs::canonicalize(&fx.state).unwrap().join("captures")
}

fn count_files(dir: &Path) -> usize {
    std::fs::read_dir(dir).map_or(0, |d| d.count())
}

async fn run(tool: &CaptureTool, args: serde_json::Value) -> ToolResult {
    tool.execute(args).await.unwrap()
}

fn text(result: &ToolResult) -> String {
    result.output.as_str().to_owned()
}

fn id_from(result: &ToolResult) -> String {
    text(result)
        .split("id=")
        .nth(1)
        .and_then(|rest| rest.split(|c: char| c.is_whitespace() || c == '.').next())
        .unwrap()
        .to_owned()
}

#[cfg(unix)]
fn pid_alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks existence.
    unsafe { libc::kill(pid, 0) == 0 }
}

#[cfg(unix)]
async fn assert_gone(pidfile: &Path) {
    let pid: i32 = loop {
        if let Ok(raw) = std::fs::read_to_string(pidfile)
            && let Ok(pid) = raw.trim().parse()
        {
            break pid;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    for _ in 0..100 {
        if !pid_alive(pid) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("process {pid} is still alive: orphaned recorder");
}

#[cfg(unix)]
async fn wait_for_file(path: &Path) {
    for _ in 0..200 {
        if path.exists() && std::fs::read_to_string(path).is_ok_and(|s| !s.trim().is_empty()) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("helper never started");
}

fn with_action(action: &str) -> serde_json::Value {
    let mut v = json!({"window_id": 42});
    v["action"] = json!(action);
    v
}

// ── Output, attachments, storage location ───────────────────────────────

#[tokio::test]
async fn screenshot_reports_id_and_size_without_path_or_attachment() {
    let fx = fixture();
    let tool = tool_with(&fx, fake(None, REC_SIMPLE), limits());
    let result = run(&tool, with_action("screenshot")).await;
    assert!(result.success, "{:?}", result.error);
    assert!(result.output.attachments().is_empty());
    let out = text(&result);
    assert!(out.contains("size=7 bytes"), "{out}");
    assert!(!out.contains(fx.state.to_str().unwrap()), "{out}");
    assert!(
        !out.contains('/'),
        "no path separators in tool output: {out}"
    );
}

#[tokio::test]
async fn attachment_is_declared_only_when_the_operator_opted_in() {
    let fx = fixture();
    let on = CaptureLimits {
        attach_images: true,
        ..limits()
    };
    let tool = tool_with(&fx, fake(None, REC_SIMPLE), on);
    let result = run(&tool, with_action("screenshot")).await;
    assert!(result.success);
    assert_eq!(result.output.attachments().len(), 1);
    assert_eq!(result.output.attachments()[0].kind, MarkerKind::Image);
}

#[tokio::test]
async fn captures_land_only_under_the_captures_dir_never_the_workspace() {
    let fx = fixture();
    let tool = tool_with(&fx, fake(None, REC_SIMPLE), limits());
    assert!(run(&tool, with_action("screenshot")).await.success);
    assert_eq!(count_files(&captures_dir(&fx)), 1);
    assert_eq!(count_files(&fx.workspace), 0, "nothing in the workspace");
}

#[test]
fn store_refuses_to_live_inside_the_workspace() {
    let fx = fixture();
    let inside = fx.workspace.join("state");
    let err = CaptureStore::open(&inside, &fx.workspace).err().unwrap();
    assert!(err.to_string().contains("workspace"), "{err}");
    // And a workspace nested inside the state dir's captures is refused too.
    let err = CaptureStore::open(&fx.state, &captures_dir_of(&fx.state)).err();
    assert!(err.is_some());
}

fn captures_dir_of(state: &Path) -> PathBuf {
    std::fs::create_dir_all(state.join("captures")).unwrap();
    state.join("captures")
}

#[cfg(unix)]
#[test]
fn store_refuses_a_symlinked_captures_dir() {
    let fx = fixture();
    std::fs::create_dir_all(&fx.state).unwrap();
    let elsewhere = fx.state.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, fx.state.join("captures")).unwrap();
    let err = CaptureStore::open(&fx.state, &fx.workspace).err().unwrap();
    assert!(err.to_string().contains("symlink"), "{err}");
}

#[cfg(unix)]
#[tokio::test]
async fn directory_is_0700_and_files_are_0600() {
    use std::os::unix::fs::PermissionsExt;
    let fx = fixture();
    let tool = tool_with(&fx, fake(None, REC_SIMPLE), limits());
    assert!(run(&tool, with_action("screenshot")).await.success);
    let dir = captures_dir(&fx);
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&dir), 0o700);
    let file = std::fs::read_dir(&dir)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(mode(&file), 0o600);
}

// ── Purge, delete ───────────────────────────────────────────────────────

fn plant(dir: &Path, id: &str, ext: &str, bytes: usize, age_secs: u64) {
    let path = dir.join(format!("{id}.{ext}"));
    std::fs::write(&path, vec![b'x'; bytes]).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(age_secs))
        .unwrap();
}

#[test]
fn ttl_and_size_cap_purge_oldest_and_spare_the_active_file() {
    let fx = fixture();
    let store = CaptureStore::open(&fx.state, &fx.workspace).unwrap();
    let dir = captures_dir(&fx);
    plant(&dir, "shot-old", "png", 10, 10_000);
    plant(&dir, "shot-a", "png", 100, 300);
    plant(&dir, "shot-b", "png", 100, 200);
    plant(&dir, "rec-live", "mov", 100, 10_000);

    // TTL: the 10_000s-old shot goes; the equally old active recording stays.
    let removed = store.purge(Duration::from_secs(3600), u64::MAX, Some("rec-live"));
    assert_eq!(removed, 1);
    assert!(!dir.join("shot-old.png").exists());
    assert!(dir.join("rec-live.mov").exists());

    // Size cap 150 over 300 bytes: the protected active file is skipped, so both
    // older shots go (oldest first) and the active recording survives.
    let removed = store.purge(Duration::from_secs(3600), 150, Some("rec-live"));
    assert_eq!(removed, 2);
    assert!(!dir.join("shot-a.png").exists());
    assert!(!dir.join("shot-b.png").exists());
    assert!(
        dir.join("rec-live.mov").exists(),
        "active file is never purged"
    );
}

#[test]
fn purge_never_touches_foreign_files_or_symlinks() {
    let fx = fixture();
    let store = CaptureStore::open(&fx.state, &fx.workspace).unwrap();
    let dir = captures_dir(&fx);
    std::fs::write(dir.join("notes.txt"), b"mine").unwrap();
    let outside = fx.workspace.join("outside.png");
    std::fs::write(&outside, b"keep").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, dir.join("shot-link.png")).unwrap();
    store.purge(Duration::ZERO, 0, None);
    assert!(dir.join("notes.txt").exists());
    assert!(outside.exists(), "a symlink is never followed");
    assert_eq!(store.delete_all(), 0);
}

#[tokio::test]
async fn delete_rejects_traversal_and_foreign_names() {
    let fx = fixture();
    let tool = tool_with(&fx, fake(None, REC_SIMPLE), limits());
    let victim = fx.workspace.join("victim.png");
    std::fs::write(&victim, b"keep").unwrap();
    for bad in [
        "../workspace/victim",
        "shot-../../workspace/victim",
        "/etc/passwd",
        "shot-a/b",
        "SHOT-1",
        "shot-1.png",
        "..",
        "",
    ] {
        let result = run(&tool, json!({"action": "delete", "id": bad})).await;
        assert!(!result.success, "{bad}");
        assert_eq!(result.error.as_deref(), Some("Invalid capture id"), "{bad}");
    }
    assert!(victim.exists());
    let missing = run(&tool, json!({"action": "delete", "id": "shot-nope"})).await;
    assert_eq!(missing.error.as_deref(), Some("No such capture"));
}

#[cfg(unix)]
#[tokio::test]
async fn delete_does_not_follow_a_planted_symlink() {
    let fx = fixture();
    let tool = tool_with(&fx, fake(None, REC_SIMPLE), limits());
    let victim = fx.workspace.join("victim.png");
    std::fs::write(&victim, b"keep").unwrap();
    std::os::unix::fs::symlink(&victim, captures_dir(&fx).join("shot-link.png")).unwrap();
    let result = run(&tool, json!({"action": "delete", "id": "shot-link"})).await;
    assert!(!result.success);
    assert!(victim.exists());
}

#[tokio::test]
async fn delete_and_list_round_trip() {
    let fx = fixture();
    let tool = tool_with(&fx, fake(None, REC_SIMPLE), limits());
    let shot = run(&tool, with_action("screenshot")).await;
    let id = id_from(&shot);
    assert!(text(&run(&tool, json!({"action": "list"})).await).contains(&id));
    assert!(
        run(&tool, json!({"action": "delete", "id": id}))
            .await
            .success
    );
    assert_eq!(
        text(&run(&tool, json!({"action": "list"})).await),
        "No captures stored."
    );
}

// ── Target validation ───────────────────────────────────────────────────

#[tokio::test]
async fn a_target_is_required_and_validated_before_any_command_is_built() {
    let fx = fixture();
    let backend = fake(None, REC_SIMPLE);
    let tool = tool_with(&fx, backend.clone(), limits());
    for args in [
        json!({"action": "screenshot"}),
        json!({"action": "screenshot", "window_id": 1, "region": "0,0,1,1"}),
        json!({"action": "screenshot", "window_id": 0}),
        json!({"action": "screenshot", "region": "0,0,0,5"}),
        json!({"action": "screenshot", "region": "0,0,99999,5"}),
        json!({"action": "screenshot", "region": "a,b,c,d"}),
        json!({"action": "record_start"}),
    ] {
        assert!(!run(&tool, args.clone()).await.success, "{args}");
    }
    assert_eq!(backend.shot_calls.load(Ordering::SeqCst), 0);
    assert_eq!(backend.rec_calls.load(Ordering::SeqCst), 0);
    assert_eq!(count_files(&captures_dir(&fx)), 0);
}

#[test]
fn real_backend_argv_is_bounded_targeted_and_never_interactive() {
    let out = Path::new("/state/captures/rec-x.mov");
    let rec = ScreencaptureBackend
        .record_command(&CaptureTarget::Window(42), out, 60)
        .unwrap();
    assert_eq!(rec.program, "screencapture");
    assert_eq!(
        rec.args,
        [
            "-x",
            "-v",
            "-V",
            "60",
            "-l",
            "42",
            "/state/captures/rec-x.mov"
        ]
    );
    let region = CaptureTarget::Region {
        x: 1,
        y: 2,
        width: 3,
        height: 4,
    };
    let shot = ScreencaptureBackend
        .screenshot_command(&region, Path::new("/state/captures/shot-x.png"))
        .unwrap();
    assert_eq!(
        shot.args,
        ["-x", "-R", "1,2,3,4", "/state/captures/shot-x.png"]
    );
    for args in [&rec.args, &shot.args] {
        assert!(!args.iter().any(|a| a == "-i" || a == "-s" || a == "-w"));
    }
}

// ── Permission ──────────────────────────────────────────────────────────

#[tokio::test]
async fn permission_denial_is_actionable_and_starts_nothing() {
    let fx = fixture();
    let backend = Arc::new(FakeBackend {
        preflight: Err(CaptureError::PermissionDenied),
        shot: Box::new(|out| sh("printf X > \"$1\"", out, &[])),
        rec: Box::new(|out| sh("exec sleep 30", out, &[])),
        shot_calls: AtomicUsize::new(0),
        rec_calls: AtomicUsize::new(0),
    });
    let tool = tool_with(&fx, backend.clone(), limits());
    for action in ["screenshot", "record_start"] {
        let result = run(&tool, with_action(action)).await;
        assert!(!result.success);
        let error = result.error.unwrap();
        assert!(error.contains("Screen Recording permission"), "{error}");
        assert!(error.contains("System Settings"), "{error}");
        assert!(error.contains("code signature"), "{error}");
    }
    assert_eq!(backend.shot_calls.load(Ordering::SeqCst), 0);
    assert_eq!(backend.rec_calls.load(Ordering::SeqCst), 0);
    assert_eq!(count_files(&captures_dir(&fx)), 0);
}

// ── Recording lifecycle (real process groups, harmless commands) ────────

#[cfg(unix)]
#[tokio::test]
async fn only_one_recording_at_a_time() {
    let fx = fixture();
    let pid = fx.state.join("pid");
    let backend = fake(Some(pid.clone()), REC_SIMPLE);
    let tool = tool_with(&fx, backend.clone(), limits());
    let first = run(&tool, with_action("record_start")).await;
    assert!(first.success, "{:?}", first.error);
    let second = run(&tool, with_action("record_start")).await;
    assert!(!second.success);
    assert!(second.error.unwrap().contains("already in progress"));
    assert_eq!(backend.rec_calls.load(Ordering::SeqCst), 1);
    wait_for_file(&pid).await;
    let stop = run(&tool, json!({"action": "record_stop"})).await;
    assert!(stop.success, "{:?}", stop.error);
    assert_gone(&pid).await;
}

#[cfg(unix)]
#[tokio::test]
async fn stop_keeps_the_movie_and_reports_id_and_size() {
    let fx = fixture();
    let pid = fx.state.join("pid");
    let tool = tool_with(&fx, fake(Some(pid.clone()), REC_SIMPLE), limits());
    let started = run(&tool, with_action("record_start")).await;
    let id = id_from(&started);
    wait_for_file(&pid).await;
    let stop = run(&tool, json!({"action": "record_stop", "id": id})).await;
    assert!(stop.success, "{:?}", stop.error);
    assert!(text(&stop).contains("size=7 bytes"));
    assert!(captures_dir(&fx).join(format!("{id}.mov")).exists());
    let again = run(&tool, json!({"action": "record_stop"})).await;
    assert!(!again.success, "nothing left to stop");
}

#[cfg(unix)]
#[tokio::test]
async fn max_duration_timer_stops_the_recorder() {
    let fx = fixture();
    let pid = fx.state.join("pid");
    let short = CaptureLimits {
        max_recording: Duration::from_millis(300),
        ..limits()
    };
    let tool = tool_with(&fx, fake(Some(pid.clone()), REC_WITH_HELPER), short);
    assert!(run(&tool, with_action("record_start")).await.success);
    wait_for_file(&pid).await;
    tokio::time::sleep(Duration::from_millis(900)).await;
    let status = text(&run(&tool, json!({"action": "status"})).await);
    assert!(status.contains("recording: no"), "{status}");
    assert!(status.contains("maximum duration reached"), "{status}");
    assert_gone(&pid).await;
}

#[cfg(unix)]
#[tokio::test]
async fn cancel_kills_the_whole_group_and_removes_the_partial_file() {
    let fx = fixture();
    let pid = fx.state.join("pid");
    let tool = tool_with(&fx, fake(Some(pid.clone()), REC_WITH_HELPER), limits());
    assert!(run(&tool, with_action("record_start")).await.success);
    wait_for_file(&pid).await;
    let cancel = run(&tool, json!({"action": "record_stop", "discard": true})).await;
    assert!(cancel.success, "{:?}", cancel.error);
    assert_gone(&pid).await;
    assert_eq!(count_files(&captures_dir(&fx)), 0);
}

#[cfg(unix)]
#[tokio::test]
async fn a_crashed_recorder_is_reaped_cleaned_and_does_not_block_a_new_one() {
    let fx = fixture();
    let backend = Arc::new(FakeBackend {
        preflight: Ok(()),
        shot: Box::new(|out| sh("printf X > \"$1\"", out, &[])),
        rec: Box::new(|out| sh("printf PARTIAL > \"$1\"; exit 3", out, &[])),
        shot_calls: AtomicUsize::new(0),
        rec_calls: AtomicUsize::new(0),
    });
    let tool = tool_with(&fx, backend, limits());
    assert!(run(&tool, with_action("record_start")).await.success);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let status = text(&run(&tool, json!({"action": "status"})).await);
    assert!(status.contains("recording: no"), "{status}");
    assert!(status.contains("exited unexpectedly"), "{status}");
    assert_eq!(count_files(&captures_dir(&fx)), 0, "partial file removed");
    assert!(run(&tool, with_action("record_start")).await.success);
}

#[cfg(unix)]
#[tokio::test]
async fn dropping_the_tool_ends_the_session_without_orphans_or_captures() {
    let fx = fixture();
    let pid = fx.state.join("pid");
    let tool = tool_with(&fx, fake(Some(pid.clone()), REC_WITH_HELPER), limits());
    assert!(run(&tool, with_action("screenshot")).await.success);
    assert!(run(&tool, with_action("record_start")).await.success);
    wait_for_file(&pid).await;
    drop(tool);
    assert_gone(&pid).await;
    assert_eq!(count_files(&captures_dir(&fx)), 0, "delete_on_session_end");
}

#[cfg(unix)]
#[tokio::test]
async fn session_end_keeps_captures_when_deletion_is_not_configured() {
    let fx = fixture();
    let keep = CaptureLimits {
        delete_on_session_end: false,
        ..limits()
    };
    let tool = tool_with(&fx, fake(None, REC_SIMPLE), keep);
    assert!(run(&tool, with_action("screenshot")).await.success);
    drop(tool);
    assert_eq!(count_files(&captures_dir(&fx)), 1);
}

#[cfg(unix)]
#[tokio::test]
async fn a_cancelled_screenshot_call_kills_its_child_and_leaves_no_file() {
    let fx = fixture();
    let pid = fx.state.join("pid");
    let backend = Arc::new(FakeBackend {
        preflight: Ok(()),
        shot: {
            let pid = pid.clone();
            Box::new(move |out| sh("echo $$ > \"$2\"; exec sleep 30", out, &[pid.as_path()]))
        },
        rec: Box::new(|out| sh("exec sleep 30", out, &[])),
        shot_calls: AtomicUsize::new(0),
        rec_calls: AtomicUsize::new(0),
    });
    let tool = tool_with(&fx, backend, limits());
    let cancelled = tokio::time::timeout(
        Duration::from_millis(400),
        tool.execute(with_action("screenshot")),
    )
    .await;
    assert!(
        cancelled.is_err(),
        "the call was still running when cancelled"
    );
    assert_gone(&pid).await;
    assert_eq!(count_files(&captures_dir(&fx)), 0);
}

#[tokio::test]
async fn failed_screenshot_command_leaves_no_file_and_hints_at_permission() {
    let fx = fixture();
    let backend = Arc::new(FakeBackend {
        preflight: Ok(()),
        shot: Box::new(|out| sh("exit 1", out, &[])),
        rec: Box::new(|out| sh("exec sleep 30", out, &[])),
        shot_calls: AtomicUsize::new(0),
        rec_calls: AtomicUsize::new(0),
    });
    let tool = tool_with(&fx, backend, limits());
    let result = run(&tool, with_action("screenshot")).await;
    assert!(!result.success);
    assert!(
        result
            .error
            .unwrap()
            .contains("Screen Recording permission")
    );
    assert_eq!(count_files(&captures_dir(&fx)), 0);
}

#[tokio::test]
async fn read_only_autonomy_blocks_every_action() {
    let fx = fixture();
    let backend = fake(None, REC_SIMPLE);
    let security = Arc::new(SecurityPolicy {
        autonomy: AutonomyLevel::ReadOnly,
        workspace_dir: fx.workspace.clone(),
        ..SecurityPolicy::default()
    });
    let store = CaptureStore::open(&fx.state, &fx.workspace).unwrap();
    let tool = CaptureTool::new(security, store, backend.clone(), limits());
    let result = run(&tool, with_action("screenshot")).await;
    assert!(!result.success);
    assert_eq!(backend.shot_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn tool_declares_desktop_capability() {
    let fx = fixture();
    let tool = tool_with(&fx, fake(None, REC_SIMPLE), limits());
    assert_eq!(tool.name(), "capture");
    assert_eq!(
        tool.capability(),
        zeroclaw_api::policy::ToolCapability::Desktop
    );
}

#[cfg(unix)]
#[tokio::test]
async fn tools_rebuilt_for_the_same_dir_share_one_recording_and_survive_each_other() {
    let fx = fixture();
    let pid = fx.state.join("pid");
    let first = tool_with(&fx, fake(Some(pid.clone()), REC_SIMPLE), limits());
    assert!(run(&first, with_action("record_start")).await.success);
    wait_for_file(&pid).await;

    // A registry rebuild creates a second tool on the same captures directory.
    let second = tool_with(&fx, fake(None, REC_SIMPLE), limits());
    let overlap = run(&second, with_action("record_start")).await;
    assert!(!overlap.success, "one recording per process, not per tool");
    // Dropping the first tool must not end the recording the second still serves.
    drop(first);
    let status = text(&run(&second, json!({"action": "status"})).await);
    assert!(status.contains("recording: yes"), "{status}");
    assert!(run(&second, json!({"action": "record_stop"})).await.success);
    assert_gone(&pid).await;
}

#[cfg(unix)]
#[tokio::test]
async fn a_screenshot_never_purges_the_active_recording() {
    let fx = fixture();
    let pid = fx.state.join("pid");
    let tiny = CaptureLimits {
        max_total_bytes: 1,
        ..limits()
    };
    let tool = tool_with(&fx, fake(Some(pid.clone()), REC_SIMPLE), tiny);
    let started = run(&tool, with_action("record_start")).await;
    let id = id_from(&started);
    wait_for_file(&pid).await;
    assert!(run(&tool, with_action("screenshot")).await.success);
    assert!(captures_dir(&fx).join(format!("{id}.mov")).exists());
    assert!(run(&tool, json!({"action": "record_stop"})).await.success);
}
