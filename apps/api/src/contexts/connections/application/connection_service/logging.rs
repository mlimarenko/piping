use super::*;

pub(super) fn summarize_payload_for_log(payload: &Value) -> String {
    let Value::Object(map) = payload else {
        return "non-object".to_string();
    };

    let host = map.get("host").and_then(Value::as_str).unwrap_or("missing");
    let port = map
        .get("port")
        .and_then(Value::as_i64)
        .map(|value| value.to_string())
        .unwrap_or_else(|| "default".to_string());
    let credential_keys = map
        .get("credentials")
        .and_then(Value::as_object)
        .map(|credentials| {
            let mut keys: Vec<String> = credentials.keys().cloned().collect();
            keys.sort();
            keys.join(",")
        })
        .unwrap_or_else(|| "none".to_string());

    format!(
        "host={} port={} has_credentials={} credential_keys=[{}]",
        host,
        port,
        credential_keys != "none",
        credential_keys
    )
}
