//! Bounded upstream error-body reads and the shared excerpt policy for error messages.
//!
//! Every upstream failure the gateway renders — gateway error mirroring, OAuth token endpoint
//! rejections, and model discovery rejections — reads its body through [`read_error_body`] and
//! truncates it with [`truncate_excerpt`], so a hostile upstream can never stream unbounded bytes
//! into gateway memory just to shape an error message. Success bodies are legitimate JSON of
//! unbounded size and are read through the normal full-body path instead.

use reqwest::Response;
use tokio_stream::StreamExt as _;

/// Maximum number of bytes read from a non-2xx upstream body before it is truncated into an error
/// message. The excerpt is capped at 400 characters anyway, so a few kilobytes leave ample margin
/// for multi-byte characters while bounding what a hostile upstream can make the gateway buffer.
const MAX_ERROR_BODY_BYTES: usize = 4 * 1024;

/// Maximum number of characters an upstream body excerpt carries in an error message.
const EXCERPT_MAX_CHARS: usize = 400;

/// Reads at most [`MAX_ERROR_BODY_BYTES`] bytes of an upstream response body into a lossy UTF-8
/// string. The full body is never buffered; a read that fails partway keeps the bytes delivered so
/// far, because a partial excerpt still serves the error message the read exists for.
pub(crate) async fn read_error_body(response: Response) -> String {
    let mut stream = response.bytes_stream();

    let mut bytes: Vec<u8> = Vec::new();

    while bytes.len() < MAX_ERROR_BODY_BYTES {
        match stream.next().await {
            Some(Ok(chunk)) => bytes.extend_from_slice(&chunk),
            Some(Err(_)) | None => break,
        }
    }

    // The loop condition checks the budget only before each extend, so one chunk larger than the
    // cap overshoots it; the bound is enforced here, after accumulation.
    bytes.truncate(MAX_ERROR_BODY_BYTES);

    // A chunk boundary can split a multi-byte character; the excerpt is human-facing, so a
    // replacement character is acceptable.
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Truncates an upstream body to [`EXCERPT_MAX_CHARS`] characters for inclusion in an error
/// message, appending an ellipsis when truncation occurred. This is the one excerpt policy for
/// every upstream error detail the gateway renders.
pub(crate) fn truncate_excerpt(body: &str) -> String {
    if body.chars().count() <= EXCERPT_MAX_CHARS {
        body.to_string()
    } else {
        let excerpt: String = body.chars().take(EXCERPT_MAX_CHARS).collect();

        format!("{excerpt}…")
    }
}
