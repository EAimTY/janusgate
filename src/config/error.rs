//! The configuration error domain.

use rustls::{Error as RustlsError, pki_types::pem::Error as PemError};
use serde_json::Error as SerdeJsonError;
use std::{io::Error as IoError, path::PathBuf};

/// A configuration failure.
///
/// Concrete, in the management API error domain's style: one variant per way loading the
/// configuration or its TLS material can fail, carrying the values that identify it, so the startup
/// panic or the reload log record prints the whole cause. A schema rejection carries serde_json's
/// own message — the offending value with its line and column — verbatim.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    /// An input file cannot be read: the configuration file, the certificate chain, or the private
    /// key.
    #[error("cannot read `{path}`: {source}")]
    Read {
        /// Path of the unreadable file.
        path: PathBuf,

        /// The filesystem failure.
        source: IoError,
    },

    /// The configuration file is not JSONC the schema accepts. The deserialization failure's own
    /// message names the offending value and its line and column.
    #[error("cannot parse the configuration file `{path}`: {source}")]
    Parse {
        /// Path of the configuration file.
        path: PathBuf,

        /// The deserialization failure.
        source: SerdeJsonError,
    },

    /// The TLS certificate chain file does not decode as PEM certificates.
    #[error("cannot parse the TLS certificate chain `{path}`: {source}")]
    ParseCertificateChain {
        /// Path of the certificate chain file.
        path: PathBuf,

        /// The PEM decoding failure.
        source: PemError,
    },

    /// The TLS certificate chain file decodes but contains no certificates.
    #[error("the TLS certificate chain `{path}` contains no certificates")]
    EmptyCertificateChain {
        /// Path of the certificate chain file.
        path: PathBuf,
    },

    /// The TLS private key file does not decode as a PEM private key.
    #[error("cannot parse the TLS private key `{path}`: {source}")]
    ParsePrivateKey {
        /// Path of the private key file.
        path: PathBuf,

        /// The PEM decoding failure, including a file with no key section.
        source: PemError,
    },

    /// The TLS material cannot be combined into a TLS server configuration: a private key that does
    /// not match the leaf certificate, or material the TLS library refuses.
    #[error("cannot build the TLS server configuration: {source}")]
    BuildTlsServerConfig {
        /// The TLS library failure.
        source: RustlsError,
    },
}
