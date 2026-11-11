use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Virtual machine type domain model.
///
/// Defaults apply only at VM creation, and only to fields whose value is null.
/// Changing a type never overwrites existing VM attributes.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct VirtualMachineType {
    pub id: Uuid,
    #[schema(max_length = 100)]
    pub name: String,
    #[schema(max_length = 100, pattern = "^[A-Za-z0-9_-]+$")]
    pub slug: String,
    /// Default platform a VM inherits when it has no explicit platform.
    pub default_platform_id: Option<Uuid>,
    /// Default vCPUs a VM inherits when it has no explicit value.
    #[schema(minimum = 0.01, maximum = 9999.99, multiple_of = 0.01)]
    pub default_vcpus: Option<f64>,
    /// Default memory in MB a VM inherits when it has no explicit value.
    #[schema(minimum = 0)]
    pub default_memory: Option<i32>,
    pub owner_id: Option<Uuid>,
    /// Derived from assigned VMs, never writable by the caller.
    #[schema(minimum = 0, read_only)]
    pub virtual_machine_count: i64,
    #[schema(max_length = 200)]
    pub description: String,
    pub comments: String,
    pub custom_fields: Option<serde_json::Value>,
}

impl VirtualMachineType {
    pub fn apply_defaults(
        &self,
        platform: &mut Option<Uuid>,
        vcpus: &mut Option<f64>,
        memory: &mut Option<i32>,
    ) {
        *platform = platform.or(self.default_platform_id);
        *vcpus = vcpus.or(self.default_vcpus);
        *memory = memory.or(self.default_memory);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creation_defaults_preserve_explicit_vm_overrides() {
        let platform = Uuid::now_v7();
        let vm_type = VirtualMachineType {
            default_platform_id: Some(platform),
            default_vcpus: Some(0.25),
            default_memory: Some(1024),
            ..Default::default()
        };
        let (mut p, mut cpu, mut memory) = (None, None, None);
        vm_type.apply_defaults(&mut p, &mut cpu, &mut memory);
        assert_eq!((p, cpu, memory), (Some(platform), Some(0.25), Some(1024)));
        let override_platform = Uuid::now_v7();
        let (mut p, mut cpu, mut memory) = (Some(override_platform), Some(1.5), Some(0));
        vm_type.apply_defaults(&mut p, &mut cpu, &mut memory);
        assert_eq!(
            (p, cpu, memory),
            (Some(override_platform), Some(1.5), Some(0))
        );
    }
}
