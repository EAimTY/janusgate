//! Google Code Assist project onboarding: resolving the Cloud Code Assist
//! project ID of the authenticated Google account.

use super::{Error, code_assist};
use crate::provider::endpoints;
use reqwest::Client;
use serde::{Deserialize, Serialize, de::IgnoredAny};
use std::time::Duration;

/// The Antigravity IDE metadata every Code Assist onboarding request carries
/// (README §5.2).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CodeAssistMetadata {
    /// IDE the request identifies.
    ide_type: &'static str,

    /// Host platform the request reports.
    platform: &'static str,

    /// Plugin type the request identifies.
    plugin_type: &'static str,
}

/// The single Antigravity IDE metadata value (README §5.2): every onboarding
/// request identifies the `ANTIGRAVITY` IDE on `PLATFORM_UNSPECIFIED` with the
/// `GEMINI` plugin.
const CODE_ASSIST_METADATA: CodeAssistMetadata = CodeAssistMetadata {
    ide_type: "ANTIGRAVITY",
    platform: "PLATFORM_UNSPECIFIED",
    plugin_type: "GEMINI",
};

/// The `loadCodeAssist` request body (README §5.2).
#[derive(Debug, Serialize)]
struct LoadCodeAssistRequest {
    /// Antigravity IDE metadata.
    metadata: CodeAssistMetadata,
}

/// The `onboardUser` request body (README §5.2).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OnboardUserRequest<'a> {
    /// Tier to onboard the account onto.
    tier_id: &'a str,

    /// Antigravity IDE metadata.
    metadata: CodeAssistMetadata,
}

/// The `cloudaicompanionProject` field of a Code Assist response (README
/// §5.2): `loadCodeAssist` carries the bare project-id string, while the
/// `onboardUser` operation result wraps it as an `{id}` object. Both shapes
/// resolve to the same project id.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum CompanionProject {
    /// The bare project-id string.
    Id(String),

    /// The `{id}` object the operation result wraps the id in.
    Descriptor {
        /// Project identifier.
        id: Option<String>,
    },

    /// Any other value, which carries no project id.
    Other(IgnoredAny),
}

impl CompanionProject {
    /// The project id this field carries. A missing or empty id counts as no
    /// project, so the caller fails the resolution instead of storing one.
    fn into_project_id(self) -> Option<String> {
        let project_id = match self {
            Self::Id(project_id) => project_id,
            Self::Descriptor { id } => id?,
            Self::Other(_) => return None,
        };

        (!project_id.is_empty()).then_some(project_id)
    }
}

/// A Code Assist tier descriptor, as the `paidTier` and `currentTier` fields
/// carry it (README §5.2).
#[derive(Debug, Deserialize)]
struct Tier {
    /// Tier identifier the account holds.
    id: Option<String>,
}

/// The `loadCodeAssist` response (README §5.2). The payload is upstream-owned,
/// so unknown fields are ignored.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoadCodeAssistResponse {
    /// Cloud Code Assist project of the account, when it already has one.
    cloudaicompanion_project: Option<CompanionProject>,

    /// Paid tier of the account, when it holds one.
    paid_tier: Option<Tier>,

    /// Tier the account currently uses.
    current_tier: Option<Tier>,
}

/// The terminal failure of a completed `google.longrunning` operation (README
/// §5.2). The payload is upstream-owned, so unknown fields are ignored.
#[derive(Debug, Deserialize)]
struct OperationError {
    /// Canonical status name of the failure.
    status: Option<String>,

    /// Human-readable failure message.
    message: Option<String>,
}

/// The result of a successfully completed `onboardUser` operation (README
/// §5.2). The payload is upstream-owned, so unknown fields are ignored.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OnboardResponse {
    /// Cloud Code Assist project the account was onboarded onto.
    cloudaicompanion_project: Option<CompanionProject>,
}

/// The `onboardUser` long-running operation (README §5.2). The payload is
/// upstream-owned, so unknown fields are ignored.
#[derive(Debug, Deserialize)]
struct OnboardOperation {
    /// Whether the operation has completed; absent while it still runs.
    done: Option<bool>,

    /// Terminal failure of the operation, when it failed.
    error: Option<OperationError>,

    /// Result of a successfully completed operation.
    response: Option<OnboardResponse>,
}

/// Delay between `onboardUser` polling attempts.
const ONBOARD_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Total wall-clock budget for the `onboardUser` long-running operation,
/// checked before every attempt. Each attempt is bounded by the control-plane
/// client's total request timeout, but a fixed attempt count alone bounds
/// nothing, so the poll must also respect a deadline inside one management
/// request.
const ONBOARD_TOTAL_BUDGET: Duration = Duration::from_secs(60);

/// Resolves the Cloud Code Assist project ID for the authenticated Google
/// account: `loadCodeAssist` first, then the `onboardUser` long-running
/// operation polled within [`ONBOARD_TOTAL_BUDGET`].
///
/// # Errors
///
/// Returns [`Error`] when neither call yields a project, the operation
/// reports a terminal failure, or the poll budget elapses.
pub(super) async fn resolve_project_id(http: &Client, access_token: &str) -> Result<String, Error> {
    let load_response: LoadCodeAssistResponse = code_assist::post_code_assist(
        http,
        endpoints::ANTIGRAVITY_LOAD_CODE_ASSIST_ACTION,
        access_token,
        &LoadCodeAssistRequest {
            metadata: CODE_ASSIST_METADATA,
        },
    )
    .await?;

    if let Some(project_id) = load_response
        .cloudaicompanion_project
        .and_then(CompanionProject::into_project_id)
    {
        return Ok(project_id);
    }

    let tier_id = load_response
        .paid_tier
        .as_ref()
        .and_then(|tier| tier.id.as_deref())
        .or_else(|| {
            load_response
                .current_tier
                .as_ref()
                .and_then(|tier| tier.id.as_deref())
        })
        .unwrap_or("free-tier");

    let onboard_request = OnboardUserRequest {
        tier_id,
        metadata: CODE_ASSIST_METADATA,
    };

    let onboard_deadline = std::time::Instant::now() + ONBOARD_TOTAL_BUDGET;

    loop {
        if std::time::Instant::now() >= onboard_deadline {
            return Err(Error::Unavailable {
                detail: "project onboarding did not complete in time".to_string(),
            });
        }

        let response: OnboardOperation = code_assist::post_code_assist(
            http,
            endpoints::ANTIGRAVITY_ONBOARD_USER_ACTION,
            access_token,
            &onboard_request,
        )
        .await?;

        if response.done.unwrap_or(false) {
            return onboard_operation_project(response)?.ok_or_else(|| Error::Unavailable {
                detail: "onboardUser completed without a project ID".to_string(),
            });
        }

        tokio::time::sleep(ONBOARD_POLL_INTERVAL).await;
    }
}

/// Extracts the project ID from a completed `onboardUser` operation.
///
/// A completed `google.longrunning` operation carries its terminal failure as
/// an `error` payload (`{code, message, status}`), which is propagated into
/// the upstream error detail instead of collapsing into a generic
/// missing-project failure.
///
/// # Errors
///
/// Returns [`Error::Unavailable`] when the completed operation carries a
/// terminal failure payload.
fn onboard_operation_project(operation: OnboardOperation) -> Result<Option<String>, Error> {
    if let Some(error) = operation.error {
        let status = error.status.as_deref().unwrap_or("unknown status");

        let message = error.message.as_deref().unwrap_or("no message");

        return Err(Error::Unavailable {
            detail: format!("onboardUser failed upstream: {status}: {message}"),
        });
    }

    Ok(operation
        .response
        .and_then(|response| response.cloudaicompanion_project)
        .and_then(CompanionProject::into_project_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The companion-project field arrives as the bare id string from
    /// `loadCodeAssist` and as an `{id}` object from the `onboardUser`
    /// operation result (README §5.2); both shapes resolve to the same
    /// project id, while an unrecognized shape carries none.
    ///
    /// # Panics
    ///
    /// Panics if a payload does not parse or an assertion fails.
    #[test]
    fn companion_project_field_accepts_both_shapes() {
        for (json, expected) in [
            (r#""proj-1""#, Some("proj-1")),
            (r#"{"id": "proj-1"}"#, Some("proj-1")),
            (r#"{"id": ""}"#, None),
            (r#"{}"#, None),
            ("5", None),
            ("null", None),
        ] {
            #[expect(
                clippy::unwrap_used,
                reason = "every payload shape is constructed to parse"
            )]
            let field: CompanionProject = serde_json::from_str(json).unwrap();

            assert_eq!(field.into_project_id().as_deref(), expected, "json: {json}");
        }
    }

    /// A completed `onboardUser` operation with no usable project id — the
    /// field missing, empty as a string or as an `{id}` object, or displaced
    /// by an upstream `error` payload — must not resolve, so the resolution
    /// fails instead of storing an empty project; a failure payload that
    /// carries no status or message still names both placeholders in the
    /// upstream detail.
    ///
    /// # Panics
    ///
    /// Panics if a payload does not parse or an assertion fails.
    #[test]
    fn onboard_operation_without_project_fails() {
        for json in [
            r#"{"done": true, "response": {}}"#,
            r#"{"done": true, "response": {"cloudaicompanionProject": ""}}"#,
            r#"{"done": true, "response": {"cloudaicompanionProject": {"id": ""}}}"#,
        ] {
            #[expect(clippy::unwrap_used, reason = "every payload is constructed to parse")]
            let operation: OnboardOperation = serde_json::from_str(json).unwrap();

            #[expect(
                clippy::unwrap_used,
                reason = "the operation without a project resolves to no project"
            )]
            let project = onboard_operation_project(operation).unwrap();

            assert_eq!(project, None);
        }

        let failed = r#"{"error":{"status":"DENIED","message":"nope"}}"#;

        #[expect(clippy::unwrap_used, reason = "the payload is constructed to parse")]
        let operation: OnboardOperation = serde_json::from_str(failed).unwrap();

        #[expect(
            clippy::unwrap_used,
            reason = "the failed operation is constructed to be rejected"
        )]
        let error = onboard_operation_project(operation).unwrap_err();

        assert_eq!(
            error.to_string(),
            "Upstream request failed: onboardUser failed upstream: DENIED: nope"
        );

        let bare_failure = r#"{"error":{}}"#;

        #[expect(clippy::unwrap_used, reason = "the payload is constructed to parse")]
        let operation: OnboardOperation = serde_json::from_str(bare_failure).unwrap();

        #[expect(
            clippy::unwrap_used,
            reason = "the failed operation is constructed to be rejected"
        )]
        let error = onboard_operation_project(operation).unwrap_err();

        assert_eq!(
            error.to_string(),
            "Upstream request failed: onboardUser failed upstream: unknown status: no message"
        );
    }

    /// A successful `onboardUser` operation resolves the project id from both
    /// the `{id}` object shape the LRO result documents and the plain-string
    /// shape observed defensively by first-party clients.
    ///
    /// # Panics
    ///
    /// Panics if a payload does not parse or an assertion fails.
    #[test]
    fn onboard_operation_resolves_both_project_shapes() {
        for json in [
            r#"{"done": true, "response": {"cloudaicompanionProject": {"id": "proj-1"}}}"#,
            r#"{"done": true, "response": {"cloudaicompanionProject": "proj-1"}}"#,
        ] {
            #[expect(clippy::unwrap_used, reason = "every payload is constructed to parse")]
            let operation: OnboardOperation = serde_json::from_str(json).unwrap();

            #[expect(
                clippy::unwrap_used,
                reason = "both project shapes are constructed to resolve"
            )]
            let project = onboard_operation_project(operation).unwrap();

            assert_eq!(project, Some("proj-1".to_string()));
        }
    }
}
