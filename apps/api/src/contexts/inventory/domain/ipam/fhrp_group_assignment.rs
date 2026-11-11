use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::errors::AppError;

/// Assignment to a device interface or virtual-machine interface.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default, deny_unknown_fields)]
pub struct FhrpGroupAssignment {
    pub id: Uuid,
    pub group_id: Uuid,
    pub interface_type: String,
    pub interface_id: Uuid,
    pub priority: i16,
    /// Server-managed creation timestamp; ignored on write.
    #[schema(read_only)]
    pub created: Option<DateTime<Utc>>,
    /// Server-managed update timestamp; ignored on write.
    #[schema(read_only)]
    pub last_updated: Option<DateTime<Utc>>,
    pub external_id: Option<i64>,
    pub piping_synced_at: Option<DateTime<Utc>>,
}

impl FhrpGroupAssignment {
    pub fn validate(&self) -> Result<(), AppError> {
        if !(0..=255).contains(&self.priority) {
            return Err(AppError::Validation(
                "priority must be between 0 and 255".into(),
            ));
        }
        if !matches!(
            self.interface_type.as_str(),
            "dcim.interface" | "virtualization.vminterface"
        ) {
            return Err(AppError::Validation(
                "interface_type must be dcim.interface or virtualization.vminterface".into(),
            ));
        }
        if self.group_id.is_nil() || self.interface_id.is_nil() {
            return Err(AppError::Validation(
                "group_id and interface_id are required".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_typed_interface_and_priority_endpoints() {
        let mut assignment = FhrpGroupAssignment {
            group_id: Uuid::from_u128(1),
            interface_id: Uuid::from_u128(2),
            interface_type: "dcim.interface".into(),
            ..Default::default()
        };
        for priority in [0, 255] {
            assignment.priority = priority;
            assert!(assignment.validate().is_ok());
        }
        for priority in [-1, 256] {
            assignment.priority = priority;
            assert!(assignment.validate().is_err());
        }
        assignment.priority = 0;
        assignment.interface_type = "virtualization.vminterface".into();
        assert!(assignment.validate().is_ok());
        assignment.interface_type = "dcim.device".into();
        assert!(assignment.validate().is_err());
    }
}
