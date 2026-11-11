use crate::auth::profiles::TokenSet;
use anyhow::{Context, Result};
use chrono::Utc;
use reqwest::Client;
use serde::Deserialize;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct DeviceCodeStart {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: Option<String>,
    pub expires_in: u64,
    pub interval: u64,
    pub message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
    #[serde(default)]
    token_type: Option<String>,
    #[serde(default)]
    scope: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    #[serde(default)]
    verification_uri_complete: Option<String>,
    expires_in: u64,
    #[serde(default)]
    interval: Option<u64>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OAuthErrorResponse {
    error: String,
    #[serde(default)]
    error_description: Option<String>,
}

/// Reject endpoint URLs that would send OAuth2 secrets (device codes, refresh
/// and access tokens) over cleartext. `http` is accepted only for loopback
/// hosts, which keeps local test/mock servers usable.
fn require_https(url: &str) -> Result<reqwest::Url> {
    let parsed =
        reqwest::Url::parse(url).with_context(|| "Email OAuth2 endpoint is not a valid URL")?;
    match parsed.scheme() {
        "https" => Ok(parsed),
        "http" => {
            let host = parsed.host_str().unwrap_or_default();
            let host = host.trim_start_matches('[').trim_end_matches(']');
            let loopback = host.eq_ignore_ascii_case("localhost")
                || host
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback());
            if loopback {
                Ok(parsed)
            } else {
                anyhow::bail!(
                    "Email OAuth2 endpoint must use https (http is allowed only for loopback hosts)"
                )
            }
        }
        other => anyhow::bail!("Email OAuth2 endpoint has unsupported scheme '{other}'"),
    }
}

/// Every credential-bearing request goes through here, so the https requirement
/// is enforced at the one place a request is built and sent.
async fn post_form(
    client: &Client,
    endpoint: &str,
    form: &[(&str, &str)],
    send_context: &'static str,
) -> Result<reqwest::Response> {
    let endpoint = require_https(endpoint)?;
    client
        .post(endpoint)
        .form(form)
        .send()
        .await
        .context(send_context)
}

/// Refresh an existing access token using the standard `refresh_token` grant.
pub async fn refresh_access_token(
    client: &Client,
    token_url: &str,
    client_id: &str,
    refresh_token: &str,
    scopes: &[String],
) -> Result<TokenSet> {
    let scope_str = scopes.join(" ");
    let form: &[(&str, &str)] = &[
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", client_id),
        ("scope", &scope_str),
    ];

    let response = post_form(
        client,
        token_url,
        form,
        "Failed to refresh email OAuth2 token",
    )
    .await?;

    parse_token_response(response).await
}

/// Start RFC 8628 device-code flow.
pub async fn start_device_code_flow(
    client: &Client,
    device_code_url: &str,
    client_id: &str,
    scopes: &[String],
) -> Result<DeviceCodeStart> {
    let scope_str = scopes.join(" ");
    let form: &[(&str, &str)] = &[("client_id", client_id), ("scope", &scope_str)];

    let response = post_form(
        client,
        device_code_url,
        form,
        "Failed to start email OAuth2 device-code flow",
    )
    .await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Email OAuth2 device-code start failed ({status}): {body}");
    }

    let parsed: DeviceCodeResponse = response
        .json()
        .await
        .context("Failed to parse email OAuth2 device-code response")?;

    Ok(DeviceCodeStart {
        device_code: parsed.device_code,
        user_code: parsed.user_code,
        verification_uri: parsed.verification_uri,
        verification_uri_complete: parsed.verification_uri_complete,
        expires_in: parsed.expires_in,
        interval: parsed.interval.unwrap_or(5).max(1),
        message: parsed.message,
    })
}

/// Poll the token endpoint until the user completes authorization or the flow expires.
pub async fn poll_device_code_tokens(
    client: &Client,
    token_url: &str,
    client_id: &str,
    device: &DeviceCodeStart,
) -> Result<TokenSet> {
    require_https(token_url)?;
    let started = Instant::now();
    let mut interval_secs = device.interval.max(1);

    loop {
        if started.elapsed() > Duration::from_secs(device.expires_in) {
            anyhow::bail!("Email OAuth2 device-code flow timed out before authorization");
        }

        tokio::time::sleep(Duration::from_secs(interval_secs)).await;

        let form: &[(&str, &str)] = &[
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ("device_code", device.device_code.as_str()),
            ("client_id", client_id),
        ];

        let response = post_form(
            client,
            token_url,
            form,
            "Failed polling email OAuth2 token endpoint",
        )
        .await?;

        if response.status().is_success() {
            return parse_token_response(response).await;
        }

        let status = response.status();
        let text = response.text().await.unwrap_or_default();

        if let Ok(err) = serde_json::from_str::<OAuthErrorResponse>(&text) {
            match err.error.as_str() {
                "authorization_pending" => continue,
                "slow_down" => {
                    interval_secs = interval_secs.saturating_add(5);
                    continue;
                }
                "access_denied" => anyhow::bail!("Email OAuth2 authorization was denied"),
                "expired_token" => anyhow::bail!("Email OAuth2 device-code expired"),
                _ => anyhow::bail!(
                    "Email OAuth2 device-code polling failed ({status}): {}",
                    err.error_description.unwrap_or(err.error)
                ),
            }
        }

        anyhow::bail!("Email OAuth2 device-code polling failed ({status}): {text}");
    }
}

async fn parse_token_response(response: reqwest::Response) -> Result<TokenSet> {
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Email OAuth2 token request failed ({status}): {body}");
    }

    let token: TokenResponse = response
        .json()
        .await
        .context("Failed to parse email OAuth2 token response")?;

    let expires_at = token.expires_in.and_then(|seconds| {
        if seconds <= 0 {
            None
        } else {
            Some(Utc::now() + chrono::Duration::seconds(seconds))
        }
    });

    Ok(TokenSet {
        access_token: token.access_token,
        refresh_token: token.refresh_token,
        id_token: None,
        expires_at,
        token_type: token.token_type,
        scope: token.scope,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn require_https_accepts_https() {
        assert!(require_https("https://login.example.com/oauth/token").is_ok());
    }

    #[test]
    fn require_https_rejects_remote_http() {
        let err = require_https("http://login.example.com/oauth/token").unwrap_err();
        assert!(err.to_string().contains("must use https"));
    }

    #[test]
    fn require_https_allows_loopback_http() {
        for url in [
            "http://localhost:8080/token",
            "http://LOCALHOST/token",
            "http://127.0.0.1:9000/token",
            "http://127.1.2.3/token",
            "http://[::1]:9000/token",
        ] {
            assert!(require_https(url).is_ok(), "{url} should be allowed");
        }
    }

    #[test]
    fn require_https_rejects_malformed_and_other_schemes() {
        assert!(require_https("not a url").is_err());
        assert!(require_https("").is_err());
        assert!(require_https("ftp://example.com/token").is_err());
        assert!(require_https("http://localhost.evil.com/token").is_err());
    }

    #[test]
    fn require_https_rejects_loopback_lookalike_hosts() {
        for url in [
            "http://localhost@evil.com/token",
            "http://127.0.0.1@evil.com/token",
            "http://[2001:db8::1]/token",
            "http://[::ffff:127.0.0.1]/token",
        ] {
            assert!(require_https(url).is_err(), "{url} should be rejected");
        }
    }

    /// Local listener plus a client that resolves a non-loopback hostname to it.
    /// Rejection must happen before a connection is opened, so no credentials
    /// reach the socket.
    fn blocked_host_fixture() -> (std::net::TcpListener, Client, String) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let client = Client::builder()
            .resolve("oauth.blocked.test", addr)
            .build()
            .unwrap();
        let url = format!("http://oauth.blocked.test:{}/token", addr.port());
        (listener, client, url)
    }

    fn assert_no_connection(listener: &std::net::TcpListener) {
        match listener.accept() {
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            other => panic!("a connection reached the endpoint: {other:?}"),
        }
    }

    #[tokio::test]
    async fn rejected_endpoints_receive_no_connection_or_credentials() {
        let (listener, client, url) = blocked_host_fixture();
        let device = DeviceCodeStart {
            device_code: "dc".into(),
            user_code: "uc".into(),
            verification_uri: "https://example.com/device".into(),
            verification_uri_complete: None,
            expires_in: 60,
            interval: 1,
            message: None,
        };

        let err = refresh_access_token(&client, &url, "id", "secret-rt", &[])
            .await
            .unwrap_err();
        assert!(err.to_string().contains("must use https"));
        let err = start_device_code_flow(&client, &url, "id", &[])
            .await
            .unwrap_err();
        assert!(err.to_string().contains("must use https"));
        let err = poll_device_code_tokens(&client, &url, "id", &device)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("must use https"));

        assert_no_connection(&listener);
    }

    #[tokio::test]
    async fn loopback_http_still_reaches_the_endpoint() {
        // Control for the test above: the same listener does observe a request
        // when the endpoint is an allowed loopback URL.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let url = format!("http://127.0.0.1:{port}/token");
        let accept = std::thread::spawn(move || listener.accept().is_ok());
        let _ = refresh_access_token(&Client::new(), &url, "id", "rt", &[]).await;
        assert!(accept.join().unwrap(), "loopback request was not sent");
    }

    const INSECURE: &str = "http://login.example.com/oauth";

    #[tokio::test]
    async fn refresh_rejects_insecure_url_before_request() {
        let err = refresh_access_token(&Client::new(), INSECURE, "id", "rt", &[])
            .await
            .unwrap_err();
        assert!(err.to_string().contains("must use https"));
    }

    #[tokio::test]
    async fn device_code_start_rejects_insecure_url_before_request() {
        let err = start_device_code_flow(&Client::new(), INSECURE, "id", &[])
            .await
            .unwrap_err();
        assert!(err.to_string().contains("must use https"));
    }

    #[tokio::test]
    async fn poll_rejects_insecure_url_before_request() {
        let device = DeviceCodeStart {
            device_code: "dc".into(),
            user_code: "uc".into(),
            verification_uri: "https://example.com/verify".into(),
            verification_uri_complete: None,
            expires_in: 600,
            interval: 5,
            message: None,
        };
        let err = poll_device_code_tokens(&Client::new(), INSECURE, "id", &device)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("must use https"));
    }
}
