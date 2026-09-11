//! Management endpoints listing, saving, and deleting providers.

use super::{error_response, parse_body, provider_json, validate_identifier};
use crate::{
    database::Database,
    error::{self, ApiError, log_api_error},
    format::UpstreamFormat,
    provider::{Credentials, NewProvider, ProviderType, storage},
};
use axum::{body::Bytes, extract::State, response::Response};
use log::info;
use serde::Deserialize;
use serde_json::Value;

/// Strictly parsed `save-provider` request. `id` selects the provider to
/// update — its absence creates one. `name` and `model_prefix` are required
/// non-nullable strings: every request states the naming, and their content
/// is the caller's choice (README §7.1); supplying a different `name` for an
/// existing provider renames it. The remaining fields are plain optional:
/// absent keeps the stored value.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveProviderBody {
    /// Provider row identifier; absent creates a new provider.
    id: Option<i64>,

    /// Unique provider name; must be non-empty. Renames the provider when it
    /// differs from the stored name.
    name: String,

    /// Public model name prefix; may be empty.
    model_prefix: String,

    /// Account kind; required for creation.
    #[serde(rename = "type")]
    provider_type: Option<String>,

    /// Upstream protocol.
    protocol: Option<String>,

    /// Upstream base URL.
    base_url: Option<String>,

    /// Plaintext credentials payload.
    credentials: Option<Value>,

    /// Serving state.
    enabled: Option<bool>,
}

/// Strictly parsed `delete-provider` request.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteProviderBody {
    /// Provider row identifier.
    id: i64,
}

/// Handles `GET /list-providers`.
pub(crate) async fn list(State(database): State<Database>) -> Response {
    match storage::list_providers(&database).await {
        Ok(providers) => {
            let rendered = providers.iter().map(provider_json).collect::<Vec<_>>();

            error::json_response(200, serde_json::json!({ "providers": rendered }))
        }

        Err(error) => {
            log_api_error("list-providers", &error);

            error_response(error)
        }
    }
}

/// Handles `POST /save-provider`.
pub(crate) async fn save(State(database): State<Database>, body: Bytes) -> Response {
    let outcome = handle_save(&database, &body).await;

    match outcome {
        Ok(rendered) => error::json_response(200, serde_json::json!({ "provider": rendered })),
        Err(error) => error_response(error),
    }
}

/// Handles `POST /delete-provider`.
pub(crate) async fn delete(State(database): State<Database>, body: Bytes) -> Response {
    let outcome = async {
        let request: DeleteProviderBody = parse_body(&body, "delete-provider")?;

        if !storage::delete_provider(&database, request.id).await? {
            return Err(ApiError::not_found(format!(
                "Provider {} does not exist",
                request.id
            )));
        }

        Ok(request.id)
    }
    .await;

    match outcome {
        Ok(id) => {
            info!("deleted provider {id}");

            error::json_response(200, serde_json::json!({"deleted": true}))
        }
        Err(error) => {
            log_api_error("delete-provider", &error);

            error_response(error)
        }
    }
}

/// Creates or updates one provider per the management contract. A request
/// carrying `id` updates (and may rename) that provider; a request without
/// `id` creates one. The outcome is logged: every failure at the provider's
/// response boundary, the successful creation or update at info level.
///
/// # Errors
///
/// Returns [`ApiError`] for empty names, unknown ids, unknown types, OAuth
/// creation attempts, type mismatches, taken names, and public name
/// collisions.
async fn handle_save(database: &Database, body: &Bytes) -> Result<serde_json::Value, ApiError> {
    let request: SaveProviderBody = parse_body(body, "save-provider")?;

    let outcome = async {
        validate_identifier("name", &request.name)?;

        // The outer read only pins request validation: the stored type is
        // immutable, so it cannot drift before the transactional write. Absent
        // fields are merged from the row re-read inside `update_provider`'s
        // transaction, never from this snapshot (README §7.1).
        let existing = match request.id {
            Some(id) => Some(
                storage::find_provider_by_id(database, id)
                    .await?
                    .ok_or_else(|| ApiError::not_found(format!("Provider {id} does not exist")))?,
            ),

            // Creation checks name availability here for a clean 409; the
            // database's UNIQUE constraint is the concurrent backstop.
            None => {
                if storage::find_provider(database, &request.name)
                    .await?
                    .is_some()
                {
                    return Err(ApiError::conflict(format!(
                        "Provider name `{}` is already taken",
                        request.name
                    )));
                }

                None
            }
        };

        // The stored row pins the account kind: an omitted `type` defaults to
        // it on update, and an explicit mismatching `type` is an
        // invalid request rather than a conflict, because no concurrent
        // state is overwritten.
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

        if let Some(id) = request.id {
            // One transactional write owns the provider row — the supplied
            // `name` included, so a different name renames the provider —
            // any credentials replacement, and the recomputed public model
            // names; a taken name or a prefix change that would collide is
            // rejected with 409 before anything is written.
            let stored = storage::update_provider(
                database,
                id,
                storage::ProviderUpdate {
                    name: &request.name,
                    model_prefix: &request.model_prefix,
                    protocol,
                    base_url: request.base_url.as_deref(),
                    is_enabled: request.enabled,
                    credentials: credentials.as_ref(),
                },
            )
            .await?;

            info!("updated provider `{}`", request.name);

            return Ok(provider_json(&stored));
        }

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

        let base_url = request.base_url.as_deref().ok_or_else(|| {
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
                model_prefix: &request.model_prefix,
                provider_type,
                protocol: Some(protocol),
                base_url: Some(base_url),
                credentials: &credentials,
                expires_at: None,
                is_enabled: request.enabled.unwrap_or(true),
            },
        )
        .await?;

        let stored = storage::find_provider(database, &request.name)
            .await?
            .ok_or_else(|| ApiError::internal("provider vanished after save"))?;

        info!("created provider `{}`", request.name);

        Ok(provider_json(&stored))
    }
    .await;

    if let Err(error) = &outcome {
        log_api_error(&format!("save-provider `{}`", request.name), error);
    }

    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support;
    use std::{error::Error as StdError, time::Duration};

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

    /// Verifies an update carrying credentials without a `type` defaults to
    /// the stored provider type instead of demanding an explicit one: the
    /// stored row already pins the account kind.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn save_provider_defaults_update_type_to_stored_row() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(2, Duration::from_secs(5)).await?;

        let provider_id =
            test_support::seed_compatible_provider(&database, "alpha", "old-key").await?;

        let rendered = handle_save(
            &database,
            &test_support::body(&format!(
                r#"{{"id":{provider_id},"name":"alpha","model_prefix":"",
                    "credentials":{{"apiKey":"new-key"}}}}"#
            )),
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

        Ok(())
    }

    /// Verifies an explicit matching `type` is still accepted on update.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn save_provider_accepts_explicit_matching_type() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(2, Duration::from_secs(5)).await?;

        let provider_id =
            test_support::seed_compatible_provider(&database, "alpha", "old-key").await?;

        handle_save(
            &database,
            &test_support::body(&format!(
                r#"{{"id":{provider_id},"name":"alpha","model_prefix":"","type":"openai_compatible",
                    "credentials":{{"apiKey":"key-2"}}}}"#
            )),
        )
        .await?;

        assert_eq!(
            read_credentials(&database, "alpha").await?,
            Credentials::ApiKey("key-2".to_string())
        );

        Ok(())
    }

    /// Verifies an explicit mismatching `type` on update is an invalid
    /// request (400) naming the field: the stored row pins the account kind,
    /// so the request can never be satisfied.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn save_provider_rejects_mismatched_update_type_as_invalid_parameter()
    -> Result<(), Box<dyn StdError>> {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(2, Duration::from_secs(5)).await?;

        let provider_id =
            test_support::seed_compatible_provider(&database, "alpha", "old-key").await?;

        let error = handle_save(
            &database,
            &test_support::body(&format!(
                r#"{{"id":{provider_id},"name":"alpha","model_prefix":"","type":"openai_oauth"}}"#
            )),
        )
        .await
        .err()
        .ok_or("a mismatching update type must be rejected")?;

        assert_eq!(error.status, 400);

        assert_eq!(error.param.as_deref(), Some("type"));

        Ok(())
    }

    /// Verifies OAuth providers cannot be created through `save-provider`:
    /// they come into existence only by completing an OAuth flow, so a
    /// creation attempt is rejected with 400 naming `type` (README §7.1).
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn save_provider_rejects_oauth_creation() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(2, Duration::from_secs(5)).await?;

        for (name, provider_type) in [
            ("fresh-oai", "openai_oauth"),
            ("fresh-agy", "antigravity_oauth"),
        ] {
            let body = test_support::body(&format!(
                r#"{{"name":"{name}","model_prefix":"","type":"{provider_type}"}}"#
            ));

            let error = handle_save(&database, &body)
                .await
                .err()
                .ok_or("creating an OAuth provider must be rejected")?;

            assert_eq!(error.status, 400);

            assert_eq!(error.param.as_deref(), Some("type"));

            assert!(
                error.message.contains("OAuth"),
                "the error must point at the OAuth flow: {error}"
            );

            assert!(
                storage::find_provider(&database, name).await?.is_none(),
                "a rejected creation must write no row"
            );
        }

        Ok(())
    }

    /// Verifies `openai_compatible` creation requires both `protocol` and
    /// `base_url`, each rejection naming its field (README §7.1).
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn save_provider_requires_protocol_and_base_url_on_creation()
    -> Result<(), Box<dyn StdError>> {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(2, Duration::from_secs(5)).await?;

        let missing_protocol = test_support::body(
            r#"{"name":"no-protocol","model_prefix":"","type":"openai_compatible",
                "base_url":"https://upstream.example","credentials":{"apiKey":"k"}}"#,
        );

        let error = handle_save(&database, &missing_protocol)
            .await
            .err()
            .ok_or("creation without protocol must be rejected")?;

        assert_eq!(error.status, 400);

        assert_eq!(error.param.as_deref(), Some("protocol"));

        let missing_base_url = test_support::body(
            r#"{"name":"no-base-url","model_prefix":"","type":"openai_compatible",
                "protocol":"openai_chat_completion","credentials":{"apiKey":"k"}}"#,
        );

        let error = handle_save(&database, &missing_base_url)
            .await
            .err()
            .ok_or("creation without base_url must be rejected")?;

        assert_eq!(error.status, 400);

        assert_eq!(error.param.as_deref(), Some("base_url"));

        Ok(())
    }

    /// Verifies credentials supplied without a `type` on creation are
    /// rejected with 400 naming `type`: the credentials cannot be interpreted
    /// without the account kind they belong to (README §7.1).
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn save_provider_rejects_credentials_without_type_on_creation()
    -> Result<(), Box<dyn StdError>> {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(2, Duration::from_secs(5)).await?;

        let body = test_support::body(
            r#"{"name":"typeless","model_prefix":"","credentials":{"apiKey":"k"}}"#,
        );

        let error = handle_save(&database, &body)
            .await
            .err()
            .ok_or("credentials without type on creation must be rejected")?;

        assert_eq!(error.status, 400);

        assert_eq!(error.param.as_deref(), Some("type"));

        assert!(
            error.message.contains("credentials require a `type`"),
            "the error must state the missing type: {error}"
        );

        assert!(
            storage::find_provider(&database, "typeless")
                .await?
                .is_none(),
            "a rejected creation must write no row"
        );

        Ok(())
    }

    /// Verifies `model_prefix` is required on every `save-provider` request —
    /// creation and update alike — with no default: an omitted prefix fails
    /// strict parsing with 400 naming the field (README §7.1).
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn save_provider_requires_model_prefix_on_every_request() -> Result<(), Box<dyn StdError>>
    {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(2, Duration::from_secs(5)).await?;

        let creation = handle_save(
            &database,
            &test_support::body(
                r#"{"name":"prefixless","type":"openai_compatible",
                    "protocol":"openai_chat_completion","base_url":"https://upstream.example",
                    "credentials":{"apiKey":"k"}}"#,
            ),
        )
        .await
        .err()
        .ok_or("creation without model_prefix must be rejected")?;

        assert_eq!(creation.status, 400);

        assert!(
            creation.message.contains("model_prefix"),
            "the parse failure must name the missing field: {creation}"
        );

        assert!(
            storage::find_provider(&database, "prefixless")
                .await?
                .is_none(),
            "a rejected creation must write no row"
        );

        let provider_id =
            test_support::seed_compatible_provider(&database, "alpha", "test-key").await?;

        let update = handle_save(
            &database,
            &test_support::body(&format!(
                r#"{{"id":{provider_id},"name":"alpha","enabled":false}}"#
            )),
        )
        .await
        .err()
        .ok_or("an update without model_prefix must be rejected")?;

        assert_eq!(update.status, 400);

        assert!(
            update.message.contains("model_prefix"),
            "the parse failure must name the missing field: {update}"
        );

        let stored = storage::find_provider_by_id(&database, provider_id)
            .await?
            .ok_or("missing provider row")?;

        assert!(
            stored.is_enabled,
            "the rejected update must not have touched the row"
        );

        Ok(())
    }

    /// Verifies the naming contract on creation: the prefix may be the empty
    /// string, and the name's content is the caller's choice accepted
    /// verbatim — the backend enforces only non-emptiness (README §7.1).
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn save_provider_keeps_empty_prefix_and_verbatim_name() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(2, Duration::from_secs(5)).await?;

        let name = "My Provider! ä";

        let body = test_support::body(&format!(
            r#"{{"name":"{name}","model_prefix":"","type":"openai_compatible",
                "protocol":"openai_chat_completion","base_url":"https://upstream.example",
                "credentials":{{"apiKey":"k"}}}}"#
        ));

        handle_save(&database, &body).await?;

        let stored = storage::find_provider(&database, name)
            .await?
            .ok_or("the created provider row must exist")?;

        assert_eq!(stored.name, name);

        assert_eq!(stored.model_prefix, "");

        Ok(())
    }

    /// Verifies an empty provider name is rejected with 400 naming `name`
    /// and writes no row: names are non-nullable and non-empty, with no
    /// fallback (README §7.1).
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn save_provider_rejects_empty_name() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(2, Duration::from_secs(5)).await?;

        let body = test_support::body(
            r#"{"name":"","model_prefix":"","type":"openai_compatible",
                "protocol":"openai_chat_completion","base_url":"https://upstream.example",
                "credentials":{"apiKey":"k"}}"#,
        );

        let error = handle_save(&database, &body)
            .await
            .err()
            .ok_or("an empty provider name must be rejected")?;

        assert_eq!(error.status, 400);

        assert_eq!(error.param.as_deref(), Some("name"));

        let providers = storage::list_providers(&database).await?;

        assert!(
            providers.is_empty(),
            "a rejected creation must write no row"
        );

        Ok(())
    }

    /// Verifies `delete-provider` addresses providers by id and rejects an
    /// unknown id with 404 (README §7.1).
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn delete_provider_rejects_unknown_id() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(2, Duration::from_secs(5)).await?;

        let provider_id =
            test_support::seed_compatible_provider(&database, "alpha", "test-key").await?;

        let response = delete(State(database.clone()), test_support::body(r#"{"id":999}"#)).await;

        assert_eq!(response.status(), 404);

        let stored = storage::find_provider_by_id(&database, provider_id)
            .await?
            .ok_or("missing provider row")?;

        assert_eq!(stored.id, provider_id);

        Ok(())
    }

    /// Verifies a `save-provider` update carrying a different `name` renames
    /// the provider in place: the id is stable, the models follow, and the
    /// new name passes through verbatim (README §7.1).
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn save_provider_renames_provider_by_id() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(2, Duration::from_secs(5)).await?;

        let provider_id =
            test_support::seed_compatible_provider(&database, "alpha", "test-key").await?;

        let rendered = handle_save(
            &database,
            &test_support::body(&format!(
                r#"{{"id":{provider_id},"name":"My Renamed Provider","model_prefix":""}}"#
            )),
        )
        .await?;

        // handle_save renders the bare provider object; the route handler
        // adds the `{"provider": ...}` wrapper. Macro-invocation attributes
        // do not cover desugared index expressions, so the rendered fields
        // are read through annotated bindings first.
        #[expect(
            clippy::indexing_slicing,
            reason = "provider_json always renders nested objects, so string keys never panic"
        )]
        let rendered_id = rendered["id"].clone();

        #[expect(
            clippy::indexing_slicing,
            reason = "provider_json always renders nested objects, so string keys never panic"
        )]
        let rendered_name = rendered["name"].clone();

        assert_eq!(rendered_id, provider_id);

        assert_eq!(rendered_name, "My Renamed Provider");

        let stored = storage::find_provider_by_id(&database, provider_id)
            .await?
            .ok_or("missing provider row")?;

        assert_eq!(stored.name, "My Renamed Provider");

        Ok(())
    }

    /// Verifies renaming a provider onto a name another provider already
    /// holds is rejected with 409 and leaves both rows untouched (README
    /// §7.1).
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn save_provider_rejects_taken_name_on_rename() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(2, Duration::from_secs(5)).await?;

        let alpha_id =
            test_support::seed_compatible_provider(&database, "alpha", "test-key").await?;

        test_support::seed_compatible_provider(&database, "beta", "test-key").await?;

        let error = handle_save(
            &database,
            &test_support::body(&format!(
                r#"{{"id":{alpha_id},"name":"beta","model_prefix":""}}"#
            )),
        )
        .await
        .err()
        .ok_or("a rename onto a taken name must be rejected")?;

        assert_eq!(error.status, 409);

        assert!(
            error.message.contains("already taken"),
            "the error must state the taken name: {error}"
        );

        let stored = storage::find_provider_by_id(&database, alpha_id)
            .await?
            .ok_or("missing provider row")?;

        assert_eq!(stored.name, "alpha");

        Ok(())
    }

    /// Verifies creating a provider with a name that is already taken is
    /// rejected with 409 (README §7.1).
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn save_provider_rejects_taken_name_on_creation() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(2, Duration::from_secs(5)).await?;

        test_support::seed_compatible_provider(&database, "alpha", "test-key").await?;

        let body = test_support::body(
            r#"{"name":"alpha","model_prefix":"","type":"openai_compatible",
                "protocol":"openai_chat_completion","base_url":"https://upstream.example",
                "credentials":{"apiKey":"k"}}"#,
        );

        let error = handle_save(&database, &body)
            .await
            .err()
            .ok_or("creation with a taken name must be rejected")?;

        assert_eq!(error.status, 409);

        assert!(
            error.message.contains("already taken"),
            "the error must state the taken name: {error}"
        );

        Ok(())
    }
}
