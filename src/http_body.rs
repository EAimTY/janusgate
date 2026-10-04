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

/// A bounded foreign error body with explicit proof of complete input.
pub(crate) struct ErrorBody {
    /// At most the configured error-body limit of original bytes.
    bytes: Vec<u8>,
    /// Whether the body ended successfully within the limit, including exact-limit input.
    pub(crate) is_complete: bool,
}

impl ErrorBody {
    /// Decodes complete valid foreign JSON; partial, failed, and malformed input cannot classify.
    pub(crate) fn complete_json(&self) -> Option<serde_json::Value> {
        self.is_complete
            .then(|| serde_json::from_slice(&self.bytes).ok())
            .flatten()
    }

    /// Renders human-facing text without treating lossy UTF-8 as classification evidence.
    pub(crate) fn lossy_text(&self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }
}

/// Reads one bounded upstream error body while preserving proof of complete input.
/// The original bytes never exceed the cap. Exact-cap input requires an EOF observation; any
/// subsequent byte or transport failure prevents classification even when the prefix is JSON.
pub(crate) async fn read_bounded_error_body(response: Response) -> ErrorBody {
    read_error_stream(response.bytes_stream()).await
}

/// Reads one body for legacy human-facing control errors using the same bounded read contract.
pub(crate) async fn read_error_body(response: Response) -> String {
    read_bounded_error_body(response).await.lossy_text()
}

/// Consumes a foreign byte stream once, retaining at most the bound and confirming its EOF.
async fn read_error_stream<S, B, E>(stream: S) -> ErrorBody
where
    S: tokio_stream::Stream<Item = Result<B, E>>,
    B: AsRef<[u8]>,
{
    let mut stream = Box::pin(stream);
    let mut bytes = Vec::new();
    loop {
        match stream.next().await {
            Some(Ok(chunk)) => {
                let chunk = chunk.as_ref();
                let remaining = MAX_ERROR_BODY_BYTES - bytes.len();
                bytes.extend_from_slice(chunk.get(..remaining).unwrap_or(chunk));
                if chunk.len() > remaining {
                    return ErrorBody {
                        bytes,
                        is_complete: false,
                    };
                }
            }
            Some(Err(_)) => {
                return ErrorBody {
                    bytes,
                    is_complete: false,
                };
            }
            None => {
                return ErrorBody {
                    bytes,
                    is_complete: true,
                };
            }
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Complete input at the cap remains classifiable; overflow and failed reads never do.
    /// A valid JSON prefix cannot disguise trailing bytes or a failed remainder.
    ///
    /// # Panics
    ///
    /// Panics if the reader buffers beyond its cap or mistakes partial input for complete JSON.
    #[tokio::test]
    async fn bounded_reads_require_a_confirmed_complete_body() {
        let mut exact = br#"{"error":{"code":"quota"}}"#.to_vec();
        exact.resize(MAX_ERROR_BODY_BYTES, b' ');
        let complete =
            read_error_stream(tokio_stream::iter(vec![Ok::<_, ()>(exact.clone())])).await;
        assert!(complete.is_complete);
        assert!(complete.complete_json().is_some());
        let overflow = read_error_stream(tokio_stream::iter(vec![
            Ok::<_, ()>(exact.clone()),
            Ok(vec![b'x']),
        ]))
        .await;
        assert!(!overflow.is_complete);
        assert_eq!(overflow.bytes.len(), MAX_ERROR_BODY_BYTES);
        assert!(overflow.complete_json().is_none());
        let failed = read_error_stream(tokio_stream::iter(vec![Ok(exact.clone()), Err(())])).await;
        assert!(!failed.is_complete);
        assert!(failed.complete_json().is_none());
        exact.push(b'x');
        let oversized_chunk = read_error_stream(tokio_stream::iter(vec![Ok::<_, ()>(exact)])).await;
        assert!(!oversized_chunk.is_complete);
        assert_eq!(oversized_chunk.bytes.len(), MAX_ERROR_BODY_BYTES);
        let invalid_utf8 =
            read_error_stream(tokio_stream::iter(vec![Ok::<_, ()>(vec![b'"', 255, b'"'])])).await;
        assert!(invalid_utf8.is_complete);
        assert!(invalid_utf8.complete_json().is_none());
    }
}
