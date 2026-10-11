//! Tests for the configuration loader: the real loading pipeline over temporary configuration
//! files — JSONC input, relative-path anchoring, TLS material resolution, the resolved
//! compaction sealing keys, the rendered CORS layers, and the debounced reload machinery
//! (`docs/reference/configuration.md`).

use super::{
    ConfigLoader,
    deserializers_tests::{KEY_MATERIAL_A, KEY_MATERIAL_B},
    error::Error,
};
use axum::body::Body;
use http::{
    HeaderMap, HeaderValue, Method, Request, Response,
    header::{ACCESS_CONTROL_ALLOW_ORIGIN, ACCESS_CONTROL_REQUEST_METHOD, ORIGIN},
};
use log::LevelFilter;
use std::{
    convert::Infallible,
    error::Error as StdError,
    future::{Future, Ready},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    task::{Context, Poll},
    time::Duration,
};
use tokio::sync::{Mutex, MutexGuard};
use tower::{Layer as _, Service};
use tower_http::cors::CorsLayer;

/// A self-signed P-256 certificate chain (one leaf) in PEM, matching [`PRIVATE_KEY_A`].
const CERTIFICATE_A: &str = r#"-----BEGIN CERTIFICATE-----
MIIBbTCCARKgAwIBAgIUQYO1Z0/b6Na9UMtRAWALd8vh4+8wCgYIKoZIzj0EAwIw
GzEZMBcGA1UEAwwQamFudXNnYXRlLXRlc3QtYTAgFw0yNjEwMTAxMTEwMjRaGA8y
MTI2MDkxNjExMTAyNFowGzEZMBcGA1UEAwwQamFudXNnYXRlLXRlc3QtYTBZMBMG
ByqGSM49AgEGCCqGSM49AwEHA0IABJJgT7aFH7wYqLEC5vAV7QXjOw+MO+HL2xMt
cgPH6+K/567t3uC5Kv7dA+iy3xQmWYGjIDmqlSid0z7Se62DrFyjMjAwMB0GA1Ud
DgQWBBSqWDW/GZGonWJcXM1SVq45HrYPtDAPBgNVHRMBAf8EBTADAQH/MAoGCCqG
SM49BAMCA0kAMEYCIQCdfqozB61R7Oza2m9TbN8a9mUz2nzk1UWX2N/bWsXseAIh
AJlbF46caMUj04uNP+dskMtBuyyWdVLLi0bVwqZlnd1Z
-----END CERTIFICATE-----
"#;

/// The P-256 private key in PEM matching [`CERTIFICATE_A`].
const PRIVATE_KEY_A: &str = r#"-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgwsFKfoa1IFTdn+Lp
Q2EMuLRr8FpJoKT4vbZm0o/SV/KhRANCAASSYE+2hR+8GKixAubwFe0F4zsPjDvh
y9sTLXIDx+viv+eu7d7guSr+3QPost8UJlmBoyA5qpUondM+0nutg6xc
-----END PRIVATE KEY-----
"#;

/// A second P-256 private key in PEM, intentionally not matching [`CERTIFICATE_A`] — its
/// certificate is never needed, only its mismatching key.
const PRIVATE_KEY_B: &str = r#"-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgcpKNO0Zh3Qnq3m4d
2gPLfXHB12YOVn+45BNxJgxT7KKhRANCAAStPjzXoype6YVrBrZZr3wDEQcUJ+/T
v7PQ2xXBhObEZSJ5SGtSbu+zUzwKw9Dvus+eddAHp8YX5XXSdIzLEcf/
-----END PRIVATE KEY-----
"#;

/// The budget one awaited reload may take: the 500 ms debounce window plus a margin for the
/// platform's filesystem-notification latency.
const RELOAD_BUDGET: Duration = Duration::from_secs(5);

/// The quiet period a rejected reload must survive without applying: several debounce windows,
/// so a wrongly applied reload cannot hide inside the notification latency.
const RELOAD_REJECT_QUIET_PERIOD: Duration = Duration::from_millis(1_600);

/// Creates a fresh, empty temporary directory for one test's fixtures, named after the test,
/// removing any stale directory an interrupted earlier run left behind.
fn fixture_directory(test: &str) -> Result<PathBuf, Box<dyn StdError>> {
    let directory = std::env::temp_dir().join(format!(
        "janusgate-config-test-{}-{test}",
        std::process::id()
    ));

    let _ = std::fs::remove_dir_all(&directory);

    std::fs::create_dir_all(&directory)?;

    Ok(directory)
}

/// Writes a configuration document to `path`, creating its parent directory.
fn write_document(path: &Path, body: &str) -> Result<(), Box<dyn StdError>> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    std::fs::write(path, body)?;

    Ok(())
}

/// Loads a configuration with a TLS section over the given material file contents, `None`
/// leaving a file absent, and asserts the load fails, returning its failure.
fn load_failing_tls(
    directory: &Path,
    certificate: Option<&str>,
    private_key: Option<&str>,
) -> Result<Error, Box<dyn StdError>> {
    if let Some(certificate) = certificate {
        std::fs::write(directory.join("cert.pem"), certificate)?;
    }

    if let Some(private_key) = private_key {
        std::fs::write(directory.join("key.pem"), private_key)?;
    }

    let path = directory.join("config.jsonc");

    write_document(
        &path,
        r#"{"tls": {"certificate": "cert.pem", "private_key": "key.pem"}}"#,
    )?;

    match ConfigLoader::new(path) {
        Ok(_) => Err(format!("the load must fail inside `{}`", directory.display()).into()),
        Err(failure) => Ok(failure),
    }
}

/// Serializes this module's reload scenarios: every applied reload moves the process-global log
/// filter to the applied generation's `log_level`, so concurrent reloads would corrupt each
/// other's observations of it.
async fn reload_lock() -> MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();

    LOCK.get_or_init(|| Mutex::new(())).lock().await
}

/// Spawns a loader's watcher future on the current runtime after registering its filesystem
/// watches, so a fixture rewrite afterwards is observed.
///
/// # Panics
///
/// Panics if the watcher future resolves on its first poll — it must run forever.
async fn spawn_watcher(watcher: impl Future<Output = ()> + Send + 'static) {
    let mut watcher = Box::pin(watcher);

    // The first poll runs the future to its first suspension point and registers the filesystem
    // watches; without it, a rewrite could land before any watch exists.
    std::future::poll_fn(|context| {
        assert!(
            watcher.as_mut().poll(context).is_pending(),
            "the watcher future never resolves"
        );

        Poll::Ready(())
    })
    .await;

    tokio::spawn(watcher);
}

/// Polls `condition` every 50 milliseconds until it holds, within [`RELOAD_BUDGET`].
async fn await_condition(condition: impl Fn() -> bool) -> Result<(), Box<dyn StdError>> {
    tokio::time::timeout(RELOAD_BUDGET, async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .map_err(|_| format!("the condition did not hold within {RELOAD_BUDGET:?}"))?;

    Ok(())
}

/// The empty configuration loads through the real pipeline: defaults fully applied with their
/// relative paths anchored beside the configuration file, the management API absent, and
/// neither TLS nor compaction sealing keys resolved.
///
/// # Panics
///
/// Panics when the behavior under test does not hold: the failed assertion is the test's failure
/// report.
#[tokio::test]
async fn empty_document_loads_with_defaults() -> Result<(), Box<dyn StdError>> {
    let directory = fixture_directory("empty")?;
    let path = directory.join("config.jsonc");

    write_document(&path, "{}")?;

    let (loader, _watcher) = ConfigLoader::new(path)?;

    assert_eq!(
        loader.listen,
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 4718)
    );
    assert_eq!(loader.log_level, LevelFilter::Info);
    assert_eq!(loader.database.path, directory.join("janusgate.db"));
    assert!(loader.management_api.is_none());
    assert!(loader.compaction.is_none());
    assert!(loader.api.logs.is_enabled);
    assert_eq!(loader.api.logs.path, directory.join("api_logs"));
    assert_eq!(loader.api.logs.retention_days.get(), 90);
    assert_eq!(loader.pooling.quota_refresh_interval_seconds.get(), 60);
    assert!(loader.tls().is_none());
    assert!(loader.sealing_keys().is_none());

    Ok(())
}

/// Relative database and accounting paths anchor against the configuration file's directory —
/// never the process working directory — and comments are stripped from the JSONC input, while
/// absolute paths are kept verbatim.
///
/// # Panics
///
/// Panics when the behavior under test does not hold: the failed assertion is the test's failure
/// report.
#[tokio::test]
async fn relative_paths_anchor_to_the_configuration_file() -> Result<(), Box<dyn StdError>> {
    let directory = fixture_directory("anchor")?;
    let path = directory.join("nested").join("config.jsonc");

    // The comments exercise the JSONC stripping in the real read pipeline.
    write_document(
        &path,
        r#"// The gateway's configuration.
{
    /* An ephemeral listener. */
    "listen": "127.0.0.1:0",
    "database": {"path": "data/main.db"},
    "api": {"logs": {"path": "logs"}}
}
"#,
    )?;

    let (loader, _watcher) = ConfigLoader::new(path)?;

    assert_eq!(
        loader.database.path,
        directory.join("nested").join("data/main.db")
    );
    assert_eq!(loader.api.logs.path, directory.join("nested").join("logs"));

    let absolute_directory = fixture_directory("anchor-absolute")?;
    let absolute_path = absolute_directory.join("elsewhere").join("main.db");
    let path = absolute_directory.join("config.jsonc");

    write_document(
        &path,
        &format!(
            r#"{{"database": {{"path": "{}"}}}}"#,
            absolute_path.display()
        ),
    )?;

    let (loader, _watcher) = ConfigLoader::new(path)?;

    assert_eq!(loader.database.path, absolute_path);

    Ok(())
}

/// TLS material resolves into a served TLS configuration through its anchored relative paths;
/// without a `tls` section the loader serves plaintext.
///
/// # Panics
///
/// Panics when the behavior under test does not hold: the failed assertion is the test's failure
/// report.
#[tokio::test]
async fn tls_material_resolves_and_absence_serves_plaintext() -> Result<(), Box<dyn StdError>> {
    let directory = fixture_directory("tls-valid")?;

    std::fs::write(directory.join("cert.pem"), CERTIFICATE_A)?;
    std::fs::write(directory.join("key.pem"), PRIVATE_KEY_A)?;

    let path = directory.join("config.jsonc");

    write_document(
        &path,
        r#"{"tls": {"certificate": "cert.pem", "private_key": "key.pem"}}"#,
    )?;

    let (loader, _watcher) = ConfigLoader::new(path)?;

    let tls = loader.tls();

    assert!(tls.is_some(), "the tls section must resolve");

    if let Some(tls) = tls {
        // The listener advertises both HTTP versions by ALPN, in preference order.
        assert_eq!(
            tls.alpn_protocols,
            vec![b"h2".to_vec(), b"http/1.1".to_vec()]
        );
    }

    let path = directory.join("plain.jsonc");

    write_document(&path, "{}")?;

    let (loader, _watcher) = ConfigLoader::new(path)?;

    assert!(loader.tls().is_none());

    Ok(())
}

/// TLS resolution failures fail the load and name the offending material: an unreadable file
/// with its anchored absolute path, a PEM section that does not decode as certificates, a file
/// holding no certificate section at all, and an unparseable private key.
///
/// # Panics
///
/// Panics when the behavior under test does not hold: the failed assertion is the test's failure
/// report.
#[tokio::test]
async fn tls_resolution_failures_name_the_failing_material() -> Result<(), Box<dyn StdError>> {
    let directory = fixture_directory("tls-missing")?;
    let failure = load_failing_tls(&directory, None, None)?;

    assert!(
        matches!(&failure, Error::Read { path, .. } if *path == directory.join("cert.pem")),
        "unexpected failure: {failure}"
    );

    let directory = fixture_directory("tls-garbage-certificate")?;
    let failure = load_failing_tls(
        &directory,
        Some("this is not a PEM certificate"),
        Some(PRIVATE_KEY_A),
    )?;

    assert!(
        matches!(
            &failure,
            Error::EmptyCertificateChain { path } if *path == directory.join("cert.pem")
        ),
        "unexpected failure: {failure}"
    );

    let directory = fixture_directory("tls-undecodable-certificate")?;
    let failure = load_failing_tls(
        &directory,
        Some("-----BEGIN CERTIFICATE-----\nnot base64!!\n-----END CERTIFICATE-----\n"),
        Some(PRIVATE_KEY_A),
    )?;

    assert!(
        matches!(
            &failure,
            Error::ParseCertificateChain { path, .. } if *path == directory.join("cert.pem")
        ),
        "unexpected failure: {failure}"
    );

    let directory = fixture_directory("tls-empty-certificate")?;
    let failure = load_failing_tls(&directory, Some(""), Some(PRIVATE_KEY_A))?;

    assert!(
        matches!(
            &failure,
            Error::EmptyCertificateChain { path } if *path == directory.join("cert.pem")
        ),
        "unexpected failure: {failure}"
    );

    let directory = fixture_directory("tls-garbage-key")?;
    let failure = load_failing_tls(
        &directory,
        Some(CERTIFICATE_A),
        Some("this is not a PEM key"),
    )?;

    assert!(
        matches!(
            &failure,
            Error::ParsePrivateKey { path, .. } if *path == directory.join("key.pem")
        ),
        "unexpected failure: {failure}"
    );

    // A PEM that decodes but holds no private key fails the key decoding, too.
    let directory = fixture_directory("tls-certificate-as-key")?;
    let failure = load_failing_tls(&directory, Some(CERTIFICATE_A), Some(CERTIFICATE_A))?;

    assert!(
        matches!(
            &failure,
            Error::ParsePrivateKey { path, .. } if *path == directory.join("key.pem")
        ),
        "unexpected failure: {failure}"
    );

    Ok(())
}

/// TLS material whose private key does not match the leaf certificate fails the TLS server
/// configuration build.
///
/// # Panics
///
/// Panics when the behavior under test does not hold: the failed assertion is the test's failure
/// report.
#[tokio::test]
async fn mismatched_key_and_certificate_fail_the_tls_build() -> Result<(), Box<dyn StdError>> {
    let directory = fixture_directory("tls-mismatched")?;
    let failure = load_failing_tls(&directory, Some(CERTIFICATE_A), Some(PRIVATE_KEY_B))?;

    assert!(
        matches!(&failure, Error::BuildTlsServerConfig { .. }),
        "unexpected failure: {failure}"
    );

    Ok(())
}

/// A configured compaction section resolves into the live sealing-key set, naming the configured
/// identifiers and never the secret material; its debug rendering likewise stays secret-free.
///
/// # Panics
///
/// Panics when the behavior under test does not hold: the failed assertion is the test's failure
/// report.
#[tokio::test]
async fn compaction_keys_resolve_into_the_live_set() -> Result<(), Box<dyn StdError>> {
    let directory = fixture_directory("compaction")?;
    let path = directory.join("config.jsonc");

    write_document(
        &path,
        &format!(
            r#"{{"compaction": {{"keys": [
                {{"id": "primary", "key": "{KEY_MATERIAL_A}"}},
                {{"id": "spare", "key": "{KEY_MATERIAL_B}"}}
            ]}}}}"#
        ),
    )?;

    let (loader, _watcher) = ConfigLoader::new(path)?;

    let keys = loader.sealing_keys();

    assert!(keys.is_some(), "the configured keys must resolve");

    if let Some(keys) = keys {
        let rendered = format!("{keys:?}");

        assert!(
            rendered.contains("\"primary\"") && rendered.contains("\"spare\""),
            "the rendering must name the configured identifiers: {rendered}"
        );

        assert!(
            !rendered.contains(KEY_MATERIAL_A) && !rendered.contains(KEY_MATERIAL_B),
            "the rendering must never carry the key material: {rendered}"
        );
    }

    Ok(())
}

/// The loader's debug rendering redacts the secrets it carries: the management token and the
/// compaction key material never appear.
///
/// # Panics
///
/// Panics when the behavior under test does not hold: the failed assertion is the test's failure
/// report.
#[tokio::test]
async fn loader_debug_rendering_redacts_secrets() -> Result<(), Box<dyn StdError>> {
    let directory = fixture_directory("redaction")?;
    let path = directory.join("config.jsonc");

    write_document(
        &path,
        &format!(
            r#"{{"management_api": {{"token": "hunter2-secret-token"}}, "compaction": {{"keys": [
                {{"id": "primary", "key": "{KEY_MATERIAL_A}"}}
            ]}}}}"#
        ),
    )?;

    let (loader, _watcher) = ConfigLoader::new(path)?;

    let rendered = format!("{loader:?}");

    assert!(
        !rendered.contains("hunter2-secret-token"),
        "the rendering must never carry the management token: {rendered}"
    );

    assert!(
        !rendered.contains(KEY_MATERIAL_A),
        "the rendering must never carry the key material: {rendered}"
    );

    assert!(
        rendered.contains("<secret:"),
        "the rendering must name the token's presence: {rendered}"
    );

    Ok(())
}

/// A schema rejection surfaces the deserializer's positioned failure verbatim: the offending
/// value's line and column, and an unknown field at startup just the same.
///
/// # Panics
///
/// Panics when the behavior under test does not hold: the failed assertion is the test's failure
/// report.
#[tokio::test]
async fn schema_rejection_carries_line_and_column() -> Result<(), Box<dyn StdError>> {
    let directory = fixture_directory("rejection-position")?;
    let path = directory.join("config.jsonc");

    write_document(
        &path,
        r#"{
    "listen": "127.0.0.1:4718",
    "compaction": {
        "keys": []
    }
}"#,
    )?;

    let failure = match ConfigLoader::new(path.clone()) {
        Ok(_) => return Err("the empty key set must be rejected".into()),
        Err(failure) => failure,
    };

    let Error::Parse { source, .. } = &failure else {
        return Err(format!("unexpected failure: {failure}").into());
    };

    assert_eq!(source.line(), 4, "the offending value's line: {failure}");
    assert_eq!(
        source.column(),
        18,
        "the offending value's column: {failure}"
    );

    let path = directory.join("unknown-field.jsonc");

    write_document(&path, r#"{"bogus": true}"#)?;

    let failure = match ConfigLoader::new(path.clone()) {
        Ok(_) => return Err("the unknown field must be rejected".into()),
        Err(failure) => failure,
    };

    assert!(
        matches!(&failure, Error::Parse { .. }),
        "unexpected failure: {failure}"
    );

    Ok(())
}

/// A rewritten configuration file applies as a new generation after the debounce window: the
/// listener address and the anchored database path change together.
///
/// # Panics
///
/// Panics when the behavior under test does not hold: the failed assertion is the test's failure
/// report.
#[tokio::test]
async fn reload_applies_the_rewritten_generation() -> Result<(), Box<dyn StdError>> {
    let _reload_lock = reload_lock().await;

    let directory = fixture_directory("reload-apply")?;
    let path = directory.join("config.jsonc");

    write_document(
        &path,
        r#"{"listen": "127.0.0.1:4701", "database": {"path": "one.db"}}"#,
    )?;

    let (loader, watcher) = ConfigLoader::new(path.clone())?;

    spawn_watcher(watcher).await;

    assert_eq!(
        loader.listen,
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 4701)
    );

    write_document(
        &path,
        r#"{"listen": "127.0.0.1:4702", "database": {"path": "two.db"}}"#,
    )?;

    await_condition(|| loader.listen.port() == 4702).await?;

    assert_eq!(loader.database.path, directory.join("two.db"));

    Ok(())
}

/// A rejected reload keeps the previously applied generation and the watcher alive: a bad body
/// never takes a running gateway down, and a later valid rewrite applies.
///
/// # Panics
///
/// Panics when the behavior under test does not hold: the failed assertion is the test's failure
/// report.
#[tokio::test]
async fn rejected_reload_keeps_the_previous_generation() -> Result<(), Box<dyn StdError>> {
    let _reload_lock = reload_lock().await;

    let directory = fixture_directory("reload-rejected")?;
    let path = directory.join("config.jsonc");

    write_document(&path, r#"{"listen": "127.0.0.1:4711"}"#)?;

    let (loader, watcher) = ConfigLoader::new(path.clone())?;

    spawn_watcher(watcher).await;

    write_document(&path, r#"{"listen": "127.0.0.1:4712", "bogus": true}"#)?;

    // The rejected reload must have been attempted and failed: wait out the debounce window
    // several times over, then assert the previous generation still serves.
    tokio::time::sleep(RELOAD_REJECT_QUIET_PERIOD).await;

    assert_eq!(
        loader.listen,
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 4711)
    );

    write_document(&path, r#"{"listen": "127.0.0.1:4713"}"#)?;

    await_condition(|| loader.listen.port() == 4713).await?;

    Ok(())
}

/// The TLS configuration reloads as part of its generation: a rewrite to unresolvable material
/// keeps the previous generation — plaintext stays plaintext — a valid rewrite installs the new
/// TLS configuration, and a rewrite back to no `tls` section returns to plaintext.
///
/// # Panics
///
/// Panics when the behavior under test does not hold: the failed assertion is the test's failure
/// report.
#[tokio::test]
async fn tls_reloads_with_its_generation() -> Result<(), Box<dyn StdError>> {
    let _reload_lock = reload_lock().await;

    let directory = fixture_directory("reload-tls")?;
    let path = directory.join("config.jsonc");

    write_document(&path, "{}")?;

    let (loader, watcher) = ConfigLoader::new(path.clone())?;

    spawn_watcher(watcher).await;

    assert!(loader.tls().is_none());

    write_document(
        &path,
        r#"{"tls": {"certificate": "cert.pem", "private_key": "key.pem"}}"#,
    )?;

    tokio::time::sleep(RELOAD_REJECT_QUIET_PERIOD).await;

    assert!(
        loader.tls().is_none(),
        "the failed reload must keep plaintext"
    );

    std::fs::write(directory.join("cert.pem"), CERTIFICATE_A)?;
    std::fs::write(directory.join("key.pem"), PRIVATE_KEY_A)?;

    await_condition(|| loader.tls().is_some()).await?;

    write_document(&path, "{}")?;

    await_condition(|| loader.tls().is_none()).await?;

    Ok(())
}

/// The process-global log filter follows the reloaded generation's `log_level`.
///
/// # Panics
///
/// Panics when the behavior under test does not hold: the failed assertion is the test's failure
/// report.
#[tokio::test]
async fn reload_moves_the_global_log_filter() -> Result<(), Box<dyn StdError>> {
    let _reload_lock = reload_lock().await;

    let directory = fixture_directory("reload-log-level")?;
    let path = directory.join("config.jsonc");

    write_document(&path, r#"{"log_level": "info"}"#)?;

    let (loader, watcher) = ConfigLoader::new(path.clone())?;

    spawn_watcher(watcher).await;

    write_document(&path, r#"{"log_level": "trace"}"#)?;

    await_condition(|| loader.log_level == LevelFilter::Trace).await?;

    // The filter moves in the same apply as the schema.
    assert_eq!(log::max_level(), LevelFilter::Trace);

    write_document(&path, r#"{"log_level": "warn"}"#)?;

    await_condition(|| loader.log_level == LevelFilter::Warn).await?;

    assert_eq!(log::max_level(), LevelFilter::Warn);

    Ok(())
}

/// Adding and removing the `management_api` section changes the applied schema generation on
/// reload. Mounting the routes is a startup decision, so the routes themselves are outside this
/// test's scope — what the loader must do is carry the new section into the live generation.
///
/// # Panics
///
/// Panics when the behavior under test does not hold: the failed assertion is the test's failure
/// report.
#[tokio::test]
async fn management_section_changes_apply_to_the_schema() -> Result<(), Box<dyn StdError>> {
    let _reload_lock = reload_lock().await;

    let directory = fixture_directory("reload-management")?;
    let path = directory.join("config.jsonc");

    write_document(&path, "{}")?;

    let (loader, watcher) = ConfigLoader::new(path.clone())?;

    spawn_watcher(watcher).await;

    assert!(loader.management_api.is_none());

    write_document(&path, r#"{"management_api": {"token": "rotated-token"}}"#)?;

    await_condition(|| loader.management_api.is_some()).await?;

    assert!(
        loader
            .management_api
            .as_ref()
            .is_some_and(|section| section.token == "rotated-token"),
        "the applied section must carry the configured token"
    );

    write_document(&path, "{}")?;

    await_condition(|| loader.management_api.is_none()).await?;

    Ok(())
}

/// The rendered CORS layers apply the configured origin policy: the API layer allows exactly its
/// configured origins, the management layer its own wildcard, a management section absent at
/// startup falls back to the restrictive default, and Private Network Access is granted only
/// when enabled and requested.
///
/// # Panics
///
/// Panics when the behavior under test does not hold: the failed assertion is the test's failure
/// report.
#[tokio::test]
async fn cors_layers_apply_the_configured_policy() -> Result<(), Box<dyn StdError>> {
    let directory = fixture_directory("cors")?;
    let path = directory.join("config.jsonc");

    write_document(
        &path,
        r#"{
    "management_api": {
        "token": "t",
        "cors": {"allowed_origin": "*"}
    },
    "api": {
        "cors": {
            "allowed_origin": ["https://a.example", "https://b.example"],
            "allow_private_network": true
        }
    }
}"#,
    )?;

    let (loader, _watcher) = ConfigLoader::new(path)?;

    let allowed = preflight(&loader.api_cors_layer(), "https://a.example", true).await?;

    assert_eq!(
        allowed.get(ACCESS_CONTROL_ALLOW_ORIGIN),
        Some(&HeaderValue::from_static("https://a.example"))
    );

    assert_eq!(
        allowed.get("access-control-allow-private-network"),
        Some(&HeaderValue::from_static("true"))
    );

    let denied = preflight(&loader.api_cors_layer(), "https://c.example", false).await?;

    assert_eq!(denied.get(ACCESS_CONTROL_ALLOW_ORIGIN), None);

    assert_eq!(denied.get("access-control-allow-private-network"), None);

    let management = preflight(
        &loader.management_api_cors_layer(),
        "https://anywhere.example",
        false,
    )
    .await?;

    assert_eq!(
        management.get(ACCESS_CONTROL_ALLOW_ORIGIN),
        Some(&HeaderValue::from_static("*"))
    );

    let directory = fixture_directory("cors-restrictive")?;
    let path = directory.join("config.jsonc");

    write_document(&path, "{}")?;

    let (loader, _watcher) = ConfigLoader::new(path)?;

    let restrictive = preflight(
        &loader.management_api_cors_layer(),
        "https://a.example",
        true,
    )
    .await?;

    assert_eq!(restrictive.get(ACCESS_CONTROL_ALLOW_ORIGIN), None);

    assert_eq!(
        restrictive.get("access-control-allow-private-network"),
        None
    );

    Ok(())
}

/// Sends a CORS preflight request with the given origin through `layer`, optionally requesting
/// Private Network Access, and returns the response headers.
///
/// # Panics
///
/// Panics when the wrapped service is not ready for the preflight request.
async fn preflight(
    layer: &CorsLayer,
    origin: &'static str,
    private_network: bool,
) -> Result<HeaderMap, Box<dyn StdError>> {
    /// The inner service every layer under test wraps: it answers every request with an empty
    /// response, so only the layer's own headers are observed.
    struct EmptyResponder;

    impl Service<Request<Body>> for EmptyResponder {
        type Response = Response<Body>;
        type Error = Infallible;
        type Future = Ready<Result<Response<Body>, Infallible>>;

        fn poll_ready(&mut self, _context: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, _request: Request<Body>) -> Self::Future {
            std::future::ready(Ok(Response::new(Body::empty())))
        }
    }

    let mut service = layer.layer(EmptyResponder);

    let mut builder = Request::builder()
        .method(Method::OPTIONS)
        .uri("https://gateway.example/api")
        .header(ORIGIN, origin)
        .header(ACCESS_CONTROL_REQUEST_METHOD, "POST");

    if private_network {
        builder = builder.header("access-control-request-private-network", "true");
    }

    let request = builder.body(Body::empty())?;

    // The Service contract: readiness precedes the call.
    std::future::poll_fn(|context| {
        assert!(service.poll_ready(context).is_ready());

        Poll::Ready(())
    })
    .await;

    let response = match service.call(request).await {
        Ok(response) => response,
        // The responder never fails.
        Err(never) => match never {},
    };

    Ok(response.headers().clone())
}
