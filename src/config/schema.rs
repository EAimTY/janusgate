//! Gateway configuration schema: pure data — the JSONC schema with serde
//! defaults and nothing else.

use log::LevelFilter;
use serde::Deserialize;
use serde_with::{As, DurationMilliSeconds};
use std::{net::SocketAddr, num::NonZero, path::PathBuf, time::Duration};
use tower_http::cors::AllowOrigin;

/// Root gateway configuration schema.
///
/// Required fields state operational facts or secrets the gateway must
/// never invent: `listen`, `database.path`, and `management_api.token`.
/// Gateway API keys live in the database and are managed through the
/// management API. Every other field carries the default documented on it and
/// may be omitted. Unknown fields are strictly rejected.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub(crate) struct Config {
    /// Minimum severity a log record must carry to be emitted to standard
    /// error. Defaults to `info`.
    #[serde(default = "super::defaults::log_level")]
    pub(crate) log_level: LevelFilter,

    /// IP address and TCP port on which the gateway listens for HTTP
    /// connections — an IP literal, never a DNS name, because a listener
    /// address is a local binding fact, not a name to resolve; the
    /// listener speaks TLS when `tls` is configured and plaintext HTTP
    /// otherwise.
    pub(crate) listen: SocketAddr,

    /// Inbound TLS termination; when this section is present the listener
    /// serves HTTPS, and when it is absent the listener serves plaintext
    /// HTTP.
    pub(crate) tls: Option<TlsConfig>,

    /// In-process SQLite database configuration.
    pub(crate) database: DatabaseConfig,

    /// Administrative management API configuration.
    pub(crate) management_api: ManagementApiConfig,

    /// Outward client API configuration.
    pub(crate) api: ApiConfig,

    /// Request accounting (`api_logs`) configuration. The section's presence is the recording
    /// switch: present, dispatched requests are recorded into the day-partitioned api-log
    /// store (README §3 *ApiLog Store*); absent, none are, while existing api-log files stay
    /// readable. The write path reads the presence live per dispatched request, so a reload
    /// applies to the next dispatch; the section itself is consumed once at startup when the
    /// api-log store is constructed, so adding it applies after restart.
    #[serde(default)]
    pub(crate) api_logs: Option<ApiLogsConfig>,
}

/// Inbound TLS termination schema.
///
/// The presence of this section selects HTTPS for the whole listener.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub(crate) struct TlsConfig {
    /// File path of the PEM certificate chain served to TLS clients, leaf
    /// certificate first.
    pub(crate) certificate: PathBuf,

    /// File path of the PEM private key matching the leaf certificate of
    /// the chain.
    pub(crate) private_key: PathBuf,
}

/// In-process SQLite database configuration.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub(crate) struct DatabaseConfig {
    /// File path to the in-process SQLite database file.
    pub(crate) path: PathBuf,

    /// Maximum number of concurrent database connections in the connection
    /// pool. Defaults to 4.
    #[serde(default = "super::defaults::max_connections")]
    pub(crate) max_connections: NonZero<usize>,

    /// Maximum time in milliseconds to wait for a free connection from the
    /// connection pool when every connection is checked out. Defaults to
    /// 5000.
    #[serde(
        with = "As::<DurationMilliSeconds>",
        default = "super::defaults::pool_timeout"
    )]
    pub(crate) pool_timeout: Duration,
}

/// Administrative management API configuration.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManagementApiConfig {
    /// Bearer-less administrative authorization token required by
    /// `/management-api/*`.
    pub(crate) token: String,

    /// Cross-Origin Resource Sharing (CORS) configuration for
    /// `/management-api/*`. Defaults to no cross-origin requests and no
    /// Private Network Access.
    #[serde(default)]
    pub(crate) cors: CorsConfig,
}

/// CORS configuration for an HTTP sub-router.
///
/// [`Default`] is the restrictive policy a section falls back to when its
/// `cors` object is omitted: [`AllowOrigin`]'s own default allows no
/// origin, and `allow_private_network` defaults to `false`.
#[derive(Debug, Deserialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct CorsConfig {
    /// Origin(s) permitted for cross-origin requests.
    ///
    /// Specifying `"*"` or `["*"]` allows any origin. Explicit origins
    /// (e.g. `"https://dashboard.example.com"` or
    /// `["https://dashboard.example.com"]`) restrict requests to those
    /// origins. An empty list `[]` permits no origins. Defaults to `[]`:
    /// no cross-origin requests.
    #[serde(deserialize_with = "super::deserializers::allowed_origin", default)]
    pub(crate) allowed_origin: AllowOrigin,

    /// Whether to allow requests from private networks (Private Network
    /// Access). Defaults to `false`.
    #[serde(default)]
    pub(crate) allow_private_network: bool,
}

/// Outward client protocol configuration: CORS only. Client authorization
/// secrets are database-managed API keys, not configuration.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub(crate) struct ApiConfig {
    /// Cross-Origin Resource Sharing (CORS) configuration for `/api/*`.
    /// Defaults to no cross-origin requests and no Private Network Access.
    #[serde(default)]
    pub(crate) cors: CorsConfig,
}

/// Request accounting configuration (`api_logs`, README §3 *ApiLog Store*).
///
/// Unknown fields are strictly rejected. The section itself is the recording switch and carries
/// no further switch field: present, rows are written; absent, none are.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub(crate) struct ApiLogsConfig {
    /// Directory holding the day-partitioned api-log files (`YYYY-MM-DD.db`). Consumed once at
    /// startup when the api-log store is constructed; a changed path applies after restart.
    pub(crate) path: PathBuf,

    /// Retention of the api-log files, in whole days: when a new api-log file is created, the
    /// sweep deletes files whose day-end is strictly older than now minus this many days.
    /// Defaults to 90.
    #[serde(default = "super::defaults::api_logs_retention_days")]
    pub(crate) retention_days: NonZero<u32>,
}

#[cfg(test)]
mod tests {
    use super::Config;
    use json_strip_comments::StripComments;
    use log::LevelFilter;
    use std::{fs, path::Path, time::Duration};

    /// Parses a schema from JSONC bytes.
    ///
    /// # Panics
    ///
    /// Panics if parsing fails; the tests feed valid configurations only.
    fn parse(bytes: &[u8]) -> Config {
        #[expect(clippy::expect_used, reason = "the tested body parses")]
        let schema: Config = parse_result(bytes).expect("the tested body parses");

        schema
    }

    /// Parses a schema from JSONC bytes, yielding the deserializer's rejection instead of
    /// panicking, for bodies whose rejection is what the test asserts.
    fn parse_result(bytes: &[u8]) -> Result<Config, serde_json::Error> {
        serde_json::from_reader(StripComments::new(bytes))
    }

    /// Verifies that `config.example.jsonc` parses into a schema with the TLS
    /// section configured, keeping the shipped example honest.
    ///
    /// # Panics
    ///
    /// Panics if the example file fails to read or parse.
    #[test]
    fn parses_the_example_configuration() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("config.example.jsonc");

        #[expect(clippy::expect_used, reason = "the example file reads")]
        let bytes = fs::read(path).expect("the example file reads");

        let schema = parse(&bytes);

        assert_eq!(schema.log_level, LevelFilter::Info);
        assert!(schema.tls.is_some());

        #[expect(clippy::expect_used, reason = "the example carries the section")]
        let api_logs = schema
            .api_logs
            .as_ref()
            .expect("the example carries api_logs");

        assert_eq!(api_logs.retention_days.get(), 90);
    }

    /// Verifies that every documented default applies when only the required
    /// fields are stated, which is the minimal valid configuration, and that
    /// a CORS section may also state one field alone.
    ///
    /// # Panics
    ///
    /// Panics if a minimal or partial configuration fails to parse or a
    /// default diverges.
    #[test]
    fn applies_defaults_to_a_minimal_configuration() {
        let schema = parse(
            br#"{
                "listen": "127.0.0.1:8080",
                "database": {"path": "janusgate.db"},
                "management_api": {"token": "management-token"},
                "api": {}
            }"#,
        );

        assert_eq!(schema.log_level, LevelFilter::Info);
        assert!(schema.tls.is_none());
        assert_eq!(schema.database.max_connections.get(), 4);
        assert_eq!(schema.database.pool_timeout, Duration::from_millis(5000));
        assert!(!schema.management_api.cors.allow_private_network);
        assert!(!schema.api.cors.allow_private_network);
        assert!(
            schema.api_logs.is_none(),
            "the omitted api_logs section means recording is off"
        );

        // The section's presence is the recording switch: present, it must state the api-log
        // directory, and retention may ride along or fall back to its default.
        let schema = parse(
            br#"{
                "listen": "127.0.0.1:8080",
                "database": {"path": "janusgate.db"},
                "management_api": {"token": "management-token"},
                "api": {},
                "api_logs": {"path": "api-logs"}
            }"#,
        );

        #[expect(clippy::expect_used, reason = "the section is present")]
        let api_logs = schema.api_logs.as_ref().expect("the section parses");

        assert_eq!(api_logs.path.to_string_lossy(), "api-logs");
        assert_eq!(
            api_logs.retention_days.get(),
            90,
            "retention defaults to 90"
        );

        let schema = parse(
            br#"{
                "listen": "127.0.0.1:8080",
                "database": {"path": "janusgate.db"},
                "management_api": {"token": "management-token"},
                "api": {},
                "api_logs": {"path": "api-logs", "retention_days": 7}
            }"#,
        );

        #[expect(clippy::expect_used, reason = "the section is present")]
        let api_logs = schema.api_logs.as_ref().expect("the section parses");

        assert_eq!(api_logs.retention_days.get(), 7);

        assert!(
            parse_result(
                br#"{
                    "listen": "127.0.0.1:8080",
                    "database": {"path": "janusgate.db"},
                    "management_api": {"token": "management-token"},
                    "api": {},
                    "api_logs": {"retention_days": 7}
                }"#
            )
            .is_err(),
            "a present api_logs section must state its path"
        );

        assert!(
            parse_result(
                br#"{
                    "listen": "127.0.0.1:8080",
                    "database": {"path": "janusgate.db"},
                    "management_api": {"token": "management-token"},
                    "api": {},
                    "api_logs": {"path": "api-logs", "retention_days": 0}
                }"#
            )
            .is_err(),
            "a zero retention must be rejected"
        );

        assert!(
            parse_result(
                br#"{
                    "listen": "127.0.0.1:8080",
                    "database": {"path": "janusgate.db"},
                    "management_api": {"token": "management-token"},
                    "api": {},
                    "api_logs": {"enabled": true}
                }"#
            )
            .is_err(),
            "an unknown api_logs field must be rejected"
        );

        let schema = parse(
            br#"{
                "listen": "127.0.0.1:8080",
                "database": {"path": "janusgate.db"},
                "management_api": {"token": "t", "cors": {"allow_private_network": true}},
                "api": {}
            }"#,
        );

        assert!(schema.management_api.cors.allow_private_network);
        assert!(!schema.api.cors.allow_private_network);
    }
}
