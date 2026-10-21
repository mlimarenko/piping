//! Integration test target.

use piping_backend::contexts::access_control::application::{
    generate_local_password, validate_local_password,
};

#[test]
fn generator_produces_policy_compliant_password() {
    for _ in 0..32 {
        let generated = generate_local_password(8).expect("password");
        validate_local_password(&generated, 8).expect("generated password must pass policy");
    }
}

#[test]
fn generator_respects_configured_min_length() {
    let generated = generate_local_password(20).expect("password");
    assert!(generated.len() >= 20);
}
