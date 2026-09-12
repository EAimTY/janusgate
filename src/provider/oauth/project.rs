//! Google Code Assist project onboarding: resolving the Cloud Code Assist
//! project ID of the authenticated Google account.

use super::{Error, code_assist};
use crate::provider::endpoints;
use reqwest::Client;
use std::time::Duration;

/// Code Assist metadata identifying the Antigravity IDE.
fn code_assist_metadata() -> serde_json::Value {
    serde_json::json!({
        "ideType": "ANTIGRAVITY",
        "platform": "PLATFORM_UNSPECIFIED",
        "pluginType": "GEMINI",
    })
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
    let load_response: serde_json::Value = code_assist::post_code_assist(
        http,
        endpoints::ANTIGRAVITY_LOAD_CODE_ASSIST_ACTION,
        access_token,
        &serde_json::json!({"metadata": code_assist_metadata()}),
    )
    .await?;

    if let Some(project) = load_response
        .get("cloudaicompanionProject")
        .and_then(companion_project_id)
    {
        return Ok(project);
    }

    let tier = load_response
        .pointer("/paidTier/id")
        .or_else(|| load_response.pointer("/currentTier/id"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("free-tier");

    let body = serde_json::json!({
        "tierId": tier,
        "metadata": code_assist_metadata(),
    });

    let onboard_deadline = std::time::Instant::now() + ONBOARD_TOTAL_BUDGET;

    loop {
        if std::time::Instant::now() >= onboard_deadline {
            return Err(Error::Unavailable {
                detail: "project onboarding did not complete in time".to_string(),
            });
        }

        let response: serde_json::Value = code_assist::post_code_assist(
            http,
            endpoints::ANTIGRAVITY_ONBOARD_USER_ACTION,
            access_token,
            &body,
        )
        .await?;

        if response
            .get("done")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            return onboard_operation_project(&response)?.ok_or_else(|| Error::Unavailable {
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
fn onboard_operation_project(operation: &serde_json::Value) -> Result<Option<String>, Error> {
    if let Some(error) = operation.get("error") {
        let status = error
            .get("status")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown status");

        let message = error
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("no message");

        return Err(Error::Unavailable {
            detail: format!("onboardUser failed upstream: {status}: {message}"),
        });
    }

    Ok(operation
        .pointer("/response/cloudaicompanionProject")
        .and_then(companion_project_id))
}

/// Reads a `cloudaicompanionProject` field. `loadCodeAssist` carries the bare
/// project id string, while the `onboardUser` operation result wraps it as an
/// `{id}` object; both shapes are accepted, and a missing or empty id counts
/// as no project so the caller fails the resolution instead of storing one.
fn companion_project_id(field: &serde_json::Value) -> Option<String> {
    let project = match field {
        serde_json::Value::String(project) => project,
        _ => field.get("id").and_then(serde_json::Value::as_str)?,
    };

    (!project.is_empty()).then(|| project.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The companion-project field arrives as the bare id string from
    /// `loadCodeAssist` and as an `{id}` object from the `onboardUser`
    /// operation result (README §5.2); both shapes resolve to the same
    /// project id.
    #[test]
    fn companion_project_field_accepts_both_shapes() {
        assert_eq!(
            companion_project_id(&serde_json::json!("proj-1")),
            Some("proj-1".to_string())
        );

        assert_eq!(
            companion_project_id(&serde_json::json!({"id": "proj-1"})),
            Some("proj-1".to_string())
        );
    }

    /// A completed `onboardUser` operation with no usable project id — the
    /// field missing, empty as a string or as an `{id}` object, or displaced
    /// by an upstream `error` payload — must not resolve, so the resolution
    /// fails instead of storing an empty project.
    #[test]
    fn onboard_operation_without_project_fails() {
        let operations = [
            serde_json::json!({"done": true, "response": {}}),
            serde_json::json!({"done": true, "response": {"cloudaicompanionProject": ""}}),
            serde_json::json!({"done": true, "response": {"cloudaicompanionProject": {"id": ""}}}),
        ];

        for operation in &operations {
            assert_eq!(onboard_operation_project(operation).unwrap(), None);
        }

        let failed = serde_json::json!({
            "error": {"status": "PERMISSION_DENIED", "message": "onboarding denied"},
        });

        let error = onboard_operation_project(&failed).unwrap_err();

        assert!(error.to_string().contains("PERMISSION_DENIED"));
    }

    /// A successful `onboardUser` operation resolves the project id from both
    /// the `{id}` object shape the LRO result documents and the plain-string
    /// shape observed defensively by first-party clients.
    #[test]
    fn onboard_operation_resolves_both_project_shapes() {
        let object = serde_json::json!({
            "done": true,
            "response": {"cloudaicompanionProject": {"id": "proj-1"}},
        });

        assert_eq!(
            onboard_operation_project(&object).unwrap(),
            Some("proj-1".to_string())
        );

        let plain = serde_json::json!({
            "done": true,
            "response": {"cloudaicompanionProject": "proj-1"},
        });

        assert_eq!(
            onboard_operation_project(&plain).unwrap(),
            Some("proj-1".to_string())
        );
    }
}
