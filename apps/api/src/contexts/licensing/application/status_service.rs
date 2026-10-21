use std::sync::Arc;

use crate::contexts::licensing::application::service::LicensingService;
use crate::contexts::licensing::domain::state::{LicenseHealthSnapshot, LicenseLifecycleState};
use crate::contexts::shared::domain::reason_codes;
use crate::errors::AppError;

#[derive(Debug, Clone)]
pub struct LicenseHealthDetails {
    pub status: String,
    pub reason_code: String,
    pub snapshot: LicenseHealthSnapshot,
}

#[derive(Debug, Clone)]
pub struct LicenseEntitlementProjection {
    pub status: String,
    pub seat_limit: i32,
    pub seats_used: i32,
    pub valid_from: chrono::DateTime<chrono::Utc>,
    pub valid_until: chrono::DateTime<chrono::Utc>,
    pub entitlement_version: String,
}

#[derive(Clone)]
pub struct LicenseStatusService {
    licensing: Arc<LicensingService>,
}

impl LicenseStatusService {
    pub fn new(licensing: Arc<LicensingService>) -> Self {
        Self { licensing }
    }

    pub async fn get_health_details(&self) -> Result<Option<LicenseHealthDetails>, AppError> {
        let Some(snapshot) = self.licensing.load_latest_health_snapshot().await? else {
            return Ok(None);
        };

        let reason_code = match snapshot.state {
            LicenseLifecycleState::Degraded | LicenseLifecycleState::Suspended => snapshot
                .last_failure_kind
                .clone()
                .unwrap_or_else(|| "CONTROL_PLANE_UNAVAILABLE".to_string()),
            LicenseLifecycleState::Grace | LicenseLifecycleState::Expired => {
                reason_codes::ENTITLEMENT_EXPIRED.to_string()
            }
            _ => "OK".to_string(),
        };

        let status =
            canonical_entitlement_status(snapshot.state.as_str(), snapshot.entitlement_expires_at)
                .to_string();

        Ok(Some(LicenseHealthDetails {
            status,
            reason_code,
            snapshot,
        }))
    }

    pub async fn get_entitlement_projection(
        &self,
    ) -> Result<LicenseEntitlementProjection, AppError> {
        let status = self.licensing.get_license_status().await?;
        let metadata = self.licensing.load_entitlement_metadata().await?;

        let now = chrono::Utc::now();
        let seats_used = match i32::try_from(status.active_seats) {
            Ok(value) => value,
            Err(_) => {
                tracing::warn!(
                    active_seats = %status.active_seats,
                    "active_seats exceeds i32::MAX; clamping for DTO"
                );
                i32::MAX
            }
        };
        let Some(metadata) = metadata else {
            return Ok(LicenseEntitlementProjection {
                status: status.status,
                seat_limit: status.max_seats,
                seats_used,
                valid_from: now,
                valid_until: now,
                entitlement_version: String::new(),
            });
        };

        let valid_until = status
            .entitlement_expires_at
            .as_deref()
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.with_timezone(&chrono::Utc))
            .unwrap_or(metadata.updated_at);

        Ok(LicenseEntitlementProjection {
            status: canonical_entitlement_status(&status.status, Some(valid_until)).to_string(),
            seat_limit: status.max_seats,
            seats_used,
            valid_from: metadata.updated_at,
            valid_until,
            entitlement_version: metadata.entitlement_version,
        })
    }
}

pub fn canonical_entitlement_status(
    raw_status: &str,
    valid_until: Option<chrono::DateTime<chrono::Utc>>,
) -> &'static str {
    if raw_status.trim() == "suspended" {
        return "suspended";
    }
    let expired = valid_until
        .map(|value| value <= chrono::Utc::now())
        .unwrap_or(false);
    if expired {
        return "expired";
    }

    match raw_status.trim() {
        "active" | "degraded" => "active",
        "grace" => "grace",
        "expired" | "inactive" => "expired",
        "suspended" => "suspended",
        _ => "suspended",
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};

    use super::canonical_entitlement_status;

    #[test]
    fn canonical_status_marks_expired_by_deadline() {
        let expired = Utc::now() - Duration::minutes(1);
        let active = Utc::now() + Duration::minutes(1);

        assert_eq!(
            canonical_entitlement_status("active", Some(expired)),
            "expired"
        );
        assert_eq!(
            canonical_entitlement_status("degraded", Some(active)),
            "active"
        );
    }
}
