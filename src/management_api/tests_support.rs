//! Shared management-API test fixtures: the gateway test harness driving the real root router —
//! the `/api` surface beside the `/management-api` one, assembled exactly the way the server
//! assembles it — and the request and error-envelope helpers every management test family uses.

#![expect(
    clippy::expect_used,
    clippy::missing_panics_doc,
    reason = "a fixture that cannot install, store, or serve is that test's failure report"
)]

use crate::{
    context::Context,
    database::Database,
    http_client,
    test_support::{
        MANAGEMENT_TOKEN, TempDatabaseGuard, configuration_lock, install_configuration,
        temp_database,
    },
};
use axum::{Router, body::Body, response::Response};
use serde_json::Value;
use std::{error::Error as StdError, num::NonZero, path::PathBuf};
use tower::Service;

/// The management-API version prefix the server mounts the version routers under.
pub(crate) const VERSION_PREFIX: &str = "/management-api/v0";

/// The static upstream credential the fixture provider rows carry; assertions that credentials
/// never cross the management wire search bodies for this value.
pub(crate) const UPSTREAM_SECRET: &str = "sk-upstream-fixture";

/// The per-request body read cap of the response collector.
const BODY_READ_LIMIT: usize = 64 * 1024 * 1024;

/// The gateway under test: one temporary main-registry database, the request context built the
/// way the server builds it, and the real root router — `/api` beside `/management-api` — driven
/// in process.
pub(crate) struct TestGateway {
    /// The gateway's database, also cloned into the context and the router's authentication
    /// middleware.
    database: Database,

    /// The api-log store the usage tests record their controlled rows into.
    api_log_store: crate::storage::api_logs::ApiLogStore,

    /// The complete root router with its state applied.
    router: Router,

    /// The throwaway directory the test's api-log rows record into.
    _api_log_directory: PathBuf,

    /// The temporary database's drop guard.
    _database_guard: TempDatabaseGuard,
}

impl TestGateway {
    /// Opens the gateway over a fresh temporary database: the shared configuration is installed,
    /// the context is assembled exactly like the server startup assembles it, and the root router
    /// is built over it the way `src/server.rs` builds it — the management nest present exactly
    /// while the `management_api` configuration section is.
    pub(crate) async fn open() -> Self {
        let lock = configuration_lock();

        install_configuration();

        drop(lock);

        crate::affinity::initialize_key();

        let (database, database_guard) = temp_database().await;

        // Every test records its accounting rows into its own throwaway directory, so tests
        // never share api-log files.
        let api_log_directory = std::env::temp_dir().join(format!(
            "janusgate-test-mgmt-logs-{}",
            rand::random::<u64>()
        ));

        let api_log_store = crate::storage::api_logs::ApiLogStore::new(
            api_log_directory.clone(),
            NonZero::<u32>::new(1).expect("the fixture retention is a nonzero literal"),
        );

        let context = Context {
            database: database.clone(),
            api_log_store: api_log_store.clone(),
            affinity_cache: std::sync::Arc::new(crate::affinity::Cache::default()),
            quota_coordinator: std::sync::Arc::new(
                crate::provider::quota::coordinator::Coordinator::default(),
            ),
            data_plane: http_client::data_plane(),
            control_plane: http_client::control_plane(),
        };

        let mut router = Router::new().nest("/api", crate::api::router(database.clone()));

        if crate::CONFIG.management_api.is_some() {
            router = router.nest("/management-api", super::router());
        }

        let router = router
            .fallback(crate::router_fallback::fallback)
            .with_state(context);

        Self {
            database,
            api_log_store,
            router,
            _api_log_directory: api_log_directory,
            _database_guard: database_guard,
        }
    }

    /// The gateway's database handle, for the storage-level reads the assertions make.
    pub(crate) fn database(&self) -> &Database {
        &self.database
    }

    /// The gateway's api-log store, for the usage tests' controlled row recordings.
    pub(crate) fn api_log_store(&self) -> &crate::storage::api_logs::ApiLogStore {
        &self.api_log_store
    }

    /// Serves one request through the real router.
    pub(crate) async fn send(&self, request: axum::extract::Request) -> Response {
        let mut router = self.router.clone();

        std::future::poll_fn(|cx| {
            <Router as Service<http::Request<Body>>>::poll_ready(&mut router, cx)
        })
        .await
        .expect("the router's readiness never fails");

        router
            .call(request)
            .await
            .expect("the router's service error is infallible")
    }

    /// Serves one fully controlled management request: `method` and `path` verbatim, `headers`
    /// carried in order, and `body` as the raw string — `None` sending no body and, with it, no
    /// content type.
    pub(crate) async fn request(
        &self,
        method: &str,
        path: &str,
        headers: &[(&'static str, String)],
        body: Option<String>,
    ) -> Response {
        let mut builder = http::Request::builder().method(method).uri(path);

        for (name, value) in headers {
            builder = builder.header(*name, value);
        }

        let request = builder
            .body(Body::from(body.unwrap_or_default()))
            .expect("static request parts are always valid");

        self.send(request).await
    }

    /// Serves the management action at `path` with the correct token, a JSON content type, and a
    /// JSON body.
    pub(crate) async fn post_management(&self, path: &str, body: Value) -> Response {
        self.post_management_with(path, Authorization::Correct, body)
            .await
    }

    /// Serves the management action at `path` with the chosen `Authorization` credential, a JSON
    /// content type, and a JSON body.
    pub(crate) async fn post_management_with(
        &self,
        path: &str,
        authorization: Authorization,
        body: Value,
    ) -> Response {
        let mut headers = vec![("content-type", "application/json".to_owned())];

        if let Some(value) = authorization.header_value() {
            headers.push(("authorization", value));
        }

        self.request("POST", path, &headers, Some(body.to_string()))
            .await
    }

    /// Collects a response body as lossy text.
    pub(crate) async fn body_text(&self, response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), BODY_READ_LIMIT)
            .await
            .expect("the response body reads");

        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// Collects a response body and parses it as JSON.
    pub(crate) async fn body_json(&self, response: Response) -> Result<Value, Box<dyn StdError>> {
        let bytes = axum::body::to_bytes(response.into_body(), BODY_READ_LIMIT).await?;

        Ok(serde_json::from_slice(&bytes)?)
    }
}

/// The `Authorization` credential a test request carries.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Authorization {
    /// The shared configuration's management token, sent verbatim — the documented spelling.
    Correct,

    /// A wrong token value, sent verbatim.
    WrongToken,

    /// The correct token sent with the `Bearer` prefix the contract forbids.
    BearerPrefixed,

    /// No `Authorization` header at all.
    Absent,
}

impl Authorization {
    /// The header value this credential sends, or `None` when it sends no header.
    fn header_value(self) -> Option<String> {
        match self {
            Self::Correct => Some(MANAGEMENT_TOKEN.to_owned()),

            Self::WrongToken => Some("not-the-management-token".to_owned()),

            Self::BearerPrefixed => Some(format!("Bearer {MANAGEMENT_TOKEN}")),

            Self::Absent => None,
        }
    }
}

/// Asserts the errno-shaped error envelope of a rejected management response: the exact status,
/// the exact numeric `errno`, and a body carrying exactly `errno` and a string `message`.
/// Returns the parsed body.
pub(crate) async fn assert_error_envelope(
    gateway: &TestGateway,
    response: Response,
    expected_status: http::StatusCode,
    expected_errno: u16,
) -> Result<Value, Box<dyn StdError>> {
    assert_eq!(response.status(), expected_status);

    let body = gateway.body_json(response).await?;

    assert_eq!(
        body.as_object().map(|fields| fields.len()),
        Some(2),
        "the error body carries exactly errno and message: {body}"
    );

    assert_eq!(
        *crate::test_support::at(&body, "/errno"),
        expected_errno,
        "{body}"
    );

    assert!(
        crate::test_support::at(&body, "/message").is_string(),
        "{body}"
    );

    Ok(body)
}
