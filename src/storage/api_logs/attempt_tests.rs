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
    let draft = || ApiLogDraft::predispatch(1, 2, 3, 4, ApiLogEndpointType::OpenaiResponses, false);
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
