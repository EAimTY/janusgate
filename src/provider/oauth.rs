//! OAuth authorization flows, token exchange, refresh leasing, and Google
//! Code Assist project resolution for OAuth-backed providers.

use crate::{
    database::{Database, Error as DatabaseError, Transaction},
    error::{self, ApiError},
    provider::{Credentials, Provider, ProviderType, endpoints, http_body, storage},
    unix_time,
};
use base64::{Engine as _, engine::general_purpose};
use log::{debug, info, warn};
use reqwest::Client;
use sha2::{Digest as _, Sha256};
use std::time::Duration;
#[cfg(test)]
use std::{cell::RefCell, collections::VecDeque};
#[cfg(test)]
use turso::Error as TursoError;

/// OpenAI OAuth client identifier of the Codex CLI.
const OPENAI_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

/// OpenAI OAuth issuer root.
const OPENAI_ISSUER: &str = "https://auth.openai.com";

/// OpenAI OAuth loopback redirect.
const OPENAI_REDIRECT_URI: &str = "http://localhost:1455/auth/callback";

/// OpenAI OAuth scopes requested during authorization.
const OPENAI_SCOPES: &str = "openid profile email offline_access";

/// `originator` parameter identifying the Codex CLI client.
const OPENAI_ORIGINATOR: &str = "codex_cli_rs";

/// Google OAuth client identifier of the Antigravity IDE.
const ANTIGRAVITY_CLIENT_ID: &str =
    "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com";

/// Google OAuth client secret of the Antigravity IDE.
const ANTIGRAVITY_CLIENT_SECRET: &str = "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";

/// Google OAuth authorization endpoint.
const ANTIGRAVITY_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";

/// Google OAuth token endpoint.
const ANTIGRAVITY_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

/// Google OAuth loopback redirect.
const ANTIGRAVITY_REDIRECT_URI: &str = "http://localhost:51121/oauth-callback";

/// Google OAuth scopes requested during authorization.
const ANTIGRAVITY_SCOPES: &str = "https://www.googleapis.com/auth/cloud-platform \
     https://www.googleapis.com/auth/userinfo.email \
     https://www.googleapis.com/auth/userinfo.profile \
     https://www.googleapis.com/auth/cclog \
     https://www.googleapis.com/auth/experimentsandconfigs";

/// Lifetime of a pending OAuth state row.
const OAUTH_STATE_TTL: i64 = 600;

/// Default token lifetime when a provider reports neither `expires_in` nor a
/// usable `exp` claim.
const DEFAULT_TOKEN_TTL: i64 = 86_400;

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

/// Code Assist metadata identifying the Antigravity IDE.
fn code_assist_metadata() -> serde_json::Value {
    serde_json::json!({
        "ideType": "ANTIGRAVITY",
        "platform": "PLATFORM_UNSPECIFIED",
        "pluginType": "GEMINI",
    })
}

/// Delay between `onboardUser` polling attempts.
const ONBOARD_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Total wall-clock budget for the `onboardUser` long-running operation,
/// checked before every attempt. A fixed attempt count alone bounds nothing:
/// each attempt inherits the shared client's 300-second read timeout, so the
/// poll must also respect a deadline inside one management request.
const ONBOARD_TOTAL_BUDGET: Duration = Duration::from_secs(60);

/// A freshly begun OAuth authorization flow.
#[derive(Debug, Clone)]
pub(crate) struct OAuthFlow {
    /// Opaque state token identifying the flow.
    pub(crate) state: String,

    /// PKCE verifier; empty for flows without PKCE.
    pub(crate) verifier: String,

    /// URL the administrator opens to authorize.
    pub(crate) authorize_url: String,

    /// Unix timestamp when the flow expires.
    pub(crate) expires_at: i64,
}

/// Begins an OAuth authorization flow for the given provider type.
///
/// # Errors
///
/// Returns [`ApiError`] when the provider type does not support OAuth.
pub(crate) fn begin_flow(provider_type: ProviderType) -> Result<OAuthFlow, ApiError> {
    let state = random_url_safe(32);

    let (verifier, authorize_url) = match provider_type {
        ProviderType::OpenAiOauth => {
            let verifier = random_url_safe(64);

            let challenge =
                general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));

            let authorize_url = format!(
                "{OPENAI_ISSUER}/oauth/authorize?response_type=code&client_id={OPENAI_CLIENT_ID}\
                 &redirect_uri={OPENAI_REDIRECT_URI}&scope={}&code_challenge={challenge}\
                 &code_challenge_method=S256&state={state}&originator={OPENAI_ORIGINATOR}",
                encode_scopes(OPENAI_SCOPES)
            );

            (verifier, authorize_url)
        }

        ProviderType::AntigravityOauth => {
            let authorize_url = format!(
                "{ANTIGRAVITY_AUTH_URL}?access_type=offline&client_id={ANTIGRAVITY_CLIENT_ID}\
                 &prompt=consent&redirect_uri={ANTIGRAVITY_REDIRECT_URI}&response_type=code\
                 &scope={}&state={state}",
                encode_scopes(ANTIGRAVITY_SCOPES)
            );

            (String::new(), authorize_url)
        }

        ProviderType::OpenAiCompatible => {
            return Err(ApiError::invalid_parameter(
                "type",
                "openai_compatible providers do not use OAuth flows",
            ));
        }
    };

    Ok(OAuthFlow {
        state,
        verifier,
        authorize_url,
        expires_at: unix_time::unix_now() + OAUTH_STATE_TTL,
    })
}

/// Tokens returned by an OAuth token endpoint.
#[derive(Debug, Clone, Default, serde::Deserialize)]
struct TokenResponse {
    /// New access token.
    access_token: Option<String>,

    /// Replacement refresh token.
    refresh_token: Option<String>,

    /// OpenAI identity token.
    id_token: Option<String>,

    /// Lifetime of the access token in seconds.
    expires_in: Option<i64>,
}

/// Result of an authorization code exchange.
#[derive(Debug, Clone)]
pub(crate) struct ExchangedTokens {
    /// Credentials built from the exchanged tokens.
    pub(crate) credentials: Credentials,

    /// Unix timestamp when the access token expires.
    pub(crate) expires_at: i64,
}

/// Exchanges an OAuth authorization code for stored credentials.
///
/// # Errors
///
/// Returns [`ApiError`] when the token endpoint fails or the response omits
/// required fields.
pub(crate) async fn exchange_code(
    http: &Client,
    provider_type: ProviderType,
    code: &str,
    verifier: &str,
) -> Result<ExchangedTokens, ApiError> {
    let (credentials, raw_response) = match provider_type {
        ProviderType::OpenAiOauth => {
            let response: TokenResponse = post_form(
                http,
                &format!("{OPENAI_ISSUER}/oauth/token"),
                &[
                    ("grant_type", "authorization_code"),
                    ("code", code),
                    ("redirect_uri", OPENAI_REDIRECT_URI),
                    ("client_id", OPENAI_CLIENT_ID),
                    ("code_verifier", verifier),
                ],
                "authorization code",
            )
            .await?;

            let access_token = response.access_token.clone().ok_or_else(|| {
                ApiError::upstream_unavailable("token exchange response lacks an access token")
            })?;

            let id_token = response.id_token.clone().ok_or_else(|| {
                ApiError::upstream_unavailable("token exchange response lacks an id token")
            })?;

            let account_id = jwt_claim(
                &id_token,
                &["https://api.openai.com/auth", "chatgpt_account_id"],
            )
            .and_then(|claim| claim.as_str().map(str::to_string))
            .ok_or_else(|| {
                ApiError::upstream_unavailable("id token lacks a ChatGPT account claim")
            })?;

            let refresh_token = response.refresh_token.clone().ok_or_else(|| {
                ApiError::upstream_unavailable("token exchange response lacks a refresh token")
            })?;

            let credentials = Credentials::OpenAiOauth {
                access_token,
                refresh_token,
                account_id,
            };

            (credentials, response)
        }

        ProviderType::AntigravityOauth => {
            let response: TokenResponse = post_form(
                http,
                ANTIGRAVITY_TOKEN_URL,
                &[
                    ("grant_type", "authorization_code"),
                    ("code", code),
                    ("client_id", ANTIGRAVITY_CLIENT_ID),
                    ("client_secret", ANTIGRAVITY_CLIENT_SECRET),
                    ("redirect_uri", ANTIGRAVITY_REDIRECT_URI),
                ],
                "authorization code",
            )
            .await?;

            let access_token = response.access_token.clone().ok_or_else(|| {
                ApiError::upstream_unavailable("token exchange response lacks an access token")
            })?;

            let refresh_token = response.refresh_token.clone().ok_or_else(|| {
                ApiError::upstream_unavailable("token exchange response lacks a refresh token")
            })?;

            let project_id = resolve_project_id(http, &access_token).await?;

            let credentials = Credentials::AntigravityOauth {
                access_token,
                refresh_token,
                project_id,
            };

            (credentials, response)
        }

        ProviderType::OpenAiCompatible => {
            return Err(ApiError::invalid_parameter(
                "type",
                "openai_compatible providers do not use OAuth flows",
            ));
        }
    };

    let expires_at = token_expiry_from(
        raw_response.expires_in,
        raw_response.access_token.as_deref(),
    )?;

    Ok(ExchangedTokens {
        credentials,
        expires_at,
    })
}

/// Result of a refresh grant.
#[derive(Debug, Clone)]
pub(crate) struct RefreshedTokens {
    /// New access token.
    pub(crate) access_token: String,

    /// Replacement refresh token, when the endpoint rotated it.
    pub(crate) refresh_token: Option<String>,

    /// Replacement identity token, when issued.
    pub(crate) id_token: Option<String>,

    /// Lifetime of the access token in seconds, when the endpoint reports it.
    pub(crate) expires_in: Option<i64>,
}

/// Refreshes an OAuth access token with the provider's refresh grant.
///
/// # Errors
///
/// Returns [`ApiError`] when the token endpoint fails or omits an access
/// token.
pub(crate) async fn refresh_tokens(
    http: &Client,
    provider_type: ProviderType,
    refresh_token: &str,
) -> Result<RefreshedTokens, ApiError> {
    let response: TokenResponse = match provider_type {
        ProviderType::OpenAiOauth => {
            post_json(
                http,
                &format!("{OPENAI_ISSUER}/oauth/token"),
                &serde_json::json!({
                    "client_id": OPENAI_CLIENT_ID,
                    "grant_type": "refresh_token",
                    "refresh_token": refresh_token,
                }),
                "refresh token",
            )
            .await?
        }

        ProviderType::AntigravityOauth => {
            post_form(
                http,
                ANTIGRAVITY_TOKEN_URL,
                &[
                    ("grant_type", "refresh_token"),
                    ("refresh_token", refresh_token),
                    ("client_id", ANTIGRAVITY_CLIENT_ID),
                    ("client_secret", ANTIGRAVITY_CLIENT_SECRET),
                ],
                "refresh token",
            )
            .await?
        }

        ProviderType::OpenAiCompatible => {
            return Err(ApiError::internal(
                "openai_compatible providers do not refresh tokens",
            ));
        }
    };

    let access_token = response.access_token.ok_or_else(|| {
        ApiError::upstream_unavailable("token refresh response lacks an access token")
    })?;

    Ok(RefreshedTokens {
        access_token,
        refresh_token: response.refresh_token,
        id_token: response.id_token,
        expires_in: response.expires_in,
    })
}

/// Resolves the Cloud Code Assist project ID for the authenticated Google
/// account: `loadCodeAssist` first, then the `onboardUser` long-running
/// operation polled within [`ONBOARD_TOTAL_BUDGET`].
///
/// # Errors
///
/// Returns [`ApiError`] when neither call yields a project, the operation
/// reports a terminal failure, or the poll budget elapses.
pub(crate) async fn resolve_project_id(
    http: &Client,
    access_token: &str,
) -> Result<String, ApiError> {
    let load_response: serde_json::Value = post_code_assist(
        http,
        endpoints::ANTIGRAVITY_LOAD_CODE_ASSIST_ACTION,
        access_token,
        &serde_json::json!({"metadata": code_assist_metadata()}),
    )
    .await?;

    if let Some(project) = load_response
        .get("cloudaicompanionProject")
        .and_then(serde_json::Value::as_str)
    {
        return Ok(project.to_string());
    }

    let tier = load_response
        .pointer("/paidTier/id")
        .or_else(|| load_response.pointer("/currentTier/id"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("free-tier");

    let body = serde_json::json!({
        "tierId": tier,
        "metadata": code_assist_metadata(),
    });

    let onboard_deadline = std::time::Instant::now() + ONBOARD_TOTAL_BUDGET;

    loop {
        if std::time::Instant::now() >= onboard_deadline {
            return Err(ApiError::upstream_unavailable(
                "project onboarding did not complete in time",
            ));
        }

        let response: serde_json::Value = post_code_assist(
            http,
            endpoints::ANTIGRAVITY_ONBOARD_USER_ACTION,
            access_token,
            &body,
        )
        .await?;

        if response
            .get("done")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            return onboard_operation_project(&response)?.ok_or_else(|| {
                ApiError::upstream_unavailable("onboardUser completed without a project ID")
            });
        }

        tokio::time::sleep(ONBOARD_POLL_INTERVAL).await;
    }
}

/// Extracts the project ID from a completed `onboardUser` operation.
///
/// A completed `google.longrunning` operation carries its terminal failure as
/// an `error` payload (`{code, message, status}`), which is propagated into
/// the upstream error detail instead of collapsing into a generic
/// missing-project failure.
fn onboard_operation_project(operation: &serde_json::Value) -> Result<Option<String>, ApiError> {
    if let Some(error) = operation.get("error") {
        let status = error
            .get("status")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown status");

        let message = error
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("no message");

        return Err(ApiError::upstream_unavailable(format!(
            "onboardUser failed upstream: {status}: {message}"
        )));
    }

    Ok(operation
        .pointer("/response/cloudaicompanionProject")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string))
}

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
/// Returns [`ApiError`] when the provider vanished, the refresh fails, or a
/// concurrent refresh does not finish in time.
pub(crate) async fn ensure_fresh_token(
    database: &Database,
    http: &Client,
    provider: &Provider,
) -> Result<Provider, ApiError> {
    if matches!(provider.credentials, Credentials::ApiKey(_)) || is_token_fresh(provider) {
        return Ok(provider.clone());
    }

    let wait_deadline = std::time::Instant::now() + REFRESH_WAIT_LIMIT;

    loop {
        let now = unix_time::unix_now();

        let mut transaction = database.transaction().await.map_err(|error| {
            ApiError::internal(format!(
                "failed to begin token refresh transaction: {error}"
            ))
        })?;

        let Some(current) =
            storage::find_provider_by_id_in_transaction(&mut transaction, provider.id).await?
        else {
            transaction
                .rollback()
                .await
                .map_err(|error| ApiError::internal(format!("failed to roll back: {error}")))?;

            return Err(ApiError::not_found(
                "provider vanished during token refresh",
            ));
        };

        if is_token_fresh(&current) {
            transaction
                .rollback()
                .await
                .map_err(|error| ApiError::internal(format!("failed to roll back: {error}")))?;

            return Ok(current);
        }

        let lease_started_at =
            match acquire_refresh_lease(&mut transaction, current.id, now).await {
                Ok(lease_started_at) => lease_started_at,

                // The acquire is a read-then-write transaction: the SELECT fixed
                // the read snapshot and the lease UPDATE upgrades it to a write.
                // A commit that landed in between invalidates the snapshot, and
                // the engine refuses the upgrade — that is the lease burst this
                // loop exists to absorb, not a failed request. The engine
                // requires a rollback before retrying, and the next iteration is
                // the retry: it re-reads the newer state and either finds the
                // token fresh or waits its turn.
                Err(lease_error) if error::is_stale_snapshot(&lease_error) => {
                    transaction.rollback().await.map_err(|error| {
                        ApiError::internal(format!("failed to roll back: {error}"))
                    })?;

                    tokio::time::sleep(REFRESH_WAIT_INTERVAL).await;

                    wait_or_fail(wait_deadline)?;

                    continue;
                }

                Err(lease_error) => return Err(lease_error.into()),
            };

        transaction.commit().await.map_err(|error| {
            ApiError::internal(format!("failed to commit refresh lease: {error}"))
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
/// transaction, returning the raw database error so the caller can classify a
/// stale snapshot as "not acquired" and retry through its poll loop.
///
/// This wrapper is the test seam for that retry arm (README §3): the arm fires
/// only when the acquire's read-then-write upgrade loses its read snapshot to
/// a concurrent commit, which otherwise requires racing a commit into the
/// loop's SELECT-to-UPDATE window. A test queues fault results with
/// [`queue_lease_acquire_fault`] and this step substitutes them for the
/// storage call, one per attempt, making the burst-and-retry cycle
/// deterministic.
///
/// # Errors
///
/// Returns the underlying database error when the acquire fails.
async fn acquire_refresh_lease(
    transaction: &mut Transaction,
    provider_id: i64,
    now: i64,
) -> Result<Option<i64>, DatabaseError> {
    #[cfg(test)]
    if let Some(fault) = pop_lease_acquire_fault() {
        return Err(fault);
    }

    storage::try_acquire_refresh_lease(
        transaction,
        provider_id,
        now,
        now - i64::try_from(REFRESH_LEASE_STALE_AFTER.as_secs()).unwrap_or(i64::MAX),
        TOKEN_FRESHNESS_MARGIN,
    )
    .await
}

// Fault results replacing the next `acquire_refresh_lease` attempts, one per
// attempt in queue order. Thread-local because each `#[tokio::test]` runs on
// its own thread, so concurrent tests cannot observe each other's faults.
#[cfg(test)]
thread_local! {
    static LEASE_ACQUIRE_FAULTS: RefCell<VecDeque<DatabaseError>> =
        const { RefCell::new(VecDeque::new()) };
}

/// Queues `fault` as the result of the next lease-acquire attempt (test-only).
#[cfg(test)]
fn queue_lease_acquire_fault(fault: DatabaseError) {
    LEASE_ACQUIRE_FAULTS.with(|faults| faults.borrow_mut().push_back(fault));
}

/// Pops the queued fault for one lease-acquire attempt, if any (test-only).
#[cfg(test)]
fn pop_lease_acquire_fault() -> Option<DatabaseError> {
    LEASE_ACQUIRE_FAULTS.with(|faults| faults.borrow_mut().pop_front())
}

/// Whether a queued lease-acquire fault remains unconsumed (test-only).
#[cfg(test)]
fn has_queued_lease_acquire_fault() -> bool {
    LEASE_ACQUIRE_FAULTS.with(|faults| !faults.borrow().is_empty())
}

/// Fails the request once the poll window for a concurrent token refresh has
/// elapsed (README §3); otherwise the caller re-enters the poll loop.
fn wait_or_fail(wait_deadline: std::time::Instant) -> Result<(), ApiError> {
    if std::time::Instant::now() >= wait_deadline {
        return Err(ApiError::upstream_unavailable(
            "timed out waiting for a concurrent token refresh",
        ));
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
/// Returns [`ApiError`] when the refresh or the persistence fails.
async fn refresh_and_store(
    database: &Database,
    http: &Client,
    provider: &Provider,
    lease_started_at: i64,
) -> Result<RefreshOutcome, ApiError> {
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
/// Returns [`ApiError`] when the refresh grant fails.
async fn refresh_and_build_credentials(
    http: &Client,
    provider: &Provider,
) -> Result<(Credentials, i64), ApiError> {
    let refreshed = match &provider.credentials {
        Credentials::OpenAiOauth { refresh_token, .. } => {
            refresh_tokens(http, ProviderType::OpenAiOauth, refresh_token).await?
        }

        Credentials::AntigravityOauth { refresh_token, .. } => {
            refresh_tokens(http, ProviderType::AntigravityOauth, refresh_token).await?
        }

        Credentials::ApiKey(_) => {
            return Err(ApiError::internal("API key credentials never refresh"));
        }
    };

    let access_token = refreshed.access_token.clone();

    // Computed before `access_token` is moved into the credentials.
    let expires_at = token_expiry_from(refreshed.expires_in, Some(&access_token))?;

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
                    jwt_claim(
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
            return Err(ApiError::internal("API key credentials never refresh"));
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

/// Computes the expiry of a token set: a reported `expires_in` when present,
/// else the remaining lifetime of the access token's `exp` claim, else the
/// default lifetime.
///
/// A non-positive `expires_in` is a malformed token response and is rejected:
/// stamping an expiry at or before `now` would mark the token permanently
/// stale and drive a refresh grant against the endpoint on every request.
///
/// Google access tokens are opaque (`ya29…`) and carry no `exp` claim, so a
/// Google refresh response without `expires_in` has no better signal than the
/// default lifetime; both Google and OpenAI token endpoints report
/// `expires_in`, which therefore always wins.
///
/// # Errors
///
/// Returns [`ApiError`] when the reported `expires_in` is zero or negative.
fn token_expiry_from(expires_in: Option<i64>, access_token: Option<&str>) -> Result<i64, ApiError> {
    if let Some(lifetime) = expires_in
        && lifetime <= 0
    {
        return Err(ApiError::upstream_unavailable(format!(
            "malformed token response: `expires_in` must be positive, got {lifetime}"
        )));
    }

    let now = unix_time::unix_now();

    Ok(expires_in
        .or_else(|| access_token.and_then(access_token_expires_in))
        .map(|lifetime| now + lifetime)
        .unwrap_or_else(|| now + DEFAULT_TOKEN_TTL))
}

/// Extracts the `exp` claim of an access token JWT as a remaining lifetime.
fn access_token_expires_in(access_token: &str) -> Option<i64> {
    let exp = jwt_claim(access_token, &["exp"])?.as_i64()?;

    Some(exp - unix_time::unix_now())
}

/// Reads a nested claim out of a JWT payload.
fn jwt_claim(jwt: &str, path: &[&str]) -> Option<serde_json::Value> {
    let payload = jwt.split('.').nth(1)?;

    let claims: serde_json::Value =
        serde_json::from_slice(&general_purpose::URL_SAFE_NO_PAD.decode(payload).ok()?).ok()?;

    path.iter()
        .try_fold(claims, |claims, segment| claims.get(segment).cloned())
}

/// Percent-encodes a space-joined OAuth scope list for placement in an
/// authorize URL query: RFC 3986 forbids raw spaces and README §5.1/§5.2
/// document the `%20` form. Scope values are HTTPS URLs whose colons and
/// slashes are legal query characters, so spaces are the only bytes that need
/// escaping.
fn encode_scopes(scopes: &str) -> String {
    scopes.replace(' ', "%20")
}

/// Generates `byte_count` random bytes encoded as unpadded URL-safe Base64.
fn random_url_safe(byte_count: usize) -> String {
    let mut bytes = vec![0_u8; byte_count];

    rand::fill(&mut bytes[..]);

    general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Classifies a token-endpoint rejection body (RFC 6749 §5.2): a terminal
/// `invalid_grant` — a revoked or expired grant that only user
/// re-authorization recovers — is diagnosed in the message so the doomed
/// grant is not re-attempted as if it were transient, while any other
/// rejection keeps the generic upstream rendering with the status embedded.
fn token_grant_rejection(status: u16, body: &str, grant: &str) -> ApiError {
    let error_name = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("error")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        });

    if error_name.as_deref() == Some("invalid_grant") {
        return ApiError::upstream_unavailable(format!(
            "the token endpoint rejected the {grant} with `invalid_grant` (RFC 6749 §5.2): \
             the grant is revoked or expired and the provider needs re-authorization"
        ));
    }

    ApiError::upstream_rejected(status, http_body::truncate_excerpt(body))
}

/// POSTs a form-encoded body and deserializes the JSON response, classifying
/// non-success statuses through [`token_grant_rejection`] with the given
/// grant description.
///
/// Only the error path is read through the bounded error-body read; a success
/// body is legitimate JSON of unbounded size and is read in full.
///
/// # Errors
///
/// Returns [`ApiError`] for transport failures, non-success statuses, and
/// malformed response bodies.
async fn post_form<T>(
    http: &Client,
    url: &str,
    form: &[(&str, &str)],
    grant: &str,
) -> Result<T, ApiError>
where
    T: serde::de::DeserializeOwned,
{
    let response = http.post(url).form(form).send().await.map_err(|error| {
        ApiError::upstream_unavailable(format!("token endpoint unreachable: {error}"))
    })?;

    let status = response.status();

    if !status.is_success() {
        let body = http_body::read_error_body(response).await;

        return Err(token_grant_rejection(status.as_u16(), &body, grant));
    }

    let body = response.text().await.map_err(|error| {
        ApiError::upstream_unavailable(format!("token endpoint read failed: {error}"))
    })?;

    serde_json::from_str(&body).map_err(|error| {
        ApiError::upstream_unavailable(format!("malformed token endpoint response: {error}"))
    })
}

/// POSTs a JSON body to a Cloud Code Assist action of the Antigravity base
/// URL, authenticated with the given access token.
///
/// Every Antigravity Code Assist call — project resolution and model
/// discovery — routes through here, so the mandatory `User-Agent: antigravity`
/// header (README §5.2) is set in exactly one place. Only the error path is
/// read through the bounded error-body read; a success body is legitimate
/// JSON of unbounded size and is read in full.
///
/// # Errors
///
/// Returns [`ApiError`] for transport failures, non-success statuses, and
/// malformed response bodies.
pub(super) async fn post_code_assist<T>(
    http: &Client,
    action: &str,
    access_token: &str,
    body: &serde_json::Value,
) -> Result<T, ApiError>
where
    T: serde::de::DeserializeOwned,
{
    let url = format!("{}{action}", endpoints::ANTIGRAVITY_BASE_URL);

    let response = http
        .post(&url)
        .bearer_auth(access_token)
        .header(
            reqwest::header::USER_AGENT,
            endpoints::ANTIGRAVITY_USER_AGENT,
        )
        .json(body)
        .send()
        .await
        .map_err(|error| {
            ApiError::upstream_unavailable(format!("upstream unreachable: {error}"))
        })?;

    let status = response.status();

    if !status.is_success() {
        let text = http_body::read_error_body(response).await;

        return Err(ApiError::upstream_rejected(
            status.as_u16(),
            http_body::truncate_excerpt(&text),
        ));
    }

    let text = response.text().await.map_err(|error| {
        ApiError::upstream_unavailable(format!("upstream read failed: {error}"))
    })?;

    serde_json::from_str(&text).map_err(|error| {
        ApiError::upstream_unavailable(format!("malformed upstream response: {error}"))
    })
}

/// POSTs a JSON body and deserializes the JSON response, classifying
/// non-success statuses through [`token_grant_rejection`] with the given
/// grant description.
///
/// Only the error path is read through the bounded error-body read; a success
/// body is legitimate JSON of unbounded size and is read in full.
///
/// # Errors
///
/// Returns [`ApiError`] for transport failures, non-success statuses, and
/// malformed response bodies.
async fn post_json<T>(
    http: &Client,
    url: &str,
    body: &serde_json::Value,
    grant: &str,
) -> Result<T, ApiError>
where
    T: serde::de::DeserializeOwned,
{
    let response = http.post(url).json(body).send().await.map_err(|error| {
        ApiError::upstream_unavailable(format!("upstream unreachable: {error}"))
    })?;

    let status = response.status();

    if !status.is_success() {
        let text = http_body::read_error_body(response).await;

        return Err(token_grant_rejection(status.as_u16(), &text, grant));
    }

    let text = response.text().await.map_err(|error| {
        ApiError::upstream_unavailable(format!("upstream read failed: {error}"))
    })?;

    serde_json::from_str(&text).map_err(|error| {
        ApiError::upstream_unavailable(format!("malformed upstream response: {error}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{database::params, test_support};
    use std::error::Error as StdError;

    /// Verifies that random_url_safe produces the expected number of random
    /// bytes encoded in unpadded URL-safe Base64 and that consecutive
    /// outputs are distinct.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails or base64 decoding fails.
    #[test]
    fn random_url_safe_produces_valid_unpadded_base64() {
        let state = random_url_safe(32);
        let verifier = random_url_safe(64);

        #[expect(
            clippy::expect_used,
            reason = "state is generated as valid unpadded URL-safe Base64"
        )]
        let decoded_state = general_purpose::URL_SAFE_NO_PAD
            .decode(&state)
            .expect("state should be valid unpadded url-safe base64");
        assert_eq!(decoded_state.len(), 32);

        #[expect(
            clippy::expect_used,
            reason = "verifier is generated as valid unpadded URL-safe Base64"
        )]
        let decoded_verifier = general_purpose::URL_SAFE_NO_PAD
            .decode(&verifier)
            .expect("verifier should be valid unpadded url-safe base64");
        assert_eq!(decoded_verifier.len(), 64);

        let second_state = random_url_safe(32);
        assert_ne!(state, second_state);
    }

    /// Verifies that jwt_claim retrieves nested claims from a mock JWT.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn jwt_claim_retrieves_nested_values() {
        let payload = serde_json::json!({
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "test-account-id"
            },
            "exp": 2_000_000_000
        });
        let encoded_payload = general_purpose::URL_SAFE_NO_PAD.encode(payload.to_string());
        let mock_jwt = format!("header.{encoded_payload}.signature");

        let claim = jwt_claim(
            &mock_jwt,
            &["https://api.openai.com/auth", "chatgpt_account_id"],
        );
        assert_eq!(claim, Some(serde_json::json!("test-account-id")));

        let missing = jwt_claim(&mock_jwt, &["nonexistent"]);
        assert_eq!(missing, None);
    }

    /// Builds a JWT carrying the given payload claims.
    fn jwt_with_claims(claims: serde_json::Value) -> String {
        format!(
            "header.{}.signature",
            general_purpose::URL_SAFE_NO_PAD.encode(claims.to_string())
        )
    }

    /// Verifies token expiry selection: a reported `expires_in` always wins,
    /// the access token's `exp` claim is the fallback, and an opaque access
    /// token without `expires_in` falls back to the default lifetime. The
    /// opaque case is the Google refresh shape: `ya29…` tokens carry no `exp`
    /// claim, so ignoring a reported `expires_in` would stamp a day-long
    /// expiry onto an hour-lived token.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn token_expiry_prefers_expires_in_then_jwt_claim_then_default() {
        let before = unix_time::unix_now();

        let opaque_token = "ya29.opaque-google-access-token";

        #[expect(
            clippy::expect_used,
            reason = "a positive expires_in is a well-formed token response"
        )]
        let from_expires_in = token_expiry_from(Some(3_599), Some(opaque_token))
            .expect("a positive expires_in must be accepted");

        assert!(
            (from_expires_in - before - 3_599).abs() <= 1,
            "expires_in must decide the expiry"
        );

        let exp = before + 1_200;
        let jwt = jwt_with_claims(serde_json::json!({ "exp": exp }));

        #[expect(
            clippy::expect_used,
            reason = "an absent expires_in carries no malformation"
        )]
        let from_claim =
            token_expiry_from(None, Some(&jwt)).expect("an absent expires_in must be accepted");

        assert!(
            (from_claim - exp).abs() <= 1,
            "exp claim must decide the expiry"
        );

        #[expect(
            clippy::expect_used,
            reason = "a positive expires_in is a well-formed token response"
        )]
        let from_both = token_expiry_from(Some(100), Some(&jwt))
            .expect("a positive expires_in must be accepted");

        assert!(
            (from_both - before - 100).abs() <= 1,
            "expires_in must win over the exp claim"
        );

        #[expect(
            clippy::expect_used,
            reason = "an absent expires_in carries no malformation"
        )]
        let from_opaque = token_expiry_from(None, Some(opaque_token))
            .expect("an absent expires_in must be accepted");

        assert!(
            from_opaque - before - DEFAULT_TOKEN_TTL <= 1,
            "opaque token without expires_in must fall back to the default lifetime"
        );

        #[expect(
            clippy::expect_used,
            reason = "an absent expires_in carries no malformation"
        )]
        let from_nothing =
            token_expiry_from(None, None).expect("an absent expires_in must be accepted");

        assert!((from_nothing - before - DEFAULT_TOKEN_TTL).abs() <= 1);
    }

    /// Verifies a non-positive `expires_in` is rejected as a malformed token
    /// response naming the offending field: an expiry stamped at or before
    /// `now` would mark the token permanently stale and drive a refresh grant
    /// against the endpoint on every request.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn token_expiry_rejects_non_positive_expires_in() {
        for lifetime in [0, -30] {
            #[expect(
                clippy::panic,
                reason = "a rejected lifetime is the very subject of this assertion"
            )]
            let error = token_expiry_from(Some(lifetime), None)
                .err()
                .unwrap_or_else(|| panic!("expires_in {lifetime} must be rejected"));

            assert_eq!(error.status, 502);
            assert_eq!(error.code.as_deref(), Some("upstream_error"));
            assert!(
                error.message.contains("expires_in"),
                "the message must name the offending field: {error}"
            );
        }
    }

    /// Verifies a token endpoint rejection carrying the terminal
    /// `invalid_grant` error (RFC 6749 §5.2) is diagnosed as requiring
    /// re-authorization instead of surfacing as a generic transient failure:
    /// only user re-authorization recovers a revoked or expired grant, so
    /// re-attempting it on every request is futile.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn invalid_grant_rejection_requires_re_authorization() {
        let error = token_grant_rejection(
            400,
            r#"{"error":"invalid_grant","error_description":"Token has been expired or revoked."}"#,
            "refresh token",
        );

        assert_eq!(error.status, 502);
        assert_eq!(error.code.as_deref(), Some("upstream_error"));
        assert!(error.message.contains("invalid_grant"));
        assert!(error.message.contains("refresh token"));
        assert!(
            error.message.contains("re-authorization"),
            "the message must name re-authorization as the recovery: {error}"
        );
    }

    /// Verifies every other token endpoint rejection keeps the generic
    /// upstream rendering with the upstream status embedded in the message.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn other_token_rejections_keep_upstream_rendering() {
        let error = token_grant_rejection(401, r#"{"error":"invalid_client"}"#, "refresh token");

        assert_eq!(error.status, 502);
        assert!(error.message.contains("401"));
        assert!(error.message.contains("invalid_client"));
        assert_eq!(error.code.as_deref(), Some("upstream_error"));
    }

    /// Verifies a completed `onboardUser` operation propagates the terminal
    /// `google.longrunning` error payload — status and message — into the
    /// upstream error instead of collapsing it into a generic
    /// missing-project failure.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn onboard_operation_propagates_terminal_error() {
        let failed = serde_json::json!({
            "done": true,
            "error": {
                "code": 403,
                "message": "onboarding is not permitted for this account",
                "status": "PERMISSION_DENIED",
            },
        });

        #[expect(
            clippy::expect_used,
            reason = "the fixture is a terminal operation failure by construction"
        )]
        let error =
            onboard_operation_project(&failed).expect_err("a terminal operation error must fail");

        assert_eq!(error.status, 502);
        assert!(error.message.contains("PERMISSION_DENIED"));
        assert!(
            error
                .message
                .contains("onboarding is not permitted for this account")
        );
    }

    /// Verifies a completed `onboardUser` operation yields its project ID,
    /// and no project when the completion carries neither a project nor an
    /// error.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn onboard_operation_extracts_project() {
        let completed = serde_json::json!({
            "done": true,
            "response": {"cloudaicompanionProject": "proj-1"},
        });

        #[expect(
            clippy::expect_used,
            reason = "the fixture is a well-formed completed operation"
        )]
        let project = onboard_operation_project(&completed).expect("completion must parse");

        assert_eq!(project, Some("proj-1".to_string()));

        #[expect(
            clippy::expect_used,
            reason = "the fixture is a well-formed completed operation"
        )]
        let projectless = onboard_operation_project(&serde_json::json!({"done": true}))
            .expect("completion must parse");

        assert_eq!(projectless, None);
    }

    /// Verifies both authorize URLs percent-encode the space-joined scope
    /// list: RFC 3986 forbids raw spaces in a query, and README §5.1/§5.2
    /// document the `%20` form.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn authorize_urls_percent_encode_scopes() {
        #[expect(
            clippy::expect_used,
            reason = "an openai_oauth flow always begins without external input"
        )]
        let openai = begin_flow(ProviderType::OpenAiOauth).expect("openai_oauth flow should begin");

        assert!(
            openai
                .authorize_url
                .contains("scope=openid%20profile%20email%20offline_access"),
            "openai authorize URL must carry the %20-encoded scope list: {}",
            openai.authorize_url
        );

        #[expect(
            clippy::expect_used,
            reason = "an antigravity_oauth flow always begins without external input"
        )]
        let antigravity =
            begin_flow(ProviderType::AntigravityOauth).expect("antigravity flow should begin");

        assert!(
            antigravity
                .authorize_url
                .contains("scope=https://www.googleapis.com/auth/cloud-platform%20"),
            "antigravity authorize URL must carry the %20-encoded scope list: {}",
            antigravity.authorize_url
        );

        assert!(!openai.authorize_url.contains(' '));

        assert!(!antigravity.authorize_url.contains(' '));
    }

    /// Verifies a failed refresh clears the refresh lease before the error is
    /// returned, so concurrent waiters can acquire the lease instead of
    /// burning their whole wait behind a dead refresh.
    ///
    /// API key credentials fail the refresh grant without any network access,
    /// isolating the lease-release contract from the grant itself.
    ///
    /// # Panics
    ///
    /// Panics if opening the test database or an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn refresh_and_store_clears_lease_on_error() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(2, Duration::from_secs(5)).await?;

        let now = unix_time::unix_now();

        database
            .execute(
                "INSERT INTO providers (name, model_prefix, type, credentials, expires_at, \
                 refresh_started_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    "lease-provider",
                    "",
                    "openai_compatible",
                    r#"{"apiKey":"test-key"}"#,
                    Option::<i64>::None,
                    now
                ],
            )
            .await?;

        let provider = Provider {
            id: 1,
            name: "lease-provider".to_string(),
            model_prefix: String::new(),
            provider_type: ProviderType::OpenAiCompatible,
            protocol: None,
            base_url: None,
            credentials: Credentials::ApiKey("test-key".to_string()),
            expires_at: None,
            is_enabled: true,
        };

        let outcome = refresh_and_store(&database, &Client::new(), &provider, now).await;

        assert!(outcome.is_err());

        let rows = database
            .query(
                "SELECT refresh_started_at FROM providers WHERE id = ?1",
                params![1_i64],
            )
            .await?;

        let stored_lease: Option<i64> = rows.first().ok_or("missing provider row")?.get(0)?;

        assert_eq!(stored_lease, None, "the refresh lease must be cleared");

        Ok(())
    }

    /// Verifies a failed lease clear never replaces the original refresh
    /// error: the caller must observe the decisive failure (for example an
    /// upstream `invalid_grant`) instead of a generic database error from the
    /// clear that typically fails during the same outage as the persist.
    ///
    /// The single pool connection is held by an open transaction, so the lease
    /// clear fails by pool timeout while the refresh grant itself has already
    /// failed synchronously (API key credentials never refresh).
    ///
    /// # Panics
    ///
    /// Panics if opening the test database or an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn refresh_and_store_prefers_original_error_over_clear_failure()
    -> Result<(), Box<dyn StdError>> {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(1, Duration::from_millis(200)).await?;

        let now = unix_time::unix_now();

        database
            .execute(
                "INSERT INTO providers (name, model_prefix, type, credentials, expires_at, \
                 refresh_started_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    "lease-provider",
                    "",
                    "openai_compatible",
                    r#"{"apiKey":"test-key"}"#,
                    Option::<i64>::None,
                    now
                ],
            )
            .await?;

        let provider = Provider {
            id: 1,
            name: "lease-provider".to_string(),
            model_prefix: String::new(),
            provider_type: ProviderType::OpenAiCompatible,
            protocol: None,
            base_url: None,
            credentials: Credentials::ApiKey("test-key".to_string()),
            expires_at: None,
            is_enabled: true,
        };

        // Exhausts the only pool connection so the lease clear cannot run.
        let held_connection = database.transaction().await?;

        let error = refresh_and_store(&database, &Client::new(), &provider, now)
            .await
            .err()
            .ok_or("the refresh grant must fail for API key credentials")?;

        drop(held_connection);

        assert_eq!(error.status, 500);

        assert!(
            error.message.contains("API key credentials never refresh"),
            "the original refresh error must win, got: {error}"
        );

        Ok(())
    }

    /// Drives `ensure_fresh_token` through the stale-snapshot retry arm at the
    /// loop level: the first lease acquire faults with the engine's
    /// stale-snapshot busy error — exactly what a commit interleaved into the
    /// acquire's read-then-write window produces (the storage staging test
    /// `lease_acquire_after_interleaved_commit_classifies_stale_snapshot`
    /// raises it for real) — and the caller must never see that class. The
    /// loop must roll back, wait, and re-enter; the second attempt faults with
    /// an ordinary constraint error that is allowed to propagate, so the
    /// propagated 409 naming that fault proves the stale snapshot was absorbed.
    /// Removing the retry arm makes the stale-snapshot error take the
    /// pass-through instead, and this test fails on the resulting 500
    /// "database is busy".
    ///
    /// # Panics
    ///
    /// Panics if opening the test database or an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn stale_snapshot_lease_acquire_is_absorbed_by_retry_loop()
    -> Result<(), Box<dyn StdError>> {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(2, Duration::from_secs(5)).await?;

        let now = unix_time::unix_now();

        database
            .execute(
                "INSERT INTO providers (name, model_prefix, type, credentials, expires_at, \
                 refresh_started_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    "stale-snapshot-provider",
                    "",
                    "openai_oauth",
                    r#"{"accessToken":"expired","refreshToken":"refresh","accountId":"acct"}"#,
                    now - 100,
                    Option::<i64>::None,
                ],
            )
            .await?;

        let provider = Provider {
            id: 1,
            name: "stale-snapshot-provider".to_string(),
            model_prefix: String::new(),
            provider_type: ProviderType::OpenAiOauth,
            protocol: None,
            base_url: None,
            credentials: Credentials::OpenAiOauth {
                access_token: "expired".to_string(),
                refresh_token: "refresh".to_string(),
                account_id: "acct".to_string(),
            },
            expires_at: Some(now - 100),
            is_enabled: true,
        };

        // Attempt 1 hits the stale-snapshot class the retry arm must absorb;
        // attempt 2 hits an ordinary constraint error that must propagate, so
        // the propagated error distinguishes a retry from a pass-through.
        queue_lease_acquire_fault(DatabaseError::Turso(TursoError::BusySnapshot(
            "read snapshot superseded".to_string(),
        )));
        queue_lease_acquire_fault(DatabaseError::Turso(TursoError::Constraint(
            "injected constraint".to_string(),
        )));

        let error = ensure_fresh_token(&database, &Client::new(), &provider)
            .await
            .err()
            .ok_or("the second injected fault must fail the call")?;

        assert_eq!(
            error.status, 409,
            "the constraint fault must propagate, not the absorbed stale snapshot: {error}"
        );

        assert_eq!(error.code.as_deref(), Some("conflict"));

        assert!(
            error.message.contains("injected constraint"),
            "the propagated error must be the second fault: {error}"
        );

        assert!(
            !error.message.contains("database is busy"),
            "the stale-snapshot busy error must never reach the caller: {error}"
        );

        assert!(
            !has_queued_lease_acquire_fault(),
            "both faults must have been consumed by exactly two acquire attempts"
        );

        Ok(())
    }
}
