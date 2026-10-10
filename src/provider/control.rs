//! The shared control-request executor: one engine drives the OAuth device authorization, token
//! exchange, refresh, provisioning, polling, and discovery requests (`docs/design/providers.md`).
//!
//! The executor evaluates the request's string expressions against the stage's [`ValueEnv`],
//! sets `Content-Type` from the body variant (every other header is an explicit row), sends, and
//! decodes the response through its declared encoding and field mappings.

use crate::{
    http_body,
    provider::definition::{
        eval::{EvalError, ValueEnv},
        provisioning::ProvisionExtraction,
        template::{
            ControlBody, ControlMethod, ControlRequest, ExtractionShape, ResponseEncoding,
            ResponseField, ResponsePath, ResponseTarget,
        },
    },
};
use reqwest::{Client, Method, RequestBuilder};
use serde_json::Value as Json;
use std::collections::BTreeMap;

/// A decoded control response body: the declared encoding's value tree.
#[derive(Debug, Clone)]
pub(super) enum DecodedBody {
    /// A JSON document.
    Json(Json),

    /// A `application/x-www-form-urlencoded` body of flat string fields.
    Form(BTreeMap<String, String>),
}

/// One extracted response value: the declared shapes render into text or integer.
#[derive(Debug, Clone)]
pub(super) enum Extracted {
    /// A string value.
    Text(String),

    /// An integer value.
    Integer(i64),
}

impl Extracted {
    /// The value as text; an integer renders in its decimal form.
    #[must_use]
    pub(super) fn as_text(&self) -> String {
        match self {
            Self::Text(text) => text.clone(),
            Self::Integer(value) => value.to_string(),
        }
    }

    /// The value as an integer; a text value is `None`.
    #[must_use]
    pub(super) const fn as_integer(&self) -> Option<i64> {
        match self {
            Self::Integer(value) => Some(*value),
            Self::Text(_) => None,
        }
    }
}

/// A control-stage failure: a rejection carrying the upstream status and the full bounded body —
/// so the token-grant classification can inspect error codes before truncation — or an
/// unavailable stage (transport failure, undecodable body, a failed required extraction), with
/// the transport timeout carried as its own variant so consumers can match the fact typed
/// instead of parsing the rendered message.
#[derive(Debug, thiserror::Error)]
pub(super) enum ControlFailure {
    /// The upstream answered a non-2xx status.
    #[error("upstream rejected with status {status}")]
    Rejected {
        /// Upstream status code.
        status: u16,

        /// The full bounded error body, not yet truncated into an excerpt.
        body: String,
    },

    /// The stage failed without a usable response: a transport failure, a malformed body, or a
    /// failed required extraction.
    #[error("{detail}")]
    Unavailable {
        /// Human-readable failure detail.
        detail: String,
    },

    /// The stage failed on a transport timeout: the request was cut off by the HTTP client's
    /// timeout budget and produced no usable response. The device poller's backoff schedules
    /// this failure class differently from every other unavailable failure.
    #[error("{detail}")]
    Timeout {
        /// Human-readable failure detail.
        detail: String,
    },
}

impl From<EvalError> for ControlFailure {
    fn from(error: EvalError) -> Self {
        Self::Unavailable {
            detail: error.to_string(),
        }
    }
}

/// Percent-decodes one `application/x-www-form-urlencoded` component: `+` renders as a space and
/// `%XX` sequences render as their bytes; an invalid escape renders its `%` verbatim and the walk
/// continues after it.
fn form_decode(component: &str) -> String {
    /// One decode step: the bytes to consume and the byte to push.
    enum Step {
        /// Push one byte.
        Push(u8),

        /// Skip the escaped bytes and push the decoded one.
        Skip(u8, usize),
    }

    let bytes = component.as_bytes();

    let mut decoded = Vec::with_capacity(bytes.len());

    let mut index = 0;

    while let Some(&byte) = bytes.get(index) {
        let step = match byte {
            b'+' => Step::Push(b' '),

            b'%' => match bytes.get(index + 1..index + 3) {
                // Both escape bytes are decoded as hexadecimal nibbles, symmetric to
                // `hex_digit`'s encoding: `u8::from_str_radix` accepts a leading `+`, which
                // would decode a `%+5` escape as the byte `0x05` instead of rendering its
                // `%` verbatim.
                Some([high, low]) => match (hex_value(*high), hex_value(*low)) {
                    (Some(high_nibble), Some(low_nibble)) => {
                        Step::Skip((high_nibble << 4) | low_nibble, 3)
                    }

                    _ => Step::Push(b'%'),
                },

                _ => Step::Push(b'%'),
            },

            byte => Step::Push(byte),
        };

        match step {
            Step::Push(push) => {
                decoded.push(push);
                index += 1;
            }

            Step::Skip(push, consumed) => {
                decoded.push(push);
                index += consumed;
            }
        }
    }

    String::from_utf8_lossy(&decoded).into_owned()
}

/// Parses an `application/x-www-form-urlencoded` body into its flat field map; a repeated field
/// keeps its last value, matching how every consumer here reads them.
#[must_use]
pub(super) fn parse_form(body: &str) -> BTreeMap<String, String> {
    body.split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((name, value)) => (form_decode(name), form_decode(value)),
            None => (form_decode(pair), String::new()),
        })
        .collect()
}

/// The uppercase hexadecimal digit of one nibble (RFC 3986 renders percent escapes with the
/// uppercase alphabet).
fn hex_digit(nibble: u8) -> char {
    match nibble {
        0..=9 => char::from(b'0' + nibble),
        _ => char::from(b'A' + (nibble - 10)),
    }
}

/// The numeric value of one ASCII hexadecimal digit byte, in either alphabet, or `None` for
/// any other byte. The decode-side twin of [`hex_digit`].
fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),

        _ => None,
    }
}

/// Percent-encodes one URL query component with the RFC 3986 unreserved set: every byte outside
/// `A–Z a–z 0–9 - . _ ~` renders as `%XX`, so a space renders `%20` and a literal plus `%2B`.
/// This strict encoding is the one vocabulary every descriptor-rendered query component uses —
/// the authorize-URL builder (`docs/design/providers.md`) and the control-request query rows built
/// here alike — because RFC 3986 §3.4 defines `+` in a query as a literal plus. The
/// `application/x-www-form-urlencoded` body variant keeps its own `+`-for-space rule, which is
/// the content type's, not the URL query's.
#[must_use]
pub(super) fn percent_encode_component(component: &str) -> String {
    let mut encoded = String::with_capacity(component.len());

    for byte in component.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(char::from(byte));
            }

            other => {
                encoded.push('%');
                encoded.push(hex_digit(other >> 4));
                encoded.push(hex_digit(other & 0x0f));
            }
        }
    }

    encoded
}

/// Appends one pre-rendered query row — `name=value` — to an already-evaluated URL string. The
/// `?`/`&` join lands in the URL's query component, and a `#` fragment, when the URL carries
/// one, rides again after the row: RFC 3986 §3 puts the query before the fragment, and an
/// unencoded `#` can never appear inside a query, so the first `#` is always the fragment
/// delimiter and a row appended blindly at the end would fold into the fragment and silently
/// drop from the request.
///
/// The row arrives exactly as it must read — the join is this helper's whole contract, so the
/// percent-encoding stays the caller's: the strict RFC 3986 unreserved-set encoder
/// ([`percent_encode_component`]) runs upstream of every descriptor-rendered component.
pub(super) fn append_query_row(url: &mut String, name: &str, value: &str) {
    let fragment_start = url.find('#').unwrap_or(url.len());

    let has_query = url[..fragment_start].contains('?');

    let mut row = String::with_capacity(1 + name.len() + 1 + value.len());

    row.push_str(if has_query { "&" } else { "?" });
    row.push_str(name);
    row.push('=');
    row.push_str(value);

    url.insert_str(fragment_start, &row);
}

/// Builds the HTTP request for one control request: the evaluated URL, query, headers, and body.
/// The executor sets `Content-Type` from the body variant; every other header is an explicit row.
///
/// Query rows render through [`percent_encode_component`] and are appended pre-encoded, so the
/// client serializes nothing again — a space stays `%20` instead of the form-urlencoded `+`.
/// The rows join through [`append_query_row`], which keeps them in the query component ahead of
/// a `#` fragment (RFC 3986 §3) so pagination and request parameters never fold into it.
///
/// # Errors
///
/// Returns [`ControlFailure::Unavailable`] when an expression references a value the stage does
/// not carry. A `GET` request never carries a body — the definition write boundary rejects that
/// configuration outright (the "a GET control request never carries a body" validation) — so no
/// runtime method check exists or is needed here.
pub(super) fn build_request(
    client: &Client,
    request: &ControlRequest,
    env: &ValueEnv<'_>,
    what: &str,
) -> Result<RequestBuilder, ControlFailure> {
    let mut url = env.evaluate(&request.url, what)?;

    let method = match request.method {
        ControlMethod::Get => Method::GET,
        ControlMethod::Post => Method::POST,
    };

    for row in &request.query {
        append_query_row(
            &mut url,
            &percent_encode_component(&row.name),
            &percent_encode_component(&env.evaluate(&row.value, what)?),
        );
    }

    let mut builder = client.request(method, &url);

    for row in &request.headers {
        builder = builder.header(row.name.as_str(), env.evaluate(&row.value, what)?.as_str());
    }

    match &request.body {
        None => {}

        Some(ControlBody::Form { fields }) => {
            let mut form: Vec<(String, String)> = Vec::with_capacity(fields.len());

            for row in fields {
                form.push((row.name.clone(), env.evaluate(&row.value, what)?));
            }

            builder = builder.form(&form);
        }

        Some(ControlBody::Json { node }) => {
            let body = env.render_json(node, &(), what)?;

            builder = builder.json(&body);
        }
    }

    Ok(builder)
}

/// Classifies one transport failure of the HTTP client into its control failure, carrying the
/// `reqwest::Error::is_timeout` fact as the [`ControlFailure::Timeout`] variant instead of
/// leaving consumers to parse the rendered detail.
fn transport_failure(detail: String, error: &reqwest::Error) -> ControlFailure {
    if error.is_timeout() {
        ControlFailure::Timeout { detail }
    } else {
        ControlFailure::Unavailable { detail }
    }
}

/// Executes one control request and decodes its 2xx response through the declared encoding.
///
/// The bounded-body policy is the shared one: a non-2xx body is read through
/// [`http_body::read_error_body`] and carried untruncated in [`ControlFailure::Rejected`], so the
/// token-grant classification can inspect error codes before any excerpt is rendered, and a 2xx
/// body is read through [`http_body::read_success_body`] — the provisioning poll loop repeats
/// this read, so a hostile upstream can never grow gateway memory without limit.
///
/// # Errors
///
/// Returns [`ControlFailure`] for transport failures, a success body beyond the bounded size,
/// non-2xx statuses, and bodies that do not decode through the declared encoding.
pub(super) async fn execute(
    client: &Client,
    request: &ControlRequest,
    env: &ValueEnv<'_>,
    encoding: ResponseEncoding,
    what: &str,
) -> Result<DecodedBody, ControlFailure> {
    let builder = build_request(client, request, env, what)?;

    let response = builder.send().await.map_err(|error| {
        transport_failure(format!("{what} upstream unreachable: {error}"), &error)
    })?;

    let status = response.status();

    if !status.is_success() {
        return Err(ControlFailure::Rejected {
            status: status.as_u16(),
            body: http_body::read_error_body(response).await,
        });
    }

    let bytes = http_body::read_success_body(response)
        .await
        .map_err(|error| match error {
            http_body::SuccessBodyError::Oversized => ControlFailure::Unavailable {
                detail: format!("{what} response body exceeds the success-body bound"),
            },

            // `Read` wraps the reqwest error, so the timeout fact stays decidable here and the
            // device poller's backoff keeps scheduling body-read timeouts as timeouts.
            http_body::SuccessBodyError::Read(error) => {
                transport_failure(format!("{what} response read failed: {error}"), &error)
            }
        })?;

    decode_body(&bytes, encoding, what)
}

/// Decodes the response body through an explicit declared encoding.
///
/// Both control encodings — JSON and form — are UTF-8/ASCII by spec, so the body decodes
/// strictly as UTF-8: the charset negotiation `text()` would apply has no conformant control
/// stage, and a lossy rendering would only smuggle a malformed body into the decoders.
///
/// # Errors
///
/// Returns [`ControlFailure::Unavailable`] when the body is not UTF-8 or does not decode.
fn decode_body(
    body: &[u8],
    encoding: ResponseEncoding,
    what: &str,
) -> Result<DecodedBody, ControlFailure> {
    let body = std::str::from_utf8(body).map_err(|error| ControlFailure::Unavailable {
        detail: format!("malformed {what} response: {error}"),
    })?;

    match encoding {
        ResponseEncoding::Json => {
            let json = serde_json::from_str(body).map_err(|error| ControlFailure::Unavailable {
                detail: format!("malformed {what} response: {error}"),
            })?;

            Ok(DecodedBody::Json(json))
        }

        ResponseEncoding::Form => Ok(DecodedBody::Form(parse_form(body))),
    }
}

/// Whether an upstream boolean flag reads as true. The truthy spellings such a flag — a
/// provisioning poll's `done_path` (`docs/design/providers.md`) or a cursor pagination's `has_more`
/// path (`docs/design/providers.md`) — may arrive in on a JSON body are the boolean `true`, the
/// number `1`, or the string `"true"`; every other shape reads as false. The flat form encoding
/// surfaces the same two non-boolean scalars in their string spellings, which each consumer reads
/// at its own call site.
#[must_use]
pub(super) fn is_truthy_scalar(value: &Json) -> bool {
    match value {
        Json::Bool(flag) => *flag,
        Json::Number(number) => number.as_i64() == Some(1),
        Json::String(text) => text == "true",
        _ => false,
    }
}

/// Extracts one declared response field from a decoded body: a missing required path fails the
/// stage, a missing optional path yields `None`, and a present value of the wrong declared
/// shape always fails (`docs/design/providers.md`). The text shape reads a JSON string or number —
/// a number renders as its decimal text (RFC 6749 §5.1) — and rejects every other JSON kind.
///
/// # Errors
///
/// Returns [`ControlFailure::Unavailable`] when a required path is missing or a present value
/// contradicts its declared shape.
pub(super) fn extract_field(
    body: &DecodedBody,
    path: &ResponsePath,
    shape: &ExtractionShape,
    what: &str,
) -> Result<Option<Extracted>, ControlFailure> {
    match body {
        DecodedBody::Json(json) => {
            // An unresolvable path — a missing member or a non-object intermediate — is a
            // missing path (`docs/design/providers.md`): the caller decides required versus
            // optional.
            let Some(value) = path.resolve(json) else {
                return Ok(None);
            };

            match shape {
                // A JSON number renders as its decimal text (RFC 6749 §5.1: "Numerical values
                // are included as JSON numbers" — the RFC 8628 §3.2 example reports
                // `"expires_in": 1800, "interval": 5`), so the text shape reads strings and
                // numbers alike; every other JSON kind is the wrong shape.
                ExtractionShape::String => match value {
                    Json::String(text) => Ok(Some(Extracted::Text(text.clone()))),
                    Json::Number(number) => Ok(Some(Extracted::Text(number.to_string()))),
                    _ => Err(ControlFailure::Unavailable {
                        detail: format!("{what} response value at {:?} must be a string", path.0),
                    }),
                },

                ExtractionShape::Integer => match value.as_i64() {
                    Some(integer) => Ok(Some(Extracted::Integer(integer))),
                    None => Err(ControlFailure::Unavailable {
                        detail: format!("{what} response value at {:?} must be an integer", path.0),
                    }),
                },

                ExtractionShape::StringOrField { field } => {
                    if let Some(text) = value.as_str() {
                        return Ok(Some(Extracted::Text(text.to_string())));
                    }

                    match value.get(field).and_then(Json::as_str) {
                        Some(text) => Ok(Some(Extracted::Text(text.to_string()))),
                        None => Err(ControlFailure::Unavailable {
                            detail: format!(
                                "{what} response value at {:?} must be a string or carry a \
                                 string `{field}` member",
                                path.0
                            ),
                        }),
                    }
                }
            }
        }

        DecodedBody::Form(fields) => {
            let [member] = path.0.as_slice() else {
                return Err(ControlFailure::Unavailable {
                    detail: format!(
                        "{what} response path {:?} addresses a flat form field",
                        path.0
                    ),
                });
            };

            match fields.get(member) {
                Some(text) => match shape {
                    ExtractionShape::Integer => match text.parse::<i64>() {
                        Ok(integer) => Ok(Some(Extracted::Integer(integer))),
                        Err(_) => Err(ControlFailure::Unavailable {
                            detail: format!(
                                "{what} response value at {:?} must be an integer",
                                path.0
                            ),
                        }),
                    },

                    // A form field is a string; the string-or-field polymorphism collapses to
                    // its string form on the flat form encoding.
                    ExtractionShape::String | ExtractionShape::StringOrField { .. } => {
                        Ok(Some(Extracted::Text(text.clone())))
                    }
                },

                None => Ok(None),
            }
        }
    }
}

/// The extraction shape every stage-level [`ResponseField`] mapping reads its value with: the
/// device-authorization, exchange, and refresh mappings all read text — a JSON number renders
/// as its decimal text (RFC 6749 §5.1) — and their integer-valued targets parse the text at
/// their consumers. A provisioning extraction declares its own shape.
pub(super) const TEXT_FIELD_SHAPE: ExtractionShape = ExtractionShape::String;

/// The descriptor-level facts of one response-field mapping, abstracted over the stage
/// descriptors whose extraction shapes differ: the stage-level [`ResponseField`] fixes the
/// shared text shape, while a [`ProvisionExtraction`] declares its own.
pub(super) trait ResponseMapping {
    /// The closed path of the response value.
    fn path(&self) -> &ResponsePath;

    /// The declared shape of the value.
    fn shape(&self) -> &ExtractionShape;

    /// Whether a missing path fails the stage.
    fn is_required(&self) -> bool;

    /// Where the extracted value is stored.
    fn target(&self) -> &ResponseTarget;
}

impl ResponseMapping for ResponseField {
    fn path(&self) -> &ResponsePath {
        &self.path
    }

    fn shape(&self) -> &ExtractionShape {
        &TEXT_FIELD_SHAPE
    }

    fn is_required(&self) -> bool {
        self.required
    }

    fn target(&self) -> &ResponseTarget {
        &self.target
    }
}

impl ResponseMapping for ProvisionExtraction {
    fn path(&self) -> &ResponsePath {
        &self.path
    }

    fn shape(&self) -> &ExtractionShape {
        &self.shape
    }

    fn is_required(&self) -> bool {
        self.required
    }

    fn target(&self) -> &ResponseTarget {
        &self.target
    }
}

/// Folds one decoded response's declared field mappings into the caller's state through `apply`
/// (`docs/design/providers.md`): every field is extracted through [`extract_field`], a missing
/// required path fails the stage naming the offending path, and a missing optional path leaves its
/// target unchanged.
///
/// # Errors
///
/// Returns the first failure: an extraction failure converted through [`ControlFailure`], the
/// missing-required failure, or the first `apply` failure returned unchanged.
pub(super) fn map_response_fields<T, E>(
    decoded: &DecodedBody,
    fields: &[T],
    what: &str,
    mut apply: impl FnMut(&ResponseTarget, Extracted) -> Result<(), E>,
) -> Result<(), E>
where
    T: ResponseMapping,
    E: From<ControlFailure>,
{
    for field in fields {
        let extracted =
            extract_field(decoded, field.path(), field.shape(), what).map_err(E::from)?;

        let Some(extracted) = extracted else {
            if field.is_required() {
                return Err(E::from(ControlFailure::Unavailable {
                    detail: format!(
                        "{what} response lacks the required path {:?}",
                        field.path().0
                    ),
                }));
            }

            continue;
        };

        apply(field.target(), extracted)?;
    }

    Ok(())
}
