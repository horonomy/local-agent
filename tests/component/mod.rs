#[cfg(all(feature = "agent-core", feature = "channel-acp-server"))]
mod acp_cli;
#[cfg(all(feature = "agent-core", feature = "channel-acp-server"))]
mod acp_retry_stop_notice_stdio;
#[cfg(feature = "channel-acp-server")]
mod acp_session_cwd_stdio;
mod agents_export_cli;
#[cfg(all(feature = "agent-core", not(feature = "gateway")))]
mod cli_only_build;
#[cfg(feature = "agent-core")]
mod config_dir_locale_regression;
mod config_patch_cli;
mod config_persistence;
mod config_schema;
#[cfg(feature = "agent-core")]
mod cost_status_cli;
#[cfg(feature = "agent-core")]
mod cron_delivery_cli;
mod cron_help_examples;
#[cfg(not(feature = "local-agent-minimal"))]
mod daemon_startup_feedback;
mod degraded_config_remediation;
#[cfg(all(feature = "agent-core", feature = "gateway", target_os = "linux"))]
mod desktop_cli_linux;
mod direct_cli_terminal_completion;
mod dockerignore_test;
#[cfg(feature = "gateway")]
mod gateway;
mod gemini_capabilities;
mod hardware_probe_feature_graph;
mod minimal_profile_cli;
mod minimal_profile_feature_graph;
mod oidc_enrollment_cli;
mod otel_dependency_feature_regression;
mod plugin_feature_graph;
mod provider_resolution;
mod provider_schema;
#[cfg(feature = "agent-core")]
#[cfg(not(feature = "local-agent-minimal"))]
mod relay_claim_cli;
mod reply_target_field_regression;
mod schema_export_feature_graph;
mod security;
mod skills_bundle_cli;
#[cfg(feature = "agent-core")]
mod verifiable_intent_notice_visibility;
#[cfg(feature = "gateway")]
mod whatsapp_webhook_security;

#[cfg(feature = "plugins-wasm")]
mod plugin_info_cli;
#[cfg(feature = "plugins-wasm")]
mod plugin_update_cli;
