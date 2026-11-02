//! NetBox integration barrel: client, resource registry, repository and schema provider re-exports.
pub mod client;
pub mod registry;
pub mod repository;
pub mod schema_provider;

pub use client::NetboxClient;
pub use registry::{
    FieldMeta, FilterParam, NetboxResourceDescriptor, NetboxResourceMeta, NetboxResourceRegistry,
};
pub use repository::{BulkFailure, BulkResult, NetboxPage, NetboxRepository};
pub use schema_provider::NetboxSchemaProvider;
