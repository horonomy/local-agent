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
        decide(&gate(&i), &mock_mcp(&mcp), &json!({}), "c").await
    ));
    // Listed by the operator and the caller: allowed.
    let both = list(&["file_read", "gh__search"]);
    let i = AgentPolicyInputs {
        mcp_servers: &servers,
        ..inputs(Some(&both), None, Some(&caller))
    };
    assert!(!denied(
        decide(&gate(&i), &mock_mcp(&mcp), &json!({}), "c").await
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
