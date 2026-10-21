//! Sync domain models for sources, mappings, inventory and jobs, with their re-exports.
pub mod entity_mapping;
pub mod field_mapping;
pub mod inbound_event;
pub mod inventory_sync;
pub mod inventory_target_schema;
pub mod mapping_configuration;
pub mod outbound_event;
pub mod provider;
pub mod provider_request_journal;
pub mod source_bootstrap_state;
pub mod source_runtime;
pub mod sync_job;
pub mod transform;

pub use entity_mapping::EntityMapping;
pub use field_mapping::FieldMapping;
pub use inbound_event::{InboundWebhookEvent, InboundWebhookEventStatus};
pub use inventory_sync::{
    CommitPull, InventoryCommitAuthorisation, InventoryCommitStats, InventorySource,
    InventoryValue, LocalInventoryCatalogView, LocalInventoryResource, LocalInventoryResourceField,
    LocalInventoryTarget, MappingConfigDocument, MappingRef, MappingTarget, SourceEntity,
    SourceIdentity, SourcePage, SourceRecord, SourceSnapshot, TargetSyncRequest,
    TypedInventoryBatch, TypedInventoryRow,
};
pub use inventory_target_schema::{
    Absent, ENTITIES, EntityMeta, FieldKind, FieldSpec, InventoryEntity, InventoryField, PgType,
    TAGGED_OBJECT_SOURCE_KEY,
};
pub use mapping_configuration::{
    ConflictMode, MappingConfiguration, MappingConfigurationStatus, MappingDirection,
};
pub use outbound_event::{OutboundEvent, OutboundEventCounters, OutboundEventStatus};
pub use provider::{
    CustomProviderConfig, Provider, ProviderType, SYNC_PROVIDER_CONSUMER_TYPE,
    SYNC_PROVIDER_FIELD_KEY_API_TOKEN, SYNC_PROVIDER_FIELD_KEY_WEBHOOK_SECRET,
    SYNC_PROVIDER_OWNER_SCOPE_TYPE, parse_custom_provider_config,
};
pub use provider_request_journal::{JournalState, ProviderRequestJournalEntry};
pub use source_bootstrap_state::{SourceBootstrapState, SourceBootstrapStateStatus};
pub use source_runtime::{
    SourceBootstrapStatus, SourceOperationalStatus, SourceRuntime, SourceRuntimeErrorCode,
};
pub use sync_job::{SyncJob, SyncJobStep, SyncStatus, SyncStepStatus};
pub use transform::{
    ArgKind, Operator, OperatorSig, OutputType, TransformDirection, evaluate_expression,
};
