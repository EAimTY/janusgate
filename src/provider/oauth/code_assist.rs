//! Transport for the Cloud Code Assist API of the Antigravity upstream.

use crate::provider::{endpoints, http_body};
use reqwest::Client;
use serde::{Serialize, de::DeserializeOwned};

/// POSTs a JSON body to a Cloud Code Assist action of the Antigravity base
/// URL, authenticated with the given access token.
///
/// Every Antigravity Code Assist call — project resolution and model
/// discovery — routes through here, so the mandatory `User-Agent: antigravity`
/// header (README §5.2) is set in exactly one place. The failure is the
/// shared [`http_body::ExchangeFailure`]; each caller classifies it into its
/// own error domain.
pub(in crate::provider) async fn post_code_assist<T>(
    http: &Client,
    action: &str,
    access_token: &str,
    body: &impl Serialize,
) -> Result<T, http_body::ExchangeFailure>
where
    T: DeserializeOwned,
{
    let request = http
        .post(format!("{}{action}", endpoints::ANTIGRAVITY_BASE_URL))
        .bearer_auth(access_token)
        .header(
            reqwest::header::USER_AGENT,
            endpoints::ANTIGRAVITY_USER_AGENT,
        )
        .json(body);

    http_body::send_and_parse_json(request, "upstream").await
}
