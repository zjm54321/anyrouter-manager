use serde::{Deserialize, Serialize};

// Only fixed metadata can cross the helper boundary. No free-form strings,
// account identifiers, URLs, request bodies, headers, or storage values.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LoginDiagnostics {
    version: Version,
    phase: Phase,
    page: Page,
    login_requested: bool,
    #[serde(deserialize_with = "Option::deserialize")]
    login_status: Option<HttpStatus>,
    login_json: bool,
    #[serde(deserialize_with = "Option::deserialize")]
    login_success: Option<bool>,
    self_requested: bool,
    #[serde(deserialize_with = "Option::deserialize")]
    self_status: Option<HttpStatus>,
    self_json: bool,
    #[serde(deserialize_with = "Option::deserialize")]
    self_success: Option<bool>,
    self_id_type: IdType,
    self_id_valid: bool,
    self_user_header_present: bool,
    user_state_ready: bool,
    pending_login: bool,
    failure_request: FailureRequest,
    exception: Exception,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    action: Option<Action>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    action_timeout_ms: Option<ActionMillis>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    action_elapsed_ms: Option<ActionMillis>,
}

impl LoginDiagnostics {
    // Conservative last-known launcher phase if no Python result arrived.
    // False flags mean no observed evidence, NOT proof Chrome never started.
    pub(crate) fn launch_timeout() -> Self {
        Self {
            version: Version,
            phase: Phase::Launch,
            page: Page::Other,
            login_requested: false,
            login_status: None,
            login_json: false,
            login_success: None,
            self_requested: false,
            self_status: None,
            self_json: false,
            self_success: None,
            self_id_type: IdType::Absent,
            self_id_valid: false,
            self_user_header_present: false,
            user_state_ready: false,
            pending_login: false,
            failure_request: FailureRequest::None,
            exception: Exception::Timeout,
            action: None,
            action_timeout_ms: None,
            action_elapsed_ms: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(try_from = "u8", into = "u8")]
struct Version;
impl TryFrom<u8> for Version {
    type Error = &'static str;
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        if value == 1 {
            Ok(Self)
        } else {
            Err("Unsupported diagnostic version.")
        }
    }
}
impl From<Version> for u8 {
    fn from(_: Version) -> Self {
        1
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(try_from = "u16", into = "u16")]
struct HttpStatus(u16);
impl TryFrom<u16> for HttpStatus {
    type Error = &'static str;
    fn try_from(value: u16) -> Result<Self, Self::Error> {
        if (100..=599).contains(&value) {
            Ok(Self(value))
        } else {
            Err("Invalid diagnostic HTTP status.")
        }
    }
}
impl From<HttpStatus> for u16 {
    fn from(value: HttpStatus) -> Self {
        value.0
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Launch,
    Navigation,
    FormWait,
    Fill,
    Submit,
    ProfileWait,
    CookieRead,
    Done,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Page {
    Login,
    Console,
    Other,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum IdType {
    Absent,
    Integer,
    String,
    Other,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum FailureRequest {
    None,
    Navigation,
    Login,
    #[serde(rename = "self")]
    Self_,
    Resource,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Exception {
    None,
    Timeout,
    NavigationTransient,
    Network,
    Unexpected,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Action {
    NoticeClose,
    NoticeWaitHidden,
    SubmitTrial,
    PageRecheck,
    SubmitClick,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(try_from = "u64", into = "u64")]
struct ActionMillis(u64);
impl TryFrom<u64> for ActionMillis {
    type Error = &'static str;
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        if value <= 120_000 {
            Ok(Self(value))
        } else {
            Err("Invalid diagnostic action duration.")
        }
    }
}
impl From<ActionMillis> for u64 {
    fn from(value: ActionMillis) -> Self {
        value.0
    }
}

#[cfg(test)]
pub(crate) fn fixture() -> serde_json::Value {
    serde_json::json!({
        "version": 1, "phase": "profile_wait", "page": "login",
        "login_requested": true, "login_status": 200, "login_json": true,
        "login_success": true, "self_requested": true, "self_status": null,
        "self_json": false, "self_success": null, "self_id_type": "absent",
        "self_id_valid": false, "self_user_header_present": false,
        "user_state_ready": false, "pending_login": false,
        "failure_request": "self", "exception": "timeout"
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn exact_schema_roundtrip_and_strict_bounds() {
        let valid = fixture();
        let typed: LoginDiagnostics = serde_json::from_value(valid.clone()).unwrap();
        assert_eq!(serde_json::to_value(typed).unwrap(), valid);
        for (field, value) in [
            ("version", json!(2)),
            ("version", json!("1")),
            ("login_status", json!(99)),
            ("self_status", json!(600)),
            ("login_status", json!(200.0)),
            ("self_status", json!("200")),
            ("phase", json!("<script>password</script>")),
            ("page", json!("https://secret.invalid/?password=sentinel")),
            ("self_id_type", json!("secret-user-id")),
            ("failure_request", json!("unknown")),
            ("exception", json!("secret")),
            ("password", json!("secret")),
            ("query", json!("secret")),
            ("key_count", json!(1)),
            ("login_success", json!("true")),
        ] {
            let mut invalid = fixture();
            invalid[field] = value;
            assert!(
                serde_json::from_value::<LoginDiagnostics>(invalid).is_err(),
                "{field}"
            );
        }
        for field in fixture().as_object().unwrap().keys() {
            let mut invalid = fixture();
            invalid.as_object_mut().unwrap().remove(field);
            assert!(
                serde_json::from_value::<LoginDiagnostics>(invalid).is_err(),
                "missing {field}"
            );
        }
        for value in [100, 599] {
            let mut boundary = fixture();
            boundary["login_status"] = json!(value);
            assert!(serde_json::from_value::<LoginDiagnostics>(boundary).is_ok());
        }
    }

    #[test]
    fn optional_action_fields_preserve_v1_and_reject_unsafe_values() {
        let old = fixture();
        let parsed: LoginDiagnostics = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), old);
        for action in [
            "notice_close",
            "notice_wait_hidden",
            "submit_trial",
            "page_recheck",
            "submit_click",
        ] {
            let mut value = fixture();
            value["phase"] = json!("submit");
            value["action"] = json!(action);
            value["action_timeout_ms"] = json!(120000);
            value["action_elapsed_ms"] = json!(0);
            let parsed: LoginDiagnostics = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(serde_json::to_value(parsed).unwrap(), value);
        }
        for (key, bad) in [
            ("action", json!("secret-selector")),
            ("action_elapsed_ms", json!(-1)),
            ("action_timeout_ms", json!(120001)),
            ("action_elapsed_ms", json!(true)),
            ("action_elapsed_ms", json!(1.5)),
            ("action_timeout_ms", json!("secret")),
            ("action_exception", json!("secret-password")),
        ] {
            let mut value = fixture();
            value[key] = bad;
            assert!(serde_json::from_value::<LoginDiagnostics>(value).is_err());
        }
    }
}
