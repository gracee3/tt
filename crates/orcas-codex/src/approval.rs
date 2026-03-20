use async_trait::async_trait;
use serde_json::Value;

use orcas_core::{OrcasError, OrcasResult};

#[derive(Debug, Clone)]
pub enum ApprovalDecision {
    Result(Value),
    Error {
        code: i64,
        message: String,
        data: Option<Value>,
    },
}

#[async_trait]
pub trait ApprovalRouter: Send + Sync {
    async fn resolve(&self, method: &str, params: Option<Value>) -> OrcasResult<ApprovalDecision>;
}

#[derive(Debug, Default)]
pub struct RejectingApprovalRouter;

#[async_trait]
impl ApprovalRouter for RejectingApprovalRouter {
    async fn resolve(&self, method: &str, _params: Option<Value>) -> OrcasResult<ApprovalDecision> {
        Ok(ApprovalDecision::Error {
            code: -32_000,
            message: format!("orcas does not yet handle server request `{method}`"),
            data: None,
        })
    }
}

impl From<&str> for ApprovalDecision {
    fn from(message: &str) -> Self {
        Self::Error {
            code: -32_000,
            message: message.to_string(),
            data: None,
        }
    }
}

impl From<OrcasError> for ApprovalDecision {
    fn from(error: OrcasError) -> Self {
        Self::Error {
            code: -32_000,
            message: error.to_string(),
            data: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{ApprovalDecision, ApprovalRouter, RejectingApprovalRouter};
    use orcas_core::OrcasError;

    #[tokio::test]
    async fn rejecting_router_returns_method_specific_server_error() {
        let router = RejectingApprovalRouter;

        let decision = router
            .resolve("approval/request", Some(json!({"scope":"sandbox"})))
            .await
            .expect("router should return decision");

        match decision {
            ApprovalDecision::Error {
                code,
                message,
                data,
            } => {
                assert_eq!(code, -32_000);
                assert_eq!(
                    message,
                    "orcas does not yet handle server request `approval/request`"
                );
                assert!(data.is_none());
            }
            other => panic!("unexpected approval decision: {other:?}"),
        }
    }

    #[test]
    fn string_conversion_builds_standard_error_shape() {
        let decision = ApprovalDecision::from("approval rejected");

        match decision {
            ApprovalDecision::Error {
                code,
                message,
                data,
            } => {
                assert_eq!(code, -32_000);
                assert_eq!(message, "approval rejected");
                assert!(data.is_none());
            }
            other => panic!("unexpected approval decision: {other:?}"),
        }
    }

    #[test]
    fn orcas_error_conversion_preserves_display_message() {
        let decision = ApprovalDecision::from(OrcasError::Transport("socket unavailable".into()));

        match decision {
            ApprovalDecision::Error {
                code,
                message,
                data,
            } => {
                assert_eq!(code, -32_000);
                assert_eq!(message, "transport error: socket unavailable");
                assert!(data.is_none());
            }
            other => panic!("unexpected approval decision: {other:?}"),
        }
    }
}
