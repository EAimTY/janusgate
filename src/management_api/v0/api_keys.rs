//! Management routes listing, creating, rewriting, and deleting gateway API keys.

use super::{EmptyResponse, Error, route};
use crate::{
    context::Context,
    storage::api_keys::{self, ApiKey, ApiKeyFields},
};
use axum::{Json, extract::State};
use axum_extra::extract::WithRejection;
use log::info;
use serde::{Deserialize, Serialize};

route! {
    pub(super) ListApiKeys: {
        path: "/api-keys:list",
        method: http::Method::POST,
        request: ListApiKeysRequest,
        response: ListApiKeysResponse,
    }
}

route! {
    pub(super) CreateApiKey: {
        path: "/api-keys:create",
        method: http::Method::POST,
        request: CreateApiKeyRequest,
        response: ApiKeyEnvelope,
    }
}

route! {
    pub(super) UpdateApiKey: {
        path: "/api-keys:update",
        method: http::Method::POST,
        request: UpdateApiKeyRequest,
        response: EmptyResponse,
    }
}

route! {
    pub(super) DeleteApiKey: {
        path: "/api-keys:delete",
        method: http::Method::POST,
        request: DeleteApiKeyRequest,
        response: EmptyResponse,
    }
}

/// One gateway API key in its management wire shape (README §7.1): the row's complete readable
/// state, plaintext secret included — the secret is database-managed and the administrator needs it
/// to configure clients.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct ApiKeyResponse {
    /// API key row identifier.
    pub id: i64,

    /// Human label; not unique.
    pub name: String,

    /// Plaintext shared secret.
    pub key: String,

    /// Whether the key authorizes traffic.
    pub is_enabled: bool,

    /// Creation timestamp in UTC microseconds.
    pub created_at: i64,
}

impl ApiKeyResponse {
    /// Renders one stored key.
    fn from_api_key(api_key: &ApiKey) -> Self {
        Self {
            id: api_key.id,
            name: api_key.name.clone(),
            key: api_key.key.clone(),
            is_enabled: api_key.is_enabled,
            created_at: api_key.created_at,
        }
    }
}

/// The `api-keys:list` request body (README §7.1): the empty request of a no-argument route.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct ListApiKeysRequest {}

/// The `api-keys:list` success body (README §7.1).
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct ListApiKeysResponse {
    /// Every stored API key, ordered by row id.
    pub api_keys: Vec<ApiKeyResponse>,
}

/// Strictly parsed `api-keys:create` request (README §7.1): the complete state of one key, every
/// field required (rule 3). The row identity and creation timestamp are server-assigned.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct CreateApiKeyRequest {
    /// Human label; must be non-empty, need not be unique.
    pub name: String,

    /// Plaintext shared secret; non-empty printable ASCII without whitespace, and not held
    /// by another key.
    pub key: String,

    /// Serving state.
    pub is_enabled: bool,
}

/// Strictly parsed `api-keys:update` request (README §7.1): the complete desired state of the row,
/// every field required (rule 3) — a client renders the row, edits it, and sends the same shape
/// back.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct UpdateApiKeyRequest {
    /// Addressed API key row.
    pub api_key_id: i64,

    /// Human label; must be non-empty, need not be unique.
    pub name: String,

    /// Plaintext shared secret; non-empty printable ASCII without whitespace, and not held
    /// by another key.
    pub key: String,

    /// Serving state.
    pub is_enabled: bool,
}

/// Strictly parsed `api-keys:delete` request.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct DeleteApiKeyRequest {
    /// Addressed API key row.
    pub api_key_id: i64,
}

/// The `api-keys:create` success body (README §7.1): the server-assigned `id` and `created_at` are
/// the facts the caller cannot know (rule 7).
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct ApiKeyEnvelope {
    /// The created API key row.
    pub api_key: ApiKeyResponse,
}

/// Handles `POST /api-keys:list`.
pub(super) async fn list(
    State(context): State<Context>,
    WithRejection(Json(_request), _): WithRejection<Json<ListApiKeysRequest>, Error>,
) -> Result<Json<ListApiKeysResponse>, Error> {
    let api_keys = api_keys::list_api_keys(&context.database).await?;

    Ok(Json(ListApiKeysResponse {
        api_keys: api_keys.iter().map(ApiKeyResponse::from_api_key).collect(),
    }))
}

/// Handles `POST /api-keys:create`.
pub(super) async fn create(
    State(context): State<Context>,
    WithRejection(Json(request), _): WithRejection<Json<CreateApiKeyRequest>, Error>,
) -> Result<Json<ApiKeyEnvelope>, Error> {
    super::validate_identifier("name", &request.name)?;
    super::validate_composition("key", &request.key)?;

    // The pre-insert check renders a clean conflict without naming the secret; the schema's UNIQUE
    // constraint is the concurrent backstop.
    if api_keys::find_api_key_by_key(&context.database, &request.key)
        .await?
        .is_some()
    {
        return Err(Error::ApiKeyTaken);
    }

    let api_key = api_keys::insert_api_key(
        &context.database,
        &ApiKeyFields {
            name: &request.name,
            key: &request.key,
            is_enabled: request.is_enabled,
        },
    )
    .await?;

    info!("created API key {} (`{}`)", api_key.id, request.name);

    Ok(Json(ApiKeyEnvelope {
        api_key: ApiKeyResponse::from_api_key(&api_key),
    }))
}

/// Handles `POST /api-keys:update`.
pub(super) async fn update(
    State(context): State<Context>,
    WithRejection(Json(request), _): WithRejection<Json<UpdateApiKeyRequest>, Error>,
) -> Result<Json<EmptyResponse>, Error> {
    super::validate_identifier("name", &request.name)?;
    super::validate_composition("key", &request.key)?;

    // The outer read pins request validation only; the transactional write re-reads the row for
    // the missing-row and taken-secret checks.
    let Some(_) = api_keys::find_api_key_by_id(&context.database, request.api_key_id).await? else {
        return Err(Error::ApiKeyNotFound {
            id: request.api_key_id,
        });
    };

    api_keys::update_api_key(
        &context.database,
        request.api_key_id,
        api_keys::ApiKeyFields {
            name: &request.name,
            key: &request.key,
            is_enabled: request.is_enabled,
        },
    )
    .await?;

    info!("updated API key {}", request.api_key_id);

    Ok(Json(EmptyResponse {}))
}

/// Handles `POST /api-keys:delete`.
/// The detached mutation invalidates affinity handles after committing even if its caller leaves.
///
/// # Panics
///
/// Panics if the mutation task fails to hand off its result, a fail-fast task invariant.
pub(super) async fn delete(
    State(context): State<Context>,
    WithRejection(Json(request), _): WithRejection<Json<DeleteApiKeyRequest>, Error>,
) -> Result<Json<EmptyResponse>, Error> {
    let database = context.database.clone();
    let cache = context.affinity_cache.clone();
    let api_key_id = request.api_key_id;
    let deletion = tokio::spawn(async move {
        let is_deleted = api_keys::delete_api_key(&database, api_key_id).await?;
        if is_deleted {
            cache.remove_api_key(api_key_id);
        }
        Ok::<_, crate::storage::Error>(is_deleted)
    });
    #[expect(
        clippy::expect_used,
        reason = "the mutation task always hands its result to the handler"
    )]
    let is_deleted = deletion.await.expect("API-key deletion task completes")?;
    if !is_deleted {
        return Err(Error::ApiKeyNotFound {
            id: request.api_key_id,
        });
    }

    info!("deleted API key {}", request.api_key_id);

    Ok(Json(EmptyResponse {}))
}

#[cfg(test)]
mod tests {
    use super::{ApiKeyEnvelope, ApiKeyResponse, CreateApiKeyRequest, UpdateApiKeyRequest};

    /// Verifies the `api-keys` wire shapes (README §7.0 rules 2-3): every field of the create and
    /// update requests is required — an omitted field or an unknown one is rejected — and the
    /// rendered key carries the complete readable row, plaintext secret included.
    ///
    /// # Panics
    ///
    /// Panics if a payload parses or rejects outside its contract.
    #[test]
    fn parses_the_api_key_payloads_strictly() {
        #[expect(
            clippy::unwrap_used,
            reason = "the documented payloads are constructed to parse"
        )]
        let created: CreateApiKeyRequest =
            serde_json::from_str(r#"{"name":"service-a","key":"sk-live","is_enabled":true}"#)
                .unwrap();

        assert_eq!(created.name, "service-a");

        // A required field's absence is named by serde's own missing-field rejection.
        assert!(
            serde_json::from_str::<CreateApiKeyRequest>(
                r#"{"name":"service-a","is_enabled":true}"#
            )
            .is_err()
        );

        // An unknown field is rejected.
        assert!(
            serde_json::from_str::<CreateApiKeyRequest>(
                r#"{"name":"service-a","key":"sk-live","is_enabled":true,"created_at":1}"#
            )
            .is_err()
        );

        #[expect(
            clippy::unwrap_used,
            reason = "the documented payloads are constructed to parse"
        )]
        let updated: UpdateApiKeyRequest = serde_json::from_str(
            r#"{"api_key_id":7,"name":"service-a","key":"sk-live","is_enabled":false}"#,
        )
        .unwrap();

        assert_eq!(updated.api_key_id, 7);

        assert!(
            serde_json::from_str::<UpdateApiKeyRequest>(
                r#"{"api_key_id":7,"name":"service-a","is_enabled":false}"#
            )
            .is_err()
        );

        #[expect(clippy::expect_used, reason = "the key body always serializes")]
        let rendered = serde_json::to_value(&ApiKeyEnvelope {
            api_key: ApiKeyResponse {
                id: 7,
                name: "service-a".to_string(),
                key: "sk-live".to_string(),
                is_enabled: true,
                created_at: 1_700_000_000_000_000,
            },
        })
        .expect("the key body serializes");

        assert_eq!(
            rendered,
            serde_json::json!({
                "api_key": {
                    "id": 7,
                    "name": "service-a",
                    "key": "sk-live",
                    "is_enabled": true,
                    "created_at": 1_700_000_000_000_000_i64,
                }
            })
        );
    }
}
