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

/// Validate the socket path without following symlinks. The directory check
/// is what stops another user from swapping the socket between this check and
/// `connect`; the peer uid check after connect backs it up.
fn check_socket_path(
    path: &Path,
    expected_uid: u32,
    current_uid: u32,
) -> Result<(), SocketCheckError> {
    if !path.is_absolute() {
        return Err(SocketCheckError::Invalid);
    }
    let mut cursor = PathBuf::from("/");
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                cursor.push(name);
                let meta = std::fs::symlink_metadata(&cursor).map_err(|_| SocketCheckError::Io)?;
                if meta.file_type().is_symlink() {
                    return Err(SocketCheckError::SymlinkComponent);
                }
            }
            _ => return Err(SocketCheckError::Invalid),
        }
    }
    let parent = path.parent().ok_or(SocketCheckError::Invalid)?;
    let parent_meta = std::fs::symlink_metadata(parent).map_err(|_| SocketCheckError::Io)?;
    let parent_owner_ok = [current_uid, expected_uid, 0].contains(&parent_meta.uid());
    if !parent_meta.is_dir() || parent_meta.mode() & 0o022 != 0 || !parent_owner_ok {
        return Err(SocketCheckError::ParentNotSafe);
    }
    let meta = std::fs::symlink_metadata(path).map_err(|_| SocketCheckError::Io)?;
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
        Ok(Self {
            socket_path,
            expected_uid: config.expected_peer_uid.unwrap_or_else(current_uid),
            timeout: Duration::from_millis(config.timeout_ms),
            identifier,
        })
    }

    async fn round_trip(
        &self,
        request: &PolicyRequest,
    ) -> Result<wire::CheckActionResponse, WireError> {
        check_socket_path(&self.socket_path, self.expected_uid, current_uid())
            .map_err(|_| WireError::Unavailable)?;
        let key = TransportKey::derive(&self.identifier)?;
        let query = build_query(request);
        let work = async {
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
