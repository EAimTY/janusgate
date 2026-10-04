//! Shared refresh cadence, cancellation, generation, and revision race contracts.

use super::*;
use crate::{
    provider::{Provider, StoredCredentials, quota::QuotaApplicability},
    test_support::{self, MockAction, MockUpstream},
};
use std::error::Error as StdError;
use tokio::sync::Notify;

/// Creates an exact readable restriction, sharing its key with rejection evidence.
///
/// # Errors
///
/// Returns fixture serialization or database failures.
async fn account(
    database: &Database,
    base: &str,
    name: &str,
) -> Result<Provider, Box<dyn StdError>> {
    let mut payload =
        test_support::static_key_definition_payload(crate::format::UpstreamFormat::OpenAiResponse);
    payload.quota = Some(serde_json::from_value(serde_json::json!({
        "request":{"method":"GET","url":[{"type":"base"},
            {"type":"literal","value":"/quota"}]},
        "sources":[{"type":"object","key":"account","at":[],"is_required":true,
            "scope":{"type":"unknown"},"entries":[{"key":"weekly","label":"Weekly",
                "at":[],"is_required":true,"routing":{"type":"all_models"},
                "window":{"type":"unknown"},"reset":{"type":"unknown"},
                "measurement":{"type":"remaining","value":{
                    "field":{"base":"entry","path":["remaining"],"is_required":true},
                    "encoding":"number"},"unit":"requests"}}]}]
    }))?);
    let definition = test_support::insert_definition(database, name, payload).await;
    Ok(test_support::insert_provider(
        database,
        &definition,
        name,
        Some(base),
        StoredCredentials::ApiKey {
            api_key: "secret".into(),
        },
    )
    .await)
}

/// Waits for a controlled upstream read without relying on scheduling sleeps.
///
/// # Errors
///
/// Returns a timeout if the intended dispatch never arrives.
async fn dispatched(notify: &Notify) -> Result<(), Box<dyn StdError>> {
    tokio::time::timeout(Duration::from_secs(2), notify.notified()).await?;
    Ok(())
}

/// Cancellation cannot cancel the account flight; concurrent forced callers join it. A newer
/// rejection defeats its captured revision without causing another automatic read.
///
/// # Errors
///
/// Returns fixture, task, database, or query failures.
///
/// # Panics
///
/// Panics if single-flight ownership, reconciliation, or completed cadence diverges.
#[tokio::test]
async fn cancelled_refresh_still_reconciles_and_stale_reads_keep_cadence()
-> Result<(), Box<dyn StdError>> {
    test_support::install_global_configuration();
    let entered = Arc::new(Notify::new());
    let signal = Arc::clone(&entered);
    let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let calls = Arc::clone(&count);
    let upstream = MockUpstream::start_responding(move || {
        calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        signal.notify_one();
        let body = r#"{"remaining":10}"#;
        MockAction::StreamAfterDelay {
            head: format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len()),
            frame: body.into(),
            delay: Duration::from_millis(150),
        }
    })
    .await;
    let (database, _guard) = test_support::create_test_database().await;
    let provider = account(&database, &upstream.base(), "A").await?;
    let service = Arc::new(Coordinator::default());
    let client = crate::http_client::control_plane();
    let leader = {
        let service = Arc::clone(&service);
        let database = database.clone();
        let client = client.clone();
        tokio::spawn(async move { service.refresh(&database, &client, provider.id).await })
    };
    dispatched(&entered).await?;
    leader.abort();
    crate::storage::quota_blocks::record_rejection(
        &database,
        provider.id,
        0,
        &["account".into(), "weekly".into()],
        &QuotaApplicability::AllModels,
        crate::unix_time::unix_now_micros(),
    )
    .await?;
    let (left, right) = tokio::join!(
        service.refresh(&database, &client, provider.id),
        service.refresh(&database, &client, provider.id),
    );
    assert_eq!(left?.quotas.len(), 1);
    assert_eq!(right?.quotas.len(), 1);
    let state = service.check(&database, &client, provider.id, "M").await?;
    #[expect(clippy::expect_used, reason = "the account remains present")]
    let state = state.expect("account exists");
    assert_eq!(
        state.blocks.len(),
        1,
        "the earlier read cannot clear the later rejection"
    );
    assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(matches!(
        service.last_refresh(provider.id),
        LastRefresh::Succeeded { .. }
    ));
    service.refresh(&database, &client, provider.id).await?;
    #[expect(clippy::expect_used, reason = "the account remains present")]
    let state = service
        .check(&database, &client, provider.id, "M")
        .await?
        .expect("account exists");
    assert!(
        state.blocks.is_empty(),
        "a new forced positive read clears the exact key"
    );
    assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 2);
    Ok(())
}

/// Reset grace survives an older in-flight callback. A forced current-generation read waits
/// for the old slot, and failures remain throttled without erasing durable restrictions.
///
/// # Errors
///
/// Returns fixture, database, task, or query failures.
///
/// # Panics
///
/// Panics if an old generation overrides reset or if failure polling is unbounded.
#[tokio::test]
async fn reset_grace_and_failures_preserve_current_generation() -> Result<(), Box<dyn StdError>> {
    test_support::install_global_configuration();
    let release_old = Arc::new(Notify::new());
    let old_gate = Arc::clone(&release_old);
    let release_current = Arc::new(Notify::new());
    let current_gate = Arc::clone(&release_current);
    let entered = Arc::new(Notify::new());
    let signal = Arc::clone(&entered);
    let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let calls = Arc::clone(&count);
    let upstream = MockUpstream::start_responding(move || {
        let index = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        signal.notify_one();
        MockAction::RespondAfterSignal {
            response: if index == 0 {
                let body = r#"{"remaining":0}"#;
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                )
            } else {
                String::new()
            },
            release: Arc::clone(if index == 0 { &old_gate } else { &current_gate }),
        }
    })
    .await;
    let (database, _guard) = test_support::create_test_database().await;
    let provider = account(&database, &upstream.base(), "A").await?;
    let service = Arc::new(Coordinator::default());
    let client = crate::http_client::control_plane();
    let old = {
        let service = Arc::clone(&service);
        let database = database.clone();
        let client = client.clone();
        tokio::spawn(async move { service.refresh(&database, &client, provider.id).await })
    };
    dispatched(&entered).await?;
    service.reset(&database, provider.id).await?;
    #[expect(clippy::expect_used, reason = "the reset account exists")]
    let state = service
        .check(&database, &client, provider.id, "M")
        .await?
        .expect("account exists");
    assert!(state.blocks.is_empty());
    assert!(matches!(
        service.last_refresh(provider.id),
        LastRefresh::Reset { .. }
    ));
    assert!(matches!(
        service.action(provider.id, &state, None, true),
        Action::Wait {
            is_current: false,
            ..
        }
    ));
    let mut forced = Box::pin(service.refresh(&database, &client, provider.id));
    // Keep the old response gated while polling the forced caller; no second query may start.
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut forced)
            .await
            .is_err()
    );
    assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);
    release_old.notify_one();
    old.await??;
    tokio::select! {
        result = &mut forced => {
            return Err(format!("forced refresh escaped its current read gate: {result:?}").into());
        }
        result = dispatched(&entered) => result?,
    }
    assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert!(matches!(
        service.last_refresh(provider.id),
        LastRefresh::Reset { .. }
    ));
    release_current.notify_one();
    assert!(forced.await.is_err());
    crate::storage::quota_blocks::record_rejection(
        &database,
        provider.id,
        state.fence.generation,
        &["account".into(), "weekly".into()],
        &QuotaApplicability::AllModels,
        crate::unix_time::unix_now_micros(),
    )
    .await?;
    #[expect(clippy::expect_used, reason = "the reset account exists")]
    let state = service
        .check(&database, &client, provider.id, "M")
        .await?
        .expect("account exists");
    assert_eq!(state.blocks.len(), 1);
    assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert!(matches!(
        service.last_refresh(provider.id),
        LastRefresh::Failed {
            reason: RefreshFailure::Transport,
            ..
        }
    ));
    Ok(())
}

/// Deadlines follow live interval changes and exact applicable future resets. A due reset remains
/// due through revision-only evidence changes, but a completed check ignores a reset already past.
///
/// # Errors
///
/// Returns an error if the deliberate observation timestamp is outside the safe domain.
///
/// # Panics
///
/// Panics if reset scheduling, live configuration, or model applicability diverges.
#[test]
fn deadlines_preserve_due_resets_and_follow_live_intervals() -> Result<(), Box<dyn StdError>> {
    let now = Instant::now();
    let utc_now = 1_000_000;
    let mut record = MemoryRecord::unobserved(QuotaFence {
        generation: 0,
        revision: 0,
    });
    assert!(record.is_due(now, Duration::from_secs(60), Some("M")));
    record.completed_at = Some(now);
    record.last_refresh = LastRefresh::Succeeded {
        attempted_at: utc_now.try_into()?,
        observed_at: utc_now.try_into()?,
    };
    let block = QuotaBlock {
        key: vec!["model".into()],
        applies_to: QuotaApplicability::Models {
            upstream_ids: vec!["M".into()],
        },
        observed_at: utc_now,
        blocked_until: Some(utc_now + 5_000_000),
    };
    record.update_resets(std::slice::from_ref(&block), now, utc_now);
    assert!(!record.is_due(
        now + Duration::from_secs(4),
        Duration::from_secs(60),
        Some("M")
    ));
    assert!(record.is_due(
        now + Duration::from_secs(5),
        Duration::from_secs(60),
        Some("M")
    ));
    assert!(!record.is_due(
        now + Duration::from_secs(5),
        Duration::from_secs(60),
        Some("other")
    ));
    // Adding unrelated rejection evidence changes revision without losing an already-due reset.
    record.update_resets(
        std::slice::from_ref(&block),
        now + Duration::from_secs(6),
        utc_now + 6_000_000,
    );
    assert!(record.is_due(
        now + Duration::from_secs(6),
        Duration::from_secs(60),
        Some("M")
    ));
    record.resets.clear();
    record.completed_at = Some(now + Duration::from_secs(6));
    record.update_resets(&[block], now + Duration::from_secs(6), utc_now + 6_000_000);
    assert!(!record.is_due(
        now + Duration::from_secs(7),
        Duration::from_secs(60),
        Some("M")
    ));
    assert!(record.is_due(
        now + Duration::from_secs(8),
        Duration::from_secs(2),
        Some("M")
    ));
    assert!(!record.is_due(
        now + Duration::from_secs(66),
        Duration::from_secs(120),
        Some("M")
    ));
    Ok(())
}

/// Administration never polls, unsupported checks are throttled without upstream I/O, and
/// provider deletion releases completed metadata instead of accumulating account tombstones.
///
/// # Errors
///
/// Returns fixture or database failures.
///
/// # Panics
///
/// Panics if read-only views, unsupported checks, or deletion cleanup violate their contract.
#[tokio::test]
async fn unsupported_and_read_only_views_do_not_dispatch() -> Result<(), Box<dyn StdError>> {
    test_support::install_global_configuration();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = Arc::clone(&calls);
    let upstream = MockUpstream::start_responding(move || {
        count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        MockAction::Close
    })
    .await;
    let (database, _guard) = test_support::create_test_database().await;
    let definition = test_support::insert_definition(
        &database,
        "unsupported",
        test_support::static_key_definition_payload(crate::format::UpstreamFormat::OpenAiResponse),
    )
    .await;
    let provider = test_support::insert_provider(
        &database,
        &definition,
        "A",
        Some(&upstream.base()),
        StoredCredentials::ApiKey {
            api_key: "secret".into(),
        },
    )
    .await;
    let service = Arc::new(Coordinator::default());
    let client = crate::http_client::control_plane();
    #[expect(clippy::expect_used, reason = "the account exists")]
    let (_, state) = service
        .availability(&database, provider.id)
        .await?
        .expect("account exists");
    assert_eq!(state, LastRefresh::NotObserved);
    service.check(&database, &client, provider.id, "M").await?;
    service.check(&database, &client, provider.id, "M").await?;
    assert!(matches!(
        service.last_refresh(provider.id),
        LastRefresh::Unsupported { .. }
    ));
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(storage::providers::delete_provider(&database, provider.id).await?);
    assert!(
        service
            .availability(&database, provider.id)
            .await?
            .is_none()
    );
    assert_eq!(service.last_refresh(provider.id), LastRefresh::NotObserved);
    Ok(())
}

/// The control-plane timeout includes reading the quota body. A stalled successful response
/// remains a timeout, is throttled, and supplies no exhausted or available evidence.
///
/// # Errors
///
/// Returns fixture, client construction, or database failures.
///
/// # Panics
///
/// Panics if body-read timeouts are mislabeled as malformed observations or repeatedly polled.
#[tokio::test]
async fn response_body_timeouts_are_throttled_as_timeouts() -> Result<(), Box<dyn StdError>> {
    test_support::install_global_configuration();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = Arc::clone(&calls);
    let upstream = MockUpstream::start_responding(move || {
        count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        MockAction::StreamAfterDelay {
            head: "HTTP/1.1 200 OK\r\nContent-Length: 15\r\n\r\n".into(),
            frame: r#"{"remaining":0}"#.into(),
            delay: Duration::from_millis(300),
        }
    })
    .await;
    let (database, _guard) = test_support::create_test_database().await;
    let provider = account(&database, &upstream.base(), "A").await?;
    let service = Arc::new(Coordinator::default());
    let client = Client::builder()
        .timeout(Duration::from_millis(100))
        .build()?;
    assert!(
        service
            .refresh(&database, &client, provider.id)
            .await
            .is_err()
    );
    assert!(matches!(
        service.last_refresh(provider.id),
        LastRefresh::Failed {
            reason: RefreshFailure::Timeout,
            ..
        }
    ));
    #[expect(clippy::expect_used, reason = "the account remains present")]
    let state = service
        .check(&database, &client, provider.id, "M")
        .await?
        .expect("the account exists");
    assert!(state.blocks.is_empty());
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    Ok(())
}

/// Reset's post-commit callback cannot apply grace to a later credential generation or replace
/// a forced check that completed within the reset generation before the callback acquired memory.
///
/// # Errors
///
/// Returns an error if the deliberate timestamp is outside the safe domain.
///
/// # Panics
///
/// Panics if stale reset publication replaces newer interpretation or a completed forced read.
#[test]
fn reset_publication_is_fenced_against_later_mutations_and_forced_reads()
-> Result<(), Box<dyn StdError>> {
    let origin = Instant::now();
    let at = 1_000_000.try_into()?;
    let reset_fence = QuotaFence {
        generation: 1,
        revision: 1,
    };
    let mut slot = Slot::default();
    let rebound = AccountQuotaState {
        fence: QuotaFence {
            generation: 2,
            revision: 2,
        },
        blocks: vec![],
    };
    slot.install_reset(reset_fence, &rebound, origin, at);
    #[expect(
        clippy::expect_used,
        reason = "the committed generation has a memory record"
    )]
    let record = slot.record.as_ref().expect("memory exists");
    assert_eq!(record.last_refresh, LastRefresh::NotObserved);
    assert!(record.is_due(origin, Duration::from_secs(60), Some("M")));
    let mut slot = Slot::default();
    let state = AccountQuotaState {
        fence: reset_fence,
        blocks: vec![],
    };
    slot.install_reset(reset_fence, &state, origin, at);
    #[expect(clippy::expect_used, reason = "the exact reset generation has grace")]
    let record = slot.record.as_ref().expect("memory exists");
    assert!(matches!(record.last_refresh, LastRefresh::Reset { .. }));
    assert!(!record.is_due(origin, Duration::from_secs(60), Some("M")));
    let reading = Ok(Observation {
        observed_at: at,
        quotas: vec![],
    });
    #[expect(clippy::expect_used, reason = "the exact reset generation has memory")]
    let record = slot.record.as_mut().expect("memory exists");
    record.last_refresh = outcome(at, &reading);
    record.completed_at = Some(origin + Duration::from_secs(1));
    slot.install_reset(reset_fence, &state, origin, at);
    #[expect(clippy::expect_used, reason = "the completed check retains memory")]
    let record = slot.record.as_ref().expect("memory exists");
    assert!(matches!(record.last_refresh, LastRefresh::Succeeded { .. }));
    Ok(())
}
