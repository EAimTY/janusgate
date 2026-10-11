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

    /// Parsed `event:` name of the frame, when the upstream sent one. Anthropic and Interactions
    /// upstream parsers discriminate on this name; the JSON payloads' own `type`/`event_type`
    /// members are not relied on for discrimination (`docs/design/providers.md`).
    pub(crate) event: Option<String>,
}

/// Incremental Server-Sent Events parser fed with raw byte chunks.
///
/// Implements the field subset of the SSE specification used by all upstream protocols: `data:`
/// and `event:` fields, `:` comment lines, optional field value prefixes, and line terminators of
/// `\n`, `\r\n`, or `\r`. The other fields the specification defines (`id`, `retry`) are
/// tolerated and ignored. A frame is dispatched on an empty line only when its joined `data:`
/// payload is non-empty, so heartbeat comments and `data:`-only keep-alive frames never surface.
/// One leading UTF-8 BOM is stripped as the specification's decoding step requires.
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

    /// `event:` name of the frame being assembled, from the last `event:` line before dispatch.
    pending_event: Option<String>,

    /// Accumulated size of the frame being assembled: every accepted `data:` line's bytes plus one
    /// join separator per line, an upper bound on the size the frame occupies once its lines are
    /// joined at dispatch. The per-line ingest bound cannot see a frame split across many lines,
    /// so this is what keeps such frames under [`MAX_FRAME_BYTES`].
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
    /// cannot skirt [`MAX_FRAME_BYTES`].
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
                        // Per the specification's "dispatch the event" steps, an empty data
                        // buffer dispatches nothing and both assembly buffers are cleared. The
                        // joined payload is empty exactly when every accepted `data:` line's
                        // value was empty after the single-space strip — the shape of
                        // `data:`-only keep-alive frames — while a second empty line joins to a
                        // lone newline and still dispatches.
                        let data = self.pending_data_lines.join("\n");
                        let event = self.pending_event.take();

                        self.pending_data_lines = Vec::new();
                        self.pending_frame_bytes = 0;

                        if !data.is_empty() {
                            frames.push(SseFrame { data, event });
                        }

                        continue;
                    }

                    let (field, value) = match line.split_once(':') {
                        Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
                        None => (line.as_str(), ""),
                    };

                    // `data` lines accumulate the payload; the last `event` line names the frame
                    // for event-name-discriminated upstreams. Every other field — including the
                    // tolerated-and-ignored `id` and `retry` — falls through.
                    if field == "event" {
                        self.pending_event = Some(value.to_string());
                    } else if field == "data" {
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

#[cfg(test)]
mod tests {
    //! Tests for the SSE frame machinery: field assembly across chunk boundaries, the three
    //! line terminators, the BOM strip, keep-alive suppression, the frame cap, and the writer's
    //! round-trip through the parser.

    #![expect(
        clippy::missing_panics_doc,
        clippy::indexing_slicing,
        reason = "a test fails by panicking, and fixtures index frame vectors whose positions \
                  the assertions pin; a # Panics section on every test would restate that"
    )]

    use super::{SseFrame, SseParseError, SseParser, format_data_frame, format_frame};

    /// Ingests `chunks` through a fresh parser and returns every frame and the parser's final
    /// flush.
    fn parsed(chunks: &[&[u8]]) -> Result<Vec<SseFrame>, SseParseError> {
        let mut parser = SseParser::new();
        let mut frames = Vec::new();

        for chunk in chunks {
            frames.extend(parser.ingest(chunk)?);
        }

        frames.extend(parser.finish()?);

        Ok(frames)
    }

    /// A frame carries its concatenated data lines and its last event name; multi-line payloads
    /// join with newlines, multiple frames dispatch per chunk, and the tolerated `id` and
    /// `retry` fields are ignored.
    #[test]
    fn frames_assemble_data_and_event_fields() -> Result<(), Box<dyn std::error::Error>> {
        let frames = parsed(&[
            b"event: alpha\nid: 7\nretry: 100\ndata: first\ndata: second\n\n",
            b"event: beta\ndata: only\n\n",
        ])?;

        assert_eq!(
            frames,
            vec![
                SseFrame {
                    data: "first\nsecond".to_string(),
                    event: Some("alpha".to_string()),
                },
                SseFrame {
                    data: "only".to_string(),
                    event: Some("beta".to_string()),
                },
            ]
        );

        Ok(())
    }

    /// Field values tolerate the optional single-space prefix and lines without a colon are
    /// whole-line fields with empty values.
    #[test]
    fn field_value_prefixes_are_tolerated() -> Result<(), Box<dyn std::error::Error>> {
        let frames = parsed(&[b"data:with nospace\ndata: with space\n\n"])?;

        assert_eq!(
            frames,
            vec![SseFrame {
                data: "with nospace\nwith space".to_string(),
                event: None,
            }]
        );

        Ok(())
    }

    /// All three specification terminators — `\n`, `\r\n`, and lone `\r` — end a line, and a
    /// `\r` at the end of one chunk pairs with a `\n` arriving in the next.
    #[test]
    fn all_terminators_end_lines_across_chunks() -> Result<(), Box<dyn std::error::Error>> {
        let frames = parsed(&[
            b"data: lf\ndata: split",
            b"\r\ndata: cr\rdata: done\r",
            b"\n\n",
        ])?;

        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].data, "lf\nsplit\ncr\ndone");

        Ok(())
    }

    /// One leading UTF-8 BOM is stripped even when split across chunks, and only at the head of
    /// the stream.
    #[test]
    fn a_leading_bom_is_stripped_across_chunks() -> Result<(), Box<dyn std::error::Error>> {
        let split = parsed(&[
            &[0xef, 0xbb],
            &[0xbf, b'd', b'a', b't', b'a', b':', b' ', b'x', b'\n', b'\n'],
        ])?;

        assert_eq!(split[0].data, "x");

        let whole = parsed(&[&[
            0xef, 0xbb, 0xbf, b'd', b'a', b't', b'a', b':', b' ', b'y', b'\n', b'\n',
        ]])?;

        assert_eq!(whole[0].data, "y");

        let mid_stream = parsed(&[b"data: a\n\n", &[0xef, 0xbb, 0xbf], b"\n"])?;

        // A BOM after the first byte is content, not a byte-order mark.
        assert_eq!(mid_stream.len(), 1);
        assert_eq!(mid_stream[0].data, "a");

        Ok(())
    }

    /// Comment-only streams and `data:`-only keep-alives dispatch nothing, while two empty data
    /// lines join to a lone newline and still dispatch.
    #[test]
    fn comments_and_data_only_keepalives_stay_suppressed() -> Result<(), Box<dyn std::error::Error>>
    {
        let comments = parsed(&[b": heartbeat\n: again\n\n"])?;

        assert!(comments.is_empty());

        let keepalive = parsed(&[b"data:\n\n"])?;

        assert!(keepalive.is_empty());

        let lone_newline = parsed(&[b"data:\ndata:\n\n"])?;

        assert_eq!(
            lone_newline,
            vec![SseFrame {
                data: "\n".to_string(),
                event: None,
            }]
        );

        let interleaved = parsed(&[b": ping\ndata: real\n: pong\n\n"])?;

        assert_eq!(interleaved.len(), 1);
        assert_eq!(interleaved[0].data, "real");

        Ok(())
    }

    /// An event line with no data dispatches nothing, and the last event line before dispatch
    /// names the frame.
    #[test]
    fn events_without_data_never_dispatch() -> Result<(), Box<dyn std::error::Error>> {
        let frames = parsed(&[
            b"event: dropped\n\n",
            b"event: first\nevent: last\ndata: x\n\n",
        ])?;

        assert_eq!(
            frames,
            vec![SseFrame {
                data: "x".to_string(),
                event: Some("last".to_string()),
            }]
        );

        Ok(())
    }

    /// A stream that ends without its dispatching blank line still yields its trailing frame,
    /// and a fully drained stream finishes empty.
    #[test]
    fn finish_flushes_the_unterminated_tail() -> Result<(), Box<dyn std::error::Error>> {
        let unterminated = parsed(&[b"data: tail\n"])?;

        assert_eq!(unterminated.len(), 1);
        assert_eq!(unterminated[0].data, "tail");

        let consumed_terminator = parsed(&[b"data: tail\ndata: two\n"])?;

        assert_eq!(consumed_terminator[0].data, "tail\ntwo");

        let drained = parsed(&[b"data: done\n\n"])?;

        assert!(drained.len() == 1);

        let empty = parsed(&[b""])?;

        assert!(empty.is_empty());

        Ok(())
    }

    /// A frame assembled across many data lines cannot skirt the cap: the per-line accounting
    /// fails the frame once its lines together exceed the bound, and the failure is sticky for
    /// the rest of the stream.
    #[test]
    fn oversized_frames_fail_across_lines_and_stay() -> Result<(), Box<dyn std::error::Error>> {
        let line = vec![b'x'; 1024 * 1024];
        let mut chunks: Vec<&[u8]> = Vec::new();

        for _ in 0..17 {
            chunks.push(b"data: ");
            chunks.push(&line);
            chunks.push(b"\n");
        }

        chunks.push(b"\n");

        let mut parser = SseParser::new();

        // Lines up to and including the one crossing the cap are accepted; the frame never
        // dispatches.
        let mut is_failed = false;

        for chunk in chunks {
            match parser.ingest(chunk) {
                Ok(_) => {}
                Err(SseParseError::FrameTooLarge) => {
                    is_failed = true;

                    break;
                }
            }
        }

        assert!(is_failed);
        assert!(matches!(parser.finish(), Err(SseParseError::FrameTooLarge)));

        Ok(())
    }

    /// A single chunk beyond the cap fails immediately, before the parser buffers it.
    #[test]
    fn an_oversized_chunk_fails_immediately() {
        let oversized = vec![b'y'; 16 * 1024 * 1024 + 1];
        let mut parser = SseParser::new();

        assert!(matches!(
            parser.ingest(&oversized),
            Err(SseParseError::FrameTooLarge)
        ));
        assert!(matches!(parser.finish(), Err(SseParseError::FrameTooLarge)));
    }

    /// The writer's frames parse back to exactly the event and data they carry, and the bare
    /// data writer emits no event line.
    #[test]
    fn written_frames_round_trip_through_the_parser() -> Result<(), Box<dyn std::error::Error>> {
        let framed = format_frame(Some("message"), "{\"i\": 1}");
        let mut parser = SseParser::new();

        assert_eq!(
            parser.ingest(framed.as_bytes())?,
            vec![SseFrame {
                data: "{\"i\": 1}".to_string(),
                event: Some("message".to_string()),
            }]
        );

        let bare = format_data_frame("just data");
        let mut bare_parser = SseParser::new();

        assert_eq!(
            bare_parser.ingest(bare.as_bytes())?,
            vec![SseFrame {
                data: "just data".to_string(),
                event: None,
            }]
        );
        assert!(format_data_frame("x").starts_with("data: x\n\n"));
        assert!(format_frame(Some("e"), "x").starts_with("event: e\ndata: x\n\n"));

        Ok(())
    }
}
