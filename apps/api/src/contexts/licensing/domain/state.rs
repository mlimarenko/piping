use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LicenseLifecycleState {
    Inactive,
    Active,
    Degraded,
    Grace,
    Expired,
    Suspended,
    Unknown,
}

impl LicenseLifecycleState {
    pub fn parse(value: &str) -> Self {
        match value.trim() {
            "inactive" => Self::Inactive,
            "active" => Self::Active,
            "degraded" => Self::Degraded,
            "grace" => Self::Grace,
            "expired" => Self::Expired,
            "suspended" => Self::Suspended,
            _ => Self::Unknown,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Inactive => "inactive",
            Self::Active => "active",
            Self::Degraded => "degraded",
            Self::Grace => "grace",
            Self::Expired => "expired",
            Self::Suspended => "suspended",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone)]
pub struct LicenseHealthSnapshot {
    pub state: LicenseLifecycleState,
    pub entitlement_expires_at: Option<DateTime<Utc>>,
    pub grace_started_at: Option<DateTime<Utc>>,
    pub grace_ends_at: Option<DateTime<Utc>>,
    pub last_sync_at: Option<DateTime<Utc>>,
    pub last_successful_sync_at: Option<DateTime<Utc>>,
    pub last_sync_error: Option<String>,
    pub last_failure_kind: Option<String>,
    pub consecutive_failures: i32,
}
