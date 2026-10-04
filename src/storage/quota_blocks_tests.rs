//! Durable availability reconciliation and stale-evidence ordering contracts.

use super::*;
use crate::{
    provider::quota, storage::test_support::insert_test_provider,
    test_support::create_test_database,
};
use std::error::Error as StdError;

/// A typed observation entry with a supplied routing applicability and measurement.
fn entry(
    key: &[&str],
    routing: quota::QuotaRouting,
    measurement: quota::QuotaMeasurement,
    reset: quota::QuotaReset,
) -> quota::QuotaEntry {
    quota::QuotaEntry {
        key: key.iter().map(|part| (*part).into()).collect(),
        label: "Display label".into(),
        scope: quota::QuotaScope::Named {
            name: "Presentation only".into(),
        },
        routing,
        measurement,
        reset,
        window: quota::QuotaWindow::Unknown,
    }
}

/// A successfully decoded full reading at the given UTC microsecond timestamp.
///
/// # Errors
///
/// Returns an error if the deliberate timestamp is outside the safe domain.
fn observation(
    at: i64,
    quotas: Vec<quota::QuotaEntry>,
) -> Result<quota::Observation, Box<dyn StdError>> {
    Ok(quota::Observation {
        observed_at: at.try_into()?,
        quotas,
    })
}

/// Unknown, omitted, and monitoring-only evidence retain blocks; positive evidence clears exact
/// identities. Canonical model sets and compact tuple keys preserve account-local applicability.
///
/// # Errors
///
/// Returns fixture and database errors.
///
/// # Panics
///
/// Panics if complete reconciliation, reset normalization, or canonical identities diverge.
#[tokio::test]
async fn observations_reconcile_only_positive_exact_evidence() -> Result<(), Box<dyn StdError>> {
    let (database, _guard) = create_test_database().await;
    let provider = insert_test_provider(&database, "A").await?;
    #[expect(clippy::expect_used, reason = "the fixture account exists")]
    let initial = load_state(&database, provider)
        .await?
        .expect("account exists");
    let reading = observation(
        100,
        vec![
            entry(
                &["account", "weekly"],
                quota::QuotaRouting::AllModels,
                quota::QuotaMeasurement::Ratio {
                    used_fraction: 1.1.try_into()?,
                },
                quota::QuotaReset::At {
                    at: 200.try_into()?,
                },
            ),
            entry(
                &["model", "M"],
                quota::QuotaRouting::Models {
                    upstream_ids: vec!["N".into(), "M".into(), "M".into()],
                },
                quota::QuotaMeasurement::Remaining {
                    remaining: 0.0.try_into()?,
                    unit: "requests".into(),
                },
                quota::QuotaReset::At { at: 50.try_into()? },
            ),
            entry(
                &["model|M"],
                quota::QuotaRouting::MonitoringOnly,
                quota::QuotaMeasurement::Ratio {
                    used_fraction: 2.0.try_into()?,
                },
                quota::QuotaReset::Unknown,
            ),
        ],
    )?;
    assert!(reconcile(&database, provider, initial.fence, &reading).await?);
    #[expect(clippy::expect_used, reason = "the account remains stored")]
    let blocked = load_state(&database, provider)
        .await?
        .expect("account exists");
    assert_eq!(blocked.blocks.len(), 2);
    #[expect(
        clippy::expect_used,
        reason = "both independently exhausted keys were applied"
    )]
    let weekly = blocked
        .blocks
        .iter()
        .find(|block| block.key == ["account", "weekly"])
        .expect("weekly block");
    assert_eq!(weekly.blocked_until, Some(200));
    #[expect(
        clippy::expect_used,
        reason = "both independently exhausted keys were applied"
    )]
    let model = blocked
        .blocks
        .iter()
        .find(|block| block.key == ["model", "M"])
        .expect("model block");
    assert_eq!(model.blocked_until, None);
    assert_eq!(
        model.applies_to,
        quota::QuotaApplicability::Models {
            upstream_ids: vec!["M".into(), "N".into()]
        }
    );
    let unknown = observation(
        150,
        vec![
            entry(
                &["account", "weekly"],
                quota::QuotaRouting::AllModels,
                quota::QuotaMeasurement::Unknown,
                quota::QuotaReset::Unknown,
            ),
            entry(
                &["model", "M"],
                quota::QuotaRouting::MonitoringOnly,
                quota::QuotaMeasurement::Unlimited,
                quota::QuotaReset::Unknown,
            ),
        ],
    )?;
    assert!(reconcile(&database, provider, blocked.fence, &unknown).await?);
    #[expect(clippy::expect_used, reason = "the account remains stored")]
    let retained = load_state(&database, provider)
        .await?
        .expect("account exists");
    assert_eq!(retained.blocks, blocked.blocks);
    let recovered = observation(
        250,
        vec![entry(
            &["account", "weekly"],
            quota::QuotaRouting::AllModels,
            quota::QuotaMeasurement::Unlimited,
            quota::QuotaReset::Unknown,
        )],
    )?;
    assert!(reconcile(&database, provider, retained.fence, &recovered).await?);
    #[expect(clippy::expect_used, reason = "the account remains stored")]
    let recovered = load_state(&database, provider)
        .await?
        .expect("account exists");
    assert_eq!(recovered.blocks.len(), 1);
    let mut state = recovered;
    for (reset, window) in [
        (
            quota::QuotaReset::Date {
                date: "2030-01-01".into(),
            },
            quota::QuotaWindow::Calendar {
                unit: crate::provider::definition::quota::QuotaCalendarUnit::Month,
            },
        ),
        (
            quota::QuotaReset::Unknown,
            quota::QuotaWindow::Duration {
                seconds: 3600.try_into()?,
            },
        ),
        (
            quota::QuotaReset::At { at: 50.try_into()? },
            quota::QuotaWindow::Duration {
                seconds: 3600.try_into()?,
            },
        ),
    ] {
        let mut restriction = entry(
            &["model", "M"],
            quota::QuotaRouting::Models {
                upstream_ids: vec!["M".into()],
            },
            quota::QuotaMeasurement::Remaining {
                remaining: 0.0.try_into()?,
                unit: "requests".into(),
            },
            reset,
        );
        restriction.window = window;
        let reading = observation(300, vec![restriction])?;
        assert!(reconcile(&database, provider, state.fence, &reading).await?);
        #[expect(clippy::expect_used, reason = "the observed account remains stored")]
        let observed = load_state(&database, provider)
            .await?
            .expect("account exists");
        state = observed;
        assert_eq!(state.blocks.len(), 1);
        assert!(
            state
                .blocks
                .iter()
                .all(|block| block.blocked_until.is_none()),
            "date and period facts cannot manufacture an absolute reset"
        );
    }
    Ok(())
}

/// Rejections advance revision even after a successful intervening read; older observations are
/// discarded atomically. Administrator reset fences every earlier generation and deleted accounts
/// are never recreated by either kind of late evidence.
///
/// # Errors
///
/// Returns fixture and database errors.
///
/// # Panics
///
/// Panics if stale evidence overwrites a rejection/reset or revives a deleted account.
#[tokio::test]
async fn generations_and_revisions_order_rejections_reads_and_resets()
-> Result<(), Box<dyn StdError>> {
    let (database, _guard) = create_test_database().await;
    let provider = insert_test_provider(&database, "A").await?;
    #[expect(clippy::expect_used, reason = "the account was just inserted")]
    let initial = load_state(&database, provider)
        .await?
        .expect("account exists");
    let reading = observation(
        100,
        vec![entry(
            &["weekly"],
            quota::QuotaRouting::AllModels,
            quota::QuotaMeasurement::Unlimited,
            quota::QuotaReset::Unknown,
        )],
    )?;
    assert!(reconcile(&database, provider, initial.fence, &reading).await?);
    assert!(
        record_rejection(
            &database,
            provider,
            initial.fence.generation,
            &["weekly".into()],
            &quota::QuotaApplicability::AllModels,
            120
        )
        .await?
    );
    assert!(!reconcile(&database, provider, initial.fence, &reading).await?);
    #[expect(clippy::expect_used, reason = "the account remains stored")]
    let rejected = load_state(&database, provider)
        .await?
        .expect("account exists");
    assert_eq!(rejected.fence.revision, initial.fence.revision + 2);
    assert_eq!(rejected.blocks.len(), 1);
    reset(&database, provider).await?;
    assert!(
        !record_rejection(
            &database,
            provider,
            initial.fence.generation,
            &["weekly".into()],
            &quota::QuotaApplicability::AllModels,
            150
        )
        .await?
    );
    assert!(!reconcile(&database, provider, rejected.fence, &reading).await?);
    #[expect(clippy::expect_used, reason = "reset preserves the account")]
    let reset_state = load_state(&database, provider)
        .await?
        .expect("account exists");
    assert_eq!(reset_state.fence.generation, initial.fence.generation + 1);
    assert!(reset_state.blocks.is_empty());
    crate::storage::providers::delete_provider(&database, provider).await?;
    assert!(!reconcile(&database, provider, reset_state.fence, &reading).await?);
    assert!(
        !record_rejection(
            &database,
            provider,
            reset_state.fence.generation,
            &["weekly".into()],
            &quota::QuotaApplicability::AllModels,
            180
        )
        .await?
    );
    assert!(load_state(&database, provider).await?.is_none());
    Ok(())
}

/// Account administration clears obsolete evidence, while label/switch edits and leased token
/// renewal preserve it. Reauthorization resets even when the exact definition stays unchanged.
///
/// # Errors
///
/// Returns fixture and database errors.
///
/// # Panics
///
/// Panics if administrator and ordinary renewal ownership boundaries are confused.
#[tokio::test]
async fn account_mutations_and_ordinary_renewal_have_distinct_quota_ownership()
-> Result<(), Box<dyn StdError>> {
    use crate::{provider::StoredCredentials, storage::providers};
    let (database, _guard) = create_test_database().await;
    let definition = crate::test_support::insert_oauth_definition(&database, "oauth").await;
    let credentials = StoredCredentials::OAuth {
        access_token: "initial".into(),
        refresh_token: Some("durable".into()),
        slots: std::collections::BTreeMap::new(),
        expires_at: Some(1),
        refresh_started_at: None,
    };
    let provider = crate::test_support::insert_provider(
        &database,
        &definition,
        "A",
        Some("https://initial.test"),
        credentials.clone(),
    )
    .await;
    assert!(
        record_rejection(
            &database,
            provider.id,
            provider.quota_generation,
            &["weekly".into()],
            &quota::QuotaApplicability::AllModels,
            100
        )
        .await?
    );
    #[expect(clippy::expect_used, reason = "the account remains present")]
    let blocked = load_state(&database, provider.id)
        .await?
        .expect("account exists");
    providers::update_provider(
        &database,
        provider.id,
        providers::ProviderUpdate {
            model_prefix: "",
            name: "renamed",
            base_url: Some("https://initial.test"),
            is_enabled: false,
            definition: None,
            credentials: None,
        },
    )
    .await?;
    #[expect(clippy::expect_used, reason = "rename preserves the account")]
    let renamed = load_state(&database, provider.id)
        .await?
        .expect("account exists");
    assert_eq!(renamed, blocked);
    let mut transaction = database.transaction().await?;
    let lease =
        providers::try_acquire_refresh_lease(&mut transaction, provider.id, 200, 0, 0).await?;
    assert_eq!(lease, Some(200));
    transaction.commit().await?;
    assert!(
        providers::persist_renewed_credentials_for_lease(&database, provider.id, 200, &credentials)
            .await?
    );
    #[expect(clippy::expect_used, reason = "renewal preserves the account")]
    let renewed = load_state(&database, provider.id)
        .await?
        .expect("account exists");
    assert_eq!(renewed, blocked);
    providers::update_provider(
        &database,
        provider.id,
        providers::ProviderUpdate {
            model_prefix: "",
            name: "renamed",
            base_url: Some("https://replacement.test"),
            is_enabled: false,
            definition: None,
            credentials: None,
        },
    )
    .await?;
    #[expect(clippy::expect_used, reason = "base replacement preserves the account")]
    let changed = load_state(&database, provider.id)
        .await?
        .expect("account exists");
    assert!(changed.blocks.is_empty());
    assert_eq!(changed.fence.generation, blocked.fence.generation + 1);
    assert!(
        record_rejection(
            &database,
            provider.id,
            changed.fence.generation,
            &["weekly".into()],
            &quota::QuotaApplicability::AllModels,
            250
        )
        .await?
    );
    let mut transaction = database.transaction().await?;
    assert!(
        providers::rebind_credentials_in_transaction(
            &mut transaction,
            provider.id,
            &definition,
            &credentials
        )
        .await?
    );
    transaction.commit().await?;
    #[expect(clippy::expect_used, reason = "reauthorization preserves the account")]
    let reauthorized = load_state(&database, provider.id)
        .await?
        .expect("account exists");
    assert!(reauthorized.blocks.is_empty());
    assert_eq!(reauthorized.fence.generation, changed.fence.generation + 1);
    Ok(())
}
