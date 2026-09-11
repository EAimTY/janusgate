//! Management endpoints discovering, listing, and overriding models.

use super::{error_response, parse_body, validate_identifier};
use crate::{
    database::Database,
    error::{self, ApiError, log_api_error},
    provider::{
        discovery, oauth,
        storage::{self, ModelEntry},
    },
};
use axum::{body::Bytes, extract::State, response::Response};
use log::info;
use serde::Deserialize;
use serde_json::Value;

/// Strictly parsed `discover-models` request.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiscoverModelsBody {
    /// Provider row identifier.
    id: i64,
}

/// Strictly parsed `save-model` request; each override field is tri-state:
/// absent leaves it unchanged, `null` clears it, a value sets it. The
/// `double_option` adapter is what makes the mapping explicit, since plain
/// serde maps JSON `null` and field absence to the same outer `None`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveModelBody {
    /// Owning provider row identifier.
    provider_id: i64,

    /// Exact upstream model identifier; must be non-empty.
    upstream_name: String,

    /// Public name override.
    #[serde(default, with = "::serde_with::rust::double_option")]
    name_override: Option<Option<String>>,

    /// Serving state override.
    #[serde(default, with = "::serde_with::rust::double_option")]
    enabled_override: Option<Option<bool>>,

    /// Canonical thinking levels override.
    #[serde(default, with = "::serde_with::rust::double_option")]
    thinking_levels_override: Option<Option<Vec<String>>>,
}

/// Handles `POST /discover-models`.
pub(crate) async fn discover(State(database): State<Database>, body: Bytes) -> Response {
    let outcome = handle_discover(&database, &body).await;

    match outcome {
        Ok(discovered) => {
            error::json_response(200, serde_json::json!({ "discovered": discovered }))
        }
        Err(error) => error_response(error),
    }
}

/// Handles `GET /list-models`.
pub(crate) async fn list(State(database): State<Database>) -> Response {
    let outcome = async {
        let entries = storage::list_model_entries(&database).await?;

        let rendered = entries
            .iter()
            .map(model_entry_json)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(serde_json::json!({ "models": rendered }))
    }
    .await;

    match outcome {
        Ok(rendered) => error::json_response(200, rendered),
        Err(error) => {
            log_api_error("list-models", &error);

            error_response(error)
        }
    }
}

/// Handles `POST /save-model`.
pub(crate) async fn save(State(database): State<Database>, body: Bytes) -> Response {
    let outcome = handle_save(&database, &body).await;

    match outcome {
        Ok(rendered) => error::json_response(200, serde_json::json!({ "model": rendered })),
        Err(error) => error_response(error),
    }
}

/// Refreshes one provider's model rows from upstream, carrying overrides over
/// by `upstream_name`. The outcome is logged: the discovered count at info
/// level, every failure at the provider's response boundary.
///
/// # Errors
///
/// Returns [`ApiError`] for an empty provider identifier, unknown providers,
/// upstream failures, and public name collisions.
async fn handle_discover(database: &Database, body: &Bytes) -> Result<usize, ApiError> {
    let request: DiscoverModelsBody = parse_body(body, "discover-models")?;

    let outcome = async {
        let provider = storage::find_provider_by_id(database, request.id)
            .await?
            .ok_or_else(|| {
                ApiError::not_found(format!("Provider {} does not exist", request.id))
            })?;

        let fresh = oauth::ensure_fresh_token(database, crate::http(), &provider).await?;

        let upstream_names = discovery::discover_upstream_models(crate::http(), &fresh).await?;

        // Discovery payloads can repeat an upstream name; keep the first
        // occurrence.
        let mut unique_names: Vec<String> = Vec::with_capacity(upstream_names.len());

        for upstream_name in upstream_names {
            if unique_names
                .iter()
                .any(|existing| existing == &upstream_name)
            {
                continue;
            }

            unique_names.push(upstream_name);
        }

        // The replace re-reads the provider row inside its transaction, so the
        // public names are composed from the provider's current prefix and
        // checked for collisions atomically with the write (README
        // §7.1).
        storage::replace_discovered_models(database, request.id, &unique_names).await
    }
    .await;

    match &outcome {
        Ok(discovered) => {
            info!("discovered {discovered} models for provider {}", request.id)
        }

        Err(error) => {
            log_api_error(
                &format!("model discovery for provider {}", request.id),
                error,
            );
        }
    }

    outcome
}

/// Writes one model's overrides and recomputes its public name. The outcome
/// is logged: the saved overrides at info level, every failure at the
/// model's response boundary.
///
/// # Errors
///
/// Returns [`ApiError`] for empty identifiers, unknown rows, invalid thinking
/// levels, and public name collisions.
async fn handle_save(database: &Database, body: &Bytes) -> Result<Value, ApiError> {
    let request: SaveModelBody = parse_body(body, "save-model")?;

    let outcome = async {
        validate_identifier("upstream_name", &request.upstream_name)?;

        let thinking_levels_override = match &request.thinking_levels_override {
            None => None,

            Some(None) => Some(None),

            Some(Some(levels)) => {
                // The shared parser keeps save-model validation byte-identical
                // with the stored-override contract (README §7.1).
                let parsed = crate::ir::parse_thinking_levels(levels, "thinking_levels_override")?;

                let canonical: Vec<&str> =
                    parsed.iter().map(|level| level.to_canonical()).collect();

                Some(Some(serde_json::to_string(&canonical).map_err(
                    |error| {
                        ApiError::internal(format!("failed to encode thinking levels: {error}"))
                    },
                )?))
            }
        };

        // The checked update re-reads the provider and model rows inside its
        // transaction, so the recomputed public name uses the provider's
        // current prefix and the collision check applies atomically
        // with the write (README §7.1).
        storage::update_model_overrides_checked(
            database,
            storage::ModelOverrideUpdate {
                provider_id: request.provider_id,
                upstream_name: &request.upstream_name,
                name_override: request.name_override.clone(),
                enabled_override: request.enabled_override,
                thinking_levels_override,
            },
        )
        .await
    }
    .await
    .and_then(|entry| model_entry_json(&entry));

    match &outcome {
        Ok(_) => info!(
            "saved the overrides of model `{}`/`{}`",
            request.provider_id, request.upstream_name
        ),

        Err(error) => log_api_error(
            &format!(
                "save-model `{}`/`{}`",
                request.provider_id, request.upstream_name
            ),
            error,
        ),
    }

    outcome
}

/// Renders one model listing entry.
///
/// # Errors
///
/// Returns [`ApiError`] when the stored thinking-level override is corrupt.
fn model_entry_json(entry: &ModelEntry) -> Result<Value, ApiError> {
    let thinking_levels_override = thinking_levels_json(&entry.model.thinking_levels_override)?;

    // The effective status comes from the canonical rule; the management wire
    // name stays `enabled` (README §7.1). `provider_id` is the public
    // identifier and `provider` the renameable label beside it.
    Ok(serde_json::json!({
        "provider_id": entry.provider_id,
        "provider": entry.provider_name,
        "upstream_name": entry.model.upstream_name,
        "name": entry.model.name,
        "name_override": entry.model.name_override,
        "enabled": entry.model.is_effectively_enabled(entry.is_provider_enabled),
        "enabled_override": entry.model.enabled_override,
        "thinking_levels_override": thinking_levels_override,
    }))
}

/// Renders the stored thinking-level override for listing, validating it
/// through the shared parser: a corrupt stored value is drift inside the
/// database — an invariant violation, symmetric with the gateway's fail-fast
/// — and is never masked as an absent override.
///
/// # Errors
///
/// Returns [`ApiError`] when the stored override is corrupt.
fn thinking_levels_json(stored: &Option<String>) -> Result<Value, ApiError> {
    let Some(json) = stored else {
        return Ok(Value::Null);
    };

    let levels = crate::ir::parse_thinking_levels_override(json).map_err(|error| {
        ApiError::internal(format!(
            "stored thinking_levels_override is invalid: {error}"
        ))
    })?;

    let canonical: Vec<&str> = levels.iter().map(|level| level.to_canonical()).collect();

    serde_json::to_value(canonical)
        .map_err(|error| ApiError::internal(format!("failed to encode thinking levels: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{database::params, test_support};
    use axum::{
        Router,
        body::{Body, to_bytes},
        http::{Request as HttpRequest, StatusCode},
    };
    use serde_json::Value;
    use std::{error::Error as StdError, time::Duration};
    use tower::ServiceExt as _;

    /// The management token the shared test configuration installs.
    const MANAGEMENT_TOKEN: &str = "test-mgmt";

    /// Inserts one `openai_compatible` provider row with one model row whose
    /// stored thinking-level override is given, returning nothing.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while inserting the rows.
    async fn insert_compatible_model(
        database: &Database,
        thinking_levels_override: Option<&str>,
    ) -> Result<i64, Box<dyn StdError>> {
        database
            .execute(
                "INSERT INTO providers (name, model_prefix, type, protocol, base_url, credentials) \
                 VALUES ('alpha', '', 'openai_compatible', 'openai_chat_completion', \
                 'https://upstream.example', '{\"apiKey\":\"test-key\"}')",
                (),
            )
            .await?;

        let rows = database
            .query("SELECT id FROM providers WHERE name = 'alpha'", ())
            .await?;

        let provider_id: i64 = rows.first().ok_or("missing provider row")?.get(0)?;

        database
            .execute(
                "INSERT INTO models (provider_id, upstream_name, name, thinking_levels_override) \
                 VALUES (?1, 'u1', 'u1', ?2)",
                params![provider_id, thinking_levels_override],
            )
            .await?;

        Ok(provider_id)
    }

    /// Builds a JSON request body for `save-model`.
    fn body(json: &str) -> Bytes {
        Bytes::from(json.as_bytes().to_vec())
    }

    /// Drives one `save-model` request through the management router,
    /// returning the status and the rendered JSON body.
    ///
    /// # Panics
    ///
    /// Panics if the request cannot be built or the in-memory oneshot serve
    /// fails, which only a router construction bug can cause.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while building the request or reading
    /// the response body.
    async fn save_model(
        router: &Router,
        json: &str,
    ) -> Result<(StatusCode, Value), Box<dyn StdError>> {
        let request = HttpRequest::builder()
            .method("POST")
            .uri("/save-model")
            .header("authorization", MANAGEMENT_TOKEN)
            .body(Body::from(json.to_string()))?;

        // SAFETY: not applicable; the in-memory oneshot serve is infallible,
        // so the unreachable arm documents the invariant.
        let response = match router.clone().oneshot(request).await {
            Ok(response) => response,
            Err(error) => unreachable!("in-memory oneshot cannot fail: {error}"),
        };

        let status = response.status();

        let body = to_bytes(response.into_body(), 1 << 20).await?;

        Ok((status, serde_json::from_slice(&body)?))
    }

    /// Verifies listing models surfaces a corrupt stored thinking-level
    /// override as an internal error (500) instead of masking the drift as an
    /// absent override, symmetric with the gateway's fail-fast.
    ///
    /// # Panics
    ///
    /// Panics if opening the test database or an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn list_surfaces_corrupt_stored_override_as_internal() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(2, Duration::from_secs(5)).await?;

        insert_compatible_model(&database, Some("[\"turbo\"]")).await?;

        let response = list(State(database)).await;

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

        Ok(())
    }

    /// Verifies save-model rejects an unknown thinking level with 400 naming
    /// the `thinking_levels_override` parameter.
    ///
    /// # Panics
    ///
    /// Panics if opening the test database or an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn save_model_rejects_unknown_thinking_level_with_parameter()
    -> Result<(), Box<dyn StdError>> {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(2, Duration::from_secs(5)).await?;

        let provider_id = insert_compatible_model(&database, None).await?;

        let error = handle_save(
            &database,
            &body(r#"{"provider_id":1,"upstream_name":"u1","thinking_levels_override":["turbo"]}"#),
        )
        .await
        .err()
        .ok_or("an unknown thinking level must be rejected")?;

        assert_eq!(provider_id, 1);

        assert_eq!(error.status, 400);

        assert_eq!(error.param.as_deref(), Some("thinking_levels_override"));

        assert!(
            error.message.contains("Unknown thinking level `turbo`"),
            "the error must name the unknown level: {error}"
        );

        Ok(())
    }

    /// Verifies the `save-model` tri-state contract over the wire: an
    /// explicit JSON `null` clears an override while an absent field keeps
    /// the stored one (README §7.1).
    ///
    /// # Panics
    ///
    /// Panics if opening the test database or an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the router or database.
    #[tokio::test]
    async fn save_model_null_clears_and_absence_keeps_overrides() -> Result<(), Box<dyn StdError>> {
        test_support::init_config();

        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(2, Duration::from_secs(5)).await?;

        let provider_id = insert_compatible_model(&database, None).await?;

        let router = crate::management_api::router().with_state(database.clone());

        let set = format!(
            r#"{{"provider_id":{provider_id},"upstream_name":"u1","name_override":"public",
                "enabled_override":false,"thinking_levels_override":["low"]}}"#,
        );

        let (status, rendered) = save_model(&router, &set).await?;

        assert_eq!(status, StatusCode::OK);

        let model = rendered
            .get("model")
            .ok_or("the body must carry the model")?;

        assert_eq!(model.get("provider_id"), Some(&Value::from(provider_id)));

        assert_eq!(
            model.get("name_override"),
            Some(&Value::String("public".to_string()))
        );

        assert_eq!(model.get("enabled_override"), Some(&Value::Bool(false)));

        assert_eq!(
            model.get("thinking_levels_override"),
            Some(&serde_json::json!(["low"]))
        );

        // An absent field keeps every stored override.
        let keep = format!(r#"{{"provider_id":{provider_id},"upstream_name":"u1"}}"#);

        let (status, rendered) = save_model(&router, &keep).await?;

        assert_eq!(status, StatusCode::OK);

        let model = rendered
            .get("model")
            .ok_or("the body must carry the model")?;

        assert_eq!(
            model.get("name_override"),
            Some(&Value::String("public".to_string()))
        );

        assert_eq!(model.get("enabled_override"), Some(&Value::Bool(false)));

        assert_eq!(
            model.get("thinking_levels_override"),
            Some(&serde_json::json!(["low"]))
        );

        // An explicit JSON `null` clears every override.
        let clear = format!(
            r#"{{"provider_id":{provider_id},"upstream_name":"u1","name_override":null,
                "enabled_override":null,"thinking_levels_override":null}}"#,
        );

        let (status, rendered) = save_model(&router, &clear).await?;

        assert_eq!(status, StatusCode::OK);

        let model = rendered
            .get("model")
            .ok_or("the body must carry the model")?;

        assert_eq!(model.get("name_override"), Some(&Value::Null));

        assert_eq!(model.get("enabled_override"), Some(&Value::Null));

        assert_eq!(model.get("thinking_levels_override"), Some(&Value::Null));

        // The cleared overrides reach the database as SQL NULL.
        let rows = database
            .query(
                "SELECT name_override, enabled_override, thinking_levels_override \
                 FROM models WHERE upstream_name = 'u1'",
                (),
            )
            .await?;

        let row = rows.first().ok_or("the model row must exist")?;

        let name_override: Option<String> = row.get(0)?;

        let enabled_override: Option<i64> = row.get(1)?;

        let thinking_levels_override: Option<String> = row.get(2)?;

        assert_eq!(name_override, None);

        assert_eq!(enabled_override, None);

        assert_eq!(thinking_levels_override, None);

        Ok(())
    }

    /// Verifies `discover-models` and `save-model` reject empty identifiers
    /// with 400 naming the field instead of a 404 lookup miss: identifiers
    /// are non-empty (README §7.1).
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails.
    ///
    /// # Errors
    ///
    /// Returns any error encountered while driving the database.
    #[tokio::test]
    async fn model_endpoints_address_providers_by_id() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) =
            test_support::TempDatabaseGuard::open_database(2, Duration::from_secs(5)).await?;

        let unknown_discover = handle_discover(&database, &body(r#"{"id":999}"#))
            .await
            .err()
            .ok_or("an unknown discover id must be rejected")?;

        assert_eq!(unknown_discover.status, 404);

        assert_eq!(unknown_discover.code.as_deref(), Some("not_found"));

        insert_compatible_model(&database, None).await?;

        let empty_upstream =
            handle_save(&database, &body(r#"{"provider_id":1,"upstream_name":""}"#))
                .await
                .err()
                .ok_or("an empty save-model upstream_name must be rejected")?;

        assert_eq!(empty_upstream.status, 400);

        assert_eq!(empty_upstream.param.as_deref(), Some("upstream_name"));

        let unknown_provider = handle_save(
            &database,
            &body(r#"{"provider_id":999,"upstream_name":"u1"}"#),
        )
        .await
        .err()
        .ok_or("an unknown save-model provider id must be rejected")?;

        assert_eq!(unknown_provider.status, 404);

        // The row lookup precedes every other step, so an unknown id never
        // reaches the upstream call.
        assert_eq!(unknown_provider.message, "Provider 999 does not exist");

        Ok(())
    }
}
