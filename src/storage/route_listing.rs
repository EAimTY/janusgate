//! Public model listing from a coherent route, pool, account, and physical inventory snapshot.

use super::{Error, models::EnabledModel};
use crate::{
    compaction::{
        mechanism::{Mechanism, MechanismSupport},
        support_facts::CompactionSupportFacts,
    },
    database::Database,
    provider::{Model, Provider},
};

/// Reads all authored routes and resolves their listing facts in one snapshot.
///
/// No transaction outlives this local read; no quota query or upstream I/O runs here. Quota
/// exhaustion does not remove structural candidates from listings.
///
/// # Errors
///
/// Returns database errors or malformed stored rows.
///
/// # Panics
///
/// Panics if an enforced account or pool foreign reference is missing within the snapshot.
pub(super) async fn list_public_models(
    database: &Database,
    api_key_id: i64,
) -> Result<Vec<EnabledModel>, Error> {
    let mut transaction = database.transaction().await?;
    let policy = super::api_keys::read_enabled_policy(&mut transaction, api_key_id).await?;
    let rows = transaction
        .query(
            "SELECT id, public_id, is_enabled, model_id, pool_id, upstream_id, is_automatic \
         FROM model_routes WHERE is_enabled = 1 ORDER BY public_id",
            (),
        )
        .await?;
    let mut entries = Vec::with_capacity(rows.len());
    for row in &rows {
        let snapshot = super::route_snapshot::read_target(
            &mut transaction,
            super::routing::route_from_row(row)?,
        )
        .await?;
        if policy.resolve(snapshot.route.id, snapshot.owner())
            == crate::access_control::AccessDecision::Deny
        {
            continue;
        }
        let entry = match &snapshot.target {
            super::route_snapshot::TargetSnapshot::Direct { provider, model } => {
                model.is_effectively_enabled(provider.is_enabled).then(|| {
                    capability_entry(
                        &snapshot.route.public_id,
                        &provider.name,
                        &[(provider, model)],
                    )
                })
            }
            super::route_snapshot::TargetSnapshot::Pool {
                name,
                is_enabled,
                members,
                ..
            } => {
                let candidates: Vec<_> = members
                    .iter()
                    .filter_map(|member| {
                        member
                            .model
                            .as_ref()
                            .filter(|_| member.is_structural_candidate())
                            .map(|model| (&member.provider, model))
                    })
                    .collect();
                (*is_enabled && !candidates.is_empty())
                    .then(|| capability_entry(&snapshot.route.public_id, name, &candidates))
            }
        };
        if let Some(entry) = entry {
            entries.push(entry);
        }
    }
    transaction.commit().await?;
    Ok(entries)
}

/// Composes effective per-account facts before combining them conservatively.
fn capability_entry(
    public_id: &str,
    owner: &str,
    candidates: &[(&Provider, &Model)],
) -> EnabledModel {
    let input = candidates
        .iter()
        .map(|(_, model)| model.effective_max_input_tokens())
        .collect::<Option<Vec<_>>>();
    let output = candidates
        .iter()
        .map(|(_, model)| model.effective_max_output_tokens())
        .collect::<Option<Vec<_>>>();
    let levels = candidates
        .iter()
        .map(|(_, model)| model.effective_thinking_levels())
        .collect::<Option<Vec<_>>>();
    let thinking_levels = levels.and_then(|levels| {
        let mut intersection = levels.first()?.to_vec();
        intersection.retain(|level| levels.iter().all(|set| set.contains(level)));
        Some(intersection)
    });
    let support: Vec<_> = candidates
        .iter()
        .map(|(provider, model)| {
            CompactionSupportFacts::from_pairs(
                model.effective_mechanism_support(provider.definition.payload.compaction.as_ref()),
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
        provider_name: owner.to_string(),
        thinking_levels,
        max_input_tokens: input.and_then(|values| values.into_iter().min()),
        max_output_tokens: output.and_then(|values| values.into_iter().min()),
        compaction,
    }
}
