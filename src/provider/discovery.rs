//! Upstream model discovery per provider type.

use super::{Provider, ProviderType, endpoints, http_body, oauth};
use crate::{error::ApiError, format::UpstreamFormat};
use reqwest::Client;
use serde_json::Value;

/// Fetches the upstream model identifiers (`upstream_name` values) of a
/// provider.
///
/// # Errors
///
/// Returns [`ApiError`] when the upstream call fails or the payload shape is
/// not recognized.
pub(crate) async fn discover_upstream_models(
    http: &Client,
    provider: &Provider,
) -> Result<Vec<String>, ApiError> {
    // Every credential kind carries an upstream secret; the codex path below
    // still fails with an explicit internal error when the credentials are
    // not of the kind its provider type requires.
    let secret = provider.credentials.secret();

    match provider.provider_type {
        ProviderType::OpenAiOauth => discover_codex(http, provider, secret).await,

        ProviderType::AntigravityOauth => discover_antigravity(http, secret).await,

        ProviderType::OpenAiCompatible => discover_compatible(http, provider, secret).await,
    }
}

/// Discovers Codex models from the ChatGPT backend.
///
/// The payload must contain an `items` or `models` array whose entries carry
/// a `slug` string.
async fn discover_codex(
    http: &Client,
    provider: &Provider,
    access_token: &str,
) -> Result<Vec<String>, ApiError> {
    let account_id = match &provider.credentials {
        super::Credentials::OpenAiOauth { account_id, .. } => account_id.clone(),
        _ => {
            return Err(ApiError::internal(
                "codex discovery requires OpenAI OAuth credentials",
            ));
        }
    };

    let response = http
        .get(endpoints::CODEX_MODELS_URL)
        .bearer_auth(access_token)
        .header("chatgpt-account-id", account_id)
        .send()
        .await
        .map_err(|error| {
            ApiError::upstream_unavailable(format!("upstream unreachable: {error}"))
        })?;

    let value = parse_success_body(response).await?;

    let entries = value
        .get("items")
        .or_else(|| value.get("models"))
        .and_then(Value::as_array)
        .ok_or_else(|| {
            ApiError::upstream_unavailable(
                "model discovery payload lacks an `items` or `models` array",
            )
        })?;

    entries
        .iter()
        .map(|entry| {
            entry
                .get("slug")
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| {
                    ApiError::upstream_unavailable("model discovery entry lacks a `slug` string")
                })
        })
        .collect()
}

/// Discovers Antigravity models from Cloud Code Assist.
async fn discover_antigravity(http: &Client, access_token: &str) -> Result<Vec<String>, ApiError> {
    let value: Value = oauth::post_code_assist(
        http,
        endpoints::ANTIGRAVITY_FETCH_AVAILABLE_MODELS_ACTION,
        access_token,
        &serde_json::json!({}),
    )
    .await?;

    let models = value
        .get("models")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            ApiError::upstream_unavailable("model discovery payload lacks a `models` object")
        })?;

    Ok(models.keys().cloned().collect())
}

/// Discovers models from an OpenAI-compatible upstream, shaped by its
/// protocol.
async fn discover_compatible(
    http: &Client,
    provider: &Provider,
    access_token: &str,
) -> Result<Vec<String>, ApiError> {
    let protocol = provider.protocol.ok_or_else(|| {
        ApiError::internal("openai_compatible provider lacks an upstream protocol")
    })?;

    let base_url = provider
        .base_url
        .as_deref()
        .ok_or_else(|| ApiError::internal("openai_compatible provider lacks a base URL"))?;

    let base_url = base_url.trim_end_matches('/');

    let mut request = http.get(format!("{base_url}/models"));

    request = match protocol {
        UpstreamFormat::OpenAiResponse | UpstreamFormat::OpenAiChatCompletion => {
            request.bearer_auth(access_token)
        }

        UpstreamFormat::GeminiGenerateContent => request.header("x-goog-api-key", access_token),
    };

    let response = request.send().await.map_err(|error| {
        ApiError::upstream_unavailable(format!("upstream unreachable: {error}"))
    })?;

    let value = parse_success_body(response).await?;

    match protocol {
        UpstreamFormat::OpenAiResponse | UpstreamFormat::OpenAiChatCompletion => {
            let entries = value.get("data").and_then(Value::as_array).ok_or_else(|| {
                ApiError::upstream_unavailable("model discovery payload lacks a `data` array")
            })?;

            entries
                .iter()
                .map(|entry| {
                    entry
                        .get("id")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .ok_or_else(|| {
                            ApiError::upstream_unavailable(
                                "model discovery entry lacks an `id` string",
                            )
                        })
                })
                .collect()
        }

        UpstreamFormat::GeminiGenerateContent => {
            let entries = value
                .get("models")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    ApiError::upstream_unavailable("model discovery payload lacks a `models` array")
                })?;

            entries
                .iter()
                .map(|entry| {
                    let name = entry.get("name").and_then(Value::as_str).ok_or_else(|| {
                        ApiError::upstream_unavailable(
                            "model discovery entry lacks a `name` string",
                        )
                    })?;

                    Ok(name.strip_prefix("models/").unwrap_or(name).to_string())
                })
                .collect()
        }
    }
}

/// Verifies a successful status and parses the JSON body.
///
/// Only the error path is read through the bounded error-body read: a success
/// body is legitimate discovery JSON of unbounded size and is read in full.
///
/// # Errors
///
/// Returns [`ApiError`] for non-success statuses and malformed bodies.
async fn parse_success_body(response: reqwest::Response) -> Result<Value, ApiError> {
    let status = response.status();

    if !status.is_success() {
        let body = http_body::read_error_body(response).await;

        return Err(ApiError::upstream_rejected(
            status.as_u16(),
            http_body::truncate_excerpt(&body),
        ));
    }

    let body = response.text().await.map_err(|error| {
        ApiError::upstream_unavailable(format!("upstream read failed: {error}"))
    })?;

    serde_json::from_str(&body).map_err(|error| {
        ApiError::upstream_unavailable(format!("malformed upstream response: {error}"))
    })
}

#[cfg(test)]
mod tests {
    use super::{discover_compatible, discover_upstream_models, parse_success_body};
    use crate::{
        error::ApiError,
        format::UpstreamFormat,
        provider::{Credentials, Provider, ProviderType},
    };
    use reqwest::Client;
    use std::{
        error::Error as StdError,
        io::{Read as _, Write as _},
    };

    /// Serves one raw HTTP response on a loopback socket after reading the
    /// client request, returning the bound address.
    ///
    /// # Panics
    ///
    /// Panics if the socket fails, which fails the test.
    fn serve_once(head: String, body: &[u8]) -> std::net::SocketAddr {
        #[expect(clippy::expect_used, reason = "a bind failure fails the test")]
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("loopback listener should bind");

        #[expect(
            clippy::expect_used,
            reason = "an address lookup failure fails the test"
        )]
        let address = listener
            .local_addr()
            .expect("bound listener should report an address");

        let body = body.to_vec();

        std::thread::spawn(move || {
            #[expect(clippy::expect_used, reason = "a socket failure fails the test")]
            let mut socket = listener.accept().expect("accept should succeed").0;

            // Read the client request before responding, mirroring the
            // established loopback fixture: an early response makes the HTTP
            // client reject the connection instead of returning it.
            let mut request = Vec::new();

            let mut scratch = [0u8; 1024];

            loop {
                #[expect(clippy::expect_used, reason = "a read failure fails the test")]
                let read = socket
                    .read(&mut scratch)
                    .expect("request should be readable");

                #[expect(
                    clippy::indexing_slicing,
                    reason = "Read::read returns a count within the buffer length"
                )]
                request.extend_from_slice(&scratch[..read]);

                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }

            #[expect(clippy::expect_used, reason = "a write failure fails the test")]
            socket.write_all(head.as_bytes()).expect("head is writable");

            #[expect(clippy::expect_used, reason = "a write failure fails the test")]
            socket.write_all(&body).expect("body is writable");
        });

        address
    }

    /// Builds an `openai_compatible` provider of the given protocol dialing
    /// the given base URL.
    fn compatible_provider(protocol: UpstreamFormat, base_url: String) -> Provider {
        Provider {
            id: 1,
            name: "compat".to_string(),
            model_prefix: String::new(),
            provider_type: ProviderType::OpenAiCompatible,
            protocol: Some(protocol),
            base_url: Some(base_url),
            credentials: Credentials::ApiKey("test-key".to_string()),
            expires_at: None,
            is_enabled: true,
        }
    }

    /// Drives `discover_compatible` for `protocol` against a loopback
    /// upstream serving `status_line` with the given JSON `body`.
    ///
    /// # Panics
    ///
    /// Panics if a socket or client build failure occurs, which fails the
    /// test.
    async fn discover_from_loopback(
        protocol: UpstreamFormat,
        status_line: &str,
        body: &str,
    ) -> Result<Vec<String>, ApiError> {
        let head = format!(
            "{status_line}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\
             connection: close\r\n\r\n",
            body.len()
        );

        let address = serve_once(head, body.as_bytes());

        #[expect(clippy::expect_used, reason = "a client build failure fails the test")]
        let http = Client::builder()
            .build()
            .expect("test HTTP client should build");

        let provider = compatible_provider(protocol, format!("http://{address}"));

        discover_compatible(&http, &provider, "test-key").await
    }

    /// Verifies a successful upstream body parses into its JSON value.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while parsing, which fails the test.
    #[tokio::test]
    async fn parse_success_body_parses_json_success_bodies() -> Result<(), Box<dyn StdError>> {
        let response = axum::http::Response::builder()
            .status(200)
            .body(r#"{"items":[]}"#.to_string())
            .map_err(|error| format!("static response parts should build: {error}"))?;

        let value = parse_success_body(response.into()).await?;

        assert_eq!(value.get("items"), Some(&serde_json::json!([])));

        Ok(())
    }

    /// Verifies a non-2xx upstream response renders as a management-served
    /// 502 `upstream_error` carrying the upstream status and a body excerpt,
    /// never the foreign status verbatim (README §7).
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[tokio::test]
    async fn parse_success_body_rejects_non_success_status_with_excerpt() {
        #[expect(
            clippy::expect_used,
            reason = "static response parts always build; a failure fails the test"
        )]
        let response = axum::http::Response::builder()
            .status(403)
            .body("forbidden by upstream".to_string())
            .expect("static response parts should build");

        #[expect(
            clippy::expect_used,
            reason = "the rejection is the subject of the test"
        )]
        let error = parse_success_body(response.into())
            .await
            .expect_err("a 403 upstream response must be rejected");

        assert_eq!(error.status, 502);

        assert_eq!(error.code.as_deref(), Some("upstream_error"));

        assert!(
            error.message.contains("403") && error.message.contains("forbidden by upstream"),
            "the message must embed the upstream status and excerpt: {error}"
        );
    }

    /// Verifies a malformed success body fails as a 502 upstream transport
    /// failure instead of being surfaced as a client fault (README §6.7).
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[tokio::test]
    async fn parse_success_body_rejects_malformed_json_as_unavailable() {
        #[expect(
            clippy::expect_used,
            reason = "static response parts always build; a failure fails the test"
        )]
        let response = axum::http::Response::builder()
            .status(200)
            .body("{not json".to_string())
            .expect("static response parts should build");

        #[expect(
            clippy::expect_used,
            reason = "the malformed body must make parsing fail"
        )]
        let error = parse_success_body(response.into())
            .await
            .expect_err("a malformed success body must be rejected");

        assert_eq!(error.status, 502);

        assert_eq!(error.code.as_deref(), Some("upstream_error"));

        assert!(
            error.message.contains("malformed upstream response"),
            "the message must state the payload was malformed: {error}"
        );
    }

    /// Verifies the Codex dispatch arm fails explicitly when its provider
    /// does not carry OpenAI OAuth credentials, before any upstream call.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[tokio::test]
    async fn codex_dispatch_rejects_mismatched_credentials_as_internal() {
        let provider = Provider {
            id: 1,
            name: "codex".to_string(),
            model_prefix: String::new(),
            provider_type: ProviderType::OpenAiOauth,
            protocol: None,
            base_url: None,
            credentials: Credentials::ApiKey("test-key".to_string()),
            expires_at: None,
            is_enabled: true,
        };

        #[expect(clippy::expect_used, reason = "a client build failure fails the test")]
        let http = Client::builder()
            .build()
            .expect("test HTTP client should build");

        #[expect(
            clippy::expect_used,
            reason = "the credentials mismatch must fail discovery"
        )]
        let error = discover_upstream_models(&http, &provider)
            .await
            .expect_err("an API-key provider dispatched as Codex must fail");

        assert_eq!(error.status, 500);

        assert_eq!(error.code.as_deref(), Some("internal"));

        assert!(
            error.message.contains("requires OpenAI OAuth credentials"),
            "the error must name the credentials contract: {error}"
        );
    }

    /// Verifies the compatible OpenAI-protocol discovery parses the
    /// `{data: [{id, ...}]}` payload shape (README §5.3).
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[tokio::test]
    async fn compatible_openai_discovery_parses_data_ids() {
        #[expect(
            clippy::expect_used,
            reason = "the success path is the subject of the test"
        )]
        let models = discover_from_loopback(
            UpstreamFormat::OpenAiChatCompletion,
            "HTTP/1.1 200 OK",
            r#"{"data":[{"id":"m2"},{"id":"m1"}]}"#,
        )
        .await
        .expect("loopback discovery should succeed");

        assert_eq!(models, vec!["m2".to_string(), "m1".to_string()]);
    }

    /// Verifies the compatible Gemini-protocol discovery parses the
    /// `{models: [{name, ...}]}` payload shape, stripping the `models/`
    /// prefix when present and keeping bare names verbatim (README §5.3).
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[tokio::test]
    async fn compatible_gemini_discovery_strips_models_prefix() {
        #[expect(
            clippy::expect_used,
            reason = "the success path is the subject of the test"
        )]
        let models = discover_from_loopback(
            UpstreamFormat::GeminiGenerateContent,
            "HTTP/1.1 200 OK",
            r#"{"models":[{"name":"models/gemini-x"},{"name":"bare-y"}]}"#,
        )
        .await
        .expect("loopback discovery should succeed");

        assert_eq!(models, vec!["gemini-x".to_string(), "bare-y".to_string()]);
    }

    /// Verifies an unrecognized payload shape fails discovery with a 502
    /// upstream error instead of wiping stored models with a silent empty
    /// result (README §5.3, §7.1).
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[tokio::test]
    async fn compatible_discovery_rejects_malformed_shape_as_upstream_unavailable() {
        #[expect(
            clippy::expect_used,
            reason = "the malformed shape must fail discovery"
        )]
        let error = discover_from_loopback(
            UpstreamFormat::OpenAiChatCompletion,
            "HTTP/1.1 200 OK",
            r#"{"unexpected":[]}"#,
        )
        .await
        .expect_err("a payload without a `data` array must be rejected");

        assert_eq!(error.status, 502);

        assert_eq!(error.code.as_deref(), Some("upstream_error"));

        assert!(
            error.message.contains("`data` array"),
            "the error must name the missing shape: {error}"
        );
    }

    /// Verifies a non-2xx discovery response is mirrored as a 502
    /// `upstream_error` with the upstream status embedded, never forwarded
    /// verbatim (README §7).
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[tokio::test]
    async fn compatible_discovery_mirrors_upstream_rejection_as_502() {
        #[expect(
            clippy::expect_used,
            reason = "the upstream rejection must fail discovery"
        )]
        let error = discover_from_loopback(
            UpstreamFormat::OpenAiChatCompletion,
            "HTTP/1.1 403 Forbidden",
            r#"{"error":"nope"}"#,
        )
        .await
        .expect_err("a 403 discovery response must be rejected");

        assert_eq!(error.status, 502);

        assert_eq!(error.code.as_deref(), Some("upstream_error"));

        assert!(
            error.message.contains("403"),
            "the message must embed the upstream status: {error}"
        );
    }
}
