//! Gateway configuration: the parsed schema
//! ([`schema::Config`]), the reloadable owner
//! ([`ConfigLoader`]), and the loading, hot-reload, TLS, and CORS machinery.
//!
//! [`ConfigLoader`] owns the live state — the applied configuration generation, the schema and
//! the TLS server configuration and compaction sealing keys resolved from it as one value
//! ([`AppliedConfig`]) — and the reload pipeline. Its watcher future, returned by
//! [`ConfigLoader::new`] for the caller to spawn, watches the configuration file and the
//! configured TLS material through the platform's native filesystem notifications; a reload is
//! debounced — it applies once the watched inputs have been quiet for a fixed window — and a
//! reload that fails for any reason keeps the previously applied state and logs the failure.
//! Only startup is fatal.

use self::schema::Config;
use crate::compaction_model::sealing::SealingKeys;
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
pub(crate) mod schema;
mod tls;

use self::error::Error;

/// Quiet period the watched inputs must stay silent for before a debounced reload applies — long
/// enough that an editor's write-rename chain, a certificate rotation writing several files, or
/// notify's event bursts all coalesce into one final apply, and imperceptible on a human time
/// scale.
const RELOAD_DEBOUNCE: Duration = Duration::from_millis(500);

/// One applied configuration generation: the schema and the TLS server configuration and the
/// compaction sealing keys resolved from it, packed into a single value so [`ConfigLoader`] can
/// swap them all with one store. Every reader — the [`Deref`]-based schema reads,
/// [`ConfigLoader::tls`], and [`ConfigLoader::sealing_keys`] alike — therefore observes exactly
/// one generation, and a reload can never present a new schema with the previous generation's
/// TLS material or sealing keys, or the reverse (README §8).
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

    /// The compaction sealing keys resolved from the schema's typed `compaction` section; `None`
    /// serves no sealed compaction artifacts. Refcounted like
    /// [`AppliedConfig::tls_server_config`]: the ciphers are initialized exactly once, per
    /// generation, in the load pipeline, and dropping a superseded generation releases its last
    /// owning reference once the operations running under it finish.
    sealing_keys: Option<Arc<SealingKeys>>,
}

/// The gateway's live configuration loader: the currently applied configuration generation
/// ([`AppliedConfig`]) and the reload pipeline that keeps it current with its files.
///
/// Schema reads are plain dereferences — `crate::CONFIG.field` — through [`Deref`]; TLS and
/// sealing-key reads go through [`ConfigLoader::tls`] and [`ConfigLoader::sealing_keys`]. All read
/// the one generation held in the loader's single [`ArcSwap`] cell, so they never disagree, and a
/// reload applies with one store.
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
    /// material and sealing keys — and returns the loader together with its watcher future, which
    /// the caller spawns.
    ///
    /// # Errors
    ///
    /// Returns the loading failure: an unreadable input, or a schema the deserializer rejects —
    /// including a `compaction` section whose key set violates its contract — or TLS material that
    /// does not resolve.
    pub(crate) fn new(path: PathBuf) -> Result<(Self, impl Future<Output = ()> + Send), Error> {
        let applied = read_inputs(&path)?;

        let loader = Self {
            path,
            applied: Arc::new(ArcSwap::from_pointee(applied)),
        };

        let watcher = loader.clone().watch();

        Ok((loader, watcher))
    }

    /// The sealing keys resolved from the currently applied configuration generation, `None` when
    /// no `compaction` section is configured. Sealing and unsealing read the one generation per
    /// operation, so a reload applies to the next operation and can never pair a new schema with
    /// the previous generation's keys, or the reverse.
    pub(crate) fn sealing_keys(&self) -> Option<Arc<SealingKeys>> {
        self.applied.load_full().sealing_keys.clone()
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
    /// A configuration the deserializer rejects — an invalid sealing-key set included — or whose
    /// TLS material does not resolve keeps the previous schema as well, and any failure keeps the
    /// previously applied state and logs the failure — a running gateway never dies over a reload.
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
/// deserialize the schema strictly — an invalid `compaction` sealing-key set is a schema
/// rejection, carrying the offending value's line and column — and resolve the TLS material into
/// a Rustls server configuration and the typed sealing keys into their initialized ciphers, packed
/// into one configuration generation.
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

    // The typed `compaction` section is valid by construction, so the ciphers initialize
    // infallibly, once per generation.
    let sealing_keys = schema
        .compaction
        .as_ref()
        .map(|section| Arc::new(SealingKeys::build(section)));

    // Leak the schema: dereferenced references borrow the process-lifetime loader, so every
    // applied schema must outlive it (see [`AppliedConfig`]). Leaked only past the resolution
    // above, so a failed load drops the schema instead of leaking it.
    Ok(AppliedConfig {
        schema: Box::leak(Box::new(schema)),
        tls_server_config,
        sealing_keys,
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
                warn!(
                    "failed to watch `{}` for configuration changes: {failure}",
                    directory.display()
                );
            }
        }
    }
}
