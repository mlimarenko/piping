use std::{str::FromStr, sync::Arc};

use chrono::{DateTime, TimeZone, Utc};
use futures_util::future::BoxFuture;
use saffron::Cron;
use uuid::Uuid;

use super::{RequestCommitAuthority, SchedulePrincipal, inventory_pull::ScheduledPullAdmission};
use crate::contexts::shared::request_context::RequestContext;
use crate::contexts::sync::domain::{MappingConfigurationStatus, MappingRef, ProviderType};
use crate::contexts::sync::infrastructure::persistence::{
    MappingConfigurationRepository, ProviderRepository, PullSchedule,
    SqlxMappingConfigurationRepository, SqlxProviderRepository, SqlxPullScheduleRepository,
};
use crate::errors::AppError;

pub struct PullScheduleService {
    pub schedules: SqlxPullScheduleRepository,
    providers: SqlxProviderRepository,
    mappings: SqlxMappingConfigurationRepository,
    authority: Arc<dyn RequestCommitAuthority>,
}

pub struct PullOccurrenceLease {
    pub repository: SqlxPullScheduleRepository,
    pub schedule_id: Uuid,
    pub scheduled_for: DateTime<Utc>,
    pub lease_id: Uuid,
    pub ttl_seconds: i32,
}

impl ScheduledPullAdmission for PullOccurrenceLease {
    fn record_job(&self, job_id: Uuid) -> BoxFuture<'_, Result<(), AppError>> {
        Box::pin(async move {
            if self
                .repository
                .record_admitted_job(self.schedule_id, self.scheduled_for, self.lease_id, job_id)
                .await?
            {
                Ok(())
            } else {
                Err(AppError::Conflict(
                    "Schedule occurrence lease was lost or cancelled".into(),
                ))
            }
        })
    }

    fn ensure_owned(&self) -> BoxFuture<'_, Result<(), AppError>> {
        Box::pin(async move {
            if self
                .repository
                .renew(
                    self.schedule_id,
                    self.scheduled_for,
                    self.lease_id,
                    self.ttl_seconds,
                )
                .await?
            {
                Ok(())
            } else {
                Err(AppError::Conflict(
                    "Schedule occurrence lease was lost or cancelled".into(),
                ))
            }
        })
    }
}

impl PullScheduleService {
    pub fn new(
        schedules: SqlxPullScheduleRepository,
        providers: SqlxProviderRepository,
        mappings: SqlxMappingConfigurationRepository,
        authority: Arc<dyn RequestCommitAuthority>,
    ) -> Self {
        Self {
            schedules,
            providers,
            mappings,
            authority,
        }
    }

    /// Recheck the source and immutable mapping before a scheduled pull.
    pub async fn validate_source(
        &self,
        provider_id: Uuid,
        mapping: MappingRef,
    ) -> Result<(), AppError> {
        let provider = self
            .providers
            .get_by_id(provider_id)
            .await?
            .ok_or_else(|| AppError::NotFound("Provider not found".into()))?;
        if !provider.is_enabled
            || provider.provider_type != ProviderType::Netbox
            || provider
                .config
                .get("sync_mode")
                .and_then(serde_json::Value::as_str)
                != Some("pull_only")
        {
            return Err(AppError::Validation(
                "An enabled pull_only NetBox source is required".into(),
            ));
        }
        let document = self
            .mappings
            .get_document(mapping.logical_id, mapping.version_id)
            .await?
            .ok_or_else(|| AppError::NotFound("Mapping configuration not found".into()))?;
        if document.source_provider_id != provider_id
            || document.status != MappingConfigurationStatus::Active
        {
            return Err(AppError::Validation(
                "An active mapping owned by the source is required".into(),
            ));
        }
        document.target_sync_request().ensure_supported()
    }

    pub async fn create(
        &self,
        provider_id: Uuid,
        mapping: MappingRef,
        cron: &str,
        timezone: &str,
        request: &RequestContext,
    ) -> Result<PullSchedule, AppError> {
        let principal = SchedulePrincipal::from_request(request)?;
        self.authority.ensure_commit_allowed(request).await?;
        self.validate_source(provider_id, mapping).await?;
        let cron = normalize_cron(cron)?;
        let timezone = normalize_timezone(timezone)?;
        let now = self.schedules.database_now().await?;
        let next = next_occurrence(&cron, &timezone, now)?
            .ok_or_else(|| AppError::Validation("schedule has no future occurrence".into()))?;
        let (method, user, token) = match principal {
            SchedulePrincipal::Session { user_id } => ("session", user_id, None),
            SchedulePrincipal::Oidc { user_id } => ("oidc", user_id, None),
            SchedulePrincipal::ApiToken { user_id, token_id } => {
                ("api_token", user_id, Some(token_id))
            }
        };
        self.schedules
            .create(&PullSchedule {
                id: Uuid::now_v7(),
                provider_id,
                mapping_logical_id: mapping.logical_id,
                mapping_version_id: mapping.version_id,
                principal_method: method.into(),
                principal_user_id: user,
                principal_token_id: token,
                cron_expression: cron,
                timezone,
                enabled: true,
                cancelled_at: None,
                next_run_at: Some(next),
                last_run_at: None,
                created_at: now,
                updated_at: now,
            })
            .await
    }
}

pub fn schedule_principal(schedule: &PullSchedule) -> Result<SchedulePrincipal, AppError> {
    match (
        schedule.principal_method.as_str(),
        schedule.principal_token_id,
    ) {
        ("session", None) => Ok(SchedulePrincipal::Session {
            user_id: schedule.principal_user_id,
        }),
        ("oidc", None) => Ok(SchedulePrincipal::Oidc {
            user_id: schedule.principal_user_id,
        }),
        ("api_token", Some(token_id)) => Ok(SchedulePrincipal::ApiToken {
            user_id: schedule.principal_user_id,
            token_id,
        }),
        _ => Err(AppError::Forbidden(
            "Schedule principal is unrepresentable".into(),
        )),
    }
}

fn normalize_cron(input: &str) -> Result<String, AppError> {
    let normalized = input
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('?', "*");
    if !(9..=200).contains(&normalized.len()) || Cron::from_str(&normalized).is_err() {
        return Err(AppError::Validation(
            "cron_expression must be a valid 5-field standard cron expression".into(),
        ));
    }
    Ok(normalized)
}

fn normalize_timezone(input: &str) -> Result<String, AppError> {
    let input = input.trim();
    if input.is_empty() || input.len() > 120 || chrono_tz::Tz::from_str(input).is_err() {
        return Err(AppError::Validation(
            "timezone must be a valid IANA timezone identifier".into(),
        ));
    }
    Ok(input.to_string())
}

/// Fire once after downtime and advance beyond the database's current instant.
pub fn next_occurrence(
    cron: &str,
    timezone: &str,
    after: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, AppError> {
    let cron =
        Cron::from_str(cron).map_err(|_| AppError::Validation("invalid schedule cron".into()))?;
    let tz = chrono_tz::Tz::from_str(timezone)
        .map_err(|_| AppError::Validation("invalid schedule timezone".into()))?;
    let mut wall = after.with_timezone(&tz).naive_local();
    for _ in 0..1500 {
        let Some(candidate) = cron.next_after(Utc.from_utc_datetime(&wall)) else {
            return Ok(None);
        };
        wall = candidate.naive_utc();
        let next = match tz.from_local_datetime(&wall) {
            chrono::LocalResult::Single(value) => Some(value.with_timezone(&Utc)),
            chrono::LocalResult::Ambiguous(first, _) => Some(first.with_timezone(&Utc)),
            chrono::LocalResult::None => None,
        };
        if let Some(next) = next
            && next > after
        {
            return Ok(Some(next));
        }
    }
    Err(AppError::Validation(
        "schedule cannot resolve a future occurrence".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downtime_advances_once_to_the_next_future_occurrence() {
        let now = DateTime::parse_from_rfc3339("2026-11-10T12:34:30Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(
            next_occurrence("*/5 * * * *", "UTC", now)
                .unwrap()
                .unwrap()
                .to_rfc3339(),
            "2026-11-10T12:35:00+00:00"
        );
        assert!(normalize_cron("not cron").is_err());
        assert!(normalize_timezone("not/a-zone").is_err());
    }

    #[test]
    fn daylight_saving_gap_and_fold_never_return_a_past_instant() {
        let gap = DateTime::parse_from_rfc3339("2026-03-08T06:59:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let next = next_occurrence("30 2 * * *", "America/New_York", gap)
            .unwrap()
            .unwrap();
        assert_eq!(next.to_rfc3339(), "2026-03-09T06:30:00+00:00");
        let fold = DateTime::parse_from_rfc3339("2026-11-01T06:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert!(
            next_occurrence("30 1 * * *", "America/New_York", fold)
                .unwrap()
                .unwrap()
                > fold
        );
    }
}
