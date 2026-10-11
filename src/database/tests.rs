//! Database open-mode tests: the refuse-to-create read open answers `None` for a missing file
//! without touching the filesystem, and the creating open builds the file and its parent
//! directory when absent.

#![expect(
    clippy::missing_panics_doc,
    reason = "a test fails by panicking; a # Panics section on every test would restate that"
)]

use super::{Database, MAIN_POOL_WAIT, main_pool_connections};
use crate::database::schema::Registry;
use std::{error::Error as StdError, path::PathBuf};

/// The pooled open arguments shared by both open-mode tests.
fn pool_args() -> (std::num::NonZero<usize>, std::time::Duration) {
    (main_pool_connections(), MAIN_POOL_WAIT)
}

/// The refuse-to-create read open answers `None` for a missing file and creates neither the file
/// nor its parent directory.
#[tokio::test]
async fn open_existing_answers_none_for_a_missing_file() -> Result<(), Box<dyn StdError>> {
    let directory =
        std::env::temp_dir().join(format!("janusgate-db-open-{}", rand::random::<u64>()));
    let path = directory.join("missing.db");

    let (connections, wait) = pool_args();

    let opened = Database::open_existing(&path, Registry::Main, connections, wait).await?;

    assert!(opened.is_none(), "a missing file opens as no database");

    assert!(!path.exists(), "the read open never creates the file");

    assert!(
        !directory.exists(),
        "the read open never creates the parent directory"
    );

    Ok(())
}

/// The creating open builds the database file and its parent directory when absent, and the
/// opened pool serves queries.
#[tokio::test]
async fn open_creates_the_file_and_its_parent_directory() -> Result<(), Box<dyn StdError>> {
    let directory: PathBuf = std::env::temp_dir()
        .join(format!("janusgate-db-open-{}", rand::random::<u64>()))
        .join("nested");

    let path = directory.join("created.db");

    let (connections, wait) = pool_args();

    let database = Database::open(&path, Registry::Main, connections, wait).await?;

    database.query("SELECT 1", ()).await?;

    drop(database);

    assert!(path.exists(), "the creating open writes the database file");

    let _ = std::fs::remove_dir_all(directory.parent().unwrap_or(&directory));

    Ok(())
}
