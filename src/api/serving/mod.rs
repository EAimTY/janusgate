//! Shared outward HTTP policy: authentication, extraction, routing fallbacks, and response
//! builders.

use crate::{
    api::{Error, log_error},
    context::Context,
    database::Database,
    security,
    storage::{
        api_keys,
        models::{self, EnabledModel},
    },
};
use axum::{
    Router,
    body::Body,
    extract::{
        DefaultBodyLimit, FromRequestParts, OriginalUri, Request, State, rejection::BytesRejection,
    },
    http::{Method, StatusCode, header, request::Parts as RequestParts},
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use std::{convert::Infallible, marker::PhantomData};

mod completion;
mod delivery;
pub(super) use completion::{OperationKind, run_compact, run_completion};

/// Maximum accepted request body size, large enough for inline image data.
pub(super) const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;

/// The per-surface identity the shared pipeline renders through: the two conventions that are
/// deliberately not shared — how a request presents its credential, and how an [`Error`] lands on
/// the wire.
///
/// Every surface module implements this trait for a private marker type and points its router's
/// authentication middleware, 404/405 fallbacks, and handlers at [`authenticate`],
/// [`error_response`], [`fallback`], [`method_not_allowed`], [`BodyError`], and
/// [`outcome_response`] through it.
pub(super) trait Surface {
    /// Renders one API failure onto the surface's error wire: the surface's error render module's
    /// `render` function, which classifies the [`Error`] variant into the surface's (HTTP status,
    /// JSON envelope) pair (README §6.7). The shared [`error_response`] composes it with the
    /// universal JSON response builder, so every rejection and handler failure the pipeline
    /// produces lands on the wire through one glue path.
    fn render_error(error: &Error) -> (StatusCode, impl Serialize);

    /// Reads the presented credential key from the request per the surface's authentication
    /// convention — the header that carries it and any companion header the convention requires —
    /// or classifies why the convention cannot accept the request's credential, having logged that
    /// rejection at the surface's boundary. The returned string is the raw presented secret;
    /// matching it against the enabled keys and rendering the rejection both stay in the shared
    /// [`authenticate`].
    fn extract_credential(request: &Request) -> Result<String, AuthRejection>;
}

/// Why a request's credential failed the surface's authentication convention before any key
/// comparison ([`Surface::extract_credential`]): no credential the convention could read, or a
/// convention requirement beyond the credential header the request failed. The shared
/// [`authenticate`] renders both through the surface's error envelope.
pub(super) enum AuthRejection {
    /// The request presented no credential the surface's convention could read; the shared
    /// pipeline classifies it as the shared invalid-key rejection.
    MissingCredential,

    /// The request failed a convention requirement beyond the credential header itself; the
    /// already-classified contract violation renders as is.
    Malformed(Error),
}

/// The row id of the gateway API key that authorized the current request: the authentication
/// middleware stamps it into the request extensions when it lets a request through, and the
/// completion handlers extract it to attribute the request's accounting row (README §3 *ApiLog*).
/// Only the never-recycled row identity travels this far — never the secret itself.
#[derive(Debug, Clone, Copy)]
pub(super) struct AuthenticatedApiKeyRow(pub(super) i64);

impl<S> FromRequestParts<S> for AuthenticatedApiKeyRow
where
    S: Send + Sync,
{
    /// Never constructed: this extractor fails fast instead of rejecting. A completion handler
    /// always runs behind [`authenticate`] first, so a missing extension is an invariant
    /// violation of the router's own pipeline, not a client-repairable condition.
    type Rejection = Infallible;

    async fn from_request_parts(
        parts: &mut RequestParts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        #[expect(
            clippy::expect_used,
            reason = "the authentication middleware always stamps this extension before a \
                      completion handler runs; a missing extension is an invariant violation \
                      that fails fast"
        )]
        let row_id = parts
            .extensions
            .get::<AuthenticatedApiKeyRow>()
            .expect("an authenticated completion request carries its API key row id")
            .0;

        Ok(Self(row_id))
    }
}

/// Authenticates the request against the enabled gateway API keys, reading the presented
/// credential through the surface's convention ([`Surface::extract_credential`]). The keys are
/// read from the live database, so a created, rotated, or disabled key takes effect on the next
/// request; a database failure renders the surface's internal error instead of an authentication
/// rejection. A request that passes is stamped with the matching key's row id, which the
/// completion handlers carry into the request accounting (README §3 *ApiLog*).
pub(super) async fn authenticate<S>(
    State(database): State<Database>,
    request: Request,
    next: Next,
) -> Response
where
    S: Surface,
{
    let presented = match S::extract_credential(&request) {
        Ok(presented) => presented,

        Err(rejection) => {
            let error = match rejection {
                AuthRejection::MissingCredential => Error::InvalidApiKey,

                AuthRejection::Malformed(error) => error,
            };

            return error_response::<S>(error);
        }
    };

    match api_keys::list_enabled_api_key_secrets(&database).await {
        Ok(keys) => {
            let matched = keys
                .into_iter()
                .find(|(_, secret)| security::constant_time_eq(&presented, secret));

            match matched {
                Some((row_id, _secret)) => {
                    let mut request = request;

                    request
                        .extensions_mut()
                        .insert(AuthenticatedApiKeyRow(row_id));

                    next.run(request).await
                }

                None => {
                    // A presented key that matched no enabled gateway API key: logged once at
                    // this shared boundary, then rendered as the invalid-key rejection.
                    let error = Error::InvalidApiKey;

                    log_error("request", &error);

                    error_response::<S>(error)
                }
            }
        }

        Err(error) => {
            let error = Error::from(error);

            log_error("request", &error);

            error_response::<S>(error)
        }
    }
}

/// Renders one API failure as the surface's JSON error response: the surface's error wire
/// renderer ([`Surface::render_error`]) through the universal JSON response builder
/// [`json_response`]. Every rejection and handler failure the pipeline produces lands on the wire
/// here, and each surface's handlers reuse it directly for their own failures (README §6.7).
///
/// # Panics
///
/// Panics if positive decimal retry seconds cannot form a header, which their type prevents.
pub(super) fn error_response<S>(error: Error) -> Response
where
    S: Surface,
{
    let (status, envelope) = S::render_error(&error);

    let mut response = json_response(status, &envelope);
    if let Error::PoolQuotaExhausted {
        retry_after: Some(seconds),
        ..
    }
    | Error::BoundAccountQuotaExhausted {
        retry_after: Some(seconds),
        ..
    } = error
    {
        #[expect(
            clippy::expect_used,
            reason = "positive decimal seconds are a valid header value"
        )]
        let value = seconds
            .to_string()
            .parse()
            .expect("decimal Retry-After header");
        response
            .headers_mut()
            .insert(axum::http::header::RETRY_AFTER, value);
    }
    response
}

/// Handles unmapped surface paths with the surface's 404, logging the rendered error once at this
/// response boundary.
pub(super) async fn fallback<S>(OriginalUri(uri): OriginalUri, request: Request) -> Response
where
    S: Surface,
{
    let error = Error::InvalidUrl {
        method: request.method().as_str().to_string(),
        path: uri.path().to_string(),
    };

    log_error("request", &error);

    error_response::<S>(error)
}

/// Handles a request whose method is not registered on an otherwise-matching path with the
/// surface's 405, logging the rendered error once at this response boundary. Axum still attaches
/// the `Allow` header listing the registered methods.
pub(super) async fn method_not_allowed<S>(method: Method, OriginalUri(uri): OriginalUri) -> Response
where
    S: Surface,
{
    let error = Error::MethodNotAllowed;

    log_error(
        &format!("request {} {}", method.as_str(), uri.path()),
        &error,
    );

    error_response::<S>(error)
}

/// Composes one surface router's shared pipeline tail — the surface's 404/405 fallbacks
/// ([`fallback`], [`method_not_allowed`]), the authentication middleware ([`authenticate`]), and
/// the body limit ([`MAX_BODY_BYTES`]) — onto the surface's route set. Each surface's `router`
/// function owns only its route set and delegates this tail, so all three surfaces compose one
/// identical service tree.
///
/// The order is load-bearing, because an axum layer wraps everything registered before it:
///
/// - The authentication middleware wraps the routes and their fallbacks — the fallbacks are
///   registered first — so an unauthorized request is rejected with the surface's 401 before a 404
///   or 405 body is produced (README §2.2 item 4).
/// - The body limit is extractor-side configuration: the [`DefaultBodyLimit`] layer only plants the
///   limit into request extensions, and each handler's [`axum::body::Bytes`] extractor enforces it,
///   rendering the surface's 413 ([`BodyError`]) at extraction time — after authentication, since
///   the handlers run within [`authenticate`]'s `next.run` (README §4: the 64 MiB outward body
///   cap).
///
/// `database` feeds the authentication middleware its own state, so every request checks the
/// presented key against the live database rather than the router state.
pub(super) fn surface_router<S>(database: Database, routes: Router<Context>) -> Router<Context>
where
    // `'static` because the fallbacks and the authentication middleware are stored in the
    // router's service tree, whose handlers must be `'static`.
    S: Surface + 'static,
{
    routes
        .fallback(fallback::<S>)
        .method_not_allowed_fallback(method_not_allowed::<S>)
        .layer(middleware::from_fn_with_state(database, authenticate::<S>))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
}

/// Body extractor rejection mapped into the surface's error shape: an over-limit body renders the
/// surface's 413, every other body failure renders as a malformed request.
///
/// The surface parameter rides as `fn() -> S`, which is unconditionally `Send` and `Sync`
/// regardless of `S`: the marker contributes nothing at runtime, and the response body must stay
/// `Send`.
#[derive(Debug)]
pub(super) struct BodyError<S>(Error, PhantomData<fn() -> S>);

impl<S> From<BytesRejection> for BodyError<S> {
    fn from(rejection: BytesRejection) -> Self {
        Self(Error::from(rejection), PhantomData)
    }
}

impl<S> IntoResponse for BodyError<S>
where
    S: Surface,
{
    fn into_response(self) -> Response {
        log_error("request body", &self.0);

        error_response::<S>(self.0)
    }
}

/// Renders a handler outcome, mapping failures to the surface's error responses.
pub(super) fn outcome_response<S>(outcome: Result<Response, Error>) -> Response
where
    S: Surface,
{
    match outcome {
        Ok(response) => response,

        Err(error) => error_response::<S>(error),
    }
}

/// Builds a JSON response with the given status code.
///
/// The outward surface's universal response builder, defined once beside the shared pipeline so
/// every surface owns the same rendering. The body is a typed payload, so serialization is total.
///
/// # Panics
///
/// Panics if the rendered body fails to serialize or the static response
/// parts fail to build, neither of which can occur.
pub(super) fn json_response<T>(status: StatusCode, body: &T) -> Response
where
    T: Serialize,
{
    #[expect(
        clippy::expect_used,
        reason = "every outward body is a typed payload whose serialization is total"
    )]
    let rendered = serde_json::to_string(body).expect("outward response bodies serialize");

    #[expect(clippy::expect_used, reason = "JSON responses have known-valid parts")]
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(rendered))
        .expect("static response parts are always valid")
}

/// Renders the outward `GET /models` listing shared by every surface (README §6.3/§6.5): it
/// lists the effectively enabled models with their effective capability declarations over the
/// shared pipeline's database and maps them onto the surface's native page shape, or converts
/// the storage failure and renders it through the surface's error envelope under the shared
/// `"model listing"` log target. Each surface supplies only its success-shape mapping.
///
/// `render_page` receives the listed models by value, ordered by public ID, and returns the
/// surface's typed success body, which [`json_response`] serializes; each entry's capability
/// values are already the effective `override ?? base` composition, with `None` (unknown) left
/// for the surface to omit (README §6.5).
pub(super) async fn model_list_response<S, T, F>(
    database: &Database,
    api_key_id: i64,
    render_page: F,
) -> Response
where
    S: Surface,
    F: FnOnce(Vec<EnabledModel>) -> T,
    T: Serialize,
{
    match models::list_enabled_models_with_owner(database, api_key_id).await {
        Ok(listed) => json_response(StatusCode::OK, &render_page(listed)),

        Err(error) => {
            let error = Error::from(error);

            log_error("model listing", &error);

            error_response::<S>(error)
        }
    }
}

#[cfg(test)]
mod tests;
