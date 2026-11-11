use core::time::Duration;
use std::time::Instant;
use tokio::time;

use reqwest::Client;
use serde::Deserialize;
use serde::de::DeserializeOwned;

use super::Failure as AuthError;
use super::copilot::{FIXED_HEADERS, REFRESH_MARGIN_MS, USER_AGENT};
use super::credential::Credential;

/// The OAuth client id of the Copilot app.
const CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";
/// The OAuth scope requested.
const SCOPE: &str = "read:user";
/// The polling interval when GitHub gives none.
const DEFAULT_INTERVAL_SECS: u64 = 5;
/// How much longer to wait when GitHub says to slow down.
const SLOW_DOWN_SECS: u64 = 5;

#[derive(Debug, Clone)]
/// The GitHub endpoints used to log in.
pub struct Endpoints {
    pub access_token: String,
    pub copilot_token: String,
    pub device_code: String,
}

impl Endpoints {
    #[must_use]
    #[inline]
    /// The real GitHub endpoints.
    pub fn github() -> Self {
        Self {
            device_code: "https://github.com/login/device/code".into(),
            access_token: "https://github.com/login/oauth/access_token".into(),
            copilot_token: "https://api.github.com/copilot_internal/v2/token".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
/// A device code to enter on GitHub.
#[expect(clippy::struct_field_names, reason = "the names follow the provider's wire format or what the type wraps")]
pub struct DeviceCode {
    pub device_code: String,
    pub expires_in: u64,
    #[serde(default)]
    pub interval: Option<u64>,
    pub user_code: String,
    pub verification_uri: String,
}

#[derive(Deserialize)]
/// A poll of the token endpoint.
struct TokenPoll {
    access_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
    interval: Option<u64>,
}

#[derive(Deserialize)]
/// A Copilot token.
struct CopilotToken {
    expires_at: u64,
    token: String,
}

#[derive(Clone, Debug)]
/// The GitHub login client.
pub struct Github {
    endpoints: Endpoints,
    http: Client,
}

impl Github {
    #[inline]
    /// Trades a GitHub token for a Copilot credential.
    ///
    /// # Errors
    ///
    /// Fails if GitHub or Copilot rejects the token.
    pub async fn exchange_token(&self, github_token: &str) -> Result<Credential, AuthError> {
        let mut request =
            self.http.get(&self.endpoints.copilot_token).header("Accept", "application/json").bearer_auth(github_token);
        for (name, value) in FIXED_HEADERS {
            request = request.header(name, value);
        }
        let response = request.send().await.map_err(|error| network(&error))?;
        let status = response.status();
        let text = response.text().await.map_err(|error| network(&error))?;
        if !status.is_success() {
            return Err(AuthError::Flow(format!("Copilot token request failed ({status})")));
        }
        let token: CopilotToken = serde_json::from_str(&text)
            .map_err(|_parse_error| AuthError::Flow("invalid Copilot token response".into()))?;
        Ok(Credential::Oauth {
            refresh: github_token.to_owned(),
            access: token.token,
            expires: token.expires_at.saturating_mul(1000).saturating_sub(REFRESH_MARGIN_MS),
        })
    }

    #[must_use]
    #[inline]
    pub fn new(endpoints: Endpoints) -> Self {
        Self { http: super::http_client(), endpoints }
    }

    #[inline]
    /// Waits for the user to approve the device code, and returns the GitHub token.
    ///
    /// # Errors
    ///
    /// Fails if the code expires or GitHub reports an error.
    pub async fn poll_token(&self, device: &DeviceCode) -> Result<String, AuthError> {
        let deadline = Instant::now().checked_add(Duration::from_secs(device.expires_in)).unwrap_or_else(Instant::now);
        let mut interval = device.interval.unwrap_or(DEFAULT_INTERVAL_SECS);

        loop {
            time::sleep(Duration::from_secs(interval)).await;
            if Instant::now() >= deadline {
                return Err(AuthError::Flow("the login code expired; run /login again".into()));
            }
            let poll: TokenPoll = self
                .post_form(
                    &self.endpoints.access_token,
                    &[
                        ("client_id", CLIENT_ID),
                        ("device_code", &device.device_code),
                        ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                    ],
                )
                .await?;

            if let Some(token) = poll.access_token {
                return Ok(token);
            }
            match poll.error.as_deref() {
                Some("authorization_pending") => {},
                Some("slow_down") => {
                    interval = poll.interval.unwrap_or_else(|| interval.saturating_add(SLOW_DOWN_SECS));
                },
                Some(error) => {
                    let detail = poll.error_description.map(|d| format!(": {d}")).unwrap_or_default();
                    return Err(AuthError::Flow(format!("device flow failed: {error}{detail}")));
                },
                None => return Err(AuthError::Flow("invalid device token response".into())),
            }
        }
    }

    /// POSTs form fields and reads the JSON reply.
    async fn post_form<T>(&self, url: &str, pairs: &[(&str, &str)]) -> Result<T, AuthError>
    where
        T: DeserializeOwned,
    {
        let response = self
            .http
            .post(url)
            .header("Accept", "application/json")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("User-Agent", USER_AGENT)
            .body(form_encode(pairs))
            .send()
            .await
            .map_err(|error| network(&error))?;
        let status = response.status();
        let text = response.text().await.map_err(|error| network(&error))?;
        if !status.is_success() {
            return Err(AuthError::Flow(format!("GitHub returned {status}")));
        }
        serde_json::from_str(&text).map_err(|_parse_error| AuthError::Flow("unexpected response from GitHub".into()))
    }

    #[inline]
    /// Asks GitHub for a device code, refusing a verification URL that is not http(s).
    ///
    /// # Errors
    ///
    /// Fails if GitHub cannot be reached or answers with an untrusted URL.
    pub async fn start_device_flow(&self) -> Result<DeviceCode, AuthError> {
        let device: DeviceCode =
            self.post_form(&self.endpoints.device_code, &[("client_id", CLIENT_ID), ("scope", SCOPE)]).await?;
        if !(device.verification_uri.starts_with("https://") || device.verification_uri.starts_with("http://")) {
            return Err(AuthError::Flow("untrusted verification_uri in the device code response".into()));
        }
        Ok(device)
    }
}

fn network(error: &reqwest::Error) -> AuthError {
    AuthError::Flow(format!("network error: {error}"))
}

fn form_encode(pairs: &[(&str, &str)]) -> String {
    let encode = |text: &str| -> String {
        text.bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => char::from(b).to_string(),
                _ => format!("%{b:02X}"),
            })
            .collect()
    };
    pairs.iter().map(|&(k, v)| format!("{}={}", encode(k), encode(v))).collect::<Vec<_>>().join("&")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::testing::MockServer;

    fn device(interval: u64, expires_in: u64) -> DeviceCode {
        DeviceCode {
            device_code: "dev".into(),
            user_code: "ABCD-1234".into(),
            verification_uri: "https://github.com/login/device".into(),
            interval: Some(interval),
            expires_in,
        }
    }

    async fn poll(responses: Vec<(u16, &str)>, device: DeviceCode) -> (Result<String, AuthError>, MockServer) {
        let queued = responses.into_iter().map(|(status, body)| (status, body.to_owned())).collect();
        let server = MockServer::start(vec![("/login/oauth/access_token", queued)]).await;
        let result = Github::new(server.endpoints()).poll_token(&device).await;
        (result, server)
    }

    #[test]
    fn form_values_are_percent_encoded() {
        let body =
            form_encode(&[("grant_type", "urn:ietf:params:oauth:grant-type:device_code"), ("scope", "read:user")]);
        assert_eq!(body, "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code&scope=read%3Auser");
    }

    #[tokio::test]
    async fn the_device_flow_starts_with_the_client_id_and_scope() {
        let body = r#"{"device_code":"dev","user_code":"ABCD-1234","verification_uri":"https://github.com/login/device","interval":5,"expires_in":900}"#;
        let server = MockServer::start(vec![("/login/device/code", vec![(200, body.to_owned())])]).await;
        let device = Github::new(server.endpoints()).start_device_flow().await.unwrap();
        assert_eq!(device.user_code, "ABCD-1234");
        let request = &server.requests()[0];
        assert_eq!(request.method, "POST");
        assert!(request.body.contains("client_id=Iv1.b507a08c87ecfe98") && request.body.contains("scope=read%3Auser"));
    }

    #[tokio::test]
    async fn a_verification_uri_that_is_not_http_is_refused() {
        let body = r#"{"device_code":"d","user_code":"c","verification_uri":"file:///etc/passwd","expires_in":900}"#;
        let server = MockServer::start(vec![("/login/device/code", vec![(200, body.to_owned())])]).await;
        Github::new(server.endpoints()).start_device_flow().await.unwrap_err();
    }

    #[tokio::test]
    async fn polling_waits_through_pending_and_slow_down_then_returns_the_token() {
        let (result, server) = poll(
            vec![
                (200, r#"{"error":"authorization_pending"}"#),
                (200, r#"{"error":"slow_down","interval":0}"#),
                (200, r#"{"access_token":"gho_abc"}"#),
            ],
            device(0, 60),
        )
        .await;
        assert_eq!(result.unwrap(), "gho_abc");
        assert_eq!(server.requests().len(), 3);
        assert!(server.requests()[0].body.contains("device_code=dev"));
    }

    #[tokio::test]
    async fn an_access_denied_error_ends_the_flow() {
        let (result, _) =
            poll(vec![(200, r#"{"error":"access_denied","error_description":"The user said no"}"#)], device(0, 60))
                .await;
        assert_eq!(result.unwrap_err().to_string(), "device flow failed: access_denied: The user said no");
    }

    #[tokio::test]
    async fn an_expired_code_ends_the_flow() {
        let (result, _) = poll(vec![(200, r#"{"error":"authorization_pending"}"#)], device(0, 0)).await;
        assert!(result.unwrap_err().to_string().contains("expired"));
    }

    #[tokio::test]
    async fn a_garbage_response_ends_the_flow() {
        let (result, _) = poll(vec![(200, r#"{"surprise":true}"#)], device(0, 60)).await;
        result.unwrap_err();
    }

    #[tokio::test]
    async fn exchanging_gives_an_oauth_credential_that_expires_early() {
        let server = MockServer::start(vec![(
            "/copilot_internal/v2/token",
            vec![(
                200,
                r#"{"token":"tid=1;proxy-ep=proxy.individual.githubcopilot.com","expires_at":2000000000}"#.into(),
            )],
        )])
        .await;
        let credential = Github::new(server.endpoints()).exchange_token("gho_abc").await.unwrap();
        assert_eq!(
            credential,
            Credential::Oauth {
                refresh: "gho_abc".into(),
                access: "tid=1;proxy-ep=proxy.individual.githubcopilot.com".into(),
                expires: 2_000_000_000_000 - 300_000,
            }
        );
        let request = &server.requests()[0];
        assert_eq!(request.header("authorization"), Some("Bearer gho_abc"));
        assert_eq!(request.header("editor-version"), Some("vscode/1.107.0"));
        assert_eq!(request.header("copilot-integration-id"), Some("vscode-chat"));
    }

    #[tokio::test]
    #[ignore = "needs network access to github.com"]
    async fn the_real_device_code_endpoint_answers() {
        let device = Github::new(Endpoints::github()).start_device_flow().await.unwrap();
        assert_ne!(device.user_code, "");
        assert!(device.verification_uri.starts_with("https://"));
        assert!(device.expires_in > 0);
    }
}
