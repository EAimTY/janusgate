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
#[derive(Debug, Clone)]
pub(crate) enum MockAction {
    /// Writes the complete HTTP response — status line, headers, and body — verbatim, then
    /// closes.
    Respond(String),

    /// Answers nothing and closes: the transport-failure shape the gateway maps to 502.
    Close,

    /// Waits for an explicit fixture release before writing a complete response; an empty
    /// response closes the connection without answering.
    RespondAfterSignal {
        /// Complete HTTP response, including its status line and body.
        response: String,
        /// Barrier retaining a notification even when the fixture releases before this wait.
        release: Arc<tokio::sync::Notify>,
    },

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

impl MockAction {
    /// Frames one JSON body as the complete 200 response the framed fixtures carry.
    #[must_use]
    pub(crate) fn respond_json(body: String) -> Self {
        Self::Respond(format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ))
    }
}

/// The `METHOD /target` lines one [`MockUpstream`] has recorded, one entry per request in
/// arrival order.
pub(crate) type RecordedRequests = Arc<Mutex<Vec<String>>>;

/// One recorded request of a body-capturing [`MockUpstream`]: the request line's method and
/// target, the carried headers in arrival order, and the full `Content-Length` request body.
#[derive(Debug, Clone)]
pub(crate) struct RecordedHttpRequest {
    /// The request method.
    pub(crate) method: String,

    /// The request target (`/path?query`).
    pub(crate) target: String,

    /// The carried headers as `(name, value)` pairs in arrival order, names as sent.
    pub(crate) headers: Vec<(String, String)>,

    /// The full request body the head's `Content-Length` declared.
    pub(crate) body: String,
}

impl RecordedHttpRequest {
    /// The request's `METHOD /target` line — the spelling the line-only recorder carries.
    #[must_use]
    pub(crate) fn line(&self) -> String {
        format!("{} {}", self.method, self.target)
    }

    /// The first header value whose name matches `name` case-insensitively, or `None`.
    #[must_use]
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header_name, _)| header_name.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// The full recorded requests of a body-capturing [`MockUpstream`], one entry per request in
/// arrival order.
pub(crate) type RecordedHttpRequests = Arc<Mutex<Vec<RecordedHttpRequest>>>;

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
        Self::start_serving(canned(responses), None).await.0
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
        let scripted = responses
            .into_iter()
            .map(MockAction::respond_json)
            .collect();

        Self::start_serving(script(scripted), None).await
    }

    /// Serves `responses` round-robin as complete HTTP responses — status line, headers, and
    /// body — sent verbatim, one entry per request, and records every request's `METHOD /target`
    /// line, so a fixture controls the response status itself while the test pins the rendered
    /// URLs and call order.
    ///
    /// # Panics
    ///
    /// Panics when the mock cannot bind, accept, or serve its fixture — a
    /// broken mock is the failure report.
    pub(crate) async fn start_raw_recording(responses: Vec<String>) -> (Self, RecordedRequests) {
        Self::start_serving(canned(responses), None).await
    }

    /// Serves one [`MockAction`] per request in order — the last action repeats once the script
    /// is exhausted — and records every request in full: method, target, headers, and body. This
    /// is the body-capturing recording mode the acceptance tests pin dispatch counts, call
    /// order, and request bodies from (README §6.8).
    ///
    /// # Panics
    ///
    /// Panics when the mock cannot bind, accept, or serve its fixture — a
    /// broken mock is the failure report.
    pub(crate) async fn start_scripted_recording_bodies(
        actions: Vec<MockAction>,
    ) -> (Self, RecordedHttpRequests) {
        let recorded = Arc::new(Mutex::new(Vec::new()));

        let (mock, _lines) =
            Self::start_serving(script(actions), Some(Arc::clone(&recorded))).await;

        (mock, recorded)
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
        Self::start_serving(Arc::new(respond), None).await.0
    }

    /// The `http://` base URL the mock answers at.
    pub(crate) fn base(&self) -> String {
        format!("http://{}", self.address)
    }

    /// Binds an ephemeral listener and serves every accepted connection: the request head plus
    /// its declared body is read, the `METHOD /target` line is recorded — beside the full
    /// request when `bodies` carries a recorder — and `respond` picks the transport action. The
    /// accept loop ends when the shutdown signal fires — on drop or on an explicit send.
    ///
    /// # Panics
    ///
    /// Panics when the mock cannot bind or address its listener — a broken mock is the failure
    /// report.
    async fn start_serving(
        respond: Arc<dyn Fn() -> MockAction + Send + Sync>,
        bodies: Option<RecordedHttpRequests>,
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
                let bodies = bodies.clone();

                tokio::spawn(async move {
                    let Some((request_head, request_body)) = read_request(&mut socket).await else {
                        return;
                    };

                    record_request(&request_head, request_body, &seen, bodies.as_ref());

                    serve_action(&mut socket, respond()).await;
                });
            }
        });

        (Self { address, shutdown }, seen)
    }
}

/// The scripted action source: one action per request in order, and the last action repeats
/// once the script is exhausted — so a test seeing the last action again has dispatched past
/// the script's end, the signal its dispatch-count pins read. An empty script is the test bug
/// the panic reports.
///
/// # Panics
///
/// Panics if the mutex is poisoned — a prior holder panicked while holding the lock — or if the
/// script is empty.
fn script(actions: Vec<MockAction>) -> Arc<dyn Fn() -> MockAction + Send + Sync> {
    let actions = Arc::new(Mutex::new(actions));

    Arc::new(move || {
        #[expect(
            clippy::unwrap_used,
            reason = "a mutex poisoned by a panicking sibling request is the failure report"
        )]
        let mut guard = actions.lock().unwrap();

        if guard.len() > 1 {
            guard.remove(0)
        } else {
            #[expect(
                clippy::indexing_slicing,
                reason = "an empty script is a test bug the panic reports"
            )]
            guard[0].clone()
        }
    })
}

/// The round-robin canned response source: every entry — already a complete HTTP response —
/// served verbatim through [`script`]'s advance rule.
fn canned(responses: Vec<String>) -> Arc<dyn Fn() -> MockAction + Send + Sync> {
    script(responses.into_iter().map(MockAction::Respond).collect())
}

/// Reads one HTTP/1.1 request head — through its blank line — plus the `Content-Length` body
/// the head declares, returning the head text beside the body text; `None` when the client left
/// before a head arrived. Read failures count as the client's departure.
async fn read_request(stream: &mut TcpStream) -> Option<(String, String)> {
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

    #[expect(
        clippy::indexing_slicing,
        reason = "body_read never exceeds the buffer the reads filled"
    )]
    let body = String::from_utf8_lossy(&collected[head_end..]).into_owned();

    Some((head, body))
}

/// Records the request line's `METHOD /target` pair, and the full request — method, target,
/// headers, and body — when a body recorder is present. A head without a well-formed request
/// line is answered but not recorded.
///
/// # Panics
///
/// Panics if a mutex is poisoned — a prior holder panicked while holding the lock.
fn record_request(
    head: &str,
    body: String,
    seen: &RecordedRequests,
    bodies: Option<&RecordedHttpRequests>,
) {
    let Some(request_line) = head.lines().next() else {
        return;
    };

    let mut parts = request_line.split_whitespace();

    let Some((method, target)) = parts.next().zip(parts.next()) else {
        return;
    };

    #[expect(
        clippy::unwrap_used,
        reason = "a mutex poisoned by a panicking sibling request is the failure report"
    )]
    seen.lock().unwrap().push(format!("{method} {target}"));

    if let Some(bodies) = bodies {
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
        bodies.lock().unwrap().push(RecordedHttpRequest {
            method: method.to_string(),
            target: target.to_string(),
            headers,
            body,
        });
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

        MockAction::RespondAfterSignal { response, release } => {
            release.notified().await;
            let _ = socket.write_all(response.as_bytes()).await;
        }

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
