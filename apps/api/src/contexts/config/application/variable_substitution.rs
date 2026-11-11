//! Variable substitution service for inventory entity fields.
//!
//! Supports dot notation (e.g. {{name}}, {{site.name}}, {{custom_fields.ssh_port}})
//! in connection template URLs.

use serde_json::Value;

/// Resolves a field path from entity data using dot notation.
///
/// Examples:
/// - `name` → `entity_data["name"]`
/// - `site.name` → `entity_data["site"]["name"]`
/// - `custom_fields.ssh_port` → `entity_data["custom_fields"]["ssh_port"]`
pub fn resolve_field(entity_data: &Value, field_path: &str) -> Option<String> {
    if field_path.is_empty() {
        return None;
    }

    let parts: Vec<&str> = field_path.split('.').collect();
    let mut value = entity_data;

    for part in parts {
        let obj = value.as_object()?;
        value = obj.get(part)?;
    }

    if value.is_null() {
        return None;
    }

    Some(match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => return None,
        _ => value.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn resolve_field_top_level() {
        let data = json!({"name": "host-01", "id": 42});
        assert_eq!(resolve_field(&data, "name"), Some("host-01".to_string()));
        assert_eq!(resolve_field(&data, "id"), Some("42".to_string()));
    }

    #[test]
    fn resolve_field_nested() {
        let data = json!({
            "name": "host-01",
            "site": {"name": "dc1", "slug": "dc1"},
            "custom_fields": {"ssh_port": 22}
        });
        assert_eq!(resolve_field(&data, "site.name"), Some("dc1".to_string()));
        assert_eq!(
            resolve_field(&data, "custom_fields.ssh_port"),
            Some("22".to_string())
        );
    }

    #[test]
    fn resolve_field_nested_target_namespace() {
        // The launch-time resolution context overlays the reserved `target` key.
        let data = json!({
            "primary_ip4": "10.0.0.5",
            "target": {
                "name": "host-01",
                "site": {"name": "dc1", "slug": "dc1"},
                "custom_fields": {"team": "ops"}
            }
        });
        assert_eq!(
            resolve_field(&data, "target.site.name"),
            Some("dc1".to_string())
        );
        assert_eq!(
            resolve_field(&data, "target.custom_fields.team"),
            Some("ops".to_string())
        );
    }

    #[test]
    fn resolve_field_missing_returns_none() {
        let data = json!({"name": "host-01"});
        assert_eq!(resolve_field(&data, "missing"), None);
        assert_eq!(resolve_field(&data, "site.name"), None);
    }

    #[test]
    fn resolve_field_null_relation_returns_none() {
        // When a relation object is omitted (id was null), the path resolves to
        // None — the caller leaves it empty, not a literal placeholder.
        let data = json!({"target": {"name": "host-01"}});
        assert_eq!(resolve_field(&data, "target.tenant.name"), None);
    }
}
