//! Server-Sent Events frame parsing and writing.

/// Maximum number of bytes a single SSE frame may occupy before the parser rejects the stream as
/// malformed. Upstream payloads are JSON documents, so this bound is far above any legitimate frame
/// while preventing unbounded buffer growth from a hostile stream.
const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

/// The UTF-8 byte-order mark the SSE decoder must strip from the head of the stream.
const UTF8_BOM: [u8; 3] = [0xef, 0xbb, 0xbf];

/// One complete Server-Sent Events frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SseFrame {
    /// Concatenated `data:` lines of the frame, joined with newlines.
    pub(crate) data: String,
}

/// Incremental Server-Sent Events parser fed with raw byte chunks.
///
/// Implements the field subset of the SSE specification used by all upstream protocols: `data:`
/// fields, `:` comment lines, optional field value prefixes, and line terminators of `\n`, `\r\n`,
/// or `\r`. The other fields the specification defines (`event`, `id`, `retry`) are tolerated and
/// ignored. A frame is dispatched on an empty line only when at least one `data:` line was seen, so
/// heartbeat comments and eventless keep-alive frames never surface. One leading UTF-8 BOM is
/// stripped as the specification's decoding step requires.
#[derive(Debug, Default)]
pub(crate) struct SseParser {
    /// Bytes received but not yet consumed as complete lines.
    buffer: Vec<u8>,

    /// Whether at least one frame was malformed beyond [`MAX_FRAME_BYTES`].
    is_oversized: bool,

    /// Whether the leading byte-order mark has been resolved (present and
    /// stripped, or found absent).
    is_bom_resolved: bool,

    /// `data:` lines of the frame being assembled across chunk boundaries.
    pending_data_lines: Vec<String>,

    /// Accumulated size of the frame being assembled: every accepted `data:` line's bytes plus one
    /// join separator per line, an upper bound on the size the frame occupies once its lines are
    /// joined at dispatch. The per-line ingest bound cannot see a frame split across many lines,
    /// so this is what keeps such frames under [`MAX_FRAME_BYTES`]..
    pending_frame_bytes: usize,
}

/// Malformed Server-Sent Events stream errors.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(crate) enum SseParseError {
    /// A frame exceeded the maximum accepted frame size, counting all of its `data:` lines
    /// together.
    #[error("SSE frame exceeds the maximum accepted size")]
    FrameTooLarge,
}

impl SseParser {
    /// Creates an empty parser.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Ingests a raw byte chunk and returns every frame completed by it.
    ///
    /// # Errors
    ///
    /// Returns an error if a single frame exceeds [`MAX_FRAME_BYTES`].
    pub(crate) fn ingest(&mut self, chunk: &[u8]) -> Result<Vec<SseFrame>, SseParseError> {
        self.buffer.extend_from_slice(chunk);

        if self.buffer.len() > MAX_FRAME_BYTES {
            self.buffer.clear();
            self.is_oversized = true;

            return Err(SseParseError::FrameTooLarge);
        }

        self.drain_complete_frames()
    }

    /// Flushes a trailing frame that was terminated by the end of the stream instead of a final
    /// empty line. This covers both a final line still sitting unterminated in the buffer and a
    /// frame whose final line terminator an earlier ingest already consumed without its dispatching
    /// blank line ever arriving.
    ///
    /// # Errors
    ///
    /// Returns an error if a single frame exceeds [`MAX_FRAME_BYTES`].
    pub(crate) fn finish(&mut self) -> Result<Vec<SseFrame>, SseParseError> {
        if self.is_oversized {
            return Err(SseParseError::FrameTooLarge);
        }

        if self.pending_data_lines.is_empty() && self.buffer.is_empty() {
            return Ok(Vec::new());
        }

        // Dispatching reuses the blank-line path: either the buffer holds the frame's unterminated
        // tail, or the frame's lines already live in the pending assembly state and only the
        // appended blank line is missing. The dispatch resets `pending_frame_bytes`, so the
        // accounting stays consistent with an ordinary mid-stream dispatch.
        self.buffer.extend_from_slice(b"\n\n");

        self.drain_complete_frames()
    }

    /// Splits complete lines off the buffer and dispatches frames on blank lines.
    ///
    /// Frame assembly state lives on the parser, so a frame whose lines and dispatching blank line
    /// arrive in different chunks is still emitted whole. The frame cap counts every accepted
    /// `data:` line of the frame being assembled, so splitting one oversized frame into many lines
    /// cannot skirt [`MAX_FRAME_BYTES`]..
    fn drain_complete_frames(&mut self) -> Result<Vec<SseFrame>, SseParseError> {
        self.strip_leading_bom();

        let mut frames = Vec::new();

        // Lines are extracted from the cursor window `buffer[offset..]` and the cursor is collapsed
        // into a single front drain when scanning ends, on every exit path. Draining per line
        // instead would shift every not-yet-consumed byte to the buffer front once per line, making
        // one chunk dense in lines quadratic to ingest even while the frame cap bounds the buffer's
        // memory.
        let mut offset = 0;

        let scan: Result<(), SseParseError> = loop {
            #[expect(
                clippy::indexing_slicing,
                reason = "offset accumulates only `consumed` values that fit the scanned window"
            )]
            let next = next_line(&self.buffer[offset..]);

            match next {
                Ok(Some((line, consumed))) => {
                    offset += consumed;

                    if line.is_empty() {
                        if !self.pending_data_lines.is_empty() {
                            frames.push(SseFrame {
                                data: self.pending_data_lines.join("\n"),
                            });

                            self.pending_data_lines = Vec::new();
                            self.pending_frame_bytes = 0;
                        }

                        continue;
                    }

                    let (field, value) = match line.split_once(':') {
                        Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
                        None => (line.as_str(), ""),
                    };

                    // Only `data` fields are parsed; every other field — including the
                    // tolerated-and-ignored `event`, `id`, and `retry` — falls through.
                    if field == "data" {
                        // One separator byte per line upper-bounds the line's eventual contribution
                        // to the dispatch-time `join`, so the cap is enforced before the line is
                        // materialized.
                        self.pending_frame_bytes += value.len() + 1;

                        if self.pending_frame_bytes > MAX_FRAME_BYTES {
                            self.is_oversized = true;

                            break Err(SseParseError::FrameTooLarge);
                        }

                        self.pending_data_lines.push(value.to_string());
                    }
                }

                Ok(None) => break Ok(()),

                Err(error) => break Err(error),
            }
        };

        self.buffer.drain(..offset);

        scan?;

        Ok(frames)
    }

    /// Resolves and strips the one leading UTF-8 BOM required to be removed by the SSE
    /// specification's decoding step, tolerating a BOM split across chunk boundaries.
    fn strip_leading_bom(&mut self) {
        if self.is_bom_resolved {
            return;
        }

        // A shorter buffer may still be an in-flight BOM prefix, so the decision waits for more
        // bytes.
        let is_pending = self.buffer.len() < UTF8_BOM.len() && UTF8_BOM.starts_with(&self.buffer);

        if is_pending {
            return;
        }

        self.is_bom_resolved = true;

        if self.buffer.starts_with(&UTF8_BOM) {
            self.buffer.drain(..UTF8_BOM.len());
        }
    }
}

/// Formats one Server-Sent Events frame with the given optional event name and data payload,
/// including the terminating empty line.
///
/// The data payload must not contain newlines; multi-line payloads must be passed as separate
/// frames or joined by the caller before formatting.
pub(crate) fn format_frame(event: Option<&str>, data: &str) -> String {
    let mut frame = String::new();

    if let Some(event) = event {
        frame.push_str("event: ");
        frame.push_str(event);
        frame.push('\n');
    }

    frame.push_str("data: ");
    frame.push_str(data);
    frame.push_str("\n\n");

    frame
}

/// Formats a bare `data:`-only Server-Sent Events frame.
pub(crate) fn format_data_frame(data: &str) -> String {
    format_frame(None, data)
}

/// Extracts the next complete line from the buffer, returning the line without its terminator and
/// the number of bytes consumed.
///
/// Accepts `\n`, `\r\n`, and lone `\r` terminators per the SSE specification. A `\r` as the final
/// buffered byte is held back un-consumed until the next byte arrives, because the specification
/// requires a following `\n` to be treated as part of one `\r\n` terminator.
///
/// # Errors
///
/// Returns an error if the buffer holds more than [`MAX_FRAME_BYTES`] bytes
/// without a line terminator.
fn next_line(buffer: &[u8]) -> Result<Option<(String, usize)>, SseParseError> {
    let newline_position = buffer.iter().position(|byte| *byte == b'\n');

    // Only a `\r` before the newline can influence the terminator choice: a `\r` at or after the
    // newline selects the same arm as an absent one. The carriage scan is therefore bounded to the
    // bytes before the newline position (the whole window when no newline exists), keeping the
    // extraction of many lines from one window linear in the window size instead of quadratic in
    // the line count.
    let carriage_position = buffer
        .iter()
        .take(newline_position.unwrap_or(buffer.len()))
        .position(|byte| *byte == b'\r');

    // A `\r` as the final buffered byte may pair with a `\n` arriving in the next chunk, so its
    // fate is deliberately left undecided here.
    let is_carriage_pending =
        carriage_position.is_some_and(|position| position + 1 == buffer.len());

    let (end, terminator_length) = match (newline_position, carriage_position) {
        (Some(newline), Some(carriage)) if carriage < newline => {
            let is_carriage_newline = newline == carriage + 1;

            (carriage, if is_carriage_newline { 2 } else { 1 })
        }

        (Some(newline), _) => (newline, 1),

        (None, Some(carriage)) if !is_carriage_pending => (carriage, 1),

        // No decidable terminator, including a pending buffer-final `\r`.
        _ => {
            if buffer.len() > MAX_FRAME_BYTES {
                return Err(SseParseError::FrameTooLarge);
            }

            return Ok(None);
        }
    };

    #[expect(
        clippy::indexing_slicing,
        reason = "end is a delimiter position found in the buffer, so the range is in bounds"
    )]
    let line = String::from_utf8_lossy(&buffer[..end]).into_owned();

    Ok(Some((line, end + terminator_length)))
}
