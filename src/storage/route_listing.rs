//! Public model listing from a coherent route, pool, account, and physical inventory snapshot.

use super::{Error, models::EnabledModel};
use crate::{
    access_control::{AccessDecision, RouteOwner},
    compaction_model::{
        mechanism::{Mechanism, MechanismSupport},
        support_facts::CompactionSupportFacts,
    },
    database::Database,
    provider::{Model, definition::compaction::CompactionSection},
};
use std::sync::LazyLock;

#[cfg(test)]
mod tests;

/// The single listing statement: direct routes and pool members over enabled accounts and
/// models, SQL-ordered by public name, route, and authored member priority. A fully fixed
/// statement over the shared column consts, built once.
static LIST_PUBLIC_MODELS_SQL: LazyLock<String> = LazyLock::new(|| {
    let columns = super::models::MODEL_COLUMNS_JOINED;
    format!(
        "SELECT r.id, r.public_id, p.id, p.name, 0 AS is_pool, 0 AS priority, \
         {columns}, d.compaction \
         FROM model_routes r JOIN models m ON m.id = r.model_id \
         JOIN providers p ON p.id = m.provider_id \
         JOIN provider_definitions d ON d.name = p.provider_definition_name \
             AND d.version = p.provider_definition_version \
         WHERE r.is_enabled = 1 AND p.is_enabled = 1 AND m.is_enabled = 1 \
             AND m.discovery_state != 'vanished' \
         UNION ALL \
         SELECT r.id, r.public_id, pool.id, pool.name, 1 AS is_pool, \
         member.priority, {columns}, d.compaction \
         FROM model_routes r JOIN provider_pools pool ON pool.id = r.pool_id \
         JOIN provider_pool_members member ON member.pool_id = pool.id \
         JOIN providers p ON p.id = member.provider_id \
         JOIN models m ON m.provider_id = p.id AND m.upstream_id = r.upstream_id \
         JOIN provider_definitions d ON d.name = p.provider_definition_name \
             AND d.version = p.provider_definition_version \
         WHERE r.is_enabled = 1 AND pool.is_enabled = 1 AND p.is_enabled = 1 \
             AND m.is_enabled = 1 AND m.discovery_state != 'vanished' \
         ORDER BY 2, 1, 6"
    )
});

/// Reads all authored routes and resolves their listing facts in one snapshot.
///
/// No transaction outlives this local read; no quota query or upstream I/O runs here. Quota
/// exhaustion does not remove structural candidates from listings.
///
/// # Errors
///
/// Returns database errors or malformed stored rows.
pub(super) async fn list_public_models(
    database: &Database,
    api_key_id: i64,
) -> Result<Vec<EnabledModel>, Error> {
    let mut transaction = database.transaction().await?;
    let policy = super::api_keys::read_enabled_policy(&mut transaction, api_key_id).await?;
    // SQL owns public-name ordering and authored membership priority; this projection never
    // decodes credentials or builds an execution snapshot.
    let rows = transaction
        .query(LIST_PUBLIC_MODELS_SQL.as_str(), ())
        .await?;
    let mut entries = Vec::new();
    let mut candidates = Vec::new();
    let mut current_route = None;
    let mut public_id = String::new();
    let mut owner_name = String::new();
    for row in &rows {
        let route_id: i64 = row.get(0)?;
        let owner_id = row.get(2)?;
        let owner = if row.get::<i64>(4)? == 0 {
            RouteOwner::Provider(owner_id)
        } else {
            RouteOwner::Pool(owner_id)
        };
        if policy.resolve(route_id, owner) == AccessDecision::Deny {
            continue;
        }
        if current_route != Some(route_id) {
            if !candidates.is_empty() {
                entries.push(capability_entry(&public_id, &owner_name, &candidates));
            }
            candidates.clear();
            current_route = Some(route_id);
            public_id = row.get(1)?;
            owner_name = row.get(3)?;
        }
        candidates.push(ListingCandidate {
            model: super::models::model_from_row_offset(row, 6)?,
            compaction: row
                .get::<Option<String>>(18)?
                .map(|json| {
                    super::provider_definitions::payload_from_json(Some(json), "compaction")
                })
                .transpose()?,
        });
    }
    if !candidates.is_empty() {
        entries.push(capability_entry(&public_id, &owner_name, &candidates));
    }
    transaction.commit().await?;
    Ok(entries)
}

/// The credential-free facts one structural listing candidate contributes.
struct ListingCandidate {
    /// The existing physical model domain projection.
    model: Model,
    /// Definition-level mechanism defaults, without unrelated descriptor sections.
    compaction: Option<CompactionSection>,
}

/// Composes effective per-account facts before combining them conservatively.
fn capability_entry(public_id: &str, owner: &str, candidates: &[ListingCandidate]) -> EnabledModel {
    let input = candidates
        .iter()
        .map(|candidate| candidate.model.effective_max_input_tokens())
        .collect::<Option<Vec<_>>>();
    let output = candidates
        .iter()
        .map(|candidate| candidate.model.effective_max_output_tokens())
        .collect::<Option<Vec<_>>>();
    let levels = candidates
        .iter()
        .map(|candidate| candidate.model.effective_thinking_levels())
        .collect::<Option<Vec<_>>>();
    let thinking_levels = levels.and_then(|levels| {
        let mut intersection = levels.first()?.to_vec();
        intersection.retain(|level| levels.iter().all(|set| set.contains(level)));
        Some(intersection)
    });
    let support: Vec<_> = candidates
        .iter()
        .map(|candidate| {
            CompactionSupportFacts::from_pairs(
                candidate
                    .model
                    .effective_mechanism_support(candidate.compaction.as_ref()),
            )
        })
        .collect();
    let compaction =
        CompactionSupportFacts::from_pairs(Mechanism::ALL.into_iter().filter_map(|mechanism| {
            let facts: Vec<_> = support
                .iter()
                .map(|facts| facts.as_ref().and_then(|facts| facts.get(mechanism)))
                .collect();
            if facts.contains(&Some(MechanismSupport::Unsupported)) {
                Some((mechanism, MechanismSupport::Unsupported))
            } else if facts
                .iter()
                .all(|fact| *fact == Some(MechanismSupport::Supported))
            {
                Some((mechanism, MechanismSupport::Supported))
            } else {
                None
            }
        }));
    EnabledModel {
        public_id: public_id.to_string(),
        owner_name: owner.to_string(),
        thinking_levels,
        max_input_tokens: input.and_then(|values| values.into_iter().min()),
        max_output_tokens: output.and_then(|values| values.into_iter().min()),
        compaction,
    }
}
