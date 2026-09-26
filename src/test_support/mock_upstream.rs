//! Test support: the mock upstream HTTP transport — a tiny HTTP/1.1 server the engine tests
//! dispatch against to pin the exact requests the gateway renders and the transport-failure
//! shapes it maps onto client-facing outcomes.

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
pub(crate) enum MockAction {
    /// Writes the complete HTTP response — status line, headers, and body — verbatim, then
    /// closes.
    Respond(String),

    /// Answers nothing and closes: the transport-failure shape the gateway maps to 502.
    Close,

    /// Writes `head` verbatim, then repeats `frame` every `interval` until a write against the
    /// departed client fails.
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

/// The `METHOD /target` lines one [`MockUpstream`] has recorded, one entry per request in
/// arrival order.
pub(crate) type RecordedRequests = Arc<Mutex<Vec<String>>>;

/// A tiny HTTP/1.1 mock upstream the tests dispatch against: it reads every request head plus
/// the body the head declares, records the request's `METHOD /target` line, answers from its
/// response source, and shuts its accept loop down on drop.
pub(crate) struct MockUpstream {
    /// The socket address the mock answers at.
    address: SocketAddr,

    /// The shutdown signal stopping the mock's accept loop early; dropping the mock stops it
    /// too.
    pub(crate) shutdown: oneshot::Sender<()>,
}

impl MockUpstream {
    /// Serves `responses` round-robin: one entry per request, each framed as a 200 JSON
    /// response.
    ///
    /// # Panics
    ///
    /// Panics when the mock cannot bind, accept, or serve its fixture — a
    /// broken mock is the failure report.
    pub(crate) async fn start(responses: Vec<String>) -> Self {
        Self::start_recording(responses).await.0
    }

    /// Serves `responses` round-robin as complete HTTP responses — status line, headers, and
    /// body — sent verbatim: one entry per request, so a fixture controls the response status
    /// itself (a redirect fixture in particular).
    ///
    /// # Panics
    ///
    /// Panics when the mock cannot bind, accept, or serve its fixture — a
    /// broken mock is the failure report.
    pub(crate) async fn start_raw(responses: Vec<String>) -> Self {
        Self::start_serving(canned(responses)).await.0
    }

    /// Serves `responses` round-robin, each entry framed as a 200 JSON response, and records
    /// every request's `METHOD /target` line, so a test can pin the exact URLs the engine
    /// rendered.
    ///
    /// # Panics
    ///
    /// Panics when the mock cannot bind, accept, or serve its fixture — a
    /// broken mock is the failure report.
    pub(crate) async fn start_recording(responses: Vec<String>) -> (Self, RecordedRequests) {
        let framed = responses
            .into_iter()
            .map(|body| {
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
            })
            .collect();

        Self::start_serving(canned(framed)).await
    }

    /// Serves every request from `respond`, so a test flips the answer between scenarios while
    /// the mock stays up. The requests are recorded, but the recorder is dropped.
    ///
    /// # Panics
    ///
    /// Panics when the mock cannot bind, accept, or serve its fixture — a
    /// broken mock is the failure report.
    pub(crate) async fn start_responding(
        respond: impl Fn() -> MockAction + Send + Sync + 'static,
    ) -> Self {
        Self::start_serving(Arc::new(respond)).await.0
    }

    /// The `http://` base URL the mock answers at.
    pub(crate) fn base(&self) -> String {
        format!("http://{}", self.address)
    }

    /// Binds an ephemeral listener and serves every accepted connection: the request head plus
    /// its declared body is read, the `METHOD /target` line is recorded, and `respond` picks the
    /// transport action. The accept loop ends when the shutdown signal fires — on drop or on an
    /// explicit send.
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

        let seen: RecordedRequests = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&seen);

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
                let seen = Arc::clone(&recorded);

                tokio::spawn(async move {
                    let Some(request_head) = read_request(&mut socket).await else {
                        return;
                    };

                    record_request(&request_head, &seen);

                    serve_action(&mut socket, respond()).await;
                });
            }
        });

        (Self { address, shutdown }, seen)
    }
}

/// The round-robin canned response source: a single entry serves every request, a multi-entry
/// fixture walks its entries in order, and an empty fixture is the test bug the panic reports.
///
/// # Panics
///
/// Panics if the mutex is poisoned — a prior holder panicked while holding the lock — or if the
/// fixture is empty.
fn canned(responses: Vec<String>) -> Arc<dyn Fn() -> MockAction + Send + Sync> {
    let responses = Arc::new(Mutex::new(responses));

    Arc::new(move || {
        #[expect(
            clippy::unwrap_used,
            reason = "a mutex poisoned by a panicking sibling request is the failure report"
        )]
        let mut guard = responses.lock().unwrap();

        #[expect(
            clippy::indexing_slicing,
            reason = "an empty fixture is a test bug the panic reports"
        )]
        let next = if guard.len() > 1 {
            guard.remove(0)
        } else {
            guard[0].clone()
        };

        MockAction::Respond(next)
    })
}

/// Reads one HTTP/1.1 request head — through its blank line — plus the `Content-Length` body
/// the head declares, returning the head text; `None` when the client left before a head
/// arrived. Read failures count as the client's departure.
async fn read_request(stream: &mut TcpStream) -> Option<String> {
    let mut collected: Vec<u8> = Vec::new();

    let head_end = loop {
        let read = stream.read_buf(&mut collected).await.unwrap_or(0);

        if read == 0 {
            return None;
        }

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
        let read = stream.read_buf(&mut collected).await.unwrap_or(0);

        if read == 0 {
            break;
        }

        body_read += read;
    }

    Some(head)
}

/// Records the request line's `METHOD /target` pair; a head without a well-formed request line
/// is answered but not recorded.
///
/// # Panics
///
/// Panics if the mutex is poisoned — a prior holder panicked while holding the lock.
fn record_request(head: &str, seen: &RecordedRequests) {
    if let Some(request_line) = head.lines().next() {
        let mut parts = request_line.split_whitespace();

        if let (Some(method), Some(target)) = (parts.next(), parts.next()) {
            #[expect(
                clippy::unwrap_used,
                reason = "a mutex poisoned by a panicking sibling request is the failure report"
            )]
            seen.lock().unwrap().push(format!("{method} {target}"));
        }
    }
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
