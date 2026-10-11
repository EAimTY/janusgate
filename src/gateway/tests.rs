//! The gateway execution-core tests that have no network or storage surface: the
//! definition-decided upstream streaming mode and the usage tracker's iteration-override rule.

#![expect(
    clippy::missing_panics_doc,
    reason = "a test fails by panicking on its own assertions; a # Panics section would \
              document nothing its name does not"
)]

use super::{IrEvent, IrUsage, UsageTracker, is_upstream_streaming};
use crate::provider::definition::inference::{Inference, UpstreamStreaming};

/// An inference descriptor carrying `upstream_streaming`.
fn inference(mode: UpstreamStreaming) -> Inference {
    Inference {
        url: crate::provider::definition::template::StringExpr::default(),
        stream_url: None,
        headers: Vec::new(),
        upstream_streaming: mode,
        render_options: Vec::new(),
        envelope: None,
        quota_exhaustion: None,
        response_unwrap: None,
    }
}

/// An always-streaming definition streams upstream regardless of the client's mode, and a
/// match-client definition mirrors it.
#[test]
fn the_definition_decides_the_upstream_streaming_mode() {
    assert!(is_upstream_streaming(
        &inference(UpstreamStreaming::Always),
        false
    ));

    assert!(is_upstream_streaming(
        &inference(UpstreamStreaming::Always),
        true
    ));

    assert!(!is_upstream_streaming(
        &inference(UpstreamStreaming::MatchClient),
        false
    ));

    assert!(is_upstream_streaming(
        &inference(UpstreamStreaming::MatchClient),
        true
    ));
}

/// The resolved usage is the latest iteration total once one arrived, and otherwise the last
/// ordinary usage observation.
#[test]
fn iteration_totals_override_the_ordinary_usage_observation() {
    let ordinary = IrUsage {
        input_tokens: 10,
        output_tokens: 20,
        cached_tokens: None,
        cache_write_tokens: None,
        reasoning_tokens: None,
    };

    let mut tracker = UsageTracker::default();

    assert!(!tracker.observe(&IrEvent::TextDelta("text".to_owned())));

    assert_eq!(tracker.resolved(), None);

    assert!(tracker.observe(&IrEvent::Usage(ordinary)));

    assert_eq!(tracker.resolved(), Some(ordinary));

    // A later ordinary observation replaces the earlier one under the last-observation rule.
    let later = IrUsage {
        input_tokens: 11,
        ..ordinary
    };

    assert!(tracker.observe(&IrEvent::Usage(later)));

    assert_eq!(tracker.resolved(), Some(later));

    let iterations = IrUsage {
        input_tokens: 30,
        output_tokens: 40,
        cached_tokens: Some(5),
        cache_write_tokens: None,
        reasoning_tokens: None,
    };

    // The iteration total rides a native vendor report, which the tracker resolves through the
    // same accounting rule the gateway ledger reports through.
    assert!(tracker.observe(&IrEvent::CompactionMetadata {
        is_paused: false,
        iterations: vec![crate::ir::compaction::UsageIteration::Native(
            serde_json::json!({
                "input_tokens": 30,
                "output_tokens": 40,
                "cache_read_input_tokens": 5
            })
        )],
    }));

    assert_eq!(tracker.resolved(), Some(iterations));
}
