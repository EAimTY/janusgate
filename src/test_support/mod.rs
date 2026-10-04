//! Test support: the crate-wide shared test infrastructure — the temporary file and directory
//! guards, the JSON-pointer read helper, and the database seeding fixtures — plus the re-exports
//! of the two fixture families beside it: the OAuth definition payload fixtures
//! ([`oauth_fixtures`]) and the mock upstream HTTP transport ([`mock_upstream`]).
//!
//! Every helper panics when its fixed fixture fails to parse, validate, or insert, or when a
//! JSON pointer read misses its path — a test bug by construction, so the panic is the failure
//! report.

#![expect(
    clippy::panic,
    reason = "a fixture that cannot parse, validate, or insert is a test bug, and the panic \
              is the failure report"
)]

mod mock_upstream;
mod oauth_fixtures;

pub(crate) use self::{
    mock_upstream::{MockAction, MockUpstream, RecordedHttpRequest, RecordedHttpRequests},
    oauth_fixtures::{
        FIXTURE_TOKEN_URL, OAuthDefinitionFixture, OAuthDeviceAuthorizationFixture,
        OAuthGrantFixture, OAuthRefreshFixture, OAuthResponseField, OAuthTarget,
        oauth_definition_payload,
    },
};
use crate::{
    database::Database,
    format::UpstreamFormat,
    provider::{Provider, ProviderDefinition, StoredCredentials, definition::DefinitionPayload},
    storage,
    storage::oauth_flows::{FlowGrant, FlowPurpose, NewFlow, OngoingFlow, insert_flow},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, num::NonZero, path::PathBuf, time::Duration};
use vec1::Vec1;

/// File guard cleaning up the temporary SQLite test database on drop.
pub(crate) struct TempDbGuard(PathBuf);

impl Drop for TempDbGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Directory guard removing the temporary test directory tree on drop.
pub(crate) struct TempDirGuard(pub(crate) PathBuf);

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The sealing key material of the shared test configuration: one valid 32-byte
/// key in standard base64, installed under the key id `test-primary`. Every test
/// configuration installer carries it, so whichever test wins the process-global
/// installation leaves the compaction sealing keys readable for the rest.
pub(crate) const FIXTURE_SEALING_KEY: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";

/// The second sealing key material of the shared test configuration, installed
/// under the key id `test-spare`: rotation and removed-key scenarios unseal
/// carriers sealed under [`FIXTURE_SEALING_KEY`] against the resolving set that
/// still knows both, and prove the failure when it no longer does.
pub(crate) const FIXTURE_SEALING_KEY_ROTATED: &str = "ICEiIyQlJicoKSorLC0uLzAxMjM0NTY3ODk6Ozw9Pj8=";

/// Installs the process-global configuration for the tests that read
/// [`crate::CONFIG`] — exactly one installation wins per process, so an
/// installer that arrives after another verifies the installed configuration
/// carries the compaction sealing keys every installer includes, and fails
/// loudly otherwise: a configuration installed without them cannot serve the
/// compaction tests, and the interference is the failure report.
///
/// # Panics
///
/// # Panics
///
/// Panics if the loader rejects its file, or when a configuration is already
/// installed without the compaction sealing keys.
pub(crate) fn set_global_configuration(loader: crate::config::ConfigLoader) {
    if crate::CONFIG.set(loader).is_err() {
        assert!(
            crate::CONFIG.compaction.is_some(),
            "the global test configuration is already installed without compaction sealing \
             keys; every test configuration installer must carry them"
        );
    }
}

/// The process-global test configuration: every test family that reads
/// [`crate::CONFIG`] installs the same canonical configuration — request
/// accounting on, the shared compaction sealing keys present — so exactly one
/// installation wins per process and every family observes a configuration it
/// can serve from, whatever the execution order.
///
/// The mutex serializing every configuration-sensitive region of the test
/// process: installation, sealing round-trips against the live configuration,
/// and the end-to-end scenario whose live reload mutates the installed
/// configuration. A caller holds the guard across its whole configuration
/// usage, so the reload scenario can never interleave with another test's
/// reads.
///
/// # Panics
///
/// Panics if the lock is poisoned — a prior holder panicked while holding it,
/// which is that test's own failure report.
pub(crate) fn configuration_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();

    #[expect(
        clippy::unwrap_used,
        reason = "a mutex poisoned by a panicking test is that test's failure report"
    )]
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap()
}

/// Installs the shared test configuration over the canonical per-process fixture
/// paths — the configuration directory's own `db.sqlite` and `api-logs` — for
/// the callers that need [`crate::CONFIG`] installed but carry no path facts of
/// their own: exactly one installation wins per process, and every consumer
/// derives its file locations from [`install_test_configuration`]'s return
/// value. Returns the installed configuration.
///
/// # Panics
///
/// Panics as [`install_test_configuration`] documents.
pub(crate) fn install_global_configuration() -> &'static crate::config::schema::Config {
    let directory = configuration_directory();

    std::fs::create_dir_all(&directory)
        .unwrap_or_else(|error| panic!("the fixture configuration directory is writable: {error}"));

    let api_logs_directory = directory.join("api-logs");

    let database_path = directory.join("db.sqlite");

    install_test_configuration(&database_path, Some(&api_logs_directory))
}

/// Enables the canonical accounting fixture for tests exercising real dispatch stamps.
/// Callers must hold [`configuration_lock`] throughout configuration-sensitive execution.
/// A prior recording-off installer or completed reload scenario may leave the shared switch off;
/// the live watcher enables it before any test dispatch instead of fabricating a draft stamp.
///
/// # Panics
///
/// Panics if the canonical fixture cannot be written or its watcher does not apply the reload.
pub(crate) async fn install_recording_configuration() {
    install_global_configuration();
    if crate::CONFIG.api_logs.is_some() {
        return;
    }
    let directory = configuration_directory().join("api-logs");
    std::fs::write(
        test_configuration_file_path(),
        configuration_body(&crate::CONFIG.database.path, Some(&directory)),
    )
    .unwrap_or_else(|error| panic!("the accounting fixture configuration writes: {error}"));
    tokio::time::timeout(Duration::from_secs(15), async {
        while crate::CONFIG.api_logs.is_none() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|error| panic!("the accounting fixture reload applies: {error}"));
}

/// The fixed per-process directory the shared test configuration lives in.
fn configuration_directory() -> PathBuf {
    std::env::temp_dir().join(format!("janusgate-test-config-{}", std::process::id()))
}

/// The configuration file path of the installed shared test configuration: the
/// file a live-reload scenario rewrites. Deterministic, so the winning
/// [`install_test_configuration`] call and every later rewrite address the same
/// file.
pub(crate) fn test_configuration_file_path() -> &'static std::path::Path {
    static PATH: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

    PATH.get_or_init(|| configuration_directory().join("config.jsonc"))
        .as_path()
}

/// Installs the shared process-global test configuration built over the given
/// database path and api-log directory — the same configuration body the
/// end-to-end scenario drives ([`configuration_body`]) — and returns the
/// installed configuration.
///
/// Exactly one installation wins per process: the winner installs the file
/// naming *its* paths, starts the reload watcher, and returns the fresh
/// configuration; every later caller — whatever paths it passed — receives the
/// already-installed configuration, whose named paths are the ones every
/// consumer must use. Callers therefore derive every file location from the
/// returned value, never from their own arguments. The `api_logs` section rides
/// only when `api_logs_directory` is given: the section's presence is the
/// recording switch.
///
/// # Panics
///
/// Panics as [`set_global_configuration`] documents, or when the configuration
/// file cannot be written or loaded — a broken fixture environment is the
/// failure report.
pub(crate) fn install_test_configuration(
    database_path: &std::path::Path,
    api_logs_directory: Option<&std::path::Path>,
) -> &'static crate::config::schema::Config {
    let directory = configuration_directory();

    std::fs::create_dir_all(&directory)
        .unwrap_or_else(|error| panic!("the fixture configuration directory is writable: {error}"));

    let config_path = test_configuration_file_path();

    if let Some(config) = crate::CONFIG.get() {
        let config: &'static crate::config::schema::Config = config;

        return config;
    }

    std::fs::write(
        config_path,
        configuration_body(database_path, api_logs_directory),
    )
    .unwrap_or_else(|error| panic!("the fixture configuration file writes: {error}"));

    let (loader, watcher) = crate::config::ConfigLoader::new(config_path.to_path_buf())
        .unwrap_or_else(|error| panic!("the fixture configuration loads: {error}"));

    set_global_configuration(loader);

    // The reload watcher needs a runtime of its own: the installing test
    // may be a synchronous unit test with none.
    let (ready_sender, ready_receiver) = std::sync::mpsc::sync_channel(0);
    std::thread::spawn(move || {
        #[expect(
            clippy::unwrap_used,
            reason = "a broken watcher runtime is the fixture failure report"
        )]
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        runtime.block_on(async move {
            use std::{future::Future as _, task::Poll};

            let mut watcher = Box::pin(watcher);
            // Its first poll registers filesystem watches before waiting for an event. Do not
            // let a reload fixture write its first update before that registration has finished.
            std::future::poll_fn(|context| {
                assert!(watcher.as_mut().poll(context).is_pending());
                Poll::Ready(())
            })
            .await;
            ready_sender
                .send(())
                .unwrap_or_else(|error| panic!("the watcher fixture announces readiness: {error}"));
            watcher.await;
        });
    });
    ready_receiver
        .recv()
        .unwrap_or_else(|error| panic!("the watcher fixture becomes ready: {error}"));

    #[expect(
        clippy::expect_used,
        reason = "the set above installed the loader, so the static reads back"
    )]
    let config: &'static crate::config::schema::Config = crate::CONFIG
        .get()
        .expect("the configuration was just installed");

    config
}

/// The canonical shared test configuration document over the given database
/// path: an ephemeral listen port, the management token the end-to-end
/// scenarios authenticate with, request accounting pointed at
/// `api_logs_directory` when given — the section's presence is the recording
/// switch — and the two shared compaction sealing keys.
pub(crate) fn configuration_body(
    database_path: &std::path::Path,
    api_logs_directory: Option<&std::path::Path>,
) -> String {
    let database_path = database_path.to_string_lossy().replace('\\', "/");

    let api_logs = api_logs_directory.map(|directory| {
        format!(
            ",\n                \"api_logs\": {{\"path\": \"{}\"}}",
            directory.to_string_lossy().replace('\\', "/")
        )
    });

    let api_logs = api_logs.as_deref().unwrap_or("");

    format!(
        r#"{{
                "listen": "127.0.0.1:0",
                "database": {{"path": "{database_path}"}},
                "management_api": {{"token": "e2e-token"}},
                "api": {{}}{api_logs},
                "compaction": {{"keys": [
                    {{"id": "test-primary", "key": "{primary}"}},
                    {{"id": "test-spare", "key": "{spare}"}}
                ]}}
            }}"#,
        primary = FIXTURE_SEALING_KEY,
        spare = FIXTURE_SEALING_KEY_ROTATED,
    )
}

/// Reads the JSON value at a pointer path of a rendered body.
///
/// # Panics
///
/// Panics if the path is absent: a missing path is the failure under test.
pub(crate) fn at<'a>(value: &'a Value, pointer: &str) -> &'a Value {
    #[expect(
        clippy::expect_used,
        reason = "rendered bodies are under test; a missing path is the failure"
    )]
    value
        .pointer(pointer)
        .expect("the rendered body carries the expected path")
}

/// Builds one non-empty list from a fixture slice.
///
/// # Panics
///
/// Panics on an empty slice, which no test fixture constructs.
pub(crate) fn list1<T: Clone>(values: &[T]) -> Vec1<T> {
    #[expect(clippy::unwrap_used, reason = "test fixtures are non-empty")]
    Vec1::try_from_vec(values.to_vec()).unwrap()
}

/// Creates an initialized temporary SQLite test database.
///
/// # Panics
///
/// Panics if opening the test database fails — a broken fixture environment is the failure
/// report.
pub(crate) async fn create_test_database() -> (Database, TempDbGuard) {
    let db_path = std::env::temp_dir().join(format!("janusgate-test-{}.db", rand::random::<u64>()));

    #[expect(clippy::unwrap_used, reason = "non-zero literal")]
    let max_connections = NonZero::new(2).unwrap();

    #[expect(clippy::unwrap_used, reason = "fresh test database opens cleanly")]
    let database = Database::open(
        &db_path,
        crate::database::schema::Registry::Main,
        max_connections,
        Duration::from_secs(5),
    )
    .await
    .unwrap();

    (database, TempDbGuard(db_path))
}

/// Parses one static-key definition payload of the given format — the in-memory fixture for
/// tests that need the payload without inserting it. Parsed without validation; callers that
/// store the payload validate it on the insert path.
///
/// # Panics
///
/// Panics if the payload does not parse — the fixed fixture payload always does.
pub(crate) fn static_key_definition_payload(
    format: crate::format::UpstreamFormat,
) -> DefinitionPayload {
    serde_json::from_value(json!({
        "format": format.as_str(),
        "inference": {
            "url": [{"type": "base"}, {"type": "literal", "value": "/chat/completions"}],
            "upstream_streaming": "match_client"
        },
        "credential": {"type": "api_key"},
        "discovery": {
            "request": {
                "method": "GET",
                "url": [{"type": "base"}, {"type": "literal", "value": "/models"}]
            },
            "extract": {"mode": "array", "array_at": ["data"], "id_field": ["id"]}
        }
    }))
    .unwrap_or_else(|error| panic!("fixture definition payload parses: {error}"))
}

/// Inserts one static-key definition version 1 of the given format and returns the stored
/// definition.
///
/// # Panics
///
/// Panics if the insert fails, the payload does not parse, or the fixture fails validation —
/// none of which can occur for the fixed fixture payloads.
pub(crate) async fn insert_static_key_definition(
    database: &Database,
    name: &str,
    format: crate::format::UpstreamFormat,
) -> ProviderDefinition {
    let payload = static_key_definition_payload(format);

    insert_definition(database, name, payload).await
}

/// Inserts one OAuth definition version 1 whose refresh grant fails against an unreachable
/// endpoint, for the pre-dispatch refresh-failure scenario.
///
/// # Panics
///
/// Panics if the insert fails or the fixture fails validation — neither can occur for the fixed
/// fixture payload.
pub(crate) async fn insert_failing_refresh_definition(
    database: &Database,
    name: &str,
) -> ProviderDefinition {
    let payload = oauth_definition_payload(OAuthDefinitionFixture {
        format: UpstreamFormat::GeminiGenerateContent,
        inference_suffix: None,
        client_id: "fixture-client",
        initial_slots: &[("project", "uuid_hex")],
        grant: OAuthGrantFixture::AuthorizationCode,
        exchange_url: FIXTURE_TOKEN_URL,
        response_fields: vec![OAuthResponseField::new(
            "refresh_token",
            OAuthTarget::RefreshToken,
        )],
        terminal_codes: &["invalid_grant"],
        refresh: OAuthRefreshFixture::Grant {
            url: FIXTURE_TOKEN_URL,
            response: None,
        },
        provisioning: None,
    });

    insert_definition(database, name, payload).await
}

/// Inserts one provider row of the given definition and credential, returning the stored row.
///
/// # Panics
///
/// Panics if the insert fails, or if the fixture credentials disagree with the definition's
/// credential kind — a test bug the assertion surfaces.
pub(crate) async fn insert_provider(
    database: &Database,
    definition: &ProviderDefinition,
    name: &str,
    base_url: Option<&str>,
    credentials: StoredCredentials,
) -> Provider {
    assert_eq!(
        credentials.kind(),
        definition.credential_kind(),
        "fixture credentials must match the definition's credential kind"
    );

    storage::providers::insert_provider(
        database,
        storage::providers::NewProvider {
            model_prefix: &format!("{name}-"),
            name,
            base_url,
            credentials: &credentials,
            definition,
            is_enabled: true,
        },
    )
    .await
    .unwrap_or_else(|error| panic!("fixture provider inserts: {error}"))
}

/// Inserts one OAuth definition version 1 — an authorization-code grant whose exchange maps the
/// access token and whose credential never renews — for the flow-begin and completion tests.
///
/// # Panics
///
/// Panics if the insert fails or the fixture fails validation — neither can occur for the fixed
/// fixture payload.
pub(crate) async fn insert_oauth_definition(database: &Database, name: &str) -> ProviderDefinition {
    let payload = oauth_definition_payload(OAuthDefinitionFixture {
        format: UpstreamFormat::OpenAiChatCompletion,
        inference_suffix: Some("/chat/completions"),
        client_id: "fixture-client",
        initial_slots: &[],
        grant: OAuthGrantFixture::AuthorizationCode,
        exchange_url: FIXTURE_TOKEN_URL,
        response_fields: vec![],
        terminal_codes: &["invalid_grant"],
        refresh: OAuthRefreshFixture::None,
        provisioning: None,
    });

    insert_definition(database, name, payload).await
}

/// The delta one flow-seeding test applies over the shared device-flow fixture base of
/// [`insert_flow_fixture`]: every field left `None` keeps the base value, and an override
/// replaces it whole. The two doubly wrapped fields carry a base the tests must clear (a
/// reauthorize flow names no provider), so their inner `Option` distinguishes "keep the base"
/// from "clear the field".
#[derive(Default)]
pub(crate) struct FlowFixtureDelta {
    /// Overrides the base state token `"state-token"`.
    pub(crate) state_token: Option<String>,

    /// Overrides the base purpose [`FlowPurpose::Create`].
    pub(crate) purpose: Option<FlowPurpose>,

    /// Overrides the base provider name `Some("created")`; `Some(None)` clears it.
    pub(crate) provider_name: Option<Option<String>>,

    /// Overrides the base target provider id `None`.
    pub(crate) target_provider_id: Option<i64>,

    /// Overrides the base device code `"device-code"`.
    pub(crate) device_code: Option<String>,

    /// Overrides the base user code `"user-code"`.
    pub(crate) device_user_code: Option<String>,

    /// Overrides the base stored poll interval of `5` seconds.
    pub(crate) device_poll_interval_seconds: Option<i64>,

    /// Overrides the base next-poll instant `now - 1` — the due-now schedule.
    pub(crate) device_next_poll_at: Option<i64>,

    /// Overrides the base expiry `now + 600` seconds — the ten-minute TTL.
    pub(crate) expires_at: Option<i64>,
}

/// Inserts one device-grant flow row against `definition` and returns the stored row: the
/// fixture base every flow-seeding test shares — a [`FlowPurpose::Create`] flow for a
/// `"created"` provider under the `"state-token"` token, pending with the 5-second interval,
/// due at `now - 1`, and expiring at `now + 600` seconds — narrowed by `delta`.
///
/// # Panics
///
/// Panics if the insert fails — the fixture row always satisfies the schema.
pub(crate) async fn insert_flow_fixture(
    database: &Database,
    definition: &ProviderDefinition,
    now: i64,
    delta: FlowFixtureDelta,
) -> OngoingFlow {
    let FlowFixtureDelta {
        state_token,
        purpose,
        provider_name,
        target_provider_id,
        device_code,
        device_user_code,
        device_poll_interval_seconds,
        device_next_poll_at,
        expires_at,
    } = delta;

    insert_flow(
        database,
        NewFlow {
            model_prefix: (purpose.unwrap_or(FlowPurpose::Create) == FlowPurpose::Create)
                .then(String::new),
            definition_name: definition.name.clone(),
            definition_version: definition.version,
            state_token: state_token.unwrap_or_else(|| "state-token".to_string()),
            purpose: purpose.unwrap_or(FlowPurpose::Create),
            provider_name: provider_name.unwrap_or_else(|| Some("created".to_string())),
            base_url: None,
            target_provider_id,
            grant_type: FlowGrant::Device,
            slots: BTreeMap::new(),
            code_verifier: None,
            device_code: Some(device_code.unwrap_or_else(|| "device-code".to_string())),
            device_user_code: Some(device_user_code.unwrap_or_else(|| "user-code".to_string())),
            device_verification_uri: Some("https://verify.example".to_string()),
            device_verification_uri_complete: None,
            device_poll_interval_seconds: Some(device_poll_interval_seconds.unwrap_or(5)),
            device_next_poll_at: Some(device_next_poll_at.unwrap_or(now - 1)),
            expires_at: expires_at.unwrap_or(now + 600 * crate::unix_time::MICROS_PER_SEC),
        },
    )
    .await
    .unwrap_or_else(|error| panic!("fixture flow inserts: {error}"))
}

/// Validates one fixture payload and inserts it as definition version 1 under `name`.
///
/// # Panics
///
/// Panics if the payload does not validate or the insert fails — neither can occur for the fixed
/// fixture payloads.
pub(crate) async fn insert_definition(
    database: &Database,
    name: &str,
    payload: DefinitionPayload,
) -> ProviderDefinition {
    #[expect(
        clippy::unwrap_used,
        reason = "the fixture definition always validates"
    )]
    payload.validate().unwrap();

    storage::provider_definitions::insert_definition(
        database,
        storage::provider_definitions::NewDefinition {
            name,
            base_version: None,
            payload: &payload,
            created_at: 1,
        },
    )
    .await
    .unwrap_or_else(|error| panic!("fixture definition inserts: {error}"))
}

/// Explicit broad-access policy for fixtures testing behavior other than permission admission.
pub(crate) fn allow_all_access() -> crate::access_control::ApiKeyAccessControl {
    crate::access_control::ApiKeyAccessControl {
        default_access: crate::access_control::AccessDecision::Allow,
        owner_defaults: Vec::new(),
        route_overrides: Vec::new(),
    }
}

/// A real enabled fixture key for structural route/listing tests, using production admission.
pub(crate) async fn structural_api_key(
    database: &crate::database::Database,
) -> Result<i64, crate::storage::Error> {
    if let Some(key) =
        crate::storage::api_keys::find_api_key_by_key(database, "structural-fixture-key").await?
    {
        return Ok(key.id);
    }
    Ok(crate::storage::api_keys::insert_api_key(
        database,
        &crate::storage::api_keys::ApiKeyFields {
            name: "Structural fixture",
            key: "structural-fixture-key",
            is_enabled: true,
            access_control: &allow_all_access(),
        },
    )
    .await?
    .id)
}
