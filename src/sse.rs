//! Server-Sent Events frame parsing and writing.

/// Maximum number of bytes a single SSE frame may occupy before the parser
/// rejects the stream as malformed. Upstream payloads are JSON documents, so
/// this bound is far above any legitimate frame while preventing unbounded
/// buffer growth from a hostile stream.
pub(crate) const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

/// The UTF-8 byte-order mark the SSE decoder must strip from the head of the
/// stream.
const UTF8_BOM: [u8; 3] = [0xef, 0xbb, 0xbf];

/// One complete Server-Sent Events frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SseFrame {
    /// Optional `event:` field of the frame.
    pub(crate) event: Option<String>,

    /// Concatenated `data:` lines of the frame, joined with newlines.
    pub(crate) data: String,
}

/// Incremental Server-Sent Events parser fed with raw byte chunks.
///
/// Implements the field subset of the SSE specification used by all upstream
/// protocols: `event:` and `data:` fields, `:` comment lines, optional field
/// value prefixes, and line terminators of `\n`, `\r\n`, or `\r`. Unknown
/// fields (`id`, `retry`) are ignored. A frame is dispatched on an empty line
/// only when at least one `data:` line was seen, so heartbeat comments and
/// eventless keep-alive frames never surface. One leading UTF-8 BOM is
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

    /// `event:` field of the frame being assembled across chunk boundaries.
    pending_event: Option<String>,

    /// `data:` lines of the frame being assembled across chunk boundaries.
    pending_data_lines: Vec<String>,

    /// Accumulated size of the frame being assembled: every accepted `data:`
    /// line's bytes plus one join separator per line, an upper bound on the
    /// size the frame occupies once its lines are joined at dispatch. The
    /// per-line ingest bound cannot see a frame split across many lines, so
    /// this is what keeps such frames under [`MAX_FRAME_BYTES`].
    pending_frame_bytes: usize,
}

/// Malformed Server-Sent Events stream errors.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(crate) enum SseParseError {
    /// A frame exceeded the maximum accepted frame size, counting all of its
    /// `data:` lines together.
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

    /// Flushes a trailing frame that was terminated by the end of the stream
    /// instead of a final empty line.
    ///
    /// # Errors
    ///
    /// Returns an error if a single frame exceeds [`MAX_FRAME_BYTES`].
    pub(crate) fn finish(&mut self) -> Result<Vec<SseFrame>, SseParseError> {
        if self.is_oversized {
            return Err(SseParseError::FrameTooLarge);
        }

        if self.buffer.is_empty() {
            return Ok(Vec::new());
        }

        self.buffer.extend_from_slice(b"\n\n");

        self.drain_complete_frames()
    }

    /// Splits complete lines off the buffer and dispatches frames on blank
    /// lines.
    ///
    /// Frame assembly state lives on the parser, so a frame whose lines and
    /// dispatching blank line arrive in different chunks is still emitted
    /// whole. The frame cap counts every accepted `data:` line of the frame
    /// being assembled, so splitting one oversized frame into many lines
    /// cannot skirt [`MAX_FRAME_BYTES`].
    fn drain_complete_frames(&mut self) -> Result<Vec<SseFrame>, SseParseError> {
        self.strip_leading_bom();

        let mut frames = Vec::new();

        while let Some((line, consumed)) = next_line(&self.buffer)? {
            self.buffer.drain(..consumed);

            if line.is_empty() {
                if !self.pending_data_lines.is_empty() {
                    frames.push(SseFrame {
                        event: self.pending_event.take(),
                        data: self.pending_data_lines.join("\n"),
                    });

                    self.pending_data_lines = Vec::new();
                    self.pending_frame_bytes = 0;
                }

                self.pending_event = None;

                continue;
            }

            let (field, value) = match line.split_once(':') {
                Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
                None => (line.as_str(), ""),
            };

            match field {
                "event" => self.pending_event = Some(value.to_string()),
                "data" => {
                    // One separator byte per line upper-bounds the line's
                    // eventual contribution to the dispatch-time `join`, so
                    // the cap is enforced before the line is materialized.
                    self.pending_frame_bytes += value.len() + 1;

                    if self.pending_frame_bytes > MAX_FRAME_BYTES {
                        self.is_oversized = true;

                        return Err(SseParseError::FrameTooLarge);
                    }

                    self.pending_data_lines.push(value.to_string());
                }
                _ => {}
            }
        }

        Ok(frames)
    }

    /// Resolves and strips the one leading UTF-8 BOM required to be removed
    /// by the SSE specification's decoding step, tolerating a BOM split
    /// across chunk boundaries.
    fn strip_leading_bom(&mut self) {
        if self.is_bom_resolved {
            return;
        }

        // A shorter buffer may still be an in-flight BOM prefix, so the
        // decision waits for more bytes.
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

/// Extracts the next complete line from the buffer, returning the line without
/// its terminator and the number of bytes consumed.
///
/// Accepts `\n`, `\r\n`, and lone `\r` terminators per the SSE specification.
/// A `\r` as the final buffered byte is held back un-consumed until the next
/// byte arrives, because the specification requires a following `\n` to be
/// treated as part of one `\r\n` terminator.
///
/// # Errors
///
/// Returns an error if the buffer holds more than [`MAX_FRAME_BYTES`] bytes
/// without a line terminator.
fn next_line(buffer: &[u8]) -> Result<Option<(String, usize)>, SseParseError> {
    let newline_position = buffer.iter().position(|byte| *byte == b'\n');
    let carriage_position = buffer.iter().position(|byte| *byte == b'\r');

    // A `\r` as the final buffered byte may pair with a `\n` arriving in the
    // next chunk, so its fate is deliberately left undecided here.
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

/// Formats one Server-Sent Events frame with the given optional event name and
/// data payload, including the terminating empty line.
///
/// The data payload must not contain newlines; multi-line payloads must be
/// passed as separate frames or joined by the caller before formatting.
///
/// # Panics
///
/// Panics in debug builds when the data payload contains a newline.
pub(crate) fn format_frame(event: Option<&str>, data: &str) -> String {
    debug_assert!(
        !data.contains('\n'),
        "SSE data payload must not contain newlines"
    );

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

#[cfg(test)]
mod tests {
    use super::{SseFrame, SseParseError, SseParser, format_data_frame, format_frame};

    /// Collects the frames produced by ingesting every chunk.
    ///
    /// # Panics
    ///
    /// Panics if ingestion or the final flush fails.
    fn parse_chunks(chunks: &[&[u8]]) -> Vec<SseFrame> {
        let mut parser = SseParser::new();

        let mut frames = Vec::new();

        for chunk in chunks {
            #[expect(clippy::expect_used, reason = "an ingestion failure fails the test")]
            frames.append(&mut parser.ingest(chunk).expect("ingest should not fail"));
        }

        #[expect(clippy::expect_used, reason = "a flush failure fails the test")]
        frames.append(&mut parser.finish().expect("finish should not fail"));

        frames
    }

    /// Parses single-line frames.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn parses_single_line_frames() {
        let frames = parse_chunks(&[b"data: {\"a\":1}\n\n"]);

        assert_eq!(
            frames,
            vec![SseFrame {
                event: None,
                data: "{\"a\":1}".to_string()
            }]
        );
    }

    /// Parses chunks split mid-frame.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn parses_chunks_split_mid_frame() {
        let frames = parse_chunks(&[b"data: hel", b"lo\n\ndata: wor", b"ld\n\n"]);

        assert_eq!(
            frames,
            vec![
                SseFrame {
                    event: None,
                    data: "hello".to_string()
                },
                SseFrame {
                    event: None,
                    data: "world".to_string()
                },
            ]
        );
    }

    /// Parses event names and multi-line data.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn parses_event_names_and_multi_line_data() {
        let frames = parse_chunks(&[b"event: error\ndata: line1\ndata: line2\n\n"]);

        assert_eq!(
            frames,
            vec![SseFrame {
                event: Some("error".to_string()),
                data: "line1\nline2".to_string()
            }]
        );
    }

    /// Verifies that `\r\n` and bare `\r` line terminators both parse.
    ///
    /// # Panics
    ///
    /// Panics if parsing does not yield the expected frames.
    #[test]
    fn accepts_crlf_and_bare_cr_terminators() {
        let frames = parse_chunks(&[b"data: a\r\n\r\ndata: b\r\r"]);

        assert_eq!(
            frames,
            vec![
                SseFrame {
                    event: None,
                    data: "a".to_string(),
                },
                SseFrame {
                    event: None,
                    data: "b".to_string(),
                },
            ]
        );
    }

    /// Verifies that a `\r\n` terminator whose `\n` arrives in the next chunk
    /// does not split the frame, so one multi-`data:`-line frame stays whole.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn parses_crlf_terminator_split_across_chunks() {
        let frames = parse_chunks(&[b"data: line1\r", b"\ndata: line2\n\n"]);

        assert_eq!(
            frames,
            vec![SseFrame {
                event: None,
                data: "line1\nline2".to_string(),
            }],
        );
    }

    /// Verifies that a lone trailing `\r` held back across the end of the
    /// stream still terminates the final frame at flush time.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn flushes_trailing_frame_terminated_by_bare_cr() {
        let frames = parse_chunks(&[b"data: tail\r"]);

        assert_eq!(
            frames,
            vec![SseFrame {
                event: None,
                data: "tail".to_string(),
            }],
        );
    }

    /// Verifies that a frame whose dispatching blank line arrives in a later
    /// chunk than its data lines is still emitted whole.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn dispatches_frame_with_blank_line_in_later_chunk() {
        let frames = parse_chunks(&[b"data: line1\r\n\r\ndata: x\n", b"\n"]);

        assert_eq!(
            frames,
            vec![
                SseFrame {
                    event: None,
                    data: "line1".to_string(),
                },
                SseFrame {
                    event: None,
                    data: "x".to_string(),
                },
            ],
        );
    }

    /// Verifies that comment, `id`, and `retry` fields are ignored and that
    /// data-less keep-alive frames never surface.
    ///
    /// # Panics
    ///
    /// Panics if parsing does not yield the expected frames.
    #[test]
    fn ignores_comments_keepalives_and_unknown_fields() {
        let frames = parse_chunks(&[b": ping\n\nid: 7\nretry: 100\ndata: x\n\n"]);

        assert_eq!(
            frames,
            vec![SseFrame {
                event: None,
                data: "x".to_string(),
            }]
        );
    }

    /// Verifies that a frame terminated by the end of the stream instead of
    /// a blank line is still flushed.
    ///
    /// # Panics
    ///
    /// Panics if parsing does not yield the expected frames.
    #[test]
    fn flushes_trailing_frame_without_final_newline() {
        let frames = parse_chunks(&[b"data: tail"]);

        assert_eq!(
            frames,
            vec![SseFrame {
                event: None,
                data: "tail".to_string(),
            }]
        );
    }

    /// Rejects oversized frames.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn rejects_oversized_frames() {
        let oversized_line = vec![b'x'; 17 * 1024 * 1024];

        let mut parser = SseParser::new();

        assert!(parser.ingest(&oversized_line).is_err());
    }

    /// Builds one chunk holding a single `data:` line of `payload_bytes`
    /// payload bytes.
    fn data_line_chunk(payload_bytes: usize) -> Vec<u8> {
        let mut chunk = b"data: ".to_vec();

        chunk.resize(chunk.len() + payload_bytes, b'x');
        chunk.push(b'\n');

        chunk
    }

    /// Rejects a frame assembled from many `data:` lines once its accumulated
    /// size crosses [`super::MAX_FRAME_BYTES`], even though every line and the
    /// ingest buffer individually stay under it, and latches the rejection so
    /// a later flush still fails.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn rejects_frame_accumulated_past_cap_across_data_lines() {
        let chunk = data_line_chunk(2 * 1024 * 1024);

        let mut parser = SseParser::new();

        // Seven two-mebibyte lines total under the cap; the eighth crosses it.
        for _ in 0..7 {
            assert_eq!(parser.ingest(&chunk), Ok(Vec::new()));
        }

        assert_eq!(parser.ingest(&chunk), Err(SseParseError::FrameTooLarge));

        assert_eq!(parser.finish(), Err(SseParseError::FrameTooLarge));
    }

    /// Dispatches consecutive multi-line frames, each staying under the cap,
    /// verifying the accumulated frame size resets at every dispatch so one
    /// frame's lines never count toward the next frame's cap.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn dispatches_consecutive_multi_line_frames_under_cap() {
        let payload = "x".repeat(5 * 1024 * 1024);

        let mut frame = b"data: ".to_vec();
        frame.extend_from_slice(payload.as_bytes());
        frame.push(b'\n');
        frame.extend_from_slice(b"data: ");
        frame.extend_from_slice(payload.as_bytes());
        frame.extend_from_slice(b"\n\n");

        let expected = SseFrame {
            event: None,
            data: format!("{payload}\n{payload}"),
        };

        assert_eq!(
            parse_chunks(&[frame.as_slice(), frame.as_slice()]),
            vec![expected.clone(), expected]
        );
    }

    /// Verifies that one leading UTF-8 BOM is stripped from the stream so the
    /// first `data` field parses, as the WHATWG SSE decoding step requires.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn strips_leading_utf8_bom() {
        let frames = parse_chunks(&[b"\xEF\xBB\xBFdata: {\"a\":1}\n\n"]);

        assert_eq!(
            frames,
            vec![SseFrame {
                event: None,
                data: "{\"a\":1}".to_string(),
            }]
        );
    }

    /// Verifies that a UTF-8 BOM split across chunk boundaries is still
    /// stripped exactly once.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn strips_utf8_bom_split_across_chunks() {
        let frames = parse_chunks(&[b"\xEF", b"\xBB", b"\xBFdata: x\n\n"]);

        assert_eq!(
            frames,
            vec![SseFrame {
                event: None,
                data: "x".to_string(),
            }]
        );
    }

    /// Verifies that a first line whose leading bytes merely resemble a BOM
    /// prefix is not stripped, so the corrupted field is dropped instead of
    /// shifting the first line's bytes.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn does_not_strip_a_bom_like_prefix() {
        let frames = parse_chunks(&[b"\xEF\xBBdata: x\n\n"]);

        assert!(frames.is_empty());
    }

    /// Verifies that frames are formatted with and without event names.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    #[test]
    fn formats_frames_with_and_without_event_names() {
        assert_eq!(format_data_frame("{}"), "data: {}\n\n");
        assert_eq!(
            format_frame(Some("error"), "x"),
            "event: error\ndata: x\n\n"
        );
    }
}
