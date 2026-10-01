//! Explicitly authorized read-only probe, excluded from the default test suite.
use crate::{config::Config, model::Portfolio, upstream, upstream_body};
use std::{
    fs::OpenOptions,
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
};

#[test]
#[ignore = "explicit authorization required; saved credentials parsed only, no network"]
fn authorized_saved_session_payload_parse_only() {
    use std::{
        io::Write,
        process::{Command, Stdio},
    };
    assert!(std::env::var("ANYROUTER_AUTHORIZE_SAVED_PARSE_ONLY").as_deref() == Ok("yes"));
    let project = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    std::env::set_current_dir(project).unwrap();
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open("config.toml")
        .expect("private config unavailable");
    let meta = file.metadata().unwrap();
    assert!(
        meta.is_file()
            && meta.mode() & 0o7777 == 0o600
            && meta.uid() == unsafe { libc::geteuid() }
            && meta.len() <= 65536
    );
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(65537)
        .read_to_end(&mut bytes)
        .unwrap();
    let config: Config = toml::from_str(std::str::from_utf8(&bytes).expect("config encoding"))
        .unwrap_or_else(|_| panic!("config invalid"));
    let bytes = crate::log_settings::read_private(&config.state_path, 8 * 1024 * 1024)
        .ok()
        .flatten()
        .expect("private saved state unavailable");
    let portfolio: Portfolio =
        serde_json::from_slice(&bytes).unwrap_or_else(|_| panic!("saved state invalid"));
    assert!(portfolio.validate());
    for account in &portfolio.accounts {
        // Exact production cookie builder. Budget is valid integer
        // work milliseconds; dynamic elapsed time cannot affect cookie encoding.
        let input = crate::helper::session_verify_input(
            &account.credentials,
            std::time::Duration::from_millis(85000),
        )
        .unwrap_or_else(|_| panic!("local session unusable"));
        let mut child = Command::new(project.join("tools/browser-helper/.venv/bin/python"))
            .args(["-B", "-m", "tests.session_payload_probe"])
            .current_dir(project.join("tools/browser-helper"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("parse probe unavailable");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&serde_json::to_vec(&input).unwrap())
            .expect("probe input");
        let output = child.wait_with_output().expect("parse probe completion");
        assert!(output.status.success());
        let summary: serde_json::Value =
            serde_json::from_slice(&output.stdout).expect("fixed parse summary");
        // Only numeric/boolean counters are allowed out, no arbitrary strings.
        assert!(
            summary
                .as_object()
                .is_some_and(|m| m.values().all(|v| v.is_boolean() || v.is_u64()))
        );
        assert!(summary["parse_valid"] == true);
        println!("{summary}");
    }
}

#[tokio::test]
#[ignore = "explicit authorization required: at most one GET self per saved account"]
async fn authorized_saved_session_self_shape_once() {
    assert!(
        std::env::var("ANYROUTER_AUTHORIZE_SELF_SHAPE_ONCE").as_deref() == Ok("yes"),
        "authorization required"
    );
    std::env::set_current_dir(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap(),
    )
    .expect("project directory unavailable");
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open("config.toml")
        .expect("private config unavailable");
    let meta = file.metadata().unwrap();
    assert!(
        meta.is_file()
            && meta.mode() & 0o7777 == 0o600
            && meta.uid() == unsafe { libc::geteuid() }
            && meta.len() <= 65536,
        "private config required"
    );
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(65537)
        .read_to_end(&mut bytes)
        .unwrap();
    assert!(bytes.len() <= 65536);
    let config: Config = toml::from_str(std::str::from_utf8(&bytes).expect("config encoding"))
        .unwrap_or_else(|_| panic!("config invalid"));
    assert!(config.bind.to_string() == "127.0.0.1:18880");
    let bytes = crate::log_settings::read_private(&config.state_path, 8 * 1024 * 1024)
        .ok()
        .flatten()
        .expect("private saved state unavailable");
    // Not store::load: no migration, recovery, or persistence side effects.
    let portfolio: Portfolio =
        serde_json::from_slice(&bytes).unwrap_or_else(|_| panic!("saved state invalid"));
    println!(
        "{}",
        serde_json::json!({"saved_accounts":portfolio.accounts.len(),"saved_schema_valid":portfolio.validate(),"supported_version":portfolio.version==2})
    );
    assert!(portfolio.version == 2 && portfolio.validate());
    let client = upstream::Upstream::production().expect("client initialization");
    let mut attempts = 0;
    // Authorization ceiling remains TWO even if the operator subsequently adds accounts.
    for account in portfolio.accounts.iter().take(2) {
        let Ok(headers) = upstream::credential_headers(&account.credentials, "/api/user/self")
        else {
            println!(
                "{}",
                serde_json::json!({"local_session_unusable":true,"request_sent":false})
            );
            continue;
        };
        attempts += 1;
        let response = client
            .client
            .get(format!("{}/api/user/self", upstream::UPSTREAM))
            .headers(headers)
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await;
        let Ok(response) = response else {
            println!(
                "{}",
                serde_json::json!({"request_sent":true,"transport_failed":true})
            );
            continue;
        };
        let status = response.status().as_u16();
        let coding = match response
            .headers()
            .get(reqwest::header::CONTENT_ENCODING)
            .map(|h| h.as_bytes())
        {
            None | Some(b"identity") => "identity",
            Some(b"gzip") => "gzip",
            _ => "unsupported",
        };
        let Ok(bytes) = upstream_body::bounded(response, upstream_body::MANAGEMENT_LIMIT).await
        else {
            println!(
                "{}",
                serde_json::json!({"request_sent":true,"http_status":status,"coding":coding,"decode_valid":false})
            );
            continue;
        };
        let value = serde_json::from_slice::<serde_json::Value>(&bytes).ok();
        let data = value.as_ref().and_then(|v| v.get("data"));
        println!(
            "{}",
            serde_json::json!({"request_sent":true,"http_status":status,"coding":coding,"decode_valid":true,
            "known_waf":upstream_body::recognized_waf(&bytes),"json_object":value.as_ref().is_some_and(|v|v.is_object()),
            "success_true":value.as_ref().and_then(|v|v.get("success")).and_then(|v|v.as_bool())==Some(true),
            "data_object":data.is_some_and(|v|v.is_object()),
            "identity_matches":data.is_some_and(|v|upstream::identifier(&v["id"]).is_ok_and(|id|id==account.credentials.api_user)),
            "profile_schema_valid":data.is_some_and(|v|upstream::profile_data(v,&account.credentials.api_user).is_ok())})
        );
    }
    println!(
        "{}",
        serde_json::json!({"self_get_attempts":attempts,"write_requests":0,"browser_launches":0,"automatic_retries":0,"credentials_persisted":false})
    );
}
