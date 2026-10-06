//! Bounded upstream body reads and the shared excerpt policy for error messages.
//!
//! Every upstream failure the gateway renders — gateway error mirroring, OAuth token endpoint
//! rejections, and model discovery rejections — reads its body through [`read_error_body`] and
//! truncates it with [`truncate_excerpt`], so a hostile upstream can never stream unbounded bytes
//! into gateway memory just to shape an error message. Upstream success bodies are read through
//! [`read_success_body`], bounded at [`MAX_SUCCESS_BODY_BYTES`] exactly as the streaming path
//! bounds every SSE frame, so neither transport shape lets a misbehaving upstream grow gateway
//! memory without limit.

use reqwest::Response;
use tokio_stream::StreamExt as _;

/// Maximum number of bytes read from a non-2xx upstream body before it is truncated into an error
/// message. The excerpt is capped at 400 characters anyway, so a few kilobytes leave ample margin
/// for multi-byte characters while bounding what a hostile upstream can make the gateway buffer.
const MAX_ERROR_BODY_BYTES: usize = 4 * 1024;

/// Maximum number of bytes read from one upstream success body before the read fails as
/// oversized. A success body is one complete non-streaming completion document or a discovery
/// listing, so the bound sits at the outward request-body cap of 64 MiB (README §4) — the
/// largest single exchange the gateway already buffers per request — far above any legitimate
/// JSON while giving the JSON transport the same hostile-upstream posture the 16 MiB per-frame
/// `MAX_FRAME_BYTES` bound gives the SSE transport (`crate::sse`).
const MAX_SUCCESS_BODY_BYTES: usize = 64 * 1024 * 1024;

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
    let (bytes, end) = read_bounded_stream(response.bytes_stream(), MAX_ERROR_BODY_BYTES).await;
    ErrorBody {
        is_complete: matches!(end, BoundedReadEnd::Complete),
        bytes,
    }
}

/// Reads one body for legacy human-facing control errors using the same bounded read contract.
pub(crate) async fn read_error_body(response: Response) -> String {
    read_bounded_error_body(response).await.lossy_text()
}

/// Why one bounded upstream success-body read failed.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SuccessBodyError {
    /// The upstream sent more than [`MAX_SUCCESS_BODY_BYTES`] before ending the body.
    #[error("oversized upstream response body")]
    Oversized,

    /// The transport failed before the body ended.
    #[error("upstream read failed: {0}")]
    Read(#[from] reqwest::Error),
}

/// Reads one upstream success body in full, bounding it at [`MAX_SUCCESS_BODY_BYTES`] — the JSON
/// transport's counterpart of the SSE frame cap. Exact-cap input with a confirmed end is the
/// complete body; anything beyond the cap or a transport failure ends the read without one.
///
/// # Errors
///
/// Returns [`SuccessBodyError::Oversized`] for a body beyond the cap and
/// [`SuccessBodyError::Read`] for a transport failure before the body's end.
pub(crate) async fn read_success_body(response: Response) -> Result<Vec<u8>, SuccessBodyError> {
    let (bytes, end) = read_bounded_stream(response.bytes_stream(), MAX_SUCCESS_BODY_BYTES).await;
    match end {
        BoundedReadEnd::Complete => Ok(bytes),
        BoundedReadEnd::Oversized => Err(SuccessBodyError::Oversized),
        BoundedReadEnd::Failed(error) => Err(SuccessBodyError::Read(error)),
    }
}

/// How one bounded stream read ended.
enum BoundedReadEnd<E> {
    /// The stream ended cleanly within the cap.
    Complete,
    /// The stream carried more than the cap before its end.
    Oversized,
    /// The stream failed before its end.
    Failed(E),
}

/// Reads one byte stream once, retaining at most `cap` bytes and classifying how the read ended.
/// Exact-cap input requires an observed end; any subsequent byte or transport failure ends the
/// read incomplete even when the prefix was well-formed.
async fn read_bounded_stream<S, B, E>(stream: S, cap: usize) -> (Vec<u8>, BoundedReadEnd<E>)
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
                let remaining = cap - bytes.len();
                bytes.extend_from_slice(chunk.get(..remaining).unwrap_or(chunk));
                if chunk.len() > remaining {
                    return (bytes, BoundedReadEnd::Oversized);
                }
            }
            Some(Err(error)) => return (bytes, BoundedReadEnd::Failed(error)),
            None => return (bytes, BoundedReadEnd::Complete),
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
        let (bytes, end) = read_bounded_stream(
            tokio_stream::iter(vec![Ok::<_, ()>(exact.clone())]),
            MAX_ERROR_BODY_BYTES,
        )
        .await;
        assert!(matches!(end, BoundedReadEnd::Complete));
        let complete = ErrorBody {
            is_complete: matches!(end, BoundedReadEnd::Complete),
            bytes,
        };
        assert!(complete.complete_json().is_some());
        let (bytes, overflow_end) = read_bounded_stream(
            tokio_stream::iter(vec![Ok::<_, ()>(exact.clone()), Ok(vec![b'x'])]),
            MAX_ERROR_BODY_BYTES,
        )
        .await;
        assert!(matches!(overflow_end, BoundedReadEnd::Oversized));
        let overflow = ErrorBody {
            is_complete: matches!(overflow_end, BoundedReadEnd::Complete),
            bytes,
        };
        assert_eq!(overflow.bytes.len(), MAX_ERROR_BODY_BYTES);
        assert!(overflow.complete_json().is_none());
        let (bytes, failed_end) = read_bounded_stream(
            tokio_stream::iter(vec![Ok::<_, ()>(exact.clone()), Err(())]),
            MAX_ERROR_BODY_BYTES,
        )
        .await;
        assert!(matches!(failed_end, BoundedReadEnd::Failed(())));
        let failed = ErrorBody {
            is_complete: matches!(failed_end, BoundedReadEnd::Complete),
            bytes,
        };
        assert!(failed.complete_json().is_none());
        exact.push(b'x');
        let (bytes, chunk_end) = read_bounded_stream(
            tokio_stream::iter(vec![Ok::<_, ()>(exact)]),
            MAX_ERROR_BODY_BYTES,
        )
        .await;
        assert!(matches!(chunk_end, BoundedReadEnd::Oversized));
        assert_eq!(bytes.len(), MAX_ERROR_BODY_BYTES);
        let (bytes, utf8_end) = read_bounded_stream(
            tokio_stream::iter(vec![Ok::<_, ()>(vec![b'"', 255, b'"'])]),
            MAX_ERROR_BODY_BYTES,
        )
        .await;
        assert!(matches!(utf8_end, BoundedReadEnd::Complete));
        let invalid_utf8 = ErrorBody {
            is_complete: matches!(utf8_end, BoundedReadEnd::Complete),
            bytes,
        };
        assert!(invalid_utf8.complete_json().is_none());
    }

    /// The success-body read shares the core at its own caller-supplied cap: a body split across
    /// chunks at exactly the cap is complete, anything beyond the cap is oversized with the
    /// prefix retained, and a failed transport never masquerades as complete.
    ///
    /// # Panics
    ///
    /// Panics if the bounded read misclassifies any end at a caller-supplied cap.
    #[tokio::test]
    async fn success_reads_classify_their_end_at_the_body_cap() {
        const CAP: usize = 8;

        let (bytes, end) = read_bounded_stream(
            tokio_stream::iter(vec![Ok::<_, ()>(b"12345".to_vec()), Ok(b"678".to_vec())]),
            CAP,
        )
        .await;
        assert_eq!(bytes, b"12345678".to_vec());
        assert!(matches!(end, BoundedReadEnd::Complete));

        let (_, end) = read_bounded_stream(
            tokio_stream::iter(vec![Ok::<_, ()>(b"12345678".to_vec()), Ok(b"9".to_vec())]),
            CAP,
        )
        .await;
        assert!(matches!(end, BoundedReadEnd::Oversized));

        let (bytes, end) = read_bounded_stream(
            tokio_stream::iter(vec![Ok::<_, ()>(vec![b'x'; CAP + 1])]),
            CAP,
        )
        .await;
        assert_eq!(bytes.len(), CAP);
        assert!(matches!(end, BoundedReadEnd::Oversized));

        let (_, end) = read_bounded_stream(
            tokio_stream::iter(vec![Ok::<_, ()>(b"1234".to_vec()), Err(())]),
            CAP,
        )
        .await;
        assert!(matches!(end, BoundedReadEnd::Failed(())));
    }
}
