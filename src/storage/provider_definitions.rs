//! Provider-definition persistence: immutable `(name, version)` rows, appended by the management
//! surface and read by every engine (README §3, §6).
//!
//! Definitions are never updated or deleted here: an edit inserts the next version under the
//! same name, and `ON DELETE RESTRICT` protects referenced versions at the database boundary.
//! The insert of a successor version is conditional on the caller naming the current latest
//! version, so a stale base version can never fork the history.

use super::{Error, SqlExecutor, definition_from_row_offset};
use crate::{
    database::{Database, Transaction, params},
    provider::definition::{DefinitionPayload, ProviderDefinition},
};
use std::num::NonZeroI64;

/// Column order of definition selects, consumed positionally by
/// [`definition_from_row_offset`].
pub(crate) const DEFINITION_COLUMNS: &str = "name, version, format, inference, credential, \
     provisioning, discovery, compaction, created_at";

/// A new definition version about to be inserted.
pub(crate) struct NewDefinition<'a> {
    /// The definition name; version 1 when created, `base_version + 1` when updated.
    pub(crate) name: &'a str,

    /// The version the caller believes to be current; creation names no base and inserts
    /// version 1, an update must name the latest version or the write is rejected.
    pub(crate) base_version: Option<NonZeroI64>,

    /// The validated payload.
    pub(crate) payload: &'a DefinitionPayload,

    /// Creation timestamp in UTC microseconds.
    pub(crate) created_at: i64,
}

/// Renders the JSON storage form of a descriptor column: the exact text the validated payload
/// serializes into.
///
/// # Panics
///
/// Panics if serialization fails, which cannot occur: every descriptor payload is a serde
/// structure without ad-hoc maps.
fn payload_json<T>(payload: &T) -> String
where
    T: serde::Serialize,
{
    #[expect(
        clippy::expect_used,
        reason = "a descriptor payload is a serde structure, which serde_json always renders"
    )]
    serde_json::to_string(payload).expect("definition payload serializes")
}

/// Inserts one definition version, returning the stored row.
///
/// # Panics
///
/// Panics if a descriptor payload fails to serialize, which cannot occur: every payload is a
/// serde structure.
///
/// Creation (no base version) inserts version 1 under a fresh name and fails on a taken name
/// with the constraint violation the caller renders as a conflict. An update (a base version)
/// runs inside a transaction whose insert is conditional on the stored latest version being
/// exactly the caller's base, so a stale base version is rejected as a conflict and nothing is
/// written. That transaction runs under the storage tier's `in_retryable_transaction` bounded
/// stale-snapshot retry: a concurrent version insert that invalidates the snapshot between the
/// latest-version read and the write re-runs the read-then-write against the newer rows — the
/// conditional insert then rejects or appends on the fresh state — instead of surfacing the
/// refusal. The post-commit read-back is outside the retried transaction; it only observes the
/// committed row for the return value.
///
/// # Errors
///
/// Returns [`Error`] when the insert fails or a stale base version is named.
pub(crate) async fn insert_definition(
    database: &Database,
    definition: NewDefinition<'_>,
) -> Result<ProviderDefinition, Error> {
    match definition.base_version {
        None => insert_first(database, definition).await,

        Some(base_version) => {
            let (name, payload, created_at) =
                (definition.name, definition.payload, definition.created_at);

            in_retryable_transaction!(database, transaction, async {
                insert_next_in_transaction(
                    &mut transaction,
                    NewDefinition {
                        name,
                        base_version: None,
                        payload,
                        created_at,
                    },
                    base_version,
                )
                .await
            })?;

            #[expect(
                clippy::unwrap_used,
                reason = "NonZeroI64 addition by one of a positive version cannot be zero"
            )]
            let next = NonZeroI64::new(base_version.get() + 1).unwrap();

            find_definition(database, definition.name, next)
                .await?
                .ok_or_else(|| Error::Drift {
                    message: format!("definition `{}` vanished after insert", definition.name),
                })
        }
    }
}

/// Inserts version 1 of a fresh definition name.
///
/// # Errors
///
/// Returns [`Error`] when the name is taken or the statement fails.
async fn insert_first(
    database: &Database,
    definition: NewDefinition<'_>,
) -> Result<ProviderDefinition, Error> {
    database
        .query(
            "INSERT INTO provider_definitions (name, version, format, inference, credential, \
             provisioning, discovery, compaction, created_at) \
             VALUES (?1, 1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
             RETURNING name, version, format, inference, credential, provisioning, discovery, \
             compaction, created_at",
            params![
                definition.name,
                definition.payload.format.as_str(),
                payload_json(&definition.payload.inference),
                payload_json(&definition.payload.credential),
                definition.payload.provisioning.as_ref().map(payload_json),
                definition.payload.discovery.as_ref().map(payload_json),
                definition.payload.compaction.as_ref().map(payload_json),
                definition.created_at,
            ],
        )
        .await?
        .first()
        .map(|row| definition_from_row_offset(row, 0))
        .unwrap_or_else(|| {
            Err(Error::Drift {
                message: "definition insert returned no row".to_string(),
            })
        })
}

/// Inserts the successor of `base_version` inside the caller's transaction, rejecting a stale
/// base version with a conflict before anything is written.
///
/// # Errors
///
/// Returns [`Error::Conflict`] when the stored latest version is not `base_version`, and
/// [`Error`] when any statement fails.
pub(crate) async fn insert_next_in_transaction(
    transaction: &mut Transaction,
    definition: NewDefinition<'_>,
    base_version: NonZeroI64,
) -> Result<(), Error> {
    let latest = latest_version_in_transaction(transaction, definition.name)
        .await?
        .ok_or_else(|| Error::NotFound {
            message: format!("Provider definition `{}` does not exist", definition.name),
        })?;

    if latest != base_version.get() {
        return Err(Error::Conflict {
            message: format!(
                "Provider definition `{}` is at version {latest}; version {} cannot succeed it",
                definition.name,
                base_version.get()
            ),
        });
    }

    transaction
        .execute(
            "INSERT INTO provider_definitions (name, version, format, inference, credential, \
             provisioning, discovery, compaction, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                definition.name,
                base_version.get() + 1,
                definition.payload.format.as_str(),
                payload_json(&definition.payload.inference),
                payload_json(&definition.payload.credential),
                definition.payload.provisioning.as_ref().map(payload_json),
                definition.payload.discovery.as_ref().map(payload_json),
                definition.payload.compaction.as_ref().map(payload_json),
                definition.created_at,
            ],
        )
        .await?;

    Ok(())
}

/// The `(name, version)` definition read generic over the pooled or transactional executor, so
/// the statement is written once for both executors.
///
/// # Errors
///
/// Returns [`Error`] when the query fails or a row is malformed.
async fn find_definition_on<E: SqlExecutor>(
    mut executor: E,
    name: &str,
    version: NonZeroI64,
) -> Result<Option<ProviderDefinition>, Error> {
    let sql = format!(
        "SELECT {DEFINITION_COLUMNS} FROM provider_definitions WHERE name = ?1 AND version = ?2"
    );

    executor
        .query(sql, params![name, version.get()])
        .await?
        .first()
        .map(|row| definition_from_row_offset(row, 0))
        .transpose()
}

/// Fetches one definition version.
///
/// # Errors
///
/// Returns [`Error`] when the query fails or a row is malformed.
pub(crate) async fn find_definition(
    database: &Database,
    name: &str,
    version: NonZeroI64,
) -> Result<Option<ProviderDefinition>, Error> {
    find_definition_on(database, name, version).await
}

/// Fetches one definition version inside a transaction, the transactional form of
/// [`find_definition`] the transactional writes validate their references through.
///
/// # Errors
///
/// Returns [`Error`] when the query fails or a row is malformed.
pub(crate) async fn find_definition_in_transaction(
    transaction: &mut Transaction,
    name: &str,
    version: NonZeroI64,
) -> Result<Option<ProviderDefinition>, Error> {
    find_definition_on(transaction, name, version).await
}

/// Lists every definition version, ordered by name then version.
///
/// # Errors
///
/// Returns [`Error`] when the query fails or a row is malformed.
pub(crate) async fn list_definitions(
    database: &Database,
) -> Result<Vec<ProviderDefinition>, Error> {
    let sql =
        format!("SELECT {DEFINITION_COLUMNS} FROM provider_definitions ORDER BY name, version");

    database
        .query(sql, ())
        .await?
        .iter()
        .map(|row| definition_from_row_offset(row, 0))
        .collect()
}

/// The stored latest version of one definition name inside a transaction, or `None` when the
/// name is unknown.
///
/// # Errors
///
/// Returns [`Error`] when the query fails.
pub(crate) async fn latest_version_in_transaction(
    transaction: &mut Transaction,
    name: &str,
) -> Result<Option<i64>, Error> {
    let rows = transaction
        .query(
            "SELECT MAX(version) FROM provider_definitions WHERE name = ?1",
            params![name],
        )
        .await?;

    Ok(rows.first().map(|row| row.get(0)).transpose()?)
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::expect_used,
        reason = "the fixtures are constructed so each expect lands on its documented side"
    )]

    use crate::{
        compaction::mechanism::MechanismSupport,
        format::UpstreamFormat,
        provider::{
            CredentialKind, StoredCredentials,
            definition::{
                compaction::{
                    CompactionSection, CompactionStrategy, MechanismDeclarations,
                    StandaloneCompactDeclaration, StandaloneCompactOperation,
                    UnsupportedClassifier,
                },
                credential::CredentialAcquisition,
                template::StringExpr,
            },
        },
        storage as storage_cr,
        test_support::{
            self, FIXTURE_TOKEN_URL, OAuthDefinitionFixture, OAuthDeviceAuthorizationFixture,
            OAuthGrantFixture, OAuthRefreshFixture, oauth_definition_payload,
        },
    };
    use std::error::Error as StdError;

    /// Verifies that a definition-version update leaves existing providers on the old pair: the
    /// new version never substitutes for an explicit reference, and the credential-kind
    /// agreement is checkable from the rows.
    ///
    /// # Panics
    ///
    /// Panics if database operations or assertions fail.
    #[tokio::test]
    async fn definition_updates_leave_existing_references_on_the_old_pair()
    -> Result<(), Box<dyn StdError>> {
        let (database, _guard) = test_support::create_test_database().await;

        let definition = test_support::insert_static_key_definition(
            &database,
            "versioned",
            UpstreamFormat::OpenAiChatCompletion,
        )
        .await;

        let provider = test_support::insert_provider(
            &database,
            &definition,
            "bound",
            "b-",
            Some("http://localhost:8080"),
            StoredCredentials::ApiKey {
                api_key: "k".to_string(),
            },
        )
        .await;

        assert_eq!(provider.definition.version.get(), 1);

        // A changed credential type is allowed in a new version: v2 of the same name speaks
        // OAuth.
        let successor = oauth_definition_payload(OAuthDefinitionFixture {
            format: UpstreamFormat::OpenAiChatCompletion,
            inference_suffix: Some("/chat/completions"),
            client_id: "fixture",
            initial_slots: &[],
            grant: OAuthGrantFixture::Device {
                has_grant_type_field: false,
                authorization: OAuthDeviceAuthorizationFixture::default(),
            },
            exchange_url: FIXTURE_TOKEN_URL,
            response_fields: vec![],
            terminal_codes: &["expired_token"],
            refresh: OAuthRefreshFixture::None,
            provisioning: None,
        });

        #[expect(clippy::unwrap_used, reason = "the successor payload validates")]
        successor.validate().unwrap();

        #[expect(clippy::unwrap_used, reason = "a positive literal version")]
        let base_version = std::num::NonZeroI64::new(1).unwrap();

        storage_cr::provider_definitions::insert_definition(
            &database,
            storage_cr::provider_definitions::NewDefinition {
                name: "versioned",
                base_version: Some(base_version),
                payload: &successor,
                created_at: 2,
            },
        )
        .await?;

        // The provider stays on v1 and its credentials stay api_key-shaped.
        let reloaded = storage_cr::providers::find_provider_by_id(&database, provider.id)
            .await?
            .expect("the provider persists");

        assert_eq!(reloaded.definition.version.get(), 1);
        assert_eq!(reloaded.credential_kind(), CredentialKind::ApiKey);

        // The v2 row carries the OAuth discriminator.
        let mut transaction = database.transaction().await?;

        let v2 = storage_cr::provider_definitions::latest_version_in_transaction(
            &mut transaction,
            "versioned",
        )
        .await?
        .expect("a latest version exists");

        transaction.commit().await?;

        #[expect(clippy::unwrap_used, reason = "a positive stored version")]
        let v2 = std::num::NonZeroI64::new(v2).unwrap();

        let row = storage_cr::provider_definitions::find_definition(&database, "versioned", v2)
            .await?
            .expect("v2 exists");

        assert!(matches!(
            row.payload.credential,
            CredentialAcquisition::OAuth { .. }
        ));

        Ok(())
    }

    /// Pins the refresh-mode vocabulary: a fixture carrying a refresh grant parses and a
    /// `reprovision` mode without steps is rejected by the write-time validation, so the
    /// definition rows can never disagree with the engine's dispatch.
    ///
    /// # Panics
    ///
    /// Panics if a fixture validates or rejects outside its contract.
    #[test]
    fn rejects_reprovision_without_steps() {
        let mut payload = oauth_definition_payload(OAuthDefinitionFixture {
            format: UpstreamFormat::OpenAiChatCompletion,
            inference_suffix: None,
            client_id: "fixture",
            initial_slots: &[],
            grant: OAuthGrantFixture::Device {
                has_grant_type_field: false,
                authorization: OAuthDeviceAuthorizationFixture::default(),
            },
            exchange_url: FIXTURE_TOKEN_URL,
            response_fields: vec![],
            terminal_codes: &["expired_token"],
            refresh: OAuthRefreshFixture::Reprovision,
            provisioning: None,
        });

        #[expect(clippy::unwrap_used, reason = "the reprovision rule rejects")]
        let error = payload.validate().unwrap_err();

        assert!(error.message.contains("reprovision"), "{error}");

        payload.provisioning = Some(Vec::new());

        #[expect(clippy::unwrap_used, reason = "the empty-steps rule rejects")]
        let error = payload.validate().unwrap_err();

        assert!(
            error.message.contains("at least one step") || error.message.contains("reprovision"),
            "{error}"
        );
    }

    /// Builds the compaction section the round-trip fixture carries: a supported standalone
    /// operation with a response unwrap and a classifier, a declared unknown trigger, and the
    /// export policy set — the conforming shape for the fixture's `openai_response` format.
    ///
    /// # Panics
    ///
    /// Panics if a budget literal is zero, which no fixture literal is.
    fn compaction_section() -> CompactionSection {
        #[expect(
            clippy::unwrap_used,
            reason = "the fixture summary budget is a positive literal"
        )]
        let summary_budget = std::num::NonZeroU64::new(2048).unwrap();

        #[expect(
            clippy::unwrap_used,
            reason = "the fixture retention budget is a positive literal"
        )]
        let retention_budget = std::num::NonZeroU64::new(4096).unwrap();

        CompactionSection {
            strategy: CompactionStrategy::GatewaySummary,
            summary_budget,
            retention_budget,
            can_export_native_summary: true,
            mechanisms: MechanismDeclarations {
                responses_standalone_compact: Some(StandaloneCompactDeclaration {
                    default: MechanismSupport::Supported,
                    operation: Some(StandaloneCompactOperation {
                        url: StringExpr::literal("https://upstream.example/responses/compact"),
                        headers: Vec::new(),
                        envelope: None,
                        response_unwrap: Some("result".to_string()),
                        unsupported: Some(UnsupportedClassifier {
                            statuses: vec![404],
                            codes: vec!["model_compaction_unsupported".to_string()],
                            parameters: Vec::new(),
                        }),
                    }),
                    unsupported: None,
                }),
                responses_explicit_trigger: None,
                responses_automatic_compaction: None,
                messages_on_demand_compaction: None,
                messages_threshold_compaction: None,
            },
            count_tokens: None,
        }
    }

    /// Verifies the compaction section rides the definition row: an insert carrying one reads
    /// back identically through the definition select and the provider join, and a payload
    /// without one reads back as `None`.
    ///
    /// # Panics
    ///
    /// Panics if a database operation, fixture validation, or the round-trip assertion fails.
    #[tokio::test]
    async fn definition_rows_carry_the_compaction_section() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) = test_support::create_test_database().await;

        let mut payload =
            test_support::static_key_definition_payload(UpstreamFormat::OpenAiResponse);

        payload.compaction = Some(compaction_section());

        payload.validate().map_err(|error| error.message)?;

        storage_cr::provider_definitions::insert_definition(
            &database,
            storage_cr::provider_definitions::NewDefinition {
                name: "compacted",
                base_version: None,
                payload: &payload,
                created_at: 1,
            },
        )
        .await?;

        // The definition select decodes the section.
        let reloaded = storage_cr::provider_definitions::list_definitions(&database).await?;

        let stored_row = reloaded
            .first()
            .expect("the inserted definition row exists");

        assert_eq!(
            stored_row.payload.compaction.as_ref(),
            Some(&compaction_section())
        );

        // A successor without the section stores the null column and reads back without one.
        let successor = test_support::static_key_definition_payload(UpstreamFormat::OpenAiResponse);

        successor.validate().map_err(|error| error.message)?;

        #[expect(clippy::unwrap_used, reason = "a positive literal version")]
        let base_version = std::num::NonZeroI64::new(1).unwrap();

        storage_cr::provider_definitions::insert_definition(
            &database,
            storage_cr::provider_definitions::NewDefinition {
                name: "compacted",
                base_version: Some(base_version),
                payload: &successor,
                created_at: 2,
            },
        )
        .await?;

        let rows = storage_cr::provider_definitions::list_definitions(&database).await?;

        let second = rows
            .iter()
            .find(|definition| definition.version.get() == 2)
            .expect("the successor row exists");

        assert_eq!(second.payload.compaction, None);

        Ok(())
    }
}
