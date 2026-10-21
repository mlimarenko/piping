//! Integration test target.

use piping_backend::config;

#[test]
fn config_defaults_match_canonical_constants() {
    // Minimal JSON payload: required sections present; inner fields rely on serde defaults.
    let raw = r#"{
  "server": { "bind": "0.0.0.0:8080" },
  "database": { "url": "postgresql://user:pass@localhost:5432/piping" },
  "redis": { "url": "redis://localhost:6379" },
  "observability": {}
}"#;

    let settings: config::Settings = serde_json::from_str(raw).expect("deserialize Settings");

    assert_eq!(
        settings.observability.log_level,
        config::DEFAULT_OBSERVABILITY_LOG_LEVEL
    );
    assert_eq!(
        settings.observability.log_output_format,
        config::DEFAULT_OBSERVABILITY_LOG_OUTPUT_FORMAT
    );
    assert_eq!(
        settings.playbook_runtime.max_concurrent_runs,
        config::DEFAULT_PLAYBOOK_RUNTIME_MAX_CONCURRENT_RUNS
    );
    assert_eq!(
        settings.playbook_runtime.max_concurrent_runs_per_group,
        config::DEFAULT_PLAYBOOK_RUNTIME_MAX_CONCURRENT_RUNS_PER_GROUP
    );
    assert_eq!(
        settings.ansible_runner.forks,
        config::DEFAULT_ANSIBLE_RUNNER_FORKS
    );
}
