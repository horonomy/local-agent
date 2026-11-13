use super::*;
use async_trait::async_trait;
use serde_json::json;
use zeroclaw_api::attribution::{Attributable, Role};
use zeroclaw_api::tool::ToolResult;

struct Mock {
    name: &'static str,
    cap: ToolCapability,
    server: Option<&'static str>,
    target: Option<(&'static str, ToolCapability)>,
}

impl Attributable for Mock {
    fn role(&self) -> Role {
        Role::System
    }
    fn alias(&self) -> &str {
        "mock"
    }
}

#[async_trait]
impl Tool for Mock {
    fn capability(&self) -> ToolCapability {
        self.cap
    }
    fn mcp_server(&self) -> Option<&str> {
        self.server
    }
    fn delegation_target(&self) -> Option<DelegationTarget<'_>> {
        self.target
            .map(|(name, capability)| DelegationTarget::Builtin { name, capability })
    }
    fn name(&self) -> &str {
        self.name
    }
    fn description(&self) -> &str {
        "mock"
    }
    fn parameters_schema(&self) -> serde_json::Value {
        json!({})
    }
    async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<ToolResult> {
        unreachable!("the gate never executes tools")
    }
}

fn mock(name: &'static str, cap: ToolCapability) -> Mock {
    Mock {
        name,
        cap,
        server: None,
        target: None,
    }
}

fn inputs<'a>(
    allowed: Option<&'a [String]>,
    excluded: Option<&'a [String]>,
    caller: Option<&'a [String]>,
) -> AgentPolicyInputs<'a> {
    AgentPolicyInputs {
        autonomy: AutonomyLevel::Supervised,
        allowed_tools: allowed,
        excluded_tools: excluded,
        always_ask: &[],
        mcp_servers: &[],
        caller_allowed: caller,
    }
}

fn list(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

fn gate(i: &AgentPolicyInputs<'_>) -> PolicyGate {
    PolicyGate::standalone("main", i.autonomy, profile_from_inputs(i), Vec::new())
        .with_caller_filter(i.caller_allowed)
}

async fn decide(g: &PolicyGate, tool: &Mock, args: &serde_json::Value, id: &str) -> GateOutcome {
    let registry: Vec<Box<dyn Tool>> = Vec::new();
    g.decide(&CallFacts {
        tool,
        registry: &registry,
        args,
        call_id: id,
        turn_id: "t",
        agent_alias: None,
        parent_agent_alias: None,
        interactive: true,
    })
    .await
}

async fn redeem(
    g: &PolicyGate,
    tool: &Mock,
    args: &serde_json::Value,
    id: &str,
) -> Result<(), RedeemError> {
    let registry: Vec<Box<dyn Tool>> = Vec::new();
    g.redeem(&CallFacts {
        tool,
        registry: &registry,
        args,
        call_id: id,
        turn_id: "t",
        agent_alias: None,
        parent_agent_alias: None,
        interactive: false,
    })
    .await
}

fn denied(o: GateOutcome) -> bool {
    matches!(o, GateOutcome::Deny { .. })
}

#[tokio::test]
async fn default_config_denies_network_tools_despite_auto_approve() {
    // Unrestricted allowed_tools (the default) never grants network class:
    // the deny happens before the approval prompt, so `default_auto_approve`
    // listing these tools no longer matters.
    let g = gate(&inputs(None, None, None));
    for (name, cap) in [
        ("web_fetch", ToolCapability::Network),
        ("web_search_tool", ToolCapability::Network),
        ("http_request", ToolCapability::Network),
        ("browser_open", ToolCapability::Browser),
    ] {
        assert!(
            denied(decide(&g, &mock(name, cap), &json!({}), "c").await),
            "{name}"
        );
    }
    assert!(!denied(
        decide(
            &g,
            &mock("file_read", ToolCapability::FsRead),
            &json!({}),
            "c"
        )
        .await
    ));
}

#[tokio::test]
async fn explicit_listing_is_the_operator_act_that_grants_network() {
    let allowed = list(&["web_fetch"]);
    let g = gate(&inputs(Some(&allowed), None, None));
    assert!(!denied(
        decide(
            &g,
            &mock("web_fetch", ToolCapability::Network),
            &json!({}),
            "c"
        )
        .await
    ));
    assert!(denied(
        decide(
            &g,
            &mock("http_request", ToolCapability::Network),
            &json!({}),
            "c"
        )
        .await
    ));
}

#[tokio::test]
async fn caller_list_never_grants_network_or_browser() {
    let caller = list(&["web_fetch", "browser_open"]);
    // No operator list.
    let g = gate(&inputs(None, None, Some(&caller)));
    assert!(denied(
        decide(
            &g,
            &mock("web_fetch", ToolCapability::Network),
            &json!({}),
            "c"
        )
        .await
    ));
    // Wildcard operator list.
    let star = list(&["*"]);
    let g = gate(&inputs(Some(&star), None, Some(&caller)));
    assert!(denied(
        decide(
            &g,
            &mock("browser_open", ToolCapability::Browser),
            &json!({}),
            "c"
        )
        .await
    ));
}

#[tokio::test]
async fn caller_list_narrows_but_never_widens() {
    let allowed = list(&["file_read", "shell"]);
    let caller = list(&["file_read", "file_write"]);
    let g = gate(&inputs(Some(&allowed), None, Some(&caller)));
    assert!(!denied(
        decide(
            &g,
            &mock("file_read", ToolCapability::FsRead),
            &json!({}),
            "c"
        )
        .await
    ));
    // In the operator list but removed by the caller.
    assert!(denied(
        decide(&g, &mock("shell", ToolCapability::Shell), &json!({}), "c").await
    ));
    // In the caller list only.
    assert!(denied(
        decide(
            &g,
            &mock("file_write", ToolCapability::FsWrite),
            &json!({}),
            "c"
        )
        .await
    ));
}

#[tokio::test]
async fn mcp_name_only_in_caller_list_is_denied() {
    let caller = list(&["gh__search"]);
    let allowed = list(&["file_read"]);
    let mcp = Mock {
        name: "gh__search",
        cap: ToolCapability::Mcp,
        server: Some("gh"),
        target: None,
    };
    let servers = list(&["gh"]);
    let i = AgentPolicyInputs {
        mcp_servers: &servers,
        ..inputs(Some(&allowed), None, Some(&caller))
    };
    assert!(denied(
        decide(&gate(&i), mock_mcp(&mcp), &json!({}), "c").await
    ));
    // Listed by the operator and the caller: allowed.
    let both = list(&["file_read", "gh__search"]);
    let i = AgentPolicyInputs {
        mcp_servers: &servers,
        ..inputs(Some(&both), None, Some(&caller))
    };
    assert!(!denied(
        decide(&gate(&i), mock_mcp(&mcp), &json!({}), "c").await
    ));
}

fn mock_mcp(m: &Mock) -> &Mock {
    m
}

#[tokio::test]
async fn skill_shell_wrapper_follows_an_excluded_shell() {
    let wrapper = Mock {
        name: "my-skill__run",
        cap: ToolCapability::Shell,
        server: None,
        target: Some(("shell", ToolCapability::Shell)),
    };
    assert!(!denied(
        decide(&gate(&inputs(None, None, None)), &wrapper, &json!({}), "c").await
    ));
    let excluded = list(&["shell"]);
    assert!(denied(
        decide(
            &gate(&inputs(None, Some(&excluded), None)),
            &wrapper,
            &json!({}),
            "c"
        )
        .await
    ));
}

#[tokio::test]
async fn composed_non_mcp_names_and_unclassified_tools_are_denied() {
    let g = gate(&inputs(None, None, None));
    assert!(denied(
        decide(&g, &mock("a__b", ToolCapability::FsRead), &json!({}), "c").await
    ));
    assert!(denied(
        decide(
            &g,
            &mock("mystery", ToolCapability::Unknown),
            &json!({}),
            "c"
        )
        .await
    ));
    assert!(denied(
        decide(
            &g,
            &mock("caf\u{e9}", ToolCapability::FsRead),
            &json!({}),
            "c"
        )
        .await
    ));
}

#[tokio::test]
async fn names_fold_so_mixed_case_tools_and_lists_agree() {
    let allowed = list(&["TodoWrite"]);
    let g = gate(&inputs(Some(&allowed), None, None));
    assert!(!denied(
        decide(
            &g,
            &mock("TodoWrite", ToolCapability::AgentInternal),
            &json!({}),
            "c"
        )
        .await
    ));
    let excluded = list(&["TODOWRITE"]);
    let g = gate(&inputs(None, Some(&excluded), None));
    assert!(denied(
        decide(
            &g,
            &mock("TodoWrite", ToolCapability::AgentInternal),
            &json!({}),
            "c"
        )
        .await
    ));
}

#[tokio::test]
async fn names_that_collide_after_folding_are_denied() {
    let g = gate(&inputs(None, None, None));
    let registry: Vec<Box<dyn Tool>> = vec![
        Box::new(mock("Notes", ToolCapability::FsRead)),
        Box::new(mock("notes", ToolCapability::FsRead)),
    ];
    let tool = mock("notes", ToolCapability::FsRead);
    let out = g
        .decide(&CallFacts {
            tool: &tool,
            registry: &registry,
            args: &json!({}),
            call_id: "c",
            turn_id: "t",
            agent_alias: None,
            parent_agent_alias: None,
            interactive: true,
        })
        .await;
    assert!(denied(out));
}

#[tokio::test]
async fn ticket_binds_exact_tool_and_arguments_and_is_single_use() {
    let g = gate(&inputs(None, None, None));
    let read = mock("file_read", ToolCapability::FsRead);
    let args = json!({"path": "a"});
    assert!(!denied(decide(&g, &read, &args, "c1").await));
    // A hook-rewritten or swapped call cannot redeem it.
    let other = mock("file_write", ToolCapability::FsWrite);
    assert!(redeem(&g, &other, &args, "c1").await.is_err());
    assert!(
        redeem(&g, &read, &json!({"path": "b"}), "c1")
            .await
            .is_err()
    );
    assert!(redeem(&g, &read, &args, "other-id").await.is_err());
    // The runtime-injected `approved` key does not change the digest.
    assert!(
        redeem(&g, &read, &json!({"path": "a", "approved": true}), "c1")
            .await
            .is_ok()
    );
    // Single use.
    assert!(redeem(&g, &read, &args, "c1").await.is_err());
}

#[tokio::test]
async fn no_ticket_means_no_execution() {
    let g = gate(&inputs(None, None, None));
    let read = mock("file_read", ToolCapability::FsRead);
    assert_eq!(
        redeem(&g, &read, &json!({}), "never-decided").await,
        Err(RedeemError::NoTicket)
    );
}

#[tokio::test]
async fn ask_ticket_needs_approval_to_redeem() {
    let ask = list(&["shell"]);
    let i = AgentPolicyInputs {
        always_ask: &ask,
        ..inputs(None, None, None)
    };
    let g = gate(&i);
    let shell = mock("shell", ToolCapability::Shell);
    let args = json!({"command": "ls"});
    assert_eq!(
        decide(&g, &shell, &args, "c").await,
        GateOutcome::Proceed { ask: true }
    );
    assert!(redeem(&g, &shell, &args, "c").await.is_err());
    g.approve("c", "shell", &args);
    assert!(redeem(&g, &shell, &args, "c").await.is_ok());
}

#[tokio::test]
async fn deferred_call_is_decided_at_execution_and_only_allow_passes() {
    let g = gate(&inputs(None, None, None));
    let read = mock("file_read", ToolCapability::FsRead);
    g.note_unresolved("c", &json!({}));
    assert!(redeem(&g, &read, &json!({}), "c").await.is_ok());
    let net = mock("web_fetch", ToolCapability::Network);
    g.note_unresolved("d", &json!({}));
    assert!(matches!(
        redeem(&g, &net, &json!({}), "d").await,
        Err(RedeemError::Denied(_))
    ));
}

#[tokio::test]
async fn subagent_cannot_escalate_past_its_parent() {
    let parent_allowed = list(&["file_read"]);
    let parent = profile_from_inputs(&inputs(Some(&parent_allowed), None, None));
    let child = profile_from_inputs(&inputs(None, None, None));
    let g = PolicyGate::standalone(
        "child",
        AutonomyLevel::Supervised,
        child,
        vec![("parent".into(), parent)],
    );
    let registry: Vec<Box<dyn Tool>> = Vec::new();
    let shell = mock("shell", ToolCapability::Shell);
    let facts = CallFacts {
        tool: &shell,
        registry: &registry,
        args: &json!({}),
        call_id: "c",
        turn_id: "t",
        agent_alias: None,
        parent_agent_alias: Some("parent"),
        interactive: true,
    };
    assert!(denied(g.decide(&facts).await));
    let unknown_parent = CallFacts {
        parent_agent_alias: Some("ghost"),
        ..facts
    };
    assert!(denied(g.decide(&unknown_parent).await));
}

#[tokio::test]
async fn managed_mode_fails_closed() {
    let gov = GovernanceConfig {
        mode: GovernanceMode::Managed,
        ..GovernanceConfig::default()
    };
    let g = PolicyGate::for_governance(&gov, "main", &inputs(None, None, None));
    let read = mock("file_read", ToolCapability::FsRead);
    assert_eq!(
        decide(&g, &read, &json!({}), "c").await,
        GateOutcome::Deny {
            reason: ReasonCode::AuthorityUnavailable
        }
    );
}

#[test]
fn fold_rule() {
    assert_eq!(fold_name("  TodoWrite ").as_deref(), Some("todowrite"));
    assert_eq!(fold_name(""), None);
    assert_eq!(fold_name("a\u{e9}"), None);
    assert_eq!(fold_name("a\nb"), None);
}

#[tokio::test]
async fn deferred_call_that_decides_ask_is_denied_not_allowed() {
    let ask = list(&["shell"]);
    let i = AgentPolicyInputs {
        always_ask: &ask,
        ..inputs(None, None, None)
    };
    let g = gate(&i);
    let shell = mock("shell", ToolCapability::Shell);
    g.note_unresolved("c", &json!({}));
    assert!(matches!(
        redeem(&g, &shell, &json!({}), "c").await,
        Err(RedeemError::Denied(_))
    ));
    // The marker is consumed: a second attempt has nothing to redeem.
    assert_eq!(
        redeem(&g, &shell, &json!({}), "c").await,
        Err(RedeemError::NoTicket)
    );
}

fn bounded(child: &AgentPolicyInputs<'_>, parent: &AgentPolicyInputs<'_>) -> PolicyGate {
    PolicyGate::bounded_child(
        &GovernanceConfig::default(),
        "child",
        child,
        "parent",
        parent,
    )
}

#[tokio::test]
async fn bounded_child_is_narrowed_by_its_parent_without_a_per_call_parent() {
    let parent_excluded = list(&["shell"]);
    let g = bounded(
        &inputs(None, None, None),
        &inputs(None, Some(&parent_excluded), None),
    );
    // The child's own profile allows shell; the parent's exclusion wins.
    assert!(denied(
        decide(&g, &mock("shell", ToolCapability::Shell), &json!({}), "a").await
    ));
    assert!(!denied(
        decide(
            &g,
            &mock("file_read", ToolCapability::FsRead),
            &json!({}),
            "b"
        )
        .await
    ));
}

#[tokio::test]
async fn bounded_child_cannot_exceed_the_parent_autonomy() {
    let child = AgentPolicyInputs {
        autonomy: AutonomyLevel::Full,
        ..inputs(None, None, None)
    };
    let g = bounded(&child, &inputs(None, None, None));
    assert!(denied(
        decide(
            &g,
            &mock("file_read", ToolCapability::FsRead),
            &json!({}),
            "a"
        )
        .await
    ));
    // A read-only parent makes shell unreachable for a Supervised child.
    let read_only = AgentPolicyInputs {
        autonomy: AutonomyLevel::ReadOnly,
        ..inputs(None, None, None)
    };
    let g = bounded(&inputs(None, None, None), &read_only);
    assert!(denied(
        decide(&g, &mock("shell", ToolCapability::Shell), &json!({}), "b").await
    ));
}

#[tokio::test]
async fn bounded_child_cannot_widen_network_the_parent_never_listed() {
    let child_allowed = list(&["web_fetch"]);
    let g = bounded(
        &inputs(Some(&child_allowed), None, None),
        &inputs(None, None, None),
    );
    assert!(denied(
        decide(
            &g,
            &mock("web_fetch", ToolCapability::Network),
            &json!({}),
            "a"
        )
        .await
    ));
}

/// A generic MCP tool (resources/prompts): the server comes from arguments.
struct ArgsMcp;

impl Attributable for ArgsMcp {
    fn role(&self) -> Role {
        Role::System
    }
    fn alias(&self) -> &str {
        "args-mcp"
    }
}

#[async_trait]
impl Tool for ArgsMcp {
    fn capability(&self) -> ToolCapability {
        ToolCapability::Mcp
    }
    fn mcp_servers_for_call(&self, args: &serde_json::Value) -> Option<Vec<String>> {
        Some(
            args.get("server")
                .and_then(|v| v.as_str())
                .map(|s| vec![s.to_owned()])
                .unwrap_or_default(),
        )
    }
    fn name(&self) -> &str {
        "mcp_resources"
    }
    fn description(&self) -> &str {
        "mock"
    }
    fn parameters_schema(&self) -> serde_json::Value {
        json!({})
    }
    async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<ToolResult> {
        unreachable!("the gate never executes tools")
    }
}

async fn decide_args_mcp(g: &PolicyGate, args: &serde_json::Value) -> GateOutcome {
    let registry: Vec<Box<dyn Tool>> = Vec::new();
    g.decide(&CallFacts {
        tool: &ArgsMcp,
        registry: &registry,
        args,
        call_id: "c",
        turn_id: "t",
        agent_alias: None,
        parent_agent_alias: None,
        interactive: true,
    })
    .await
}

#[tokio::test]
async fn generic_mcp_tool_honours_server_grants_and_exclusions() {
    let servers = list(&["gh", "slack"]);
    let excluded = list(&["slack"]);
    let i = AgentPolicyInputs {
        mcp_servers: &servers,
        ..inputs(None, Some(&excluded), None)
    };
    let g = gate(&i);
    // Granted and not excluded.
    assert!(!denied(decide_args_mcp(&g, &json!({"server": "gh"})).await));
    // Excluded by name, even though a bundle granted it.
    assert!(denied(
        decide_args_mcp(&g, &json!({"server": "slack"})).await
    ));
    // Never granted.
    assert!(denied(
        decide_args_mcp(&g, &json!({"server": "other"})).await
    ));
    // An all-server call cannot be filtered, so it is refused while a granted
    // server is excluded.
    assert!(denied(decide_args_mcp(&g, &json!({})).await));
}

#[tokio::test]
async fn generic_mcp_tool_all_server_call_passes_when_nothing_is_excluded() {
    let servers = list(&["gh"]);
    let i = AgentPolicyInputs {
        mcp_servers: &servers,
        ..inputs(None, None, None)
    };
    assert!(!denied(decide_args_mcp(&gate(&i), &json!({})).await));
    // No grants at all: a named server is denied.
    assert!(denied(
        decide_args_mcp(&gate(&inputs(None, None, None)), &json!({"server": "gh"})).await
    ));
}

struct ModelSwitchLike;

impl Attributable for ModelSwitchLike {
    fn role(&self) -> Role {
        Role::System
    }
    fn alias(&self) -> &str {
        "model-switch-like"
    }
}

#[async_trait]
impl Tool for ModelSwitchLike {
    fn capability(&self) -> ToolCapability {
        ToolCapability::AgentInternal
    }
    fn capability_for_call(&self, args: &serde_json::Value) -> ToolCapability {
        match args.get("action").and_then(|v| v.as_str()) {
            Some("list_models") => ToolCapability::Network,
            _ => ToolCapability::AgentInternal,
        }
    }
    fn name(&self) -> &str {
        "model_switch"
    }
    fn description(&self) -> &str {
        "mock"
    }
    fn parameters_schema(&self) -> serde_json::Value {
        json!({})
    }
    async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<ToolResult> {
        unreachable!("the gate never executes tools")
    }
}

#[tokio::test]
async fn a_call_is_decided_on_its_per_call_capability() {
    let registry: Vec<Box<dyn Tool>> = Vec::new();
    let g = gate(&inputs(None, None, None));
    let facts = |args: &'static serde_json::Value| CallFacts {
        tool: &ModelSwitchLike,
        registry: &[] as &[Box<dyn Tool>],
        args,
        call_id: "c",
        turn_id: "t",
        agent_alias: None,
        parent_agent_alias: None,
        interactive: true,
    };
    let _ = &registry;
    let set: &'static serde_json::Value = Box::leak(Box::new(json!({"action": "set"})));
    let list: &'static serde_json::Value = Box::leak(Box::new(json!({"action": "list_models"})));
    assert!(!denied(g.decide(&facts(set)).await));
    // Live provider catalog request: network class, default-deny.
    assert!(denied(g.decide(&facts(list)).await));
    let allowed = vec!["model_switch".to_owned()];
    let g = gate(&inputs(Some(&allowed), None, None));
    // Naming the tool is the explicit operator act that grants network class.
    assert!(!denied(g.decide(&facts(list)).await));
}

struct Runs {
    name: &'static str,
    cap: ToolCapability,
    runs: Arc<std::sync::atomic::AtomicUsize>,
}

impl Attributable for Runs {
    fn role(&self) -> Role {
        Role::System
    }
    fn alias(&self) -> &str {
        "runs"
    }
}

#[async_trait]
impl Tool for Runs {
    fn capability(&self) -> ToolCapability {
        self.cap
    }
    fn name(&self) -> &str {
        self.name
    }
    fn description(&self) -> &str {
        "counts"
    }
    fn parameters_schema(&self) -> serde_json::Value {
        json!({})
    }
    async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<ToolResult> {
        self.runs.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(ToolResult {
            success: true,
            output: zeroclaw_api::tool::ToolOutput::default(),
            error: None,
        })
    }
}

fn pipeline(
    gate: PolicyGate,
    runs: &Arc<std::sync::atomic::AtomicUsize>,
) -> zeroclaw_tools::pipeline::PipelineTool {
    let tool = |name, cap| -> Arc<dyn Tool> {
        Arc::new(Runs {
            name,
            cap,
            runs: Arc::clone(runs),
        })
    };
    let config = zeroclaw_config::schema::PipelineConfig {
        enabled: true,
        max_steps: 20,
        allowed_tools: vec!["web_fetch".into(), "file_read".into(), "shell".into()],
    };
    zeroclaw_tools::pipeline::PipelineTool::new(
        config,
        vec![
            tool("web_fetch", ToolCapability::Network),
            tool("file_read", ToolCapability::FsRead),
            tool("shell", ToolCapability::Shell),
        ],
    )
    .with_step_admission(Arc::new(gate).step_admission())
}

async fn run_pipeline(
    pipe: &zeroclaw_tools::pipeline::PipelineTool,
    steps: &[&str],
    parallel: bool,
) -> ToolResult {
    let steps: Vec<_> = steps
        .iter()
        .map(|t| json!({"tool": t, "args": {}}))
        .collect();
    pipe.execute(json!({"steps": steps, "parallel": parallel}))
        .await
        .unwrap()
}

#[tokio::test]
async fn pipeline_step_needing_explicit_network_listing_is_denied_by_default() {
    let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let pipe = pipeline(gate(&inputs(None, None, None)), &runs);
    let out = run_pipeline(&pipe, &["web_fetch"], false).await;
    assert!(!out.success);
    assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst), 0);
    // An ordinary step passes through the same gate.
    assert!(run_pipeline(&pipe, &["file_read"], false).await.success);
    assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn pipeline_step_listed_by_the_operator_runs() {
    let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let allowed = list(&["web_fetch", "file_read", "shell"]);
    let pipe = pipeline(gate(&inputs(Some(&allowed), None, None)), &runs);
    assert!(run_pipeline(&pipe, &["web_fetch"], false).await.success);
}

#[tokio::test]
async fn pipeline_step_in_ask_tools_is_denied_not_prompted() {
    let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let ask = list(&["file_read"]);
    let i = AgentPolicyInputs {
        always_ask: &ask,
        ..inputs(None, None, None)
    };
    let pipe = pipeline(gate(&i), &runs);
    let out = run_pipeline(&pipe, &["file_read"], false).await;
    assert!(!out.success);
    assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test]
async fn parallel_pipeline_denies_before_any_step_starts() {
    let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let pipe = pipeline(gate(&inputs(None, None, None)), &runs);
    let out = run_pipeline(&pipe, &["shell", "web_fetch"], true).await;
    assert!(!out.success);
    assert_eq!(
        runs.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "no step half-runs"
    );
}

#[test]
fn a_tool_named_star_cannot_fold_into_the_wildcard() {
    assert_eq!(fold_name("*"), None);
    assert_eq!(fold_name(" * "), None);
    // As a list entry it still means the wildcard.
    let profile = profile_from_inputs(&inputs(Some(&list(&["*"])), None, None));
    assert!(profile.allowed_tools.unwrap().contains("*"));
}

#[tokio::test]
async fn deny_all_gate_denies_everything() {
    let g = PolicyGate::deny_all();
    assert!(denied(
        decide(
            &g,
            &mock("file_read", ToolCapability::FsRead),
            &json!({}),
            "c"
        )
        .await
    ));
}

#[tokio::test]
async fn caller_list_is_exposed_to_the_executing_tool_and_cleared_outside() {
    let caller = list(&["file_read"]);
    let g = gate(&inputs(None, None, Some(&caller)));
    assert_eq!(g.caller_list(), Some(caller.clone()));
    assert_eq!(active_caller_list(), None);
    let seen = scope_caller_list(g.caller_list(), async { active_caller_list() }).await;
    assert_eq!(seen, Some(caller));
    assert_eq!(active_caller_list(), None);
    assert_eq!(gate(&inputs(None, None, None)).caller_list(), None);
}

#[tokio::test]
async fn an_extension_tool_cannot_take_a_built_in_name_even_when_listed() {
    // The operator listed `tool_search` meaning the built-in; a plugin that
    // registered under that name must not inherit the grant.
    let allowed = list(&["tool_search", "my-plugin"]);
    let g = gate(&inputs(Some(&allowed), None, None));
    assert!(denied(
        decide(
            &g,
            &mock("tool_search", ToolCapability::Plugin),
            &json!({}),
            "a"
        )
        .await
    ));
    assert!(denied(
        decide(&g, &mock("shell", ToolCapability::Plugin), &json!({}), "b").await
    ));
    // The inventory names `TodoWrite` in mixed case; the gate folds names, so
    // a plugin under any casing of it is refused when the built-in is listed.
    let todo = list(&["TodoWrite"]);
    let g_todo = gate(&inputs(Some(&todo), None, None));
    for name in ["TodoWrite", "todowrite"] {
        assert!(
            denied(
                decide(
                    &g_todo,
                    &mock(name, ToolCapability::Plugin),
                    &json!({}),
                    "t"
                )
                .await
            ),
            "{name}"
        );
    }
    // A plugin with its own name, listed, still runs.
    assert!(!denied(
        decide(
            &g,
            &mock("my-plugin", ToolCapability::Plugin),
            &json!({}),
            "c"
        )
        .await
    ));
}

// ── Decision audit and approval receipts (ADR-019 D6) ───────────────────

mod audit {
    use super::*;
    use crate::security::policy_audit::{FileDecisionSink, POLICY_AUDIT_FILE};
    use crate::security::policy_contract::{ApprovalResponse, DecisionRecord, DecisionSink};
    use zeroclaw_config::governance::GovernanceAuditConfig;

    #[derive(Default)]
    struct MemorySink(Mutex<Vec<DecisionRecord>>);

    impl DecisionSink for MemorySink {
        fn record(&self, record: &DecisionRecord) -> anyhow::Result<()> {
            self.0.lock().push(record.clone());
            Ok(())
        }
    }

    /// Accepts `allowed` writes, then fails every later one.
    struct FlakySink {
        allowed: AtomicU64,
    }

    impl DecisionSink for FlakySink {
        fn record(&self, _record: &DecisionRecord) -> anyhow::Result<()> {
            let left = self.allowed.load(Ordering::SeqCst);
            if left == 0 {
                anyhow::bail!("disk full at /secret/path");
            }
            self.allowed.store(left - 1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn audited_gate(sink: Arc<dyn DecisionSink>) -> PolicyGate {
        let ask = list(&["shell"]);
        let i = AgentPolicyInputs {
            always_ask: &ask,
            ..inputs(None, None, None)
        };
        gate(&i).with_sink(sink)
    }

    const READ: ToolCapability = ToolCapability::FsRead;

    #[tokio::test]
    async fn allow_deny_and_ask_each_produce_one_record() {
        let sink = Arc::new(MemorySink::default());
        let g = audited_gate(sink.clone());
        let args = json!({"path": "a"});
        assert!(!denied(
            decide(&g, &mock("file_read", READ), &args, "allow").await
        ));
        assert!(denied(
            decide(&g, &mock("mystery", ToolCapability::Unknown), &args, "deny").await
        ));
        assert!(!denied(
            decide(&g, &mock("shell", ToolCapability::Shell), &args, "ask").await
        ));

        let records = sink.0.lock();
        let effective: Vec<_> = records
            .iter()
            .map(|r| {
                (
                    r.call_id.as_str(),
                    r.decision.effective,
                    r.decision_id.is_some(),
                )
            })
            .collect();
        assert_eq!(
            effective,
            [
                ("allow", Verdict::Allow, true),
                ("deny", Verdict::Deny, false),
                ("ask", Verdict::Ask, true),
            ]
        );
        assert_eq!(records[0].tool_name, "file_read");
        assert_eq!(records[0].capability, READ);
        assert_eq!(records[0].args_sha256, args_sha256(&args));
        assert!(records.iter().all(|r| r.approval.is_none()));
    }

    #[tokio::test]
    async fn granted_ask_writes_a_receipt_scoped_to_that_decision_and_is_single_use() {
        let sink = Arc::new(MemorySink::default());
        let g = audited_gate(sink.clone());
        let tool = mock("shell", ToolCapability::Shell);
        let args = json!({"command": "ls"});
        decide(&g, &tool, &args, "c1").await;
        assert!(g.approve("c1", "shell", &args));
        let receipt = {
            let records = sink.0.lock();
            assert_eq!(records.len(), 2);
            records[1].approval.clone().expect("receipt")
        };
        assert_eq!(receipt.response, ApprovalResponse::Granted);
        assert_eq!(
            Some(&receipt.decision_id),
            sink.0.lock()[0].decision_id.as_ref()
        );
        assert_eq!(redeem(&g, &tool, &args, "c1").await, Ok(()));
        assert_eq!(
            redeem(&g, &tool, &args, "c1").await,
            Err(RedeemError::NoTicket)
        );
        // A receipt does not carry over to a different call or arguments.
        decide(&g, &tool, &args, "c2").await;
        assert!(!g.approve("c2", "shell", &json!({"command": "rm"})));
        assert_eq!(sink.0.lock().len(), 3);
    }

    #[tokio::test]
    async fn rejected_ask_writes_a_receipt_and_leaves_nothing_redeemable() {
        let sink = Arc::new(MemorySink::default());
        let g = audited_gate(sink.clone());
        let tool = mock("shell", ToolCapability::Shell);
        let args = json!({"command": "ls"});
        decide(&g, &tool, &args, "c1").await;
        g.reject("c1", "shell", &args);
        assert_eq!(
            sink.0.lock()[1].approval.as_ref().map(|a| a.response),
            Some(ApprovalResponse::Rejected)
        );
        assert_eq!(
            redeem(&g, &tool, &args, "c1").await,
            Err(RedeemError::NoTicket)
        );
        // A late approval cannot revive it.
        assert!(!g.approve("c1", "shell", &args));
    }

    #[tokio::test]
    async fn write_failure_denies_and_mints_no_ticket() {
        let g = audited_gate(Arc::new(FlakySink {
            allowed: AtomicU64::new(0),
        }));
        let tool = mock("file_read", READ);
        let args = json!({});
        assert_eq!(
            decide(&g, &tool, &args, "c").await,
            GateOutcome::Deny {
                reason: ReasonCode::AuditWriteFailed
            }
        );
        assert_eq!(
            redeem(&g, &tool, &args, "c").await,
            Err(RedeemError::NoTicket)
        );
    }

    #[tokio::test]
    async fn receipt_write_failure_keeps_the_ask_unredeemable() {
        let g = audited_gate(Arc::new(FlakySink {
            allowed: AtomicU64::new(1),
        }));
        let tool = mock("shell", ToolCapability::Shell);
        let args = json!({});
        assert!(!denied(decide(&g, &tool, &args, "c").await));
        assert!(!g.approve("c", "shell", &args));
        assert_eq!(
            redeem(&g, &tool, &args, "c").await,
            Err(RedeemError::NoTicket)
        );
    }

    #[tokio::test]
    async fn the_log_never_contains_arguments_or_secrets() {
        const SECRET: &str = "sk-live-TOP-SECRET-0123456789";
        let tmp = tempfile::tempdir().unwrap();
        let sink = Arc::new(FileDecisionSink::new(
            tmp.path(),
            &GovernanceAuditConfig::default(),
        ));
        let g = audited_gate(sink);
        let args = json!({"command": format!("curl -H 'Authorization: {SECRET}'"), "path": "/home/u/.ssh/id_rsa"});
        let shell = mock("shell", ToolCapability::Shell);
        decide(&g, &shell, &args, "c1").await;
        g.approve("c1", "shell", &args);
        decide(&g, &mock("mystery", ToolCapability::Unknown), &args, "c2").await;
        // Output never reaches the gate; assert the file holds digests only.
        let text = std::fs::read_to_string(tmp.path().join(POLICY_AUDIT_FILE)).unwrap();
        assert_eq!(text.lines().count(), 3);
        for needle in [SECRET, "curl", "id_rsa", "Authorization"] {
            assert!(!text.contains(needle), "{needle} leaked into the log");
        }
        assert!(text.contains(&args_sha256(&args)));
    }

    #[tokio::test]
    async fn write_failure_reaches_the_model_only_as_a_closed_code() {
        let g = audited_gate(Arc::new(FlakySink {
            allowed: AtomicU64::new(0),
        }));
        let GateOutcome::Deny { reason } =
            decide(&g, &mock("file_read", READ), &json!({}), "c").await
        else {
            panic!("expected deny");
        };
        let message = denial_message("file_read", reason);
        assert!(message.contains("audit_write_failed"));
        assert!(!message.contains("/secret/path"));
    }
}
