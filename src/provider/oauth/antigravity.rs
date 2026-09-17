//! Transport for the Antigravity upstream.
//!
//! The reference port sends richer headers on these calls (an Electron `User-Agent` plus
//! `Client-Metadata` in `fetchProjectID` at rev `16e0056`); the gateway sends the mandatory
//! `User-Agent: antigravity` alone, which the endpoint accepts — verified live.

use crate::provider::{endpoints, http_body, http_body::ExchangeFailure};
use reqwest::{Client, header};
use serde::{Serialize, de::DeserializeOwned};

/// POSTs a JSON body to an Antigravity action on the Antigravity base URL, authenticated with
/// the given access token.
///
/// Every Antigravity call — project resolution and model discovery — routes through
/// here, so the mandatory `User-Agent: antigravity` header (README §5.2) is set in exactly one
/// place. The failure is the shared [`http_body::ExchangeFailure`]; each caller classifies it into
/// its own error domain.
pub(in crate::provider) async fn post_antigravity<T>(
    http: &Client,
    action: &str,
    access_token: &str,
    body: &impl Serialize,
) -> Result<T, ExchangeFailure>
where
    T: DeserializeOwned,
{
    let request = http
        .post(format!("{}{action}", endpoints::ANTIGRAVITY_BASE_URL))
        .bearer_auth(access_token)
        .header(header::USER_AGENT, endpoints::ANTIGRAVITY_USER_AGENT)
        .json(body);

    http_body::send_and_parse_json(request, "upstream").await
}
