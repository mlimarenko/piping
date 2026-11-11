//! SQLx-backed configuration repositories for instance branding and monitoring labels.
pub mod instance_branding_repository;
pub mod monitoring_sd_label_settings_repository;

pub use instance_branding_repository::SqlxInstanceBrandingRepository;
pub use monitoring_sd_label_settings_repository::SqlxMonitoringSdLabelSettingsRepository;
