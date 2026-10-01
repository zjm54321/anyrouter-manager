use crate::diagnostics::LoginDiagnostics;
use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorSource {
    Helper,
    Credentials,
    #[serde(rename = "self")]
    SelfAccount,
    Tokens,
    Key,
    Persistence,
}

#[derive(Clone, Debug, Serialize)]
pub struct SafeError {
    pub code: &'static str,
    pub message: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<ErrorSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<LoginDiagnostics>,
    // Internal typed failure context: never includes the response body or parameters.
    #[serde(skip)]
    pub stage: Option<crate::system_log::EventStage>,
    #[serde(skip)]
    pub reason: Option<crate::system_log::EventReason>,
    #[serde(skip)]
    pub http_status: Option<u16>,
}

impl SafeError {
    pub fn new(code: &'static str) -> Self {
        let message = match code {
            "checkin_retry_confirmation_required" => {
                "The previous check-in may have completed. Confirm before retrying."
            }
            "checkin_outcome_unknown" => {
                "Check-in was not confirmed. Review the saved result before retrying."
            }
            "invalid_credentials" => "The upstream explicitly rejected the login credentials.",
            "upstream_challenge" => "The upstream browser challenge could not be completed.",
            "upstream_session_expired" => "The upstream session is no longer valid.",
            "checkin_preflight_failed" => "签到前会话检查失败，未发送签到请求。",
            "checkin_cycle_ambiguous" => "签到跨越周期边界，结果需要确认。",
            "upstream_unavailable" => "The upstream service is unavailable.",
            "login_form_unavailable" => "未能识别上游登录表单，未确认账号凭据是否有效。",
            "upstream_session_unverified" => "未能验证上游登录会话与账号身份。",
            "upstream_login_failed" => "上游浏览器登录流程失败，未确认账号凭据是否有效。",
            "helper_input_invalid" => "浏览器辅助程序拒绝了输入参数。",
            "browser_unavailable" => "浏览器辅助运行环境或浏览器不可用。",
            "upstream_timeout" => "上游浏览器登录流程超时。",
            "key_material_unavailable" => "The existing key's full material is unavailable.",
            "key_not_selected" => "Select an existing key before routing this account.",
            "schedule_overflow" => "The account schedule does not fit within the check-in cycle.",
            "account_limit" => "The portfolio already contains the maximum number of accounts.",
            "deprecated_endpoint" => "Use the account-specific management endpoint.",
            "checkin_history_full" => "Protected check-in history cannot be safely pruned.",
            "persistence_failed" => "The account state could not be saved safely.",
            "operation_in_progress" => "An account operation is already running.",
            "account_not_configured" => "No active account is configured.",
            "candidate_expired" => "The candidate is missing or expired. Log in again.",
            "stale_revision" => "The active account revision has changed.",
            "unauthorized" => "Local administrator authentication is required.",
            "forbidden" => "The request host or origin is not allowed.",
            "bad_request" => "The request is invalid.",
            "not_found" => "The endpoint does not exist.",
            "server_stopping" => "The server is shutting down.",
            "too_many_keys" => "The upstream token list exceeds the supported limit.",
            _ => "The upstream returned an unsupported response.",
        };
        Self {
            code,
            message,
            source: None,
            diagnostics: None,
            stage: None,
            reason: None,
            http_status: None,
        }
    }

    pub fn with_source(mut self, source: ErrorSource) -> Self {
        self.source = Some(source);
        self
    }

    pub fn with_reason(mut self, reason: crate::system_log::EventReason) -> Self {
        self.reason = Some(reason);
        self
    }

    pub fn at(mut self, stage: crate::system_log::EventStage, status: Option<u16>) -> Self {
        self.stage = Some(stage);
        self.http_status = status;
        self
    }

    pub fn log_context(
        &self,
    ) -> (
        crate::system_log::EventStage,
        crate::system_log::EventReason,
    ) {
        use crate::system_log::{EventReason as R, EventStage as S};
        let stage = self.stage.unwrap_or(match self.source {
            Some(ErrorSource::SelfAccount) => S::SelfAccount,
            Some(ErrorSource::Tokens) => S::Tokens,
            Some(ErrorSource::Key) => S::Key,
            Some(ErrorSource::Persistence) => S::Storage,
            Some(ErrorSource::Credentials) => S::HttpLogin,
            Some(ErrorSource::Helper) => S::HelperResult,
            None => S::Admission,
        });
        let reason = self.reason.unwrap_or(match self.code {
            "upstream_timeout" => R::DeadlineExpired,
            "upstream_challenge" => R::KnownChallenge,
            "upstream_session_expired" => R::SessionExpired,
            "upstream_session_unverified" => R::IdentityMismatch,
            "upstream_unavailable" => R::NetworkFailure,
            "persistence_failed" => R::StorageUnavailable,
            "checkin_preflight_failed" => R::PreflightFailed,
            "checkin_outcome_unknown" | "checkin_cycle_ambiguous" => R::OutcomeUnknown,
            "invalid_credentials" | "upstream_login_failed" => R::AuthenticationRejected,
            _ if matches!(self.source, Some(ErrorSource::Helper)) => R::HelperFailed,
            _ => R::UnsupportedResponse,
        });
        (stage, reason)
    }
}

#[derive(Debug)]
pub struct ApiError(pub StatusCode, pub SafeError);

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str) -> Self {
        Self(status, SafeError::new(code))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(serde_json::json!({ "error": self.1 }))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_metadata_is_finite_and_absent_by_default() {
        let plain = serde_json::to_value(SafeError::new("unauthorized")).unwrap();
        assert_eq!(plain.as_object().unwrap().len(), 2);
        for (source, expected) in [
            (ErrorSource::Helper, "helper"),
            (ErrorSource::Credentials, "credentials"),
            (ErrorSource::SelfAccount, "self"),
            (ErrorSource::Tokens, "tokens"),
            (ErrorSource::Key, "key"),
            (ErrorSource::Persistence, "persistence"),
        ] {
            let dto =
                serde_json::to_value(SafeError::new("upstream_unavailable").with_source(source))
                    .unwrap();
            assert_eq!(dto["source"], expected);
            assert!(dto.get("diagnostics").is_none());
        }
    }
}
