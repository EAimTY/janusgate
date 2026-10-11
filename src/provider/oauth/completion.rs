//! The completion pipeline shared by the synchronous authorization-code completion and the
//! background device poller (`docs/design/providers.md`): one routine folds a completed exchange —
//! the identity-token claims, the response's slots, its extracted base URL, and the provisioning
//! sequence — into the working state and builds the stored OAuth credentials, and one routine
//! writes the flow's provider row inside the caller's transaction. The callers differ only in
//! transaction discipline: the poller commits the provider write together with the flow's
//! succeeded outcome inside its claim transaction, while the synchronous completion commits the
//! provider write alone, the flow row already consumed.

use super::{
    exchange::{ExchangeOutcome, ExchangeProduct, absorb_exchange_slots, resolve_expiry},
    provisioning::{self, WorkingState},
};
use crate::{
    database::Transaction,
    provider::{Error, StoredCredentials, definition::ProviderDefinition},
    storage,
    storage::oauth_flows::{FlowPurpose, OngoingFlow},
};
use reqwest::Client;

#[cfg(test)]
mod tests;

/// The product of one completed flow's exchange-and-provisioning pipeline: the stored OAuth
/// credentials and the final working base URL — with its provenance — that the provider write
/// commits.
pub(crate) struct CompletionProduct {
    /// The stored credentials the completed flow produced.
    credentials: StoredCredentials,

    /// The working base URL after the exchange and any provisioning extraction.
    base_url: Option<String>,

    /// Whether [`CompletionProduct::base_url`] is an upstream-derived replacement — the
    /// exchange's `base_url` fold or a provisioning extraction — rather than the flow's
    /// begin-time base. Only a derived base joins a reauthorize flow's write
    /// (`docs/reference/management-api.md`); a create flow stores the working base either way,
    /// the begin-time base being the operator-supplied initial value there
    /// (`docs/reference/management-api.md`'s `oauth:start` contract).
    is_base_url_derived: bool,
}

/// Maps one executed exchange's classification onto an authorization-code completion: the
/// device-polling classifications are engine drift under this grant, and the configured
/// rejections render the authorization-code-specific failures.
///
/// # Errors
///
/// Returns [`Error`] for every classification but [`ExchangeProduct::Completed`].
pub(crate) fn completed_outcome(product: ExchangeProduct) -> Result<ExchangeOutcome, Error> {
    match product {
        ExchangeProduct::Completed(outcome) => Ok(outcome),

        ExchangeProduct::Pending | ExchangeProduct::SlowDown => Err(Error::Internal {
            message: "the exchange classified as a poll outcome under an authorization-code \
                          flow"
                .to_string(),
        }),

        ExchangeProduct::Terminal(code) => Err(Error::Unavailable {
            detail: format!(
                "the token endpoint rejected the authorization code with `{code}`: the code is \
                 expired, already used, or invalid — start a fresh flow, authorize its URL, and \
                 complete it promptly"
            ),
        }),

        ExchangeProduct::Other(code) => Err(Error::Unavailable {
            detail: format!("the token endpoint rejected the authorization code: `{code}`"),
        }),
    }
}

/// Runs the completion pipeline over one flow's completed exchange (`docs/design/providers.md`):
/// the flow's working slots absorb the identity-token claims and the exchange's slots, the
/// exchange's extracted base URL replaces the flow's stored one, the provisioning sequence
/// folds its extractions over the working state, and the stored credentials are built from the
/// result. Per `docs/design/providers.md` the access token may be supplied by a later provisioning
/// step instead of the exchange, but must exist before the provider write, and the expiry resolves
/// from a provisioning extraction, else the response's token data.
///
/// # Errors
///
/// Returns [`Error`] when a provisioning step fails, the expiry data is malformed, or no source
/// supplies the access token — the write-time validation guarantees the exchange or a
/// provisioning step does, so that case is stored drift.
pub(crate) async fn complete_exchange(
    http: &Client,
    definition: &ProviderDefinition,
    outcome: ExchangeOutcome,
    flow: &OngoingFlow,
) -> Result<CompletionProduct, Error> {
    let recipe = definition.oauth_recipe().ok_or_else(|| Error::Internal {
        message: "the flow's definition carries no OAuth credential".to_string(),
    })?;

    // The working slots absorb the identity-token claims and the exchange's slots; a claimed
    // value outside the routing-safety contract fails the flow like any other fold.
    let working_slots = absorb_exchange_slots(&flow.slots, &outcome, &recipe.id_token_claims)?;

    // The working base URL follows the slots' precedence (`docs/design/providers.md`): the
    // exchange's extracted `base_url` replaces the flow's stored one, and a provisioning
    // extraction replaces it again when one runs below.
    let mut state = WorkingState::new(
        working_slots,
        outcome.base_url.clone().or_else(|| flow.base_url.clone()),
    );

    // Provenance (`docs/reference/management-api.md`): only an upstream-derived base URL — the
    // exchange's fold here, or a provisioning extraction below — replaces the stored base at
    // the provider write; the begin-time base the flow echoed is never a reauthorize write
    // candidate.
    state.is_base_url_derived = outcome.base_url.is_some();

    // The stage constants thread the exchange's access token — the secret `Secret` references
    // render — and the client credentials down to the per-request environment rebuilds.
    let secret = outcome.access_token.clone();

    let constants = provisioning::ProvisioningEnv {
        secret: secret.as_deref(),
        client_id: recipe.client_id.as_str(),
        client_secret: recipe.client_secret.as_deref(),
    };

    state.access_token = outcome.access_token;
    state.refresh_token = outcome.refresh_token;

    if let Some(steps) = &definition.payload.provisioning {
        provisioning::run_steps(http, steps, &constants, &mut state).await?;
    }

    // The access token must exist before the provider write; the definition's validation
    // guarantees the exchange or a provisioning step supplies it.
    let Some(access_token) = state.access_token else {
        return Err(Error::Internal {
            message: "the completed exchange produced no access token".to_string(),
        });
    };

    let expires_at = match state.expires_at {
        Some(expires_at) => Some(expires_at),

        None => resolve_expiry(
            outcome.expires_in_seconds,
            Some(&access_token),
            recipe.access_token_lifetime_seconds,
        )?,
    };

    // An empty slot evaluates exactly like an absent one, so the stored credentials keep only
    // the slots carrying values. The state is consumed field by field from here on: nothing
    // reads it whole after the access token check.
    let mut slots = std::mem::take(&mut state.slots);

    slots.retain(|_, value| !value.is_empty());

    Ok(CompletionProduct {
        credentials: StoredCredentials::OAuth {
            access_token,
            refresh_token: state.refresh_token,
            slots,
            expires_at,
            refresh_started_at: None,
        },
        base_url: state.base_url,
        is_base_url_derived: state.is_base_url_derived,
    })
}

/// Writes the provider row one completed flow produces, inside the caller's transaction: a
/// create flow inserts the new row carrying the working base URL, and a reauthorize flow
/// atomically replaces the target's definition reference and credentials plus — only when the
/// flow derived one — the working base URL. The caller owns the transaction so the device
/// poller commits the write together with the flow's succeeded outcome in its claim
/// transaction.
///
/// Returns the written row's id, or [`None`] when a reauthorize flow's target row vanished
/// before its update — the caller rolls the transaction back and classifies the loss its own
/// surface's way.
///
/// # Errors
///
/// Returns [`Error`] when the write fails; a concurrent creation of the same provider name
/// surfaces the storage layer's unique-constraint error.
pub(crate) async fn write_completion_provider(
    transaction: &mut Transaction,
    flow: &OngoingFlow,
    definition: &ProviderDefinition,
    product: &CompletionProduct,
) -> Result<Option<i64>, Error> {
    match flow.purpose {
        FlowPurpose::Create => {
            // A concurrent creation of the same name is the unique constraint's call; the flow
            // fails with the conflict instead of racing.
            let id = storage::providers::insert_provider_in_transaction(
                transaction,
                storage::providers::NewProvider {
                    name: flow.create_provider_name(),
                    model_prefix: flow.create_model_prefix(),
                    base_url: product.base_url.as_deref(),
                    credentials: &product.credentials,
                    definition,
                    is_enabled: true,
                },
            )
            .await?;

            Ok(Some(id))
        }

        FlowPurpose::Reauthorize => {
            let target = flow.reauthorize_target_id();

            let is_updated = storage::providers::rebind_credentials_in_transaction(
                transaction,
                target,
                definition,
                &product.credentials,
            )
            .await?;

            if !is_updated {
                // A concurrent `providers:delete` won; the caller rolls back and classifies.
                return Ok(None);
            }

            // The stored base joins the write only when the flow derived a replacement
            // (`docs/design/storage.md`): the begin-time base the product otherwise carries is a
            // stale echo by completion time, and rewriting it would clobber a base a
            // concurrent `providers:update` stored while the user approved. A derived
            // base lands in the same transaction as the rebind, so a rebind to a
            // base-referencing definition never sits on a missing base
            // (`docs/reference/management-api.md`).
            if product.is_base_url_derived {
                let Some(base_url) = &product.base_url else {
                    return Err(Error::Internal {
                        message: "the completed flow derived a base URL without carrying one"
                            .to_string(),
                    });
                };

                storage::providers::set_base_url_in_transaction(transaction, target, base_url)
                    .await?;
            }

            Ok(Some(target))
        }
    }
}
