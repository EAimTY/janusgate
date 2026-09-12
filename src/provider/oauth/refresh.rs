//! The lease-guarded token refresh loop: one request drives the database
//! refresh lease, the refresh grant, and the lease-conditioned persist, so
//! concurrent requests never overwrite newer credentials (README §3).

use super::{Error, flow, token};
use crate::{
    database::{Database, Error as DatabaseError, Transaction},
    provider::{Credentials, OAuthProviderType, Provider, storage},
    unix_time,
};
use log::{debug, info, warn};
use reqwest::Client;
use std::time::{Duration, Instant};

/// Margin subtracted from the token expiry when deciding freshness, so a
/// token is never used into its final seconds. The same margin participates
/// in the refresh-lease predicate, keeping "expired enough to refresh"
/// consistent with [`is_token_fresh`].
const TOKEN_FRESHNESS_MARGIN: i64 = 60;

/// Lease duration after which an abandoned token refresh is considered dead.
const REFRESH_LEASE_STALE_AFTER: Duration = Duration::from_secs(90);

/// Upper bound on how long a request waits for a concurrent token refresh to
/// finish before failing.
///
/// This must exceed [`REFRESH_LEASE_STALE_AFTER`]: a waiter outlives the
/// stale-lease window and therefore reaches the stale-lease reacquire path
/// promised by README §3 instead of failing while the lease abandoned by a
/// crashed holder is still live.
const REFRESH_WAIT_LIMIT: Duration = Duration::from_secs(100);

/// Interval between polls while waiting for a concurrent token refresh.
const REFRESH_WAIT_INTERVAL: Duration = Duration::from_millis(250);

// Compile-time guarantee that the wait limit outlives lease staleness.
const _: () = assert!(REFRESH_WAIT_LIMIT.as_secs() > REFRESH_LEASE_STALE_AFTER.as_secs());

// Compile-time guarantee that a stalled control-plane request aborts — and
// its lease clears — before the lease is considered stale, so an in-flight
// grant is never stolen from a live holder.
const _: () = assert!(
    crate::http_client::CONTROL_PLANE_TOTAL_TIMEOUT.as_secs() < REFRESH_LEASE_STALE_AFTER.as_secs()
);

/// The result of a lease-owned refresh attempt.
enum RefreshOutcome {
    /// The renewed credentials were persisted while the caller still owned
    /// the lease.
    Renewed(Provider),

    /// The lease was lost while the grant was in flight; the refreshed
    /// credentials were derived from superseded credentials and were not
    /// persisted, leaving the newer state — its lease included — untouched.
    LostLease,
}

/// Returns the provider with a valid access token, refreshing it under the
/// database lease when expired.
///
/// A refresh whose persist observes a lost lease — a concurrent credentials
/// replacement or a stale-lease reacquisition won the row — discards its
/// tokens-derived-from-superseded-credentials and re-enters the lease
/// competition, so the newer state is never overwritten (README §3). The
/// same holds for a lease acquire whose read snapshot a concurrent commit
/// invalidated: the engine refuses the read-then-write upgrade with the
/// stale-snapshot busy error, which is classified as "not acquired" so the
/// burst of concurrent acquirers takes the poll-and-wait path instead of
/// failing.
///
/// # Errors
///
/// Returns [`Error`] when the provider vanished, the refresh fails, or a
/// concurrent refresh does not finish in time.
pub(crate) async fn ensure_fresh_token(
    database: &Database,
    http: &Client,
    provider: &Provider,
) -> Result<Provider, Error> {
    if matches!(provider.credentials, Credentials::ApiKey(_)) || is_token_fresh(provider) {
        return Ok(provider.clone());
    }

    let wait_deadline = Instant::now() + REFRESH_WAIT_LIMIT;

    loop {
        let now = unix_time::unix_now();

        let mut transaction = database
            .transaction()
            .await
            .map_err(|error| Error::Internal {
                message: format!("failed to begin token refresh transaction: {error}"),
            })?;

        let Some(current) =
            storage::find_provider_by_id_in_transaction(&mut transaction, provider.id).await?
        else {
            transaction
                .rollback()
                .await
                .map_err(|error| Error::Internal {
                    message: format!("failed to roll back: {error}"),
                })?;

            return Err(Error::NotFound {
                message: "provider vanished during token refresh".to_string(),
            });
        };

        if is_token_fresh(&current) {
            transaction
                .rollback()
                .await
                .map_err(|error| Error::Internal {
                    message: format!("failed to roll back: {error}"),
                })?;

            return Ok(current);
        }

        let lease_started_at = match acquire_refresh_lease(&mut transaction, current.id, now).await
        {
            Ok(lease_started_at) => lease_started_at,

            // The acquire is a read-then-write transaction: the SELECT fixed
            // the read snapshot and the lease UPDATE upgrades it to a write.
            // A commit that landed in between invalidates the snapshot, and
            // the engine refuses the upgrade — that is the lease burst this
            // loop exists to absorb, not a failed request. The engine
            // requires a rollback before retrying, and the next iteration is
            // the retry: it re-reads the newer state and either finds the
            // token fresh or waits its turn.
            Err(lease_error) if lease_error.is_stale_snapshot() => {
                transaction
                    .rollback()
                    .await
                    .map_err(|error| Error::Internal {
                        message: format!("failed to roll back: {error}"),
                    })?;

                tokio::time::sleep(REFRESH_WAIT_INTERVAL).await;

                wait_or_fail(wait_deadline)?;

                continue;
            }

            Err(lease_error) => return Err(storage::Error::from(lease_error).into()),
        };

        transaction
            .commit()
            .await
            .map_err(|error| Error::Internal {
                message: format!("failed to commit refresh lease: {error}"),
            })?;

        if let Some(lease_started_at) = lease_started_at {
            // Grant failures return directly; losing the lease falls through
            // to re-read the newer state on the next iteration.
            if let RefreshOutcome::Renewed(renewed) =
                refresh_and_store(database, http, &current, lease_started_at).await?
            {
                return Ok(renewed);
            }
        } else {
            tokio::time::sleep(REFRESH_WAIT_INTERVAL).await;
        }

        wait_or_fail(wait_deadline)?;
    }
}

/// Attempts to acquire the token refresh lease inside the loop's open
/// transaction, binding the storage call to the loop's constants: a lease is
/// stale — and therefore stealable — when older than
/// [`REFRESH_LEASE_STALE_AFTER`], and the same [`TOKEN_FRESHNESS_MARGIN`] that
/// [`is_token_fresh`] applies decides that an expired-but-recently-expiring
/// token needs a refresh.
///
/// The raw [`DatabaseError`] is returned so the caller can classify a stale
/// snapshot as "not acquired" and retry through its poll loop: the acquire
/// upgrades the transaction's read snapshot to a write, and a concurrent
/// commit landing between the read and the write surfaces as the engine's
/// stale-snapshot busy error rather than a failed request.
///
/// # Errors
///
/// Returns the underlying database error when the acquire fails.
async fn acquire_refresh_lease(
    transaction: &mut Transaction,
    provider_id: i64,
    now: i64,
) -> Result<Option<i64>, DatabaseError> {
    storage::try_acquire_refresh_lease(
        transaction,
        provider_id,
        now,
        now - i64::try_from(REFRESH_LEASE_STALE_AFTER.as_secs()).unwrap_or(i64::MAX),
        TOKEN_FRESHNESS_MARGIN,
    )
    .await
}

/// Fails the request once the poll window for a concurrent token refresh has
/// elapsed (README §3); otherwise the caller re-enters the poll loop.
fn wait_or_fail(wait_deadline: Instant) -> Result<(), Error> {
    if Instant::now() >= wait_deadline {
        return Err(Error::Unavailable {
            detail: "timed out waiting for a concurrent token refresh".to_string(),
        });
    }

    Ok(())
}

/// Performs the refresh grant and persists the renewed credentials while the
/// caller still owns the refresh lease stamped `lease_started_at`.
///
/// Every error after the lease was acquired clears the lease before the
/// original error is returned, so concurrent waiters can acquire the lease
/// instead of burning their whole wait behind a refresh that already failed.
/// A failed lease clear never replaces the original error: the decisive
/// failure is the refresh or persistence error itself, and an uncleared lease
/// is bounded by the staleness window (README §3), so waiters still recover
/// by reacquiring it.
///
/// # Errors
///
/// Returns [`Error`] when the refresh or the persistence fails.
async fn refresh_and_store(
    database: &Database,
    http: &Client,
    provider: &Provider,
    lease_started_at: i64,
) -> Result<RefreshOutcome, Error> {
    let outcome = async {
        let (credentials, expires_at) = refresh_and_build_credentials(http, provider).await?;

        let is_persisted = storage::update_credentials_for_lease(
            database,
            provider.id,
            lease_started_at,
            &credentials,
            Some(expires_at),
        )
        .await?;

        if !is_persisted {
            return Ok(RefreshOutcome::LostLease);
        }

        info!(
            "refreshed the access token for provider `{}` (expires at {expires_at})",
            provider.name
        );

        let renewed = storage::reload_provider(database, provider.id, "token refresh").await?;

        Ok(RefreshOutcome::Renewed(renewed))
    }
    .await;

    if outcome.is_err() {
        // The original refresh failure is decisive; a failed lease clear —
        // typically the same database outage that failed the persist — must
        // not replace it. The clear failure itself is tolerated and bounded
        // by the lease staleness window (README §3), so the warning is the
        // only record it ever gets.
        if let Err(clear_error) = storage::clear_refresh_lease(database, provider.id).await {
            warn!(
                "failed to clear the refresh lease of provider `{}`: {clear_error}",
                provider.name
            );
        }
    }

    // The refreshed credentials were derived from superseded credentials
    // and were discarded; the lease competition re-entry that follows is
    // ordinary contention, so the discard is recorded at debug level.
    if let Ok(RefreshOutcome::LostLease) = &outcome {
        debug!(
            "lost the refresh lease of provider `{}`; discarding the refreshed credentials",
            provider.name
        );
    }

    outcome
}

/// Calls the provider's refresh grant and merges the result into new
/// credentials.
///
/// # Errors
///
/// Returns [`Error`] when the refresh grant fails.
async fn refresh_and_build_credentials(
    http: &Client,
    provider: &Provider,
) -> Result<(Credentials, i64), Error> {
    let refreshed = match &provider.credentials {
        Credentials::OpenAiOauth { refresh_token, .. } => {
            flow::refresh_tokens(http, OAuthProviderType::OpenAiOauth, refresh_token).await?
        }

        Credentials::AntigravityOauth { refresh_token, .. } => {
            flow::refresh_tokens(http, OAuthProviderType::AntigravityOauth, refresh_token).await?
        }

        Credentials::ApiKey(_) => {
            return Err(Error::Internal {
                message: "API key credentials never refresh".to_string(),
            });
        }
    };

    let access_token = refreshed.access_token.clone();

    // Computed before `access_token` is moved into the credentials.
    let expires_at = token::token_expiry_from(refreshed.expires_in, Some(&access_token))?;

    let credentials = match &provider.credentials {
        Credentials::OpenAiOauth {
            account_id,
            refresh_token: stored_refresh_token,
            ..
        } => Credentials::OpenAiOauth {
            account_id: refreshed
                .id_token
                .as_deref()
                .and_then(|id_token| {
                    token::jwt_claim(
                        id_token,
                        &["https://api.openai.com/auth", "chatgpt_account_id"],
                    )
                })
                .and_then(|claim| claim.as_str().map(str::to_string))
                .unwrap_or_else(|| account_id.clone()),
            access_token,
            refresh_token: refreshed
                .refresh_token
                .unwrap_or_else(|| stored_refresh_token.clone()),
        },

        Credentials::AntigravityOauth {
            project_id,
            refresh_token: stored_refresh_token,
            ..
        } => Credentials::AntigravityOauth {
            project_id: project_id.clone(),
            access_token,
            refresh_token: refreshed
                .refresh_token
                .unwrap_or_else(|| stored_refresh_token.clone()),
        },

        Credentials::ApiKey(_) => {
            return Err(Error::Internal {
                message: "API key credentials never refresh".to_string(),
            });
        }
    };

    Ok((credentials, expires_at))
}

/// Whether the provider's OAuth token is still valid long enough to use.
fn is_token_fresh(provider: &Provider) -> bool {
    provider
        .expires_at
        .is_some_and(|expires_at| expires_at > unix_time::unix_now() + TOKEN_FRESHNESS_MARGIN)
}
