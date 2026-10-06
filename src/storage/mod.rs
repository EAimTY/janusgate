//! The storage tier: the single home of every Turso statement and of the shared storage error
//! domain.
//!
//! Every database statement outside the frozen schema lives here, one module per table:
//! [`provider_definitions`], [`providers`], [`models`], [`oauth_flows`], [`api_keys`], and
//! [`api_logs`]. This module owns the shared error domain ([`Error`]) every surface maps onto its
//! own taxonomy, and shared transaction coordination. Each domain module owns its row codecs and
//! Turso value conversions. The api-side code (outward authentication, the gateway pipeline, the
//! management surfaces) depends on this tier one-way — the error taxonomy in particular is
//! provider-agnostic, so it lives here rather than under `provider`.
//!
//! The provider domain interlocks with this tier in both directions, acknowledged rather than
//! hidden: storage owns every statement and projects its rows onto the domain types it imports
//! from [`crate::provider`], while the OAuth and discovery engines — the domain's executors — call
//! storage to read and write those same rows.
//!
//! Physical model writes and OAuth commits re-read their account inside short transactions.
//! Public route selection and listing share a coherent account/model/membership snapshot.

use crate::database::{Database, Error as DatabaseError, IntoParams, Row, Transaction, TursoError};

/// How many times `in_retryable_transaction!` runs one read-then-write attempt, counting the
/// first: three attempts absorb two intervening commits on top of the initial one.
///
/// A stale-snapshot refusal needs a concurrent commit to land inside one attempt's own
/// read-to-write span — the body's runtime — so the refusals against one management write are
/// exactly as frequent as concurrent writes are, and two retries cover any plausible burst of
/// concurrent token-refresh lease persists. The bound is a safety net, not a spin loop: after
/// it, the last error surfaces exactly as it did before the retry existed.
const RETRYABLE_TRANSACTION_ATTEMPTS: usize = 3;

/// Runs one management read-then-write transaction, re-running the whole attempt while the
/// engine refuses it with the stale-snapshot busy error — a concurrent commit landed between
/// the attempt's read snapshot and its first write, a refusal busy timeouts never help. `$body`
/// is an async block over the attempt's `$transaction` binding, re-created on a fresh
/// transaction per attempt: every converted entry point reads inside it and re-validates its
/// conditional outcomes (404s and conflicts) against those fresh reads, so a retry observes
/// the newer committed state instead of writing through it, and nothing external happens
/// inside these transactions that a retry would repeat. The engine's unique constraints remain
/// the concurrent backstop behind the checked conflicts either way.
///
/// The attempt bound is `RETRYABLE_TRANSACTION_ATTEMPTS`; after it, the last error surfaces
/// exactly as an unretryable failure would. On any failed attempt the transaction is rolled
/// back before the retry or the return: a failed rollback discards the pooled connection by
/// `Transaction::rollback`'s contract, so the outcome is safe either way, and the failed
/// attempt's own error stays authoritative — the next begin, or the return, surfaces any real
/// connection failure in its place.
///
/// The loop lives in a macro because the body's future borrows both the per-attempt
/// transaction and the caller's own captured state: a closure literal's return type cannot
/// carry the argument's lifetime, so the loop cannot be one generic function over that shape —
/// while the local expansion, splicing the body over a caller-named binding, types it exactly.
macro_rules! in_retryable_transaction {
    ($database:expr, $transaction:ident, $body:expr) => {{
        let mut remaining_attempts = $crate::storage::RETRYABLE_TRANSACTION_ATTEMPTS;

        loop {
            let mut $transaction = $database.transaction().await?;

            // The body's future borrows `$transaction`; binding the awaited result ends that
            // borrow before the commit or rollback arms below consume the transaction.
            let outcome = $body.await;

            match outcome {
                Ok(value) => {
                    break $transaction
                        .commit()
                        .await
                        .map_err($crate::storage::Error::from)
                        .map(|()| value);
                }

                Err(error) => {
                    // The failed attempt's own error stays authoritative: a failed rollback
                    // discards the pooled connection by contract, and the next begin — or the
                    // return below — surfaces any real connection failure in its place.
                    let _ = $transaction.rollback().await;

                    remaining_attempts -= 1;

                    if !error.is_stale_snapshot() || remaining_attempts == 0 {
                        break Err(error);
                    }
                }
            }
        }
    }};
}

#[cfg(test)]
mod api_key_access_tests;
pub(crate) mod api_keys;
pub(crate) mod api_logs;
mod automatic_routes;
pub(crate) mod models;
pub(crate) mod oauth_flows;
pub(crate) mod provider_definitions;
pub(crate) mod providers;
pub(crate) mod quota_blocks;
mod route_listing;
pub(crate) mod route_snapshot;
pub(crate) mod routing;
#[cfg(test)]
mod test_support;

/// Provider, model, and OAuth state persistence failures.
///
/// Semantic outcomes the checked operations promise — a missing row, a conflicting write — are
/// their own variants, while every engine failure is classified once here so the HTTP surfaces
/// never inspect the database error domain themselves.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    /// Repeated authored rule identity; equal decisions are duplicates too.
    #[error("duplicate API key access reference: {reference:?}")]
    DuplicateAccessReference {
        /// Typed collection and rejected identity, never the key secret.
        reference: api_keys::AccessReference,
    },
    /// A referenced permission target vanished or never existed.
    #[error("API key access reference does not exist: {reference:?}")]
    MissingAccessReference {
        /// Typed collection and missing identity.
        reference: api_keys::AccessReference,
    },
    /// The authenticated key was removed or disabled before the admission snapshot.
    #[error("authenticated API key {id} is missing or disabled")]
    InactiveApiKey {
        /// Previously matched identity.
        id: i64,
    },

    /// A checked lookup or precondition failed: the addressed row does not exist.
    #[error("{message}")]
    NotFound {
        /// Human-readable failure message.
        message: String,
    },

    /// The write collides with existing state.
    #[error("{message}")]
    Conflict {
        /// Human-readable failure message.
        message: String,
    },

    /// The engine rejected a write with a constraint violation — the concurrent backstop behind a
    /// checked conflict.
    #[error("database constraint violated: {detail}")]
    Constraint {
        /// Engine-provided violation detail.
        detail: String,
    },

    /// Database contention: the file lock stayed busy until the write's busy timeout elapsed.
    ///
    /// The engine's stale-snapshot refusal classifies separately — through
    /// [`Error::is_stale_snapshot`] — because this tier's conversion deliberately parks it under
    /// [`Error::Database`] instead of flattening it in here; every rendered surface treats both
    /// as internal failures.
    #[error("database is busy: {detail}")]
    Busy {
        /// Engine-provided contention detail.
        detail: String,
    },

    /// Stored data violated an invariant written by the validated management path — database
    /// drift — or a database file is not interpretable under its schema registry: an api-log
    /// day file carrying an unknown schema version or another registry's application-id stamp
    /// answers drift here, so its write is swallowed as a recording failure (README §8) and
    /// its read fails that one request instead of killing the process (README §3 *ApiLog
    /// Store*).
    #[error("{message}")]
    Drift {
        /// Human-readable failure message.
        message: String,
    },

    /// The database engine failed.
    #[error("database error: {0}")]
    Database(DatabaseError),
}

impl From<TursoError> for Error {
    fn from(error: TursoError) -> Self {
        Self::Database(DatabaseError::Turso(error))
    }
}

impl From<DatabaseError> for Error {
    fn from(error: DatabaseError) -> Self {
        match error {
            DatabaseError::Turso(TursoError::Constraint(detail)) => Self::Constraint { detail },

            // The stale-snapshot refusal deliberately stays under `Database` instead of
            // flattening into `Busy`: the transactional surfaces must still tell the two
            // contention kinds apart after the conversion (see [`Error::is_stale_snapshot`]),
            // and every rendered surface treats both as internal failures either way.
            DatabaseError::Turso(TursoError::Busy(detail)) => Self::Busy { detail },

            // A database file this build cannot interpret under its registry — an api-log day
            // file carrying an unknown version or a foreign stamp — is stored file state, not
            // an engine failure: it classifies as drift so its request fails explicitly.
            DatabaseError::SchemaDrift { detail } => Self::Drift { message: detail },

            error => Self::Database(error),
        }
    }
}

impl Error {
    /// Whether this failure is the engine's stale-snapshot busy refusal: a read-then-write
    /// transaction attempted its first write after a concurrent commit invalidated its read
    /// snapshot ("database snapshot is stale, rollback and retry"; busy timeouts never help).
    /// The transaction must be rolled back and the whole attempt retried, re-reading the newer
    /// state — `in_retryable_transaction!` owns that bounded retry for the management
    /// read-then-write writes, and the refresh lease classifies the same refusal as "not
    /// acquired" rather than as failed work.
    fn is_stale_snapshot(&self) -> bool {
        matches!(
            self,
            Self::Database(DatabaseError::Turso(TursoError::BusySnapshot(_)))
        )
    }
}

/// The pooled or transactional SQL executor behind the storage tier's shared statements: a
/// borrowed [`Database`] runs a statement against a pooled connection, an exclusively borrowed
/// [`Transaction`] against its checked-out connection. Statements used by both executors are
/// written once as functions generic over this abstraction instead of being hand-mirrored with
/// duplicated SQL and parameter lists, matching the database module's own delegation of both
/// surfaces onto shared free functions.
trait SqlExecutor {
    /// Executes one row-returning statement, returning all rows eagerly materialized.
    ///
    /// # Errors
    ///
    /// Returns [`DatabaseError`] if preparing or executing the statement fails.
    async fn query(
        &mut self,
        sql: impl AsRef<str>,
        params: impl IntoParams,
    ) -> Result<Vec<Row>, DatabaseError>;
}

impl SqlExecutor for &Database {
    async fn query(
        &mut self,
        sql: impl AsRef<str>,
        params: impl IntoParams,
    ) -> Result<Vec<Row>, DatabaseError> {
        (**self).query(sql, params).await
    }
}

impl SqlExecutor for &mut Transaction {
    async fn query(
        &mut self,
        sql: impl AsRef<str>,
        params: impl IntoParams,
    ) -> Result<Vec<Row>, DatabaseError> {
        (**self).query(sql, params).await
    }
}

/// Renders the IN-list placeholders of a batched existence probe: one `?N` per probed id,
/// numbered from one — the single spelling the storage tier's batched reference checks bind
/// their id lists through.
fn in_placeholders(count: usize) -> String {
    (1..=count)
        .map(|index| format!("?{index}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The documented wire wording for a rejected provider name (README §7.1), owned once here so the
/// checked rename and both management name-taken pre-checks render a taken name identically.
pub(crate) fn provider_name_taken_message(name: &str) -> String {
    format!("Provider name `{name}` is already taken")
}

/// The wire wording for an unknown provider id (README §7 404 `not_found`), owned once here so the
/// transactional writes that re-read the row and the management lookups that pre-check it render a
/// missing provider identically.
pub(crate) fn provider_not_found_message(id: i64) -> String {
    format!("Provider {id} does not exist")
}

/// The wire wording for a referenced provider-definition version that does not exist, owned once
/// here so the storage rebind path and the management error variant render a missing definition
/// identically — the same single-homing its sibling wordings carry.
pub(crate) fn provider_definition_not_found_message(name: &str, version: i64) -> String {
    format!("Provider definition `{name}` version {version} does not exist")
}

/// The wire wording for an unknown model row id (README §7 404 `not_found`), owned once here so the
/// row-addressed writes that re-read the row render a missing model identically.
pub(crate) fn model_not_found_message(id: i64) -> String {
    format!("Model {id} does not exist")
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::panic,
        reason = "a fixture violating its own invariant fails the test loudly"
    )]

    use super::{test_support::oauth_definition, *};
    use crate::{
        database::{Value, params::IntoValue},
        provider::{
            CredentialKind,
            definition::credential::{CredentialAcquisition, TokenRefresh},
        },
        test_support,
    };
    use std::{cell::Cell, error::Error as StdError};

    /// Synthesizes the engine's stale-snapshot refusal through the real constructor path: the
    /// `BusySnapshot` Turso variant, wrapped once into the storage error domain exactly as a
    /// refused read-then-write upgrade classifies.
    fn stale_snapshot_error() -> Error {
        Error::from(DatabaseError::Turso(TursoError::BusySnapshot(
            "database snapshot is stale, rollback and retry".to_string(),
        )))
    }

    /// Verifies the retryable-transaction loop's happy retry: a body whose first attempt fails
    /// with a synthesized stale-snapshot refusal runs a second time on a fresh transaction, and
    /// that second attempt's write commits — the discarded first attempt leaves nothing behind
    /// and the pool stays healthy.
    ///
    /// # Panics
    ///
    /// Panics if database operations or assertions fail.
    #[tokio::test]
    async fn stale_snapshot_retry_succeeds_on_the_second_attempt() -> Result<(), Box<dyn StdError>>
    {
        let (database, _guard) = test_support::create_test_database().await;

        let attempts = &Cell::new(0);

        in_retryable_transaction!(&database, transaction, async {
            attempts.set(attempts.get() + 1);

            if attempts.get() == 1 {
                return Err(stale_snapshot_error());
            }

            transaction
                .execute(
                    "INSERT INTO api_keys (name, key, is_enabled, created_at) \
                     VALUES ('written-on-retry', 'sk-retry', 1, 1)",
                    (),
                )
                .await?;

            Ok(())
        })?;

        assert_eq!(attempts.get(), 2, "the refusal runs the body a second time");

        assert_eq!(
            api_keys::list_api_keys(&database).await?.len(),
            1,
            "the retry attempt's commit landed"
        );

        Ok(())
    }

    /// Verifies the attempt bound: a body that fails with a stale-snapshot refusal every time
    /// runs exactly [`RETRYABLE_TRANSACTION_ATTEMPTS`] times, and the last refusal surfaces
    /// unchanged — the bound exhausts instead of spinning.
    ///
    /// # Panics
    ///
    /// Panics if database operations or assertions fail.
    #[tokio::test]
    async fn stale_snapshot_retry_surfaces_the_last_error_after_the_attempt_bound()
    -> Result<(), Box<dyn StdError>> {
        let (database, _guard) = test_support::create_test_database().await;

        let attempts = &Cell::new(0);

        let outcome: Result<(), Error> =
            in_retryable_transaction!(&database, _transaction, async {
                attempts.set(attempts.get() + 1);

                Err::<(), Error>(stale_snapshot_error())
            });

        assert!(
            matches!(outcome, Err(ref error) if error.is_stale_snapshot()),
            "the exhausted bound surfaces the last stale-snapshot error"
        );

        assert_eq!(
            attempts.get(),
            RETRYABLE_TRANSACTION_ATTEMPTS,
            "the bound caps the attempts"
        );

        Ok(())
    }

    /// Verifies the retry's selectivity: a non-stale failure — a 404 the body's own
    /// re-validation produced — surfaces unchanged after a single attempt, never retried.
    ///
    /// # Panics
    ///
    /// Panics if database operations or assertions fail.
    #[tokio::test]
    async fn stale_snapshot_retry_passes_non_stale_failures_through_without_retrying()
    -> Result<(), Box<dyn StdError>> {
        let (database, _guard) = test_support::create_test_database().await;

        let attempts = &Cell::new(0);

        let outcome: Result<(), Error> =
            in_retryable_transaction!(&database, _transaction, async {
                attempts.set(attempts.get() + 1);

                Err::<(), Error>(Error::NotFound {
                    message: "the row is gone".to_string(),
                })
            });

        assert!(
            matches!(outcome, Err(Error::NotFound { message }) if message == "the row is gone"),
            "a non-stale failure surfaces unchanged"
        );

        assert_eq!(attempts.get(), 1, "a non-stale failure is never retried");

        Ok(())
    }

    /// Verifies the stale-snapshot classification against the real engine: a transaction whose
    /// read snapshot a concurrent commit invalidates fails its first write with the engine's
    /// stale-snapshot refusal, and the storage error domain classifies it as such — the failure
    /// [`in_retryable_transaction`] retries on and the exact interleaving a concurrent
    /// lease persist produces against a management read-then-write transaction.
    ///
    /// # Panics
    ///
    /// Panics if database operations or assertions fail.
    #[tokio::test]
    async fn a_write_refused_after_a_concurrent_commit_classifies_as_stale_snapshot()
    -> Result<(), Box<dyn StdError>> {
        let (database, _guard) = test_support::create_test_database().await;

        // The reader pins its snapshot with a read; the writer commits between that read and
        // the reader's first write.
        let mut reader = database.transaction().await?;

        reader.query("SELECT id FROM api_keys", ()).await?;

        let mut writer = database.transaction().await?;

        writer
            .execute(
                "INSERT INTO api_keys (name, key, is_enabled, created_at) \
                 VALUES ('concurrent', 'sk-concurrent', 1, 1)",
                (),
            )
            .await?;

        writer.commit().await?;

        let refused = reader.execute(
            "INSERT INTO api_keys (name, key, is_enabled, created_at) \
             VALUES ('stale', 'sk-stale', 1, 1)",
            (),
        );

        #[expect(
            clippy::expect_used,
            reason = "the stale upgrade must fail; a success is the failure report"
        )]
        let error = refused
            .await
            .expect_err("the write after a concurrent commit must be refused");

        assert!(
            Error::from(error).is_stale_snapshot(),
            "the refused upgrade classifies as a stale snapshot"
        );

        // The refused transaction is dead; dropping it discards its connection, the documented
        // path for an unfinished handle.
        drop(reader);

        Ok(())
    }

    /// Verifies the credential kind conversions: the storage form and the wire form are one
    /// vocabulary.
    ///
    /// # Panics
    ///
    /// Panics if value serialization fails.
    #[test]
    fn converts_credential_kind_database_values() {
        for kind in [CredentialKind::ApiKey, CredentialKind::OAuth] {
            #[expect(clippy::unwrap_used, reason = "into_value returns Ok for the kind")]
            let value = kind.into_value().unwrap();
            assert_eq!(value, Value::Text(kind.as_str().to_string()));
        }
    }

    /// Verifies that the schema itself enforces the `provider_definitions.format` closed domain:
    /// a drifted value is rejected at write time and classifies as a storage constraint error.
    ///
    /// # Panics
    ///
    /// Panics if the drifted insert unexpectedly succeeds.
    #[tokio::test]
    async fn rejects_unknown_definition_format_at_write_time() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) = test_support::create_test_database().await;

        let outcome = database
            .execute(
                "INSERT INTO provider_definitions (name, version, format, inference, credential, \
                 created_at) VALUES ('drifted', 1, 'bogus_format', '{}', '{}', 1)",
                (),
            )
            .await;

        #[expect(clippy::expect_used, reason = "the schema CHECK rejects the drift")]
        let error = outcome.expect_err("drifted format must violate the schema CHECK");

        assert!(
            matches!(Error::from(error), Error::Constraint { .. }),
            "a schema CHECK violation must classify as a storage constraint error"
        );

        Ok(())
    }

    /// The refresh-mode of the fixture OAuth definition is `none`: the engine never enters the
    /// lease for it, which the renewable predicate pins.
    ///
    /// # Panics
    ///
    /// Panics if the fixture disagrees with the predicate.
    #[test]
    fn the_none_refresh_mode_never_renews() {
        let definition = oauth_definition();

        let CredentialAcquisition::OAuth(recipe) = &definition.payload.credential else {
            panic!("the fixture carries OAuth credentials");
        };

        assert!(matches!(recipe.refresh, TokenRefresh::None));
    }
}
