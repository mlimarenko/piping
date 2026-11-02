// Immutable mapping configuration domain entity.
//
// PK = (logical_id, version_id). INSERT-ONLY: edits MUST create a new
// (logical_id, version_id+1) row. UPDATE is not permitted on this table.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::inventory_sync::{MappingRef, MappingTarget, TargetSyncRequest};

/// Lifecycle status of a mapping configuration version.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MappingConfigurationStatus {
    Draft,
    Active,
    Retired,
}

impl MappingConfigurationStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            MappingConfigurationStatus::Draft => "draft",
            MappingConfigurationStatus::Active => "active",
            MappingConfigurationStatus::Retired => "retired",
        }
    }

    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "draft" => Some(MappingConfigurationStatus::Draft),
            "active" => Some(MappingConfigurationStatus::Active),
            "retired" => Some(MappingConfigurationStatus::Retired),
            _ => None,
        }
    }
}

/// Sync direction.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MappingDirection {
    Pull,
    Push,
    Bidirectional,
}

impl MappingDirection {
    pub fn as_str(&self) -> &'static str {
        match self {
            MappingDirection::Pull => "pull",
            MappingDirection::Push => "push",
            MappingDirection::Bidirectional => "bidirectional",
        }
    }

    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "pull" => Some(MappingDirection::Pull),
            "push" => Some(MappingDirection::Push),
            "bidirectional" => Some(MappingDirection::Bidirectional),
            _ => None,
        }
    }
}

/// Per-job conflict-resolution mode.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConflictMode {
    NetboxWins,
    PipingWins,
    LastWriteWins,
    FieldLevel,
}

impl ConflictMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            ConflictMode::NetboxWins => "netbox_wins",
            ConflictMode::PipingWins => "piping_wins",
            ConflictMode::LastWriteWins => "last_write_wins",
            ConflictMode::FieldLevel => "field_level",
        }
    }

    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "netbox_wins" => Some(ConflictMode::NetboxWins),
            "piping_wins" => Some(ConflictMode::PipingWins),
            "last_write_wins" => Some(ConflictMode::LastWriteWins),
            "field_level" => Some(ConflictMode::FieldLevel),
            _ => None,
        }
    }
}

/// Immutable mapping configuration (one version of one logical mapping).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MappingConfiguration {
    pub logical_id: Uuid,
    pub version_id: i32,
    pub name: String,
    pub description: Option<String>,
    pub source_provider_id: Uuid,
    /// `LocalInventory` or a remote provider target.
    pub target: MappingTarget,
    pub status: MappingConfigurationStatus,
    pub direction: MappingDirection,
    pub conflict_mode: ConflictMode,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub retired_at: Option<DateTime<Utc>>,
}

impl MappingConfiguration {
    /// Immutable identity `<logical_id>:<version_id>`.
    #[must_use]
    pub fn mapping_ref(&self) -> MappingRef {
        MappingRef::new(self.logical_id, self.version_id)
    }

    /// The job request this configuration authorises, subject to the supported
    /// shape gate in [`TargetSyncRequest::ensure_supported`].
    #[must_use]
    pub fn target_sync_request(&self) -> TargetSyncRequest {
        TargetSyncRequest {
            reference: self.mapping_ref(),
            source_provider_id: self.source_provider_id,
            target: self.target.clone(),
            direction: self.direction,
            conflict_mode: self.conflict_mode,
        }
    }
}
