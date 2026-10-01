//! Offline, exact-ID filtering. Headers/content decoding belong to the gateway.
pub const MAX_MODEL_BYTES: usize = 2 * 1024 * 1024;
pub const DENIED_IDS: [&str; 13] = [
    "gpt-5-codex",
    "claude-3-5-haiku-20241022",
    "claude-3-5-sonnet-20241022",
    "claude-3-7-sonnet-20250219",
    "claude-haiku-4-5-20251001",
    "claude-opus-4-1-20250805",
    "claude-opus-4-20250514",
    "claude-opus-4-5-20251101",
    "claude-opus-4-6",
    "claude-opus-4-7",
    "claude-sonnet-4-20250514",
    "claude-sonnet-4-5-20250929",
    "gemini-2.5-pro",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterError {
    TooLarge,
    InvalidResponse,
}
impl FilterError {
    pub fn code(self) -> &'static str {
        match self {
            Self::TooLarge => "models_response_too_large",
            Self::InvalidResponse => "models_response_invalid",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct FilteredModels {
    pub bytes: Vec<u8>,
    pub changed: bool,
}

// Split already-validated JSON at top-level punctuation, without rewriting any
// retained values (including number spellings, field order or duplicate models).
fn split(bytes: &[u8], separator: u8) -> Vec<(usize, usize)> {
    let (mut depth, mut quoted, mut escaped, mut start) = (0usize, false, false, 0);
    let mut parts = Vec::new();
    for (index, &byte) in bytes.iter().enumerate() {
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
        } else {
            match byte {
                b'"' => quoted = true,
                b'[' | b'{' => depth += 1,
                b']' | b'}' => depth -= 1,
                _ if byte == separator && depth == 0 => {
                    parts.push((start, index));
                    start = index + 1;
                }
                _ => {}
            }
        }
    }
    parts.push((start, bytes.len()));
    parts
}

pub fn filter_models(input: &[u8]) -> Result<FilteredModels, FilterError> {
    use serde_json::Value;
    if input.len() > MAX_MODEL_BYTES {
        return Err(FilterError::TooLarge);
    }
    let value: Value = serde_json::from_slice(input).map_err(|_| FilterError::InvalidResponse)?;
    let data = value
        .as_object()
        .and_then(|o| o.get("data"))
        .and_then(Value::as_array)
        .ok_or(FilterError::InvalidResponse)?;
    let keep: Vec<bool> = data
        .iter()
        .map(|item| {
            let id = item
                .as_object()
                .and_then(|o| o.get("id"))
                .and_then(Value::as_str)
                .ok_or(FilterError::InvalidResponse)?;
            Ok(!DENIED_IDS.contains(&id))
        })
        .collect::<Result<_, FilterError>>()?;
    let first = input.iter().position(|b| !b.is_ascii_whitespace()).unwrap();
    let last = input
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .unwrap();
    let root = &input[first + 1..last];
    let mut bounds = None;
    for (start, end) in split(root, b',') {
        let member = &root[start..end];
        let colon = split(member, b':')[0].1;
        let key: String =
            serde_json::from_slice(&member[..colon]).map_err(|_| FilterError::InvalidResponse)?;
        if key == "data" {
            // Duplicate data members are ambiguous and must not be filtered partially.
            if bounds.is_some() {
                return Err(FilterError::InvalidResponse);
            }
            let offset = first + 1 + start + colon + 1;
            let body = &input[offset..first + 1 + end];
            let left = body.iter().position(|b| !b.is_ascii_whitespace()).unwrap();
            let right = body.iter().rposition(|b| !b.is_ascii_whitespace()).unwrap();
            bounds = Some((offset + left + 1, offset + right));
        }
    }
    let (start, end) = bounds.ok_or(FilterError::InvalidResponse)?;
    if keep.iter().all(|keep| *keep) {
        return Ok(FilteredModels {
            bytes: input.to_vec(),
            changed: false,
        });
    }
    let elements = split(&input[start..end], b',');
    if elements.len() != keep.len() {
        return Err(FilterError::InvalidResponse);
    }
    let mut bytes = input[..start].to_vec();
    let mut written = false;
    for ((left, right), keep) in elements.into_iter().zip(keep) {
        if keep {
            if written {
                bytes.push(b',');
            }
            bytes.extend_from_slice(&input[start + left..start + right]);
            written = true;
        }
    }
    bytes.extend_from_slice(&input[end..]);
    Ok(FilteredModels {
        bytes,
        changed: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_thirteen_only() {
        let mut ids: Vec<String> = DENIED_IDS.iter().map(|id| (*id).into()).collect();
        let retained: Vec<String> = DENIED_IDS
            .iter()
            .flat_map(|id| {
                [
                    id.to_uppercase(),
                    format!("{id}-extra"),
                    format!("prefix-{id}"),
                ]
            })
            .collect();
        ids.extend(retained.clone());
        ids.push("other".into());
        ids.push("other".into());
        let body = serde_json::json!({"object":"list","data":ids.iter().map(|id| serde_json::json!({"id":id,"owned_by":"fixture"})).collect::<Vec<_>>()});
        let result = filter_models(&serde_json::to_vec(&body).unwrap()).unwrap();
        assert!(result.changed);
        let filtered: serde_json::Value = serde_json::from_slice(&result.bytes).unwrap();
        assert_eq!(filtered["data"].as_array().unwrap().len(), 41);
        for (item, id) in filtered["data"].as_array().unwrap().iter().zip(
            retained
                .iter()
                .chain(["other".into(), "other".into()].iter()),
        ) {
            assert_eq!(item["id"], *id);
            assert_eq!(item["owned_by"], "fixture");
        }
    }
    #[test]
    fn retains_raw_field_order_metadata_and_numbers() {
        let input = br#" {"z":1.00,"data":[{"id":"gpt-5-codex"},{"z":"[,\\\"]","id":"safe","meta":{"b":2,"a":1}}],"a":true} "#;
        let expected =
            br#" {"z":1.00,"data":[{"z":"[,\\\"]","id":"safe","meta":{"b":2,"a":1}}],"a":true} "#;
        assert_eq!(filter_models(input).unwrap().bytes, expected);
        for body in [br#"{"data":[]}"#.as_slice(), br#"{"data":[{"id":"safe"}]}"#] {
            assert_eq!(
                filter_models(body).unwrap(),
                FilteredModels {
                    bytes: body.into(),
                    changed: false
                }
            );
        }
        assert_eq!(
            filter_models(br#"{"data":[{"id":"gpt-5-codex"}]}"#)
                .unwrap()
                .bytes,
            br#"{"data":[]}"#
        );
    }
    #[test]
    fn malformed_and_size_fail_closed() {
        for body in [
            "[]",
            "{}",
            "{\"data\":{}}",
            "{\"data\":[null]}",
            "{\"data\":[{}]}",
            "{\"data\":[{\"id\":1}]}",
            "{\"data\":[",
            "{\"data\":[],\"data\":[]}",
        ] {
            assert_eq!(
                filter_models(body.as_bytes()),
                Err(FilterError::InvalidResponse)
            );
        }
        assert_eq!(
            filter_models(&vec![b' '; MAX_MODEL_BYTES + 1]),
            Err(FilterError::TooLarge)
        );
    }
}
