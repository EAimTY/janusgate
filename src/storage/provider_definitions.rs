//! Provider-definition persistence: immutable `(name, version)` rows, appended by the management
//! surface and read by every engine (README §3, §6).
//!
//! Definitions are never updated or deleted here: an edit inserts the next version under the
//! same name, and `ON DELETE RESTRICT` protects referenced versions at the database boundary.
//! The insert of a successor version is conditional on the caller naming the current latest
//! version, so a stale base version can never fork the history.

use super::{Error, SqlExecutor};
use crate::{
    database::{Database, Row, Transaction, params},
    provider::definition::{DefinitionPayload, ProviderDefinition},
};
use std::{num::NonZeroI64, sync::LazyLock};

/// Column order of definition selects, consumed positionally by
/// [`definition_from_row_offset`].
const DEFINITION_COLUMNS: &str = "name, version, format, inference, credential, \
     provisioning, discovery, compaction, quota, created_at";

/// The insert column list of a definition version and the `RETURNING` projection the insert
/// reads the stored row back through: one spelling for both the creation and the successor
/// insert, identical to [`DEFINITION_COLUMNS`].
const INSERT_COLUMNS: &str = DEFINITION_COLUMNS;

/// The version-1 insert of a fresh definition name: a fully fixed statement over
/// [`INSERT_COLUMNS`], built once.
static INSERT_FIRST_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "INSERT INTO provider_definitions ({INSERT_COLUMNS}) \
         VALUES (?1, 1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) \
         RETURNING {INSERT_COLUMNS}"
    )
});

/// The successor-version insert: a fully fixed statement over [`INSERT_COLUMNS`], built once.
static INSERT_NEXT_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "INSERT INTO provider_definitions ({INSERT_COLUMNS}) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) \
         RETURNING {INSERT_COLUMNS}"
    )
});

/// The `(name, version)` definition read: a fully fixed statement, built once.
static FIND_DEFINITION_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "SELECT {DEFINITION_COLUMNS} FROM provider_definitions WHERE name = ?1 AND version = ?2"
    )
});

/// The every-version listing, ordered by name then version: a fully fixed statement, built
/// once.
static LIST_DEFINITIONS_SQL: LazyLock<String> = LazyLock::new(|| {
    format!("SELECT {DEFINITION_COLUMNS} FROM provider_definitions ORDER BY name, version")
});

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
/// refusal.
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
            INSERT_FIRST_SQL.as_str(),
            params![
                definition.name,
                definition.payload.format.as_str(),
                payload_json(&definition.payload.inference),
                payload_json(&definition.payload.credential),
                definition.payload.provisioning.as_ref().map(payload_json),
                definition.payload.discovery.as_ref().map(payload_json),
                definition.payload.compaction.as_ref().map(payload_json),
                definition.payload.quota.as_ref().map(payload_json),
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
/// base version with a conflict before anything is written, and returning the stored row.
///
/// # Errors
///
/// Returns [`Error::Conflict`] when the stored latest version is not `base_version`, and
/// [`Error`] when any statement fails.
pub(crate) async fn insert_next_in_transaction(
    transaction: &mut Transaction,
    definition: NewDefinition<'_>,
    base_version: NonZeroI64,
) -> Result<ProviderDefinition, Error> {
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
        .query(
            INSERT_NEXT_SQL.as_str(),
            params![
                definition.name,
                base_version.get() + 1,
                definition.payload.format.as_str(),
                payload_json(&definition.payload.inference),
                payload_json(&definition.payload.credential),
                definition.payload.provisioning.as_ref().map(payload_json),
                definition.payload.discovery.as_ref().map(payload_json),
                definition.payload.compaction.as_ref().map(payload_json),
                definition.payload.quota.as_ref().map(payload_json),
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
    executor
        .query(FIND_DEFINITION_SQL.as_str(), params![name, version.get()])
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
    database
        .query(LIST_DEFINITIONS_SQL.as_str(), ())
        .await?
        .iter()
        .map(|row| definition_from_row_offset(row, 0))
        .collect()
}

/// The stored latest version of one definition name inside a transaction, or `None` when the
/// name is unknown. `MAX(version)` over an absent name aggregates to SQL `NULL`, which the
/// optional read turns into `None` — the unknown name carries no version, and a successor
/// insert maps that to the unknown-reference `NotFound` (README §7.1).
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

    Ok(rows
        .first()
        .map(|row| row.get::<Option<i64>>(0))
        .transpose()?
        .flatten())
}

/// Parses a definition payload JSON column, wrapped exactly once as internal storage drift: the
/// column was written only through the validated management path, so a payload failing here is
/// never the requesting client's fault.
pub(super) fn payload_from_json<T>(json: Option<String>, what: &str) -> Result<T, Error>
where
    T: serde::de::DeserializeOwned,
{
    let json = json.ok_or_else(|| Error::Drift {
        message: format!("{what} column must not be null"),
    })?;

    serde_json::from_str(&json).map_err(|error| Error::Drift {
        message: format!("{what} column carries a malformed payload: {error}"),
    })
}

/// Parses a provider definition row in the definition column order of
/// [`DEFINITION_COLUMNS`] at the given column offset.
pub(super) fn definition_from_row_offset(
    row: &Row,
    offset: usize,
) -> Result<ProviderDefinition, Error> {
    let name: String = row.get(offset)?;
    let version: i64 = row.get(offset + 1)?;

    let version = NonZeroI64::new(version).ok_or_else(|| Error::Drift {
        message: format!("definition `{name}` carries a zero version"),
    })?;

    let format: String = row.get(offset + 2)?;

    let format = crate::format::UpstreamFormat::parse(&format).ok_or_else(|| Error::Drift {
        message: format!("definition `{name}` carries an unknown format `{format}`"),
    })?;

    Ok(ProviderDefinition {
        name,
        version,
        payload: crate::provider::DefinitionPayload {
            format,
            inference: payload_from_json(row.get(offset + 3)?, "inference")?,
            credential: payload_from_json(row.get(offset + 4)?, "credential")?,
            provisioning: row
                .get::<Option<String>>(offset + 5)?
                .map(|json| payload_from_json(Some(json), "provisioning"))
                .transpose()?,
            discovery: row
                .get::<Option<String>>(offset + 6)?
                .map(|json| payload_from_json(Some(json), "discovery"))
                .transpose()?,
            quota: row
                .get::<Option<String>>(offset + 8)?
                .map(|json| payload_from_json(Some(json), "quota"))
                .transpose()?,
            compaction: row
                .get::<Option<String>>(offset + 7)?
                .map(|json| payload_from_json(Some(json), "compaction"))
                .transpose()?,
        },
        created_at: row.get(offset + 9)?,
    })
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::expect_used,
        reason = "the fixtures are constructed so each expect lands on its documented side"
    )]

    use crate::{
        compaction_model::mechanism::MechanismSupport,
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

    /// An unknown definition name carries no latest version: the aggregate read resolves the
    /// `NULL` row to `None`, and a successor insert over the unknown name is the `NotFound`
    /// error the management boundary maps to the version's unknown-reference 404.
    ///
    /// # Panics
    ///
    /// Panics if database operations fail or the unknown-name contract diverges.
    #[tokio::test]
    async fn latest_version_is_none_for_an_unknown_name() -> Result<(), Box<dyn StdError>> {
        let (database, _guard) = test_support::create_test_database().await;

        let mut transaction = database.transaction().await?;

        let latest = storage_cr::provider_definitions::latest_version_in_transaction(
            &mut transaction,
            "absent",
        )
        .await?;

        transaction.commit().await?;

        assert_eq!(latest, None);

        let payload = test_support::static_key_definition_payload(UpstreamFormat::OpenAiResponse);

        #[expect(
            clippy::unwrap_used,
            reason = "the rejection is the documented side of this call"
        )]
        let error = storage_cr::provider_definitions::insert_definition(
            &database,
            storage_cr::provider_definitions::NewDefinition {
                name: "absent",
                base_version: std::num::NonZeroI64::new(1),
                payload: &payload,
                created_at: 1,
            },
        )
        .await
        .unwrap_err();

        assert!(
            matches!(error, storage_cr::Error::NotFound { .. }),
            "a successor insert over an unknown name is the NotFound error: {error}"
        );

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

    /// Optional sections survive unrelated edits and joins while old bindings stay immutable.
    ///
    /// # Panics
    ///
    /// Panics if a database operation, fixture validation, or the round-trip assertion fails.
    #[tokio::test]
    async fn definition_rows_preserve_optional_sections_and_exact_bindings()
    -> Result<(), Box<dyn StdError>> {
        let (database, _guard) = test_support::create_test_database().await;

        let mut payload =
            test_support::static_key_definition_payload(UpstreamFormat::OpenAiResponse);

        payload.compaction = Some(compaction_section());
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../fixtures/quota/kimi-code.json"))?;
        #[expect(
            clippy::indexing_slicing,
            reason = "the checked-in fixture has a descriptor"
        )]
        let descriptor = fixture["descriptor"].clone();
        payload.quota = Some(serde_json::from_value(descriptor)?);

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
        assert_eq!(stored_row.payload.quota, payload.quota);
        let bound = test_support::insert_provider(
            &database,
            stored_row,
            "bound",
            Some("https://upstream.example"),
            StoredCredentials::ApiKey {
                api_key: "secret".into(),
            },
        )
        .await;

        // Editing compaction alone preserves the independently authored quota recipe.
        let mut successor = payload.clone();
        successor.compaction = None;

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
        assert_eq!(second.payload.quota, payload.quota);
        let still_bound = storage_cr::providers::find_provider_by_id(&database, bound.id)
            .await?
            .expect("the bound provider remains");
        assert_eq!(still_bound.definition.version.get(), 1);
        assert_eq!(still_bound.definition.payload, payload);
        let removed = test_support::static_key_definition_payload(UpstreamFormat::OpenAiResponse);
        storage_cr::provider_definitions::insert_definition(
            &database,
            storage_cr::provider_definitions::NewDefinition {
                name: "compacted",
                base_version: Some(second.version),
                payload: &removed,
                created_at: 3,
            },
        )
        .await?;
        let latest = storage_cr::provider_definitions::find_definition(
            &database,
            "compacted",
            std::num::NonZeroI64::new(3).expect("positive version"),
        )
        .await?
        .expect("third version exists");
        assert_eq!(latest.payload.quota, None);

        Ok(())
    }
}
