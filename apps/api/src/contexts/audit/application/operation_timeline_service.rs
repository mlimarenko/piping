use chrono::Utc;
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::contexts::audit::domain::AuditEvent;
use crate::errors::AppError;

use super::AuditRecorder;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationTimelineActionKind {
    SignupCompleted,
    SignupRejected,
    SigninCompleted,
    SigninRejected,
    ActivationCompleted,
    ActivationRejected,
}

impl OperationTimelineActionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SignupCompleted => "signup_completed",
            Self::SignupRejected => "signup_rejected",
            Self::SigninCompleted => "signin_completed",
            Self::SigninRejected => "signin_rejected",
            Self::ActivationCompleted => "activation_completed",
            Self::ActivationRejected => "activation_rejected",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationTimelineResult {
    Success,
    Rejected,
}

impl OperationTimelineResult {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Rejected => "rejected",
        }
    }
}

#[derive(Debug, Clone)]
pub struct OperationTimelineWriteRequest {
    pub organization_id: Option<Uuid>,
    pub actor_user_id: Option<Uuid>,
    pub action_kind: OperationTimelineActionKind,
    pub result: OperationTimelineResult,
    pub reason_code: Option<String>,
    pub metadata: Value,
}

pub struct OperationTimelineService<R>
where
    R: AuditRecorder,
{
    recorder: R,
}

impl<R> OperationTimelineService<R>
where
    R: AuditRecorder,
{
    pub fn new(recorder: R) -> Self {
        Self { recorder }
    }

    pub async fn write(&self, request: OperationTimelineWriteRequest) -> Result<Uuid, AppError> {
        let event = build_operation_timeline_event(request);
        let timeline_id = event.id;

        self.recorder.record_event(event).await?;
        Ok(timeline_id)
    }
}

pub fn build_operation_timeline_event(request: OperationTimelineWriteRequest) -> AuditEvent {
    let timeline_id = Uuid::now_v7();
    let created_at = Utc::now();
    let metadata = build_metadata(&request, timeline_id, created_at);
    AuditEvent {
        id: timeline_id,
        event_type: format!("timeline.{}", request.action_kind.as_str()),
        actor_id: request.actor_user_id,
        actor_kind: "user".to_string(),
        target_type: Some("organization".to_string()),
        target_id: request.organization_id,
        metadata,
        created_at,
    }
}

impl<R> Clone for OperationTimelineService<R>
where
    R: AuditRecorder + Clone,
{
    fn clone(&self) -> Self {
        Self {
            recorder: self.recorder.clone(),
        }
    }
}

fn build_metadata(
    request: &OperationTimelineWriteRequest,
    timeline_id: Uuid,
    created_at: chrono::DateTime<Utc>,
) -> Value {
    let mut metadata = Map::new();
    metadata.insert("timeline_id".to_string(), json!(timeline_id));
    metadata.insert(
        "action_kind".to_string(),
        json!(request.action_kind.as_str()),
    );
    metadata.insert("result".to_string(), json!(request.result.as_str()));
    metadata.insert("created_at".to_string(), json!(created_at.to_rfc3339()));
    if let Some(reason_code) = request.reason_code.as_ref() {
        metadata.insert("reason_code".to_string(), json!(reason_code));
    }
    if let Some(actor_user_id) = request.actor_user_id {
        metadata.insert("actor_user_id".to_string(), json!(actor_user_id));
    }
    if let Some(organization_id) = request.organization_id {
        metadata.insert("organization_id".to_string(), json!(organization_id));
    }
    if let Value::Object(extra) = request.metadata.clone() {
        metadata.extend(extra);
    }

    Value::Object(metadata)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;

    use super::*;

    #[derive(Clone, Default)]
    struct TestRecorder {
        events: Arc<Mutex<Vec<AuditEvent>>>,
    }

    #[async_trait]
    impl AuditRecorder for TestRecorder {
        async fn record_event(&self, event: AuditEvent) -> Result<(), AppError> {
            self.events
                .lock()
                .expect("test recorder mutex should not be poisoned")
                .push(event);
            Ok(())
        }
    }

    #[tokio::test]
    async fn write_records_typed_timeline_event_with_metadata() {
        let recorder = TestRecorder::default();
        let service = OperationTimelineService::new(recorder.clone());
        let organization_id = Uuid::now_v7();
        let actor_user_id = Uuid::now_v7();

        let timeline_id = service
            .write(OperationTimelineWriteRequest {
                organization_id: Some(organization_id),
                actor_user_id: Some(actor_user_id),
                action_kind: OperationTimelineActionKind::SigninCompleted,
                result: OperationTimelineResult::Success,
                reason_code: Some("AUTH_OK".to_string()),
                metadata: json!({
                    "identifier": "owner@example.com",
                    "auth_method": "local",
                }),
            })
            .await
            .expect("timeline write should succeed");

        let events = recorder
            .events
            .lock()
            .expect("test recorder mutex should not be poisoned")
            .clone();
        assert_eq!(events.len(), 1);

        let event = &events[0];
        assert_eq!(event.id, timeline_id);
        assert_eq!(event.event_type, "timeline.signin_completed");
        assert_eq!(event.actor_id, Some(actor_user_id));
        assert_eq!(event.target_type.as_deref(), Some("organization"));
        assert_eq!(event.target_id, Some(organization_id));
        assert_eq!(event.metadata["timeline_id"], json!(timeline_id));
        assert_eq!(event.metadata["action_kind"], json!("signin_completed"));
        assert_eq!(event.metadata["result"], json!("success"));
        assert_eq!(event.metadata["reason_code"], json!("AUTH_OK"));
        assert_eq!(event.metadata["identifier"], json!("owner@example.com"));
        assert_eq!(event.metadata["auth_method"], json!("local"));
        assert_eq!(event.metadata["organization_id"], json!(organization_id));
        assert_eq!(event.metadata["actor_user_id"], json!(actor_user_id));
        assert!(
            event.metadata["created_at"].as_str().is_some(),
            "created_at must be serialized into metadata"
        );
    }
}
