//! Request context injected into every router handler.

use super::{database::Database, storage::api_logs::ApiLogStore};
use reqwest::Client;

/// State shared by every router handler: the database handle, the api-log store, and the
/// two upstream HTTP clients, one per traffic class.
#[derive(Clone)]
pub(crate) struct Context {
    /// Database handle all handlers execute against.
    pub(crate) database: Database,

    /// The api-log store, constructed at startup when the `api_logs` configuration section
    /// was present. `None` records nothing and reads as the empty store (README §3 *ApiLog
    /// Store*).
    pub(crate) api_log_store: Option<ApiLogStore>,

    /// Data-plane upstream client streaming long-lived completions.
    pub(crate) data_plane: Client,

    /// Control-plane upstream client for short upstream round-trips (OAuth token exchange and
    /// refresh, model discovery, project onboarding).
    pub(crate) control_plane: Client,
}
