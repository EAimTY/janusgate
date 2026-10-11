//! Test support: the mock upstream HTTP transport — a minimal HTTP/1.1 server test families
//! dispatch against to pin the exact requests the gateway renders and the transport-failure and
//! streaming shapes it maps onto client-facing outcomes.

use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::{TcpListener, TcpStream},
    sync::oneshot,
};

/// The transport action one [`MockUpstream`] connection takes once its request has arrived.
#[derive(Debug, Clone)]
pub(crate) enum MockAction {
    /// Writes the complete HTTP response — status line, headers, and body — verbatim, then
    /// closes.
    Respond(String),

    /// Answers nothing and closes: the transport-failure shape the gateway maps onto its
    /// upstream-unavailable outcome.
    Close,

    /// Writes `head` verbatim, then repeats `frame` every `interval` until a write against the
    /// departed client fails — the unbounded stream a client disconnects from mid-flight.
    StreamForever {
        /// The response head written once.
        head: String,

        /// The frame written repeatedly.
        frame: String,

        /// The pause between two frames.
        interval: Duration,
    },

    /// Writes `head` verbatim, waits `delay`, writes `frame` once, and closes — so a client can
    /// leave before any frame was enqueued.
    StreamAfterDelay {
        /// The response head written once.
        head: String,

        /// The single frame written after the delay.
        frame: String,

        /// The pause between the head and the frame.
        delay: Duration,
    },
}

impl MockAction {
    /// Frames one JSON body as the complete 200 JSON response the scripted fixtures carry.
    #[must_use]
    pub(crate) fn json(body: String) -> Self {
        Self::Respond(format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ))
    }
}

/// One recorded request: the request line's method and target, the carried headers in arrival
/// order, and the full `Content-Length` request body.
#[derive(Debug, Clone)]
pub(crate) struct RecordedRequest {
    /// The request method.
    pub(crate) method: String,

    /// The request target (`/path?query`).
    pub(crate) target: String,

    /// The carried headers as `(name, value)` pairs in arrival order, names as sent.
    pub(crate) headers: Vec<(String, String)>,

    /// The full request body the head's `Content-Length` declared.
    pub(crate) body: String,
}

impl RecordedRequest {
    /// The first header value whose name matches `name` case-insensitively, or `None`.
    #[must_use]
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header_name, _)| header_name.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// The recorded requests of a [`MockUpstream`], one entry per request in arrival order.
pub(crate) type RecordedRequests = Arc<Mutex<Vec<RecordedRequest>>>;

/// A minimal HTTP/1.1 mock upstream the tests dispatch against: it reads every request head
/// plus the body the head declares, records the request, answers from its configured action
/// source, and shuts its accept loop down on drop or on an explicit shutdown send.
pub(crate) struct MockUpstream {
    /// The socket address the mock answers at.
    address: SocketAddr,

    /// The shutdown signal stopping the mock's accept loop early; dropping the mock stops it
    /// too. Never read directly — holding the sending half is what makes the loop's shutdown
    /// receive resolve when the mock is dropped — and no test signals it explicitly.
    #[expect(
        dead_code,
        reason = "the held sender is the drop-driven shutdown; no test signals it explicitly"
    )]
    pub(crate) shutdown: oneshot::Sender<()>,
}

impl MockUpstream {
    /// Serves one JSON-framed body per request in order and records every request in full. The
    /// last body repeats once the script is exhausted, so a test receiving its final response
    /// again has dispatched past the script's end — the signal its dispatch-count pins read.
    ///
    /// # Panics
    ///
    /// Panics when the mock cannot bind, accept, or serve its fixture — a broken mock is the
    /// failure report — and when `bodies` is empty, which is a test bug.
    pub(crate) async fn start_json(bodies: Vec<String>) -> (Self, RecordedRequests) {
        assert!(!bodies.is_empty(), "an empty mock script is a test bug");

        Self::start(bodies.into_iter().map(MockAction::json).collect()).await
    }

    /// Serves one complete HTTP response — status line, headers, and body — verbatim per request
    /// in order, and records every request in full, so a fixture controls the response status
    /// itself while the test pins the rendered requests. The last response repeats once the
    /// script is exhausted.
    ///
    /// # Panics
    ///
    /// Panics as [`MockUpstream::start_json`] documents.
    pub(crate) async fn start_raw(responses: Vec<String>) -> (Self, RecordedRequests) {
        assert!(!responses.is_empty(), "an empty mock script is a test bug");

        Self::start(responses.into_iter().map(MockAction::Respond).collect()).await
    }

    /// Serves every request from `action`, so a test flips the answer between scenarios while
    /// the mock stays up, and records every request in full.
    ///
    /// # Panics
    ///
    /// Panics as [`MockUpstream::start_json`] documents.
    pub(crate) async fn start_answering(
        action: impl Fn() -> MockAction + Send + Sync + 'static,
    ) -> (Self, RecordedRequests) {
        Self::start_serving(Arc::new(action)).await
    }

    /// Serves one [`MockAction`] per request in order — the last action repeats once the script
    /// is exhausted — and records every request in full.
    ///
    /// # Panics
    ///
    /// Panics as [`MockUpstream::start_json`] documents.
    pub(crate) async fn start(actions: Vec<MockAction>) -> (Self, RecordedRequests) {
        assert!(!actions.is_empty(), "an empty mock script is a test bug");

        let served = std::sync::atomic::AtomicUsize::new(0);

        Self::start_serving(Arc::new(move || {
            // The last action repeats past the script's end; the clamp keeps the index within
            // the non-empty script.
            let index = served
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                .min(actions.len() - 1);

            #[expect(
                clippy::indexing_slicing,
                reason = "the index is clamped below the non-empty script length"
            )]
            actions[index].clone()
        }))
        .await
    }

    /// The `http://` base URL the mock answers at.
    #[must_use]
    pub(crate) fn base(&self) -> String {
        format!("http://{}", self.address)
    }

    /// Binds an ephemeral listener and serves every accepted connection: the request head plus
    /// its declared body is read, the full request is recorded, and `respond` picks the served
    /// action. The accept loop ends when the shutdown signal fires — on drop or on an explicit
    /// send.
    ///
    /// # Panics
    ///
    /// Panics when the mock cannot bind or address its listener — a broken mock is the failure
    /// report.
    async fn start_serving(
        respond: Arc<dyn Fn() -> MockAction + Send + Sync>,
    ) -> (Self, RecordedRequests) {
        #[expect(
            clippy::unwrap_used,
            reason = "a mock that cannot bind is the failure report"
        )]
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();

        #[expect(clippy::unwrap_used, reason = "a bound listener reports its address")]
        let address = listener.local_addr().unwrap();

        let (shutdown, mut shutdown_received) = oneshot::channel::<()>();
        let requests: RecordedRequests = Arc::default();
        let serving_requests = Arc::clone(&requests);

        tokio::spawn(async move {
            loop {
                let accepted = tokio::select! {
                    _ = &mut shutdown_received => break,
                    accepted = listener.accept() => accepted,
                };

                let Ok((mut socket, _)) = accepted else {
                    break;
                };

                let respond = Arc::clone(&respond);
                let requests = Arc::clone(&serving_requests);

                tokio::spawn(async move {
                    let Some((head, body)) = read_request(&mut socket).await else {
                        return;
                    };

                    record_request(&head, body, &requests);

                    serve_action(&mut socket, respond()).await;
                });
            }
        });

        (Self { address, shutdown }, requests)
    }
}

/// Reads one HTTP/1.1 request head — through its blank line — plus the `Content-Length` body the
/// head declares, returning the head text beside the body text. `None` when the client left
/// before a head arrived; read failures count as the client's departure.
async fn read_request(stream: &mut TcpStream) -> Option<(String, String)> {
    let mut collected: Vec<u8> = Vec::new();

    let head_end = loop {
        let mut buffer = [0_u8; 4096];

        let read = stream.read(&mut buffer).await.unwrap_or(0);

        if read == 0 {
            return None;
        }

        #[expect(
            clippy::indexing_slicing,
            reason = "read returns the number of bytes filled into the buffer"
        )]
        collected.extend_from_slice(&buffer[..read]);

        if let Some(position) = collected
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
        {
            break position + 4;
        }
    };

    #[expect(
        clippy::indexing_slicing,
        reason = "head_end is an in-bounds offset into the same buffer"
    )]
    let head = String::from_utf8_lossy(&collected[..head_end]).into_owned();

    let body_length = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;

            if name.eq_ignore_ascii_case("content-length") {
                value.trim().parse::<usize>().ok()
            } else {
                None
            }
        })
        .unwrap_or(0);

    let mut body_read = collected.len() - head_end;

    while body_read < body_length {
        let mut buffer = [0_u8; 4096];

        let read = stream.read(&mut buffer).await.unwrap_or(0);

        if read == 0 {
            break;
        }

        #[expect(
            clippy::indexing_slicing,
            reason = "read returns the number of bytes filled into the buffer"
        )]
        collected.extend_from_slice(&buffer[..read]);

        body_read += read;
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "body_read never exceeds the buffer the reads filled"
    )]
    let body = String::from_utf8_lossy(&collected[head_end..]).into_owned();

    Some((head, body))
}

/// Records one request in full: method, target, headers, and body. A head without a well-formed
/// request line is answered but not recorded.
///
/// # Panics
///
/// Panics if the mutex is poisoned — a prior holder panicked while holding the lock.
fn record_request(head: &str, body: String, recorded: &RecordedRequests) -> Option<()> {
    let mut parts = head.lines().next()?.split_whitespace();
    let (method, target) = parts.next().zip(parts.next())?;

    let headers = head
        .lines()
        .skip(1)
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;

            Some((name.trim().to_string(), value.trim().to_string()))
        })
        .collect();

    #[expect(
        clippy::unwrap_used,
        reason = "a mutex poisoned by a panicking sibling request is the failure report"
    )]
    recorded.lock().unwrap().push(RecordedRequest {
        method: method.to_string(),
        target: target.to_string(),
        headers,
        body,
    });

    Some(())
}

/// Carries one connection's [`MockAction`] out against `socket`: write failures — a client that
/// already left — end the action where a write is the only exit, and are ignored elsewhere,
/// exactly like a close.
async fn serve_action(socket: &mut TcpStream, action: MockAction) {
    match action {
        MockAction::Respond(response) => {
            let _ = socket.write_all(response.as_bytes()).await;
        }

        MockAction::Close => {}

        MockAction::StreamForever {
            head,
            frame,
            interval,
        } => {
            if socket.write_all(head.as_bytes()).await.is_ok() {
                loop {
                    if socket.write_all(frame.as_bytes()).await.is_err() {
                        break;
                    }

                    tokio::time::sleep(interval).await;
                }
            }
        }

        MockAction::StreamAfterDelay { head, frame, delay } => {
            if socket.write_all(head.as_bytes()).await.is_ok() {
                tokio::time::sleep(delay).await;

                let _ = socket.write_all(frame.as_bytes()).await;
            }
        }
    }
}
