//! OAuth authorization state persistence: the pending-flow lifecycle.

use super::Error;
use crate::{
    database::{Database, params},
    provider::OAuthProviderType,
};

/// One pending OAuth state row.
#[derive(Debug, Clone)]
pub(crate) struct OAuthState {
    /// Provider type the flow is for.
    pub(crate) state_type: OAuthProviderType,

    /// PKCE verifier, when the flow uses PKCE.
    pub(crate) verifier: String,
}

/// Inserts one pending OAuth state row.
///
/// # Errors
///
/// Returns [`Error`] when the insert fails.
pub(crate) async fn insert_oauth_state(
    database: &Database,
    state_type: OAuthProviderType,
    state: &str,
    verifier: &str,
    expires_at: i64,
) -> Result<(), Error> {
    database
        .execute(
            "INSERT INTO oauth_states (type, state, verifier, expires_at) VALUES (?1, ?2, ?3, ?4)",
            params![state_type, state, verifier, expires_at],
        )
        .await?;

    Ok(())
}

/// Reads the provider type of an unexpired OAuth state row without consuming it, so a completion
/// that cannot succeed can be rejected before the atomic consume spends the state.
///
/// # Errors
///
/// Returns [`Error`] when the query fails or the stored type is unknown.
pub(crate) async fn find_oauth_state_type(
    database: &Database,
    state: &str,
    now: i64,
) -> Result<Option<OAuthProviderType>, Error> {
    let rows = database
        .query(
            "SELECT type FROM oauth_states WHERE state = ?1 AND expires_at > ?2",
            params![state, now],
        )
        .await?;

    let Some(row) = rows.first() else {
        return Ok(None);
    };

    let state_type = super::oauth_provider_type_from_value(row.get_value(0)?)?;

    Ok(Some(state_type))
}

/// Consumes the OAuth state row for `state`, returning its facts only when it is unexpired.
///
/// Consumption is a single atomic `DELETE … RETURNING` statement, so exactly one concurrent
/// completion can ever observe the row and a state can never complete twice (README §3). An expired
/// or unknown state consumes nothing here; expired rows are swept separately by
/// [`delete_expired_oauth_states`]..
///
/// # Errors
///
/// Returns [`Error`] when the statement fails.
pub(crate) async fn consume_oauth_state(
    database: &Database,
    state: &str,
    now: i64,
) -> Result<Option<OAuthState>, Error> {
    let rows = database
        .query(
            "DELETE FROM oauth_states WHERE state = ?1 AND expires_at > ?2 \
             RETURNING type, verifier",
            params![state, now],
        )
        .await?;

    let Some(row) = rows.first() else {
        return Ok(None);
    };

    let state_type = super::oauth_provider_type_from_value(row.get_value(0)?)?;
    let verifier: String = row.get(1)?;

    Ok(Some(OAuthState {
        state_type,
        verifier,
    }))
}

/// Deletes every expired OAuth state row.
///
/// # Errors
///
/// Returns [`Error`] when the delete fails.
pub(crate) async fn delete_expired_oauth_states(
    database: &Database,
    now: i64,
) -> Result<(), Error> {
    database
        .execute(
            "DELETE FROM oauth_states WHERE expires_at <= ?1",
            params![now],
        )
        .await?;

    Ok(())
}
