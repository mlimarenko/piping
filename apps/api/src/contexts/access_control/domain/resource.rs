use regex::Regex;
use serde::{Deserialize, Serialize};

use super::resource_id::ResourceId;

/// Resource type enum representing resource types in the system
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema, strum::EnumIter,
)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ResourceType {
    /// Connection target resource
    #[serde(rename = "CONNECTION_TARGET")]
    ConnectionTarget,
    /// Connection session resource
    ConnectionSession,
    /// Shared connection filter preset resource
    ConnectionFilterPreset,
    /// User resource
    User,
    /// Group resource
    Group,
    /// Role resource
    Role,
    /// Policy resource
    Policy,
    /// Inventory resource
    Inventory,
    /// Configuration context resource
    ConfigContext,
    /// Secret variable resource
    SecretVariable,
    /// Audit event resource
    AuditEvent,
    /// Keycloak admin API resource
    Keycloak,
    /// Webhook subscription resource
    Webhook,
    /// Built-in assistant feature resource
    Assistant,
    /// Built-in assistant conversation transcript resource
    AssistantConversation,
    /// Playbook catalog resource
    Playbook,
    /// Immutable playbook version resource
    PlaybookVersion,
    /// Playbook-to-scope assignment resource
    PlaybookAssignment,
    /// Static or dynamic playbook target set resource
    PlaybookTargetSet,
    /// Bulk playbook run group resource
    PlaybookRunGroup,
    /// Per-target playbook run resource
    PlaybookRun,
    /// Scheduled playbook trigger resource
    PlaybookSchedule,
    /// Playbook run artifact resource
    PlaybookArtifact,
    /// Headless principal for scheduled playbook runs
    PlaybookServicePrincipal,
    /// Container image/settings used to execute Ansible jobs
    PlaybookExecutionEnvironment,
    /// SCM/manual project containing playbooks and roles
    PlaybookProject,
    /// Reusable launch template for a playbook/project entrypoint
    PlaybookJobTemplate,
    /// Licensing/entitlement administration resource
    License,
    /// Instance settings (admin defaults) resource
    Settings,
    /// Instance branding/appearance resource
    Branding,
    /// Proxy/relay node infrastructure resource
    ProxyNode,
    /// Observability/monitoring configuration resource
    Monitoring,
    /// Full-instance backup/restore resource
    Backup,
    /// CCTV device-management plane (device-mutating controls) resource
    Cctv,
}

impl ResourceType {
    /// Every resource-type variant, used to enumerate the capability grid.
    pub const ALL: [ResourceType; 34] = [
        ResourceType::ConnectionTarget,
        ResourceType::ConnectionSession,
        ResourceType::ConnectionFilterPreset,
        ResourceType::User,
        ResourceType::Group,
        ResourceType::Role,
        ResourceType::Policy,
        ResourceType::Inventory,
        ResourceType::ConfigContext,
        ResourceType::SecretVariable,
        ResourceType::AuditEvent,
        ResourceType::Keycloak,
        ResourceType::Webhook,
        ResourceType::Assistant,
        ResourceType::AssistantConversation,
        ResourceType::Playbook,
        ResourceType::PlaybookVersion,
        ResourceType::PlaybookAssignment,
        ResourceType::PlaybookTargetSet,
        ResourceType::PlaybookRunGroup,
        ResourceType::PlaybookRun,
        ResourceType::PlaybookSchedule,
        ResourceType::PlaybookArtifact,
        ResourceType::PlaybookServicePrincipal,
        ResourceType::PlaybookExecutionEnvironment,
        ResourceType::PlaybookProject,
        ResourceType::PlaybookJobTemplate,
        ResourceType::License,
        ResourceType::Settings,
        ResourceType::Branding,
        ResourceType::ProxyNode,
        ResourceType::Monitoring,
        ResourceType::Backup,
        ResourceType::Cctv,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ConnectionTarget => "connection_target",
            Self::ConnectionSession => "connection_session",
            Self::ConnectionFilterPreset => "connection_filter_preset",
            Self::User => "user",
            Self::Group => "group",
            Self::Role => "role",
            Self::Policy => "policy",
            Self::Inventory => "inventory",
            Self::ConfigContext => "config_context",
            Self::SecretVariable => "secret_variable",
            Self::AuditEvent => "audit_event",
            Self::Keycloak => "keycloak",
            Self::Webhook => "webhook",
            Self::Assistant => "assistant",
            Self::AssistantConversation => "assistant_conversation",
            Self::Playbook => "playbook",
            Self::PlaybookVersion => "playbook_version",
            Self::PlaybookAssignment => "playbook_assignment",
            Self::PlaybookTargetSet => "playbook_target_set",
            Self::PlaybookRunGroup => "playbook_run_group",
            Self::PlaybookRun => "playbook_run",
            Self::PlaybookSchedule => "playbook_schedule",
            Self::PlaybookArtifact => "playbook_artifact",
            Self::PlaybookServicePrincipal => "playbook_service_principal",
            Self::PlaybookExecutionEnvironment => "playbook_execution_environment",
            Self::PlaybookProject => "playbook_project",
            Self::PlaybookJobTemplate => "playbook_job_template",
            Self::License => "license",
            Self::Settings => "settings",
            Self::Branding => "branding",
            Self::ProxyNode => "proxy_node",
            Self::Monitoring => "monitoring",
            Self::Backup => "backup",
            Self::Cctv => "cctv",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        let normalized = value.trim().to_lowercase().replace('-', "_");
        match normalized.as_str() {
            "connection_target" => Some(Self::ConnectionTarget),
            "connection_session" => Some(Self::ConnectionSession),
            "connection_filter_preset" => Some(Self::ConnectionFilterPreset),
            "user" => Some(Self::User),
            "group" => Some(Self::Group),
            "role" => Some(Self::Role),
            "policy" => Some(Self::Policy),
            "inventory" => Some(Self::Inventory),
            "config_context" => Some(Self::ConfigContext),
            "secret_variable" => Some(Self::SecretVariable),
            "audit_event" => Some(Self::AuditEvent),
            "keycloak" => Some(Self::Keycloak),
            "webhook" => Some(Self::Webhook),
            "assistant" => Some(Self::Assistant),
            "assistant_conversation" => Some(Self::AssistantConversation),
            "playbook" => Some(Self::Playbook),
            "playbook_version" => Some(Self::PlaybookVersion),
            "playbook_assignment" => Some(Self::PlaybookAssignment),
            "playbook_target_set" => Some(Self::PlaybookTargetSet),
            "playbook_run_group" => Some(Self::PlaybookRunGroup),
            "playbook_run" => Some(Self::PlaybookRun),
            "playbook_schedule" => Some(Self::PlaybookSchedule),
            "playbook_artifact" => Some(Self::PlaybookArtifact),
            "playbook_service_principal" => Some(Self::PlaybookServicePrincipal),
            "playbook_execution_environment" => Some(Self::PlaybookExecutionEnvironment),
            "playbook_project" => Some(Self::PlaybookProject),
            "playbook_job_template" => Some(Self::PlaybookJobTemplate),
            "license" => Some(Self::License),
            "settings" => Some(Self::Settings),
            "branding" => Some(Self::Branding),
            "proxy_node" => Some(Self::ProxyNode),
            "monitoring" => Some(Self::Monitoring),
            "backup" => Some(Self::Backup),
            "cctv" => Some(Self::Cctv),
            _ => None,
        }
    }
}

pub fn matches_resource_id(pattern: &str, resource_id: &ResourceId) -> bool {
    if pattern == "*" {
        return true;
    }

    let target = if pattern.contains(':') {
        resource_id.canonical()
    } else {
        resource_id.resource_id.clone()
    };

    let regex_pattern = if let Some(stripped) = pattern.strip_prefix("re:") {
        // Anchor user-supplied regex so `re:foo` matches the whole id (not a
        // substring) and an empty `re:` cannot match everything.
        format!("^{stripped}$")
    } else {
        let escaped = regex::escape(pattern);
        let wildcard = escaped.replace("\\*", ".*").replace("\\?", ".");
        format!("^{}$", wildcard)
    };

    Regex::new(&regex_pattern)
        .map(|re| re.is_match(&target))
        .unwrap_or(false)
}

/// Whether `pattern` is a resource-id matcher the engine can actually evaluate.
///
/// `matches_resource_pattern` compiles a non-`*` pattern with the Rust `regex`
/// crate and returns `false` on a compile error (`unwrap_or(false)`), so a `re:`
/// pattern the crate rejects — lookaround, backreferences, POSIX collating
/// classes — matches NOTHING there. Callers that render a pattern to a DIFFERENT
/// regex dialect (the row-visibility SQL compiler → Postgres POSIX regex) must
/// use this to reject such patterns first, otherwise the database engine might
/// accept a pattern the policy engine rejects and disclose rows the engine would
/// deny. The policy write path also rejects an invalid pattern at 400 rather
/// than persisting one whose two engines disagree.
pub fn resource_pattern_is_valid(pattern: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    let source = if let Some(stripped) = pattern.strip_prefix("re:") {
        format!("^{stripped}$")
    } else {
        // Glob/exact patterns are `regex::escape`d before wildcard expansion, so
        // they always compile; this branch is effectively always valid.
        let escaped = regex::escape(pattern);
        let wildcard = escaped.replace("\\*", ".*").replace("\\?", ".");
        format!("^{}$", wildcard)
    };
    Regex::new(&source).is_ok()
}

pub fn matches_resource_pattern(pattern: &str, resource_id: &str) -> bool {
    if pattern == "*" {
        return true;
    }

    let target = if pattern.contains(':') {
        resource_id
    } else {
        resource_id
            .split_once(':')
            .map(|(_, id)| id)
            .unwrap_or(resource_id)
    };

    let regex_pattern = if let Some(stripped) = pattern.strip_prefix("re:") {
        // Anchor user-supplied regex so `re:foo` matches the whole id (not a
        // substring) and an empty `re:` cannot match everything.
        format!("^{stripped}$")
    } else {
        let escaped = regex::escape(pattern);
        let wildcard = escaped.replace("\\*", ".*").replace("\\?", ".");
        format!("^{}$", wildcard)
    };

    Regex::new(&regex_pattern)
        .map(|re| re.is_match(target))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::ResourceType;

    #[test]
    fn resource_type_rejects_the_retired_host_alias() {
        assert_eq!(
            ResourceType::parse("CONNECTION_TARGET"),
            Some(ResourceType::ConnectionTarget)
        );
        assert_eq!(ResourceType::parse("host"), None);
        assert!(serde_json::from_str::<ResourceType>("\"HOST\"").is_err());
        assert_eq!(
            serde_json::to_string(&ResourceType::ConnectionTarget).unwrap(),
            "\"CONNECTION_TARGET\""
        );
    }

    #[test]
    fn resource_type_roundtrips_first_class_playbook_resources() {
        for (wire, resource_type) in [
            ("playbook", ResourceType::Playbook),
            ("playbook_version", ResourceType::PlaybookVersion),
            ("playbook_assignment", ResourceType::PlaybookAssignment),
            ("playbook_target_set", ResourceType::PlaybookTargetSet),
            ("playbook_run_group", ResourceType::PlaybookRunGroup),
            ("playbook_run", ResourceType::PlaybookRun),
            ("playbook_schedule", ResourceType::PlaybookSchedule),
            ("playbook_artifact", ResourceType::PlaybookArtifact),
            (
                "playbook_service_principal",
                ResourceType::PlaybookServicePrincipal,
            ),
            (
                "playbook_execution_environment",
                ResourceType::PlaybookExecutionEnvironment,
            ),
            ("playbook_project", ResourceType::PlaybookProject),
            ("playbook_job_template", ResourceType::PlaybookJobTemplate),
        ] {
            assert_eq!(resource_type.as_str(), wire);
            assert_eq!(ResourceType::parse(wire), Some(resource_type.clone()));
            assert_eq!(
                ResourceType::parse(&wire.replace('_', "-")),
                Some(resource_type.clone())
            );
            assert_eq!(
                serde_json::from_str::<ResourceType>(
                    format!("\"{}\"", wire.to_ascii_uppercase()).as_str()
                )
                .unwrap(),
                resource_type
            );
        }
    }
}
