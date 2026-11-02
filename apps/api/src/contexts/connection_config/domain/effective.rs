use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use super::scope::{ConnConfProtocol, ConnConfScope};

/// Source attribution for a single resolved leaf in the effective config.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ConnConfSource {
    /// The record whose value won at this leaf.
    pub record_id: Uuid,
    pub record_name: String,
    /// The scope the winning record was assigned at (None for template-only
    /// values that came purely from a `base_record_id` chain with no scope).
    pub scope_type: Option<ConnConfScope>,
    pub scope_id: Option<Uuid>,
}

/// Result of resolving the effective connection config for a device + protocol.
///
/// `params` is the deep-merged parameter tree (most-specific-wins, with
/// `{"unset": true}` deletions already applied). `attribution` maps a dotted
/// leaf path to the source that produced it.
#[derive(Debug, Clone, Serialize)]
pub struct EffectiveConnConfig {
    pub protocol: ConnConfProtocol,
    pub params: Value,
    pub attribution: std::collections::BTreeMap<String, ConnConfSource>,
    /// Ordered list of record ids that participated, least → most specific.
    pub resolution_chain: Vec<Uuid>,
}
