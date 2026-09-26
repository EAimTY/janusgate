//! OAuth authorization flows: flow begin and completion, the token exchange with terminal-code
//! classification, the lease-guarded renewal, the background device poller, and the shared
//! exchange-to-provider-write completion pipeline.
//!
//! The submodule layout mirrors the concerns: [`flow`] is the flow begin orchestration over
//! both grants (state minting, authorize-URL building, device authorization, expiry resolution,
//! and the persisted flow row) plus the exchange mechanics (token exchange with terminal-code
//! classification, expiry resolution), [`completion`] is the
//! pipeline both completion surfaces share (the exchange folded through provisioning into the
//! stored credentials, and the provider write), [`provisioning`] is the ordered
//! control-request sequence a completed exchange — and a reprovisioning renewal — folds into
//! the working state, [`refresh`] is the lease-guarded renewal loop over the definition's
//! refresh mode, and [`poller`] is the background scheduler resuming every unexpired pending
//! device flow from its stored schedule.
//!
//! Every provider-specific fact lives in the definition the flow references; the engines carry
//! the grant mechanics alone and never special-case a provider.

mod completion;
mod flow;
mod poller;
mod provisioning;
mod refresh;

pub(crate) use completion::{complete_exchange, completed_outcome, write_completion_provider};
pub(crate) use flow::{
    BeganFlow, BeginFacts, BeginPurpose, begin_flow, execute_exchange, initialize_slots,
};
#[cfg(test)]
pub(crate) use flow::{DeviceAuthorization, ExchangeOutcome, execute_device_authorization};
pub(crate) use poller::spawn_device_poller;
#[cfg(test)]
pub(crate) use provisioning::{ProvisioningEnv, WorkingState, run_steps};
pub(crate) use refresh::ensure_fresh_token;
