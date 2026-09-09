//! Server-Sent Events frame parsing and writing.

/// Maximum number of bytes a single SSE frame may occupy before the parser
/// rejects the stream as malformed. Upstream payloads are JSON documents, so
/// this bound is far above any legitimate frame while preventing unbounded
/// buffer growth from a hostile stream.
pub(crate) const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

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
/// eventless keep-alive frames never surface.
#[derive(Debug, Default)]
pub(crate) struct SseParser {
    /// Bytes received but not yet consumed as complete lines.
    buffer: Vec<u8>,

    /// Whether at least one frame was malformed beyond [`MAX_FRAME_BYTES`].
    is_oversized: bool,
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
    fn drain_complete_frames(&mut self) -> Result<Vec<SseFrame>, SseParseError> {
        let mut frames = Vec::new();
        let mut event_name: Option<String> = None;
        let mut data_lines: Vec<String> = Vec::new();

        while let Some((line, consumed)) = next_line(&self.buffer)? {
            self.buffer.drain(..consumed);

            if line.is_empty() {
                if !data_lines.is_empty() {
                    frames.push(SseFrame {
                        event: event_name.take(),
                        data: data_lines.join("\n"),
                    });

                    data_lines = Vec::new();
                }

                event_name = None;

                continue;
            }

            let (field, value) = match line.split_once(':') {
                Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
                None => (line.as_str(), ""),
            };

            match field {
                "event" => event_name = Some(value.to_string()),
                "data" => data_lines.push(value.to_string()),
                _ => {}
            }
        }

        Ok(frames)
    }
}

/// Malformed Server-Sent Events stream errors.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(crate) enum SseParseError {
    /// A single frame exceeded the maximum accepted frame size.
    #[error("SSE frame exceeds the maximum accepted size")]
    FrameTooLarge,
}

/// Extracts the next complete line from the buffer, returning the line without
/// its terminator and the number of bytes consumed.
///
/// Accepts `\n`, `\r\n`, and lone `\r` terminators per the SSE specification.
///
/// # Errors
///
/// Returns an error if the buffer holds more than [`MAX_FRAME_BYTES`] bytes
/// without a line terminator.
fn next_line(buffer: &[u8]) -> Result<Option<(String, usize)>, SseParseError> {
    let newline_position = buffer.iter().position(|byte| *byte == b'\n');
    let carriage_position = buffer.iter().position(|byte| *byte == b'\r');

    let (end, terminator_length) = match (newline_position, carriage_position) {
        (Some(newline), Some(carriage)) if carriage < newline => {
            let is_carriage_newline = newline == carriage + 1;

            (carriage, if is_carriage_newline { 2 } else { 1 })
        }

        (Some(newline), _) => (newline, 1),

        (None, Some(carriage)) => (carriage, 1),

        (None, None) => {
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
    use super::{SseFrame, SseParser, format_data_frame, format_frame};

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

    /// Verifies that parses single line frames holds.
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

    /// Verifies that parses chunks split mid frame holds.
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

    /// Verifies that parses event names and multi line data holds.
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

    /// Verifies that rejects oversized frames holds.
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

    /// Verifies that formats frames with and without event names holds.
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
