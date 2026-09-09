//! Management endpoints listing, saving, and deleting providers.

use super::{error_response, json_response, parse_body, provider_json};
use crate::{
    database::Database,
    error::ApiError,
    format::UpstreamFormat,
    provider::{self, Credentials, NewProvider, ProviderType, storage},
};
use axum::{body::Bytes, extract::State, response::Response};
use serde::Deserialize;
use serde_json::Value;

/// Strictly parsed `save-provider` request.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveProviderBody {
    /// Provider identifier.
    name: String,

    /// Account kind; required for creation.
    #[serde(default, rename = "type")]
    provider_type: Option<String>,

    /// Public model name prefix.
    #[serde(default)]
    model_prefix: Option<String>,

    /// Upstream protocol.
    #[serde(default)]
    protocol: Option<String>,

    /// Upstream base URL.
    #[serde(default)]
    base_url: Option<String>,

    /// Plaintext credentials payload.
    #[serde(default)]
    credentials: Option<Value>,

    /// Serving state.
    #[serde(default)]
    enabled: Option<bool>,
}

/// Strictly parsed `delete-provider` request.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteProviderBody {
    /// Provider identifier.
    name: String,
}

/// Handles `GET /list-providers`.
pub(crate) async fn list(State(database): State<Database>) -> Response {
    match storage::list_providers(&database).await {
        Ok(providers) => {
            let rendered = providers.iter().map(provider_json).collect::<Vec<_>>();

            json_response(200, serde_json::json!({ "providers": rendered }))
        }

        Err(error) => error_response(error),
    }
}

/// Handles `POST /save-provider`.
pub(crate) async fn save(State(database): State<Database>, body: Bytes) -> Response {
    let outcome = handle_save(&database, &body).await;

    match outcome {
        Ok(rendered) => json_response(200, serde_json::json!({ "provider": rendered })),
        Err(error) => error_response(error),
    }
}

/// Handles `POST /delete-provider`.
pub(crate) async fn delete(State(database): State<Database>, body: Bytes) -> Response {
    let outcome = async {
        let request: DeleteProviderBody = parse_body(&body)?;

        if !storage::delete_provider(&database, &request.name).await? {
            return Err(ApiError::not_found(format!(
                "Provider `{}` does not exist",
                request.name
            )));
        }

        Ok(())
    }
    .await;

    match outcome {
        Ok(()) => json_response(200, serde_json::json!({"deleted": true})),
        Err(error) => error_response(error),
    }
}

/// Creates or updates one provider per the management contract.
///
/// # Errors
///
/// Returns [`ApiError`] for invalid names, unknown types, OAuth creation
/// attempts, and type mismatches.
async fn handle_save(database: &Database, body: &Bytes) -> Result<serde_json::Value, ApiError> {
    let request: SaveProviderBody = parse_body(body)?;

    provider::validate_provider_name(&request.name)?;

    let existing = storage::find_provider(database, &request.name).await?;

    // The stored row pins the account kind: an omitted `type` defaults to it
    // on update, and an explicit mismatching `type` is an invalid request
    // rather than a conflict, because no concurrent state is overwritten.
    let provider_type = match request.provider_type.as_deref() {
        Some(raw) => {
            let provider_type = ProviderType::parse(raw).ok_or_else(|| {
                ApiError::invalid_parameter("type", format!("Unknown provider type `{raw}`"))
            })?;

            if let Some(existing) = &existing
                && provider_type != existing.provider_type
            {
                return Err(ApiError::invalid_parameter(
                    "type",
                    format!(
                        "Provider `{}` has type `{}` and cannot become `{}`",
                        existing.name,
                        existing.provider_type.as_str(),
                        provider_type.as_str()
                    ),
                ));
            }

            Some(provider_type)
        }

        None => existing.as_ref().map(|provider| provider.provider_type),
    };

    let protocol = match request.protocol.as_deref() {
        None => None,
        Some(raw) => Some(UpstreamFormat::parse(raw).ok_or_else(|| {
            ApiError::invalid_parameter("protocol", format!("Unknown protocol `{raw}`"))
        })?),
    };

    let credentials = match (&request.credentials, provider_type) {
        (None, _) => None,

        (Some(value), Some(kind)) => Some(Credentials::parse(kind, &value.to_string())?),

        (Some(_), None) => {
            return Err(ApiError::invalid_parameter(
                "type",
                "credentials require a `type` in the same request",
            ));
        }
    };

    if let Some(existing) = existing {
        let model_prefix = request
            .model_prefix
            .clone()
            .unwrap_or_else(|| existing.model_prefix.clone());

        let updated_protocol = protocol.or(existing.protocol);

        let base_url = request
            .base_url
            .clone()
            .or_else(|| existing.base_url.clone());

        let enabled = request.enabled.unwrap_or(existing.enabled);

        // One transactional write owns the provider row, any credentials
        // replacement, and the recomputed public model names; a prefix change
        // that would collide is rejected with 409 before anything is written.
        storage::update_provider(
            database,
            &existing,
            storage::ProviderUpdate {
                model_prefix: &model_prefix,
                protocol: updated_protocol,
                base_url: base_url.as_deref(),
                enabled,
                credentials: credentials.as_ref(),
            },
        )
        .await?;
    } else {
        let Some(provider_type) = provider_type else {
            return Err(ApiError::invalid_parameter(
                "type",
                "type is required when creating a provider",
            ));
        };

        if provider_type != ProviderType::OpenAiCompatible {
            return Err(ApiError::invalid_parameter(
                "type",
                format!(
                    "`{}` providers are created by completing an OAuth flow",
                    provider_type.as_str()
                ),
            ));
        }

        let protocol = protocol.ok_or_else(|| {
            ApiError::invalid_parameter(
                "protocol",
                "protocol is required for openai_compatible providers",
            )
        })?;

        let base_url = request.base_url.clone().ok_or_else(|| {
            ApiError::invalid_parameter(
                "base_url",
                "base_url is required for openai_compatible providers",
            )
        })?;

        let credentials = credentials.ok_or_else(|| {
            ApiError::invalid_parameter(
                "credentials",
                "credentials are required when creating a provider",
            )
        })?;

        storage::insert_provider(
            database,
            &NewProvider {
                name: &request.name,
                model_prefix: request.model_prefix.as_deref().unwrap_or_default(),
                provider_type,
                protocol: Some(protocol),
                base_url: Some(&base_url),
                credentials: &credentials,
                expires_at: None,
                enabled: request.enabled.unwrap_or(true),
            },
        )
        .await?;
    }

    let stored = storage::find_provider(database, &request.name)
        .await?
        .ok_or_else(|| ApiError::internal("provider vanished after save"))?;

    Ok(provider_json(&stored))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::NewProvider;
    use std::{
        env,
        error::Error as StdError,
        fs,
        num::NonZero,
        path::PathBuf,
        process,
        sync::atomic::{AtomicU64, Ordering},
        time::Duration,
    };

    /// Monotonic counter making every test database path unique within the
    /// process.
    static PATH_COUNTER: AtomicU64 = AtomicU64::new(0);

    /// Opens an isolated test database at a unique temporary path, returning
    /// the database and its file path for cleanup.
    ///
    /// # Panics
    ///
    /// Panics if opening the test database fails.
    async fn open_test_database() -> (Database, PathBuf) {
        let unique_id = PATH_COUNTER.fetch_add(1, Ordering::Relaxed);

        let path: PathBuf = env::temp_dir().join(format!(
            "janusgate_providers_test_{}_{unique_id}.db",
            process::id()
        ));

        #[expect(
            clippy::expect_used,
            reason = "the test pool size is a non-zero constant"
        )]
        let database = Database::open(
            &path,
            NonZero::new(2).expect("pool size is non-zero"),
            Duration::from_secs(5),
        )
        .await
        .expect("failed to open isolated test database");

        (database, path)
    }

    /// Inserts one `openai_compatible` provider row with a known API key.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while inserting the row.
    async fn insert_compatible_provider(
        database: &Database,
        name: &str,
    ) -> Result<(), Box<dyn StdError>> {
        storage::insert_provider(
            database,
            &NewProvider {
                name,
                model_prefix: "",
                provider_type: ProviderType::OpenAiCompatible,
                protocol: Some(UpstreamFormat::OpenAiChatCompletion),
                base_url: Some("https://upstream.example"),
                credentials: &Credentials::ApiKey("old-key".to_string()),
                expires_at: None,
                enabled: true,
            },
        )
        .await?;

        Ok(())
    }

    /// Reads back the stored credentials of a provider.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while querying the row.
    async fn read_credentials(
        database: &Database,
        name: &str,
    ) -> Result<Credentials, Box<dyn StdError>> {
        let stored = storage::find_provider(database, name)
            .await?
            .ok_or("missing provider row")?;

        Ok(stored.credentials)
    }

    /// Builds a JSON request body for `save-provider`.
    fn body(json: &str) -> Bytes {
        Bytes::from(json.as_bytes().to_vec())
    }

    /// Verifies an update carrying credentials without a `type` defaults to
    /// the stored provider type instead of demanding an explicit one: the
    /// stored row already pins the account kind.
    ///
    /// # Panics
    ///
    /// Panics if opening the test database or an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn save_provider_defaults_update_type_to_stored_row() -> Result<(), Box<dyn StdError>> {
        let (database, path) = open_test_database().await;

        insert_compatible_provider(&database, "alpha").await?;

        let rendered = handle_save(
            &database,
            &body(r#"{"name":"alpha","credentials":{"apiKey":"new-key"}}"#),
        )
        .await?;

        // handle_save renders the bare provider object; the route handler
        // adds the `{"provider": ...}` wrapper. Macro-invocation attributes
        // do not cover desugared index expressions, so the rendered field is
        // read through an annotated binding first.
        #[expect(
            clippy::indexing_slicing,
            reason = "provider_json always renders nested objects, so string keys never panic"
        )]
        let rendered_type = rendered["type"].clone();

        assert_eq!(rendered_type, "openai_compatible");

        assert_eq!(
            read_credentials(&database, "alpha").await?,
            Credentials::ApiKey("new-key".to_string())
        );

        let _ = fs::remove_file(path);

        Ok(())
    }

    /// Verifies an explicit matching `type` is still accepted on update.
    ///
    /// # Panics
    ///
    /// Panics if opening the test database or an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn save_provider_accepts_explicit_matching_type() -> Result<(), Box<dyn StdError>> {
        let (database, path) = open_test_database().await;

        insert_compatible_provider(&database, "alpha").await?;

        handle_save(
            &database,
            &body(
                r#"{"name":"alpha","type":"openai_compatible","credentials":{"apiKey":"key-2"}}"#,
            ),
        )
        .await?;

        assert_eq!(
            read_credentials(&database, "alpha").await?,
            Credentials::ApiKey("key-2".to_string())
        );

        let _ = fs::remove_file(path);

        Ok(())
    }

    /// Verifies an explicit mismatching `type` on update is an invalid
    /// request (400) naming the field: the stored row pins the account kind,
    /// so the request can never be satisfied.
    ///
    /// # Panics
    ///
    /// Panics if opening the test database or an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn save_provider_rejects_mismatched_update_type_as_invalid_parameter()
    -> Result<(), Box<dyn StdError>> {
        let (database, path) = open_test_database().await;

        insert_compatible_provider(&database, "alpha").await?;

        let error = handle_save(
            &database,
            &body(r#"{"name":"alpha","type":"openai_oauth"}"#),
        )
        .await
        .err()
        .ok_or("a mismatching update type must be rejected")?;

        assert_eq!(error.status, 400);

        assert_eq!(error.param.as_deref(), Some("type"));

        let _ = fs::remove_file(path);

        Ok(())
    }
}
