use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::scope::ConnConfProtocol;
use crate::errors::AppError;

/// Parse the explicit native inventory trust scope. Record inheritance is applied
/// before calling this for dispatch; the unset sentinel is accepted on writes.
pub(crate) fn native_host_key_endpoints(params: &Value) -> Result<Vec<(String, u16)>, AppError> {
    let invalid = || AppError::Validation("Invalid native_host_key_endpoints".into());
    let Some(value) = params.get("native_host_key_endpoints") else {
        return Ok(Vec::new());
    };
    if value
        .as_object()
        .is_some_and(|o| o.len() == 1 && o.get("unset") == Some(&Value::Bool(true)))
    {
        return Ok(Vec::new());
    }
    let entries = value.as_array().ok_or_else(invalid)?;
    if entries.len() > 256 {
        return Err(invalid());
    }
    entries
        .iter()
        .map(|entry| {
            let entry = entry.as_object().ok_or_else(invalid)?;
            if entry.keys().any(|key| key != "host" && key != "port") {
                return Err(invalid());
            }
            let host = entry
                .get("host")
                .and_then(Value::as_str)
                .ok_or_else(invalid)?;
            // Match the runner's SSH host-token boundary, with IP parsing for IPv6.
            if host.is_empty()
                || host.len() > 253
                || host.starts_with('-')
                || !host
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b".:-_".contains(&c))
                || (host.contains(':') && host.parse::<std::net::Ipv6Addr>().is_err())
                || (!host.contains(':')
                    && host.trim_end_matches('.').split('.').any(|label| {
                        label.is_empty()
                            || label.len() > 63
                            || label.starts_with('-')
                            || label.ends_with('-')
                    }))
            {
                return Err(invalid());
            }
            let port = match entry.get("port") {
                None => 22,
                Some(value) => value
                    .as_u64()
                    .filter(|port| (1..=65535).contains(port))
                    .ok_or_else(invalid)? as u16,
            };
            Ok((host.to_owned(), port))
        })
        .collect()
}

#[cfg(test)]
mod native_endpoint_tests {
    use super::native_host_key_endpoints;
    use serde_json::json;

    #[test]
    fn native_endpoint_schema_enforces_exact_bounded_tokens() {
        assert_eq!(native_host_key_endpoints(&json!({"native_host_key_endpoints":[{"host":"Host.example"},{"host":"2001:db8::1","port":2222}]})).unwrap(), vec![("Host.example".into(), 22), ("2001:db8::1".into(), 2222)]);
        for value in [
            json!(null),
            json!([{"host":"*.example"}]),
            json!([{"host":"{{primary_ip4}}"}]),
            json!([{"host":"host","port":0}]),
            json!([{"host":"host","extra":1}]),
            json!([{"host":"."}]),
            json!(vec![json!({"host":"host"}); 257]),
        ] {
            assert!(
                native_host_key_endpoints(&json!({"native_host_key_endpoints":value})).is_err()
            );
        }
        assert!(
            native_host_key_endpoints(&json!({"native_host_key_endpoints":{"unset":true}}))
                .unwrap()
                .is_empty()
        );
    }
}

/// A named, protocol-typed bundle of connection parameters.
///
/// Maps to the `connconf_record` table. The `params` JSONB carries the typed
/// parameter tree; `base_record_id` is the optional template-inheritance parent
/// (same protocol, cycle-checked in the service). `owner_user_id = None` means
/// org-shared, `Some` means a personal/private record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnConfRecord {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub protocol: ConnConfProtocol,
    pub base_record_id: Option<Uuid>,
    pub enabled: bool,
    pub owner_user_id: Option<Uuid>,
    pub params: Value,
    pub custom_field_data: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NewConnConfRecord {
    pub name: String,
    pub description: Option<String>,
    pub protocol: ConnConfProtocol,
    pub base_record_id: Option<Uuid>,
    pub enabled: Option<bool>,
    pub owner_user_id: Option<Uuid>,
    pub params: Option<Value>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct UpdateConnConfRecord {
    pub name: Option<String>,
    pub description: Option<String>,
    pub protocol: Option<ConnConfProtocol>,
    /// Outer `Some` = caller set the field; inner `Option` carries null-able value.
    pub base_record_id: Option<Option<Uuid>>,
    pub enabled: Option<bool>,
    pub params: Option<Value>,
}
