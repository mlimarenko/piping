//! Integration test target.

use piping_backend::config::{self, ConfigValidationError};

#[test]
fn validate_rejects_local_auth_password_min_below_internal_baseline() {
    let mut settings = config::load_settings().expect("load settings");
    settings.database.url = "postgres://user:password@localhost:5432/piping".to_string();
    settings.redis.url = "redis://localhost:6379".to_string();
    settings.secrets.cipher.key_material_ref = "inline:test-key-material".to_string();
    settings.auth.local_enabled = true;
    settings.auth.password_pepper = "pepper".to_string();
    settings.auth.password_min_length = 7;

    let error = settings
        .validate()
        .expect_err("must reject password min below 8");
    assert!(matches!(
        error,
        ConfigValidationError::InvalidAuthPasswordMinLength
    ));
}

/// Production posture: strong-secret enforcement on, local auth enabled, and no
/// local-admin password override. Startup must fail closed rather than fall
/// back to a predictable emergency credential.
///
/// The assertion deliberately stays behavioral (`is_err`) so it is meaningful
/// against the pre-change code, where `validate()` accepted this configuration.
#[test]
fn validate_fails_closed_without_production_local_admin_password() {
    let mut settings = config::load_settings().expect("load settings");
    settings.database.url =
        "postgresql://piping:strong-db-password@localhost:5432/piping".to_string();
    settings.redis.url = "redis://:strong-redis-password@localhost:6379/0".to_string();
    settings.secrets.cipher.key_material_ref =
        "inline:c3Ryb25nLTMyLWJ5dGUta2V5LWZvci10ZXN0aW5nISE=".to_string();
    settings.security.allow_weak_dev_secrets = false;
    settings.auth.local_enabled = true;
    settings.auth.password_pepper = "a-strong-production-pepper-value".to_string();

    assert!(
        settings.validate().is_err(),
        "production with local auth enabled must fail closed without a local-admin password"
    );
}
