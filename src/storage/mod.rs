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
    /// answers drift here, so its write is swallowed as a recording failure
    /// (`docs/reference/configuration.md`) and its read fails that one request instead of
    /// killing the process (`docs/design/storage.md` *ApiLog Store*).
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

/// Renders a run of `count` numbered SQL placeholders `?first`, `?{first + 1}`, …, one per
/// binding: the single spelling the storage tier's dynamic clauses bind their value lists
/// through, so a placeholder's number is always the position of its value in the binding
/// order.
fn numbered_placeholders(count: usize, first: usize) -> String {
    (0..count)
        .map(|offset| format!("?{}", first + offset))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Renders the IN-list placeholders of a batched existence probe: one `?N` per probed id,
/// numbered from one — the single spelling the storage tier's batched reference checks bind
/// their id lists through.
fn in_placeholders(count: usize) -> String {
    numbered_placeholders(count, 1)
}

/// The documented wire wording for a rejected provider name (`docs/reference/management-api.md`),
/// owned once here so the checked rename and both management name-taken pre-checks render a taken
/// name identically.
pub(crate) fn provider_name_taken_message(name: &str) -> String {
    format!("Provider name `{name}` is already taken")
}

/// The wire wording for an unknown provider id (`docs/reference/management-api.md` 404
/// `not_found`), owned once here so the transactional writes that re-read the row and the
/// management lookups that pre-check it render a missing provider identically.
pub(crate) fn provider_not_found_message(id: i64) -> String {
    format!("Provider {id} does not exist")
}

/// The wire wording for a referenced provider-definition version that does not exist, owned once
/// here so the storage rebind path and the management error variant render a missing definition
/// identically — the same single-homing its sibling wordings carry.
pub(crate) fn provider_definition_not_found_message(name: &str, version: i64) -> String {
    format!("Provider definition `{name}` version {version} does not exist")
}

/// The wire wording for an unknown model row id (`docs/reference/management-api.md` 404
/// `not_found`), owned once here so the row-addressed writes that re-read the row render a missing
/// model identically.
fn model_not_found_message(id: i64) -> String {
    format!("Model {id} does not exist")
}
