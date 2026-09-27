//! OAuth authorization-flow persistence: the unconsumed-flow lifecycle of
//! `ongoing_oauth_flows` (README §3, §5.6).
//!
//! *Ongoing* means unconsumed: a flow row is deleted by its completion (an authorization-code
//! exchange, or the management client retrieving a terminal device result) or by the expiry
//! sweep — never by any other path. A terminal device result stays until the management client
//! retrieves it or the flow expires. Device polling claims a due flow with a conditional lease
//! (`device_poll_started_at`), so competing pollers never double-exchange an upstream grant
//! concurrently; the lease alone cannot cover a poller that dies after its exchange completed,
//! so the claimant persists the exchange checkpoint the moment the exchange completes, and a
//! restart that recovers the claim resumes from it into provisioning and commit instead of
//! re-presenting the consumed device code (README §3, §7.2).

use super::Error;
use crate::{
    database::{Database, Transaction, params},
    unix_time,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, num::NonZeroI64};

/// The column order of flow selects, consumed positionally by [`flow_from_row`].
const FLOW_COLUMNS: &str = "id, provider_definition_name, provider_definition_version, \
     state_token, purpose, provider_name, model_prefix, base_url, target_provider_id, \
     grant_type, slots, code_verifier, device_code, device_user_code, \
     device_verification_uri, device_verification_uri_complete, device_poll_interval_seconds, \
     device_next_poll_at, device_poll_started_at, device_exchange_checkpoint, outcome, \
     result_provider_id, failure_code, failure_message, expires_at";

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
    /// The storage form of the purpose.
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Reauthorize => "reauthorize",
        }
    }

    /// Parses the storage form of the purpose; anything else is storage drift.
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "create" => Some(Self::Create),
            "reauthorize" => Some(Self::Reauthorize),
            _ => None,
        }
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
    /// The storage form of the grant kind.
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::AuthorizationCode => "authorization_code",
            Self::Device => "device",
        }
    }

    /// Parses the storage form of the grant kind; anything else is storage drift.
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "authorization_code" => Some(Self::AuthorizationCode),
            "device" => Some(Self::Device),
            _ => None,
        }
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
    /// The storage form of the outcome.
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
        }
    }

    /// Parses the storage form of the outcome; anything else is storage drift.
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "succeeded" => Some(Self::Succeeded),
            "failed" => Some(Self::Failed),
            _ => None,
        }
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
    pub(crate) provider_name: Option<String>,

    /// The new provider's model prefix, for a create flow.
    pub(crate) model_prefix: Option<String>,

    /// The working base URL: the creation's initial instance URL, or the target provider's
    /// stored one, replaceable by an exchange or provisioning extraction before commit.
    pub(crate) base_url: Option<String>,

    /// The reauthorized provider's row identifier, for a reauthorize flow.
    pub(crate) target_provider_id: Option<i64>,

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
    pub(crate) result_provider_id: Option<i64>,

    /// The gateway-owned failure code of a failed flow; never an upstream body.
    pub(crate) failure_code: Option<String>,

    /// The gateway-owned failure message of a failed flow; never an upstream body.
    pub(crate) failure_message: Option<String>,

    /// Unix timestamp in microseconds after which the flow is invalid; the authorization-code
    /// TTL is fixed at ten minutes, a device flow's comes from its authorization response or the
    /// definition's explicit fallback.
    pub(crate) expires_at: i64,
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

    /// The new provider's model prefix, for a create flow.
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

    let rows = database
        .query(
            "INSERT INTO ongoing_oauth_flows (provider_definition_name, \
             provider_definition_version, state_token, purpose, provider_name, model_prefix, \
             base_url, target_provider_id, grant_type, slots, code_verifier, device_code, \
             device_user_code, device_verification_uri, device_verification_uri_complete, \
             device_poll_interval_seconds, device_next_poll_at, expires_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, \
             ?18) RETURNING id",
            params![
                definition_name.clone(),
                definition_version.get(),
                state_token.clone(),
                purpose.as_str(),
                provider_name.clone(),
                model_prefix.clone(),
                base_url.clone(),
                target_provider_id,
                grant_type.as_str(),
                slots_json,
                code_verifier.clone(),
                device_code.clone(),
                device_user_code.clone(),
                device_verification_uri.clone(),
                device_verification_uri_complete.clone(),
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
    let sql = format!(
        "SELECT {FLOW_COLUMNS} FROM ongoing_oauth_flows WHERE state_token = ?1 AND expires_at > ?2"
    );

    database
        .query(sql, params![state_token, now])
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
    let sql = format!(
        "DELETE FROM ongoing_oauth_flows WHERE state_token = ?1 AND expires_at > ?2 RETURNING \
         {FLOW_COLUMNS}"
    );

    database
        .query(sql, params![state_token, now])
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
    let sql = format!(
        "DELETE FROM ongoing_oauth_flows WHERE state_token = ?1 AND expires_at > ?2 \
         AND outcome IS NOT NULL RETURNING {FLOW_COLUMNS}"
    );

    database
        .query(sql, params![state_token, now])
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
/// exchange, README §3) or between its completed exchange and its commit. The claimed row may
/// carry an exchange checkpoint (see [`persist_exchange_checkpoint`]), which the claimant
/// resumes from instead of exchanging again.
///
/// # Errors
///
/// Returns [`Error`] when the statement fails or a row is malformed.
pub(crate) async fn claim_due_device_flow(
    database: &Database,
    now: i64,
    claim_stale_before: i64,
) -> Result<Option<OngoingFlow>, Error> {
    let sql = format!(
        "UPDATE ongoing_oauth_flows SET device_poll_started_at = ?1 WHERE id = (\
         SELECT id FROM ongoing_oauth_flows WHERE outcome IS NULL AND grant_type = ?2 \
         AND expires_at > ?1 AND device_next_poll_at <= ?1 \
         AND (device_poll_started_at IS NULL OR device_poll_started_at < ?3) \
         ORDER BY device_next_poll_at LIMIT 1) RETURNING {FLOW_COLUMNS}"
    );

    database
        .query(
            sql,
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
/// (README §3, §5.4). `None` — no row matched — means the claim was stolen: the winner owns
/// the row and the caller must abandon instead of committing.
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
/// consumed device code to the upstream (README §3, §7.2). `false` means the claim was lost —
/// the winner owns the row, and the caller discards its outcome like every persist.
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
/// exactly while it is pending (README §3). Both arms clear any exchange checkpoint: a retry is
/// only ever scheduled for an exchange that did not complete, and a terminal outcome ends the
/// flow, so the recorded exchange is never resumed — the row a released claim or a terminal
/// outcome leaves behind carries no checkpoint, exactly as the schema pins it. `None` means the
/// claim was lost — a concurrent commit superseded this poller — and the computed schedule was
/// discarded.
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
/// exactly while it is pending (README §3), and the exchange checkpoint, whose recorded
/// exchange the commit itself completes — a terminal row carries neither.
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

#[cfg(test)]
mod tests {
    #![expect(
        clippy::expect_used,
        reason = "the fixtures are constructed so each expect lands on its documented side"
    )]

    use super::{
        DevicePollOutcome, FlowGrant, FlowOutcome, FlowPurpose, NewFlow, claim_due_device_flow,
        commit_device_success_in_transaction, consume_flow, consume_terminal_flow,
        find_flow_by_state, insert_flow, persist_device_poll, sweep_expired_flows,
    };
    use crate::{
        storage::{
            self as storage_cr,
            test_support::{insert_test_provider, oauth_definition},
        },
        test_support,
    };
    use std::{collections::BTreeMap, error::Error as StdError};

    /// The storage-layer flow fixture: the crate-wide device-flow fixture
    /// [`test_support::insert_flow_fixture`] narrowed to the fixed small schedule and the
    /// `"st-device-1"` token these tests assert against. The delta pins the whole schedule, so
    /// the fixture instant the callers pass never reaches the row.
    fn new_device_flow_delta() -> test_support::FlowFixtureDelta {
        test_support::FlowFixtureDelta {
            state_token: Some("st-device-1".to_string()),
            purpose: Some(FlowPurpose::Reauthorize),
            provider_name: Some(None),
            model_prefix: Some(None),
            target_provider_id: Some(1),
            device_code: Some("dc".to_string()),
            device_user_code: Some("uc".to_string()),
            device_next_poll_at: Some(500),
            expires_at: Some(1_000_000),
            ..test_support::FlowFixtureDelta::default()
        }
    }

    /// Verifies the OAuth flow lifecycle against the real database: a device flow's stored
    /// schedule drives the conditional claim, the claim owner alone persists its outcome, a
    /// terminal success commits atomically with the provider write, the result is consumed once,
    /// and the expiry sweep clears unconsumed rows.
    ///
    /// # Panics
    ///
    /// Panics if database operations or assertions fail.
    #[tokio::test]
    async fn device_flow_lifecycle_claims_and_consumes_once() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) = test_support::create_test_database().await;

        let definition = oauth_definition();

        storage_cr::provider_definitions::insert_definition(
            &database,
            storage_cr::provider_definitions::NewDefinition {
                name: definition.name.as_str(),
                base_version: None,
                payload: &definition.payload,
                created_at: definition.created_at,
            },
        )
        .await?;

        let flow =
            test_support::insert_flow_fixture(&database, &definition, 0, new_device_flow_delta())
                .await;

        // Nothing is due before its next-poll instant.
        let now = 400;

        assert!(claim_due_device_flow(&database, now, 0).await?.is_none());

        // The due flow is claimed by exactly one conditional update.
        let claimed = claim_due_device_flow(&database, 500, 0)
            .await?
            .expect("the due flow claims");

        assert_eq!(claimed.id, flow.id);
        assert_eq!(claimed.device_poll_started_at, Some(500));

        // A second claimant cannot steal a live claim.
        assert!(claim_due_device_flow(&database, 500, 0).await?.is_none());

        // A lost claim is the owner's alone to persist; the schedule retry lands.
        let is_persisted = persist_device_poll(
            &database,
            flow.id,
            999,
            DevicePollOutcome::Retry {
                next_poll_at: 10_000,
                interval_seconds: 5,
            },
        )
        .await?;

        assert!(!is_persisted);

        let is_persisted = persist_device_poll(
            &database,
            flow.id,
            500,
            DevicePollOutcome::Retry {
                next_poll_at: 10_000,
                interval_seconds: 5,
            },
        )
        .await?;

        assert!(is_persisted);

        let rescheduled = find_flow_by_state(&database, "st-device-1", 0)
            .await?
            .expect("the flow persists");

        assert_eq!(rescheduled.device_next_poll_at, Some(10_000));
        assert_eq!(rescheduled.device_poll_started_at, None);

        // The success commits the provider write and the terminal outcome in one transaction.
        let target = insert_test_provider(&database, "oauth-target", "ot-").await?;

        let mut transaction = database.transaction().await?;

        let is_committed =
            commit_device_success_in_transaction(&mut transaction, flow.id, 10_000 - 5, target)
                .await?;

        assert!(!is_committed, "a stale claim cannot commit");

        // The discarded attempt releases its write lock before the next claim.
        transaction.rollback().await?;

        let mut transaction = database.transaction().await?;

        let claim = claim_due_device_flow(&database, 10_000, 0)
            .await?
            .expect("the rescheduled flow claims again");

        assert_eq!(claim.device_poll_started_at, Some(10_000));

        let is_committed =
            commit_device_success_in_transaction(&mut transaction, flow.id, 10_000, target).await?;

        assert!(is_committed);

        transaction.commit().await?;

        let terminal = find_flow_by_state(&database, "st-device-1", 0)
            .await?
            .expect("the terminal row stays until consumed");

        assert_eq!(terminal.outcome, Some(FlowOutcome::Succeeded));
        assert_eq!(terminal.result_provider_id, Some(target));
        assert_eq!(
            terminal.device_next_poll_at, None,
            "the terminal row carries no next-poll instant"
        );
        assert_eq!(terminal.device_poll_started_at, None);

        // The result is consumed exactly once.
        let consumed = consume_terminal_flow(&database, "st-device-1", 0).await?;

        assert!(consumed.is_some());
        assert!(
            consume_terminal_flow(&database, "st-device-1", 0)
                .await?
                .is_none()
        );

        // A pending authorization-code flow is never consumed by the terminal-only consume.
        let auth_flow = insert_flow(
            &database,
            NewFlow {
                definition_name: definition.name.clone(),
                definition_version: definition.version,
                state_token: "st-auth-1".to_string(),
                purpose: FlowPurpose::Create,
                provider_name: Some("new-provider".to_string()),
                model_prefix: Some("np-".to_string()),
                base_url: None,
                target_provider_id: None,
                grant_type: FlowGrant::AuthorizationCode,
                slots: BTreeMap::new(),
                code_verifier: Some("verifier".to_string()),
                device_code: None,
                device_user_code: None,
                device_verification_uri: None,
                device_verification_uri_complete: None,
                device_poll_interval_seconds: None,
                device_next_poll_at: None,
                expires_at: 1_000_000,
            },
        )
        .await?;

        assert!(
            consume_terminal_flow(&database, "st-auth-1", 0)
                .await?
                .is_none()
        );

        // The atomic consume spends the row exactly once, whatever its state.
        let consumed = consume_flow(&database, "st-auth-1", 0).await?;

        assert_eq!(consumed.map(|flow| flow.id), Some(auth_flow.id));
        assert!(consume_flow(&database, "st-auth-1", 0).await?.is_none());

        // The expiry sweep clears unconsumed rows.
        test_support::insert_flow_fixture(&database, &definition, 0, new_device_flow_delta()).await;

        sweep_expired_flows(&database, 2_000_000).await?;

        assert!(
            find_flow_by_state(&database, "st-device-1", 0)
                .await?
                .is_none()
        );

        Ok(())
    }

    /// Verifies the terminal failure persist: the flow records its gateway-owned pair and drops
    /// the next-poll instant — a device flow carries its schedule exactly while it is pending
    /// (README §3) — while keeping the device columns and interval that identify its grant, and
    /// the failure is consumable exactly once as the flow's terminal result.
    ///
    /// # Panics
    ///
    /// Panics if database operations or assertions fail.
    #[tokio::test]
    async fn device_flow_terminal_failure_clears_the_next_poll_instant()
    -> Result<(), Box<dyn StdError>> {
        let (database, _guard) = test_support::create_test_database().await;

        let definition = oauth_definition();

        storage_cr::provider_definitions::insert_definition(
            &database,
            storage_cr::provider_definitions::NewDefinition {
                name: definition.name.as_str(),
                base_version: None,
                payload: &definition.payload,
                created_at: definition.created_at,
            },
        )
        .await?;

        test_support::insert_flow_fixture(&database, &definition, 0, new_device_flow_delta()).await;

        let claim = claim_due_device_flow(&database, 500, 0)
            .await?
            .expect("the due flow claims");

        let is_persisted = persist_device_poll(
            &database,
            claim.id,
            500,
            DevicePollOutcome::Failed {
                code: "expired_token".to_string(),
                message: "the device code expired".to_string(),
            },
        )
        .await?;

        assert!(is_persisted);

        let terminal = find_flow_by_state(&database, "st-device-1", 0)
            .await?
            .expect("the terminal row stays until consumed");

        assert_eq!(terminal.outcome, Some(FlowOutcome::Failed));
        assert_eq!(terminal.failure_code.as_deref(), Some("expired_token"));
        assert_eq!(
            terminal.failure_message.as_deref(),
            Some("the device code expired")
        );
        assert_eq!(
            terminal.device_next_poll_at, None,
            "the terminal row carries no next-poll instant"
        );
        assert_eq!(terminal.device_poll_started_at, None);
        assert_eq!(terminal.device_poll_interval_seconds, Some(5));
        assert_eq!(terminal.device_code.as_deref(), Some("dc"));

        // The failure is consumed exactly once, like every terminal result.
        assert!(
            consume_terminal_flow(&database, "st-device-1", 0)
                .await?
                .is_some()
        );
        assert!(
            consume_terminal_flow(&database, "st-device-1", 0)
                .await?
                .is_none()
        );

        Ok(())
    }
}
