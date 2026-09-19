//! Token lifetime and JWT claim helpers shared by the flow and refresh paths.

use super::Error;
use crate::unix_time;
use base64::{Engine as _, engine::general_purpose};
use serde::{Deserialize, de::DeserializeOwned};

/// Default token lifetime in database-clock microseconds, when a provider reports neither
/// `expires_in` nor a usable `exp` claim.
const DEFAULT_TOKEN_TTL: i64 = 86_400 * unix_time::MICROS_PER_SEC;

/// The `exp` lifetime claim of an access token JWT. The claims set is upstream-owned, so only the
/// claim the gateway reads is modeled and unknown claims are ignored.
#[derive(Debug, Deserialize)]
struct ExpClaim {
    /// Unix timestamp in seconds the token expires at (RFC 7519); scaled to the database clock
    /// on read.
    exp: Option<i64>,
}

/// The claims of the OpenAI identity token that the gateway reads (README §5.1). The claims set is
/// upstream-owned, so only the claim the gateway reads is modeled and unknown claims are ignored.
#[derive(Debug, Deserialize)]
struct OpenAiIdTokenClaims {
    /// The OpenAI authorization claim object carrying the ChatGPT account identifier.
    #[serde(rename = "https://api.openai.com/auth")]
    authorization: Option<OpenAiAuthorizationClaim>,
}

/// The OpenAI authorization claim object of an identity token.
#[derive(Debug, Deserialize)]
struct OpenAiAuthorizationClaim {
    /// ChatGPT account ID the credentials authenticate as.
    chatgpt_account_id: Option<String>,
}

/// Reads the ChatGPT account claim of an OpenAI identity token (README §5.1). `None` means the
/// token is not a decodable JWT or carries no usable account claim.
pub(super) fn openai_account_claim(id_token: &str) -> Option<String> {
    jwt_claims::<OpenAiIdTokenClaims>(id_token)?
        .authorization?
        .chatgpt_account_id
}

/// Computes the expiry of a token set in database-clock microseconds: a reported `expires_in`
/// when present, else the remaining lifetime of the access token's `exp` claim, else the default
/// lifetime.
///
/// Upstream reports `expires_in` in seconds while the stamp renders microseconds, so the reported
/// value is scaled before the stamp is computed. A non-positive `expires_in` is a malformed token
/// response and is rejected, and so is an `exp` claim already in the past: stamping an expiry at
/// or before `now` would mark the token permanently stale and drive a refresh grant against the
/// endpoint on every request, while substituting the default lifetime would present an
/// already-expired access token to the endpoint. The mirror extreme is the same malformed
/// response: a lifetime so large that the scaled stamp `now + lifetime * 1_000_000` leaves `i64`
/// would wrap to a hugely negative expiry in a release build — the permanently stale token
/// again — and panic in a debug one.
///
/// Google access tokens are opaque (`ya29…`) and carry no `exp` claim, so a Google refresh response
/// without `expires_in` has no better signal than the default lifetime; both Google and OpenAI
/// token endpoints report `expires_in`, which therefore always wins.
///
/// # Errors
///
/// Returns [`Error`] when the reported `expires_in` is zero or negative or overflows the token
/// expiry stamp, or when the access token's `exp` claim is not in the future.
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

    let now = unix_time::unix_now_micros();

    let lifetime_micros = match expires_in {
        Some(lifetime) => lifetime
            .checked_mul(unix_time::MICROS_PER_SEC)
            .ok_or_else(|| Error::Unavailable {
                detail: format!("malformed token response: `expires_in` overflow, got {lifetime}"),
            })?,
        None => access_token
            .map(access_token_expires_in)
            .transpose()?
            .flatten()
            .unwrap_or(DEFAULT_TOKEN_TTL),
    };

    // The plain add would wrap a hostile `expires_in` near `i64::MAX` to a hugely negative expiry
    // in a release build. Only a reported `expires_in` can reach that bound — the `exp`-claim
    // fallback leaves at most `i64::MAX - now` of remaining lifetime — so the error names the
    // reported value when there is one.
    now.checked_add(lifetime_micros)
        .ok_or_else(|| Error::Unavailable {
            detail: format!(
                "malformed token response: `expires_in` overflow, got {}",
                expires_in.unwrap_or(lifetime_micros)
            ),
        })
}

/// Decodes the claims a JWT payload carries, or `None` when the token is not a decodable JWT or its
/// payload does not carry the modeled claims.
fn jwt_claims<TClaims>(jwt: &str) -> Option<TClaims>
where
    TClaims: DeserializeOwned,
{
    let payload = jwt.split('.').nth(1)?;

    serde_json::from_slice(&general_purpose::URL_SAFE_NO_PAD.decode(payload).ok()?).ok()
}

/// Remaining lifetime of an access token in database-clock microseconds according to its `exp`
/// claim, or `None` when the token carries no decodable `exp` claim. A claim not in the
/// future — including one so far in the past that the remaining lifetime leaves `i64` — is a
/// malformed token response.
fn access_token_expires_in(access_token: &str) -> Result<Option<i64>, Error> {
    let Some(exp) = jwt_claims::<ExpClaim>(access_token).and_then(|claims| claims.exp) else {
        return Ok(None);
    };

    // The JWT `exp` claim renders seconds while the database clock renders microseconds; both the
    // scale-up and the subtraction are upstream-controlled and therefore checked. A product that
    // leaves `i64` would otherwise wrap to a huge positive lifetime in a release build and slip
    // past the non-future guard below.
    let exp_micros =
        exp.checked_mul(unix_time::MICROS_PER_SEC)
            .ok_or_else(|| Error::Unavailable {
                detail: format!(
                    "malformed token response: `exp` claim must be in the future, got {exp}"
                ),
            })?;

    let now = unix_time::unix_now_micros();

    let lifetime = exp_micros
        .checked_sub(now)
        .ok_or_else(|| Error::Unavailable {
            detail: format!(
                "malformed token response: `exp` claim must be in the future, got {exp}"
            ),
        })?;

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

#[cfg(test)]
mod tests {
    use super::*;

    /// Encodes a JWT whose payload carries the absolute `exp` claim `exp`, so the `exp` fallback of
    /// [`token_expiry_from`] can be exercised without a token endpoint.
    ///
    /// # Panics
    ///
    /// Panics if the test claim does not serialize.
    fn jwt_with_exp_claim(exp: i64) -> String {
        /// The projected expiry a test token carries.
        #[derive(serde::Serialize)]
        struct TestExpClaim {
            /// Unix timestamp in microseconds the token expires at.
            exp: i64,
        }

        #[expect(clippy::unwrap_used, reason = "the test claim always serializes")]
        let claims = serde_json::to_string(&TestExpClaim { exp }).unwrap();

        let payload = general_purpose::URL_SAFE_NO_PAD.encode(claims);

        format!("header.{payload}.signature")
    }

    /// Encodes a JWT whose payload carries `exp` at `now + remaining_secs`: the claim renders
    /// seconds per RFC 7519, like every upstream token does.
    fn jwt_with_exp(remaining_secs: i64) -> String {
        jwt_with_exp_claim(unix_time::unix_now() + remaining_secs)
    }

    /// The ChatGPT account claim is read from the nested
    /// `https://api.openai.com/auth` claim of an identity token (README §5.1);
    /// a token that is not a decodable JWT resolves no account instead of
    /// failing.
    ///
    /// # Panics
    ///
    /// Panics if the test identity token does not serialize.
    #[test]
    fn reads_openai_account_claim() {
        /// The claims a test identity token carries.
        #[derive(serde::Serialize)]
        struct TestIdTokenClaims {
            /// The OpenAI authorization claim object.
            #[serde(rename = "https://api.openai.com/auth")]
            authorization: TestAuthorizationClaim,
        }

        /// The OpenAI authorization claim object a test identity token carries.
        #[derive(serde::Serialize)]
        struct TestAuthorizationClaim {
            /// ChatGPT account ID the token authenticates as.
            chatgpt_account_id: String,
        }

        #[expect(
            clippy::unwrap_used,
            reason = "the test identity token always serializes"
        )]
        let claims = serde_json::to_string(&TestIdTokenClaims {
            authorization: TestAuthorizationClaim {
                chatgpt_account_id: "account-1".to_string(),
            },
        })
        .unwrap();

        let payload = general_purpose::URL_SAFE_NO_PAD.encode(claims);

        assert_eq!(
            openai_account_claim(&format!("header.{payload}.signature")).as_deref(),
            Some("account-1")
        );

        assert_eq!(openai_account_claim("not-a-jwt"), None);
    }

    /// An `exp` claim already in the past is malformed exactly like a non-positive `expires_in`:
    /// the response must fail instead of storing a permanently stale token or presenting an expired
    /// token under a default lifetime. A reported `expires_in` still wins and is never overridden
    /// by the spent claim.
    ///
    /// # Panics
    ///
    /// Panics if the expiry lookup or assertions fail.
    #[test]
    fn rejects_past_exp_claim_as_malformed() {
        let jwt = jwt_with_exp(-3_600);

        assert!(token_expiry_from(None, Some(&jwt)).is_err());

        #[expect(
            clippy::unwrap_used,
            reason = "the reported expires_in is constructed to be accepted"
        )]
        let expiry = token_expiry_from(Some(600), Some(&jwt)).unwrap();

        assert!(expiry > unix_time::unix_now_micros());
    }

    /// Without `expires_in`, a decodable future `exp` claim supplies the lifetime and a token with
    /// no usable claim (opaque Google `ya29…` tokens) falls through to the default lifetime instead
    /// of failing.
    ///
    /// # Panics
    ///
    /// Panics if the expiry lookup or assertions fail.
    #[test]
    fn falls_back_to_exp_claim_then_default_lifetime() {
        #[expect(
            clippy::unwrap_used,
            reason = "the future exp claim is constructed to be accepted"
        )]
        let expiry = token_expiry_from(None, Some(&jwt_with_exp(3_600))).unwrap();

        assert!((3_595_000_000..=3_600_000_000).contains(&(expiry - unix_time::unix_now_micros())));

        #[expect(
            clippy::unwrap_used,
            reason = "the opaque token falls back to the default lifetime"
        )]
        let expiry = token_expiry_from(None, Some("ya29-opaque-token")).unwrap();

        assert!(
            (86_399_000_000..=86_400_000_000).contains(&(expiry - unix_time::unix_now_micros()))
        );
    }

    /// An `expires_in` large enough that the stamp `now + expires_in` leaves `i64` is malformed
    /// exactly like a non-positive one: a release build would wrap the add to a hugely negative
    /// expiry — the permanently stale token that drives a refresh grant on every request — and a
    /// debug build would panic. Both bounds overflow for any current timestamp, so the rejection is
    /// deterministic.
    ///
    /// # Panics
    ///
    /// Panics if assertions fail.
    #[test]
    fn rejects_overflowing_expires_in_as_malformed() {
        for expires_in in [i64::MAX, i64::MAX - DEFAULT_TOKEN_TTL] {
            #[expect(
                clippy::unwrap_used,
                reason = "the overflowing expires_in is constructed to be rejected"
            )]
            let error = token_expiry_from(Some(expires_in), None).unwrap_err();

            assert!(
                error
                    .to_string()
                    .contains("malformed token response: `expires_in` overflow")
            );
        }
    }

    /// An `exp` claim so far in the past that the remaining lifetime `exp - now` leaves `i64` would
    /// wrap to a huge positive lifetime in a release build and slip past the non-future guard, so
    /// the subtraction is checked and the claim is rejected as malformed like an ordinary past
    /// claim.
    ///
    /// # Panics
    ///
    /// Panics if assertions fail.
    #[test]
    fn rejects_underflowing_exp_claim_as_malformed() {
        #[expect(
            clippy::unwrap_used,
            reason = "the underflowing exp claim is constructed to be rejected"
        )]
        let error = token_expiry_from(None, Some(&jwt_with_exp_claim(i64::MIN))).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("malformed token response: `exp` claim must be in the future")
        );
    }
}
