//! The per-request compaction accounting ledger (`docs/design/compaction.md`): the
//! numerical record of every actual data-plane dispatch an outer compaction
//! request performs, one iteration at a time.
//!
//! The ledger is measurement, not billing policy: each dispatched call — a
//! native compact attempt or a gateway summary inference — appends one
//! iteration carrying the usage the upstream reported. Continuation usage belongs to its answer
//! aggregation; the response task combines it with this ledger's billed totals. A native token
//! counter records no iteration: its measurement is the count it returns, never billed usage, and
//! numerical threshold estimates are never recorded — an estimate describes
//! context size, not billed work.

use crate::ir::IrUsage;
use serde::Deserialize as _;

/// The per-request ledger of dispatched compaction-path calls.
#[derive(Debug, Default)]
pub(crate) struct CompactionLedger {
    /// The recorded iterations' usage observations, in dispatch order.
    iterations: Vec<IrUsage>,
}

impl CompactionLedger {
    /// Records one dispatched call's usage.
    ///
    /// A call the upstream answered without a usage report acquires no
    /// fabricated token counts: record nothing for it.
    pub(crate) fn record(&mut self, usage: Option<IrUsage>) {
        if let Some(usage) = usage {
            self.iterations.push(usage);
        }
    }

    /// The billed totals: input and output sums over every recorded iteration,
    /// each breakdown summed when any iteration reported it. A breakdown no
    /// iteration reported stays unreported. Sums saturate rather than overflow.
    #[must_use]
    pub(crate) fn billed_totals(&self) -> IrUsage {
        let mut totals = IrUsage::default();

        for usage in &self.iterations {
            totals.input_tokens = totals.input_tokens.saturating_add(usage.input_tokens);
            totals.output_tokens = totals.output_tokens.saturating_add(usage.output_tokens);

            if let Some(cached) = usage.cached_tokens {
                totals.cached_tokens =
                    Some(totals.cached_tokens.unwrap_or(0).saturating_add(cached));
            }

            if let Some(cache_write) = usage.cache_write_tokens {
                totals.cache_write_tokens = Some(
                    totals
                        .cache_write_tokens
                        .unwrap_or(0)
                        .saturating_add(cache_write),
                );
            }

            if let Some(reasoning) = usage.reasoning_tokens {
                totals.reasoning_tokens = Some(
                    totals
                        .reasoning_tokens
                        .unwrap_or(0)
                        .saturating_add(reasoning),
                );
            }
        }

        totals
    }

    /// Returns accounting facts only when a dispatched stage actually reported usage.
    /// An explicit zero report remains present; protocol-required zero totals for an empty
    /// ledger are not upstream measurements and must not become recorded token facts.
    pub(crate) fn reported_billed_totals(&self) -> Option<IrUsage> {
        (!self.iterations.is_empty()).then(|| self.billed_totals())
    }

    /// The recorded iterations' usage observations, in dispatch order.
    #[must_use]
    pub(crate) fn iterations(&self) -> &[IrUsage] {
        &self.iterations
    }
}

/// Sums a vendor's authoritative sampling iteration reports without inventing absent usage.
pub(crate) fn reported_iteration_totals(
    iterations: &[crate::ir::compaction::UsageIteration],
) -> Option<IrUsage> {
    /// Known usage counters read from an otherwise producer-owned iteration report.
    #[derive(serde::Deserialize)]
    struct Report {
        /// Input tokens consumed in this sampling iteration.
        input_tokens: u64,
        /// Output tokens generated in this sampling iteration.
        output_tokens: u64,
        /// Cached input usage when reported.
        #[serde(default)]
        cache_read_input_tokens: Option<u64>,
        /// Cache creation usage when reported.
        #[serde(default)]
        cache_creation_input_tokens: Option<u64>,
    }
    let mut total = None;
    for iteration in iterations {
        let report = match iteration {
            crate::ir::compaction::UsageIteration::Gateway(report) => Report {
                input_tokens: report.input_tokens,
                output_tokens: report.output_tokens,
                cache_read_input_tokens: report.cache_read_input_tokens,
                cache_creation_input_tokens: report.cache_creation_input_tokens,
            },
            crate::ir::compaction::UsageIteration::Native(value) => {
                Report::deserialize(value).ok()?
            }
        };
        let usage = IrUsage {
            input_tokens: report.input_tokens,
            output_tokens: report.output_tokens,
            cached_tokens: report.cache_read_input_tokens,
            cache_write_tokens: report.cache_creation_input_tokens,
            reasoning_tokens: None,
        };
        total = Some(total.map_or(usage, |previous: IrUsage| previous.saturating_sum(usage)));
    }
    total
}
