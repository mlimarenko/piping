use chrono::{DateTime, Duration, Utc};

use crate::contexts::licensing::infrastructure::control_plane_dto::CommercialRefreshRequest;
use crate::contexts::licensing::infrastructure::repositories::repository::RefreshSuccess;
use crate::contexts::licensing::infrastructure::repositories::{
    ActivationContext, LicensingRepository, RefreshFailure,
};
use crate::errors::AppError;

use super::{
    LicenseRefreshResult, LicensingService, ResolvedEntitlement, classify_refresh_failure_kind,
    refresh_failure_persistence_error, request_entitlement_remote,
    require_license_control_plane_base_url, resolve_activation_token_for_refresh,
    resolve_local_entitlement_token,
};

#[derive(Debug, PartialEq)]
struct RefreshFailureTransition {
    status: &'static str,
    consecutive_failures: i32,
    grace_started_at: Option<DateTime<Utc>>,
    grace_ends_at: Option<DateTime<Utc>>,
}

/// Persisted failure counters and grace window a refresh attempt starts from.
#[derive(Debug)]
struct RefreshFailureState {
    status: String,
    consecutive_failures: i32,
    grace_started_at: Option<DateTime<Utc>>,
    grace_ends_at: Option<DateTime<Utc>>,
}

fn refresh_failure_transition(
    current: RefreshFailureState,
    now: DateTime<Utc>,
    failure_threshold: i32,
    grace_period: Duration,
) -> RefreshFailureTransition {
    let consecutive_failures = current.consecutive_failures.saturating_add(1);
    if current.status == "suspended" {
        return RefreshFailureTransition {
            status: "suspended",
            consecutive_failures,
            grace_started_at: None,
            grace_ends_at: None,
        };
    }
    let threshold_reached = consecutive_failures >= failure_threshold;
    let (grace_started_at, grace_ends_at) = if current.status != "expired" && threshold_reached {
        let grace_started_at = current.grace_started_at.or(Some(now));
        let grace_ends_at = current
            .grace_ends_at
            .or_else(|| grace_started_at.map(|started| started + grace_period));
        (grace_started_at, grace_ends_at)
    } else {
        (current.grace_started_at, current.grace_ends_at)
    };
    let grace_status = if grace_ends_at.is_some_and(|ends_at| ends_at <= now) {
        "expired"
    } else {
        "grace"
    };

    // First match preserves the enforcement invariant: an already expired license
    // stays expired before threshold or existing-grace rules may select a status.
    let status = [
        (current.status == "expired", "expired"),
        (threshold_reached, grace_status),
        (current.status == "grace", grace_status),
    ]
    .into_iter()
    .find_map(|(applies, status)| applies.then_some(status))
    .unwrap_or("degraded");

    RefreshFailureTransition {
        status,
        consecutive_failures,
        grace_started_at,
        grace_ends_at,
    }
}

impl LicensingService {
    pub async fn refresh_license_state(&self) -> Result<LicenseRefreshResult, AppError> {
        let ActivationContext {
            revision,
            instance_id,
            activation_token_encrypted,
            entitlement_version: last_entitlement_version,
            status: current_status,
            consecutive_failures: current_consecutive_failures,
            grace_started_at: current_grace_started_at,
            grace_ends_at: current_grace_ends_at,
        } = self
            .licensing_repo
            .begin_refresh()
            .await?
            .ok_or_else(|| AppError::Validation("License is not activated".to_string()))?;

        let activation_token = resolve_activation_token_for_refresh(&activation_token_encrypted)?;

        let entitlement = match self
            .request_entitlement_refresh(&activation_token, &last_entitlement_version, &instance_id)
            .await
        {
            Ok(value) => value,
            Err(error) => {
                let now = Utc::now();
                let transition = if matches!(error, AppError::LicenseIssuerDenied(_)) {
                    RefreshFailureTransition {
                        status: "suspended",
                        consecutive_failures: current_consecutive_failures.saturating_add(1),
                        grace_started_at: None,
                        grace_ends_at: None,
                    }
                } else {
                    refresh_failure_transition(
                        RefreshFailureState {
                            status: current_status,
                            consecutive_failures: current_consecutive_failures,
                            grace_started_at: current_grace_started_at,
                            grace_ends_at: current_grace_ends_at,
                        },
                        now,
                        self.settings.license.grace_failure_threshold as i32,
                        Duration::seconds(self.settings.license.grace_period_seconds as i64),
                    )
                };

                let failure_kind = classify_refresh_failure_kind(&error);
                self.licensing_repo
                    .mark_refresh_failure(
                        &instance_id,
                        revision,
                        now,
                        RefreshFailure {
                            status: transition.status,
                            error_message: &error.to_string(),
                            failure_kind,
                            consecutive_failures: transition.consecutive_failures,
                            grace_started_at: transition.grace_started_at,
                            grace_ends_at: transition.grace_ends_at,
                        },
                    )
                    .await
                    .map_err(|persistence_error| {
                        refresh_failure_persistence_error(&error, persistence_error)
                    })?;
                return Err(error);
            }
        };

        let now = Utc::now();
        self.licensing_repo
            .mark_refresh_success(RefreshSuccess {
                revision,
                instance_id: &instance_id,
                entitlement_expires_at: entitlement.entitlement_expires_at,
                max_seats: entitlement.max_seats,
                entitlement_jwt: &entitlement.entitlement_jwt,
                entitlement_version: &entitlement.entitlement_version,
                now,
            })
            .await?;

        Ok(LicenseRefreshResult {
            status: "ok".to_string(),
            synced_at: now.to_rfc3339(),
            refresh_after_seconds: self.settings.license.refresh_interval_seconds,
        })
    }

    async fn request_entitlement_refresh(
        &self,
        activation_token: &str,
        last_entitlement_version: &str,
        instance_id: &str,
    ) -> Result<ResolvedEntitlement, AppError> {
        if let Some(entitlement) = resolve_local_entitlement_token(activation_token, instance_id)
            .map_err(|error| AppError::LicenseEntitlementInvalid(error.to_string()))?
        {
            return Ok(entitlement);
        }

        let base_url = require_license_control_plane_base_url(&self.settings.license.base_url)?;
        let payload = CommercialRefreshRequest {
            activation_token: activation_token.to_string(),
            last_entitlement_version: last_entitlement_version.to_string(),
        };
        request_entitlement_remote(
            &self.settings,
            &format!("{base_url}/api/v1/instances/refresh"),
            &payload,
            instance_id,
            false,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_failure_transition_preserves_status_rule_order() {
        let now = Utc::now();
        let grace_started_at = Some(now - Duration::minutes(30));
        let future_grace_end = Some(now + Duration::minutes(30));
        let past_grace_end = Some(now - Duration::minutes(1));
        let cases = [
            ("expired", 4, grace_started_at, future_grace_end, "expired"),
            ("active", 3, None, None, "degraded"),
            ("active", 4, None, None, "grace"),
            ("grace", 0, grace_started_at, future_grace_end, "grace"),
            ("grace", 0, grace_started_at, past_grace_end, "expired"),
        ];

        for (current_status, current_failures, grace_start, grace_end, expected_status) in cases {
            let transition = refresh_failure_transition(
                RefreshFailureState {
                    status: current_status.to_string(),
                    consecutive_failures: current_failures,
                    grace_started_at: grace_start,
                    grace_ends_at: grace_end,
                },
                now,
                5,
                Duration::hours(1),
            );

            assert_eq!(
                transition.status, expected_status,
                "unexpected transition from {current_status} after {current_failures} failures"
            );
        }
    }

    #[test]
    fn refresh_failure_transition_initializes_and_preserves_grace_window() {
        let now = Utc::now();
        let grace_period = Duration::hours(1);
        let initialized = refresh_failure_transition(
            RefreshFailureState {
                status: "active".to_string(),
                consecutive_failures: 4,
                grace_started_at: None,
                grace_ends_at: None,
            },
            now,
            5,
            grace_period,
        );

        assert_eq!(initialized.grace_started_at, Some(now));
        assert_eq!(initialized.grace_ends_at, Some(now + grace_period));

        let original_start = now - Duration::minutes(30);
        let original_end = now + Duration::minutes(30);
        let preserved = refresh_failure_transition(
            RefreshFailureState {
                status: "grace".to_string(),
                consecutive_failures: 5,
                grace_started_at: Some(original_start),
                grace_ends_at: Some(original_end),
            },
            now,
            5,
            grace_period,
        );

        assert_eq!(preserved.grace_started_at, Some(original_start));
        assert_eq!(preserved.grace_ends_at, Some(original_end));
    }
}
