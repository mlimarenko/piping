use std::sync::Arc;

use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

use crate::contexts::audit::application::AuditRecorder;
use crate::contexts::audit::domain::AuditEvent;
use crate::contexts::inventory::domain::Tenant;
use crate::contexts::inventory::infrastructure::persistence::TenantRepository;
use crate::errors::AppError;

pub struct InventoryService<TRepo, A>
where
    TRepo: TenantRepository,
    A: AuditRecorder,
{
    tenant_repository: Arc<TRepo>,
    audit_service: Arc<A>,
}

impl<TRepo, A> InventoryService<TRepo, A>
where
    TRepo: TenantRepository,
    A: AuditRecorder,
{
    pub fn new(tenant_repository: Arc<TRepo>, audit_service: Arc<A>) -> Self {
        Self {
            tenant_repository,
            audit_service,
        }
    }

    pub async fn get_tenant(&self, tenant_id: Uuid) -> Result<Option<Tenant>, AppError> {
        let tenant = self.tenant_repository.get_by_id(tenant_id).await?;
        if tenant.is_some() {
            let event = AuditEvent {
                id: Uuid::now_v7(),
                event_type: "inventory.tenant.read".to_string(),
                actor_id: None,
                actor_kind: "user".to_string(),
                target_type: Some("tenant".to_string()),
                target_id: Some(tenant_id),
                metadata: json!({}),
                created_at: Utc::now(),
            };
            self.audit_service.record_event(event).await?;
        }
        Ok(tenant)
    }

    pub async fn create_tenant(
        &self,
        tenant: Tenant,
        actor_id: Option<Uuid>,
    ) -> Result<(), AppError> {
        self.tenant_repository.create(&tenant).await?;
        let event = AuditEvent {
            id: Uuid::now_v7(),
            event_type: "inventory.tenant.created".to_string(),
            actor_id,
            actor_kind: "user".to_string(),
            target_type: Some("tenant".to_string()),
            target_id: Some(tenant.id),
            metadata: json!({ "name": tenant.name }),
            created_at: Utc::now(),
        };
        self.audit_service.record_event(event).await?;
        Ok(())
    }

    pub async fn update_tenant(
        &self,
        tenant: Tenant,
        actor_id: Option<Uuid>,
    ) -> Result<(), AppError> {
        self.tenant_repository.update(&tenant).await?;
        let event = AuditEvent {
            id: Uuid::now_v7(),
            event_type: "inventory.tenant.updated".to_string(),
            actor_id,
            actor_kind: "user".to_string(),
            target_type: Some("tenant".to_string()),
            target_id: Some(tenant.id),
            metadata: json!({ "name": tenant.name }),
            created_at: Utc::now(),
        };
        self.audit_service.record_event(event).await?;
        Ok(())
    }

    pub async fn delete_tenant(
        &self,
        tenant_id: Uuid,
        actor_id: Option<Uuid>,
    ) -> Result<(), AppError> {
        self.tenant_repository.delete(tenant_id).await?;
        let event = AuditEvent {
            id: Uuid::now_v7(),
            event_type: "inventory.tenant.deleted".to_string(),
            actor_id,
            actor_kind: "user".to_string(),
            target_type: Some("tenant".to_string()),
            target_id: Some(tenant_id),
            metadata: json!({}),
            created_at: Utc::now(),
        };
        self.audit_service.record_event(event).await?;
        Ok(())
    }
}
