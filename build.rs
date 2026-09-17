//! Build-time database schema fingerprinter.
//!
//! This script computes a canonical schema fingerprint for the SQLite database schema in
//! `SCHEMA.sqlite` and writes it to `$OUT_DIR/database_schema_fingerprint`.

use std::{env, error::Error as StdError, fs, path::PathBuf};
use syntaqlite::{FormatConfig, Formatter, fmt::KeywordCase};

/// Relative path to the canonical database schema file.
const DATABASE_SCHEMA_PATH: &str = "src/database/SCHEMA.sqlite";

/// Computes the database schema fingerprint and emits build artifacts.
///
/// # Errors
///
/// Returns an error if reading `SCHEMA.sqlite`, formatting, hashing, or writing the fingerprint
/// file fails.
fn main() -> Result<(), Box<dyn StdError>> {
    println!("cargo::rerun-if-changed={DATABASE_SCHEMA_PATH}");

    let out_dir = PathBuf::from(env::var("OUT_DIR")?);

    let mut sql_formatter = Formatter::with_config(
        &FormatConfig::default()
            .with_keyword_case(KeywordCase::Lower)
            .with_line_width(120),
    );

    let schema = fs::read_to_string(DATABASE_SCHEMA_PATH)?;
    let fingerprint = schema_fingerprint_i32(&mut sql_formatter, &schema)?;

    let fingerprint_file = out_dir.join("database_schema_fingerprint");
    fs::write(&fingerprint_file, fingerprint.to_be_bytes())?;

    Ok(())
}

/// Normalizes the SQL schema using `syntaqlite` and hashes it with BLAKE3 into a non-zero 32-bit
/// signed integer.
///
/// # Errors
///
/// Returns an error if schema formatting fails or if the fingerprint collides with the reserved
/// value 0.
fn schema_fingerprint_i32(
    formatter: &mut Formatter,
    schema: &str,
) -> Result<i32, Box<dyn StdError>> {
    let canonical = formatter.format(schema)?;

    let hash = blake3::hash(canonical.trim().as_bytes());

    let mut bytes = [0u8; 4];
    bytes.copy_from_slice(&hash.as_bytes()[..4]);

    let fingerprint = i32::from_be_bytes(bytes);

    if fingerprint == 0 {
        return Err("database schema fingerprint collided with reserved value 0".into());
    }

    Ok(fingerprint)
}
