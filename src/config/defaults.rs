//! Default values for the optional fields of the configuration schema, referenced by the schema's
//! serde attributes.

use log::LevelFilter;
use std::{num::NonZero, time::Duration};

/// Default maximum number of concurrent database pool connections.
///
/// # Panics
///
/// Never: the literal 4 is nonzero, and the `expect` states that invariant.
pub(super) fn max_connections() -> NonZero<usize> {
    #[expect(clippy::expect_used, reason = "the literal 4 is nonzero")]
    let connections = NonZero::new(4).expect("4 is nonzero");

    connections
}

/// Default minimum log severity.
pub(super) fn log_level() -> LevelFilter {
    LevelFilter::Info
}

/// Default connection pool wait in milliseconds.
pub(super) fn pool_timeout() -> Duration {
    Duration::from_millis(5000)
}

/// Default request-accounting switch: recording is on unless the configuration says otherwise.
pub(super) fn api_logs_enabled() -> bool {
    true
}
