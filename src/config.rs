//! Gateway configuration schema and custom deserializers.

use host_port_pair::HostPortPair;
use log::LevelFilter;
use nestify::nest;
use serde::Deserialize;
use serde_with::{As, DurationMilliSeconds};
use std::{num::NonZero, path::PathBuf, time::Duration};
use tower_http::cors::{AllowOrigin, Any, CorsLayer};

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

impl CorsConfig {
    /// Renders this configuration into the tower-http CORS layer both HTTP
    /// sub-routers apply: the configured origins and Private Network Access
    /// policy, with every request method and header allowed.
    pub(crate) fn cors_layer(&self) -> CorsLayer {
        CorsLayer::new()
            .allow_origin(self.allowed_origin.clone())
            .allow_private_network(self.allow_private_network)
            .allow_methods(Any)
            .allow_headers(Any)
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
