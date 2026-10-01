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
}
