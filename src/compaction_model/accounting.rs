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

#[cfg(test)]
mod tests {
    #![expect(
        clippy::missing_panics_doc,
        reason = "the tests' assertions panic; a # Panics section on every test would \
                  restate that"
    )]

    use super::{CompactionLedger, reported_iteration_totals};
    use crate::ir::{IrUsage, compaction::UsageIteration};
    use std::error::Error as StdError;

    /// A ledger records only dispatches the upstream answered with usage; an empty ledger and a
    /// usage-less dispatch both leave the reported billed totals absent, so no protocol-required
    /// zero can become a recorded token fact.
    #[test]
    fn usage_less_dispatches_acquire_no_fabricated_totals() {
        let mut ledger = CompactionLedger::default();

        assert_eq!(ledger.reported_billed_totals(), None);

        ledger.record(None);

        assert_eq!(ledger.reported_billed_totals(), None);

        assert!(ledger.iterations().is_empty());
    }

    /// Billed totals sum every recorded iteration field by field; a breakdown sums when any
    /// iteration reported it, stays unreported when none did, and the sums saturate rather than
    /// overflow. An explicit zero report remains a present measurement.
    #[test]
    fn billed_totals_sum_breakdowns_and_saturate() {
        let mut ledger = CompactionLedger::default();

        ledger.record(Some(IrUsage {
            input_tokens: 100,
            output_tokens: 10,
            cached_tokens: Some(40),
            cache_write_tokens: None,
            reasoning_tokens: None,
        }));

        ledger.record(Some(IrUsage {
            input_tokens: 30,
            output_tokens: 5,
            cached_tokens: None,
            cache_write_tokens: Some(7),
            reasoning_tokens: Some(3),
        }));

        let totals = ledger.billed_totals();

        assert_eq!(totals.input_tokens, 130);
        assert_eq!(totals.output_tokens, 15);
        assert_eq!(totals.cached_tokens, Some(40), "a single report is kept");
        assert_eq!(totals.cache_write_tokens, Some(7));
        assert_eq!(totals.reasoning_tokens, Some(3));

        assert_eq!(
            ledger.reported_billed_totals(),
            Some(totals),
            "a non-empty ledger reports its totals"
        );

        ledger.record(Some(IrUsage {
            input_tokens: u64::MAX,
            output_tokens: 0,
            cached_tokens: None,
            cache_write_tokens: None,
            reasoning_tokens: None,
        }));

        assert_eq!(ledger.billed_totals().input_tokens, u64::MAX);
    }

    /// An explicit zero usage report is a real observation: it records an iteration and reports
    /// present zero totals, distinct from the absent totals of an empty ledger.
    #[test]
    fn explicit_zero_reports_stay_present() -> Result<(), Box<dyn StdError>> {
        let mut ledger = CompactionLedger::default();

        ledger.record(Some(IrUsage::default()));

        assert_eq!(ledger.iterations().len(), 1);

        let totals = ledger
            .reported_billed_totals()
            .ok_or("an explicit report is present")?;

        assert_eq!(totals.input_tokens, 0);
        assert_eq!(totals.output_tokens, 0);
        assert_eq!(totals.cached_tokens, None);

        Ok(())
    }

    /// Iteration totals sum the known counters of gateway and producer-owned reports and invent
    /// nothing: a producer iteration outside the known counter shape aborts the total entirely.
    #[test]
    fn iteration_totals_sum_known_counters_only() {
        assert_eq!(reported_iteration_totals(&[]), None);

        let gateway = [UsageIteration::gateway(
            crate::ir::compaction::SamplingKind::Compaction,
            IrUsage {
                input_tokens: 10,
                output_tokens: 4,
                cached_tokens: Some(2),
                cache_write_tokens: None,
                reasoning_tokens: None,
            },
        )];

        assert_eq!(
            reported_iteration_totals(&gateway),
            Some(IrUsage {
                input_tokens: 10,
                output_tokens: 4,
                cached_tokens: Some(2),
                cache_write_tokens: None,
                reasoning_tokens: None,
            })
        );

        let native = [
            serde_json::json!({"input_tokens": 7, "output_tokens": 3}),
            serde_json::json!({
                "input_tokens": 1,
                "output_tokens": 1,
                "cache_read_input_tokens": 5,
                "cache_creation_input_tokens": 6,
                "vendor_extension": {"anything": true}
            }),
        ]
        .into_iter()
        .map(UsageIteration::from)
        .collect::<Vec<_>>();

        assert_eq!(
            reported_iteration_totals(&native),
            Some(IrUsage {
                input_tokens: 8,
                output_tokens: 4,
                cached_tokens: Some(5),
                cache_write_tokens: Some(6),
                reasoning_tokens: None,
            }),
            "producer reports sum beside their vendor extensions"
        );

        let unusable = [UsageIteration::from(serde_json::json!({
            "output_tokens": 3
        }))];

        assert_eq!(
            reported_iteration_totals(&unusable),
            None,
            "a producer report without the known counters yields no total"
        );
    }
}
