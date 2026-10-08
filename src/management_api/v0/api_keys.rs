//! Management routes listing, creating, rewriting, and deleting gateway API keys.

use super::{EmptyResponse, Error, route};
use crate::{
    access_control::ApiKeyAccessControl,
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
struct ApiKeyResponse {
    /// API key row identifier.
    id: i64,

    /// Human label; not unique.
    name: String,

    /// Plaintext shared secret.
    key: String,

    /// Whether the key authorizes traffic.
    is_enabled: bool,

    /// Complete authored model access policy.
    access_control: ApiKeyAccessControl,

    /// Creation timestamp in UTC microseconds.
    created_at: i64,
}

impl ApiKeyResponse {
    /// Renders one stored key, consuming it.
    fn from_api_key(api_key: ApiKey) -> Self {
        Self {
            id: api_key.id,
            name: api_key.name,
            key: api_key.key,
            is_enabled: api_key.is_enabled,
            created_at: api_key.created_at,
            access_control: api_key.access_control,
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
    api_keys: Vec<ApiKeyResponse>,
}

/// Strictly parsed `api-keys:create` request (README §7.1): the complete state of one key, every
/// field required (rule 3). The row identity and creation timestamp are server-assigned.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct CreateApiKeyRequest {
    /// Human label; must be non-empty, need not be unique.
    name: String,

    /// Plaintext shared secret; non-empty printable ASCII without whitespace, and not held
    /// by another key.
    key: String,

    /// Serving state.
    is_enabled: bool,

    /// Complete authored model access policy.
    access_control: ApiKeyAccessControl,
}

/// Strictly parsed `api-keys:update` request (README §7.1): the complete desired state of the row,
/// every field required (rule 3) — a client renders the row, edits it, and sends the same shape
/// back.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct UpdateApiKeyRequest {
    /// Addressed API key row.
    api_key_id: i64,

    /// Human label; must be non-empty, need not be unique.
    name: String,

    /// Plaintext shared secret; non-empty printable ASCII without whitespace, and not held
    /// by another key.
    key: String,

    /// Serving state.
    is_enabled: bool,

    /// Complete authored model access policy.
    access_control: ApiKeyAccessControl,
}

/// Strictly parsed `api-keys:delete` request.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct DeleteApiKeyRequest {
    /// Addressed API key row.
    api_key_id: i64,
}

/// The `api-keys:create` success body (README §7.1): the server-assigned `id` and `created_at` are
/// the facts the caller cannot know (rule 7).
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(super) struct ApiKeyEnvelope {
    /// The created API key row.
    api_key: ApiKeyResponse,
}

/// Handles `POST /api-keys:list`.
pub(super) async fn list(
    State(context): State<Context>,
    WithRejection(Json(_request), _): WithRejection<Json<ListApiKeysRequest>, Error>,
) -> Result<Json<ListApiKeysResponse>, Error> {
    let api_keys = api_keys::list_api_keys(&context.database).await?;

    Ok(Json(ListApiKeysResponse {
        api_keys: api_keys
            .into_iter()
            .map(ApiKeyResponse::from_api_key)
            .collect(),
    }))
}

/// Handles `POST /api-keys:create`.
pub(super) async fn create(
    State(context): State<Context>,
    WithRejection(Json(request), _): WithRejection<Json<CreateApiKeyRequest>, Error>,
) -> Result<Json<ApiKeyEnvelope>, Error> {
    super::validate_identifier("name", &request.name)?;
    super::validate_composition("key", &request.key)?;

    let api_key = api_keys::insert_api_key(
        &context.database,
        &ApiKeyFields {
            name: &request.name,
            key: &request.key,
            is_enabled: request.is_enabled,
            access_control: &request.access_control,
        },
    )
    .await?;

    info!("created API key {} (`{}`)", api_key.id, request.name);

    Ok(Json(ApiKeyEnvelope {
        api_key: ApiKeyResponse::from_api_key(api_key),
    }))
}

/// Handles `POST /api-keys:update`.
pub(super) async fn update(
    State(context): State<Context>,
    WithRejection(Json(request), _): WithRejection<Json<UpdateApiKeyRequest>, Error>,
) -> Result<Json<EmptyResponse>, Error> {
    super::validate_identifier("name", &request.name)?;
    super::validate_composition("key", &request.key)?;

    api_keys::update_api_key(
        &context.database,
        request.api_key_id,
        api_keys::ApiKeyFields {
            name: &request.name,
            key: &request.key,
            is_enabled: request.is_enabled,
            access_control: &request.access_control,
        },
    )
    .await?;

    info!("updated API key {}", request.api_key_id);

    Ok(Json(EmptyResponse {}))
}

/// Handles `POST /api-keys:delete`.
/// Deletion and the affinity handle invalidation share the version's detached mutation owner across
/// caller cancellation: ids are never reused, so a committed write whose invalidation is lost to a
/// cancelled caller would leave a stale handle forever.
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
    let is_deleted = super::detach_mutation(async move {
        let is_deleted = api_keys::delete_api_key(&database, api_key_id).await?;
        if is_deleted {
            cache.remove_api_key(api_key_id);
        }
        Ok(is_deleted)
    })
    .await?;

    if !is_deleted {
        return Err(Error::ApiKeyNotFound {
            id: request.api_key_id,
        });
    }

    info!("deleted API key {}", request.api_key_id);

    Ok(Json(EmptyResponse {}))
}
