//! Byte-preserving, bounded Responses compatibility. A generated cache key is
//! experimental routing metadata, not a client identity or acceptance guarantee.
use crate::gateway_settings::GatewayMode;
use axum::http::{HeaderMap, Method, header};
use serde::{
    Deserialize,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use serde_json::value::RawValue;
use std::{collections::HashSet, fmt};

pub const MAX_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ForwardingMode {
    Pass,
    Adapt,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompatError {
    Invalid,
    TooLarge,
    Integrity,
    Timeout,
}
impl CompatError {
    pub fn code(self) -> &'static str {
        match self {
            Self::Invalid => "responses_request_invalid",
            Self::TooLarge => "responses_request_too_large",
            Self::Integrity => "responses_integrity_unsupported",
            Self::Timeout => "responses_body_timeout",
        }
    }
}

pub fn selected(mode: GatewayMode, method: &Method, path: &str, headers: &HeaderMap) -> bool {
    if method != Method::POST || path != "/v1/responses" {
        return false;
    }
    match mode {
        GatewayMode::Pass => false,
        GatewayMode::Adapt => true,
        GatewayMode::Auto => !supported_client(headers),
    }
}

// Only the single, surviving leading product is classified. Never manufacture
// a UA, copy another client's session fields, or use this as an auth decision.
fn supported_client(headers: &HeaderMap) -> bool {
    let mut values = headers.get_all(header::USER_AGENT).iter();
    let Some(value) = values.next().and_then(|v| v.to_str().ok()) else {
        return false;
    };
    if values.next().is_some() {
        return false;
    }
    let Some(version) = value
        .strip_prefix("opencode/")
        .or_else(|| value.strip_prefix("codex_cli_rs/"))
    else {
        return false;
    };
    let token = version.split_ascii_whitespace().next().unwrap_or("");
    !version.starts_with(char::is_whitespace)
        && !token.is_empty()
        && token.len() <= 64
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b))
}

pub fn validate_headers(headers: &HeaderMap) -> Result<(), CompatError> {
    let mut types = headers.get_all(header::CONTENT_TYPE).iter();
    let value = types
        .next()
        .and_then(|v| v.to_str().ok())
        .ok_or(CompatError::Invalid)?;
    if types.next().is_some() {
        return Err(CompatError::Invalid);
    }
    let mut parts = value.split(';');
    if !parts
        .next()
        .unwrap_or("")
        .trim()
        .eq_ignore_ascii_case("application/json")
    {
        return Err(CompatError::Invalid);
    }
    for part in parts {
        let (key, value) = part.trim().split_once('=').ok_or(CompatError::Invalid)?;
        if !key.trim().eq_ignore_ascii_case("charset")
            || !value.trim().trim_matches('"').eq_ignore_ascii_case("utf-8")
        {
            return Err(CompatError::Invalid);
        }
    }
    if headers.get_all(header::CONTENT_ENCODING).iter().any(|v| {
        !v.to_str()
            .is_ok_and(|v| v.trim().eq_ignore_ascii_case("identity"))
    }) {
        return Err(CompatError::Invalid);
    }
    Ok(())
}

pub fn has_integrity_or_trailers(headers: &HeaderMap) -> bool {
    [
        "content-digest",
        "repr-digest",
        "digest",
        "content-md5",
        "signature",
        "signature-input",
        "trailer",
        "trailers",
    ]
    .iter()
    .any(|name| headers.contains_key(*name))
}

// RawValue validates JSON grammar without converting numbers to floats. These
// visitors inspect only decoded keys and container structure; scalar spellings
// and the complete original body remain untouched. Explicit depth bounds also
// apply when recursively parsing separate RawValues.
struct ObjectVisitor {
    depth: usize,
}
impl<'de> Visitor<'de> for ObjectVisitor {
    type Value = HashSet<String>;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("JSON object")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut keys = HashSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if !keys.insert(key) {
                return Err(de::Error::custom("duplicate key"));
            }
            let value: &RawValue = map.next_value()?;
            validate_value(value, self.depth + 1).map_err(de::Error::custom)?;
        }
        Ok(keys)
    }
}
struct ArrayVisitor {
    depth: usize,
}
impl<'de> Visitor<'de> for ArrayVisitor {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("JSON array")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        while let Some(value) = seq.next_element::<&RawValue>()? {
            validate_value(value, self.depth + 1).map_err(de::Error::custom)?;
        }
        Ok(())
    }
}
fn object(raw: &str, depth: usize) -> Result<HashSet<String>, serde_json::Error> {
    use serde::Deserializer;
    serde_json::Deserializer::from_str(raw).deserialize_map(ObjectVisitor { depth })
}
fn validate_value(raw: &RawValue, depth: usize) -> Result<(), &'static str> {
    use serde::Deserializer;
    if depth > 128 {
        return Err("depth");
    }
    match raw.get().as_bytes()[0] {
        b'{' => {
            object(raw.get(), depth).map_err(|_| "object")?;
        }
        b'[' => {
            serde_json::Deserializer::from_str(raw.get())
                .deserialize_seq(ArrayVisitor { depth })
                .map_err(|_| "array")?;
        }
        b'"' => {
            serde_json::from_str::<String>(raw.get()).map_err(|_| "string")?;
        }
        _ => {} // Already validated by RawValue, including extreme JSON numbers.
    }
    Ok(())
}

pub fn transform(input: &[u8], protected: bool) -> Result<(Vec<u8>, ForwardingMode), CompatError> {
    if input.len() > MAX_BYTES {
        return Err(CompatError::TooLarge);
    }
    let raw: &RawValue = serde_json::from_slice(input).map_err(|_| CompatError::Invalid)?;
    let keys = object(raw.get(), 0).map_err(|_| CompatError::Invalid)?;
    if keys.contains("prompt_cache_key") {
        return Ok((input.to_vec(), ForwardingMode::Pass));
    }
    if protected {
        return Err(CompatError::Integrity);
    }
    let field = format!(
        "{}\"prompt_cache_key\":{}",
        if keys.is_empty() { "" } else { "," },
        serde_json::to_string(&uuid::Uuid::new_v4().to_string()).expect("UUID JSON")
    );
    if input.len() + field.len() > MAX_BYTES {
        return Err(CompatError::TooLarge);
    }
    let end = input
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .ok_or(CompatError::Invalid)?;
    let mut bytes = Vec::with_capacity(input.len() + field.len());
    bytes.extend_from_slice(&input[..end]);
    bytes.extend_from_slice(field.as_bytes());
    bytes.extend_from_slice(&input[end..]);
    Ok((bytes, ForwardingMode::Adapt))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_raw_values_and_only_inserts_missing_root_key() {
        for body in [
            br#" {"prompt_cache_key":null,"n":1e999999} "#.as_slice(),
            br#"{"prompt_\u0063ache_key":"","nested":{"x":-1e-99999}}"#,
        ] {
            assert_eq!(
                transform(body, true).unwrap(),
                (body.to_vec(), ForwardingMode::Pass)
            );
        }
        let body = " {\"input\":[{\"role\":\"user\",\"content\":\"青柠\"},{\"role\":\"assistant\",\"content\":\"OK\"}],\"tools\":[{\"x\":1e999999}],\"n\":-0.000,\"max_output_tokens\":32,\"stream\":false,\"include\":[]} \n".as_bytes();
        let (out, mode) = transform(body, false).unwrap();
        assert_eq!(mode, ForwardingMode::Adapt);
        let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
        let key = value["prompt_cache_key"].as_str().unwrap();
        let id = uuid::Uuid::parse_str(key).unwrap();
        assert_eq!(id.get_version_num(), 4);
        assert_eq!(id.to_string(), key);
        let inserted = format!(",\"prompt_cache_key\":\"{key}\"");
        assert_eq!(
            String::from_utf8(out.clone())
                .unwrap()
                .replace(&inserted, "")
                .as_bytes(),
            body
        );
        assert_ne!(transform(body, false).unwrap().0, out);
        assert!(transform(b"{}", false).is_ok());
    }

    #[test]
    fn rejects_ambiguous_encoding_duplicates_integrity_and_bounds() {
        assert_eq!(
            transform(b"{\"x\":\"\xff\"}", false),
            Err(CompatError::Invalid)
        );
        for body in [
            r#"{"a":1,"\u0061":2}"#,
            r#"{"a":[{"x":1,"x":2}]}"#,
            r#"{"prompt_cache_key":null,"a":{"x":1,"x":2}}"#,
            "[]",
            "null",
            "{\"a\":NaN}",
            "{\"a\":Infinity}",
            r#"{"a":"\ud800"}"#,
            "{}{}",
        ] {
            assert_eq!(transform(body.as_bytes(), false), Err(CompatError::Invalid));
        }
        assert_eq!(transform(b"{}", true), Err(CompatError::Integrity));
        let deep = format!("{{\"a\":{}0{}}}", "[".repeat(256), "]".repeat(256));
        assert_eq!(transform(deep.as_bytes(), false), Err(CompatError::Invalid));
        for name in [
            "content-digest",
            "repr-digest",
            "digest",
            "content-md5",
            "signature",
            "signature-input",
            "trailer",
            "trailers",
        ] {
            let mut protected = HeaderMap::new();
            protected.insert(
                axum::http::HeaderName::from_static(name),
                "synthetic".parse().unwrap(),
            );
            assert!(has_integrity_or_trailers(&protected));
            assert_eq!(
                transform(b"{}", has_integrity_or_trailers(&protected)),
                Err(CompatError::Integrity)
            );
        }
        assert_eq!(
            transform(&vec![b' '; MAX_BYTES + 1], false),
            Err(CompatError::TooLarge)
        );
        let mut near = b"{}".to_vec();
        near.resize(MAX_BYTES, b' ');
        assert_eq!(transform(&near, false), Err(CompatError::TooLarge));
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            "application/json; charset=utf-8".parse().unwrap(),
        );
        assert!(validate_headers(&headers).is_ok());
        for (name, value) in [
            ("content-encoding", "gzip"),
            ("content-type", "text/json"),
            ("content-type", "application/json; charset=latin1"),
        ] {
            let mut invalid = headers.clone();
            invalid.insert(
                axum::http::HeaderName::from_static(name),
                value.parse().unwrap(),
            );
            assert_eq!(validate_headers(&invalid), Err(CompatError::Invalid));
        }
    }

    #[test]
    fn auto_classifies_only_single_surviving_leading_product() {
        for (ua, pass) in [
            ("opencode/1.18.33", true),
            ("codex_cli_rs/0.1 (linux)", true),
            ("OpenCode/1", false),
            ("other opencode/1", false),
            ("opencode/", false),
            ("opencode/ 1", false),
            ("codex_cli_rs/1,other", false),
        ] {
            let mut h = HeaderMap::new();
            h.insert(header::USER_AGENT, ua.parse().unwrap());
            assert_eq!(
                selected(GatewayMode::Auto, &Method::POST, "/v1/responses", &h),
                !pass
            );
            assert!(!selected(
                GatewayMode::Pass,
                &Method::POST,
                "/v1/responses",
                &h
            ));
            h.append(header::USER_AGENT, "opencode/1".parse().unwrap());
            assert!(selected(
                GatewayMode::Auto,
                &Method::POST,
                "/v1/responses",
                &h
            ));
        }
        let mut h = HeaderMap::new();
        h.insert(header::USER_AGENT, "opencode/1".parse().unwrap());
        h.insert(header::CONNECTION, "user-agent".parse().unwrap());
        crate::gateway::sanitize(&mut h, true);
        assert!(selected(
            GatewayMode::Auto,
            &Method::POST,
            "/v1/responses",
            &h
        ));
        for (method, path) in [
            (Method::GET, "/v1/responses"),
            (Method::POST, "/v1/responses/"),
            (Method::POST, "/v1/chat/completions"),
        ] {
            assert!(!selected(GatewayMode::Adapt, &method, path, &h));
        }
    }
}
