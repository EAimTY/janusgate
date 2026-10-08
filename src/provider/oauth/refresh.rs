//! The lease-guarded token renewal: one request drives the database refresh lease, the
//! definition's refresh mode — a refresh grant or a reprovisioning — and the lease-conditioned
//! persist, so concurrent requests never overwrite newer credentials (README §3).
//!
//! The lease lives inside the credentials JSON document and every transition conditions on
//! `json_extract(credentials, …)`; the engine never read–modify–writes a stale document in Rust.
//! Only a known, elapsed `expires_at` initiates time-based refresh: a `null` expiry never does,
//! and `refresh: none` never enters the lease. A renewable row whose expiry is `null` is
//! therefore dispatched as-is — the lease cannot be granted for it, so the loop returns the
//! freshly read row instead of waiting for a concurrent refresher that can never exist.

use super::{
    exchange::{absorb_exchange_slots, execute_refresh_grant, resolve_expiry},
    provisioning::{self, WorkingState},
};
use crate::{
    database::{Database, Error as DatabaseError, Transaction},
    http_client,
    provider::{
        Error, Provider, StoredCredentials,
        definition::{
            credential::{OAuthRecipe, TokenRefresh},
            eval::ValueEnv,
        },
    },
    storage,
    storage::Error as StorageError,
    unix_time,
};
use log::{debug, info, warn};
use reqwest::Client;
use std::time::{Duration, Instant};
use tokio::time as tokio_time;

/// Margin subtracted from the token expiry when deciding freshness, in database-clock
/// microseconds, so a token is never used into its final seconds. The same margin participates
/// in the refresh-lease predicate, keeping "expired enough to refresh" consistent with
/// [`is_token_fresh`].
const TOKEN_FRESHNESS_MARGIN: i64 = 60 * unix_time::MICROS_PER_SEC;

/// Lease duration after which an abandoned token refresh is considered dead.
const REFRESH_LEASE_STALE_AFTER: Duration = Duration::from_secs(90);

/// Upper bound on how long a request waits for a concurrent token refresh to finish before
/// failing.
///
/// This must exceed [`REFRESH_LEASE_STALE_AFTER`]: a waiter outlives the stale-lease window and
/// therefore reaches the stale-lease reacquire path instead of failing while the lease abandoned
/// by a crashed holder is still live.
const REFRESH_WAIT_LIMIT: Duration = Duration::from_secs(100);

/// Interval between polls while waiting for a concurrent token refresh.
const REFRESH_WAIT_INTERVAL: Duration = Duration::from_millis(250);

// Compile-time guarantee that the wait limit outlives lease staleness.
const _: () = assert!(REFRESH_WAIT_LIMIT.as_secs() > REFRESH_LEASE_STALE_AFTER.as_secs());

// Compile-time guarantee that a stalled control-plane request aborts — and its lease clears —
// before the lease is considered stale, so an in-flight grant is never stolen from a live holder.
const _: () = assert!(
    http_client::CONTROL_PLANE_TOTAL_TIMEOUT.as_secs() < REFRESH_LEASE_STALE_AFTER.as_secs()
);

/// The result of a lease-owned refresh attempt.
#[allow(
    clippy::large_enum_variant,
    reason = "one control-plane refresh result is moved to its caller, never stored in a collection"
)]
enum RefreshOutcome {
    /// The renewed credentials were persisted while the caller still owned the lease.
    Renewed(Provider),

    /// The lease was lost while the grant was in flight; the refreshed credentials were derived
    /// from superseded credentials and were not persisted, leaving the newer state — its lease
    /// included — untouched.
    LostLease,
}

/// Whether the provider's OAuth token is still valid long enough to use. A `null` expiry is the
/// no-scheduled-refresh state, never a stale one: the token is sent until the upstream rejects
/// it or an explicit credential update or reauthorization occurs.
#[must_use]
fn is_token_fresh(provider: &Provider) -> bool {
    match &provider.credentials {
        StoredCredentials::OAuth { expires_at, .. } => expires_at.is_some_and(|expires_at| {
            expires_at > unix_time::unix_now_micros() + TOKEN_FRESHNESS_MARGIN
        }),
        StoredCredentials::ApiKey { .. } => true,
    }
}

/// Whether the provider's credentials are OAuth ones whose definition renews through a refresh
/// grant or reprovisioning. Static keys never refresh, and `refresh: none` never enters the
/// lease.
#[must_use]
fn is_renewable(provider: &Provider) -> bool {
    let StoredCredentials::OAuth { .. } = &provider.credentials else {
        return false;
    };

    provider
        .definition
        .oauth_recipe()
        .is_some_and(|recipe| !matches!(recipe.refresh, TokenRefresh::None))
}

/// Whether the provider's credentials carry a refresh schedule at all: a known `expires_at`. A
/// `null` expiry is the no-scheduled-refresh state — the lease is never granted for it, because
/// the acquire requires a known expiry on both of its arms — so a renewable row in this state
/// is dispatched as-is: waiting for a concurrent refresher would poll a lease that can never be
/// granted until the wait limit fails every dispatch (README §3).
#[must_use]
fn is_refresh_scheduled(provider: &Provider) -> bool {
    match &provider.credentials {
        StoredCredentials::OAuth { expires_at, .. } => expires_at.is_some(),
        StoredCredentials::ApiKey { .. } => false,
    }
}

/// Returns the provider with a valid access token, renewing it under the database lease when
/// expired.
///
/// A refresh whose persist observes a lost lease — a concurrent credentials replacement or a
/// stale-lease reacquisition won the row — discards its tokens-derived-from-superseded-
/// credentials and re-enters the lease competition, so the newer state is never overwritten. The
/// same holds for a lease acquire whose read snapshot a concurrent commit invalidated: the engine
/// refuses the write upgrade with the stale-snapshot busy error, which is classified as "lease
/// not acquired" so the burst of simultaneous acquirers on one expired token waits its turn
/// instead of failing. A freshly read row whose expiry is `null` — the no-scheduled-refresh
/// state, for which the lease can never be granted — is returned as-is: the token is sent until
/// the upstream rejects it, never waited on.
///
/// # Errors
///
/// Returns [`Error`] when the provider vanished, the renewal fails, or a concurrent refresh does
/// not finish in time.
pub(crate) async fn ensure_fresh_token(
    database: &Database,
    http: &Client,
    provider: &Provider,
) -> Result<Provider, Error> {
    if !is_renewable(provider) || is_token_fresh(provider) {
        return Ok(provider.clone());
    }

    let wait_deadline = Instant::now() + REFRESH_WAIT_LIMIT;

    loop {
        let now = unix_time::unix_now_micros();

        let mut transaction = database
            .transaction()
            .await
            .map_err(|error| Error::Internal {
                message: format!("failed to begin token refresh transaction: {error}"),
            })?;

        let Some(current) =
            storage::providers::find_provider_by_id_in_transaction(&mut transaction, provider.id)
                .await?
        else {
            rollback(transaction).await?;

            return Err(Error::NotFound {
                message: "provider vanished during token refresh".to_string(),
            });
        };

        // The re-read is authoritative for all three short-circuits: fresh tokens and
        // `refresh: none` need no lease, and a `null` expiry is the no-scheduled-refresh state
        // the dispatch sends as-is — the acquire would refuse it on both arms, so reading its
        // refusal as "a live holder exists" would wedge every dispatch until the wait limit.
        if !is_renewable(&current) || is_token_fresh(&current) || !is_refresh_scheduled(&current) {
            rollback(transaction).await?;

            return Ok(current);
        }

        let lease_started_at = match acquire_refresh_lease(&mut transaction, current.id, now).await
        {
            Ok(lease_started_at) => lease_started_at,

            // The acquire is a read-then-write transaction: the SELECT fixed the read snapshot and
            // the lease UPDATE upgrades it to a write. A commit that landed in between invalidates
            // the snapshot, and the engine refuses the upgrade — that is the lease burst this loop
            // exists to absorb, not a failed request. The engine requires a rollback before
            // retrying, and the next iteration is the retry: it re-reads the newer state and
            // either finds the token fresh or waits its turn.
            Err(lease_error) if lease_error.is_stale_snapshot() => {
                rollback(transaction).await?;

                tokio_time::sleep(REFRESH_WAIT_INTERVAL).await;

                wait_or_fail(wait_deadline)?;

                continue;
            }

            Err(lease_error) => return Err(StorageError::from(lease_error).into()),
        };

        transaction
            .commit()
            .await
            .map_err(|error| Error::Internal {
                message: format!("failed to commit refresh lease: {error}"),
            })?;

        if let Some(lease_started_at) = lease_started_at {
            // Grant failures return directly; losing the lease falls through to re-read the
            // newer state on the next iteration.
            if let RefreshOutcome::Renewed(renewed) =
                renew_and_store(database, http, &current, lease_started_at).await?
            {
                return Ok(renewed);
            }
        } else {
            tokio_time::sleep(REFRESH_WAIT_INTERVAL).await;
        }

        wait_or_fail(wait_deadline)?;
    }
}

/// Rolls back the loop's open transaction, consuming the handle. A failed rollback is classified
/// like every other failed transaction step.
///
/// # Errors
///
/// Returns [`Error`] when the rollback fails.
async fn rollback(transaction: Transaction) -> Result<(), Error> {
    transaction
        .rollback()
        .await
        .map_err(|error| Error::Internal {
            message: format!("failed to roll back: {error}"),
        })
}

/// Attempts to acquire the token refresh lease inside the loop's open transaction, binding the
/// storage call to the loop's constants: a lease is stale — and therefore stealable — when older
/// than [`REFRESH_LEASE_STALE_AFTER`], and the same [`TOKEN_FRESHNESS_MARGIN`] that
/// [`is_token_fresh`] applies decides that an expired-but-recently-expiring token needs a
/// renewal.
///
/// The raw [`DatabaseError`] is returned so the caller can classify a stale snapshot as "not
/// acquired" and retry through its poll loop.
///
/// # Errors
///
/// Returns the underlying database error when the acquire fails.
async fn acquire_refresh_lease(
    transaction: &mut Transaction,
    provider_id: i64,
    now: i64,
) -> Result<Option<i64>, DatabaseError> {
    storage::providers::try_acquire_refresh_lease(
        transaction,
        provider_id,
        now,
        now - i64::try_from(REFRESH_LEASE_STALE_AFTER.as_micros()).unwrap_or(i64::MAX),
        TOKEN_FRESHNESS_MARGIN,
    )
    .await
}

/// Fails the request once the poll window for a concurrent token refresh has elapsed;
/// otherwise the caller re-enters the poll loop.
fn wait_or_fail(wait_deadline: Instant) -> Result<(), Error> {
    if Instant::now() >= wait_deadline {
        return Err(Error::Unavailable {
            detail: "timed out waiting for a concurrent token refresh".to_string(),
        });
    }

    Ok(())
}

/// Performs the definition's renewal — the refresh grant or the reprovisioning — and persists the
/// renewed state while the caller still owns the refresh lease stamped `lease_started_at`.
///
/// Every error after the lease was acquired clears the lease before the original error is
/// returned, so concurrent waiters can acquire the lease instead of burning their whole wait
/// behind a refresh that already failed. A failed lease clear never replaces the original error:
/// an uncleared lease is bounded by the staleness window, so waiters still recover by
/// reacquiring it.
///
/// # Errors
///
/// Returns [`Error`] when the renewal or the persistence fails.
async fn renew_and_store(
    database: &Database,
    http: &Client,
    provider: &Provider,
    lease_started_at: i64,
) -> Result<RefreshOutcome, Error> {
    let renewal = match renew_and_build_state(http, provider).await {
        Ok(renewal) => renewal,

        Err(error) => {
            // The renewal failure is decisive; the clear conditions on the caller's own
            // acquired stamp, so a caller that lost the lease meanwhile never erases the
            // winner's live stamp — that not-owned case is ordinary contention and ignored.
            clear_lease_tolerating_failure(database, provider, lease_started_at).await;

            return Err(error);
        }
    };

    let is_persisted = match persistence(database, provider.id, lease_started_at, &renewal).await {
        Ok(is_persisted) => is_persisted,

        Err(error) => {
            // The persistence failure gets the same clear-on-error treatment as the renewal
            // failure: a persist that errors must not leave the caller's stamp riding on an
            // expired token, where concurrent waiters would poll a dead lease up to the
            // staleness window.
            clear_lease_tolerating_failure(database, provider, lease_started_at).await;

            return Err(error);
        }
    };

    if !is_persisted {
        // The renewed credentials were derived from superseded credentials and were discarded;
        // the lease competition re-entry that follows is ordinary contention, so the discard is
        // recorded at debug level.
        debug!(
            "lost the refresh lease of provider `{}`; discarding the renewed credentials",
            provider.name
        );

        return Ok(RefreshOutcome::LostLease);
    }

    let expires_at = match &renewal {
        Renewal::Grant { credentials } => match credentials {
            StoredCredentials::OAuth { expires_at, .. } => *expires_at,
            StoredCredentials::ApiKey { .. } => None,
        },
        Renewal::Reprovision { expires_at, .. } => Some(*expires_at),
    };

    info!(
        "renewed the credentials of provider `{}` (expires at {expires_at:?})",
        provider.name
    );

    let renewed =
        storage::providers::reload_provider(database, provider.id, "token renewal").await?;

    Ok(RefreshOutcome::Renewed(renewed))
}

/// Clears the caller's own refresh lease after a failed renewal or persistence step, so
/// concurrent waiters can acquire the lease instead of burning their whole wait behind a
/// refresh that already failed. The clear conditions on the caller's own acquired stamp, so a
/// caller that lost the lease meanwhile never erases the winner's live stamp; that not-owned
/// case is ordinary contention and ignored here. A failed clear — typically the same database
/// outage that failed the step — must not replace the original error: an uncleared lease is
/// bounded by the staleness window, so waiters still recover by reacquiring it, and the
/// warning is the only record the failed clear ever gets.
async fn clear_lease_tolerating_failure(
    database: &Database,
    provider: &Provider,
    lease_started_at: i64,
) {
    if let Err(clear_error) =
        storage::providers::clear_refresh_lease(database, provider.id, lease_started_at).await
    {
        warn!(
            "failed to clear the refresh lease of provider `{}`: {clear_error}",
            provider.name
        );
    }
}

/// The persistence arm of [`renew_and_store`]: a grant renewal writes the whole merged
/// credentials document under the lease; a reprovisioning writes its fields over the stored
/// document in SQL.
async fn persistence(
    database: &Database,
    provider_id: i64,
    lease_started_at: i64,
    renewal: &Renewal,
) -> Result<bool, Error> {
    match renewal {
        Renewal::Grant { credentials } => {
            storage::providers::persist_renewed_credentials_for_lease(
                database,
                provider_id,
                lease_started_at,
                credentials,
            )
            .await
            .map_err(Error::from)
        }

        Renewal::Reprovision {
            access_token,
            expires_at,
            refresh_token,
            base_url,
        } => storage::providers::persist_reprovision_for_lease(
            database,
            provider_id,
            lease_started_at,
            access_token,
            Some(*expires_at),
            refresh_token.as_deref(),
            base_url.as_deref(),
        )
        .await
        .map_err(Error::from),
    }
}

/// The state one renewal produced, ready for its lease-conditioned persist.
enum Renewal {
    /// A refresh grant's merged credentials document, persisted whole.
    Grant {
        /// The merged credentials.
        credentials: StoredCredentials,
    },

    /// A reprovisioning's merged fields, persisted over the stored document in SQL.
    Reprovision {
        /// The renewed access token.
        access_token: String,

        /// The renewed expiry; write-time validation guarantees the reprovisioning extracts
        /// one, so a renewal never persists a `null` over a known expiry.
        expires_at: i64,

        /// The refresh token the reprovisioning sequence extracted, when it rotated one; the
        /// persist carries the member only when present, so an absent extraction leaves the
        /// stored token in place — the reprovisioning arm's mirror of the grant arm's
        /// retention of an omitted rotation (README §3).
        refresh_token: Option<String>,

        /// The renewed base URL, when provisioning supplied one.
        base_url: Option<String>,
    },
}

/// Performs the definition's renewal and builds the state its lease-conditioned persist writes.
///
/// The grant arm reads the stored refresh token and slots, decodes its response through the
/// declared mapping (or the exchange mapping), and retains an omitted refresh token; the
/// reprovisioning arm runs the provisioning sequence over the stored slots and may update the
/// access token, expiry, refresh token, and base URL together.
///
/// # Errors
///
/// Returns [`Error`] when the renewal fails.
async fn renew_and_build_state(http: &Client, provider: &Provider) -> Result<Renewal, Error> {
    let StoredCredentials::OAuth {
        access_token: _,
        refresh_token,
        slots,
        expires_at: _,
        refresh_started_at: _,
    } = &provider.credentials
    else {
        return Err(Error::Internal {
            message: "API key credentials never renew".to_string(),
        });
    };

    let recipe = provider
        .definition
        .oauth_recipe()
        .ok_or_else(|| Error::Internal {
            message: "the provider's definition carries no OAuth credential".to_string(),
        })?;

    let OAuthRecipe {
        client_id,
        client_secret,
        access_token_lifetime_seconds,
        id_token_claims,
        exchange,
        refresh,
        ..
    } = recipe;

    match refresh {
        TokenRefresh::Grant {
            request,
            response,
            terminal_codes,
        } => {
            let env = ValueEnv::for_refresh_grant(
                slots,
                provider.base_url.as_deref(),
                client_id.as_str(),
                client_secret.as_deref(),
                refresh_token.as_deref(),
            );

            let outcome = execute_refresh_grant(
                http,
                request,
                &env,
                response.as_ref().unwrap_or(&exchange.response),
                terminal_codes,
            )
            .await?;

            // The working slots absorb the identity-token claims and the response's slot
            // targets; the stored slots the flow carried stay in place. The fold borrows the
            // outcome, so it runs before the access token and rotation are moved out of it, and
            // a claimed value outside the routing-safety contract fails the renewal like any
            // other fold.
            let renewed_slots = absorb_exchange_slots(slots, &outcome, id_token_claims)?;

            let access_token = outcome.access_token.ok_or_else(|| Error::Unavailable {
                detail: "token refresh response lacks an access token".to_string(),
            })?;

            let expires_at = resolve_expiry(
                outcome.expires_in_seconds,
                Some(&access_token),
                *access_token_lifetime_seconds,
            )?;

            Ok(Renewal::Grant {
                credentials: StoredCredentials::OAuth {
                    access_token,
                    // An omitted rotation retains the stored refresh token; the renewed
                    // document persists atomically under the lease.
                    refresh_token: outcome.refresh_token.or_else(|| refresh_token.clone()),
                    slots: renewed_slots,
                    expires_at,
                    refresh_started_at: None,
                },
            })
        }

        TokenRefresh::Reprovision => {
            let Some(steps) = &provider.definition.payload.provisioning else {
                return Err(Error::Internal {
                    message: "reprovisioning requires provisioning steps".to_string(),
                });
            };

            // The stage constants thread the credential's current secret — the old access
            // token, until the reprovisioned one commits — and the client credentials down to
            // the per-request environment rebuilds.
            let constants = provisioning::ProvisioningEnv {
                secret: Some(provider.credentials.secret()),
                client_id: client_id.as_str(),
                client_secret: client_secret.as_deref(),
            };

            // The seed base feeds only the steps' request environments; the persist carries a
            // base only when an extraction derived one. Writing the acquire-time snapshot
            // unconditionally would clobber a base a concurrent `providers:update` stored
            // while the sequence ran — the flow path's identical hazard, which refuses a
            // non-derived base for the same reason (README §3).
            let mut state = WorkingState::new(slots.clone(), provider.base_url.clone());

            provisioning::run_steps(http, steps, &constants, &mut state).await?;

            let Some(access_token) = state.access_token else {
                return Err(Error::Internal {
                    message: "reprovisioning produced no access token".to_string(),
                });
            };

            // Write-time validation requires the provisioning sequence to extract
            // `expires_at_unix_seconds`, so a completed reprovisioning without one is stored
            // drift: persisting `null` would rewrite the stored expiry into the
            // no-scheduled-refresh state, and the credential would never refresh again.
            let Some(expires_at) = state.expires_at else {
                return Err(Error::Internal {
                    message: "reprovisioning produced no expiry".to_string(),
                });
            };

            Ok(Renewal::Reprovision {
                access_token,
                expires_at,
                refresh_token: state.refresh_token,
                // The stored base joins the persist only when this renewal's provisioning
                // extracted a replacement (README §3): the seed base aboard otherwise is the
                // lease-acquire-time snapshot, and the persist's `COALESCE` keeps the stored
                // column — a concurrent `providers:update`'s write — untouched.
                base_url: state
                    .is_base_url_derived
                    .then(|| state.base_url.clone())
                    .flatten(),
            })
        }

        TokenRefresh::None => Err(Error::Internal {
            message: "the credential never renews".to_string(),
        }),
    }
}
