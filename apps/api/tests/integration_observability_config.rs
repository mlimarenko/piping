//! Integration test target.

use piping_backend::config::{self, ConfigValidationError};

fn validatable_settings() -> Result<config::Settings, Box<dyn std::error::Error + Send + Sync>> {
    let mut settings = config::load_settings().map_err(|e| format!("load settings: {e}"))?;
    settings.database.url = "postgres://user:password@localhost:5432/piping".to_string();
    settings.redis.url = "redis://localhost:6379".to_string();
    // Keep config validation focused on observability noise-policy assertions.
    // Avoid coupling these tests to process env for secrets key material.
    settings.secrets.cipher.key_material_ref = "inline:test-key-material".to_string();
    settings.security.allow_weak_dev_secrets = true;
    Ok(settings)
}

#[test]
fn observability_noise_policy_defaults_are_loaded() {
    let settings = config::load_settings().expect("load settings");

    assert_eq!(settings.observability.noise_window_ms, 30_000);
    assert_eq!(settings.observability.noise_max_events_per_window, 40);
    assert_eq!(settings.observability.noise_overflow_mode, "summarize");
}

#[test]
fn validate_rejects_zero_noise_window() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut settings = validatable_settings()?;
    settings.observability.noise_window_ms = 0;

    let error = settings.validate().expect_err("must reject zero window");
    assert!(matches!(
        error,
        ConfigValidationError::InvalidObservabilityNoiseWindow
    ));
    Ok(())
}

#[test]
fn validate_rejects_zero_noise_max_events_per_window()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut settings = validatable_settings()?;
    settings.observability.noise_max_events_per_window = 0;

    let error = settings
        .validate()
        .expect_err("must reject zero max events per window");
    assert!(matches!(
        error,
        ConfigValidationError::InvalidObservabilityNoiseMaxEventsPerWindow
    ));
    Ok(())
}

#[test]
fn validate_rejects_unknown_noise_overflow_mode()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut settings = validatable_settings()?;
    settings.observability.noise_overflow_mode = "drop-all".to_string();

    let error = settings
        .validate()
        .expect_err("must reject invalid overflow mode");
    assert!(matches!(
        error,
        ConfigValidationError::InvalidObservabilityNoiseOverflowMode
    ));
    Ok(())
}

#[test]
fn validate_accepts_noise_policy_with_valid_values()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut settings = validatable_settings()?;
    settings.observability.noise_window_ms = 30_000;
    settings.observability.noise_max_events_per_window = 50;
    settings.observability.noise_overflow_mode = "suppress".to_string();

    settings.validate().expect("valid settings");
    Ok(())
}

#[test]
fn validate_accepts_noise_policy_off_mode() -> Result<(), Box<dyn std::error::Error + Send + Sync>>
{
    let mut settings = validatable_settings()?;
    settings.observability.noise_overflow_mode = "off".to_string();
    settings.observability.noise_max_events_per_window = 1;

    settings.validate().expect("off mode should be valid");
    Ok(())
}

#[test]
fn validate_rejects_summarize_mode_with_too_small_budget()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut settings = validatable_settings()?;
    settings.observability.noise_overflow_mode = "summarize".to_string();
    settings.observability.noise_max_events_per_window = 1;

    let error = settings
        .validate()
        .expect_err("summarize with budget=1 must be rejected");
    assert!(matches!(
        error,
        ConfigValidationError::InvalidObservabilityNoiseCombination
    ));
    Ok(())
}
