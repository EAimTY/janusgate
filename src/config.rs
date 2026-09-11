//! Gateway configuration schema and custom deserializers.

use host_port_pair::HostPortPair;
use log::LevelFilter;
use nestify::nest;
use serde::Deserialize;
use serde_with::{As, DurationMilliSeconds};
use std::{num::NonZero, path::PathBuf, time::Duration};
use tower_http::cors::AllowOrigin;

nest! {
    #[derive(Debug, Deserialize)]*
    #[serde(deny_unknown_fields)]*
    /// Root gateway configuration schema.
    ///
    /// Every field is mandatory and unknown fields are strictly rejected.
    pub(crate) struct Config {
        /// Minimum severity a log record must carry to be emitted to standard
        /// error.
        pub(crate) log_level: LevelFilter,

        /// Address and TCP port on which the gateway listens for plaintext HTTP
        /// connections.
        pub(crate) listen: HostPortPair,

        /// In-process SQLite database configuration.
        pub(crate) database:
            /// In-process SQLite database configuration.
            pub(crate) struct DatabaseConfig {
                /// File path to the in-process SQLite database file.
                pub(crate) path: PathBuf,

                /// Maximum number of concurrent database connections in the connection
                /// pool.
                pub(crate) max_connections: NonZero<usize>,

                /// Maximum time in milliseconds to wait for a free connection from the
                /// connection pool when every connection is checked out.
                #[serde(with = "As::<DurationMilliSeconds>")]
                pub(crate) pool_timeout: Duration,
            },

        /// Administrative management API configuration.
        pub(crate) management_api:
            /// Administrative management API configuration.
            pub(crate) struct ManagementApiConfig {
                /// Bearer-less administrative authorization token required by
                /// `/management-api/*`.
                pub(crate) token: String,

                /// Cross-Origin Resource Sharing (CORS) configuration for
                /// `/management-api/*`.
                pub(crate) cors:
                    /// CORS configuration for an HTTP sub-router.
                    pub(crate) struct CorsConfig {
                        /// Origin(s) permitted for cross-origin requests.
                        ///
                        /// Specifying `"*"` or `["*"]` allows any origin. Explicit origins
                        /// (e.g. `"https://dashboard.example.com"` or
                        /// `["https://dashboard.example.com"]`) restrict requests to those
                        /// origins. An empty list `[]` permits no origins.
                        #[serde(deserialize_with = "deserializer::allowed_origin")]
                        pub(crate) allowed_origin: AllowOrigin,

                        /// Whether to allow requests from private networks
                        /// (Private Network Access).
                        pub(crate) allow_private_network: bool,
                    },
            },

        /// Outward client API configuration.
        pub(crate) api:
            /// Outward client protocol configuration.
            pub(crate) struct ApiConfig {
                /// Shared API key for outward protocol endpoints under `/api/*`.
                pub(crate) key: String,

                /// Cross-Origin Resource Sharing (CORS) configuration for `/api/*`.
                pub(crate) cors: CorsConfig,
            },
    }
}

/// Custom deserializers for gateway configuration.
mod deserializer {
    use axum::http::HeaderValue;
    use serde::{
        Deserializer,
        de::{Error, SeqAccess, Visitor},
    };
    use std::fmt;
    use tower_http::cors::AllowOrigin;

    /// Deserializes origin strings into an [`AllowOrigin`].
    ///
    /// Accepts either a single origin string (e.g. `"*"` or
    /// `"https://dashboard.example.com"`) or a sequence of origin strings
    /// (e.g. `["*"]`, `["https://dashboard.example.com"]`, or `[]`).
    ///
    /// # Errors
    ///
    /// Returns a deserialization error if any origin string is not a valid HTTP
    /// header value or the input type is neither a string nor a sequence.
    pub(super) fn allowed_origin<'de, D>(deserializer: D) -> Result<AllowOrigin, D::Error>
    where
        D: Deserializer<'de>,
    {
        /// Visitor that deserializes an [`AllowOrigin`] from either a single
        /// origin string or a sequence of origin strings.
        struct AllowedOriginVisitor;

        impl<'de> Visitor<'de> for AllowedOriginVisitor {
            type Value = AllowOrigin;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an origin string or a list of origin strings")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: Error,
            {
                if value == "*" {
                    Ok(AllowOrigin::any())
                } else {
                    let header_value = HeaderValue::from_str(value).map_err(Error::custom)?;

                    Ok(AllowOrigin::list([header_value]))
                }
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut origins = Vec::new();
                let mut has_wildcard = false;

                while let Some(string) = seq.next_element::<String>()? {
                    if string == "*" {
                        has_wildcard = true;
                    } else {
                        let header_value = HeaderValue::from_str(&string).map_err(Error::custom)?;

                        origins.push(header_value);
                    }
                }

                if has_wildcard {
                    Ok(AllowOrigin::any())
                } else {
                    Ok(AllowOrigin::list(origins))
                }
            }
        }

        deserializer.deserialize_any(AllowedOriginVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::Config;
    use log::LevelFilter;

    /// Renders a JSON configuration body carrying the given log level and
    /// secrets.
    fn config_json(log_level: &str, api_key: &str, management_token: &str) -> String {
        format!(
            concat!(
                r#"{{"log_level":"{log_level}","listen":"127.0.0.1:0","database":{{"#,
                r#""path":"test.db","max_connections":1,"pool_timeout":1}},"#,
                r#""management_api":{{"token":"{management_token}","cors":{{"#,
                r#""allowed_origin":"*","allow_private_network":false}}}},"#,
                r#""api":{{"key":"{api_key}","cors":{{"#,
                r#""allowed_origin":"*","allow_private_network":false}}}}}}"#,
            ),
            log_level = log_level,
            management_token = management_token,
            api_key = api_key,
        )
    }

    /// Verifies that valid configuration parses, carrying the configured
    /// log level.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn parses_valid_configuration() {
        #[expect(clippy::expect_used, reason = "valid configuration must parse")]
        let config: Config = serde_json::from_str(&config_json("info", "key", "token"))
            .expect("valid configuration should parse");

        assert_eq!(config.log_level, LevelFilter::Info);
    }

    /// Verifies that log levels parse case-insensitively.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn parses_log_level_case_insensitively() {
        #[expect(clippy::expect_used, reason = "valid configuration must parse")]
        let config: Config = serde_json::from_str(&config_json("WARN", "key", "token"))
            .expect("valid configuration should parse");

        assert_eq!(config.log_level, LevelFilter::Warn);
    }

    /// Verifies that a configuration missing the log level is rejected.
    ///
    /// # Panics
    ///
    /// Panics if the malformed configuration parses unexpectedly.
    #[test]
    fn rejects_missing_log_level() {
        let json = concat!(
            r#"{"listen":"127.0.0.1:0","database":{"path":"test.db","#,
            r#""max_connections":1,"pool_timeout":1},"management_api":{"token":"token","#,
            r#""cors":{"allowed_origin":"*","allow_private_network":false}},"#,
            r#""api":{"key":"key","cors":{"allowed_origin":"*","allow_private_network":false}}}"#,
        );

        #[expect(
            clippy::expect_used,
            reason = "invalid configuration must fail to parse"
        )]
        let error = serde_json::from_str::<Config>(json).expect_err("should reject");

        assert!(error.to_string().contains("missing field `log_level`"));
    }

    /// Verifies that a configuration missing the CORS field in
    /// `management_api` is rejected.
    ///
    /// # Panics
    ///
    /// Panics if the malformed configuration parses unexpectedly.
    #[test]
    fn rejects_missing_management_cors() {
        let json = concat!(
            r#"{"log_level":"info","listen":"127.0.0.1:0","database":{"path":"test.db","#,
            r#""max_connections":1,"pool_timeout":1},"management_api":{"token":"token"},"#,
            r#""api":{"key":"key","cors":{"allowed_origin":"*","allow_private_network":false}}}"#,
        );

        #[expect(
            clippy::expect_used,
            reason = "invalid configuration must fail to parse"
        )]
        let error = serde_json::from_str::<Config>(json).expect_err("should reject");

        assert!(error.to_string().contains("missing field `cors`"));
    }

    /// Verifies that a configuration missing the CORS field in `api` is
    /// rejected.
    ///
    /// # Panics
    ///
    /// Panics if the malformed configuration parses unexpectedly.
    #[test]
    fn rejects_missing_api_cors() {
        let json = concat!(
            r#"{"log_level":"info","listen":"127.0.0.1:0","database":{"path":"test.db","#,
            r#""max_connections":1,"pool_timeout":1},"management_api":{"token":"token","#,
            r#""cors":{"allowed_origin":"*","allow_private_network":false}},"#,
            r#""api":{"key":"key"}}"#,
        );

        #[expect(
            clippy::expect_used,
            reason = "invalid configuration must fail to parse"
        )]
        let error = serde_json::from_str::<Config>(json).expect_err("should reject");

        assert!(error.to_string().contains("missing field `cors`"));
    }

    /// Verifies that a configuration missing `allowed_origin` in CORS is
    /// rejected.
    ///
    /// # Panics
    ///
    /// Panics if the malformed configuration parses unexpectedly.
    #[test]
    fn rejects_missing_cors_allowed_origin() {
        let json = concat!(
            r#"{"log_level":"info","listen":"127.0.0.1:0","database":{"path":"test.db","#,
            r#""max_connections":1,"pool_timeout":1},"management_api":{"token":"token","#,
            r#""cors":{"allow_private_network":false}},"#,
            r#""api":{"key":"key","cors":{"allowed_origin":"*","allow_private_network":false}}}"#,
        );

        #[expect(
            clippy::expect_used,
            reason = "invalid configuration must fail to parse"
        )]
        let error = serde_json::from_str::<Config>(json).expect_err("should reject missing field");

        assert!(error.to_string().contains("missing field `allowed_origin`"));
    }

    /// Verifies that a configuration missing `allow_private_network` in CORS is
    /// rejected.
    ///
    /// # Panics
    ///
    /// Panics if the malformed configuration parses unexpectedly.
    #[test]
    fn rejects_missing_cors_allow_private_network() {
        let json = concat!(
            r#"{"log_level":"info","listen":"127.0.0.1:0","database":{"path":"test.db","#,
            r#""max_connections":1,"pool_timeout":1},"management_api":{"token":"token","#,
            r#""cors":{"allowed_origin":"*"}},"#,
            r#""api":{"key":"key","cors":{"allowed_origin":"*","allow_private_network":false}}}"#,
        );

        #[expect(
            clippy::expect_used,
            reason = "invalid configuration must fail to parse"
        )]
        let error = serde_json::from_str::<Config>(json).expect_err("should reject missing field");

        assert!(
            error
                .to_string()
                .contains("missing field `allow_private_network`")
        );
    }

    /// Helper to test CORS layer origin header behavior on preflight OPTIONS
    /// requests.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    async fn assert_origin_allows(
        origin: tower_http::cors::AllowOrigin,
        request_origin: &str,
        expected_allow_origin: Option<&str>,
    ) {
        let app = axum::Router::new()
            .route("/", axum::routing::get(|| async { "ok" }))
            .layer(tower_http::cors::CorsLayer::new().allow_origin(origin));

        #[expect(clippy::expect_used, reason = "valid request must build")]
        let request = axum::http::Request::builder()
            .method("OPTIONS")
            .uri("/")
            .header("origin", request_origin)
            .header("access-control-request-method", "GET")
            .body(axum::body::Body::empty())
            .expect("request should build");

        // SAFETY: not applicable; in-memory oneshot cannot fail.
        let response = match tower::ServiceExt::oneshot(app, request).await {
            Ok(response) => response,
            Err(error) => unreachable!("in-memory oneshot cannot fail: {error}"),
        };

        assert_eq!(
            response
                .headers()
                .get("access-control-allow-origin")
                .and_then(|value| value.to_str().ok()),
            expected_allow_origin
        );
    }

    /// Verifies that explicit origins, list origins, and private network
    /// settings parse correctly.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[tokio::test]
    async fn parses_cors_configuration_variants() {
        let json = concat!(
            r#"{"log_level":"info","listen":"127.0.0.1:0","database":{"path":"test.db","#,
            r#""max_connections":1,"pool_timeout":1},"management_api":{"token":"token","#,
            r#""cors":{"allowed_origin":"https://dashboard.example.com","#,
            r#""allow_private_network":true}},"#,
            r#""api":{"key":"key","cors":{"allowed_origin":[],"allow_private_network":false}}}"#,
        );

        #[expect(clippy::expect_used, reason = "valid configuration must parse")]
        let config: Config = serde_json::from_str(json).expect("valid configuration should parse");

        assert!(config.management_api.cors.allow_private_network);
        assert!(!config.api.cors.allow_private_network);

        assert_origin_allows(
            config.management_api.cors.allowed_origin,
            "https://dashboard.example.com",
            Some("https://dashboard.example.com"),
        )
        .await;

        assert_origin_allows(config.api.cors.allowed_origin, "https://any.com", None).await;
    }

    /// Verifies that `deserializer::allowed_origin` deserializes valid headers
    /// and rejects invalid header values directly.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[tokio::test]
    async fn allowed_origin_deserializer_directly() {
        use serde_json::json;

        // String wildcard:
        #[expect(clippy::expect_used, reason = "valid origin must deserialize")]
        let origin = super::deserializer::allowed_origin(json!("*")).expect("should parse");
        assert_origin_allows(origin, "https://any.com", Some("*")).await;

        // Sequence wildcard:
        #[expect(clippy::expect_used, reason = "valid origin must deserialize")]
        let origin = super::deserializer::allowed_origin(json!(["*"])).expect("should parse");
        assert_origin_allows(origin, "https://any.com", Some("*")).await;

        // Single explicit origin:
        #[expect(clippy::expect_used, reason = "valid origin must deserialize")]
        let origin = super::deserializer::allowed_origin(json!("https://example.com"))
            .expect("should parse");
        assert_origin_allows(
            origin.clone(),
            "https://example.com",
            Some("https://example.com"),
        )
        .await;
        assert_origin_allows(origin, "https://other.com", None).await;

        // Multiple explicit origins:
        #[expect(clippy::expect_used, reason = "valid origin must deserialize")]
        let origin = super::deserializer::allowed_origin(json!(["https://a.com", "https://b.com"]))
            .expect("should parse");
        assert_origin_allows(origin.clone(), "https://a.com", Some("https://a.com")).await;
        assert_origin_allows(origin.clone(), "https://b.com", Some("https://b.com")).await;
        assert_origin_allows(origin, "https://c.com", None).await;

        // Empty sequence:
        #[expect(clippy::expect_used, reason = "empty origin list must deserialize")]
        let origin = super::deserializer::allowed_origin(json!([])).expect("should parse");
        assert_origin_allows(origin, "https://any.com", None).await;

        // Invalid string:
        #[expect(clippy::expect_used, reason = "invalid origin must fail")]
        let error = super::deserializer::allowed_origin(json!("invalid\nheader"))
            .expect_err("should reject");
        assert!(error.to_string().contains("failed to parse header value"));

        // Invalid origin in sequence:
        #[expect(clippy::expect_used, reason = "invalid origin in list must fail")]
        let error = super::deserializer::allowed_origin(json!(["invalid\nheader"]))
            .expect_err("should reject");
        assert!(error.to_string().contains("failed to parse header value"));

        // Invalid type:
        #[expect(clippy::expect_used, reason = "number must fail")]
        let error = super::deserializer::allowed_origin(json!(123)).expect_err("should reject");
        assert!(
            error
                .to_string()
                .contains("expected an origin string or a list of origin strings")
        );
    }

    /// Verifies that an invalid header value in `allowed_origin` is rejected.
    ///
    /// # Panics
    ///
    /// Panics if the invalid origin parses unexpectedly.
    #[test]
    fn rejects_invalid_cors_origin_value() {
        let json = concat!(
            r#"{"log_level":"info","listen":"127.0.0.1:0","database":{"path":"test.db","#,
            r#""max_connections":1,"pool_timeout":1},"management_api":{"token":"token","#,
            r#""cors":{"allowed_origin":["invalid\norigin"],"allow_private_network":false}},"#,
            r#""api":{"key":"key","cors":{"allowed_origin":"*","allow_private_network":false}}}"#,
        );

        #[expect(
            clippy::expect_used,
            reason = "invalid configuration must fail to parse"
        )]
        let error = serde_json::from_str::<Config>(json).expect_err("should reject invalid origin");

        assert!(error.to_string().contains("failed to parse header value"));
    }

    /// Verifies that an unknown log level is rejected.
    ///
    /// # Panics
    ///
    /// Panics if the unknown log level parses unexpectedly.
    #[test]
    fn rejects_unknown_log_level_value() {
        #[expect(
            clippy::expect_used,
            reason = "invalid configuration must fail to parse"
        )]
        let error = serde_json::from_str::<Config>(&config_json("bogus", "key", "token"))
            .expect_err("should reject unknown level");

        assert!(error.to_string().contains("unknown variant `bogus`"));
    }
}
