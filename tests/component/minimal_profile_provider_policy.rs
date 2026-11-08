//! HORO-2385: an agent whose provider is not an approved, configured alias must
//! fail before any network activity. A loopback recorder stands in for every
//! endpoint; the approved-provider control in the same test proves the recorder
//! sees a connection when one is made, so "zero connections" cannot pass vacuously.
#![cfg(feature = "local-agent-minimal")]

use std::net::TcpListener;
use std::process::{Command, Output, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

struct Recorder {
    port: u16,
    seen: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
}

impl Recorder {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let (seen, stop) = (
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicBool::new(false)),
        );
        let (s, st) = (Arc::clone(&seen), Arc::clone(&stop));
        std::thread::spawn(move || {
            while !st.load(Ordering::Relaxed) {
                if listener.accept().is_ok() {
                    s.fetch_add(1, Ordering::Relaxed);
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        Self { port, seen, stop }
    }

    fn connections(&self) -> usize {
        self.seen.load(Ordering::Relaxed)
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn config(agent_provider: Option<&str>, recorder_port: u16, root: &std::path::Path) -> String {
    let provider_line = agent_provider
        .map(|p| format!("model_provider = \"{p}\"\n"))
        .unwrap_or_default();
    format!(
        "schema_version = 3\n\
         [providers.models.custom.approved]\n\
         uri = \"http://127.0.0.1:{recorder_port}/v1\"\n\
         model = \"approved-model\"\n\
         wire_api = \"chat_completions\"\n\
         timeout_secs = 3\n\
         [agents.probe]\n{provider_line}\
         risk_profile = \"probe\"\nruntime_profile = \"probe\"\n\
         [agents.probe.workspace]\npath = \"{ws}\"\n\
         [agents.probe.memory]\nbackend = \"none\"\n\
         [runtime_profiles.probe]\nagentic = true\nmax_tool_iterations = 1\n\
         [risk_profiles.probe]\nlevel = \"supervised\"\nworkspace_only = true\n\
         allowed_tools = []\nauto_approve = []\n",
        ws = root.join("workspace").display()
    )
}

fn agent_command(root: &std::path::Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_zeroclaw"));
    cmd.arg("--config-dir")
        .arg(root.join("config"))
        .args(["agent", "--agent", "probe", "--message", "hello"])
        .env("HOME", root.join("home"))
        .env("LC_ALL", "C")
        .stdin(Stdio::null());
    cmd
}

fn prepare(provider: Option<&str>, recorder: &Recorder) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    for dir in ["config", "workspace", "home"] {
        std::fs::create_dir(root.path().join(dir)).unwrap();
    }
    let toml = config(provider, recorder.port, root.path());
    std::fs::write(root.path().join("config/config.toml"), toml).unwrap();
    root
}

fn run_to_exit(root: &std::path::Path) -> Output {
    agent_command(root).output().expect("spawn zeroclaw")
}

#[test]
fn approved_provider_is_reached() {
    let recorder = Recorder::start();
    let root = prepare(Some("custom.approved"), &recorder);
    let mut child = agent_command(root.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn zeroclaw");
    let deadline = Instant::now() + Duration::from_secs(120);
    while recorder.connections() == 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        recorder.connections() > 0,
        "control: the configured, approved provider must be contacted"
    );
}

#[test]
fn unresolved_or_missing_provider_makes_no_connection() {
    for provider in [Some("custom.ghost"), None] {
        let recorder = Recorder::start();
        let root = prepare(provider, &recorder);
        let output = run_to_exit(root.path());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "provider {provider:?} must be refused; stderr: {stderr}"
        );
        assert!(
            stderr.contains("model_provider does not resolve"),
            "the refusal must name the unresolved provider reference; got: {stderr}"
        );
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(
            recorder.connections(),
            0,
            "provider {provider:?}: no endpoint may be contacted"
        );
    }
}
