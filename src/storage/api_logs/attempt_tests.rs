//! Account-attempt stage commitment independent of the accounting recording switch.

use super::*;

/// Only a rejection from the very first dispatch permits abandonment. Accepted headers commit
/// before parsing, and any earlier counter/native-unsupported stage locks later work to the
/// same account. The first upstream header status survives every subsequent stage.
///
/// # Panics
///
/// Panics if attempt progress broadens quota failover or rewrites the first header status.
#[test]
fn only_first_unaccepted_dispatch_can_fail_over() {
    crate::test_support::install_global_configuration();
    let draft = || ApiLogDraft {
        started_at: None,
        api_key_id: 1,
        provider_id: 2,
        model_id: 3,
        model_route_id: 4,
        upstream_http_status: None,
        progress: AttemptProgress::default(),
        endpoint_type: ApiLogEndpointType::OpenaiResponses,
        is_stream: false,
        billed_usage: None,
    };
    let mut rejection = draft();
    assert!(!rejection.can_abandon_quota_rejection());
    rejection.stamp();
    rejection.observe_response(reqwest::StatusCode::TOO_MANY_REQUESTS);
    assert!(rejection.can_abandon_quota_rejection());
    rejection.stamp();
    rejection.observe_response(reqwest::StatusCode::FORBIDDEN);
    assert!(!rejection.can_abandon_quota_rejection());
    assert_eq!(rejection.upstream_http_status, Some(429));
    let mut accepted = draft();
    accepted.stamp();
    accepted.observe_response(reqwest::StatusCode::OK);
    assert!(!accepted.can_abandon_quota_rejection());
    accepted.stamp();
    accepted.observe_response(reqwest::StatusCode::TOO_MANY_REQUESTS);
    assert!(!accepted.can_abandon_quota_rejection());
    assert_eq!(accepted.upstream_http_status, Some(200));
}
