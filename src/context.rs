//! Request context injected into every router handler.

use crate::database::Database;
use reqwest::Client;

/// State shared by every router handler: the database handle and the two
/// upstream HTTP clients, one per traffic class.
#[derive(Clone)]
pub(crate) struct Context {
    /// Database handle all handlers execute against.
    pub(crate) database: Database,

    /// Data-plane upstream client streaming long-lived completions.
    pub(crate) data_plane: Client,

    /// Control-plane upstream client for short upstream round-trips (OAuth
    /// token exchange and refresh, model discovery, project onboarding).
    pub(crate) control_plane: Client,
}
