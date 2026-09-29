//! The shared control-request executor: one engine drives the OAuth device authorization, token
//! exchange, refresh, provisioning, polling, and discovery requests (README §5.1).
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
            ResponseField, ResponsePath, ResponseTarget, TemplateNode,
        },
    },
};
use reqwest::{Client, Method, RequestBuilder};
use serde_json::Value as Json;
use std::collections::BTreeMap;

/// A decoded control response body: the declared encoding's value tree.
#[derive(Debug, Clone)]
pub(crate) enum DecodedBody {
    /// A JSON document.
    Json(Json),

    /// A `application/x-www-form-urlencoded` body of flat string fields.
    Form(BTreeMap<String, String>),
}

/// One extracted response value: the declared shapes render into text or integer.
#[derive(Debug, Clone)]
pub(crate) enum Extracted {
    /// A string value.
    Text(String),

    /// An integer value.
    Integer(i64),
}

impl Extracted {
    /// The value as text; an integer renders in its decimal form.
    #[must_use]
    pub(crate) fn as_text(&self) -> String {
        match self {
            Self::Text(text) => text.clone(),
            Self::Integer(value) => value.to_string(),
        }
    }

    /// The value as an integer; a text value is `None`.
    #[must_use]
    pub(crate) const fn as_integer(&self) -> Option<i64> {
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
pub(crate) enum ControlFailure {
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

            b'%' => match bytes
                .get(index + 1..index + 3)
                .and_then(|hex| std::str::from_utf8(hex).ok())
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
            {
                Some(decoded_byte) => Step::Skip(decoded_byte, 3),

                None => Step::Push(b'%'),
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
pub(crate) fn parse_form(body: &str) -> BTreeMap<String, String> {
    body.split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((name, value)) => (form_decode(name), form_decode(value)),
            None => (form_decode(pair), String::new()),
        })
        .collect()
}

/// Renders one JSON template tree against the environment, splicing the rendered upstream body
/// into the tree's single body placeholder.
///
/// # Errors
///
/// Returns [`ControlFailure::Unavailable`] when a string leaf references a value the stage does
/// not carry.
fn render_json_node(
    node: &TemplateNode,
    body: &Json,
    env: &ValueEnv<'_>,
    what: &str,
) -> Result<Json, ControlFailure> {
    env.render_json(node, body, what)
        .map_err(ControlFailure::from)
}

/// Percent-encodes one URL query component with the RFC 3986 unreserved set: every byte outside
/// `A–Z a–z 0–9 - . _ ~` renders as `%XX`, so a space renders `%20` and a literal plus `%2B`.
/// This strict encoding is the one vocabulary every descriptor-rendered query component uses —
/// the authorize-URL builder (README §5.3) and the control-request query rows built here alike —
/// because RFC 3986 §3.4 defines `+` in a query as a literal plus. The
/// `application/x-www-form-urlencoded` body variant keeps its own `+`-for-space rule, which is
/// the content type's, not the URL query's.
#[must_use]
pub(crate) fn percent_encode_component(component: &str) -> String {
    let mut encoded = String::with_capacity(component.len());

    for byte in component.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(char::from(byte));
            }

            other => {
                encoded.push_str(&format!("%{other:02X}"));
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
pub(crate) fn build_request(
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
            let body = render_json_node(node, &Json::Null, env, what)?;

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
/// The bounded error-body policy is the shared one: a non-2xx body is read through
/// [`http_body::read_error_body`] and carried untruncated in [`ControlFailure::Rejected`], so the
/// token-grant classification can inspect error codes before any excerpt is rendered.
///
/// # Errors
///
/// Returns [`ControlFailure`] for transport failures, non-2xx statuses, and bodies that do not
/// decode through the declared encoding.
pub(crate) async fn execute(
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

    let body = response.text().await.map_err(|error| {
        transport_failure(format!("{what} response read failed: {error}"), &error)
    })?;

    decode_body(&body, encoding, what)
}

/// Decodes the response body through an explicit declared encoding.
///
/// # Errors
///
/// Returns [`ControlFailure::Unavailable`] when the body does not decode.
pub(crate) fn decode_body(
    body: &str,
    encoding: ResponseEncoding,
    what: &str,
) -> Result<DecodedBody, ControlFailure> {
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
/// provisioning poll's `done_path` (README §5.4) or a cursor pagination's `has_more` path
/// (README §5.5) — may arrive in on a JSON body are the boolean `true`, the number `1`, or the
/// string `"true"`; every other shape reads as false. The flat form encoding surfaces the same
/// two non-boolean scalars in their string spellings, which each consumer reads at its own call
/// site.
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
/// shape always fails (README §5.1). The text shape reads a JSON string or number — a number
/// renders as its decimal text (RFC 6749 §5.1) — and rejects every other JSON kind.
///
/// # Errors
///
/// Returns [`ControlFailure::Unavailable`] when a required path is missing or a present value
/// contradicts its declared shape.
pub(crate) fn extract_field(
    body: &DecodedBody,
    path: &ResponsePath,
    shape: &ExtractionShape,
    what: &str,
) -> Result<Option<Extracted>, ControlFailure> {
    match body {
        DecodedBody::Json(json) => {
            // An unresolvable path — a missing member or a non-object intermediate — is a
            // missing path (README §5.1): the caller decides required versus optional.
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
                        detail: format!("{what} response value must be a string"),
                    }),
                },

                ExtractionShape::Integer => match value.as_i64() {
                    Some(integer) => Ok(Some(Extracted::Integer(integer))),
                    None => Err(ControlFailure::Unavailable {
                        detail: format!("{what} response value must be an integer"),
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
                                "{what} response value must be a string or carry a string \
                                 `{field}` member"
                            ),
                        }),
                    }
                }
            }
        }

        DecodedBody::Form(fields) => {
            let [member] = path.0.as_slice() else {
                return Err(ControlFailure::Unavailable {
                    detail: format!("{what} response path addresses a flat form field"),
                });
            };

            match fields.get(member) {
                Some(text) => match shape {
                    ExtractionShape::Integer => match text.parse::<i64>() {
                        Ok(integer) => Ok(Some(Extracted::Integer(integer))),
                        Err(_) => Err(ControlFailure::Unavailable {
                            detail: format!("{what} response value must be an integer"),
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
pub(crate) const TEXT_FIELD_SHAPE: ExtractionShape = ExtractionShape::String;

/// The descriptor-level facts of one response-field mapping, abstracted over the stage
/// descriptors whose extraction shapes differ: the stage-level [`ResponseField`] fixes the
/// shared text shape, while a [`ProvisionExtraction`] declares its own.
pub(crate) trait ResponseMapping {
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
/// (README §5.1): every field is extracted through [`extract_field`], a missing required path
/// fails the stage with the label, and a missing optional path leaves its target unchanged.
///
/// # Errors
///
/// Returns the first failure: an extraction failure converted through [`ControlFailure`], the
/// missing-required failure, or the first `apply` failure returned unchanged.
pub(crate) fn map_response_fields<T, E>(
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
                    detail: format!("{what} response lacks a required field"),
                }));
            }

            continue;
        };

        apply(field.target(), extracted)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::unwrap_used,
        reason = "the fixtures are constructed so each unwrap lands on its documented side"
    )]
    #![expect(
        clippy::panic,
        reason = "a fixture landing on the wrong side of its check is the failure report"
    )]

    use super::*;
    use crate::provider::definition::template::{ParamRow, StringExpr};

    /// Control-request query rows render through the strict RFC 3986 unreserved-set percent
    /// encoding the authorize-URL builder shares: a space renders `%20` and a literal plus
    /// `%2B` — RFC 3986 §3.4 makes `+` in a query a literal plus — and the pre-encoded
    /// components reach the built request unre-encoded. A row on a URL already carrying a
    /// query joins with `&`. The form body variant is untouched: its `+`-for-space rule is the
    /// `application/x-www-form-urlencoded` content type's own.
    ///
    /// # Panics
    ///
    /// Panics when the built request's query diverges from the strict encoding.
    #[test]
    fn encodes_query_rows_with_the_strict_unreserved_set() {
        let request = ControlRequest {
            method: ControlMethod::Get,
            url: StringExpr::literal("https://upstream.example/discovery?fixed=1"),
            headers: Vec::new(),
            query: vec![
                ParamRow {
                    name: "scope".to_string(),
                    value: StringExpr::literal("a b+c"),
                },
                ParamRow {
                    name: "label key".to_string(),
                    value: StringExpr::literal("é"),
                },
            ],
            body: None,
        };

        let slots = BTreeMap::new();

        let env = ValueEnv::for_exchange(
            &slots,
            None,
            "client-id",
            None,
            "code",
            Some("verifier"),
            "redirect",
        );

        let built = build_request(&Client::new(), &request, &env, "test")
            .unwrap()
            .build()
            .unwrap();

        assert_eq!(
            built.url().query(),
            Some("fixed=1&scope=a%20b%2Bc&label%20key=%C3%A9")
        );
    }

    /// A control request whose evaluated URL carries a `#` fragment gets its query rows joined
    /// into the query component before the fragment, and the fragment re-attached after the
    /// last row (RFC 3986 §3): rows appended after the first `#` would fold into the fragment
    /// and silently drop from the request.
    ///
    /// # Panics
    ///
    /// Panics when the built request's URL diverges from the byte-exact expectation.
    #[test]
    fn build_request_joins_query_rows_before_a_url_fragment() {
        let request = ControlRequest {
            method: ControlMethod::Get,
            url: StringExpr::literal("https://upstream.example/discovery#tenant"),
            headers: Vec::new(),
            query: vec![
                ParamRow {
                    name: "scope".to_string(),
                    value: StringExpr::literal("a b+c"),
                },
                ParamRow {
                    name: "page".to_string(),
                    value: StringExpr::literal("2"),
                },
            ],
            body: None,
        };

        let slots = BTreeMap::new();

        let env = ValueEnv::for_exchange(
            &slots,
            None,
            "client-id",
            None,
            "code",
            Some("verifier"),
            "redirect",
        );

        let built = build_request(&Client::new(), &request, &env, "test")
            .unwrap()
            .build()
            .unwrap();

        assert_eq!(
            built.url().as_str(),
            "https://upstream.example/discovery?scope=a%20b%2Bc&page=2#tenant"
        );

        assert_eq!(built.url().query(), Some("scope=a%20b%2Bc&page=2"));

        assert_eq!(built.url().fragment(), Some("tenant"));
    }

    /// The shared query-row join inserts the `?`/`&` separator into the URL's query component
    /// and re-attaches a `#` fragment after the appended row (RFC 3986 §3): the first `#` is
    /// always the fragment delimiter, and a `?` inside a fragment never flips the join. The
    /// join is the helper's whole contract — the row text lands verbatim.
    ///
    /// # Panics
    ///
    /// Panics when a rendered URL diverges from its byte-exact expectation.
    #[test]
    fn append_query_row_joins_into_the_query_before_the_fragment() {
        let cases = [
            (
                "https://upstream.example/discovery",
                "a",
                "1",
                "https://upstream.example/discovery?a=1",
            ),
            (
                "https://upstream.example/discovery?a=1",
                "b",
                "2",
                "https://upstream.example/discovery?a=1&b=2",
            ),
            (
                "https://upstream.example/discovery#tenant",
                "a",
                "1",
                "https://upstream.example/discovery?a=1#tenant",
            ),
            (
                "https://upstream.example/discovery?a=1#tenant",
                "b",
                "2",
                "https://upstream.example/discovery?a=1&b=2#tenant",
            ),
            (
                "https://upstream.example/discovery#tenant?a=1",
                "b",
                "2",
                "https://upstream.example/discovery?b=2#tenant?a=1",
            ),
        ];

        for (before, name, value, after) in cases {
            let mut url = before.to_string();

            append_query_row(&mut url, name, value);

            assert_eq!(url, after, "join mismatch for `{before}`");
        }
    }

    /// Pins the shared field-mapping fold (README §5.1): present values apply in order, a
    /// missing optional path leaves its target unchanged, a missing required path fails the
    /// stage with its label, and a present value of the wrong declared shape always fails. The
    /// text shape reads strings and JSON numbers — the number renders as its decimal text
    /// (RFC 6749 §5.1) — and rejects every other JSON kind.
    ///
    /// # Panics
    ///
    /// Panics when a fixture lands on the wrong side of its check.
    #[test]
    fn folds_response_fields_per_their_declaration() {
        let decoded = DecodedBody::Json(serde_json::json!({
            "present": "value",
            "nested": { "leaf": "refresh-token" },
            "count": 1,
            "flag": true
        }));

        let fields = [
            ResponseField {
                path: ResponsePath(vec!["absent".to_string()]),
                target: ResponseTarget::Slot {
                    name: "optional".to_string(),
                },
                required: false,
            },
            ResponseField {
                path: ResponsePath(vec!["present".to_string()]),
                target: ResponseTarget::AccessToken,
                required: false,
            },
            ResponseField {
                path: ResponsePath(vec!["nested".to_string(), "leaf".to_string()]),
                target: ResponseTarget::RefreshToken,
                required: true,
            },
        ];

        let mut applied = Vec::new();

        map_response_fields(&decoded, &fields, "stage", |target, extracted| {
            applied.push((target.clone(), extracted.as_text()));

            Ok::<(), ControlFailure>(())
        })
        .unwrap();

        assert_eq!(
            applied,
            vec![
                (ResponseTarget::AccessToken, "value".to_string()),
                (ResponseTarget::RefreshToken, "refresh-token".to_string()),
            ]
        );

        let required = [ResponseField {
            path: ResponsePath(vec!["absent".to_string()]),
            target: ResponseTarget::AccessToken,
            required: true,
        }];

        let Err(failure) = map_response_fields(&decoded, &required, "stage", |_, _| {
            Ok::<(), ControlFailure>(())
        }) else {
            panic!("a missing required path must fail the stage");
        };

        let ControlFailure::Unavailable { detail } = failure else {
            panic!("a missing required path is an unavailable stage");
        };

        assert_eq!(detail, "stage response lacks a required field");

        // The text shape reads the integer the `count` member carries, rendered as its decimal
        // text (RFC 6749 §5.1 includes numerical values as JSON numbers).
        let numeric = [ResponseField {
            path: ResponsePath(vec!["count".to_string()]),
            target: ResponseTarget::AccessToken,
            required: true,
        }];

        let mut applied = Vec::new();

        map_response_fields(&decoded, &numeric, "stage", |target, extracted| {
            applied.push((target.clone(), extracted.as_text()));

            Ok::<(), ControlFailure>(())
        })
        .unwrap();

        assert_eq!(
            applied,
            vec![(ResponseTarget::AccessToken, "1".to_string())]
        );

        // Any other non-string, non-number kind stays the wrong shape.
        let wrong_shape = [ResponseField {
            path: ResponsePath(vec!["flag".to_string()]),
            target: ResponseTarget::AccessToken,
            required: true,
        }];

        let is_failed = map_response_fields(&decoded, &wrong_shape, "stage", |_, _| {
            Ok::<(), ControlFailure>(())
        })
        .is_err();

        assert!(is_failed);
    }
}
