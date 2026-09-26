//! Gateway configuration: the parsed schema
//! ([`schema::Config`]), the reloadable owner
//! ([`ConfigLoader`]), and the loading, hot-reload, TLS, and CORS machinery.
//!
//! [`ConfigLoader`] owns the live state — the applied configuration generation, the schema and the
//! TLS server configuration resolved from it as one value ([`AppliedConfig`]) — and the reload
//! pipeline. Its watcher future, returned by [`ConfigLoader::new`] for the caller to spawn, watches
//! the configuration file and the configured TLS material through the platform's native filesystem
//! notifications; a reload is debounced — it applies once the watched inputs have been quiet for a
//! fixed window — and a reload that fails for any reason keeps the previously applied state and
//! logs the failure. Only startup is fatal.

use self::schema::Config;
use arc_swap::ArcSwap;
use futures::{future, future::Either};
use json_strip_comments::StripComments;
use log::{error, info, warn};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher as _};
use rustls::ServerConfig;
use std::{
    fs,
    future::Future,
    ops::Deref,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

mod cors;
mod defaults;
mod deserializers;
mod error;
mod schema;
mod tls;

pub(crate) use self::error::Error;

/// Quiet period the watched inputs must stay silent for before a debounced reload applies — long
/// enough that an editor's write-rename chain, a certificate rotation writing several files, or
/// notify's event bursts all coalesce into one final apply, and imperceptible on a human time
/// scale.
const RELOAD_DEBOUNCE: Duration = Duration::from_millis(500);

/// One applied configuration generation: the schema and the TLS server configuration resolved from
/// it, packed into a single value so [`ConfigLoader`] can swap both with one store. Every reader —
/// the [`Deref`]-based schema reads and [`ConfigLoader::tls`] alike — therefore observes exactly
/// one generation, and a reload can never present a new schema with the previous generation's TLS
/// material, or the reverse (README §8).
#[derive(Debug)]
struct AppliedConfig {
    /// The applied schema. Deliberately leaked: a dereferenced schema reference is borrowed from
    /// [`ConfigLoader`], a process-lifetime static, so an applied schema must outlive it — no
    /// schema is ever freed, at a cost of a few hundred bytes per reload. The leak is what lets
    /// [`Deref::deref`] hand out a plain `&Config` with no lock, guard, or copy: the generation
    /// handle keeps only the shell below alive, while the schema reference itself is `'static`.
    schema: &'static Config,

    /// The TLS server configuration resolved from [`AppliedConfig::schema`]; `None` serves
    /// plaintext HTTP. Refcounted: dropping a superseded generation releases the superseded
    /// configuration's last owning reference once the connections accepted under it finish, so
    /// reloads never accumulate.
    tls_server_config: Option<Arc<ServerConfig>>,
}

/// The gateway's live configuration loader: the currently applied configuration generation
/// ([`AppliedConfig`]) and the reload pipeline that keeps it current with its files.
///
/// Schema reads are plain dereferences — `crate::CONFIG.field` — through [`Deref`]; TLS reads go
/// through [`ConfigLoader::tls`]. Both read the one generation held in the loader's single
/// [`ArcSwap`] cell, so they never disagree, and a reload applies with one store.
#[derive(Clone, Debug)]
pub(crate) struct ConfigLoader {
    /// Path of the configuration file, as given on the command line.
    path: PathBuf,

    /// The currently applied configuration generation; [`AppliedConfig`] documents the lifetime of
    /// its halves.
    applied: Arc<ArcSwap<AppliedConfig>>,
}

/// Reads the schema as plain fields: `crate::CONFIG.management_api.token` dereferences the static,
/// the loader, and the schema in one expression.
impl Deref for ConfigLoader {
    type Target = Config;

    fn deref(&self) -> &Self::Target {
        // The generation handle keeps the generation shell alive while the field is read; the
        // schema reference itself is `'static` — an applied schema is deliberately leaked, see
        // [`AppliedConfig`] — so the copied reference stays valid for the rest of the process and
        // no unsafe dereference is needed.
        self.applied.load_full().schema
    }
}

impl ConfigLoader {
    /// Loads the configuration from `path` — reads and parses the schema and resolves its TLS
    /// material — and returns the loader together with its watcher future, which the caller spawns.
    ///
    /// # Errors
    ///
    /// Returns the loading failure: an unreadable input, a schema the deserializer rejects, or TLS
    /// material that does not resolve.
    pub(crate) fn new(path: PathBuf) -> Result<(Self, impl Future<Output = ()> + Send), Error> {
        let applied = read_inputs(&path)?;

        let loader = Self {
            path,
            applied: Arc::new(ArcSwap::from_pointee(applied)),
        };

        let watcher = loader.clone().watch();

        Ok((loader, watcher))
    }

    /// The watcher future: watches — through the platform's native filesystem notification API —
    /// the parent directories of the configuration file and of the currently configured TLS
    /// material files, so editors that replace files by rename are seen.
    ///
    /// Reloads are debounced: an event opens a [`RELOAD_DEBOUNCE`] quiet window, every further
    /// event restarts it, and one unconditional reload ([`ConfigLoader::reload`]) runs once the
    /// inputs fall quiet — so an editor's write-rename chain, or any other intermediate state,
    /// never reaches the parser. Watched directories re-arm after every reload, so a reload that
    /// changes the TLS paths takes effect. The future runs forever and never resolves.
    ///
    /// # Panics
    ///
    /// Panics if the platform file watcher cannot be created, or if the event channel closes — the
    /// future owns the watcher holding the sending half, so that never happens while it runs.
    async fn watch(self) {
        let (event_sender, mut event_receiver) = tokio::sync::mpsc::unbounded_channel();

        #[expect(
            clippy::expect_used,
            reason = "creating the platform file watcher at startup is fatal on failure"
        )]
        let mut watcher = notify::recommended_watcher(event_sender)
            .expect("cannot create the configuration file watcher");

        let mut watched = Vec::new();
        resync_watches(&mut watcher, &mut watched, watch_dirs(&self.path, &self));

        loop {
            // The first event of a burst opens the window.
            handle_event(event_receiver.recv().await);

            // The window: race the next event against a fresh sleep. An event makes the next
            // iteration's sleep the replacement — the old one is dropped at the end of this
            // iteration, cancelling it — so at most one sleep is ever alive, and only its expiry,
            // the inputs staying quiet for the whole window, lets the reload run.
            loop {
                let sleep = std::pin::pin!(tokio::time::sleep(RELOAD_DEBOUNCE));
                let next_event = std::pin::pin!(event_receiver.recv());

                match future::select(sleep, next_event).await {
                    Either::Left(((), _superseded_event)) => break,

                    Either::Right((event, _superseded_sleep)) => handle_event(event),
                }
            }

            self.reload();
            resync_watches(&mut watcher, &mut watched, watch_dirs(&self.path, &self));
        }
    }

    /// Reads the configuration file and its TLS material and applies them: parse, resolve, and —
    /// only when everything succeeded — swap the one configuration generation in a single store,
    /// moving the global log filter to the new `log_level`.
    ///
    /// A configuration whose TLS material does not resolve keeps the previous schema as well, and
    /// any failure keeps the previously applied state and logs the failure — a running gateway
    /// never dies over a reload.
    fn reload(&self) {
        match read_inputs(&self.path) {
            Ok(applied) => {
                log::set_max_level(applied.schema.log_level);

                // One store: the schema and the TLS server configuration are one generation and
                // swap together, so no reader ever observes a half-applied reload (README §8).
                self.applied.store(Arc::new(applied));

                info!("applied the configuration");
            }

            Err(failure) => {
                error!("failed to reload the configuration, keeping the previous one: {failure}");
            }
        }
    }
}

/// The one configuration loading pipeline shared by startup ([`ConfigLoader::new`]) and every
/// reload ([`ConfigLoader::reload`]): read the configuration file at `path`, strip its comments,
/// deserialize the schema strictly, and resolve the TLS material into a Rustls server
/// configuration, packed into one configuration generation.
///
/// # Errors
///
/// Returns the loading failure: an unreadable input, a schema the deserializer rejects, or TLS
/// material that does not resolve.
fn read_inputs(path: &Path) -> Result<AppliedConfig, Error> {
    let configuration = fs::read(path).map_err(|source| Error::Read {
        path: path.to_path_buf(),
        source,
    })?;

    let schema: Config = serde_json::from_reader(StripComments::new(configuration.as_slice()))
        .map_err(|source| Error::Parse {
            path: path.to_path_buf(),
            source,
        })?;

    let tls_server_config = self::tls::resolve(&schema)?;

    // Leak the schema: dereferenced references borrow the process-lifetime loader, so every
    // applied schema must outlive it (see [`AppliedConfig`]). Leaked only past the resolution
    // above, so a failed load drops the schema instead of leaking it.
    Ok(AppliedConfig {
        schema: Box::leak(Box::new(schema)),
        tls_server_config,
    })
}

/// Records one filesystem event's arrival: the event's shape is ignored — every event re-arms the
/// reload window — and only a backend failure is worth a log record.
///
/// # Panics
///
/// Panics if the event channel closed, which the watcher future's ownership of the sending half
/// makes impossible while it runs.
fn handle_event(event: Option<notify::Result<Event>>) {
    #[expect(
        clippy::panic,
        reason = "the watcher future owns the sending half, so the channel never closes \
                  while it runs"
    )]
    match event {
        Some(Ok(_event)) => {}

        Some(Err(notification)) => {
            warn!("configuration file watching failed: {notification}");
        }

        None => panic!("the configuration event channel closed"),
    }
}

/// Resolves the parent directory of a file path: the current directory for a bare filename, so it
/// is watchable.
fn parent_directory(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

/// Directories to watch for the inputs of a schema: the parent directories of the configuration
/// file and of each TLS material file, canonicalized and deduplicated. A directory that cannot be
/// canonicalized is skipped with a warning — its files keep serving, only their changes go
/// unnoticed.
fn watch_dirs(path: &Path, schema: &Config) -> Vec<PathBuf> {
    let mut parents = vec![parent_directory(path)];

    if let Some(tls) = schema.tls.as_ref() {
        parents.extend([
            parent_directory(&tls.certificate),
            parent_directory(&tls.private_key),
        ]);
    }

    let mut directories = Vec::new();

    for parent in parents {
        match fs::canonicalize(parent) {
            Ok(directory) => {
                if !directories.contains(&directory) {
                    directories.push(directory);
                }
            }

            Err(failure) => {
                warn!(
                    "cannot watch `{}` for configuration changes: {failure}",
                    parent.display()
                );
            }
        }
    }

    directories
}

/// Reconciles the watcher's watched directories with `desired`: stops watching directories no input
/// lives in and starts watching directories of newly configured inputs.
fn resync_watches(
    watcher: &mut RecommendedWatcher,
    watched: &mut Vec<PathBuf>,
    desired: Vec<PathBuf>,
) {
    watched.retain(|directory| {
        if desired.contains(directory) {
            return true;
        }

        match watcher.unwatch(directory) {
            Ok(()) => false,

            Err(failure) => {
                warn!(
                    "failed to stop watching `{}`: {failure}",
                    directory.display()
                );

                true
            }
        }
    });

    for directory in desired {
        if watched.contains(&directory) {
            continue;
        }

        match watcher.watch(&directory, RecursiveMode::NonRecursive) {
            Ok(()) => watched.push(directory),

            Err(failure) => {
                error!(
                    "failed to watch `{}` for configuration changes: {failure}",
                    directory.display()
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process;

    /// Self-signed certificate for `first.janusgate.test`, matching [`FIRST_PRIVATE_KEY_PEM`].
    const FIRST_CERTIFICATE_PEM: &str = concat!(
        "-----BEGIN CERTIFICATE-----\n",
        "MIIBVTCCAQegAwIBAgIUHDRC86nTQhdmLvdUULnMWH+iew0wBQYDK2VwMB8xHTAb\n",
        "BgNVBAMMFGZpcnN0LmphbnVzZ2F0ZS50ZXN0MCAXDTI2MDkxNzA4MzE0M1oYDzIx\n",
        "MjYwODI0MDgzMTQzWjAfMR0wGwYDVQQDDBRmaXJzdC5qYW51c2dhdGUudGVzdDAq\n",
        "MAUGAytlcAMhALUQYWkKbtTIS026qlxTrvVF5wuMbz1K1QdfnWj1avxZo1MwUTAd\n",
        "BgNVHQ4EFgQUrRW3wb+U1HLzSalv0PPbBYcWcqUwHwYDVR0jBBgwFoAUrRW3wb+U\n",
        "1HLzSalv0PPbBYcWcqUwDwYDVR0TAQH/BAUwAwEB/zAFBgMrZXADQQCj8ZbOkNoY\n",
        "f/WHm2esTEp1CORPDgot2SCL45AUsdn1rpygGi/C5eQkANM5jit8E5cnhw1CTuoC\n",
        "ChioJUsYoGYD\n",
        "-----END CERTIFICATE-----\n",
    );

    /// Private key matching [`FIRST_CERTIFICATE_PEM`].
    const FIRST_PRIVATE_KEY_PEM: &str = concat!(
        "-----BEGIN PRIVATE KEY-----\n",
        "MC4CAQAwBQYDK2VwBCIEIBCnShkxc+NajkF+uM0MeUsxCwJnNP2nwjp+3TxfmS8z\n",
        "-----END PRIVATE KEY-----\n",
    );

    /// Self-signed certificate for `second.janusgate.test`, matching [`SECOND_PRIVATE_KEY_PEM`].
    const SECOND_CERTIFICATE_PEM: &str = concat!(
        "-----BEGIN CERTIFICATE-----\n",
        "MIIBVzCCAQmgAwIBAgIUYOV+Z4ZH2BvCaZyiCrA4uhUMG0gwBQYDK2VwMCAxHjAc\n",
        "BgNVBAMMFXNlY29uZC5qYW51c2dhdGUudGVzdDAgFw0yNjA5MTcwODMxNDNaGA8y\n",
        "MTI2MDgyNDA4MzE0M1owIDEeMBwGA1UEAwwVc2Vjb25kLmphbnVzZ2F0ZS50ZXN0\n",
        "MCowBQYDK2VwAyEAXDsWE23dQYbXYJGT/TbJzr3GOgZjryG2/QWxsPGxt2GjUzBR\n",
        "MB0GA1UdDgQWBBQNP/DVjfYiWjEHWRTy08UAhd8HvDAfBgNVHSMEGDAWgBQNP/DV\n",
        "jfYiWjEHWRTy08UAhd8HvDAPBgNVHRMBAf8EBTADAQH/MAUGAytlcANBAKrrqD8e\n",
        "u82Z8ysh6SNIX+jKf8VsKOzXycb+3bbV3wEPn+EkZz8cxcd75SxuYXrzJQYoGWwk\n",
        "78gFXPU6r0x6oQI=\n",
        "-----END CERTIFICATE-----\n",
    );

    /// Private key matching [`SECOND_CERTIFICATE_PEM`].
    const SECOND_PRIVATE_KEY_PEM: &str = concat!(
        "-----BEGIN PRIVATE KEY-----\n",
        "MC4CAQAwBQYDK2VwBCIEILvVgw2hWzBckZOo4aL59T6qeyWPnh4c9T9AD0IN4hE5\n",
        "-----END PRIVATE KEY-----\n",
    );

    /// One failure-classification case: a label, a configuration body, and whether its loading
    /// failure matches the expected variant.
    type FailureCase = (&'static str, String, fn(&Error) -> bool);

    /// Writes `body` to a uniquely named file for `test`, creating its parent directory.
    ///
    /// # Panics
    ///
    /// Panics if the test environment's temporary directory cannot be created or written — a broken
    /// test environment.
    fn write_test_file(test: &str, name: &str, body: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("janusgate-config-{test}-{}", process::id()));

        #[expect(clippy::expect_used, reason = "a broken test environment is fatal")]
        fs::create_dir_all(&directory).expect("the test directory creates");

        let file = directory.join(name);

        #[expect(clippy::expect_used, reason = "a broken test environment is fatal")]
        fs::write(&file, body).expect("the test file writes");

        file
    }

    /// Renders a path as a JSON string value, with forward slashes so it parses on every platform.
    fn json_path(path: &Path) -> String {
        path.to_string_lossy().replace('\\', "/")
    }

    /// Renders a configuration body with the given management token; the TLS material paths are
    /// stated as a pair, because a `tls` section always states both.
    fn configuration_body(token: &str, tls: Option<(&Path, &Path)>) -> String {
        let tls = tls.map(|(certificate, private_key)| {
            format!(
                "\"tls\": {{\"certificate\": \"{}\", \"private_key\": \"{}\"}},",
                json_path(certificate),
                json_path(private_key)
            )
        });

        let tls = tls.as_deref().unwrap_or("");

        format!(
            r#"{{{tls}
                "listen": "127.0.0.1:8080",
                "database": {{"path": "janusgate.db"}},
                "management_api": {{"token": "{token}"}},
                "api": {{}}
            }}"#
        )
    }

    /// TLS material paths that exist on no filesystem, for configurations whose loading must fail.
    fn missing_tls_paths() -> (&'static Path, &'static Path) {
        (
            Path::new("missing-certificate.pem"),
            Path::new("missing-key.pem"),
        )
    }

    /// Verifies that [`ConfigLoader::new`] loads a plaintext configuration and one with TLS
    /// material, exposing both through its accessors.
    ///
    /// # Panics
    ///
    /// Panics if loading fails or the loaded state diverges.
    #[test]
    fn loads_plaintext_and_tls_configurations() {
        let plaintext_path = write_test_file(
            "loads",
            "plaintext.jsonc",
            &configuration_body("token", None),
        );

        #[expect(clippy::expect_used, reason = "the plaintext configuration loads")]
        let (plaintext, _watcher) =
            ConfigLoader::new(plaintext_path).expect("the plaintext configuration loads");

        assert_eq!(plaintext.management_api.token, "token");
        assert!(plaintext.tls().is_none());

        let certificate = write_test_file("loads", "certificate.pem", FIRST_CERTIFICATE_PEM);
        let private_key = write_test_file("loads", "private-key.pem", FIRST_PRIVATE_KEY_PEM);

        let tls_path = write_test_file(
            "loads",
            "tls.jsonc",
            &configuration_body("token", Some((&certificate, &private_key))),
        );

        #[expect(clippy::expect_used, reason = "the TLS configuration loads")]
        let (tls, _watcher) = ConfigLoader::new(tls_path).expect("the TLS configuration loads");

        assert!(tls.tls().is_some());
    }

    /// Verifies the failure classification of [`ConfigLoader::new`]: an unreadable configuration or
    /// certificate file, a rejected schema — a missing field, an unknown field, or a DNS `listen`
    /// host — a certificate chain with no certificates, an undecodable chain, an undecodable
    /// private key, and a key that does not match the leaf certificate.
    ///
    /// # Panics
    ///
    /// Panics if a failure lands on the wrong variant or a case succeeds.
    #[test]
    fn classifies_loading_failures() {
        let certificate = write_test_file("classify", "certificate.pem", FIRST_CERTIFICATE_PEM);
        let empty_chain = write_test_file("classify", "empty.pem", "not a pem file\n");

        let malformed_chain = write_test_file(
            "classify",
            "malformed.pem",
            "-----BEGIN CERTIFICATE-----\n!!!\n-----END CERTIFICATE-----\n",
        );

        let not_a_key = write_test_file("classify", "not-a-key.pem", "not a pem\n");
        let other_key = write_test_file("classify", "other-key.pem", SECOND_PRIVATE_KEY_PEM);

        let cases: [FailureCase; 8] = [
            (
                "unreadable certificate",
                configuration_body("token", Some(missing_tls_paths())),
                |failure| matches!(failure, Error::Read { .. }),
            ),
            (
                "missing field",
                String::from(
                    "{\"listen\": \"127.0.0.1:8080\", \"database\": {\"path\": \"x.db\"}, \
                     \"management_api\": {\"token\": \"t\"}}",
                ),
                |failure| matches!(failure, Error::Parse { .. }),
            ),
            (
                "unknown field",
                String::from(
                    "{\"listen\": \"127.0.0.1:8080\", \"database\": {\"path\": \"x.db\", \
                     \"conections\": 4}, \"management_api\": {\"token\": \"t\"}, \
                     \"api\": {}}",
                ),
                |failure| matches!(failure, Error::Parse { .. }),
            ),
            (
                "DNS listen host",
                String::from(
                    "{\"listen\": \"localhost:8080\", \"database\": {\"path\": \"x.db\"}, \
                     \"management_api\": {\"token\": \"t\"}, \"api\": {}}}",
                ),
                |failure| matches!(failure, Error::Parse { .. }),
            ),
            (
                "empty chain",
                configuration_body("token", Some((&empty_chain, &other_key))),
                |failure| matches!(failure, Error::EmptyCertificateChain { .. }),
            ),
            (
                "malformed chain",
                configuration_body("token", Some((&malformed_chain, &other_key))),
                |failure| matches!(failure, Error::ParseCertificateChain { .. }),
            ),
            (
                "undecodable key",
                configuration_body("token", Some((&certificate, &not_a_key))),
                |failure| matches!(failure, Error::ParsePrivateKey { .. }),
            ),
            (
                "mismatched key",
                configuration_body("token", Some((&certificate, &other_key))),
                |failure| matches!(failure, Error::BuildTlsServerConfig { .. }),
            ),
        ];

        for (index, (label, body, is_expected)) in cases.into_iter().enumerate() {
            let path = write_test_file("classify", &format!("case-{index}.jsonc"), &body);

            #[expect(
                clippy::panic,
                reason = "a case that loads must fail, which is what it asserts"
            )]
            let Err(failure) = ConfigLoader::new(path) else {
                panic!("case {index} ({label}) loads but must fail");
            };

            assert!(is_expected(&failure), "case {index} ({label})");
        }
    }

    /// Verifies that an unknown field's rejection message names the field and its line and column,
    /// so a typo cannot silently disable a setting.
    ///
    /// # Panics
    ///
    /// Panics if the rejection message omits the field or the location.
    #[test]
    fn rejects_unknown_fields_with_location() {
        let path = write_test_file(
            "unknown",
            "configuration.jsonc",
            r#"{
                "listen": "127.0.0.1:8080",
                "database": {"path": "janusgate.db", "conections": 4},
                "management_api": {"token": "t"},
                "api": {}
            }"#,
        );

        #[expect(
            clippy::panic,
            reason = "an unknown field is rejected, which is what the test asserts"
        )]
        let Err(failure) = ConfigLoader::new(path) else {
            panic!("an unknown field parses but must be rejected");
        };

        let rendered = failure.to_string();

        assert!(rendered.contains("unknown field"), "{rendered}");
        assert!(rendered.contains("line"), "{rendered}");
    }

    /// Verifies that a reload applies a changed configuration file, that a failed reload keeps the
    /// previous state — an unparsable file, and a configuration whose TLS material does not resolve
    /// keeps the previous schema as well — and that dropping the TLS section applies and serves
    /// plaintext. Together these pin the single-generation property (README §8): a successful
    /// reload replaces the schema and the TLS server configuration together even when the TLS
    /// material files are unchanged, and a failed one keeps both halves.
    ///
    /// # Panics
    ///
    /// Panics if the applied state diverges.
    #[test]
    fn reloads_the_configuration_or_keeps_it() {
        let certificate = write_test_file("reload", "certificate.pem", FIRST_CERTIFICATE_PEM);
        let private_key = write_test_file("reload", "private-key.pem", FIRST_PRIVATE_KEY_PEM);
        let path = write_test_file(
            "reload",
            "configuration.jsonc",
            &configuration_body("first", Some((&certificate, &private_key))),
        );

        #[expect(clippy::expect_used, reason = "the initial configuration loads")]
        let (loader, _watcher) = ConfigLoader::new(path).expect("the initial configuration loads");

        let schema_before: &Config = &loader;

        #[expect(clippy::expect_used, reason = "the initial TLS configuration exists")]
        let tls_before = loader.tls().expect("the TLS configuration exists");

        write_test_file(
            "reload",
            "configuration.jsonc",
            &configuration_body("second", Some((&certificate, &private_key))),
        );

        loader.reload();
        assert_eq!(loader.management_api.token, "second");
        assert!(!std::ptr::eq(schema_before, &*loader));

        // The two halves move together: one generation, replaced wholly, though the TLS material
        // files themselves are unchanged.
        #[expect(clippy::expect_used, reason = "the reloaded TLS configuration exists")]
        let tls_second = loader.tls().expect("the reloaded TLS configuration exists");

        assert!(!Arc::ptr_eq(&tls_before, &tls_second));

        write_test_file("reload", "configuration.jsonc", "not json");

        loader.reload();
        assert_eq!(loader.management_api.token, "second");
        assert!(
            loader
                .tls()
                .is_some_and(|tls| Arc::ptr_eq(&tls, &tls_second))
        );

        write_test_file(
            "reload",
            "configuration.jsonc",
            &configuration_body("third", Some(missing_tls_paths())),
        );

        // Or not at all: a failed reload keeps the whole previous generation, both halves.
        loader.reload();
        assert_eq!(loader.management_api.token, "second");
        assert!(
            loader
                .tls()
                .is_some_and(|tls| Arc::ptr_eq(&tls, &tls_second))
        );

        write_test_file(
            "reload",
            "configuration.jsonc",
            &configuration_body("fourth", None),
        );

        loader.reload();
        assert_eq!(loader.management_api.token, "fourth");
        assert!(loader.tls().is_none());
    }

    /// Verifies that changed TLS material files alone — the configuration file untouched — still
    /// reach the TLS server configuration through a reload, and that material that does not resolve
    /// keeps the previous one.
    ///
    /// # Panics
    ///
    /// Panics if the applied state diverges.
    #[test]
    fn reloads_tls_material() {
        let certificate = write_test_file("tls-reload", "certificate.pem", FIRST_CERTIFICATE_PEM);
        let private_key = write_test_file("tls-reload", "private-key.pem", FIRST_PRIVATE_KEY_PEM);
        let path = write_test_file(
            "tls-reload",
            "configuration.jsonc",
            &configuration_body("token", Some((&certificate, &private_key))),
        );

        #[expect(clippy::expect_used, reason = "the initial configuration loads")]
        let (loader, _watcher) = ConfigLoader::new(path).expect("the initial configuration loads");

        #[expect(clippy::expect_used, reason = "the TLS configuration exists")]
        let tls_before = loader.tls().expect("the TLS configuration exists");

        write_test_file("tls-reload", "certificate.pem", SECOND_CERTIFICATE_PEM);
        write_test_file("tls-reload", "private-key.pem", SECOND_PRIVATE_KEY_PEM);

        loader.reload();

        #[expect(clippy::expect_used, reason = "the reloaded TLS configuration exists")]
        let tls_after = loader.tls().expect("the reloaded TLS configuration exists");

        assert!(!Arc::ptr_eq(&tls_before, &tls_after));

        write_test_file("tls-reload", "certificate.pem", "not a pem\n");

        loader.reload();
        assert!(
            loader
                .tls()
                .is_some_and(|tls| Arc::ptr_eq(&tls, &tls_after))
        );
    }
}
