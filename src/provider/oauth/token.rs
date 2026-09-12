//! Token lifetime and JWT claim helpers shared by the flow and refresh
//! paths.

use super::Error;
use crate::unix_time;
use base64::{Engine as _, engine::general_purpose};

/// Default token lifetime when a provider reports neither `expires_in` nor a
/// usable `exp` claim.
const DEFAULT_TOKEN_TTL: i64 = 86_400;

/// Computes the expiry of a token set: a reported `expires_in` when present,
/// else the remaining lifetime of the access token's `exp` claim, else the
/// default lifetime.
///
/// A non-positive `expires_in` is a malformed token response and is rejected:
/// stamping an expiry at or before `now` would mark the token permanently
/// stale and drive a refresh grant against the endpoint on every request.
///
/// Google access tokens are opaque (`ya29…`) and carry no `exp` claim, so a
/// Google refresh response without `expires_in` has no better signal than the
/// default lifetime; both Google and OpenAI token endpoints report
/// `expires_in`, which therefore always wins.
///
/// # Errors
///
/// Returns [`Error`] when the reported `expires_in` is zero or negative.
pub(super) fn token_expiry_from(
    expires_in: Option<i64>,
    access_token: Option<&str>,
) -> Result<i64, Error> {
    if let Some(lifetime) = expires_in
        && lifetime <= 0
    {
        return Err(Error::Unavailable {
            detail: format!(
                "malformed token response: `expires_in` must be positive, got {lifetime}"
            ),
        });
    }

    let now = unix_time::unix_now();

    Ok(expires_in
        .or_else(|| access_token.and_then(access_token_expires_in))
        .map(|lifetime| now + lifetime)
        .unwrap_or_else(|| now + DEFAULT_TOKEN_TTL))
}

/// Extracts the `exp` claim of an access token JWT as a remaining lifetime.
fn access_token_expires_in(access_token: &str) -> Option<i64> {
    let exp = jwt_claim(access_token, &["exp"])?.as_i64()?;

    Some(exp - unix_time::unix_now())
}

/// Reads a nested claim out of a JWT payload.
pub(super) fn jwt_claim(jwt: &str, path: &[&str]) -> Option<serde_json::Value> {
    let payload = jwt.split('.').nth(1)?;

    let claims: serde_json::Value =
        serde_json::from_slice(&general_purpose::URL_SAFE_NO_PAD.decode(payload).ok()?).ok()?;

    path.iter()
        .try_fold(claims, |claims, segment| claims.get(segment).cloned())
}
