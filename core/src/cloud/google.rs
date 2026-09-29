//! Google desktop OAuth in the system browser, with an ephemeral loopback port.

use std::time::Duration;

use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use reqwest::Url;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use uuid::Uuid;

use super::auth::{self, AuthSession, LoginResponse};

#[derive(Deserialize)]
struct GoogleConfig {
    client_id: String,
}

fn random_secret() -> String {
    // Two OS-random UUIDs provide 244 random bits and a valid 64-byte verifier.
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

pub async fn sign_in_with_google(
    link: bool,
    open_browser: impl FnOnce(&str) -> Result<()>,
) -> Result<AuthSession> {
    tokio::time::timeout(Duration::from_secs(300), login(link, open_browser))
        .await
        .context("Google login timed out")?
}

async fn login(link: bool, open_browser: impl FnOnce(&str) -> Result<()>) -> Result<AuthSession> {
    let base_url = auth::configured_base_url()
        .ok_or_else(|| anyhow::anyhow!("socai pro server URL is not configured"))?;
    let credentials = if link {
        Some(
            auth::load_credentials()
                .filter(|c| !c.user_id.is_empty() && !c.device_token.is_empty())
                .ok_or_else(|| anyhow::anyhow!("sign in before linking Google"))?,
        )
    } else {
        None
    };
    let client = auth::http_client()?;
    let config_response = client
        .get(format!("{base_url}/v1/auth/google/config"))
        .send()
        .await
        .context("failed to load Google login configuration")?;
    if matches!(config_response.status().as_u16(), 404 | 503) {
        bail!("Google login is not configured");
    }
    let config: GoogleConfig = auth::require_success(config_response, "Google login config")
        .await?
        .json()
        .await?;
    if config.client_id.trim().is_empty() {
        bail!("Google login is not configured");
    }
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .context("failed to start Google login callback")?;
    let redirect_uri = format!("http://{}/oauth/google/callback", listener.local_addr()?);
    let state = random_secret();
    let nonce = random_secret();
    let verifier = random_secret();
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let mut url = Url::parse("https://accounts.google.com/o/oauth2/v2/auth")?;
    url.query_pairs_mut().extend_pairs([
        ("client_id", config.client_id.as_str()),
        ("redirect_uri", redirect_uri.as_str()),
        ("response_type", "code"),
        ("scope", "openid email"),
        ("state", state.as_str()),
        ("nonce", nonce.as_str()),
        ("code_challenge", challenge.as_str()),
        ("code_challenge_method", "S256"),
        ("prompt", "select_account"),
    ]);
    open_browser(url.as_str())?;
    let code = loop {
        let (mut stream, _) = listener.accept().await?;
        // Ignore unrelated or malformed requests without aborting the login.
        match tokio::time::timeout(Duration::from_secs(3), callback(&mut stream, &state)).await {
            Ok(Ok(Some(code))) => break code,
            Ok(Err(error)) if error.to_string() == "Google login cancelled" => return Err(error),
            _ => continue,
        }
    };
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("could not resolve $HOME"))?;
    let install_id = crate::identity::load_or_create_install_id(&home.join(".socai"));
    let endpoint = if link { "link" } else { "verify" };
    let mut request = client
        .post(format!("{base_url}/v1/auth/google/{endpoint}"))
        .json(&json!({
            "code": code, "code_verifier": verifier, "redirect_uri": redirect_uri,
            "nonce": nonce, "install_id": install_id,
            "app_version": env!("CARGO_PKG_VERSION"), "label": "desktop",
        }));
    if let Some(creds) = credentials.as_ref() {
        request = request.bearer_auth(&creds.device_token);
    }
    let response = request
        .send()
        .await
        .context("Google login request failed")?;
    let body: LoginResponse = auth::require_success(response, "Google login")
        .await?
        .json()
        .await?;
    auth::save_login_credentials(&base_url, String::new(), body)
}

async fn callback(stream: &mut TcpStream, state: &str) -> Result<Option<String>> {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 1024];
    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
        let length = stream.read(&mut buffer).await?;
        if length == 0 || request.len() + length > 8192 {
            return Ok(None);
        }
        request.extend_from_slice(&buffer[..length]);
    }
    let text = std::str::from_utf8(&request)?;
    let mut parts = text.lines().next().unwrap_or_default().split_whitespace();
    let method = parts.next().unwrap_or_default();
    let path = parts.next().unwrap_or_default();
    let url = Url::parse(&format!("http://127.0.0.1{path}"))?;
    let values: Vec<_> = url.query_pairs().collect();
    let get = |key: &str| -> Option<String> {
        let mut matches = values.iter().filter(|(k, _)| k == key);
        let value = matches.next()?.1.to_string();
        if matches.next().is_some() {
            None
        } else {
            Some(value)
        }
    };
    let valid = method == "GET"
        && url.path() == "/oauth/google/callback"
        && get("state").as_deref() == Some(state);
    let code = get("code").filter(|c| !c.is_empty());
    let denied = get("error").is_some();
    let accepted = valid && (code.is_some() || denied);
    let message = if accepted {
        "Return to socai to finish signing in."
    } else {
        "Invalid callback. Return to socai and try again."
    };
    let body = format!(
        "<!doctype html><html><meta charset=\"utf-8\"><title>socai</title><p>{message}</p></html>"
    );
    let status = if accepted {
        "200 OK"
    } else {
        "400 Bad Request"
    };
    let response = format!("HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: default-src 'none'; frame-ancestors 'none'\r\nConnection: close\r\n\r\n{body}", body.len());
    // Browser disconnection must not discard an otherwise valid authorization.
    let _ = stream.write_all(response.as_bytes()).await;
    if valid && denied {
        bail!("Google login cancelled");
    }
    Ok(if accepted { code } else { None })
}
