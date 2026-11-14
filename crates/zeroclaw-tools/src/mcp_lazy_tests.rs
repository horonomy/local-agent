//! Synthetic, local-only coverage for on-demand MCP servers. The "server" is a
//! shell script that records every spawn (and a sleeping descendant) to a temp
//! dir; no real browser, Node or network is involved.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tempfile::TempDir;
use zeroclaw_config::schema::{McpServerConfig, McpSpawnMode};

use crate::mcp_client::McpRegistry;

const FAKE_SERVER: &str = r#"#!/bin/sh
echo "$$" >> "$REC_DIR/spawns"
sleep 300 </dev/null >/dev/null 2>&1 &
echo "$!" >> "$REC_DIR/descendants"
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  [ -z "$id" ] && continue
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}}}}\n' "$id" ;;
    *'"crash_me"'*) exit 3 ;;
    *'"hang_me"'*) sleep 300 ;;
    *'"method":"tools/call"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"ok"}]}}\n' "$id" ;;
  esac
done
"#;

struct Fixture {
    dir: TempDir,
}

impl Fixture {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("server.sh"), FAKE_SERVER).unwrap();
        Self { dir }
    }

    fn config(&self, idle_secs: u64) -> McpServerConfig {
        McpServerConfig {
            name: "srv".into(),
            command: "/bin/sh".into(),
            args: vec![self.dir.path().join("server.sh").display().to_string()],
            env: [("REC_DIR".to_string(), self.dir.path().display().to_string())].into(),
            spawn: McpSpawnMode::OnDemand,
            declared_tools: vec!["echo".into(), "crash_me".into(), "hang_me".into()],
            idle_timeout_secs: Some(idle_secs),
            ..McpServerConfig::default()
        }
    }

    fn pids(&self, file: &str) -> Vec<i32> {
        std::fs::read_to_string(self.dir.path().join(file))
            .map(|raw| raw.lines().filter_map(|l| l.trim().parse().ok()).collect())
            .unwrap_or_default()
    }

    fn spawns(&self) -> Vec<i32> {
        self.pids("spawns")
    }

    fn all_pids(&self) -> Vec<i32> {
        let mut pids = self.spawns();
        pids.extend(self.pids("descendants"));
        pids
    }
}

fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only probes existence.
    unsafe { libc::kill(pid, 0) == 0 }
}

async fn wait_all_dead(pids: &[i32], within: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        if pids.iter().all(|pid| !alive(*pid)) {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_for_spawns(fixture: &Fixture, count: usize) {
    for _ in 0..100 {
        if fixture.spawns().len() >= count {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("child never spawned");
}

async fn registry(config: McpServerConfig) -> Arc<McpRegistry> {
    Arc::new(McpRegistry::connect_all(&[config]).await.unwrap())
}

#[tokio::test(flavor = "multi_thread")]
async fn assembly_spawns_no_child_for_on_demand_servers() {
    let fixture = Fixture::new();
    let registry = registry(fixture.config(60)).await;
    let mut names = registry.tool_names();
    names.sort();
    assert_eq!(names, vec!["srv__crash_me", "srv__echo", "srv__hang_me"]);
    assert_eq!(registry.server_count(), 1);
    assert!(registry.health_check_all().is_empty());
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(fixture.spawns().is_empty(), "idle registry spawned a child");
}

#[tokio::test(flavor = "multi_thread")]
async fn first_call_spawns_exactly_one_child_and_reuses_it() {
    let fixture = Fixture::new();
    let registry = registry(fixture.config(60)).await;
    for _ in 0..3 {
        registry.call_tool("srv__echo", json!({})).await.unwrap();
    }
    assert_eq!(fixture.spawns().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_first_calls_share_one_child() {
    let fixture = Fixture::new();
    let registry = registry(fixture.config(60)).await;
    let calls: Vec<_> = (0..8)
        .map(|_| {
            let registry = Arc::clone(&registry);
            #[allow(
                clippy::disallowed_methods,
                reason = "test needs real concurrent callers; handles are awaited below"
            )]
            let handle =
                tokio::spawn(async move { registry.call_tool("srv__echo", json!({})).await });
            handle
        })
        .collect();
    for call in calls {
        call.await.unwrap().unwrap();
    }
    assert_eq!(fixture.spawns().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn idle_expiry_reaps_the_whole_tree_and_next_call_respawns() {
    let fixture = Fixture::new();
    let registry = registry(fixture.config(1)).await;
    registry.call_tool("srv__echo", json!({})).await.unwrap();
    let first = fixture.all_pids();
    assert_eq!(first.len(), 2, "child plus one descendant");
    assert!(first.iter().all(|pid| alive(*pid)));

    assert!(
        wait_all_dead(&first, Duration::from_secs(10)).await,
        "idle child tree was not reaped"
    );
    registry.call_tool("srv__echo", json!({})).await.unwrap();
    assert_eq!(fixture.spawns().len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn zero_idle_window_does_not_race_the_next_call() {
    let fixture = Fixture::new();
    let registry = registry(fixture.config(0)).await;
    // idle_timeout 0 reaps right after each call; later calls must succeed by
    // respawning, never by writing to a child the reaper is tearing down.
    for _ in 0..5 {
        registry.call_tool("srv__echo", json!({})).await.unwrap();
    }
    assert!(
        fixture.spawns().len() >= 2,
        "idle 0 should reap between calls"
    );
    assert!(
        wait_all_dead(&fixture.all_pids(), Duration::from_secs(10)).await,
        "tree left running after the last call"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn crash_then_next_call_respawns() {
    let fixture = Fixture::new();
    let registry = registry(fixture.config(60)).await;
    registry.call_tool("srv__echo", json!({})).await.unwrap();
    let first = fixture.all_pids();

    // The call errors whether or not the client replays it; only "at least one
    // respawn" is guaranteed, so do not assert an exact spawn count.
    assert!(
        registry
            .call_tool("srv__crash_me", json!({}))
            .await
            .is_err()
    );
    registry.call_tool("srv__echo", json!({})).await.unwrap();

    assert!(fixture.spawns().len() >= 2);
    assert!(
        wait_all_dead(&first, Duration::from_secs(10)).await,
        "crashed child's tree was not cleaned up"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelled_call_leaves_no_orphan() {
    let fixture = Fixture::new();
    // The short idle window is a backstop; reset-on-cancel normally reaps first.
    let registry = registry(fixture.config(3)).await;
    let call_registry = Arc::clone(&registry);
    let call =
        zeroclaw_spawn::spawn!(
            async move { call_registry.call_tool("srv__hang_me", json!({})).await }
        );
    // Cancel only once the child exists, so the cancel hits an in-flight call
    // rather than a handshake that was merely slow under load.
    wait_for_spawns(&fixture, 1).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    call.abort();
    assert!(call.await.unwrap_err().is_cancelled());
    assert!(
        wait_all_dead(&fixture.all_pids(), Duration::from_secs(15)).await,
        "cancelled call left a process behind"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn dropping_the_registry_kills_the_tree() {
    let fixture = Fixture::new();
    let registry = registry(fixture.config(60)).await;
    registry.call_tool("srv__echo", json!({})).await.unwrap();
    let pids = fixture.all_pids();
    assert!(pids.iter().all(|pid| alive(*pid)));
    drop(registry);
    assert!(
        wait_all_dead(&pids, Duration::from_secs(10)).await,
        "registry teardown left a process behind"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn undeclared_tool_is_denied_without_spawning() {
    let fixture = Fixture::new();
    let registry = registry(fixture.config(60)).await;
    let error = registry
        .call_tool("srv__not_declared", json!({}))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("unknown MCP tool"), "{error:#}");
    assert!(fixture.spawns().is_empty());
}

#[tokio::test]
async fn on_demand_rejects_configs_that_would_spawn_at_startup() {
    let fixture = Fixture::new();
    let mut pinned = fixture.config(60);
    pinned.pinned_resources = vec!["file:///x".into()];
    let mut no_tools = fixture.config(60);
    no_tools.declared_tools.clear();
    let mut http = fixture.config(60);
    http.transport = zeroclaw_config::schema::McpTransport::Http;
    for config in [pinned, no_tools, http] {
        let registry = McpRegistry::connect_all(&[config]).await.unwrap();
        assert_eq!(
            registry.server_count(),
            0,
            "invalid on_demand config skipped"
        );
    }
    assert!(fixture.spawns().is_empty());
}
