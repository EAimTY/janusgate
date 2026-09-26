//! TLS server configuration resolution from the schema's TLS material, and the live TLS accessor of
//! [`ConfigLoader`].

use super::{ConfigLoader, Error, schema::Config};
use rustls::{
    ServerConfig,
    crypto::aws_lc_rs,
    pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject as _},
    version,
};
use std::{fs, path::Path, sync::Arc};

/// ALPN protocols offered on TLS listeners, in preference order.
///
/// Rustls negotiates one of them with the TLS client, and hyper-util's protocol auto-detection
/// independently identifies the spoken protocol from the decrypted stream's leading bytes, so both
/// ALPN-negotiated and plaintext prior-knowledge HTTP/2 clients are served.
const ALPN_PROTOCOLS: [&[u8]; 2] = [b"h2", b"http/1.1"];

impl ConfigLoader {
    /// The TLS server configuration of the currently applied configuration generation, `None` for
    /// plaintext HTTP.
    ///
    /// Established connections keep the configuration they were accepted with; each new connection
    /// reads the current one.
    pub(crate) fn tls(&self) -> Option<Arc<ServerConfig>> {
        self.applied.load_full().tls_server_config.clone()
    }
}

/// Reads a schema's TLS material and resolves it into the shared TLS server configuration offered
/// to every inbound TLS connection; `None` when the schema configures no TLS.
pub(super) fn resolve(schema: &Config) -> Result<Option<Arc<ServerConfig>>, Error> {
    let Some(tls) = schema.tls.as_ref() else {
        return Ok(None);
    };

    let certificate = fs::read(&tls.certificate).map_err(|source| Error::Read {
        path: tls.certificate.clone(),
        source,
    })?;

    let private_key = fs::read(&tls.private_key).map_err(|source| Error::Read {
        path: tls.private_key.clone(),
        source,
    })?;

    let mut configuration =
        ServerConfig::builder_with_provider(Arc::new(aws_lc_rs::default_provider()))
            .with_protocol_versions(&[&version::TLS13, &version::TLS12])
            .map_err(|source| Error::BuildTlsServerConfig { source })?
            .with_no_client_auth()
            .with_single_cert(
                decode_certificate_chain(&tls.certificate, &certificate)?,
                decode_private_key(&tls.private_key, &private_key)?,
            )
            .map_err(|source| Error::BuildTlsServerConfig { source })?;

    configuration.alpn_protocols = ALPN_PROTOCOLS.into_iter().map(Vec::from).collect();

    Ok(Some(Arc::new(configuration)))
}

/// Decodes a PEM certificate chain from its file bytes, rejecting a chain with no certificates —
/// rustls would accept one and fail only at handshake time.
fn decode_certificate_chain(
    path: &Path,
    bytes: &[u8],
) -> Result<Vec<CertificateDer<'static>>, Error> {
    let certificates = CertificateDer::pem_slice_iter(bytes)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| Error::ParseCertificateChain {
            path: path.to_owned(),
            source,
        })?;

    if certificates.is_empty() {
        return Err(Error::EmptyCertificateChain {
            path: path.to_owned(),
        });
    }

    Ok(certificates)
}

/// Decodes a PEM private key from its file bytes.
fn decode_private_key(path: &Path, bytes: &[u8]) -> Result<PrivateKeyDer<'static>, Error> {
    PrivateKeyDer::from_pem_slice(bytes).map_err(|source| Error::ParsePrivateKey {
        path: path.to_owned(),
        source,
    })
}
