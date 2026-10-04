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

#[cfg(test)]
mod tests {
    #![expect(
        clippy::unwrap_used,
        reason = "the fixtures are constructed so each unwrap lands on its documented side"
    )]

    use super::{Error, ensure_fresh_token, is_renewable, renew_and_build_state, renew_and_store};
    use crate::{
        database::{Database, schema::Registry},
        format::UpstreamFormat,
        http_client,
        provider::{Provider, StoredCredentials},
        storage,
        test_support::{
            FIXTURE_TOKEN_URL, MockUpstream, OAuthDefinitionFixture, OAuthGrantFixture,
            OAuthRefreshFixture, OAuthResponseField, OAuthTarget, oauth_definition_payload,
            static_key_definition_payload,
        },
        unix_time,
    };
    use std::{collections::BTreeMap, num::NonZero, time::Duration};

    /// The pool wait the failure-injection database is opened with: long enough that the
    /// persist's failed acquire and the clear's successful one are separated by the release
    /// instant with wide margins on both sides.
    const POOL_WAIT: Duration = Duration::from_millis(1000);

    /// When the test releases the held connection: after the persist's pool wait has failed
    /// (it times out at one wait) but within the clear's own wait window, so the clear —
    /// whose success the assertion depends on — runs against the recovered pool.
    const RELEASE_AFTER: Duration = Duration::from_millis(1500);

    /// The bound the null-expiry dispatch test holds [`ensure_fresh_token`] to: a correct
    /// dispatch of the no-scheduled-refresh row answers after one transaction, while the wedge
    /// this test pins — polling the ungrantable lease to the 100-second wait limit — fails the
    /// bound in seconds instead of costing the full wait.
    const NULL_EXPIRY_DISPATCH_BOUND: Duration = Duration::from_secs(5);

    /// Parses the reprovision-drift fixture: a reprovision refresh whose provisioning sequence
    /// supplies only the access token, never an expiry. Parsed without validation, which is
    /// exactly the stored-drift state the write-time rule rejects.
    ///
    /// # Panics
    ///
    /// Panics when the fixture fails to parse — a broken fixture is the failure report.
    fn reprovision_without_expiry_definition(
        token_url: String,
    ) -> crate::provider::ProviderDefinition {
        let payload = oauth_definition_payload(OAuthDefinitionFixture {
            format: UpstreamFormat::OpenAiChatCompletion,
            inference_suffix: Some("/chat/completions"),
            client_id: "fixture-client",
            initial_slots: &[],
            grant: OAuthGrantFixture::AuthorizationCode,
            exchange_url: FIXTURE_TOKEN_URL,
            response_fields: vec![OAuthResponseField::new(
                "refresh_token",
                OAuthTarget::RefreshToken,
            )],
            terminal_codes: &["invalid_grant"],
            refresh: OAuthRefreshFixture::Reprovision,
            provisioning: Some(serde_json::json!([{
                "request": {
                    "method": "POST",
                    "url": [{"type": "literal", "value": token_url}]
                },
                "response_encoding": "json",
                "extract": [{
                    "path": ["token"],
                    "target": {"type": "access_token"},
                    "shape": {"type": "string"},
                    "required": true
                }]
            }])),
        });

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
            model_prefix: String::new(),
            quota_generation: 0,
            quota_revision: 0,
            id: 1,
            name: "reprovisioned".to_string(),
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

    /// Inserts one reprovision-renewal definition whose single provisioning step posts to
    /// `token_url` and extracts the access token and the future expiry — plus a rotated
    /// refresh token when `with_rotation` holds — for the reprovision-renewal tests.
    ///
    /// # Panics
    ///
    /// Panics if the payload does not parse, validate, or insert — a broken fixture is the
    /// failure report.
    async fn insert_reprovision_definition(
        database: &Database,
        name: &str,
        token_url: &str,
        with_rotation: bool,
    ) -> crate::provider::ProviderDefinition {
        let mut extract = vec![
            serde_json::json!({
                "path": ["token"],
                "target": {"type": "access_token"},
                "shape": {"type": "string"},
                "required": true
            }),
            serde_json::json!({
                "path": ["expiry"],
                "target": {"type": "expires_at_unix_seconds"},
                "shape": {"type": "integer"},
                "required": true
            }),
        ];

        if with_rotation {
            extract.push(serde_json::json!({
                "path": ["refresh"],
                "target": {"type": "refresh_token"},
                "shape": {"type": "string"},
                "required": true
            }));
        }

        let payload = oauth_definition_payload(OAuthDefinitionFixture {
            format: UpstreamFormat::OpenAiChatCompletion,
            inference_suffix: Some("/chat/completions"),
            client_id: "fixture-client",
            initial_slots: &[],
            grant: OAuthGrantFixture::AuthorizationCode,
            exchange_url: FIXTURE_TOKEN_URL,
            response_fields: vec![],
            terminal_codes: &["invalid_grant"],
            refresh: OAuthRefreshFixture::Reprovision,
            provisioning: Some(serde_json::json!([{
                "request": {
                    "method": "POST",
                    "url": [{"type": "literal", "value": token_url}]
                },
                "response_encoding": "json",
                "extract": extract
            }])),
        });

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

    /// A reprovisioning renewal whose sequence extracts a rotated refresh token persists the
    /// new token over the stored one under the lease (README §3): the working state folds the
    /// extraction, and the lease-conditioned reprovision persist carries it, so the next
    /// renewal never replays a token the upstream may have revoked on rotation (RFC 6749
    /// §10.4).
    ///
    /// # Panics
    ///
    /// Panics if the fixtures fail to insert or the renewal does not persist the rotation.
    #[tokio::test]
    async fn a_reprovision_renewal_persists_an_extracted_refresh_token_rotation() {
        let db_path = std::env::temp_dir().join(format!(
            "janusgate-refresh-repro-rotation-test-{}.db",
            rand::random::<u64>()
        ));

        #[expect(clippy::unwrap_used, reason = "non-zero literal")]
        let one = NonZero::new(1_usize).unwrap();

        #[expect(clippy::unwrap_used, reason = "fresh test database opens cleanly")]
        let database = Database::open(&db_path, Registry::Main, one, POOL_WAIT)
            .await
            .unwrap();

        let expiry_seconds = unix_time::unix_now_micros() / unix_time::MICROS_PER_SEC + 3_600;

        let mock = MockUpstream::start(vec![
            serde_json::to_string(&serde_json::json!(
                {"token": "renewed", "refresh": "rotated", "expiry": expiry_seconds}
            ))
            .unwrap(),
        ])
        .await;

        let definition = insert_reprovision_definition(
            &database,
            "reprovision-rotation-def",
            &mock.base(),
            true,
        )
        .await;

        let provider = crate::test_support::insert_provider(
            &database,
            &definition,
            "rotated",
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

        renew_and_store(&database, &http_client::control_plane(), &provider, lease)
            .await
            .unwrap();

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
            access_token,
            refresh_token,
            refresh_started_at,
            ..
        } = settled.credentials
        else {
            panic!("the renewed row carries OAuth credentials");
        };

        assert_eq!(access_token, "renewed", "the reprovisioning persisted");

        assert_eq!(
            refresh_token.as_deref(),
            Some("rotated"),
            "the extracted rotation replaces the stored refresh token"
        );

        assert_eq!(refresh_started_at, None, "the persist clears the lease");

        mock.shutdown.send(()).ok();

        let _ = std::fs::remove_file(&db_path);
    }

    /// A reprovisioning renewal whose sequence extracts no refresh token leaves the stored one
    /// untouched, in both stored shapes (README §3): a pinned token survives for the next
    /// renewal, and an unpinned one stays unpinned — an absent extraction patches nothing,
    /// exactly like an omitted rotation in the grant path.
    ///
    /// # Panics
    ///
    /// Panics if the fixtures fail to insert or a renewal does not retain the stored shape.
    #[tokio::test]
    async fn a_reprovision_without_a_refresh_extraction_retains_the_stored_refresh_token() {
        let db_path = std::env::temp_dir().join(format!(
            "janusgate-refresh-repro-retain-test-{}.db",
            rand::random::<u64>()
        ));

        #[expect(clippy::unwrap_used, reason = "non-zero literal")]
        let one = NonZero::new(1_usize).unwrap();

        #[expect(clippy::unwrap_used, reason = "fresh test database opens cleanly")]
        let database = Database::open(&db_path, Registry::Main, one, POOL_WAIT)
            .await
            .unwrap();

        let expiry_seconds = unix_time::unix_now_micros() / unix_time::MICROS_PER_SEC + 3_600;

        let response = serde_json::to_string(
            &serde_json::json!({"token": "renewed", "expiry": expiry_seconds}),
        )
        .unwrap();

        let mock = MockUpstream::start(vec![response.clone(), response]).await;

        let definition =
            insert_reprovision_definition(&database, "reprovision-retain-def", &mock.base(), false)
                .await;

        let pinned = crate::test_support::insert_provider(
            &database,
            &definition,
            "pinned",
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

        let unpinned = crate::test_support::insert_provider(
            &database,
            &definition,
            "unpinned",
            None,
            StoredCredentials::OAuth {
                access_token: "elapsed".to_string(),
                refresh_token: None,
                slots: BTreeMap::new(),
                expires_at: Some(unix_time::unix_now_micros() - 1),
                refresh_started_at: None,
            },
        )
        .await;

        for (provider, expected) in [(pinned, Some("stored")), (unpinned, None)] {
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

            renew_and_store(&database, &http_client::control_plane(), &provider, lease)
                .await
                .unwrap();

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
                access_token,
                refresh_token,
                ..
            } = settled.credentials
            else {
                panic!("the renewed row carries OAuth credentials");
            };

            assert_eq!(access_token, "renewed", "the reprovisioning persisted");

            assert_eq!(
                refresh_token.as_deref(),
                expected,
                "an absent extraction leaves the stored refresh token shape untouched"
            );
        }

        mock.shutdown.send(()).ok();

        let _ = std::fs::remove_file(&db_path);
    }

    /// A reprovisioning renewal whose sequence extracts no `base_url` leaves a base URL a
    /// concurrent `providers:update` stored untouched (README §3): the working state's seed
    /// base is the lease-acquire-time snapshot, so only an extraction marks it derived, and
    /// the persist carries the base only when this renewal's provisioning supplied one — the
    /// reprovisioning arm's mirror of the flow path's refusal to rewrite a non-derived base
    /// (README §7.1). The access token renews regardless.
    ///
    /// # Panics
    ///
    /// Panics if the fixtures fail to insert or the renewal rewrites the concurrently stored
    /// base.
    #[tokio::test]
    async fn a_reprovision_without_a_base_extraction_retains_a_concurrently_updated_base_url() {
        let db_path = std::env::temp_dir().join(format!(
            "janusgate-refresh-repro-base-test-{}.db",
            rand::random::<u64>()
        ));

        #[expect(clippy::unwrap_used, reason = "non-zero literal")]
        let one = NonZero::new(1_usize).unwrap();

        #[expect(clippy::unwrap_used, reason = "fresh test database opens cleanly")]
        let database = Database::open(&db_path, Registry::Main, one, POOL_WAIT)
            .await
            .unwrap();

        let expiry_seconds = unix_time::unix_now_micros() / unix_time::MICROS_PER_SEC + 3_600;

        let mock = MockUpstream::start(vec![
            serde_json::to_string(
                &serde_json::json!({"token": "renewed", "expiry": expiry_seconds}),
            )
            .unwrap(),
        ])
        .await;

        let definition =
            insert_reprovision_definition(&database, "reprovision-base-def", &mock.base(), false)
                .await;

        let provider = crate::test_support::insert_provider(
            &database,
            &definition,
            "based",
            Some("https://acquire-time.example"),
            StoredCredentials::OAuth {
                access_token: "elapsed".to_string(),
                refresh_token: None,
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

        // The concurrent write under test: a `providers:update` changing only the base URL.
        // It never touches the credentials document, so the renewal's lease stamp survives
        // and the persist's conditioning cannot mask the clobber this test pins.
        #[expect(clippy::unwrap_used, reason = "the base-only update commits")]
        storage::providers::update_provider(
            &database,
            provider.id,
            storage::providers::ProviderUpdate {
                model_prefix: "",
                name: provider.name.as_str(),
                base_url: Some("https://concurrent.example"),
                is_enabled: true,
                definition: None,
                credentials: None,
            },
        )
        .await
        .unwrap();

        renew_and_store(&database, &http_client::control_plane(), &provider, lease)
            .await
            .unwrap();

        #[expect(clippy::expect_used, reason = "the provider row persists")]
        let settled = storage::providers::find_provider_by_id(&database, provider.id)
            .await
            .unwrap()
            .expect("the provider row persists");

        #[expect(
            clippy::panic,
            reason = "a divergent credential kind is the failure report"
        )]
        let StoredCredentials::OAuth { access_token, .. } = settled.credentials else {
            panic!("the renewed row carries OAuth credentials");
        };

        assert_eq!(access_token, "renewed", "the reprovisioning persisted");

        assert_eq!(
            settled.base_url.as_deref(),
            Some("https://concurrent.example"),
            "a base URL a concurrent providers:update stored survives a renewal that extracted \
             none"
        );

        mock.shutdown.send(()).ok();

        let _ = std::fs::remove_file(&db_path);
    }

    /// Inserts one refresh-grant definition whose grant posts to `refresh_url`, for the lease
    /// tests.
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
        let payload = oauth_definition_payload(OAuthDefinitionFixture {
            format: UpstreamFormat::OpenAiChatCompletion,
            inference_suffix: Some("/chat/completions"),
            client_id: "fixture-client",
            initial_slots: &[],
            grant: OAuthGrantFixture::AuthorizationCode,
            exchange_url: FIXTURE_TOKEN_URL,
            response_fields: vec![OAuthResponseField::new(
                "refresh_token",
                OAuthTarget::RefreshToken,
            )],
            terminal_codes: &["invalid_grant"],
            refresh: OAuthRefreshFixture::Grant {
                url: refresh_url,
                response: Some(vec![OAuthResponseField::new(
                    "access_token",
                    OAuthTarget::AccessToken,
                )]),
            },
            provisioning: None,
        });

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

    /// Parses the renewal-fold fixture: a refresh grant whose response maps the access token, a
    /// `base_url` target, and a slot target, so the fold's storage boundary is exercised on the
    /// renewal path. Parsed without validation — the boundary under test is the fold, not the
    /// write-time descriptor checks.
    ///
    /// # Panics
    ///
    /// Panics when the fixture fails to parse — a broken fixture is the failure report.
    fn renewal_fold_definition(token_url: String) -> crate::provider::ProviderDefinition {
        let payload = oauth_definition_payload(OAuthDefinitionFixture {
            format: UpstreamFormat::OpenAiChatCompletion,
            inference_suffix: Some("/chat/completions"),
            client_id: "fixture-client",
            initial_slots: &[],
            grant: OAuthGrantFixture::AuthorizationCode,
            exchange_url: &token_url,
            response_fields: vec![OAuthResponseField::new(
                "refresh_token",
                OAuthTarget::RefreshToken,
            )],
            terminal_codes: &["invalid_grant"],
            refresh: OAuthRefreshFixture::Grant {
                url: &token_url,
                response: Some(vec![
                    OAuthResponseField::new("access_token", OAuthTarget::AccessToken),
                    OAuthResponseField::new("endpoint", OAuthTarget::BaseUrl),
                    OAuthResponseField::new("stage", OAuthTarget::Slot("stage")),
                ]),
            },
            provisioning: None,
        });

        crate::provider::ProviderDefinition {
            name: "renewal-fold".to_string(),
            version: NonZero::new(1).unwrap(),
            payload,
            created_at: 0,
        }
    }

    /// A renewable provider whose credentials carry the `null` no-scheduled-refresh expiry
    /// dispatches as-is (README §3): the lease is never granted for a `null` expiry — both
    /// acquire arms require a known `expires_at` — so no concurrent refresher can ever finish,
    /// and reading the failed acquire as "a live holder exists" polls the ungrantable lease
    /// until the wait limit fails every dispatch. The row flows to dispatch untouched: no
    /// renewal runs and the `null` expiry survives.
    ///
    /// # Panics
    ///
    /// Panics if the fixtures fail to insert or the dispatch does not answer within the bound.
    #[tokio::test]
    async fn a_renewable_provider_with_a_null_expiry_dispatches_without_the_lease() {
        let db_path = std::env::temp_dir().join(format!(
            "janusgate-refresh-null-test-{}.db",
            rand::random::<u64>()
        ));

        #[expect(clippy::unwrap_used, reason = "non-zero literal")]
        let one = NonZero::new(1_usize).unwrap();

        #[expect(clippy::unwrap_used, reason = "fresh test database opens cleanly")]
        let database = Database::open(&db_path, Registry::Main, one, POOL_WAIT)
            .await
            .unwrap();

        // The mock's renewal answer must never be consumed: a consumed answer would mean the
        // loop treated the unscheduled row as refreshable.
        let mock = MockUpstream::start(vec![
            serde_json::to_string(
                &serde_json::json!({"access_token": "renewed", "expires_in": 3600}),
            )
            .unwrap(),
        ])
        .await;

        let definition =
            insert_refresh_grant_definition(&database, "null-expiry-def", &mock.base()).await;

        let provider = crate::test_support::insert_provider(
            &database,
            &definition,
            "unscheduled",
            None,
            StoredCredentials::OAuth {
                access_token: "elapsed".to_string(),
                refresh_token: Some("stored".to_string()),
                slots: BTreeMap::new(),
                expires_at: None,
                refresh_started_at: None,
            },
        )
        .await;

        let dispatched = tokio::time::timeout(
            NULL_EXPIRY_DISPATCH_BOUND,
            ensure_fresh_token(&database, &http_client::control_plane(), &provider),
        )
        .await
        .unwrap()
        .unwrap();

        assert_eq!(dispatched.id, provider.id);

        assert_eq!(dispatched.credentials.secret(), "elapsed");

        #[expect(
            clippy::panic,
            reason = "a divergent credential kind is the failure report"
        )]
        let StoredCredentials::OAuth {
            expires_at,
            refresh_started_at,
            ..
        } = dispatched.credentials
        else {
            panic!("the dispatched row carries OAuth credentials");
        };

        assert_eq!(expires_at, None, "the null expiry flows to dispatch as-is");

        assert_eq!(refresh_started_at, None, "no lease is ever stamped");

        mock.shutdown.send(()).ok();

        let _ = std::fs::remove_file(&db_path);
    }

    /// The renewal's response fold is a storage boundary (README §7): a refresh response whose
    /// `base_url` carries no `http(s)://` scheme or whose slot text falls outside routing-safe
    /// machine-consumed text fails the renewal instead of persisting the poisoned values, and a
    /// response whose values satisfy the contracts renews with them stored.
    ///
    /// # Panics
    ///
    /// Panics if the fixture fails to parse or a renewal lands on the wrong side of its check.
    #[tokio::test]
    async fn a_renewal_response_mapping_unroutable_slot_or_base_url_text_fails_the_renewal() {
        // One mock serving the responses in walk order: the scheme-less base URL, the slot text
        // with whitespace, and the valid response the successful renewal consumes.
        let mock = MockUpstream::start(vec![
            serde_json::to_string(&serde_json::json!({
                "access_token": "renewed",
                "endpoint": "upstream.example",
                "stage": "clean"
            }))
            .unwrap(),
            serde_json::to_string(&serde_json::json!({
                "access_token": "renewed",
                "endpoint": "https://upstream.example",
                "stage": "has space"
            }))
            .unwrap(),
            serde_json::to_string(&serde_json::json!({
                "access_token": "renewed",
                "endpoint": "https://upstream.example",
                "stage": "clean"
            }))
            .unwrap(),
        ])
        .await;

        let http = http_client::control_plane();

        let provider = Provider {
            model_prefix: String::new(),
            quota_generation: 0,
            quota_revision: 0,
            id: 1,
            name: "renewal-fold".to_string(),
            base_url: None,
            is_enabled: true,
            credentials: StoredCredentials::OAuth {
                access_token: "elapsed".to_string(),
                refresh_token: Some("stored".to_string()),
                slots: BTreeMap::new(),
                expires_at: Some(1),
                refresh_started_at: None,
            },
            definition: renewal_fold_definition(mock.base()),
        };

        #[expect(
            clippy::panic,
            reason = "a success where the fold failed is the failure report"
        )]
        let Err(Error::Unavailable { detail }) = renew_and_build_state(&http, &provider).await
        else {
            panic!("a scheme-less renewed base_url must fail the renewal");
        };

        assert!(detail.contains("base_url"), "{detail}");

        #[expect(
            clippy::panic,
            reason = "a success where the fold failed is the failure report"
        )]
        let Err(Error::Unavailable { detail }) = renew_and_build_state(&http, &provider).await
        else {
            panic!("renewed slot text with whitespace must fail the renewal");
        };

        assert!(detail.contains("stage"), "{detail}");

        let renewal = renew_and_build_state(&http, &provider).await.unwrap();

        #[expect(
            clippy::panic,
            reason = "a divergent renewal arm is the failure report"
        )]
        let super::Renewal::Grant { credentials } = renewal else {
            panic!("the grant renewal builds a grant renewal state");
        };

        #[expect(
            clippy::panic,
            reason = "a divergent credential kind is the failure report"
        )]
        let StoredCredentials::OAuth { slots, .. } = credentials else {
            panic!("the renewed credentials carry OAuth credentials");
        };

        assert_eq!(
            slots.get("stage").map(String::as_str),
            Some("clean"),
            "valid renewed values store"
        );

        mock.shutdown.send(()).ok();
    }

    /// A provider whose stored credentials are OAuth but whose definition carries an api_key
    /// credential is not renewable (README §3): the stored-drift row must short-circuit before
    /// the lease loop instead of reaching a renewal whose definition carries no OAuth recipe —
    /// the internal drift error — so the predicate reports the drift as non-renewable.
    ///
    /// # Panics
    ///
    /// Panics if the fixture payload fails to parse — a broken fixture is the failure report.
    #[test]
    fn a_provider_whose_definition_lost_the_oauth_recipe_is_not_renewable() {
        let provider = Provider {
            model_prefix: String::new(),
            quota_generation: 0,
            quota_revision: 0,
            id: 1,
            name: "drifted".to_string(),
            base_url: None,
            is_enabled: true,
            credentials: StoredCredentials::OAuth {
                access_token: "elapsed".to_string(),
                refresh_token: Some("stored".to_string()),
                slots: BTreeMap::new(),
                expires_at: Some(unix_time::unix_now_micros() - 1),
                refresh_started_at: None,
            },
            definition: crate::provider::ProviderDefinition {
                name: "static-key-drift".to_string(),
                version: NonZero::new(1).unwrap(),
                payload: static_key_definition_payload(UpstreamFormat::OpenAiChatCompletion),
                created_at: 0,
            },
        };

        assert!(
            !is_renewable(&provider),
            "OAuth credentials under a definition without an OAuth recipe are not renewable"
        );
    }
}
