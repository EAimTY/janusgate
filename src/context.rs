//! Request context injected into every router handler.

use super::{database::Database, storage::api_logs::ApiLogStore};
use reqwest::Client;
use std::sync::Arc;

/// State shared by every router handler: persistence, accounting, per-traffic-class HTTP clients,
/// and the process-local quota and affinity coordinators.
#[derive(Clone)]
pub(crate) struct Context {
    /// Database handle all handlers execute against.
    pub(crate) database: Database,

    /// The api-log store, constructed once at startup from the resolved `api.logs` configuration
    /// section. `api.logs.is_enabled` is the live recording switch read per dispatched request,
    /// while the store's directory is consumed here (`docs/design/storage.md` *ApiLog Store*).
    pub(crate) api_log_store: ApiLogStore,

    /// Data-plane upstream client streaming long-lived completions.
    pub(crate) data_plane: Client,

    /// Control-plane upstream client for short upstream round-trips (OAuth token exchange and
    /// refresh, model discovery, project onboarding).
    pub(crate) control_plane: Client,

    /// Shared cancellation-resistant quota checks and current-generation refresh metadata.
    pub(crate) quota_coordinator: Arc<crate::provider::quota::coordinator::Coordinator>,

    /// One shared bounded digest cache and cancellation-safe account-establishment lifecycle.
    pub(crate) affinity_cache: Arc<crate::affinity::Cache>,
}
