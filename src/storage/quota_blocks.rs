//! Durable account availability and generation/revision ordering of derived quota evidence.

use super::Error;
use crate::{
    database::{Database, Transaction, params},
    provider::quota,
};
use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests;

/// Captured account interpretation and derived-evidence ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct QuotaFence {
    /// Administrator mutation generation.
    pub(crate) generation: i64,
    /// Derived evidence revision within this generation.
    pub(crate) revision: i64,
}

/// One independently identified restriction that blocks physical dispatch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub(crate) struct QuotaBlock {
    /// Ordered restriction identity tuple.
    pub(crate) key: Vec<String>,
    /// Canonical resolved applicability, never a presentation label.
    pub(crate) applies_to: quota::QuotaApplicability,
    /// UTC microseconds when exhaustion was observed.
    observed_at: i64,
    /// Known future absolute recheck instant; unknown resets remain null.
    pub(crate) blocked_until: Option<i64>,
}

/// One coherent account fence and its durable restrictions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AccountQuotaState {
    /// Captured generation/revision.
    pub(crate) fence: QuotaFence,
    /// Blocks in lexicographic identity-tuple order.
    pub(crate) blocks: Vec<QuotaBlock>,
}

/// Loads current account fences and blocks from one short transaction.
///
/// # Errors
///
/// Returns database failures or malformed stored applicability.
pub(crate) async fn load_state(
    database: &Database,
    provider_id: i64,
) -> Result<Option<AccountQuotaState>, Error> {
    let mut transaction = database.transaction().await?;
    let state = match read_fence(&mut transaction, provider_id).await? {
        Some(fence) => Some(AccountQuotaState {
            fence,
            blocks: read_blocks(&mut transaction, provider_id).await?,
        }),
        None => None,
    };
    transaction.commit().await?;
    Ok(state)
}

/// Reads one account's current authoritative fence inside the caller's transaction.
///
/// # Errors
///
/// Returns database failures.
async fn read_fence(
    transaction: &mut Transaction,
    provider_id: i64,
) -> Result<Option<QuotaFence>, Error> {
    let rows = transaction
        .query(
            "SELECT quota_generation, quota_revision FROM providers WHERE id = ?1",
            params![provider_id],
        )
        .await?;
    rows.first()
        .map(|row| {
            Ok(QuotaFence {
                generation: row.get(0)?,
                revision: row.get(1)?,
            })
        })
        .transpose()
}

/// Decodes typed blocks without delimiter-based restriction identities.
///
/// # Errors
///
/// Returns database failures or malformed stored JSON.
async fn read_blocks(
    transaction: &mut Transaction,
    provider_id: i64,
) -> Result<Vec<QuotaBlock>, Error> {
    let rows = transaction
        .query(
            "SELECT restriction_key, applies_to, observed_at, blocked_until \
         FROM provider_quota_blocks WHERE provider_id = ?1",
            params![provider_id],
        )
        .await?;
    let mut blocks = Vec::with_capacity(rows.len());
    for row in &rows {
        let applicability: quota::QuotaApplicability =
            super::provider_definitions::payload_from_json(row.get(1)?, "quota applicability")?;
        blocks.push(QuotaBlock {
            key: super::provider_definitions::payload_from_json(
                row.get(0)?,
                "quota restriction identity",
            )?,
            applies_to: applicability.canonicalized(),
            observed_at: row.get(2)?,
            blocked_until: row.get(3)?,
        });
    }
    blocks.sort_by(|left, right| left.key.cmp(&right.key));
    Ok(blocks)
}

/// Reconciles a successful full reading only when both captured ordering fences still match.
/// Unknown, omitted, and monitoring-only entries retain any established block.
///
/// # Errors
///
/// Returns database failures; stale or deleted accounts produce a false application result.
pub(crate) async fn reconcile(
    database: &Database,
    provider_id: i64,
    captured: QuotaFence,
    observation: &quota::Observation,
) -> Result<bool, Error> {
    in_retryable_transaction!(database, transaction, async {
        if read_fence(&mut transaction, provider_id).await? != Some(captured) {
            return Ok::<bool, Error>(false);
        }
        let observed_at: i64 = observation.observed_at.into();
        for entry in &observation.quotas {
            let Some(applies_to) = quota::QuotaApplicability::from_routing(&entry.routing) else {
                continue;
            };
            let Some(is_exhausted) = entry.measurement.is_exhausted() else {
                continue;
            };
            if is_exhausted {
                let blocked_until = match entry.reset {
                    quota::QuotaReset::At { at } if i64::from(at) > observed_at => Some(at.into()),
                    _ => None,
                };
                upsert_block(
                    &mut transaction,
                    provider_id,
                    &QuotaBlock {
                        key: entry.key.clone(),
                        applies_to,
                        observed_at,
                        blocked_until,
                    },
                )
                .await?;
            } else {
                transaction
                    .execute(
                        "DELETE FROM provider_quota_blocks WHERE provider_id = ?1 \
                     AND restriction_key = ?2",
                        params![provider_id, encoded(&entry.key)],
                    )
                    .await?;
            }
        }
        advance_revision(&mut transaction, provider_id).await?;
        Ok::<bool, Error>(true)
    })
}

/// Records a positively classified rejected dispatch against its captured generation only.
/// A newer revision cannot suppress a real rejection, while a newer generation must suppress it.
///
/// # Errors
///
/// Returns database failures; stale generations and deleted accounts return false.
pub(crate) async fn record_rejection(
    database: &Database,
    provider_id: i64,
    captured_generation: i64,
    key: &[String],
    applies_to: &quota::QuotaApplicability,
    observed_at: i64,
) -> Result<bool, Error> {
    in_retryable_transaction!(database, transaction, async {
        let Some(current) = read_fence(&mut transaction, provider_id).await? else {
            return Ok::<bool, Error>(false);
        };
        if current.generation != captured_generation {
            return Ok::<bool, Error>(false);
        }
        // The preserved reset of this one restriction is a primary-key point lookup — the
        // account's other blocks never matter here, so none are loaded or decoded.
        let rows = transaction
            .query(
                "SELECT blocked_until FROM provider_quota_blocks \
                 WHERE provider_id = ?1 AND restriction_key = ?2",
                params![provider_id, encoded(key)],
            )
            .await?;

        // A preserved reset survives only while it stays strictly future against the NEW
        // observation — the stamp this rejection then writes, so the schema CHECK
        // `blocked_until > observed_at` holds on the upserted row by construction.
        let blocked_until = rows
            .first()
            .map(|row| row.get::<Option<i64>>(0))
            .transpose()?
            .flatten()
            .filter(|reset| *reset > observed_at);
        upsert_block(
            &mut transaction,
            provider_id,
            &QuotaBlock {
                key: key.to_vec(),
                applies_to: applies_to.clone().canonicalized(),
                observed_at,
                blocked_until,
            },
        )
        .await?;
        advance_revision(&mut transaction, provider_id).await?;
        Ok::<bool, Error>(true)
    })
}

/// Clears obsolete restrictions and advances both fences as part of an administrator mutation.
///
/// # Errors
///
/// Returns database failures.
pub(super) async fn invalidate_in_transaction(
    transaction: &mut Transaction,
    provider_id: i64,
) -> Result<(), Error> {
    transaction
        .execute(
            "UPDATE providers SET quota_generation = quota_generation + 1, \
        quota_revision = quota_revision + 1 WHERE id = ?1",
            params![provider_id],
        )
        .await?;
    transaction
        .execute(
            "DELETE FROM provider_quota_blocks WHERE provider_id = ?1",
            params![provider_id],
        )
        .await?;
    Ok(())
}

/// Applies an explicit availability reset without changing administrator serving switches.
///
/// # Errors
///
/// Returns not-found for an absent account or database failures.
///
/// # Panics
///
/// Panics if an account disappears inside its own reset transaction.
pub(crate) async fn reset(database: &Database, provider_id: i64) -> Result<QuotaFence, Error> {
    in_retryable_transaction!(database, transaction, async {
        if read_fence(&mut transaction, provider_id).await?.is_none() {
            return Err(Error::NotFound {
                message: super::provider_not_found_message(provider_id),
            });
        }
        invalidate_in_transaction(&mut transaction, provider_id).await?;
        #[expect(
            clippy::expect_used,
            reason = "the reset transaction retains its existing account"
        )]
        let fence = read_fence(&mut transaction, provider_id)
            .await?
            .expect("the reset account exists within its transaction");
        Ok::<QuotaFence, Error>(fence)
    })
}

/// Serializes a gateway-owned identity/applicability with compact, deterministic JSON.
///
/// # Panics
///
/// Panics if a gateway-owned typed value violates its serialization invariant.
fn encoded<T: Serialize + ?Sized>(value: &T) -> String {
    #[expect(
        clippy::expect_used,
        reason = "typed restriction identities and applicability always serialize"
    )]
    serde_json::to_string(value).expect("typed quota state serializes")
}

/// Upserts one exhausted restriction while preserving SQL's future-reset contract.
///
/// # Errors
///
/// Returns database failures.
async fn upsert_block(
    transaction: &mut Transaction,
    provider_id: i64,
    block: &QuotaBlock,
) -> Result<(), Error> {
    transaction
        .execute(
            "INSERT INTO provider_quota_blocks \
         (provider_id, restriction_key, applies_to, observed_at, blocked_until) \
         VALUES (?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT (provider_id, restriction_key) DO UPDATE SET \
         applies_to = excluded.applies_to, observed_at = excluded.observed_at, \
         blocked_until = excluded.blocked_until",
            params![
                provider_id,
                encoded(&block.key),
                encoded(&block.applies_to),
                block.observed_at,
                block.blocked_until
            ],
        )
        .await?;
    Ok(())
}

/// Advances derived evidence ordering exactly once per applied event.
///
/// # Errors
///
/// Returns database failures.
async fn advance_revision(transaction: &mut Transaction, provider_id: i64) -> Result<(), Error> {
    transaction
        .execute(
            "UPDATE providers SET quota_revision = quota_revision + 1 WHERE id = ?1",
            params![provider_id],
        )
        .await?;
    Ok(())
}
