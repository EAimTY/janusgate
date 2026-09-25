//! The lease-guarded token renewal: one request drives the database refresh lease, the
//! definition's refresh mode — a refresh grant or a reprovisioning — and the lease-conditioned
//! persist, so concurrent requests never overwrite newer credentials (README §3).
//!
//! The lease lives inside the credentials JSON document and every transition conditions on
//! `json_extract(credentials, …)`; the engine never read–modify–writes a stale document in Rust.
//! Only a known, elapsed `expires_at` initiates time-based refresh: a `null` expiry never does,
//! and `refresh: none` never enters the lease.

use super::{Error, flow};
use crate::{
    database::{Database, Error as DatabaseError, Transaction},
    http_client,
    provider::{
        Provider, StoredCredentials,
        definition::{
            credential::{CredentialAcquisition, OAuthRecipe, TokenRefresh},
            eval::ValueEnv,
        },
        provisioning::{self, WorkingState},
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
pub(crate) const TOKEN_FRESHNESS_MARGIN: i64 = 60 * unix_time::MICROS_PER_SEC;

/// Lease duration after which an abandoned token refresh is considered dead.
pub(crate) const REFRESH_LEASE_STALE_AFTER: Duration = Duration::from_secs(90);

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
enum RefreshOutcome {
    /// The renewed credentials were persisted while the caller still owned the lease.
    Renewed(Box<Provider>),

    /// The lease was lost while the grant was in flight; the refreshed credentials were derived
    /// from superseded credentials and were not persisted, leaving the newer state — its lease
    /// included — untouched.
    LostLease,
}

/// Whether the provider's OAuth token is still valid long enough to use. A `null` expiry is the
/// no-scheduled-refresh state, never a stale one: the token is sent until the upstream rejects
/// it or an explicit credential update or reauthorization occurs.
#[must_use]
pub(crate) fn is_token_fresh(provider: &Provider) -> bool {
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

    let CredentialAcquisition::OAuth(recipe) = &provider.definition.payload.credential else {
        return false;
    };

    !matches!(recipe.refresh, TokenRefresh::None)
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
/// instead of failing.
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

        if !is_renewable(&current) || is_token_fresh(&current) {
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
                return Ok(*renewed);
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

    Ok(RefreshOutcome::Renewed(Box::new(renewed)))
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
            base_url,
        } => storage::providers::persist_reprovision_for_lease(
            database,
            provider_id,
            lease_started_at,
            access_token,
            Some(*expires_at),
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

        /// The renewed base URL, when provisioning supplied one.
        base_url: Option<String>,
    },
}

/// Performs the definition's renewal and builds the state its lease-conditioned persist writes.
///
/// The grant arm reads the stored refresh token and slots, decodes its response through the
/// declared mapping (or the exchange mapping), and retains an omitted refresh token; the
/// reprovisioning arm runs the provisioning sequence over the stored slots and may update the
/// access token, expiry, and base URL together.
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

    let CredentialAcquisition::OAuth(recipe) = &provider.definition.payload.credential else {
        return Err(Error::Internal {
            message: "the provider's definition carries no OAuth credential".to_string(),
        });
    };

    let OAuthRecipe {
        client_id,
        client_secret,
        access_token_lifetime_seconds,
        id_token_claims,
        exchange,
        refresh,
        ..
    } = &**recipe;

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

            let outcome = flow::execute_refresh_grant(
                http,
                request,
                &env,
                response.as_ref().unwrap_or(&exchange.response),
                terminal_codes,
            )
            .await?;

            // The working slots absorb the identity-token claims and the response's slot
            // targets; the stored slots the flow carried stay in place. The fold borrows the
            // outcome, so it runs before the access token and rotation are moved out of it.
            let renewed_slots = flow::absorb_exchange_slots(slots, &outcome, id_token_claims);

            let access_token = outcome.access_token.ok_or_else(|| Error::Unavailable {
                detail: "token refresh response lacks an access token".to_string(),
            })?;

            let expires_at = flow::resolve_expiry(
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
                base_url: state.base_url,
            })
        }

        TokenRefresh::None => Err(Error::Internal {
            message: "the credential never renews".to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::unwrap_used,
        reason = "the fixtures are constructed so each unwrap lands on its documented side"
    )]

    use super::{Error, renew_and_build_state, renew_and_store};
    use crate::{
        database::{Database, schema::Registry},
        http_client,
        provider::{
            Provider, StoredCredentials, definition::DefinitionPayload, engine_tests::MockUpstream,
        },
        storage, unix_time,
    };
    use std::{collections::BTreeMap, num::NonZero, time::Duration};

    /// The pool wait the failure-injection database is opened with: long enough that the
    /// persist's failed acquire and the clear's successful one are separated by the release
    /// instant with wide margins on both sides.
    const POOL_WAIT: Duration = Duration::from_millis(1000);

    /// When the test releases the held connection: after the persist's pool wait has failed
    /// (it times out at one wait) but within the clear's own wait window, so the clear — whose
    /// success the assertion depends on — runs against the recovered pool.
    const RELEASE_AFTER: Duration = Duration::from_millis(1500);

    /// Parses the reprovision-drift fixture: a reprovision refresh whose provisioning sequence
    /// supplies only the access token, never an expiry. Parsed without validation, which is
    /// exactly the stored-drift state the write-time rule rejects.
    ///
    /// The exchange references the `redirect_uri` value part because the write boundary requires
    /// that of every authorization-code grant (RFC 6749 §4.1.3).
    ///
    /// # Panics
    ///
    /// Panics when the fixture fails to parse — a broken fixture is the failure report.
    fn reprovision_without_expiry_definition(
        token_url: String,
    ) -> crate::provider::ProviderDefinition {
        #[expect(clippy::unwrap_used, reason = "the fixture payload always parses")]
        let payload: DefinitionPayload = serde_json::from_value(serde_json::json!({
            "format": "openai_chat_completion",
            "inference": {
                "url": [{"type": "base"}, {"type": "literal", "value": "/chat/completions"}],
                "upstream_streaming": "match_client"
            },
            "credential": {
                "type": "oauth",
                "client_id": "fixture-client",
                "grant": {
                    "type": "authorization_code",
                    "authorize_url": [
                        {"type": "literal", "value": "https://accounts.example/auth"}
                    ],
                    "redirect_uri": "http://localhost:1/callback",
                    "pkce": "none"
                },
                "exchange": {
                    "request": {
                        "method": "POST",
                        "url": [{"type": "literal", "value": "http://127.0.0.1:1/token"}],
                        "body": {"kind": "form", "fields": [
                            {"name": "grant_type", "value": [
                                {"type": "literal", "value": "authorization_code"}
                            ]},
                            {"name": "code", "value": [{"type": "authorization_code"}]},
                            {"name": "redirect_uri", "value": [{"type": "redirect_uri"}]}
                        ]}
                    },
                    "response": {"encoding": "json", "fields": [
                        {
                            "path": ["access_token"],
                            "target": {"type": "access_token"},
                            "required": true
                        },
                        {
                            "path": ["refresh_token"],
                            "target": {"type": "refresh_token"},
                            "required": true
                        }
                    ]},
                    "terminal_codes": ["invalid_grant"]
                },
                "refresh": {"type": "reprovision"}
            },
            "provisioning": [
                {
                    "request": {
                        "method": "POST",
                        "url": [{"type": "literal", "value": token_url}]
                    },
                    "response_encoding": "json",
                    "extract": [
                        {
                            "path": ["token"],
                            "target": {"type": "access_token"},
                            "shape": {"type": "string"},
                            "required": true
                        }
                    ]
                }
            ]
        }))
        .unwrap();

        crate::provider::ProviderDefinition {
            name: "reprovision-drift".to_string(),
            version: NonZero::new(1).unwrap(),
            payload,
            created_at: 0,
        }
    }

    /// A reprovisioning whose sequence extracts no `expires_at_unix_seconds` is stored drift
    /// (README §5.4): write-time validation makes the state unrepresentable, so the runtime
    /// guard fails the renewal instead of persisting a `null` over a known expiry.
    ///
    /// # Panics
    ///
    /// Panics if the fixture fails to parse or the renewal does not fail with the drift error.
    #[tokio::test]
    async fn a_reprovision_without_a_provisioned_expiry_is_stored_drift() {
        let mock = MockUpstream::start(vec![
            serde_json::to_string(&serde_json::json!({"token": "renewed"})).unwrap(),
        ])
        .await;

        let provider = Provider {
            id: 1,
            name: "reprovisioned".to_string(),
            model_prefix: String::new(),
            base_url: None,
            is_enabled: true,
            credentials: StoredCredentials::OAuth {
                access_token: "elapsed".to_string(),
                refresh_token: None,
                slots: BTreeMap::new(),
                expires_at: Some(1),
                refresh_started_at: None,
            },
            definition: reprovision_without_expiry_definition(format!("{}/token", mock.base())),
        };

        #[expect(
            clippy::panic,
            reason = "a success or a divergent error variant is the failure report"
        )]
        let Err(Error::Internal { message }) =
            renew_and_build_state(&http_client::control_plane(), &provider).await
        else {
            panic!("a missing reprovisioned expiry is stored drift");
        };

        assert!(message.contains("no expiry"), "{message}");

        mock.shutdown.send(()).ok();
    }

    /// Inserts one refresh-grant definition whose grant posts to `refresh_url`, for the lease
    /// tests.
    ///
    /// The exchange references the `redirect_uri` value part because the write boundary requires
    /// that of every authorization-code grant (RFC 6749 §4.1.3).
    ///
    /// # Panics
    ///
    /// Panics if the payload does not parse, validate, or insert — a broken fixture is the
    /// failure report.
    async fn insert_refresh_grant_definition(
        database: &Database,
        name: &str,
        refresh_url: &str,
    ) -> crate::provider::ProviderDefinition {
        #[expect(clippy::unwrap_used, reason = "the fixture payload always parses")]
        let payload: DefinitionPayload = serde_json::from_value(serde_json::json!({
            "format": "openai_chat_completion",
            "inference": {
                "url": [{"type": "base"}, {"type": "literal", "value": "/chat/completions"}],
                "upstream_streaming": "match_client"
            },
            "credential": {
                "type": "oauth",
                "client_id": "fixture-client",
                "grant": {
                    "type": "authorization_code",
                    "authorize_url": [
                        {"type": "literal", "value": "https://accounts.example/auth"}
                    ],
                    "redirect_uri": "http://localhost:1/callback",
                    "pkce": "none"
                },
                "exchange": {
                    "request": {
                        "method": "POST",
                        "url": [{"type": "literal", "value": "http://127.0.0.1:1/token"}],
                        "body": {"kind": "form", "fields": [
                            {"name": "grant_type", "value": [
                                {"type": "literal", "value": "authorization_code"}
                            ]},
                            {"name": "code", "value": [{"type": "authorization_code"}]},
                            {"name": "redirect_uri", "value": [{"type": "redirect_uri"}]}
                        ]}
                    },
                    "response": {"encoding": "json", "fields": [
                        {
                            "path": ["access_token"],
                            "target": {"type": "access_token"},
                            "required": true
                        },
                        {
                            "path": ["refresh_token"],
                            "target": {"type": "refresh_token"},
                            "required": true
                        }
                    ]},
                    "terminal_codes": ["invalid_grant"]
                },
                "refresh": {
                    "type": "grant",
                    "request": {
                        "method": "POST",
                        "url": [{"type": "literal", "value": refresh_url}],
                        "body": {"kind": "form", "fields": [
                            {"name": "grant_type", "value": [
                                {"type": "literal", "value": "refresh_token"}
                            ]},
                            {"name": "refresh_token", "value": [{"type": "refresh_token"}]}
                        ]}
                    },
                    "response": {"encoding": "json", "fields": [
                        {
                            "path": ["access_token"],
                            "target": {"type": "access_token"},
                            "required": true
                        }
                    ]},
                    "terminal_codes": ["invalid_grant"]
                }
            }
        }))
        .unwrap();

        #[expect(
            clippy::unwrap_used,
            reason = "the fixture definition always validates"
        )]
        payload.validate().unwrap();

        #[expect(clippy::unwrap_used, reason = "the fixture definition always inserts")]
        storage::provider_definitions::insert_definition(
            database,
            storage::provider_definitions::NewDefinition {
                name,
                base_version: None,
                payload: &payload,
                created_at: 1,
            },
        )
        .await
        .unwrap()
    }

    /// A persistence failure after the lease was acquired clears the lease before the original
    /// error is returned (README §3): the renewal succeeds, the lease-conditioned persist
    /// fails, and the caller's own stamp is gone afterwards — concurrent waiters can acquire
    /// the lease instead of polling a dead one up to the staleness window.
    ///
    /// The failure is injected by exhausting a single-connection pool while the persist runs;
    /// the held connection is released inside the clear's own wait window, so the clear —
    /// whose success the assertion depends on — runs against the recovered pool.
    ///
    /// # Panics
    ///
    /// Panics if the fixtures fail to insert or the lease stamp survives the failed
    /// persistence.
    #[tokio::test]
    async fn a_failed_persistence_clears_the_lease() {
        let db_path = std::env::temp_dir().join(format!(
            "janusgate-refresh-test-{}.db",
            rand::random::<u64>()
        ));

        #[expect(clippy::unwrap_used, reason = "non-zero literal")]
        let one = NonZero::new(1_usize).unwrap();

        #[expect(clippy::unwrap_used, reason = "fresh test database opens cleanly")]
        let database = Database::open(&db_path, Registry::Main, one, POOL_WAIT)
            .await
            .unwrap();

        let mock = MockUpstream::start(vec![
            serde_json::to_string(
                &serde_json::json!({"access_token": "renewed", "expires_in": 3600}),
            )
            .unwrap(),
        ])
        .await;

        let definition =
            insert_refresh_grant_definition(&database, "refresh-grant-def", &mock.base()).await;

        let provider = crate::test_support::insert_provider(
            &database,
            &definition,
            "renewed",
            "r-",
            None,
            StoredCredentials::OAuth {
                access_token: "elapsed".to_string(),
                refresh_token: Some("stored".to_string()),
                slots: BTreeMap::new(),
                expires_at: Some(unix_time::unix_now_micros() - 1),
                refresh_started_at: None,
            },
        )
        .await;

        let now = unix_time::unix_now_micros();

        let mut transaction = database.transaction().await.unwrap();

        #[expect(clippy::expect_used, reason = "the elapsed token acquires the lease")]
        let lease = storage::providers::try_acquire_refresh_lease(
            &mut transaction,
            provider.id,
            now,
            now - 1,
            0,
        )
        .await
        .unwrap()
        .expect("the elapsed token acquires the lease");

        transaction.commit().await.unwrap();

        // The held connection makes the persist's pool acquire fail; releasing it 500 ms into
        // the clear's own wait lets the clear run.
        let held = database.transaction().await.unwrap();

        let releaser = tokio::spawn(async move {
            tokio::time::sleep(RELEASE_AFTER).await;

            drop(held);
        });

        #[expect(
            clippy::panic,
            reason = "a success where the persist failed is the failure report"
        )]
        let Err(error) =
            renew_and_store(&database, &http_client::control_plane(), &provider, lease).await
        else {
            panic!("a failed persistence must fail the renewal");
        };

        assert!(
            matches!(error, Error::Internal { .. }),
            "the original persistence error surfaces: {error}"
        );

        releaser.await.unwrap();

        #[expect(clippy::expect_used, reason = "the provider row persists")]
        let settled = storage::providers::find_provider_by_id(&database, provider.id)
            .await
            .unwrap()
            .expect("the provider row persists");

        #[expect(
            clippy::panic,
            reason = "a divergent credential kind is the failure report"
        )]
        let StoredCredentials::OAuth {
            refresh_started_at, ..
        } = settled.credentials
        else {
            panic!("the renewed row carries OAuth credentials");
        };

        assert_eq!(
            refresh_started_at, None,
            "the failed persistence must clear the caller's own lease"
        );

        mock.shutdown.send(()).ok();

        let _ = std::fs::remove_file(&db_path);
    }
}
