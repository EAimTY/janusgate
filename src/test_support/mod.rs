//! Test support: the crate-wide shared test infrastructure. Only machinery that more than one
//! test family needs lives here — the process-global test configuration installer, the
//! temporary test database, the JSON pointer read helper, and the mock upstream HTTP transport
//! ([`mock_upstream`]). A helper needed by a single test family belongs beside that family's
//! tests, not here.

#![expect(
    clippy::panic,
    reason = "a fixture that cannot parse, install, or serve is a test bug, and the panic \
              is the failure report"
)]

pub(crate) mod mock_upstream;

use crate::database::{Database, MAIN_POOL_WAIT, schema::Registry};
use serde_json::Value;
use std::{num::NonZero, path::PathBuf, time::Duration};

/// The sealing key material of the shared test configuration, installed under the key id
/// `test-primary`: one valid 32-byte key in standard base64. The first installer wins the
/// process-global configuration, so every installer carries the same keys and whichever test
/// wins leaves the compaction sealing keys readable for the rest of the process.
pub(crate) const SEALING_KEY_PRIMARY: &str = "KywtLi8wMTIzNDU2Nzg5Ojs8PT4/QEFCQ0RFRkdISUo=";

/// The second sealing key material of the shared test configuration, installed under the key id
/// `test-spare`: rotation scenarios unseal carriers sealed under [`SEALING_KEY_PRIMARY`] against
/// the resolving set that still knows both keys.
pub(crate) const SEALING_KEY_SECONDARY: &str = "Y2RlZmdoaWprbG1ub3BxcnN0dXZ3eHl6e3x9fn+AgYI=";

/// The management token of the shared test configuration, which the management-API scenarios
/// authenticate with.
pub(crate) const MANAGEMENT_TOKEN: &str = "test-management-token";

/// The mutex serializing every configuration-sensitive region of the test process:
/// installation, sealing round-trips against the live configuration, and any scenario whose
/// live reload mutates the installed configuration. A caller holds the guard across its whole
/// configuration usage, so a reload scenario can never interleave with another test's reads.
///
/// # Panics
///
/// Panics if the lock is poisoned — a prior holder panicked while holding it, which is that
/// test's own failure report.
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

/// Installs the shared process-global test configuration and returns the applied schema: the
/// canonical body ([`configuration_body`]) written to a fixed per-process temporary path,
/// loaded through the real loading pipeline, and installed into [`crate::CONFIG`].
///
/// Exactly one installation wins per process. Every later caller receives the already-installed
/// configuration, so every consumer derives its file locations from the returned value and from
/// [`configuration_file_path`] instead of assuming its own paths won. The installer that wins
/// also starts the reload watcher on a dedicated thread, so a later live-reload scenario rewrites
/// [`configuration_file_path`] and the change applies.
///
/// The canonical body enables request accounting, configures the management API, and carries
/// both sealing keys; a test family needing different content holds [`configuration_lock`],
/// rewrites [`configuration_file_path`], and awaits the reload with [`await_reload`].
///
/// # Panics
///
/// Panics if the fixture directory or file cannot be written, the loader rejects the body, or
/// an already-installed configuration lacks the sealing keys — the latter means an installer
/// disobeyed the shared-body contract, and the interference is the failure report.
pub(crate) fn install_configuration() -> &'static crate::config::schema::Config {
    let directory = configuration_directory();

    std::fs::create_dir_all(&directory)
        .unwrap_or_else(|error| panic!("the fixture configuration directory is writable: {error}"));

    if let Some(loader) = crate::CONFIG.get() {
        assert!(
            loader.sealing_keys().is_some(),
            "the global test configuration is already installed without compaction sealing \
             keys; every installer must write the shared body"
        );

        return loader;
    }

    std::fs::write(
        configuration_file_path(),
        configuration_body(
            &directory.join("db.sqlite"),
            Some(&directory.join("api-logs")),
        ),
    )
    .unwrap_or_else(|error| panic!("the fixture configuration file writes: {error}"));

    let (loader, watcher) = crate::config::ConfigLoader::new(configuration_file_path())
        .unwrap_or_else(|error| panic!("the fixture configuration loads: {error}"));

    crate::CONFIG
        .set(loader)
        .unwrap_or_else(|_| panic!("the configuration installation raced and lost"));

    // The reload watcher needs a runtime of its own: the installing test may be a synchronous
    // unit test with none.
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

            // The first poll registers the filesystem watches before any event is expected.
            // Without this, a reload fixture could rewrite the file before the watch exists and
            // the reload would never apply.
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

/// The fixed per-process directory the shared test configuration and its database and api-log
/// fixtures live in.
pub(crate) fn configuration_directory() -> PathBuf {
    std::env::temp_dir().join(format!("janusgate-test-config-{}", std::process::id()))
}

/// The configuration file path of the installed shared test configuration: the file a
/// live-reload scenario rewrites. Deterministic, so the winning [`install_configuration`] call
/// and every later rewrite address the same file.
pub(crate) fn configuration_file_path() -> PathBuf {
    configuration_directory().join("config.jsonc")
}

/// The canonical shared test configuration document over the given database path: an ephemeral
/// listen port, the shared management token, request accounting pointed at `api_logs_directory`
/// when given, and the two shared sealing keys.
pub(crate) fn configuration_body(
    database_path: &std::path::Path,
    api_logs_directory: Option<&std::path::Path>,
) -> String {
    let database_path = database_path.to_string_lossy().replace('\\', "/");

    let logs = api_logs_directory
        .map(|directory| {
            let directory = directory.to_string_lossy().replace('\\', "/");

            format!(",\n        \"logs\": {{\"is_enabled\": true, \"path\": \"{directory}\"}}")
        })
        .unwrap_or_default();

    format!(
        r#"{{
    "listen": "127.0.0.1:0",
    "database": {{"path": "{database_path}"}},
    "management_api": {{"token": "{MANAGEMENT_TOKEN}"}},
    "api": {{"cors": {{"allowed_origin": "*"}}{logs}}},
    "compaction": {{"keys": [
        {{"id": "test-primary", "key": "{primary}"}},
        {{"id": "test-spare", "key": "{secondary}"}}
    ]}}
}}"#,
        primary = SEALING_KEY_PRIMARY,
        secondary = SEALING_KEY_SECONDARY,
    )
}

/// Awaits a debounced reload the shared fixture watcher applies after
/// [`configuration_file_path`] is rewritten: polls `predicate` every 10 milliseconds until it
/// holds, within a fifteen-second budget — the reload's 500-millisecond debounce window plus a
/// generous margin. Callers must hold [`configuration_lock`] from before the rewrite through the
/// await, so no other test's rewrite interleaves.
///
/// # Panics
///
/// Panics when `predicate` never holds within the budget — an unapplied fixture reload is the
/// test's failure report.
pub(crate) async fn await_reload(predicate: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(15), async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("the fixture configuration reload applies within the budget"));
}

/// File guard removing the temporary SQLite test database and its engine sidecar files on drop.
pub(crate) struct TempDatabaseGuard(PathBuf);

impl Drop for TempDatabaseGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);

        let mut sidecar = self.0.clone().into_os_string();
        sidecar.push("-wal");
        let _ = std::fs::remove_file(&sidecar);

        let mut sidecar = self.0.clone().into_os_string();
        sidecar.push("-shm");
        let _ = std::fs::remove_file(&sidecar);
    }
}

/// Opens a fresh temporary main-registry test database and returns it beside its drop guard.
///
/// # Panics
///
/// Panics if opening the test database fails — a broken fixture environment is the failure
/// report.
pub(crate) async fn temp_database() -> (Database, TempDatabaseGuard) {
    let path = std::env::temp_dir().join(format!("janusgate-test-{}.db", rand::random::<u64>()));

    #[expect(clippy::unwrap_used, reason = "non-zero literal")]
    let connections = NonZero::new(2).unwrap();

    #[expect(clippy::unwrap_used, reason = "a fresh test database opens cleanly")]
    let database = Database::open(&path, Registry::Main, connections, MAIN_POOL_WAIT)
        .await
        .unwrap();

    (database, TempDatabaseGuard(path))
}

/// Reads the JSON value at a pointer path of a body under test.
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
        .expect("the body under test carries the expected path")
}
