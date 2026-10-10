//! OAuth authorization-flow persistence: the unconsumed-flow lifecycle of
//! `ongoing_oauth_flows` (`docs/design/storage.md` and `docs/design/providers.md`).
//!
//! *Ongoing* means unconsumed: a flow row is deleted by its completion (an authorization-code
//! exchange, or the management client retrieving a terminal device result) or by the expiry
//! sweep — never by any other path. A terminal device result stays until the management client
//! retrieves it or the flow expires. Device polling claims a due flow with a conditional lease
//! (`device_poll_started_at`), so competing pollers never double-exchange an upstream grant
//! concurrently; the lease alone cannot cover a poller that dies after its exchange completed,
//! so the claimant persists the exchange checkpoint the moment the exchange completes, and a
//! restart that recovers the claim resumes from it into provisioning and commit instead of
//! re-presenting the consumed device code (`docs/design/storage.md` and
//! `docs/reference/management-api.md`).

use super::Error;
use crate::{
    database::{Database, Transaction, params},
    unix_time,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, num::NonZeroI64, sync::LazyLock};

/// The column order of flow selects — the table's own order — consumed positionally by
/// [`flow_from_row`].
const FLOW_COLUMNS: &str = "id, provider_definition_name, provider_definition_version, \
     state_token, purpose, provider_name, model_prefix, \
     base_url, target_provider_id, grant_type, slots, code_verifier, device_code, \
     device_user_code, device_verification_uri, device_verification_uri_complete, \
     device_poll_interval_seconds, device_next_poll_at, device_poll_started_at, \
     device_exchange_checkpoint, outcome, result_provider_id, failure_code, \
     failure_message, expires_at";

/// The unexpired-flow read a state token identifies without consuming: a fully fixed statement
/// over [`FLOW_COLUMNS`], built once.
static FIND_FLOW_BY_STATE_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "SELECT {FLOW_COLUMNS} FROM ongoing_oauth_flows WHERE state_token = ?1 AND expires_at > ?2"
    )
});

/// The atomic consume — the single `DELETE … RETURNING` of the unexpired flow a state token
/// identifies: a fully fixed statement, built once.
static CONSUME_FLOW_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "DELETE FROM ongoing_oauth_flows WHERE state_token = ?1 AND expires_at > ?2 \
         RETURNING {FLOW_COLUMNS}"
    )
});

/// The terminal-flow consume — the single `DELETE … RETURNING` of an unexpired flow carrying a
/// pending result: a fully fixed statement, built once.
static CONSUME_TERMINAL_FLOW_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "DELETE FROM ongoing_oauth_flows WHERE state_token = ?1 AND expires_at > ?2 \
         AND outcome IS NOT NULL RETURNING {FLOW_COLUMNS}"
    )
});

/// The due device-flow claim — the single conditional `UPDATE … RETURNING`: a fully fixed
/// statement, built once.
static CLAIM_DUE_DEVICE_FLOW_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "UPDATE ongoing_oauth_flows SET device_poll_started_at = ?1 WHERE id = (\
         SELECT id FROM ongoing_oauth_flows WHERE outcome IS NULL AND grant_type = ?2 \
         AND expires_at > ?1 AND device_next_poll_at <= ?1 \
         AND (device_poll_started_at IS NULL OR device_poll_started_at < ?3) \
         ORDER BY device_next_poll_at LIMIT 1) RETURNING {FLOW_COLUMNS}"
    )
});

/// What a flow is for: creating a new provider, or reauthorizing (and optionally rebinding) one
/// existing provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub(crate) enum FlowPurpose {
    /// The flow creates a new provider when it completes.
    Create,

    /// The flow replaces the credentials — and optionally the definition reference — of one
    /// existing provider.
    Reauthorize,
}

impl FlowPurpose {
    /// Every variant in canonical order: the closed table [`Self::parse`] searches. The storage
    /// words live once each, in the exhaustive [`Self::as_str`] match. A new variant joins
    /// `ALL`; a partial table update is a compile error (the length pin), and the exhaustive
    /// match is what forces the variant's spelling to be stated.
    const ALL: [Self; 2] = [Self::Create, Self::Reauthorize];

    /// The storage form of the purpose.
    #[must_use]
    const fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Reauthorize => "reauthorize",
        }
    }

    /// Parses the storage form of the purpose; anything else is storage drift.
    fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|purpose| purpose.as_str() == value)
    }
}

/// The grant kind a flow runs, pinned at begin time and always matched against the referenced
/// definition version's grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub(crate) enum FlowGrant {
    /// The authorization-code grant: the administrator pastes back the redirected code.
    AuthorizationCode,

    /// The device grant: the poller exchanges the device code until approval or expiry.
    Device,
}

impl FlowGrant {
    /// Every variant in canonical order: the closed table [`Self::parse`] searches. The storage
    /// words live once each, in the exhaustive [`Self::as_str`] match. A new variant joins
    /// `ALL`; a partial table update is a compile error (the length pin), and the exhaustive
    /// match is what forces the variant's spelling to be stated.
    const ALL: [Self; 2] = [Self::AuthorizationCode, Self::Device];

    /// The storage form of the grant kind.
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::AuthorizationCode => "authorization_code",
            Self::Device => "device",
        }
    }

    /// Parses the storage form of the grant kind; anything else is storage drift.
    fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|grant| grant.as_str() == value)
    }
}

/// The terminal outcome of a flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub(crate) enum FlowOutcome {
    /// The flow's provider write committed.
    Succeeded,

    /// The flow failed with a gateway-owned failure code and message.
    Failed,
}

impl FlowOutcome {
    /// Every variant in canonical order: the closed table [`Self::parse`] searches. The storage
    /// words live once each, in the exhaustive [`Self::as_str`] match. A new variant joins
    /// `ALL`; a partial table update is a compile error (the length pin), and the exhaustive
    /// match is what forces the variant's spelling to be stated.
    const ALL: [Self; 2] = [Self::Succeeded, Self::Failed];

    /// The storage form of the outcome.
    #[must_use]
    const fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
        }
    }

    /// Parses the storage form of the outcome; anything else is storage drift.
    fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|outcome| outcome.as_str() == value)
    }
}

/// One ongoing (unconsumed) OAuth authorization flow.
#[derive(Debug, Clone)]
pub(crate) struct OngoingFlow {
    /// Row identifier.
    pub(crate) id: i64,

    /// The exact definition name the flow's OAuth recipe is read from; the flow stores no
    /// duplicate recipe snapshot.
    pub(crate) definition_name: String,

    /// The exact definition version the flow's OAuth recipe is read from.
    pub(crate) definition_version: NonZeroI64,

    /// The opaque random state token identifying the flow; the management API names it `state`.
    pub(crate) state_token: String,

    /// What the flow is for.
    pub(crate) purpose: FlowPurpose,

    /// The new provider's name, for a create flow.
    provider_name: Option<String>,

    /// Direct-model prefix for a create flow; absent for reauthorization.
    model_prefix: Option<String>,

    /// The working base URL: the creation's initial instance URL, or the target provider's
    /// stored one, replaceable by an exchange or provisioning extraction before commit.
    pub(crate) base_url: Option<String>,

    /// The reauthorized provider's row identifier, for a reauthorize flow.
    target_provider_id: Option<i64>,

    /// The grant kind the flow runs.
    pub(crate) grant_type: FlowGrant,

    /// The working slots, initialized at begin and transferred to the provider on success.
    pub(crate) slots: BTreeMap<String, String>,

    /// The PKCE verifier of an authorization-code flow.
    pub(crate) code_verifier: Option<String>,

    /// The device code issued by the authorization response.
    pub(crate) device_code: Option<String>,

    /// The human-readable user code.
    pub(crate) device_user_code: Option<String>,

    /// The verification URI, when the response carried one.
    pub(crate) device_verification_uri: Option<String>,

    /// The verification URI carrying the user code, when carried.
    pub(crate) device_verification_uri_complete: Option<String>,

    /// The stored poll interval, in seconds.
    pub(crate) device_poll_interval_seconds: Option<i64>,

    /// The next attempt instant, in UTC microseconds.
    pub(crate) device_next_poll_at: Option<i64>,

    /// The claim stamp of the poller currently exchanging.
    pub(crate) device_poll_started_at: Option<i64>,

    /// The JSON checkpoint of the exchange the held claim completed: the mapped token fields,
    /// persisted before provisioning runs, so a restart that recovers the claim resumes into
    /// provisioning and commit instead of re-presenting the consumed device code. `None` until
    /// an exchange completes under the claim.
    pub(crate) device_exchange_checkpoint: Option<String>,

    /// The terminal outcome; `None` while pending.
    pub(crate) outcome: Option<FlowOutcome>,

    /// The provider row the successful flow produced; a bare id without a foreign key, so a
    /// later provider deletion cannot strand the result row.
    result_provider_id: Option<i64>,

    /// The gateway-owned failure code of a failed flow; never an upstream body.
    failure_code: Option<String>,

    /// The gateway-owned failure message of a failed flow; never an upstream body.
    failure_message: Option<String>,

    /// Unix timestamp in microseconds after which the flow is invalid; the authorization-code
    /// TTL is fixed at ten minutes, a device flow's comes from its authorization response or the
    /// definition's explicit fallback.
    pub(crate) expires_at: i64,
}

impl OngoingFlow {
    /// The new provider's name of a create flow.
    ///
    /// The schema's cross-column `CHECK`s pin `provider_name` non-null exactly while `purpose`
    /// is `create` (`docs/design/storage.md`), so only a create flow reads it; any other read is
    /// storage drift and fails fast instead of completing a flow under an empty name.
    ///
    /// # Panics
    ///
    /// Panics when a non-create flow reads the name — the stored state the `CHECK`s make
    /// unrepresentable.
    #[must_use]
    pub(crate) fn create_provider_name(&self) -> &str {
        #[expect(
            clippy::expect_used,
            reason = "the schema's cross-column checks pin a create flow's provider name non-null"
        )]
        self.provider_name
            .as_deref()
            .expect("a create flow names its new provider")
    }

    /// Reads the prefix guaranteed present by the create-flow schema invariant.
    ///
    /// # Panics
    ///
    /// Panics if a create flow violates its durable field-presence invariant.
    pub(crate) fn create_model_prefix(&self) -> &str {
        #[expect(
            clippy::expect_used,
            reason = "create flows require a durable model prefix"
        )]
        self.model_prefix
            .as_deref()
            .expect("create flow has a model prefix")
    }

    /// The target provider row of a reauthorize flow.
    ///
    /// The schema's cross-column `CHECK`s pin `target_provider_id` non-null exactly while
    /// `purpose` is `reauthorize` (`docs/design/storage.md`), so only a reauthorize flow reads it;
    /// any other read is storage drift and fails fast instead of attributing the completion to
    /// row `0`.
    ///
    /// # Panics
    ///
    /// Panics when a non-reauthorize flow reads the target — the stored state the `CHECK`s make
    /// unrepresentable.
    #[must_use]
    pub(crate) fn reauthorize_target_id(&self) -> i64 {
        #[expect(
            clippy::expect_used,
            reason = "the schema's cross-column checks pin a reauthorize flow's target non-null"
        )]
        self.target_provider_id
            .expect("a reauthorize flow names its target provider")
    }

    /// The result provider row of a succeeded flow.
    ///
    /// The schema's cross-column `CHECK`s pin `result_provider_id` non-null exactly while
    /// `outcome` is `succeeded` (`docs/design/storage.md`), so only a succeeded outcome reads it;
    /// any other read is storage drift and fails fast instead of resolving row `0`.
    ///
    /// # Panics
    ///
    /// Panics when a pending or failed flow reads the result — the stored state the `CHECK`s
    /// make unrepresentable.
    #[must_use]
    pub(crate) fn result_provider_id(&self) -> i64 {
        #[expect(
            clippy::expect_used,
            reason = "the schema's cross-column checks pin a succeeded outcome's result non-null"
        )]
        self.result_provider_id
            .expect("a succeeded flow carries its result provider")
    }

    /// The gateway-owned failure code and message of a failed flow.
    ///
    /// The schema's cross-column `CHECK`s pin the failure pair non-null exactly while `outcome`
    /// is `failed` (`docs/design/storage.md`), so only a failed outcome reads it; any other read is
    /// storage drift and fails fast instead of surfacing empty failure facts.
    ///
    /// # Panics
    ///
    /// Panics when a pending or succeeded flow reads the pair — the stored state the `CHECK`s
    /// make unrepresentable.
    #[must_use]
    pub(crate) fn failure_pair(&self) -> (String, String) {
        #[expect(
            clippy::expect_used,
            reason = "the schema's cross-column checks pin a failed outcome's failure pair non-null"
        )]
        let code = self
            .failure_code
            .as_deref()
            .expect("a failed flow carries its failure code")
            .to_string();

        #[expect(
            clippy::expect_used,
            reason = "the schema's cross-column checks pin a failed outcome's failure pair non-null"
        )]
        let message = self
            .failure_message
            .as_deref()
            .expect("a failed flow carries its failure message")
            .to_string();

        (code, message)
    }
}

/// The facts a new flow row is inserted from; the device columns are already decoded when a
/// device flow is inserted, so every stored state satisfies the schema's cross-column checks.
pub(crate) struct NewFlow {
    /// The exact definition version the flow's recipe is read from.
    pub(crate) definition_name: String,

    /// The exact definition version the flow's recipe is read from.
    pub(crate) definition_version: NonZeroI64,

    /// The opaque random state token identifying the flow.
    pub(crate) state_token: String,

    /// What the flow is for.
    pub(crate) purpose: FlowPurpose,

    /// The new provider's name, for a create flow.
    pub(crate) provider_name: Option<String>,

    /// Direct-model prefix for a create flow; absent for reauthorization.
    pub(crate) model_prefix: Option<String>,

    /// The working base URL.
    pub(crate) base_url: Option<String>,

    /// The reauthorized provider's row id, for a reauthorize flow.
    pub(crate) target_provider_id: Option<i64>,

    /// The grant kind the flow runs.
    pub(crate) grant_type: FlowGrant,

    /// The initialized working slots.
    pub(crate) slots: BTreeMap<String, String>,

    /// The PKCE verifier of an authorization-code flow.
    pub(crate) code_verifier: Option<String>,

    /// The device code issued by the authorization response.
    pub(crate) device_code: Option<String>,

    /// The human-readable user code.
    pub(crate) device_user_code: Option<String>,

    /// The verification URI, when the response carried one.
    pub(crate) device_verification_uri: Option<String>,

    /// The verification URI carrying the user code, when carried.
    pub(crate) device_verification_uri_complete: Option<String>,

    /// The stored poll interval, in seconds.
    pub(crate) device_poll_interval_seconds: Option<i64>,

    /// The first attempt instant, in UTC microseconds.
    pub(crate) device_next_poll_at: Option<i64>,

    /// The flow's expiry, in UTC microseconds.
    pub(crate) expires_at: i64,
}

/// Parses one flow row in [`FLOW_COLUMNS`] order.
fn flow_from_row(row: &crate::database::Row) -> Result<OngoingFlow, Error> {
    /// Reads one non-null text column.
    fn text(row: &crate::database::Row, index: usize) -> Result<String, Error> {
        Ok(row.get(index)?)
    }

    let purpose = FlowPurpose::parse(&text(row, 4)?).ok_or_else(|| Error::Drift {
        message: "flow purpose column carries an unknown value".to_string(),
    })?;

    let grant_type = FlowGrant::parse(&text(row, 9)?).ok_or_else(|| Error::Drift {
        message: "flow grant_type column carries an unknown value".to_string(),
    })?;

    let slots: String = row.get(10)?;

    Ok(OngoingFlow {
        id: row.get(0)?,
        definition_name: text(row, 1)?,
        definition_version: NonZeroI64::new(row.get(2)?).ok_or_else(|| Error::Drift {
            message: "flow definition version must be positive".to_string(),
        })?,
        state_token: text(row, 3)?,
        purpose,
        provider_name: row.get(5)?,
        model_prefix: row.get(6)?,
        base_url: row.get(7)?,
        target_provider_id: row.get(8)?,
        grant_type,
        slots: serde_json::from_str(&slots).map_err(|error| Error::Drift {
            message: format!("flow slots column carries a malformed payload: {error}"),
        })?,
        code_verifier: row.get(11)?,
        device_code: row.get(12)?,
        device_user_code: row.get(13)?,
        device_verification_uri: row.get(14)?,
        device_verification_uri_complete: row.get(15)?,
        device_poll_interval_seconds: row.get(16)?,
        device_next_poll_at: row.get(17)?,
        device_poll_started_at: row.get(18)?,
        device_exchange_checkpoint: row.get(19)?,
        outcome: match row.get::<Option<String>>(20)? {
            Some(text) => Some(FlowOutcome::parse(&text).ok_or_else(|| Error::Drift {
                message: "flow outcome column carries an unknown value".to_string(),
            })?),
            None => None,
        },
        result_provider_id: row.get(21)?,
        failure_code: row.get(22)?,
        failure_message: row.get(23)?,
        expires_at: row.get(24)?,
    })
}

/// Inserts one flow row and returns the stored row.
///
/// # Errors
///
/// Returns [`Error`] when the insert fails or the row reads back malformed.
pub(crate) async fn insert_flow(database: &Database, flow: NewFlow) -> Result<OngoingFlow, Error> {
    let NewFlow {
        definition_name,
        definition_version,
        state_token,
        purpose,
        provider_name,
        model_prefix,
        base_url,
        target_provider_id,
        grant_type,
        slots,
        code_verifier,
        device_code,
        device_user_code,
        device_verification_uri,
        device_verification_uri_complete,
        device_poll_interval_seconds,
        device_next_poll_at,
        expires_at,
    } = flow;

    let slots_json = serde_json::to_string(&slots).map_err(|error| Error::Drift {
        message: format!("flow slots failed to serialize: {error}"),
    })?;

    // The statement borrows the fields — every text value is passed as `&str` and converted
    // into its stored `Value` before the await — and the returned row is constructed once from
    // the same fields, exactly what the statement wrote.
    let rows = database
        .query(
            "INSERT INTO ongoing_oauth_flows (provider_definition_name, \
             provider_definition_version, state_token, purpose, provider_name, model_prefix, \
             base_url, target_provider_id, grant_type, slots, code_verifier, device_code, \
             device_user_code, device_verification_uri, device_verification_uri_complete, \
             device_poll_interval_seconds, device_next_poll_at, expires_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, \
             ?16, ?17, ?18) RETURNING id",
            params![
                definition_name.as_str(),
                definition_version.get(),
                state_token.as_str(),
                purpose.as_str(),
                provider_name.as_deref(),
                model_prefix.as_deref(),
                base_url.as_deref(),
                target_provider_id,
                grant_type.as_str(),
                slots_json,
                code_verifier.as_deref(),
                device_code.as_deref(),
                device_user_code.as_deref(),
                device_verification_uri.as_deref(),
                device_verification_uri_complete.as_deref(),
                device_poll_interval_seconds,
                device_next_poll_at,
                expires_at,
            ],
        )
        .await?;

    #[expect(
        clippy::indexing_slicing,
        reason = "a single-row INSERT RETURNING yields exactly the inserted row, or errors"
    )]
    let id: i64 = rows[0].get(0)?;

    Ok(OngoingFlow {
        id,
        definition_name,
        definition_version,
        state_token,
        purpose,
        provider_name,
        model_prefix,
        base_url,
        target_provider_id,
        grant_type,
        slots,
        code_verifier,
        device_code,
        device_user_code,
        device_verification_uri,
        device_verification_uri_complete,
        device_poll_interval_seconds,
        device_next_poll_at,
        device_poll_started_at: None,
        device_exchange_checkpoint: None,
        outcome: None,
        result_provider_id: None,
        failure_code: None,
        failure_message: None,
        expires_at,
    })
}

/// Reads the unexpired flow a state token identifies without consuming it, so a completion that
/// cannot succeed can be rejected — and a device flow's pending outcome polled — before the
/// atomic consume spends the row.
///
/// # Errors
///
/// Returns [`Error`] when the query fails or a row is malformed.
pub(crate) async fn find_flow_by_state(
    database: &Database,
    state_token: &str,
    now: i64,
) -> Result<Option<OngoingFlow>, Error> {
    database
        .query(FIND_FLOW_BY_STATE_SQL.as_str(), params![state_token, now])
        .await?
        .first()
        .map(flow_from_row)
        .transpose()
}

/// Consumes the unexpired flow a state token identifies, returning its facts.
///
/// Consumption is a single atomic `DELETE … RETURNING` statement, so exactly one concurrent
/// completion can ever observe a row and a state can never complete twice. An expired or unknown
/// state consumes nothing here; expired rows are swept separately by [`sweep_expired_flows`].
///
/// # Errors
///
/// Returns [`Error`] when the statement fails or a row is malformed.
pub(crate) async fn consume_flow(
    database: &Database,
    state_token: &str,
    now: i64,
) -> Result<Option<OngoingFlow>, Error> {
    database
        .query(CONSUME_FLOW_SQL.as_str(), params![state_token, now])
        .await?
        .first()
        .map(flow_from_row)
        .transpose()
}

/// Consumes one terminal flow by its state token: the single-consume retrieval of a device
/// flow's result. A pending row is never consumed here; `None` means no terminal row was there
/// to consume.
///
/// # Errors
///
/// Returns [`Error`] when the statement fails or a row is malformed.
pub(crate) async fn consume_terminal_flow(
    database: &Database,
    state_token: &str,
    now: i64,
) -> Result<Option<OngoingFlow>, Error> {
    database
        .query(
            CONSUME_TERMINAL_FLOW_SQL.as_str(),
            params![state_token, now],
        )
        .await?
        .first()
        .map(flow_from_row)
        .transpose()
}

/// Deletes every expired flow row, pending and unconsumed-terminal alike: the same sweep policy
/// covers both.
///
/// # Errors
///
/// Returns [`Error`] when the delete fails.
pub(crate) async fn sweep_expired_flows(database: &Database, now: i64) -> Result<(), Error> {
    database
        .execute(
            "DELETE FROM ongoing_oauth_flows WHERE expires_at <= ?1",
            params![now],
        )
        .await?;

    Ok(())
}

/// Claims one due pending device flow for polling, stamping the claim and returning the claimed
/// row: a single conditional `UPDATE … RETURNING`, so competing pollers never double-claim. A
/// claim older than `claim_stale_before` is recovered — stealable by this claim — covering a
/// poller that died mid-exchange once its claim ages past the caller's staleness window (the
/// poller passes a window strictly beyond the control-request timeout that bounds every
/// exchange, `docs/design/storage.md`) or between its completed exchange and its commit. The
/// claimed row may carry an exchange checkpoint (see [`persist_exchange_checkpoint`]), which the
/// claimant resumes from instead of exchanging again.
///
/// # Errors
///
/// Returns [`Error`] when the statement fails or a row is malformed.
pub(crate) async fn claim_due_device_flow(
    database: &Database,
    now: i64,
    claim_stale_before: i64,
) -> Result<Option<OngoingFlow>, Error> {
    database
        .query(
            CLAIM_DUE_DEVICE_FLOW_SQL.as_str(),
            params![now, FlowGrant::Device.as_str(), claim_stale_before],
        )
        .await?
        .first()
        .map(flow_from_row)
        .transpose()
}

/// Re-stamps the caller's device-poll claim with a fresh instant — one conditional
/// compare-and-set over the held stamp. Provisioning runs inside the claim, so a sequence
/// longer than the claim-staleness window would hand the claim to another poller tick before
/// the commit; the re-stamp after provisioning returns refreshes ownership for the commit, so
/// a provisioning sequence longer than the staleness window cannot be stolen mid-commit
/// (`docs/design/storage.md` and `docs/design/providers.md`). `None` — no row matched — means the
/// claim was stolen: the winner owns the row and the caller must abandon instead of committing.
///
/// The statement stamps the claim exactly like [`claim_due_device_flow`] and conditions on it
/// exactly like [`commit_device_success_in_transaction`], whose commit the freshened stamp
/// feeds.
///
/// # Errors
///
/// Returns [`Error`] when the update fails.
pub(crate) async fn restamp_claim(
    database: &Database,
    flow_id: i64,
    held_claim: i64,
) -> Result<Option<i64>, Error> {
    let new_claim = unix_time::unix_now_micros();

    let changes = database
        .execute(
            "UPDATE ongoing_oauth_flows SET device_poll_started_at = ?1 \
             WHERE id = ?2 AND device_poll_started_at = ?3 AND outcome IS NULL",
            params![new_claim, flow_id, held_claim],
        )
        .await?;

    Ok((changes == 1).then_some(new_claim))
}

/// Persists the checkpoint of one completed exchange while the caller still owns the claim
/// stamped `claim_started_at`: the mapped token fields ride the flow row, so a restart that
/// recovers the claim resumes into provisioning and commit instead of re-presenting the
/// consumed device code to the upstream (`docs/design/storage.md` and
/// `docs/reference/management-api.md`). `false` means the claim was lost — the winner owns the row,
/// and the caller discards its outcome like every persist.
///
/// # Errors
///
/// Returns [`Error`] when the update fails.
pub(crate) async fn persist_exchange_checkpoint(
    database: &Database,
    flow_id: i64,
    claim_started_at: i64,
    checkpoint_json: &str,
) -> Result<bool, Error> {
    let changes = database
        .execute(
            "UPDATE ongoing_oauth_flows SET device_exchange_checkpoint = ?1 \
             WHERE id = ?2 AND device_poll_started_at = ?3 AND outcome IS NULL",
            params![checkpoint_json, flow_id, claim_started_at],
        )
        .await?;

    Ok(changes == 1)
}

/// Persists one poll attempt's outcome while the caller still owns the claim stamped
/// `claim_started_at`: either the next schedule (an `authorization_pending` or backoff retry, the
/// interval optionally raised permanently), or the terminal failure with its gateway-owned code
/// and message. A terminal persist also drops the next-poll instant, which a device flow carries
/// exactly while it is pending (`docs/design/storage.md`). Both arms clear any exchange checkpoint:
/// a retry is only ever scheduled for an exchange that did not complete, and a terminal outcome
/// ends the flow, so the recorded exchange is never resumed — the row a released claim or a
/// terminal outcome leaves behind carries no checkpoint, exactly as the schema pins it. `None`
/// means the claim was lost — a concurrent commit superseded this poller — and the computed
/// schedule was discarded.
///
/// # Errors
///
/// Returns [`Error`] when the update fails.
pub(crate) async fn persist_device_poll(
    database: &Database,
    flow_id: i64,
    claim_started_at: i64,
    poll: DevicePollOutcome,
) -> Result<bool, Error> {
    let changes = match poll {
        DevicePollOutcome::Retry {
            next_poll_at,
            interval_seconds,
        } => {
            database
                .execute(
                    "UPDATE ongoing_oauth_flows SET device_next_poll_at = ?1, \
                     device_poll_interval_seconds = ?2, device_poll_started_at = NULL, \
                     device_exchange_checkpoint = NULL \
                     WHERE id = ?3 AND device_poll_started_at = ?4 AND outcome IS NULL",
                    params![next_poll_at, interval_seconds, flow_id, claim_started_at],
                )
                .await?
        }

        DevicePollOutcome::Failed { code, message } => {
            database
                .execute(
                    "UPDATE ongoing_oauth_flows SET outcome = ?1, failure_code = ?2, \
                     failure_message = ?3, device_next_poll_at = NULL, \
                     device_poll_started_at = NULL, device_exchange_checkpoint = NULL \
                     WHERE id = ?4 AND device_poll_started_at = ?5 AND outcome IS NULL",
                    params![
                        FlowOutcome::Failed.as_str(),
                        code,
                        message,
                        flow_id,
                        claim_started_at
                    ],
                )
                .await?
        }
    };

    Ok(changes == 1)
}

/// One device poll attempt's persisted outcome.
pub(crate) enum DevicePollOutcome {
    /// The upstream reported the flow still pending (or the transport backed off): the next
    /// attempt is scheduled at `next_poll_at`, and the stored interval is raised permanently
    /// only by `slow_down` — a transport-timeout backoff rides `next_poll_at` alone.
    Retry {
        /// The next attempt instant, in UTC microseconds; it carries any transient backoff.
        next_poll_at: i64,

        /// The flow interval persisted as the stored schedule, in seconds.
        interval_seconds: i64,
    },

    /// The exchange failed terminally; the flow records the gateway-owned failure pair.
    Failed {
        /// The gateway-owned failure code.
        code: String,

        /// The gateway-owned failure message.
        message: String,
    },
}

/// Commits a successful device flow inside the caller's transaction: the outcome update is part
/// of the same transaction as the provider write, so the provider and the consumable result
/// appear together. The commit also drops the next-poll instant, which a device flow carries
/// exactly while it is pending (`docs/design/storage.md`), and the exchange checkpoint, whose
/// recorded exchange the commit itself completes — a terminal row carries neither.
///
/// The update conditions on the claim stamp, so only the poller that won the exchange can commit
/// the success; `false` means the claim was lost and the caller's provider write must be
/// abandoned.
///
/// # Errors
///
/// Returns [`Error`] when the update fails.
pub(crate) async fn commit_device_success_in_transaction(
    transaction: &mut Transaction,
    flow_id: i64,
    claim_started_at: i64,
    result_provider_id: i64,
) -> Result<bool, Error> {
    let changes = transaction
        .execute(
            "UPDATE ongoing_oauth_flows SET outcome = ?1, result_provider_id = ?2, \
             device_next_poll_at = NULL, device_poll_started_at = NULL, \
             device_exchange_checkpoint = NULL \
             WHERE id = ?3 AND device_poll_started_at = ?4 AND outcome IS NULL",
            params![
                FlowOutcome::Succeeded.as_str(),
                result_provider_id,
                flow_id,
                claim_started_at
            ],
        )
        .await?;

    Ok(changes == 1)
}
