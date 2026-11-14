//! On-demand stdio transport: the MCP child is started by the first real
//! request, shared by concurrent callers, and reaped after an idle window.
//!
//! The wrapper sits under `McpServer`, so tool routing, the recovery barrier
//! and the policy gate above it are unchanged. A call the policy denies never
//! reaches `send_and_recv`, so it never starts a child.

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Weak};

use anyhow::Result;
use tokio::sync::Mutex;
use tokio::time::Duration;

use crate::mcp_client::handshake;
use crate::mcp_protocol::{JSONRPC_VERSION, JsonRpcRequest, JsonRpcResponse, MCP_PROTOCOL_VERSION};
use crate::mcp_transport::{
    McpRequestLifecycle, McpTransportError, SharedMcpTransportConn, StdioTransport,
    kill_process_group,
};
use zeroclaw_config::schema::{DEFAULT_MCP_IDLE_TIMEOUT_SECS, McpServerConfig};

#[derive(Default)]
struct LazyState {
    child: Option<Arc<StdioTransport>>,
}

struct Shared {
    config: McpServerConfig,
    idle: Duration,
    state: Mutex<LazyState>,
    /// Calls currently inside `send_and_recv`; the child is never reaped for
    /// idleness while this is non-zero.
    inflight: AtomicUsize,
    /// Bumped on every call start/finish so a pending idle timer can tell
    /// whether anything happened since it was armed.
    activity: AtomicU64,
    closed: AtomicBool,
    /// Process group of the live child (0 = none), so teardown without a
    /// graceful close can still kill the tree synchronously.
    pgid: AtomicI32,
}

impl Shared {
    fn name(&self) -> &str {
        &self.config.name
    }

    async fn retire(&self, child: Arc<StdioTransport>, reason: &'static str) {
        self.pgid.store(0, Ordering::Release);
        if let Err(error) = child.close().await {
            ::zeroclaw_log::record!(
                WARN,
                ::zeroclaw_log::Event::new(module_path!(), ::zeroclaw_log::Action::Fail)
                    .with_outcome(::zeroclaw_log::EventOutcome::Failure)
                    .with_attrs(::serde_json::json!({
                        "mcp_server": self.name(),
                        "reason": reason,
                    })),
                &format!("on-demand MCP child cleanup failed: {error:#}")
            );
        }
        ::zeroclaw_log::record!(
            INFO,
            ::zeroclaw_log::Event::new(module_path!(), ::zeroclaw_log::Action::Note).with_attrs(
                ::serde_json::json!({
                    "mcp_server": self.name(),
                    "reason": reason,
                })
            ),
            "on-demand MCP child reaped"
        );
    }

    async fn ensure_child(&self) -> Result<Arc<StdioTransport>> {
        let mut state = self.state.lock().await;
        if self.closed.load(Ordering::Acquire) {
            return Err(McpTransportError::TransportClosed.into());
        }
        if let Some(child) = state.child.clone() {
            if child.health_check() {
                return Ok(child);
            }
            state.child = None;
            self.retire(child, "crashed").await;
        }

        let child = Arc::new(StdioTransport::new_with_group(&self.config, true)?);
        if let Some(pgid) = child.process_group_id().await {
            self.pgid.store(pgid, Ordering::Release);
        }
        ::zeroclaw_log::record!(
            INFO,
            ::zeroclaw_log::Event::new(module_path!(), ::zeroclaw_log::Action::Note)
                .with_attrs(::serde_json::json!({ "mcp_server": self.name() })),
            "on-demand MCP child started"
        );
        if let Err(error) = handshake(child.as_ref(), self.name(), 0).await {
            self.retire(child, "handshake_failed").await;
            return Err(error);
        }
        state.child = Some(Arc::clone(&child));
        Ok(child)
    }

    async fn reap_if_idle(&self, armed_at: u64) {
        let mut state = self.state.lock().await;
        if self.activity.load(Ordering::Acquire) != armed_at
            || self.inflight.load(Ordering::Acquire) != 0
        {
            return;
        }
        if let Some(child) = state.child.take() {
            self.retire(child, "idle").await;
        }
    }

    async fn drop_child(&self, reason: &'static str) {
        let mut state = self.state.lock().await;
        if let Some(child) = state.child.take() {
            self.retire(child, reason).await;
        }
    }
}

/// Counts a call as in flight; arms the idle timer when the last one ends.
struct InflightGuard {
    shared: Arc<Shared>,
}

impl InflightGuard {
    fn enter(shared: &Arc<Shared>) -> Self {
        shared.inflight.fetch_add(1, Ordering::AcqRel);
        shared.activity.fetch_add(1, Ordering::AcqRel);
        Self {
            shared: Arc::clone(shared),
        }
    }
}

impl Drop for InflightGuard {
    fn drop(&mut self) {
        let shared = &self.shared;
        shared.activity.fetch_add(1, Ordering::AcqRel);
        if shared.inflight.fetch_sub(1, Ordering::AcqRel) != 1 {
            return;
        }
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let armed_at = shared.activity.load(Ordering::Acquire);
        let idle = shared.idle;
        let weak: Weak<Shared> = Arc::downgrade(shared);
        drop(zeroclaw_spawn::spawn!(async move {
            tokio::time::sleep(idle).await;
            if let Some(shared) = weak.upgrade() {
                shared.reap_if_idle(armed_at).await;
            }
        }));
    }
}

pub(crate) struct LazyStdioTransport {
    shared: Arc<Shared>,
}

impl LazyStdioTransport {
    pub(crate) fn new(config: &McpServerConfig) -> Self {
        let idle_secs = config
            .idle_timeout_secs
            .unwrap_or(DEFAULT_MCP_IDLE_TIMEOUT_SECS);
        Self {
            shared: Arc::new(Shared {
                config: config.clone(),
                idle: Duration::from_secs(idle_secs),
                state: Mutex::new(LazyState::default()),
                inflight: AtomicUsize::new(0),
                activity: AtomicU64::new(0),
                closed: AtomicBool::new(false),
                pgid: AtomicI32::new(0),
            }),
        }
    }
}

impl Drop for LazyStdioTransport {
    fn drop(&mut self) {
        self.shared.closed.store(true, Ordering::Release);
        let pgid = self.shared.pgid.swap(0, Ordering::AcqRel);
        if pgid != 0 {
            kill_process_group(pgid);
        }
    }
}

fn synthetic_response(
    id: Option<serde_json::Value>,
    result: Option<serde_json::Value>,
) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id,
        result,
        error: None,
    }
}

#[async_trait::async_trait]
impl SharedMcpTransportConn for LazyStdioTransport {
    async fn send_and_recv(
        &self,
        request: &JsonRpcRequest,
        lifecycle: &McpRequestLifecycle,
    ) -> Result<JsonRpcResponse> {
        // The caller's handshake (initial or post-recovery) is answered
        // locally; the real handshake happens when a child is actually needed.
        match request.method.as_str() {
            "initialize" => {
                return Ok(synthetic_response(
                    request.id.clone(),
                    Some(serde_json::json!({
                        "protocolVersion": MCP_PROTOCOL_VERSION,
                        "capabilities": {},
                    })),
                ));
            }
            "notifications/initialized" => return Ok(synthetic_response(None, None)),
            _ => {}
        }
        let _guard = InflightGuard::enter(&self.shared);
        let child = self.shared.ensure_child().await?;
        child.send_and_recv(request, lifecycle).await
    }

    async fn reset(&self) -> Result<()> {
        if self.shared.closed.load(Ordering::Acquire) {
            return Err(McpTransportError::TransportClosed.into());
        }
        self.shared.drop_child("reset").await;
        Ok(())
    }

    fn health_check(&self) -> bool {
        // No child is a healthy idle state; a crashed child is replaced by the
        // next call, so neither should make the daemon drop this server.
        !self.shared.closed.load(Ordering::Acquire)
    }

    async fn close(&self) -> Result<()> {
        self.shared.closed.store(true, Ordering::Release);
        self.shared.drop_child("close").await;
        Ok(())
    }
}

#[cfg(all(test, unix))]
#[path = "mcp_lazy_tests.rs"]
mod tests;
