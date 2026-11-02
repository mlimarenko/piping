//! Configuration domain models for instance branding and monitoring service-discovery labels.
pub mod instance_branding;
pub mod monitoring_sd_label_settings;

pub use instance_branding::{InstanceBranding, InstanceBrandingUpdate};
pub use monitoring_sd_label_settings::MonitoringSdLabelGroups;
