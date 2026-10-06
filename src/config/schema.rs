//! Gateway configuration schema: pure data — the JSONC schema with serde
//! defaults and nothing else.

use log::LevelFilter;
use serde::Deserialize;
use serde_with::{As, DurationMilliSeconds};
use std::{
    fmt::{self, Formatter},
    net::SocketAddr,
    num::NonZero,
    path::PathBuf,
    time::Duration,
};
use tower_http::cors::AllowOrigin;
use vec1::Vec1;

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

    /// Remote-compaction sealing keys. The section's presence is the switch for the gateway's
    /// sealed compaction artifacts: present, gateway summaries and wrapped native windows are
    /// sealed under the configured keys; absent, any operation that would produce or consume a
    /// sealed carrier fails while native passthrough keeps working. The first key seals; every
    /// configured key unseals by its carrier-carried identifier, so keys rotate by appending.
    /// Removing a key intentionally invalidates its artifacts.
    #[serde(default)]
    pub(crate) compaction: Option<CompactionConfig>,

    /// Live quota refresh cadence and bounded content-free pool affinity.
    #[serde(default)]
    pub(crate) pooling: PoolingConfig,
}

/// Positive pool runtime limits, defaulted as a whole and field by field.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub(crate) struct PoolingConfig {
    /// Automatic quota check interval, in whole seconds; defaults to 60.
    #[serde(default = "super::defaults::quota_refresh_interval_seconds")]
    pub(crate) quota_refresh_interval_seconds: NonZero<u32>,
    /// Historical anchor idle lifetime, in whole seconds; defaults to 86400.
    #[serde(default = "super::defaults::sticky_idle_timeout_seconds")]
    pub(crate) sticky_idle_timeout_seconds: NonZero<u32>,
    /// Exact fresh-request retry lifetime, in whole seconds; defaults to 60.
    #[serde(default = "super::defaults::sticky_retry_timeout_seconds")]
    pub(crate) sticky_retry_timeout_seconds: NonZero<u32>,
    /// Maximum counted cache entries; defaults to 100000.
    #[serde(default = "super::defaults::sticky_maximum_entries")]
    pub(crate) sticky_maximum_entries: NonZero<usize>,
}

impl Default for PoolingConfig {
    fn default() -> Self {
        Self {
            quota_refresh_interval_seconds: super::defaults::quota_refresh_interval_seconds(),
            sticky_idle_timeout_seconds: super::defaults::sticky_idle_timeout_seconds(),
            sticky_retry_timeout_seconds: super::defaults::sticky_retry_timeout_seconds(),
            sticky_maximum_entries: super::defaults::sticky_maximum_entries(),
        }
    }
}

/// Remote-compaction sealing key configuration.
///
/// Deserialization enforces the whole key-set contract and decodes every key to its material, so
/// an invalid set fails the parse with the offending value's line and column — exactly like any
/// other schema rejection — and a parsed section is valid by construction.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub(crate) struct CompactionConfig {
    /// Configured keys in rotation order: the first entry seals, every entry unseals by its
    /// identifier. At least one key is required; identifiers must be unique, non-empty, and
    /// free of `.`; each key is standard base64 of exactly 32 bytes. Deserialization enforces
    /// the whole contract, rejecting an empty set and repeated identifiers.
    #[serde(deserialize_with = "super::deserializers::compaction_keys")]
    pub(crate) keys: Vec1<CompactionKeyConfig>,
}

/// The decoded byte length of every compaction sealing key's material.
pub(super) const SEALING_KEY_BYTES: usize = 32;

/// One configured sealing key: its carrier identifier and its decoded material.
#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub(crate) struct CompactionKeyConfig {
    /// The key's carrier identifier, ridden in the clear by every carrier it
    /// sealed. Non-empty and free of `.`, the carrier's key-id separator.
    #[serde(deserialize_with = "super::deserializers::sealing_key_id")]
    pub(crate) id: String,

    /// The key material: the [`SEALING_KEY_BYTES`] bytes the standard-base64 configuration value
    /// decodes to.
    #[serde(deserialize_with = "super::deserializers::sealing_key_material")]
    pub(crate) key: [u8; SEALING_KEY_BYTES],
}

impl fmt::Debug for CompactionKeyConfig {
    // The material is a secret: the rendering names the identifier and the material's presence,
    // never its bytes.
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CompactionKeyConfig")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
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
        assert!(
            schema.compaction.is_none(),
            "the omitted compaction section means sealing is unavailable"
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

    /// Renders a configuration body carrying the given `compaction.keys` JSON fragment.
    fn sealing_keys_body(keys: &str) -> String {
        format!(
            r#"{{
                "listen": "127.0.0.1:8080",
                "database": {{"path": "janusgate.db"}},
                "management_api": {{"token": "management-token"}},
                "api": {{}},
                "compaction": {{"keys": {keys}}}
            }}"#
        )
    }

    /// Verifies the compaction sealing-key contract at parse time: a valid set parses into
    /// identifiers and decoded 32-byte materials, and an empty set, a repeated, empty, or
    /// separator-carrying identifier, a non-base64 or wrong-length key value, and an unknown
    /// entry field are all rejected — with the offending value's location, like any other
    /// schema rejection.
    ///
    /// # Panics
    ///
    /// Panics if a valid set fails to parse, an invalid set parses, or a rejection omits the
    /// location.
    #[test]
    fn enforces_the_sealing_key_contract_at_parse_time() {
        let key = crate::test_support::FIXTURE_SEALING_KEY;
        let rotated = crate::test_support::FIXTURE_SEALING_KEY_ROTATED;

        let schema = parse(
            sealing_keys_body(&format!(
                r#"[{{"id": "primary", "key": "{key}"}}, {{"id": "spare", "key": "{rotated}"}}]"#
            ))
            .as_bytes(),
        );

        #[expect(clippy::expect_used, reason = "the valid set carries the section")]
        let compaction = schema.compaction.as_ref().expect("the section parses");

        assert_eq!(compaction.keys.len(), 2);
        assert_eq!(compaction.keys.first().id, "primary");
        assert_eq!(
            compaction.keys.first().key,
            std::array::from_fn(|index| index as u8),
            "the base64 value decodes to its material"
        );

        let repeated =
            format!(r#"[{{"id": "a", "key": "{key}"}}, {{"id": "a", "key": "{rotated}"}}]"#);

        for (index, (label, keys)) in [
            ("empty set", String::from("[]")),
            ("repeated id", repeated),
            (
                "separator id",
                format!(r#"[{{"id": "a.b", "key": "{key}"}}]"#),
            ),
            ("empty id", format!(r#"[{{"id": "", "key": "{key}"}}]"#)),
            (
                "non-base64 key",
                r#"[{"id": "a", "key": "!!!"}]"#.to_string(),
            ),
            ("short key", r#"[{"id": "a", "key": "AAAA"}]"#.to_string()),
            (
                "unknown entry field",
                format!(r#"[{{"id": "a", "key": "{key}", "comment": 1}}]"#),
            ),
        ]
        .into_iter()
        .enumerate()
        {
            assert!(
                parse_result(sealing_keys_body(&keys).as_bytes()).is_err(),
                "case {index} ({label}) parses but must be rejected"
            );
        }

        #[expect(
            clippy::panic,
            reason = "the repeated-id set is rejected above, which is what the test asserts"
        )]
        let Err(failure) = parse_result(
            sealing_keys_body(&format!(
                r#"[{{"id": "a", "key": "{key}"}}, {{"id": "a", "key": "{rotated}"}}]"#
            ))
            .as_bytes(),
        ) else {
            panic!("a repeated id parses but must be rejected");
        };

        let rendered = failure.to_string();

        assert!(rendered.contains("repeated"), "{rendered}");
        assert!(rendered.contains("line"), "{rendered}");
    }
}
