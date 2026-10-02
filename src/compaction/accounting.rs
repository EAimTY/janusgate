//! The per-request compaction accounting ledger (README §6.8): the
//! numerical record of every actual data-plane dispatch an outer compaction
//! request performs, one iteration at a time.
//!
//! The ledger is measurement, not billing policy: each dispatched call — a
//! native compact attempt, a gateway summary inference, a threshold counter, a
//! continuation generation — appends one iteration carrying the usage the
//! upstream reported. The billed totals sum the iterations; the client-visible
//! top-level usage is a separate view the codecs render from the ledger's
//! message-stage observation. Numerical threshold estimates are never recorded:
//! an estimate describes context size, not billed work.

use crate::ir::IrUsage;
use serde::Deserialize as _;

/// The kind of one dispatched iteration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IterationKind {
    /// A compaction-producing call: a native compact dispatch or a gateway
    /// summary inference.
    Compaction,

    /// An ordinary message generation — the continuation after a checkpoint.
    Message,

    /// A native token-count call backing a threshold decision; accounted for
    /// according to its real endpoint contract, never as a billed generation.
    Counter,
}

impl IterationKind {
    /// Whether the iteration is a compaction-producing call.
    #[must_use]
    pub(crate) const fn is_compaction(self) -> bool {
        matches!(self, Self::Compaction)
    }
}

/// One dispatched call's accounting record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IterationRecord {
    /// The call kind.
    pub(crate) kind: IterationKind,

    /// The usage the upstream reported for the call.
    pub(crate) usage: IrUsage,
}

/// The per-request ledger of dispatched compaction-path calls.
#[derive(Debug, Default)]
pub(crate) struct CompactionLedger {
    /// The recorded iterations in dispatch order.
    iterations: Vec<IterationRecord>,
}

impl CompactionLedger {
    /// Records one dispatched call's usage.
    ///
    /// A call the upstream answered without a usage report acquires no
    /// fabricated token counts: record nothing for it.
    pub(crate) fn record(&mut self, kind: IterationKind, usage: Option<IrUsage>) {
        if let Some(usage) = usage {
            self.iterations.push(IterationRecord { kind, usage });
        }
    }

    /// The billed totals: input and output sums over every recorded iteration,
    /// each breakdown summed when any iteration reported it. A breakdown no
    /// iteration reported stays unreported. Sums saturate rather than overflow.
    #[must_use]
    pub(crate) fn billed_totals(&self) -> IrUsage {
        let mut totals = IrUsage::default();

        let mut has_cached = false;
        let mut has_cache_write = false;
        let mut has_reasoning = false;

        for record in &self.iterations {
            let usage = record.usage;

            totals.input_tokens = totals.input_tokens.saturating_add(usage.input_tokens);
            totals.output_tokens = totals.output_tokens.saturating_add(usage.output_tokens);

            if let Some(cached) = usage.cached_tokens {
                has_cached = true;
                totals.cached_tokens =
                    Some(totals.cached_tokens.unwrap_or(0).saturating_add(cached));
            }

            if let Some(cache_write) = usage.cache_write_tokens {
                has_cache_write = true;
                totals.cache_write_tokens = Some(
                    totals
                        .cache_write_tokens
                        .unwrap_or(0)
                        .saturating_add(cache_write),
                );
            }

            if let Some(reasoning) = usage.reasoning_tokens {
                has_reasoning = true;
                totals.reasoning_tokens = Some(
                    totals
                        .reasoning_tokens
                        .unwrap_or(0)
                        .saturating_add(reasoning),
                );
            }
        }

        if !has_cached {
            totals.cached_tokens = None;
        }

        if !has_cache_write {
            totals.cache_write_tokens = None;
        }

        if !has_reasoning {
            totals.reasoning_tokens = None;
        }

        totals
    }

    /// The usage of the last recorded message iteration — the top-level view a
    /// Messages response renders: the non-compaction iteration the caller's
    /// answer consumed, excluding compaction work.
    #[must_use]
    pub(crate) fn message_iteration_usage(&self) -> Option<IrUsage> {
        self.iterations
            .iter()
            .rev()
            .find(|record| record.kind == IterationKind::Message)
            .map(|record| record.usage)
    }

    /// The recorded iterations in dispatch order.
    #[must_use]
    pub(crate) fn iterations(&self) -> &[IterationRecord] {
        &self.iterations
    }
}

/// Sums a vendor's authoritative sampling iteration reports without inventing absent usage.
pub(crate) fn reported_iteration_totals(iterations: &[serde_json::Value]) -> Option<IrUsage> {
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
        let report = Report::deserialize(iteration).ok()?;
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
    use super::{CompactionLedger, IterationKind};
    use crate::ir::IrUsage;

    /// Builds one usage observation.
    fn usage(input: u64, output: u64) -> IrUsage {
        IrUsage {
            input_tokens: input,
            output_tokens: output,
            cached_tokens: None,
            cache_write_tokens: None,
            reasoning_tokens: None,
        }
    }

    /// The ledger sums billed totals across compaction and message iterations,
    /// and a call without a usage report acquires no fabricated counts
    /// (README §6.8's accounting contract: 180000/3500 plus 23000/1000 bills
    /// 203000/4500).
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails — the assertion is the failure report.
    #[test]
    fn bills_the_sum_of_every_dispatched_iteration() {
        let mut ledger = CompactionLedger::default();

        ledger.record(
            IterationKind::Compaction,
            Some(IrUsage {
                input_tokens: 180_000,
                output_tokens: 3_500,
                cached_tokens: Some(500),
                cache_write_tokens: None,
                reasoning_tokens: None,
            }),
        );

        ledger.record(IterationKind::Message, Some(usage(23_000, 1_000)));

        // A dispatch the upstream answered without usage: no fabricated row.
        ledger.record(IterationKind::Counter, None);

        assert_eq!(ledger.iterations().len(), 2);

        let totals = ledger.billed_totals();

        assert_eq!(totals.input_tokens, 203_000);
        assert_eq!(totals.output_tokens, 4_500);
        assert_eq!(totals.cached_tokens, Some(500));
        assert_eq!(totals.cache_write_tokens, None);
        assert_eq!(totals.reasoning_tokens, None);

        assert_eq!(ledger.message_iteration_usage(), Some(usage(23_000, 1_000)));
    }

    /// Breakdown columns only appear when at least one iteration reported them,
    /// and they sum when several do.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails — the assertion is the failure report.
    #[test]
    fn breakdowns_sum_only_when_reported() {
        let mut ledger = CompactionLedger::default();

        ledger.record(
            IterationKind::Compaction,
            Some(IrUsage {
                cached_tokens: Some(10),
                cache_write_tokens: Some(20),
                reasoning_tokens: Some(30),
                ..usage(100, 50)
            }),
        );
        ledger.record(
            IterationKind::Message,
            Some(IrUsage {
                cached_tokens: Some(1),
                cache_write_tokens: Some(2),
                reasoning_tokens: Some(3),
                ..usage(10, 5)
            }),
        );

        let totals = ledger.billed_totals();

        assert_eq!(totals.cached_tokens, Some(11));
        assert_eq!(totals.cache_write_tokens, Some(22));
        assert_eq!(totals.reasoning_tokens, Some(33));

        // A ledger without any breakdown reports none.
        let mut plain = CompactionLedger::default();
        plain.record(IterationKind::Message, Some(usage(1, 1)));

        let totals = plain.billed_totals();

        assert_eq!(totals.cached_tokens, None);
        assert_eq!(totals.cache_write_tokens, None);
        assert_eq!(totals.reasoning_tokens, None);
    }

    /// The message-iteration view is the LAST message record, skipping later
    /// compaction records, and is absent before any message dispatch.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails — the assertion is the failure report.
    #[test]
    fn message_view_is_the_last_message_iteration() {
        let mut ledger = CompactionLedger::default();

        assert_eq!(ledger.message_iteration_usage(), None);

        ledger.record(IterationKind::Message, Some(usage(1, 1)));
        ledger.record(IterationKind::Compaction, Some(usage(2, 2)));

        assert_eq!(ledger.message_iteration_usage(), Some(usage(1, 1)));

        ledger.record(IterationKind::Message, Some(usage(3, 3)));

        assert_eq!(ledger.message_iteration_usage(), Some(usage(3, 3)));
        assert!(IterationKind::Compaction.is_compaction());
        assert!(!IterationKind::Message.is_compaction());
    }
}
