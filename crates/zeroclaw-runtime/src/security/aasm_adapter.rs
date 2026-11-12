//! AASM managed-mode adapter (ADR-019 D4/D6). Unix only, feature `aasm-managed`.
//!
//! Local Agent is the *client* of an AASM runtime's Unix socket; this build
//! opens no listener. Every query uses a fresh connection, so the socket checks
//! (absolute path, no symlink component, directory not group/world writable,
//! socket and peer owned by the expected uid) are re-applied each time.
//!
//! Fail closed: any error, timeout, short read, oversize frame, unknown tag or
//! a verdict other than `ALLOW` is a `Deny`. There is no fallback to
//! standalone. [`ComposedAuthority`] combines the local engine with this
//! adapter through `strictest`, so AASM can only tighten a local decision.
//!
//! Operator requirements for `governance.aasm.socket_path`:
//! - Put the socket in a `0700` directory owned by the client uid. AASM's own
//!   default, `/tmp/aa-runtime-<id>.sock`, is refused by design: `/tmp` is
//!   world-writable (`1777`) and, on macOS, a symlink.
//! - Every ancestor must be a real directory, owned by root or the client uid
//!   and not group/world writable. A sticky directory owned by root or the
//!   client uid is tolerated only as a distant ancestor, never as the
//!   immediate parent.
//! - `expected_peer_uid` may only be unset or equal to the client's own
//!   effective uid: the AASM server accepts connections from its own uid only,
//!   so any other value could never be served and is rejected at construction.
//! - The runtime's agent id must match the socket file name
//!   (`aa-runtime-<agent_id>.sock`); see the handshake note in ADR-019.
//!
//! Protocol mirrored from AASM rev `f6b0e2b70` (protocol v0.0.1), not verified
//! against a live daemon.

use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use prost::Message;
use ring::signature::{Ed25519KeyPair, KeyPair};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::UnixStream;
use zeroclaw_config::governance::{AasmConfig, GovernanceConfig};

use super::policy_contract::{
    DecisionSource, PolicyAuthority, PolicyDecision, PolicyRequest, ReasonCode, Verdict,
    reject_unsupported_version, strictest,
};

/// Local Agent accepts at most 1 MiB per frame (AASM itself accepts 8 MiB).
pub const MAX_FRAME_LEN: usize = 1024 * 1024;

const TAG_POLICY_QUERY: u8 = 1;
const TAG_HANDSHAKE_PROOF: u8 = 5;
const TAG_POLICY_RESPONSE: u8 = 1;
const TAG_HANDSHAKE_CHALLENGE: u8 = 5;

/// `ActionType::TOOL_CALL`.
const ACTION_TYPE_TOOL_CALL: i32 = 2;
/// `Decision::ALLOW`; every other value is treated as a deny.
const DECISION_ALLOW: i32 = 1;

/// Signed into the handshake proof. Fixed (not the crate version) so the
/// golden vectors in the tests do not churn on release.
const SDK_VERSION: &str = "local-agent-aasm-adapter/1";
/// Runtime socket names look like `aa-runtime-<agent_id>.sock`.
const SOCKET_PREFIX: &str = "aa-runtime-";
const SOCKET_SUFFIX: &str = ".sock";
const MAX_AUTHORITY_REF_LEN: usize = 128;

/// Prost mirror of the AASM messages this adapter needs. Source: AASM rev
/// `f6b0e2b70`, `proto/{policy,common,ipc}.proto`. Only the used field subset
/// is mirrored; fields absent here are skipped on decode.
mod wire {
    /// `assembly.common.v1.AgentId`.
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct AgentId {
        #[prost(string, tag = "1")]
        pub org_id: String,
        #[prost(string, tag = "2")]
        pub team_id: String,
        #[prost(string, tag = "3")]
        pub agent_id: String,
    }

    /// `assembly.policy.v1.CheckActionRequest`.
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct CheckActionRequest {
        #[prost(message, optional, tag = "1")]
        pub agent_id: Option<AgentId>,
        /// Always empty: the credential token file is never read (ADR-019 D4).
        #[prost(string, tag = "2")]
        pub credential_token: String,
        #[prost(string, tag = "3")]
        pub trace_id: String,
        #[prost(string, tag = "4")]
        pub span_id: String,
        #[prost(int32, tag = "5")]
        pub action_type: i32,
        #[prost(message, optional, tag = "6")]
        pub context: Option<ActionContext>,
        #[prost(message, optional, tag = "7")]
        pub caller_agent_id: Option<AgentId>,
    }

    /// `assembly.policy.v1.ActionContext`; only the `tool_call` arm (tag 2).
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct ActionContext {
        #[prost(oneof = "action_context::Action", tags = "2")]
        pub action: Option<action_context::Action>,
    }

    pub mod action_context {
        #[derive(Clone, PartialEq, prost::Oneof)]
        pub enum Action {
            #[prost(message, tag = "2")]
            ToolCall(super::ToolCallContext),
        }
    }

    /// `assembly.policy.v1.ToolCallContext`.
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct ToolCallContext {
        #[prost(string, tag = "1")]
        pub tool_name: String,
        #[prost(string, tag = "2")]
        pub tool_source: String,
        #[prost(bytes = "vec", tag = "3")]
        pub args_json: Vec<u8>,
        #[prost(string, tag = "4")]
        pub target_url: String,
    }

    /// `assembly.policy.v1.CheckActionResponse` (`redact` = 5 is not mirrored).
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct CheckActionResponse {
        #[prost(int32, tag = "1")]
        pub decision: i32,
        #[prost(string, tag = "2")]
        pub reason: String,
        #[prost(string, tag = "3")]
        pub policy_rule: String,
        #[prost(string, tag = "4")]
        pub approval_id: String,
        #[prost(int64, tag = "6")]
        pub decision_latency_us: i64,
        #[prost(int32, tag = "7")]
        pub agent_identity_assurance: i32,
        #[prost(string, tag = "8")]
        pub decision_id: String,
    }

    /// `assembly.ipc.v1.HandshakeChallenge`.
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct HandshakeChallenge {
        #[prost(bytes = "vec", tag = "1")]
        pub nonce: Vec<u8>,
    }

    /// `assembly.ipc.v1.HandshakeProof`.
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct HandshakeProof {
        #[prost(string, tag = "1")]
        pub agent_did: String,
        #[prost(string, tag = "2")]
        pub public_key: String,
        #[prost(bytes = "vec", tag = "3")]
        pub signature: Vec<u8>,
        #[prost(string, tag = "4")]
        pub sdk_version: String,
    }
}

/// Why a query could not be answered; maps to the deny reason code.
#[derive(Debug, PartialEq, Eq)]
enum WireError {
    /// Peer closed before sending a frame byte, or an I/O/connect failure.
    Unavailable,
    /// Peer closed in the middle of a frame.
    Truncated,
    Oversize,
    BadVarint,
    UnknownTag(u8),
    Decode,
    Timeout,
}

impl WireError {
    fn reason(&self) -> ReasonCode {
        match self {
            Self::Unavailable => ReasonCode::AuthorityUnavailable,
            Self::Timeout => ReasonCode::Timeout,
            Self::Truncated
            | Self::Oversize
            | Self::BadVarint
            | Self::UnknownTag(_)
            | Self::Decode => ReasonCode::MalformedResponse,
        }
    }
}

fn deny(err: &WireError) -> PolicyDecision {
    PolicyDecision::deny(err.reason(), DecisionSource::Aasm)
}

fn encode_frame(tag: u8, payload: &[u8]) -> Result<Vec<u8>, WireError> {
    if payload.len() > MAX_FRAME_LEN {
        return Err(WireError::Oversize);
    }
    let mut out = Vec::with_capacity(payload.len() + 4);
    out.push(tag);
    let mut len = payload.len() as u64;
    loop {
        let byte = (len & 0x7F) as u8;
        len >>= 7;
        if len == 0 {
            out.push(byte);
            break;
        }
        out.push(byte | 0x80);
    }
    out.extend_from_slice(payload);
    Ok(out)
}

fn eof_as(err: std::io::Error, on_eof: WireError) -> WireError {
    if err.kind() == std::io::ErrorKind::UnexpectedEof {
        on_eof
    } else {
        WireError::Unavailable
    }
}

/// Read one frame. The declared length is checked before any allocation.
async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R) -> Result<(u8, Vec<u8>), WireError> {
    let tag = reader
        .read_u8()
        .await
        .map_err(|e| eof_as(e, WireError::Unavailable))?;
    let mut len: u64 = 0;
    let mut shift = 0u32;
    loop {
        let byte = reader
            .read_u8()
            .await
            .map_err(|e| eof_as(e, WireError::Truncated))?;
        // The 10th byte carries only bit 63; a larger value overflows u64.
        if shift == 63 && byte & 0x7F > 1 {
            return Err(WireError::BadVarint);
        }
        len |= u64::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            break;
        }
        shift += 7;
        if shift >= 64 {
            return Err(WireError::BadVarint);
        }
    }
    if len > MAX_FRAME_LEN as u64 {
        return Err(WireError::Oversize);
    }
    let mut payload = vec![0u8; len as usize];
    reader
        .read_exact(&mut payload)
        .await
        .map_err(|e| eof_as(e, WireError::Truncated))?;
    Ok((tag, payload))
}

/// Ed25519 transport key. Derived from the public agent id, so it proves
/// protocol participation, not identity (AASM `AAASM-3922`): server
/// authenticity rests on the socket checks.
struct TransportKey(Ed25519KeyPair);

impl TransportKey {
    fn derive(identifier: &str) -> Result<Self, WireError> {
        let seed = Sha256::digest(identifier.as_bytes());
        Ed25519KeyPair::from_seed_unchecked(&seed)
            .map(Self)
            .map_err(|_| WireError::Unavailable)
    }

    fn proof(&self, nonce: &[u8]) -> wire::HandshakeProof {
        let mut signed = nonce.to_vec();
        signed.extend_from_slice(SDK_VERSION.as_bytes());
        let public = self.0.public_key().as_ref();
        let mut multicodec = vec![0xed, 0x01];
        multicodec.extend_from_slice(public);
        wire::HandshakeProof {
            agent_did: format!("did:key:z{}", base58(&multicodec)),
            public_key: hex::encode(public),
            signature: self.0.sign(&signed).as_ref().to_vec(),
            sdk_version: SDK_VERSION.to_string(),
        }
    }
}

fn base58(input: &[u8]) -> String {
    const ALPHABET: &[u8; 58] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    let zeros = input.iter().take_while(|b| **b == 0).count();
    let mut digits: Vec<u8> = Vec::new();
    for &byte in input {
        let mut carry = u32::from(byte);
        for digit in digits.iter_mut() {
            carry += u32::from(*digit) << 8;
            *digit = (carry % 58) as u8;
            carry /= 58;
        }
        while carry > 0 {
            digits.push((carry % 58) as u8);
            carry /= 58;
        }
    }
    let mut out = String::new();
    out.extend(std::iter::repeat_n('1', zeros));
    out.extend(
        digits
            .iter()
            .rev()
            .map(|d| ALPHABET[usize::from(*d)] as char),
    );
    out
}

/// Agent id the runtime derives its transport key from: its socket file name,
/// `aa-runtime-<agent_id>.sock` (UNVERIFIED mapping, read from AASM
/// `aa-runtime/src/ipc/server.rs`).
fn agent_identifier(socket: &Path) -> Option<String> {
    let name = socket.file_name()?.to_str()?;
    let stem = name.strip_suffix(SOCKET_SUFFIX).unwrap_or(name);
    let id = stem.strip_prefix(SOCKET_PREFIX).unwrap_or(stem);
    (!id.is_empty()).then(|| id.to_string())
}

fn build_query(request: &PolicyRequest) -> wire::CheckActionRequest {
    // The context carries a digest, never raw arguments (ADR-019 D4).
    // UNVERIFIED: AASM policy rules cannot inspect fields inside the digest.
    #[derive(serde::Serialize)]
    struct ArgsDigest<'a> {
        args_sha256: &'a str,
        capability: zeroclaw_api::policy::ToolCapability,
        session_id: &'a str,
        via_skill: Option<&'a str>,
        mcp_server: Option<&'a str>,
    }
    // A struct (not a JSON map) so field order is fixed regardless of
    // serde_json's `preserve_order` feature, keeping the golden vectors stable.
    let args_json = serde_json::to_vec(&ArgsDigest {
        args_sha256: &request.args_sha256,
        capability: request.capability,
        session_id: &request.session_id,
        via_skill: request.via_skill.as_deref(),
        mcp_server: request.mcp_server.as_deref(),
    })
    .unwrap_or_default();
    // UNVERIFIED: org/team ids are unknown to Local Agent and left empty;
    // `agent_alias` stands in for the AASM agent id.
    let agent = |alias: &str| wire::AgentId {
        org_id: String::new(),
        team_id: String::new(),
        agent_id: alias.to_string(),
    };
    wire::CheckActionRequest {
        agent_id: Some(agent(&request.agent_alias)),
        credential_token: String::new(),
        trace_id: request.turn_id.clone(),
        span_id: request.call_id.clone(),
        action_type: ACTION_TYPE_TOOL_CALL,
        context: Some(wire::ActionContext {
            action: Some(wire::action_context::Action::ToolCall(
                wire::ToolCallContext {
                    tool_name: request.tool_name.clone(),
                    tool_source: if request.mcp_server.is_some() {
                        "mcp"
                    } else {
                        "function"
                    }
                    .to_string(),
                    args_json,
                    target_url: String::new(),
                },
            )),
        }),
        caller_agent_id: request.parent_agent_alias.as_deref().map(agent),
    }
}

/// Only `ALLOW` allows. `PENDING`, `REDACT` (unsupported in v1),
/// `UNSPECIFIED` and unknown codes deny.
fn map_response(response: &wire::CheckActionResponse) -> PolicyDecision {
    let authority_ref = Some(response.decision_id.as_str())
        .filter(|id| {
            !id.is_empty()
                && id.len() <= MAX_AUTHORITY_REF_LEN
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
        })
        .map(str::to_string);
    let mut decision = if response.decision == DECISION_ALLOW {
        // `ReasonCode` has no AASM-allow code; the source field carries it.
        PolicyDecision::new(Verdict::Allow, ReasonCode::LocalAllow, DecisionSource::Aasm)
    } else {
        PolicyDecision::deny(ReasonCode::AasmDeny, DecisionSource::Aasm)
    };
    decision.authority_ref = authority_ref;
    decision
}

/// Run the handshake and one policy query over an established stream.
async fn exchange<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    key: &TransportKey,
    query: &wire::CheckActionRequest,
) -> Result<wire::CheckActionResponse, WireError> {
    let (tag, bytes) = read_frame(stream).await?;
    if tag != TAG_HANDSHAKE_CHALLENGE {
        return Err(WireError::UnknownTag(tag));
    }
    let challenge =
        wire::HandshakeChallenge::decode(bytes.as_slice()).map_err(|_| WireError::Decode)?;
    if challenge.nonce.is_empty() {
        return Err(WireError::Decode);
    }
    let proof = encode_frame(
        TAG_HANDSHAKE_PROOF,
        &key.proof(&challenge.nonce).encode_to_vec(),
    )?;
    let query = encode_frame(TAG_POLICY_QUERY, &query.encode_to_vec())?;
    for frame in [proof, query] {
        stream
            .write_all(&frame)
            .await
            .map_err(|_| WireError::Unavailable)?;
    }
    stream.flush().await.map_err(|_| WireError::Unavailable)?;

    let (tag, bytes) = read_frame(stream).await?;
    if tag != TAG_POLICY_RESPONSE {
        return Err(WireError::UnknownTag(tag));
    }
    wire::CheckActionResponse::decode(bytes.as_slice()).map_err(|_| WireError::Decode)
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AasmAdapterError {
    #[error("governance.aasm.socket_path must be an absolute path")]
    InvalidSocketPath,
    /// AASM accepts only connections from its own uid, so a client can never
    /// be served unless `expected_peer_uid` is this process's effective uid.
    #[error("governance.aasm.expected_peer_uid must equal this process's effective uid")]
    ExpectedUidMismatch,
    #[error("invalid managed governance configuration: {0}")]
    InvalidConfig(String),
}

#[derive(Debug, PartialEq, Eq)]
enum SocketCheckError {
    Invalid,
    SymlinkComponent,
    ParentNotSafe,
    NotASocket,
    WrongOwner,
    Io,
}

fn current_uid() -> u32 {
    // SAFETY: `geteuid` has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

/// Validate the socket path without following symlinks.
///
/// Every ancestor directory must be a real directory (no symlink), owned by
/// root, the current uid or the expected uid, and not group/world writable;
/// the one exception is a sticky directory owned by root or the current uid
/// (for example `/tmp` itself, `1777`), which is accepted only as a distant
/// ancestor. The immediate parent has no sticky exception. This is what stops
/// another user from swapping the socket between this check and `connect`;
/// the peer uid check after connect backs it up.
fn check_socket_path(
    path: &Path,
    expected_uid: u32,
    current_uid: u32,
) -> Result<(), SocketCheckError> {
    const STICKY: u32 = 0o1000;
    const GROUP_WORLD_WRITE: u32 = 0o022;
    if !path.is_absolute() {
        return Err(SocketCheckError::Invalid);
    }
    let mut names = Vec::new();
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => names.push(name),
            _ => return Err(SocketCheckError::Invalid),
        }
    }
    let (socket_name, ancestors) = names.split_last().ok_or(SocketCheckError::Invalid)?;
    let trusted_owner = |uid: u32| [current_uid, expected_uid, 0].contains(&uid);
    let mut cursor = PathBuf::from("/");
    let visit = |cursor: &Path, immediate_parent: bool| -> Result<(), SocketCheckError> {
        let meta = std::fs::symlink_metadata(cursor).map_err(|_| SocketCheckError::Io)?;
        if meta.file_type().is_symlink() {
            return Err(SocketCheckError::SymlinkComponent);
        }
        if !meta.is_dir() || !trusted_owner(meta.uid()) {
            return Err(SocketCheckError::ParentNotSafe);
        }
        if meta.mode() & GROUP_WORLD_WRITE != 0 {
            let sticky_ok = !immediate_parent
                && meta.mode() & STICKY != 0
                && [current_uid, 0].contains(&meta.uid());
            if !sticky_ok {
                return Err(SocketCheckError::ParentNotSafe);
            }
        }
        Ok(())
    };
    visit(&cursor, ancestors.is_empty())?;
    for (index, name) in ancestors.iter().enumerate() {
        cursor.push(name);
        visit(&cursor, index + 1 == ancestors.len())?;
    }
    cursor.push(socket_name);
    let meta = std::fs::symlink_metadata(&cursor).map_err(|_| SocketCheckError::Io)?;
    if meta.file_type().is_symlink() {
        return Err(SocketCheckError::SymlinkComponent);
    }
    if !meta.file_type().is_socket() {
        return Err(SocketCheckError::NotASocket);
    }
    if meta.uid() != expected_uid {
        return Err(SocketCheckError::WrongOwner);
    }
    Ok(())
}

/// Queries an AASM runtime. Total: every failure is a `Deny`.
#[derive(Debug)]
pub struct AasmAdapter {
    socket_path: PathBuf,
    expected_uid: u32,
    timeout: Duration,
    identifier: String,
}

impl AasmAdapter {
    pub fn new(config: &AasmConfig) -> Result<Self, AasmAdapterError> {
        let socket_path = config
            .socket_path
            .as_deref()
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .ok_or(AasmAdapterError::InvalidSocketPath)?;
        let identifier =
            agent_identifier(&socket_path).ok_or(AasmAdapterError::InvalidSocketPath)?;
        let uid = current_uid();
        if config
            .expected_peer_uid
            .is_some_and(|expected| expected != uid)
        {
            return Err(AasmAdapterError::ExpectedUidMismatch);
        }
        Ok(Self {
            socket_path,
            expected_uid: uid,
            timeout: Duration::from_millis(config.timeout_ms),
            identifier,
        })
    }

    async fn round_trip(
        &self,
        request: &PolicyRequest,
    ) -> Result<wire::CheckActionResponse, WireError> {
        let key = TransportKey::derive(&self.identifier)?;
        let query = build_query(request);
        let work = async {
            // The filesystem walk blocks, so it runs off the async threads and
            // inside the query deadline.
            let (path, expected_uid) = (self.socket_path.clone(), self.expected_uid);
            tokio::task::spawn_blocking(move || {
                check_socket_path(&path, expected_uid, current_uid())
            })
            .await
            .map_err(|_| WireError::Unavailable)?
            .map_err(|_| WireError::Unavailable)?;
            let mut stream = UnixStream::connect(&self.socket_path)
                .await
                .map_err(|_| WireError::Unavailable)?;
            let peer = stream.peer_cred().map_err(|_| WireError::Unavailable)?;
            if peer.uid() != self.expected_uid {
                return Err(WireError::Unavailable);
            }
            exchange(&mut stream, &key, &query).await
        };
        tokio::time::timeout(self.timeout, work)
            .await
            .map_err(|_| WireError::Timeout)?
    }
}

#[async_trait]
impl PolicyAuthority for AasmAdapter {
    async fn decide(&self, request: &PolicyRequest) -> PolicyDecision {
        if let Some(denied) = reject_unsupported_version(request) {
            return denied;
        }
        match self.round_trip(request).await {
            Ok(response) => map_response(&response),
            Err(err) => deny(&err),
        }
    }
}

/// Local engine composed with a second authority. The second authority can
/// only tighten: a local `Deny` is final and is not sent to it at all.
pub struct ComposedAuthority {
    local: Arc<dyn PolicyAuthority>,
    peer: Arc<dyn PolicyAuthority>,
}

impl ComposedAuthority {
    pub fn new(local: Arc<dyn PolicyAuthority>, peer: Arc<dyn PolicyAuthority>) -> Self {
        Self { local, peer }
    }
}

#[async_trait]
impl PolicyAuthority for ComposedAuthority {
    async fn decide(&self, request: &PolicyRequest) -> PolicyDecision {
        let local = self.local.decide(request).await;
        if local.verdict == Verdict::Deny {
            return local;
        }
        let peer = self.peer.decide(request).await;
        let authority_ref = peer.authority_ref.clone();
        let mut decision = strictest(local, peer);
        decision.source = DecisionSource::Composed;
        if decision.authority_ref.is_none() {
            decision.authority_ref = authority_ref;
        }
        decision
    }
}

/// Managed authority: `local` composed with the AASM adapter. Validates the
/// `[governance]` section; an unreachable socket is not an error here (each
/// call is then denied), matching ADR-019 D6.
pub fn build_managed_authority(
    config: &GovernanceConfig,
    local: Arc<dyn PolicyAuthority>,
) -> Result<Arc<dyn PolicyAuthority>, AasmAdapterError> {
    config
        .validate()
        .map_err(|e| AasmAdapterError::InvalidConfig(e.to_string()))?;
    let adapter = AasmAdapter::new(&config.aasm)?;
    Ok(Arc::new(ComposedAuthority::new(local, Arc::new(adapter))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::net::UnixListener;
    use zeroclaw_api::policy::ToolCapability;
    use zeroclaw_config::autonomy::AutonomyLevel;
    use zeroclaw_config::governance::GovernanceMode;

    use crate::security::policy_contract::{CONTRACT_VERSION, args_sha256};

    const AGENT: &str = "test-agent";
    const NONCE: [u8; 32] = [
        0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24,
        25, 26, 27, 28, 29, 30, 31,
    ];

    fn request() -> PolicyRequest {
        PolicyRequest {
            contract_version: CONTRACT_VERSION,
            session_id: "sess-1".into(),
            turn_id: "turn-1".into(),
            call_id: "call-1".into(),
            agent_alias: "main".into(),
            parent_agent_alias: Some("parent".into()),
            capability: ToolCapability::Shell,
            tool_name: "shell".into(),
            via_skill: None,
            mcp_server: None,
            args_sha256: args_sha256(&serde_json::json!({"command": "ls"})),
            autonomy: AutonomyLevel::Supervised,
            interactive: true,
        }
    }

    /// What the in-process AASM double does after the handshake.
    #[derive(Clone)]
    enum Reply {
        Decision {
            code: i32,
            decision_id: &'static str,
        },
        Hang,
        /// Raw bytes written verbatim, then the connection closes.
        Raw(Vec<u8>),
        /// Close right after reading the query.
        Close,
        /// Send a non-challenge frame first instead of the handshake.
        BadChallenge,
    }

    struct Double {
        _dir: tempfile::TempDir,
        dir: PathBuf,
        socket: PathBuf,
        queries: Arc<Mutex<Vec<wire::CheckActionRequest>>>,
        accepted: Arc<AtomicUsize>,
        proofs_valid: Arc<AtomicUsize>,
    }

    impl Double {
        fn start(reply: Reply) -> Self {
            let tmp = tempfile::tempdir().unwrap();
            // Resolve symlinked temp roots (macOS `/var`): the adapter refuses them.
            let dir = tmp.path().canonicalize().unwrap();
            let socket = dir.join(format!("{SOCKET_PREFIX}{AGENT}{SOCKET_SUFFIX}"));
            let listener = UnixListener::bind(&socket).unwrap();
            let queries = Arc::new(Mutex::new(Vec::new()));
            let accepted = Arc::new(AtomicUsize::new(0));
            let proofs_valid = Arc::new(AtomicUsize::new(0));
            let (q, a, p) = (queries.clone(), accepted.clone(), proofs_valid.clone());
            zeroclaw_spawn::spawn!(async move {
                while let Ok((mut stream, _)) = listener.accept().await {
                    a.fetch_add(1, Ordering::SeqCst);
                    let (q, p, reply) = (q.clone(), p.clone(), reply.clone());
                    zeroclaw_spawn::spawn!(async move {
                        serve(&mut stream, q, p, reply).await;
                    });
                }
            });
            Self {
                _dir: tmp,
                dir,
                socket,
                queries,
                accepted,
                proofs_valid,
            }
        }

        fn adapter(&self) -> AasmAdapter {
            self.adapter_with(|_| {})
        }

        fn adapter_with(&self, tweak: impl FnOnce(&mut AasmConfig)) -> AasmAdapter {
            let mut cfg = AasmConfig {
                socket_path: Some(self.socket.to_string_lossy().into_owned()),
                timeout_ms: 500,
                ..AasmConfig::default()
            };
            tweak(&mut cfg);
            AasmAdapter::new(&cfg).unwrap()
        }
    }

    async fn serve(
        stream: &mut UnixStream,
        queries: Arc<Mutex<Vec<wire::CheckActionRequest>>>,
        proofs_valid: Arc<AtomicUsize>,
        reply: Reply,
    ) {
        if matches!(reply, Reply::BadChallenge) {
            let _ = stream
                .write_all(&encode_frame(9, &[1, 2, 3]).unwrap())
                .await;
            return;
        }
        let challenge = wire::HandshakeChallenge {
            nonce: NONCE.to_vec(),
        };
        let frame = encode_frame(TAG_HANDSHAKE_CHALLENGE, &challenge.encode_to_vec()).unwrap();
        stream.write_all(&frame).await.unwrap();

        let (tag, bytes) = read_frame(stream).await.unwrap();
        assert_eq!(tag, TAG_HANDSHAKE_PROOF);
        let proof = wire::HandshakeProof::decode(bytes.as_slice()).unwrap();
        let mut signed = NONCE.to_vec();
        signed.extend_from_slice(proof.sdk_version.as_bytes());
        let seed = Sha256::digest(AGENT.as_bytes());
        let expected = Ed25519KeyPair::from_seed_unchecked(&seed).unwrap();
        let verified = ring::signature::UnparsedPublicKey::new(
            &ring::signature::ED25519,
            expected.public_key().as_ref(),
        )
        .verify(&signed, &proof.signature)
        .is_ok();
        if verified && proof.public_key == hex::encode(expected.public_key().as_ref()) {
            proofs_valid.fetch_add(1, Ordering::SeqCst);
        }

        let (tag, bytes) = read_frame(stream).await.unwrap();
        assert_eq!(tag, TAG_POLICY_QUERY);
        queries
            .lock()
            .unwrap()
            .push(wire::CheckActionRequest::decode(bytes.as_slice()).unwrap());

        match reply {
            Reply::Decision { code, decision_id } => {
                let response = wire::CheckActionResponse {
                    decision: code,
                    decision_id: decision_id.into(),
                    reason: "free text must not surface".into(),
                    ..Default::default()
                };
                let frame = encode_frame(TAG_POLICY_RESPONSE, &response.encode_to_vec()).unwrap();
                stream.write_all(&frame).await.unwrap();
            }
            Reply::Hang => std::future::pending::<()>().await,
            Reply::Raw(bytes) => stream.write_all(&bytes).await.unwrap(),
            Reply::Close | Reply::BadChallenge => {}
        }
    }

    fn allow() -> Reply {
        Reply::Decision {
            code: 1,
            decision_id: "01-abc",
        }
    }

    #[tokio::test]
    async fn allow_round_trip_sends_digest_and_valid_proof() {
        let double = Double::start(allow());
        let decision = double.adapter().decide(&request()).await;
        assert_eq!(decision.verdict, Verdict::Allow);
        assert_eq!(decision.source, DecisionSource::Aasm);
        assert_eq!(decision.authority_ref.as_deref(), Some("01-abc"));
        assert_eq!(double.proofs_valid.load(Ordering::SeqCst), 1);

        let queries = double.queries.lock().unwrap();
        let query = &queries[0];
        assert_eq!(query.agent_id.as_ref().unwrap().agent_id, "main");
        assert_eq!(query.caller_agent_id.as_ref().unwrap().agent_id, "parent");
        assert_eq!(query.trace_id, "turn-1");
        assert_eq!(query.span_id, "call-1");
        assert_eq!(query.action_type, ACTION_TYPE_TOOL_CALL);
        assert!(query.credential_token.is_empty());
        let Some(wire::action_context::Action::ToolCall(call)) =
            query.context.as_ref().unwrap().action.clone()
        else {
            panic!("tool_call context expected");
        };
        assert_eq!(call.tool_name, "shell");
        assert_eq!(call.tool_source, "function");
        let args: serde_json::Value = serde_json::from_slice(&call.args_json).unwrap();
        assert_eq!(args["args_sha256"], request().args_sha256);
        assert!(!String::from_utf8_lossy(&call.args_json).contains("ls"));
    }

    #[tokio::test]
    async fn deny_verdict_is_aasm_deny_without_reply_text() {
        let double = Double::start(Reply::Decision {
            code: 2,
            decision_id: "bad id with spaces",
        });
        let decision = double.adapter().decide(&request()).await;
        assert_eq!(decision.verdict, Verdict::Deny);
        assert_eq!(decision.reason, ReasonCode::AasmDeny);
        assert_eq!(decision.authority_ref, None);
    }

    #[tokio::test]
    async fn non_allow_verdicts_deny() {
        for code in [0, 3, 4, 5, -1, 99] {
            let double = Double::start(Reply::Decision {
                code,
                decision_id: "",
            });
            let decision = double.adapter().decide(&request()).await;
            assert_eq!(decision.verdict, Verdict::Deny, "code {code}");
            assert_eq!(decision.reason, ReasonCode::AasmDeny, "code {code}");
        }
    }

    async fn denied_with(reply: Reply, reason: ReasonCode) {
        let double = Double::start(reply);
        let decision = double.adapter().decide(&request()).await;
        assert_eq!(decision.verdict, Verdict::Deny);
        assert_eq!(decision.reason, reason);
    }

    #[tokio::test]
    async fn timeout_denies() {
        let double = Double::start(Reply::Hang);
        let adapter = double.adapter_with(|c| c.timeout_ms = 100);
        let decision = adapter.decide(&request()).await;
        assert_eq!(decision.verdict, Verdict::Deny);
        assert_eq!(decision.reason, ReasonCode::Timeout);
    }

    #[tokio::test]
    async fn truncated_frame_denies() {
        // tag 1, declared length 10, only 3 payload bytes.
        denied_with(
            Reply::Raw(vec![1, 10, 8, 1, 0]),
            ReasonCode::MalformedResponse,
        )
        .await;
    }

    #[tokio::test]
    async fn oversize_frame_denies_before_reading_payload() {
        let mut frame = vec![TAG_POLICY_RESPONSE];
        let mut len = (MAX_FRAME_LEN as u64) + 1;
        while len >= 0x80 {
            frame.push((len & 0x7F) as u8 | 0x80);
            len >>= 7;
        }
        frame.push(len as u8);
        denied_with(Reply::Raw(frame), ReasonCode::MalformedResponse).await;
    }

    #[tokio::test]
    async fn unknown_tag_denies() {
        denied_with(Reply::Raw(vec![7, 0]), ReasonCode::MalformedResponse).await;
    }

    #[tokio::test]
    async fn undecodable_payload_denies() {
        // Field 1 with an impossible wire type.
        denied_with(
            Reply::Raw(vec![1, 2, 0x0F, 0xFF]),
            ReasonCode::MalformedResponse,
        )
        .await;
    }

    #[tokio::test]
    async fn bad_handshake_denies() {
        denied_with(Reply::BadChallenge, ReasonCode::MalformedResponse).await;
    }

    #[tokio::test]
    async fn server_closing_mid_exchange_denies() {
        denied_with(Reply::Close, ReasonCode::AuthorityUnavailable).await;
        // Closed after the tag byte only: a truncated frame.
        denied_with(Reply::Raw(vec![1]), ReasonCode::MalformedResponse).await;
    }

    #[tokio::test]
    async fn unreachable_socket_denies_with_no_fallback() {
        let double = Double::start(allow());
        std::fs::remove_file(&double.socket).unwrap();
        let decision = double.adapter().decide(&request()).await;
        assert_eq!(decision.verdict, Verdict::Deny);
        assert_eq!(decision.reason, ReasonCode::AuthorityUnavailable);
    }

    #[tokio::test]
    async fn group_or_world_writable_directory_is_refused_before_connect() {
        use std::os::unix::fs::PermissionsExt;
        let double = Double::start(allow());
        std::fs::set_permissions(&double.dir, std::fs::Permissions::from_mode(0o770)).unwrap();
        let decision = double.adapter().decide(&request()).await;
        assert_eq!(decision.reason, ReasonCode::AuthorityUnavailable);
        assert_eq!(double.accepted.load(Ordering::SeqCst), 0);
        std::fs::set_permissions(&double.dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[test]
    fn expected_uid_other_than_own_euid_is_rejected_at_construction() {
        let cfg = AasmConfig {
            socket_path: Some("/run/u/aa-runtime-x.sock".into()),
            expected_peer_uid: Some(current_uid().wrapping_add(1)),
            ..AasmConfig::default()
        };
        assert_eq!(
            AasmAdapter::new(&cfg).unwrap_err(),
            AasmAdapterError::ExpectedUidMismatch
        );
        let own = AasmConfig {
            expected_peer_uid: Some(current_uid()),
            ..cfg
        };
        assert!(AasmAdapter::new(&own).is_ok());
    }

    #[tokio::test]
    async fn socket_owned_by_another_uid_is_refused() {
        let double = Double::start(allow());
        let other = current_uid().wrapping_add(1);
        assert_eq!(
            check_socket_path(&double.socket, other, current_uid()),
            Err(SocketCheckError::WrongOwner)
        );
    }

    fn bind_nested(base: &Path, grandparent_mode: u32, parent_mode: u32) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let gp = base.join("gp");
        let parent = gp.join("p");
        std::fs::create_dir_all(&parent).unwrap();
        let socket = parent.join("aa-runtime-x.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        std::mem::forget(listener);
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(parent_mode)).unwrap();
        std::fs::set_permissions(&gp, std::fs::Permissions::from_mode(grandparent_mode)).unwrap();
        socket
    }

    #[test]
    fn every_ancestor_is_checked_for_group_or_world_write() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let uid = current_uid();
        let ok = bind_nested(&base.join("a"), 0o755, 0o700);
        assert_eq!(check_socket_path(&ok, uid, uid), Ok(()));
        // Sticky + owned by us is accepted as a distant ancestor (like /tmp).
        let sticky = bind_nested(&base.join("b"), 0o1777, 0o700);
        assert_eq!(check_socket_path(&sticky, uid, uid), Ok(()));
        // World-writable without sticky is not.
        let open = bind_nested(&base.join("c"), 0o777, 0o700);
        assert_eq!(
            check_socket_path(&open, uid, uid),
            Err(SocketCheckError::ParentNotSafe)
        );
        let group = bind_nested(&base.join("d"), 0o775, 0o700);
        assert_eq!(
            check_socket_path(&group, uid, uid),
            Err(SocketCheckError::ParentNotSafe)
        );
        // No sticky exception for the immediate parent.
        let sticky_parent = bind_nested(&base.join("e"), 0o755, 0o1777);
        assert_eq!(
            check_socket_path(&sticky_parent, uid, uid),
            Err(SocketCheckError::ParentNotSafe)
        );
    }

    #[tokio::test]
    async fn symlinked_socket_path_is_refused_before_connect() {
        let double = Double::start(allow());
        let link = double.dir.join("link.sock");
        std::os::unix::fs::symlink(&double.socket, &link).unwrap();
        let adapter = double.adapter_with(|c| c.socket_path = Some(link.to_string_lossy().into()));
        let decision = adapter.decide(&request()).await;
        assert_eq!(decision.reason, ReasonCode::AuthorityUnavailable);
        assert_eq!(double.accepted.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn symlinked_directory_component_and_non_socket_are_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().canonicalize().unwrap();
        std::fs::create_dir(real.join("d")).unwrap();
        std::os::unix::fs::symlink(real.join("d"), real.join("ld")).unwrap();
        std::fs::write(real.join("d").join("x.sock"), b"").unwrap();
        let uid = current_uid();
        assert_eq!(
            check_socket_path(&real.join("ld").join("x.sock"), uid, uid),
            Err(SocketCheckError::SymlinkComponent)
        );
        assert_eq!(
            check_socket_path(&real.join("d").join("x.sock"), uid, uid),
            Err(SocketCheckError::NotASocket)
        );
        assert_eq!(
            check_socket_path(Path::new("relative.sock"), uid, uid),
            Err(SocketCheckError::Invalid)
        );
        assert_eq!(
            check_socket_path(
                &real.join("d").join("..").join("d").join("x.sock"),
                uid,
                uid
            ),
            Err(SocketCheckError::Invalid)
        );
    }

    #[tokio::test]
    async fn unsupported_contract_version_denies_without_connecting() {
        let double = Double::start(allow());
        let mut req = request();
        req.contract_version = CONTRACT_VERSION + 1;
        let decision = double.adapter().decide(&req).await;
        assert_eq!(decision.reason, ReasonCode::ContractVersionMismatch);
        assert_eq!(double.accepted.load(Ordering::SeqCst), 0);
    }

    struct Fixed {
        decision: PolicyDecision,
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl PolicyAuthority for Fixed {
        async fn decide(&self, _: &PolicyRequest) -> PolicyDecision {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.decision.clone()
        }
    }

    fn fixed(
        verdict: Verdict,
        source: DecisionSource,
    ) -> (Arc<dyn PolicyAuthority>, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let reason = if verdict == Verdict::Deny {
            ReasonCode::LocalDeny
        } else {
            ReasonCode::LocalAllow
        };
        let authority = Fixed {
            decision: PolicyDecision::new(verdict, reason, source),
            calls: calls.clone(),
        };
        (Arc::new(authority), calls)
    }

    #[tokio::test]
    async fn strictest_wins_and_managed_never_widens_local() {
        use Verdict::{Allow, Ask, Deny};
        for local in [Allow, Ask, Deny] {
            for peer in [Allow, Ask, Deny] {
                let (l, _) = fixed(local, DecisionSource::Local);
                let (p, peer_calls) = fixed(peer, DecisionSource::Aasm);
                let decision = ComposedAuthority::new(l, p).decide(&request()).await;
                assert_eq!(decision.verdict, local.max(peer), "{local:?}+{peer:?}");
                if local == Deny {
                    assert_eq!(decision.source, DecisionSource::Local);
                    assert_eq!(peer_calls.load(Ordering::SeqCst), 0, "peer not consulted");
                } else {
                    assert_eq!(decision.source, DecisionSource::Composed);
                }
            }
        }
    }

    #[tokio::test]
    async fn aasm_deny_of_a_locally_allowed_call_keeps_aasm_attribution() {
        let double = Double::start(Reply::Decision {
            code: 2,
            decision_id: "01-xyz",
        });
        let (local, _) = fixed(Verdict::Allow, DecisionSource::Local);
        let composed = ComposedAuthority::new(local, Arc::new(double.adapter()));
        let decision = composed.decide(&request()).await;
        assert_eq!(decision.verdict, Verdict::Deny);
        assert_eq!(decision.source, DecisionSource::Composed);
        assert_eq!(decision.reason, ReasonCode::AasmDeny);
        assert_eq!(decision.authority_ref.as_deref(), Some("01-xyz"));
    }

    #[tokio::test]
    async fn local_ask_stays_ask_when_aasm_allows() {
        let double = Double::start(allow());
        let (local, _) = fixed(Verdict::Ask, DecisionSource::Local);
        let composed = ComposedAuthority::new(local, Arc::new(double.adapter()));
        assert_eq!(composed.decide(&request()).await.verdict, Verdict::Ask);
    }

    #[tokio::test]
    async fn unreachable_aasm_denies_a_locally_allowed_call() {
        let double = Double::start(allow());
        std::fs::remove_file(&double.socket).unwrap();
        let (local, _) = fixed(Verdict::Allow, DecisionSource::Local);
        let composed = ComposedAuthority::new(local, Arc::new(double.adapter()));
        let decision = composed.decide(&request()).await;
        assert_eq!(decision.verdict, Verdict::Deny);
        assert_eq!(decision.reason, ReasonCode::AuthorityUnavailable);
    }

    #[test]
    fn build_managed_authority_validates_configuration() {
        let (local, _) = fixed(Verdict::Allow, DecisionSource::Local);
        let mut cfg = GovernanceConfig {
            mode: GovernanceMode::Managed,
            ..GovernanceConfig::default()
        };
        assert!(matches!(
            build_managed_authority(&cfg, local.clone()),
            Err(AasmAdapterError::InvalidConfig(_))
        ));
        cfg.aasm.socket_path = Some("/run/aasm/aa-runtime-x.sock".into());
        assert!(build_managed_authority(&cfg, local.clone()).is_ok());
        cfg.aasm.timeout_ms = 1;
        assert!(build_managed_authority(&cfg, local).is_err());
    }

    #[test]
    fn factory_builds_managed_with_the_feature_and_rejects_missing_socket() {
        use crate::security::authority_factory::{AuthorityBuildError, build_authority};
        let mut cfg = GovernanceConfig {
            mode: GovernanceMode::Managed,
            ..GovernanceConfig::default()
        };
        assert!(matches!(
            build_authority(&cfg),
            Err(AuthorityBuildError::ManagedConfigInvalid(_))
        ));
        cfg.aasm.socket_path = Some("/run/aasm/aa-runtime-x.sock".into());
        assert!(build_authority(&cfg).is_ok());
    }

    #[test]
    fn agent_identifier_comes_from_the_socket_file_name() {
        for (path, id) in [
            ("/r/aa-runtime-abc.sock", Some("abc")),
            ("/r/plain.sock", Some("plain")),
            ("/r/noext", Some("noext")),
            ("/r/aa-runtime-.sock", None),
        ] {
            assert_eq!(agent_identifier(Path::new(path)).as_deref(), id, "{path}");
        }
    }

    #[test]
    fn base58_matches_known_vectors() {
        assert_eq!(base58(b"Hello World!"), "2NEpo7TZRRrLZSi2U");
        assert_eq!(base58(&[0, 0, 1]), "112");
        assert_eq!(base58(&[]), "");
    }

    #[tokio::test]
    async fn codec_round_trips_and_rejects_bad_frames() {
        let payload = vec![7u8; 300];
        let frame = encode_frame(1, &payload).unwrap();
        assert_eq!(&frame[..3], &[1, 0xAC, 0x02]);
        let (tag, back) = read_frame(&mut frame.as_slice()).await.unwrap();
        assert_eq!((tag, back), (1, payload));
        assert_eq!(
            encode_frame(1, &vec![0; MAX_FRAME_LEN + 1]),
            Err(WireError::Oversize)
        );
        assert!(encode_frame(1, &vec![0; MAX_FRAME_LEN]).is_ok());
        let overlong = [
            1u8, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x01,
        ];
        assert_eq!(
            read_frame(&mut overlong.as_slice()).await,
            Err(WireError::BadVarint)
        );
        assert_eq!(
            read_frame(&mut [].as_slice()).await,
            Err(WireError::Unavailable)
        );
    }

    #[tokio::test]
    async fn tenth_varint_byte_above_one_is_malformed_via_the_decoder() {
        let frame = |last: u8| {
            let mut bytes = vec![TAG_POLICY_RESPONSE];
            bytes.extend([0xFF; 9]);
            bytes.push(last);
            bytes
        };
        assert_eq!(
            read_frame(&mut frame(0x02).as_slice()).await,
            Err(WireError::BadVarint)
        );
        assert_eq!(
            read_frame(&mut frame(0x01).as_slice()).await,
            Err(WireError::Oversize)
        );
    }

    fn golden_request() -> wire::CheckActionRequest {
        build_query(&request())
    }

    fn golden_response() -> wire::CheckActionResponse {
        wire::CheckActionResponse {
            decision: 1,
            reason: "ok".into(),
            decision_id: "01-abc".into(),
            ..Default::default()
        }
    }

    fn golden_proof() -> wire::HandshakeProof {
        TransportKey::derive(AGENT).unwrap().proof(&NONCE)
    }

    /// Framed bytes of the fixed request/response/proof used as golden
    /// vectors. Recorded in ADR-019 "Test vectors".
    #[test]
    fn golden_vectors_are_pinned() {
        let req = hex::encode(encode_frame(1, &golden_request().encode_to_vec()).unwrap());
        let resp = hex::encode(encode_frame(1, &golden_response().encode_to_vec()).unwrap());
        let proof = hex::encode(encode_frame(5, &golden_proof().encode_to_vec()).unwrap());
        assert_eq!(
            [req, resp, proof].join("\n"),
            [GOLDEN_REQUEST, GOLDEN_RESPONSE, GOLDEN_PROOF].join("\n")
        );
    }

    const GOLDEN_REQUEST: &str = "01de010a061a046d61696e1a067475726e2d31220663616c6c2d31280232b70112b4010a057368656c6c120866756e6374696f6e1aa0017b22617267735f736861323536223a2234636632393631316136363933343836326632396163666363383137653330623930356331616237336435653635383331343133656236623435346434396462222c226361706162696c697479223a227368656c6c222c2273657373696f6e5f6964223a22736573732d31222c227669615f736b696c6c223a6e756c6c2c226d63705f736572766572223a6e756c6c7d3a081a06706172656e74";
    const GOLDEN_RESPONSE: &str = "010e080112026f6b420630312d616263";
    const GOLDEN_PROOF: &str = "05da010a386469643a6b65793a7a364d6b75674466666b646478615576554a6967485332694a4a6b546b786547314e70697250596f73694d36647869701240653233313261363666303761646262656636366161616464333465323262346435306335613935336434393430656466356638383630623430626366616365351a404ef43312301d1d71d7558f5d50761b6928d2a533b0ae43575d49a79301e9efdd4dac22aac06094bd4027321cd366370f25d605645fad53ad3d0a27566d7d7408221a6c6f63616c2d6167656e742d6161736d2d616461707465722f31";

    /// The adapter must take every input from `[governance]`; nothing here may
    /// consult the process environment.
    #[test]
    fn adapter_never_reads_environment_variables() {
        let source = include_str!("aasm_adapter.rs");
        let production = source.split("#[cfg(test)]").next().unwrap();
        let forbidden = [
            ["std::", "env"].concat(),
            ["env", "::var"].concat(),
            ["env", "!("].concat(),
            ["env", "::"].concat(),
            ["var", "_os"].concat(),
            ["AASM", "_"].concat(),
            ["AA", "_AGENT"].concat(),
            ["getenv", ""].concat(),
        ];
        for needle in forbidden {
            assert!(!production.contains(&needle), "found `{needle}`");
        }
    }

    #[test]
    fn credential_token_file_is_never_read() {
        let source = include_str!("aasm_adapter.rs");
        let production = source.split("#[cfg(test)]").next().unwrap();
        assert!(!production.contains(&["credential_token", "_file"].concat()));
        assert!(!production.contains("read_to_string"));
    }
}
