//! Lease-guarded renewal tests: the pass-through classifications, the grant renewal's rotation
//! and retention semantics, the dead-grant diagnosis, and the reprovisioning renewal
//! (`docs/design/providers.md`, `docs/design/storage.md`).

#![expect(
    clippy::expect_used,
    clippy::missing_panics_doc,
    reason = "a test fails by panicking: a # Panics section on every test would restate that, \
              and a fixture that cannot parse, store, or serve is that test's failure report"
)]

use super::ensure_fresh_token;
use crate::{
    provider::{
        Error as ProviderError, StoredCredentials, oauth::tests_support,
        tests_support::failure_detail,
    },
    storage,
    test_support::{mock_upstream::MockUpstream, temp_database},
    unix_time,
};
use serde_json::json;
use std::{collections::BTreeMap, error::Error as StdError};

/// The database-clock microseconds of one expired token (well past the freshness margin).
fn expired() -> Option<i64> {
    Some(unix_time::unix_now_micros() - 3600 * unix_time::MICROS_PER_SEC)
}

/// The database-clock microseconds of one comfortably fresh token.
fn fresh() -> Option<i64> {
    Some(unix_time::unix_now_micros() + 3600 * unix_time::MICROS_PER_SEC)
}

/// Seeds one OAuth provider whose credentials carry `access_token`, `refresh_token`,
/// `expires_at`, and the `refresh_started_at` lease stamp, bound to the recipe parsed from
/// `payload`.
async fn seed(
    payload: serde_json::Value,
    access_token: &str,
    refresh_token: Option<&str>,
    expires_at: Option<i64>,
    refresh_started_at: Option<i64>,
) -> Result<
    (
        crate::database::Database,
        crate::provider::Provider,
        crate::test_support::TempDatabaseGuard,
    ),
    Box<dyn StdError>,
> {
    let (database, guard) = temp_database().await;

    let credentials = StoredCredentials::OAuth {
        access_token: access_token.to_string(),
        refresh_token: refresh_token.map(str::to_string),
        slots: BTreeMap::from([("carried".to_string(), "kept".to_string())]),
        expires_at,
        refresh_started_at,
    };

    let provider = crate::provider::tests_support::seed_provider(
        &database,
        "fixture-provider",
        payload,
        &credentials,
        Some("https://base.test"),
    )
    .await?;

    Ok((database, provider, guard))
}

/// A static API key, a `refresh: none` credential, and a credential without a scheduled
/// expiry never enter the lease: the freshly read row is returned as-is.
#[tokio::test]
async fn nonrenewable_credentials_pass_through() -> Result<(), Box<dyn StdError>> {
    let (database, _guard) = temp_database().await;

    let client = reqwest::Client::new();

    let api_payload = serde_json::json!({
        "format": "openai_chat_completion",
        "inference": {
            "url": [{"type": "literal", "value": "https://up.test/v1/chat/completions"}],
            "upstream_streaming": "match_client"
        },
        "credential": {"type": "api_key"}
    });

    let api_provider = crate::provider::tests_support::seed_provider(
        &database,
        "fixture-provider",
        api_payload,
        &crate::provider::tests_support::api_key_credentials("key-1"),
        None,
    )
    .await?;

    let renewed = ensure_fresh_token(&database, &client, &api_provider).await?;

    assert_eq!(renewed.credentials.secret(), "key-1");

    let none_payload = tests_support::code_payload("http://127.0.0.1:1");

    let none_payload = tests_support::tap_pointer(
        none_payload,
        "/credential/refresh",
        serde_json::json!({"type": "none"}),
    );

    let (database, provider, _guard) = seed(none_payload, "at-1", None, expired(), None).await?;

    let renewed = ensure_fresh_token(&database, &client, &provider).await?;

    assert_eq!(renewed.credentials.secret(), "at-1");

    let grant_payload = tests_support::code_payload("http://127.0.0.1:1");

    let (database, provider, _guard) =
        seed(grant_payload, "at-1", Some("rt-1"), None, None).await?;

    let renewed = ensure_fresh_token(&database, &client, &provider).await?;

    assert_eq!(renewed.credentials.secret(), "at-1");

    Ok(())
}

/// A token with a known future expiry never refreshes: the row is returned untouched without
/// any upstream request.
#[tokio::test]
async fn fresh_tokens_pass_through() -> Result<(), Box<dyn StdError>> {
    let payload = tests_support::code_payload("http://127.0.0.1:1");

    let (database, provider, _guard) = seed(payload, "at-1", Some("rt-1"), fresh(), None).await?;

    let renewed = ensure_fresh_token(&database, &reqwest::Client::new(), &provider).await?;

    assert_eq!(renewed.credentials.secret(), "at-1");

    Ok(())
}

/// An expired credential renews through the definition's refresh grant: the new access token
/// and a rotated refresh token persist under the lease, and the returned row carries them.
#[tokio::test]
async fn expired_tokens_refresh_and_rotate() -> Result<(), Box<dyn StdError>> {
    let (mock, _) = MockUpstream::start_json(vec![
        json!({
            "access_token": "at-2",
            "refresh_token": "rt-2",
            "expires_in": 3600
        })
        .to_string(),
    ])
    .await;

    let (database, provider, _guard) = seed(
        tests_support::code_payload(&mock.base()),
        "at-1",
        Some("rt-1"),
        expired(),
        None,
    )
    .await?;

    let renewed = ensure_fresh_token(&database, &reqwest::Client::new(), &provider).await?;

    let StoredCredentials::OAuth {
        access_token,
        refresh_token,
        expires_at,
        refresh_started_at,
        slots,
    } = &renewed.credentials
    else {
        return Err("the renewed credentials are OAuth".into());
    };

    assert_eq!(access_token, "at-2");

    assert_eq!(
        refresh_token.as_deref(),
        Some("rt-2"),
        "rotation replaces the token"
    );

    assert!(expires_at.is_some(), "the response lifetime schedules");

    let remaining = expires_at.expect("the lifetime schedules") - unix_time::unix_now_micros();

    assert!(
        remaining > 3599 * unix_time::MICROS_PER_SEC
            && remaining < 3601 * unix_time::MICROS_PER_SEC
    );

    assert_eq!(*refresh_started_at, None, "the lease cleared on success");

    assert_eq!(slots.get("carried").map(String::as_str), Some("kept"));

    Ok(())
}

/// An omitted rotation retains the stored refresh token (RFC 6749 §5.1 leaves it optional).
#[tokio::test]
async fn omitted_rotation_retains_the_stored_token() -> Result<(), Box<dyn StdError>> {
    let (mock, _) = MockUpstream::start_json(vec![
        json!({
            "access_token": "at-2",
            "expires_in": 60
        })
        .to_string(),
    ])
    .await;

    let (database, provider, _guard) = seed(
        tests_support::code_payload(&mock.base()),
        "at-1",
        Some("rt-1"),
        expired(),
        None,
    )
    .await?;

    let renewed = ensure_fresh_token(&database, &reqwest::Client::new(), &provider).await?;

    let StoredCredentials::OAuth { refresh_token, .. } = &renewed.credentials else {
        return Err("the renewed credentials are OAuth".into());
    };

    assert_eq!(refresh_token.as_deref(), Some("rt-1"));

    Ok(())
}

/// A rejection carrying a configured terminal code diagnoses the dead grant — the message
/// names the code and the remedy — and clears the lease so waiters can proceed.
#[tokio::test]
async fn terminal_rejections_diagnose_the_dead_grant() -> Result<(), Box<dyn StdError>> {
    let body = r#"{"error": "invalid_grant"}"#;

    let (mock, _) = MockUpstream::start_raw(vec![format!(
        "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    )])
    .await;

    let (database, provider, _guard) = seed(
        tests_support::code_payload(&mock.base()),
        "at-1",
        Some("rt-1"),
        expired(),
        None,
    )
    .await?;

    let renewed = ensure_fresh_token(&database, &reqwest::Client::new(), &provider).await;

    let detail = failure_detail(renewed);

    assert!(
        detail.contains("invalid_grant") && detail.contains("re-authorization"),
        "got: {detail}"
    );

    // The failed renewal cleared its lease: the stored row carries no live stamp.
    let stored = storage::providers::find_provider_by_id(&database, provider.id)
        .await?
        .expect("the provider persists");

    let StoredCredentials::OAuth {
        refresh_started_at,
        access_token,
        ..
    } = &stored.credentials
    else {
        return Err("the stored credentials are OAuth".into());
    };

    assert_eq!(*refresh_started_at, None);

    assert_eq!(access_token, "at-1", "the old credentials stand");

    Ok(())
}

/// A rejection carrying no machine code keeps the generic upstream rendering with the status.
#[tokio::test]
async fn nonterminal_rejections_render_the_status() -> Result<(), Box<dyn StdError>> {
    let (mock, _) = MockUpstream::start_raw(vec![String::from(
        "HTTP/1.1 503 Service Unavailable\r\nContent-Type: text/plain\r\nContent-Length: \
         3\r\nConnection: close\r\n\r\nout",
    )])
    .await;

    let (database, provider, _guard) = seed(
        tests_support::code_payload(&mock.base()),
        "at-1",
        Some("rt-1"),
        expired(),
        None,
    )
    .await?;

    let renewed = ensure_fresh_token(&database, &reqwest::Client::new(), &provider).await;

    let Err(ProviderError::Rejected { status, .. }) = renewed else {
        return Err("a codeless rejection carries the status".into());
    };

    assert_eq!(status, 503);

    Ok(())
}

/// A provider row deleted mid-refresh reports not-found.
#[tokio::test]
async fn vanishing_providers_report_not_found() -> Result<(), Box<dyn StdError>> {
    let (mock, _) =
        MockUpstream::start_json(vec![json!({"access_token": "at-2"}).to_string()]).await;

    let (database, provider, _guard) = seed(
        tests_support::code_payload(&mock.base()),
        "at-1",
        Some("rt-1"),
        expired(),
        None,
    )
    .await?;

    storage::providers::delete_provider(&database, provider.id).await?;

    let renewed = ensure_fresh_token(&database, &reqwest::Client::new(), &provider).await;

    assert!(
        matches!(renewed, Err(ProviderError::NotFound { .. })),
        "a vanished provider classifies as not-found"
    );

    Ok(())
}

/// The reprovisioning renewal re-runs the provisioning sequence: the extracted token and
/// absolute expiry persist, and an absent rotation retains the stored refresh token.
#[tokio::test]
async fn reprovision_renewal_runs_the_sequence() -> Result<(), Box<dyn StdError>> {
    let expiry_seconds = unix_time::unix_now() + 1800;

    let (mock, recorded) = MockUpstream::start_json(vec![
        json!({
            "token": "at-3",
            "expiry": expiry_seconds
        })
        .to_string(),
    ])
    .await;

    let (database, provider, _guard) = seed(
        tests_support::reprovision_payload(&mock.base()),
        "at-1",
        Some("rt-1"),
        expired(),
        None,
    )
    .await?;

    let renewed = ensure_fresh_token(&database, &reqwest::Client::new(), &provider).await?;

    let StoredCredentials::OAuth {
        access_token,
        refresh_token,
        expires_at,
        ..
    } = &renewed.credentials
    else {
        return Err("the renewed credentials are OAuth".into());
    };

    assert_eq!(access_token, "at-3");

    assert_eq!(
        *expires_at,
        Some(expiry_seconds * unix_time::MICROS_PER_SEC)
    );

    assert_eq!(refresh_token.as_deref(), Some("rt-1"));

    // The reprovisioning step authenticated with the stored secret, which the `secret`
    // stage value renders verbatim.
    let guard = recorded
        .lock()
        .map_err(|error| format!("the recorded requests lock: {error}"))?;

    assert_eq!(guard.len(), 1);

    let request = guard.first().expect("one request");

    assert_eq!(request.header("authorization"), Some("at-1"));

    Ok(())
}

/// A refresh lease a dead holder abandoned — its stamp older than the staleness window — is
/// stolen by the next refresher: the renewal proceeds under the recovered lease and the
/// renewed credentials persist.
#[tokio::test]
async fn a_stale_lease_is_recovered_by_the_next_refresher() -> Result<(), Box<dyn StdError>> {
    let (mock, _) = MockUpstream::start_json(vec![
        json!({"access_token": "at-2", "expires_in": 3600}).to_string(),
    ])
    .await;

    // The stamp rides 91 seconds in the past: one second past the 90-second staleness window.
    let abandoned_at = unix_time::unix_now_micros() - 91 * unix_time::MICROS_PER_SEC;

    let (database, provider, _guard) = seed(
        tests_support::code_payload(&mock.base()),
        "at-1",
        Some("rt-1"),
        expired(),
        Some(abandoned_at),
    )
    .await?;

    let renewed = ensure_fresh_token(&database, &reqwest::Client::new(), &provider).await?;

    assert_eq!(renewed.credentials.secret(), "at-2");

    let stored = storage::providers::find_provider_by_id(&database, provider.id)
        .await?
        .expect("the provider persists");

    let StoredCredentials::OAuth {
        refresh_started_at, ..
    } = &stored.credentials
    else {
        return Err("the stored credentials are OAuth".into());
    };

    assert_eq!(*refresh_started_at, None, "the lease cleared on success");

    Ok(())
}
