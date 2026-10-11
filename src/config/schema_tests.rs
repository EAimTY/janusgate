//! Tests for the configuration schema: the empty document's defaults, the section presence
//! switches, and the strict deserialization contract (`docs/reference/configuration.md`).

use super::schema::Config;
use log::LevelFilter;
use serde_json::Error as SerdeJsonError;
use std::{
    error::Error as StdError,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    path::PathBuf,
};

/// Parses a configuration document at the schema boundary: strictly, with unknown fields and
/// invalid typed values rejected and every omitted field defaulted.
fn parse(body: &str) -> Result<Config, SerdeJsonError> {
    serde_json::from_str(body)
}

/// Asserts a configuration document is rejected by the schema.
///
/// # Panics
///
/// Panics when the schema accepts the document.
fn assert_rejected(body: &str) {
    assert!(
        parse(body).is_err(),
        "the document must be rejected: {body}"
    );
}

/// The empty document is a valid configuration: every documented default applies and the
/// management API is absent, its section being the switch.
///
/// # Panics
///
/// Panics when the behavior under test does not hold: the failed assertion is the test's failure
/// report.
#[test]
fn empty_document_defaults() -> Result<(), Box<dyn StdError>> {
    let config = parse("{}")?;

    assert_eq!(config.log_level, LevelFilter::Info);
    assert_eq!(
        config.listen,
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 4718)
    );
    assert!(config.tls.is_none());
    assert_eq!(config.database.path, PathBuf::from("janusgate.db"));
    assert!(config.management_api.is_none());
    assert!(config.compaction.is_none());
    assert!(config.api.logs.is_enabled);
    assert_eq!(config.api.logs.path, PathBuf::from("api_logs"));
    assert_eq!(config.api.logs.retention_days.get(), 90);
    assert_eq!(config.pooling.quota_refresh_interval_seconds.get(), 60);
    assert_eq!(config.pooling.sticky_idle_timeout_seconds.get(), 86400);
    assert_eq!(config.pooling.sticky_retry_timeout_seconds.get(), 60);
    assert_eq!(config.pooling.sticky_maximum_entries.get(), 100000);
    assert!(!config.api.cors.allow_private_network);

    Ok(())
}

/// A section given partially keeps the documented defaults of its unspecified fields.
///
/// # Panics
///
/// Panics when the behavior under test does not hold: the failed assertion is the test's failure
/// report.
#[test]
fn partial_sections_keep_field_defaults() -> Result<(), Box<dyn StdError>> {
    let config = parse(
        r#"{"api": {"logs": {"retention_days": 7}}, "pooling": {"sticky_maximum_entries": 1}}"#,
    )?;

    assert!(config.api.logs.is_enabled);
    assert_eq!(config.api.logs.path, PathBuf::from("api_logs"));
    assert_eq!(config.api.logs.retention_days.get(), 7);
    assert_eq!(config.pooling.quota_refresh_interval_seconds.get(), 60);
    assert_eq!(config.pooling.sticky_maximum_entries.get(), 1);

    Ok(())
}

/// Unknown fields are rejected at the root and inside every nested section.
#[test]
fn unknown_fields_are_rejected() {
    assert_rejected(r#"{"bogus": true}"#);
    assert_rejected(r#"{"api": {"bogus": true}}"#);
    assert_rejected(r#"{"api": {"logs": {"bogus": true}}}"#);
    assert_rejected(r#"{"api": {"cors": {"bogus": true}}}"#);
    assert_rejected(r#"{"pooling": {"bogus": true}}"#);
    assert_rejected(r#"{"database": {"bogus": true}}"#);
    assert_rejected(r#"{"tls": {"certificate": "cert.pem", "bogus": true}}"#);
    assert_rejected(r#"{"management_api": {"token": "t", "bogus": true}}"#);
    assert_rejected(r#"{"compaction": {"keys": [], "bogus": true}}"#);
}

/// Values of the wrong JSON type are rejected everywhere they appear.
#[test]
fn wrong_types_are_rejected() {
    assert_rejected(r#"{"listen": 4718}"#);
    assert_rejected(r#"{"log_level": true}"#);
    assert_rejected(r#"{"database": "janusgate.db"}"#);
    assert_rejected(r#"{"api": {"logs": {"is_enabled": "yes"}}}"#);
    assert_rejected(r#"{"api": {"logs": {"retention_days": "90"}}}"#);
    assert_rejected(r#"{"api": {"logs": {"retention_days": 1.5}}}"#);
    assert_rejected(r#"{"pooling": {"quota_refresh_interval_seconds": 1.5}}"#);
}

/// Non-positive pooling and retention limits are rejected: they are positive integers by
/// contract.
#[test]
fn non_positive_limits_are_rejected() {
    assert_rejected(r#"{"api": {"logs": {"retention_days": 0}}}"#);
    assert_rejected(r#"{"api": {"logs": {"retention_days": -1}}}"#);
    assert_rejected(r#"{"pooling": {"quota_refresh_interval_seconds": 0}}"#);
    assert_rejected(r#"{"pooling": {"sticky_idle_timeout_seconds": 0}}"#);
    assert_rejected(r#"{"pooling": {"sticky_retry_timeout_seconds": 0}}"#);
    assert_rejected(r#"{"pooling": {"sticky_maximum_entries": 0}}"#);
}

/// The listen address accepts IP-literal socket addresses, including IPv6, and rejects DNS names
/// and malformed addresses: a listener address is a local binding fact, never a name to resolve.
///
/// # Panics
///
/// Panics when the behavior under test does not hold: the failed assertion is the test's failure
/// report.
#[test]
fn listen_accepts_ip_literals_only() -> Result<(), Box<dyn StdError>> {
    let config = parse(r#"{"listen": "127.0.0.1:4718"}"#)?;

    assert_eq!(
        config.listen,
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 4718)
    );

    let config = parse(r#"{"listen": "[::1]:80"}"#)?;

    assert_eq!(
        config.listen,
        SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 80)
    );

    for invalid in [
        "localhost:4718",
        "example.com:443",
        "127.0.0.1",
        "127.0.0.1:99999",
        "127.0.0.1:0x1f",
        "127.0.0.1:",
        ":4718",
        "",
    ] {
        assert_rejected(&format!(r#"{{"listen": "{invalid}"}}"#));
    }

    Ok(())
}

/// Log levels parse case-insensitively and reject unknown severities.
///
/// # Panics
///
/// Panics when the behavior under test does not hold: the failed assertion is the test's failure
/// report.
#[test]
fn log_level_is_case_insensitive_and_strict() -> Result<(), Box<dyn StdError>> {
    let config = parse(r#"{"log_level": "INFO"}"#)?;

    assert_eq!(config.log_level, LevelFilter::Info);

    let config = parse(r#"{"log_level": "Trace"}"#)?;

    assert_eq!(config.log_level, LevelFilter::Trace);

    let config = parse(r#"{"log_level": "off"}"#)?;

    assert_eq!(config.log_level, LevelFilter::Off);

    for invalid in ["verbose", "warning", "notice", ""] {
        assert_rejected(&format!(r#"{{"log_level": "{invalid}"}}"#));
    }

    Ok(())
}

/// The management API section is the switch: absent by default, present with its required token
/// when configured, and rejected without a token — the gateway never invents a secret.
///
/// # Panics
///
/// Panics when the behavior under test does not hold: the failed assertion is the test's failure
/// report.
#[test]
fn management_api_presence_and_required_token() -> Result<(), Box<dyn StdError>> {
    assert_rejected(r#"{"management_api": {}}"#);

    let config = parse(r#"{"management_api": {"token": "secret"}}"#)?;

    let management = match config.management_api {
        Some(management) => management,
        None => return Err("the management section must be present".into()),
    };

    assert_eq!(management.token, "secret");

    Ok(())
}

/// A TLS section requires both material files at the schema level: the files themselves are only
/// read when the loader resolves the section.
///
/// # Panics
///
/// Panics when the behavior under test does not hold: the failed assertion is the test's failure
/// report.
#[test]
fn tls_section_requires_both_material_files() -> Result<(), Box<dyn StdError>> {
    assert_rejected(r#"{"tls": {}}"#);
    assert_rejected(r#"{"tls": {"certificate": "cert.pem"}}"#);
    assert_rejected(r#"{"tls": {"private_key": "key.pem"}}"#);

    assert!(
        parse(r#"{"tls": {"certificate": "cert.pem", "private_key": "key.pem"}}"#).is_ok(),
        "a complete tls section must parse"
    );

    Ok(())
}
