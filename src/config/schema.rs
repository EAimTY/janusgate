//! Gateway configuration schema: pure data — the JSONC schema with serde
//! defaults and nothing else.

use log::LevelFilter;
use serde::Deserialize;
use std::{
    fmt::{self, Formatter},
    net::SocketAddr,
    num::NonZero,
    path::PathBuf,
};
use tower_http::cors::AllowOrigin;
use vec1::Vec1;

/// Root gateway configuration schema.
///
/// Every field carries the default documented on it and may be omitted: the empty object is a
/// valid configuration serving the outward API with request recording on and the management API
/// disabled. The management API exists only while its section is configured, and its `token` is
/// then required — it is a secret the gateway must never invent. Gateway API keys live in the
/// database and are managed through the management API. Unknown fields are strictly rejected.
///
/// Every relative path in the schema — `database.path`, `api.logs.path`, and the TLS material —
/// resolves against the configuration file's directory, never against the process's current
/// directory.
#[derive(Debug, Deserialize)]
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
    /// otherwise. Defaults to `127.0.0.1:4718`: loopback only, so a
    /// defaulted gateway is never exposed beyond its host.
    #[serde(default = "super::defaults::listen")]
    pub(crate) listen: SocketAddr,

    /// Inbound TLS termination; when this section is present the listener
    /// serves HTTPS, and when it is absent the listener serves plaintext
    /// HTTP.
    pub(super) tls: Option<TlsConfig>,

    /// In-process SQLite database configuration. Defaults as a whole.
    #[serde(default)]
    pub(crate) database: DatabaseConfig,

    /// Administrative management API configuration. The section's presence is the switch for
    /// the whole management API: present, `/management-api/*` is mounted and its `token` is
    /// required; absent, the management API does not exist. The mount is decided at startup —
    /// adding or removing the section applies after a restart — while a configured token
    /// reloads live.
    #[serde(default)]
    pub(crate) management_api: Option<ManagementApiConfig>,

    /// Outward client API configuration. Defaults as a whole.
    #[serde(default)]
    pub(crate) api: ApiConfig,

    /// Remote-compaction sealing keys. The section's presence is the switch for the gateway's
    /// sealed compaction artifacts: present, gateway summaries and wrapped native windows are
    /// sealed under the configured keys; absent, any operation that would produce or consume a
    /// sealed carrier fails while native passthrough keeps working. The first key seals; every
    /// configured key unseals by its carrier-carried identifier, so rotation prepends a new active
    /// key while retaining old entries. Removing a key intentionally invalidates its
    /// artifacts.
    #[serde(default)]
    pub(super) compaction: Option<CompactionConfig>,

    /// Live quota refresh cadence and bounded content-free pool affinity.
    #[serde(default)]
    pub(crate) pooling: PoolingConfig,
}

/// Positive pool runtime limits, defaulted as a whole and field by field.
#[derive(Debug, Deserialize)]
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
#[derive(Debug, Deserialize)]
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
#[derive(Deserialize)]
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
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TlsConfig {
    /// File path of the PEM certificate chain served to TLS clients, leaf
    /// certificate first; resolved beside the configuration file when
    /// relative.
    pub(super) certificate: PathBuf,

    /// File path of the PEM private key matching the leaf certificate of
    /// the chain; resolved beside the configuration file when relative.
    pub(super) private_key: PathBuf,
}

/// In-process SQLite database configuration.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DatabaseConfig {
    /// File path to the in-process SQLite database file; resolved beside the
    /// configuration file when relative. Defaults to `janusgate.db`.
    #[serde(default = "super::defaults::database_path")]
    pub(crate) path: PathBuf,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            path: super::defaults::database_path(),
        }
    }
}

/// Administrative management API configuration.
///
/// The section's presence is the switch for the whole management API: the routes are mounted
/// only when the section is configured at startup, so adding or removing the section applies
/// after a restart, while the token reloads live. Its absence disables the management API
/// entirely.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManagementApiConfig {
    /// Bearer-less administrative authorization token required by
    /// `/management-api/*`.
    pub(crate) token: String,

    /// Cross-Origin Resource Sharing (CORS) configuration for
    /// `/management-api/*`. Defaults to no cross-origin requests and no
    /// Private Network Access.
    #[serde(default)]
    pub(super) cors: CorsConfig,
}

impl fmt::Debug for ManagementApiConfig {
    // The token is a secret: the rendering names the token's presence and length, never its
    // value.
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ManagementApiConfig")
            .field(
                "token",
                &format_args!("<secret: {} characters>", self.token.len()),
            )
            .field("cors", &self.cors)
            .finish()
    }
}

/// CORS configuration for an HTTP sub-router.
///
/// [`Default`] is the restrictive policy a section falls back to when its
/// `cors` object is omitted: [`AllowOrigin`]'s own default allows no
/// origin, and `allow_private_network` defaults to `false`.
#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(super) struct CorsConfig {
    /// Origin(s) permitted for cross-origin requests.
    ///
    /// Specifying `"*"` or `["*"]` allows any origin. Explicit origins
    /// (e.g. `"https://dashboard.example.com"` or
    /// `["https://dashboard.example.com"]`) restrict requests to those
    /// origins. An empty list `[]` permits no origins. Defaults to `[]`:
    /// no cross-origin requests.
    #[serde(deserialize_with = "super::deserializers::allowed_origin", default)]
    pub(super) allowed_origin: AllowOrigin,

    /// Whether to allow requests from private networks (Private Network
    /// Access). Defaults to `false`.
    #[serde(default)]
    pub(super) allow_private_network: bool,
}

/// Outward client protocol configuration: CORS and request accounting.
/// Client authorization secrets are database-managed API keys, not
/// configuration.
#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct ApiConfig {
    /// Cross-Origin Resource Sharing (CORS) configuration for `/api/*`.
    /// Defaults to no cross-origin requests and no Private Network Access.
    #[serde(default)]
    pub(super) cors: CorsConfig,

    /// Request accounting configuration (`api.logs`, `docs/design/storage.md` *ApiLog Store*).
    /// Defaults as a whole: recording on, the day files under `api_logs` beside the
    /// configuration file, and 90 days of retention.
    #[serde(default)]
    pub(crate) logs: LogsConfig,
}

/// Request accounting configuration (`api.logs`, `docs/design/storage.md` *ApiLog Store*).
///
/// `is_enabled` is the recording switch: `true`, dispatched requests are recorded into the
/// day-partitioned api-log store; `false`, none are, while existing api-log files stay readable
/// through the management usage reads. The switch is read live per dispatched request, so a
/// reload applies to the very next dispatch; the directory is consumed once at startup when the
/// api-log store is constructed, so a changed path applies after restart.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LogsConfig {
    /// Whether dispatched requests are recorded into the day-partitioned
    /// api-log store. Defaults to `true`.
    #[serde(default = "super::defaults::logs_is_enabled")]
    pub(crate) is_enabled: bool,

    /// Directory holding the day-partitioned api-log files (`YYYY-MM-DD.db`);
    /// resolved beside the configuration file when relative. Consumed once at
    /// startup when the api-log store is constructed; a changed path applies
    /// after restart. Defaults to `api_logs`.
    #[serde(default = "super::defaults::logs_path")]
    pub(crate) path: PathBuf,

    /// Retention of the api-log files, in whole days: when a new api-log file is created, the
    /// sweep deletes files whose day-end is strictly older than now minus this many days.
    /// Defaults to 90.
    #[serde(default = "super::defaults::logs_retention_days")]
    pub(crate) retention_days: NonZero<u32>,
}

impl Default for LogsConfig {
    fn default() -> Self {
        Self {
            is_enabled: super::defaults::logs_is_enabled(),
            path: super::defaults::logs_path(),
            retention_days: super::defaults::logs_retention_days(),
        }
    }
}
