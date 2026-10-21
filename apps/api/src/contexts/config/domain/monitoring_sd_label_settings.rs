use serde::{Deserialize, Serialize};

/// Operator-controlled toggle of which connection-target metadata label groups the
/// Prometheus HTTP Service Discovery output emits.
///
/// Backed by the single-row `monitoring_sd_label_settings` table (`id = true`).
/// In per-target discovery output, target basics
/// (piping_target_id/name/kind/address) are always emitted and are not
/// represented here. Defaults preserve today's behavior: location/tenant/role/
/// status ON; hardware/network/contacts/description OFF.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitoringSdLabelGroups {
    pub location: bool,
    pub tenant: bool,
    pub role: bool,
    pub status: bool,
    pub hardware: bool,
    pub network: bool,
    pub contacts: bool,
    pub description: bool,
    /// Raw UUID / slug identifier labels (host_id, site_id/slug, tenant_id/slug,
    /// role_id/slug). Off by default; existing jsonb rows without the key
    /// deserialize to `false` via serde default.
    #[serde(default)]
    pub identifiers: bool,
    #[serde(default = "default_enabled_statuses")]
    pub device_statuses: Vec<String>,
    #[serde(default = "default_enabled_statuses")]
    pub site_statuses: Vec<String>,
}

fn default_enabled_statuses() -> Vec<String> {
    vec!["active".to_string()]
}

impl Default for MonitoringSdLabelGroups {
    fn default() -> Self {
        Self {
            location: true,
            tenant: true,
            role: true,
            status: true,
            hardware: false,
            network: false,
            contacts: false,
            description: false,
            identifiers: false,
            device_statuses: default_enabled_statuses(),
            site_statuses: default_enabled_statuses(),
        }
    }
}
