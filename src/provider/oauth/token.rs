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
/// A non-positive `expires_in` is a malformed token response and is rejected,
/// and so is an `exp` claim already in the past: stamping an expiry at or
/// before `now` would mark the token permanently stale and drive a refresh
/// grant against the endpoint on every request, while substituting the
/// default lifetime would present an already-expired access token to the
/// endpoint.
///
/// Google access tokens are opaque (`ya29…`) and carry no `exp` claim, so a
/// Google refresh response without `expires_in` has no better signal than the
/// default lifetime; both Google and OpenAI token endpoints report
/// `expires_in`, which therefore always wins.
///
/// # Errors
///
/// Returns [`Error`] when the reported `expires_in` is zero or negative, or
/// when the access token's `exp` claim is not in the future.
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

    let lifetime = match expires_in {
        Some(lifetime) => lifetime,
        None => access_token
            .map(access_token_expires_in)
            .transpose()?
            .flatten()
            .unwrap_or(DEFAULT_TOKEN_TTL),
    };

    Ok(now + lifetime)
}

/// Remaining lifetime of an access token according to its `exp` claim, or
/// `None` when the token carries no decodable `exp` claim.
fn access_token_expires_in(access_token: &str) -> Result<Option<i64>, Error> {
    let Some(exp) = jwt_claim(access_token, &["exp"]).and_then(|exp| exp.as_i64()) else {
        return Ok(None);
    };

    let lifetime = exp - unix_time::unix_now();

    if lifetime <= 0 {
        return Err(Error::Unavailable {
            detail: format!(
                "malformed token response: `exp` claim must be in the future, \
                 got {lifetime}s of remaining lifetime"
            ),
        });
    }

    Ok(Some(lifetime))
}

/// Reads a nested claim out of a JWT payload.
pub(super) fn jwt_claim(jwt: &str, path: &[&str]) -> Option<serde_json::Value> {
    let payload = jwt.split('.').nth(1)?;

    let claims: serde_json::Value =
        serde_json::from_slice(&general_purpose::URL_SAFE_NO_PAD.decode(payload).ok()?).ok()?;

    path.iter()
        .try_fold(claims, |claims, segment| claims.get(segment).cloned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encodes a JWT whose payload carries `exp` at `now + remaining`, so the
    /// `exp` fallback of [`token_expiry_from`] can be exercised without a
    /// token endpoint.
    fn jwt_with_exp(remaining: i64) -> String {
        let payload = general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::json!({"exp": unix_time::unix_now() + remaining}).to_string());

        format!("header.{payload}.signature")
    }

    /// An `exp` claim already in the past is malformed exactly like a
    /// non-positive `expires_in`: the response must fail instead of storing a
    /// permanently stale token or presenting an expired token under a default
    /// lifetime. A reported `expires_in` still wins and is never overridden
    /// by the spent claim.
    #[test]
    fn rejects_past_exp_claim_as_malformed() {
        let jwt = jwt_with_exp(-3_600);

        assert!(token_expiry_from(None, Some(&jwt)).is_err());

        let expiry = token_expiry_from(Some(600), Some(&jwt)).unwrap();

        assert!(expiry > unix_time::unix_now());
    }

    /// Without `expires_in`, a decodable future `exp` claim supplies the
    /// lifetime and a token with no usable claim (opaque Google `ya29…`
    /// tokens) falls through to the default lifetime instead of failing.
    #[test]
    fn falls_back_to_exp_claim_then_default_lifetime() {
        let expiry = token_expiry_from(None, Some(&jwt_with_exp(3_600))).unwrap();

        assert!((3595..=3600).contains(&(expiry - unix_time::unix_now())));

        let expiry = token_expiry_from(None, Some("ya29-opaque-token")).unwrap();

        assert!((86_399..=86_400).contains(&(expiry - unix_time::unix_now())));
    }
}
