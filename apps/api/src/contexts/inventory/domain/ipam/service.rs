use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Service domain model — NetBox `IPAM > Services` 1:1 mirror.
///
/// The parent link is polymorphic: `parent_object_type_id` identifies the
/// content-type (e.g. device, virtual machine) and `parent_object_id` is the UUID
/// of the parent record.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Service {
    pub id: Uuid,
    pub name: String,
    /// Ordered protocol/port endpoints, e.g. `tcp/80`, `udp/53`.
    pub port_mappings: Vec<String>,
    pub ipaddresses: Vec<Uuid>,
    /// UUID of the content-type entry identifying the parent object type.
    pub parent_object_type_id: Uuid,
    /// UUID of the parent object (device or virtual machine).
    pub parent_object_id: Uuid,
    pub description: Option<String>,
    /// piping ownership axis (soft-audit; created/owned-by user).
    pub owner_id: Option<Uuid>,
    pub custom_fields: Option<serde_json::Value>,
}

/// Validate the NetBox 4.7.1 service endpoint contract at mutation boundaries.
pub fn validate_port_mappings(mappings: &[String]) -> Result<Vec<String>, crate::errors::AppError> {
    use std::collections::HashSet;

    let invalid = || {
        crate::errors::AppError::Validation(
            "port_mappings must contain unique tcp/udp/sctp endpoints with ports 1..65535".into(),
        )
    };
    if mappings.is_empty() {
        return Err(invalid());
    }
    let mut seen = HashSet::new();
    let mut normalized = Vec::with_capacity(mappings.len());
    for mapping in mappings {
        let (protocol, port) = mapping.split_once('/').ok_or_else(invalid)?;
        let protocol = protocol.to_ascii_lowercase();
        if !matches!(protocol.as_str(), "tcp" | "udp" | "sctp") {
            return Err(invalid());
        }
        let port: u16 = port.trim().parse().map_err(|_| invalid())?;
        let canonical = format!("{protocol}/{port}");
        if port == 0 || !seen.insert(canonical.clone()) {
            return Err(invalid());
        }
        normalized.push(canonical);
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::validate_port_mappings;

    #[test]
    fn mixed_protocol_endpoints_and_boundaries() {
        assert!(
            validate_port_mappings(&["tcp/1".into(), "udp/53".into(), "sctp/65535".into()]).is_ok()
        );
        for mappings in [
            vec![],
            vec!["tcp/0"],
            vec!["udp/65536"],
            vec!["icmp/80"],
            vec!["tcp/-1"],
            vec!["tcp/80/90"],
            vec!["tcp/80", "tcp/80"],
            vec!["tcp/80", "tcp/080"],
        ] {
            assert!(
                validate_port_mappings(
                    &mappings.into_iter().map(str::to_owned).collect::<Vec<_>>()
                )
                .is_err()
            );
        }
        assert_eq!(
            validate_port_mappings(&["TCP/080".into()]).unwrap(),
            vec!["tcp/80"]
        );
    }
}
