//! Integration test target.

use piping_backend::contexts::access_control::application::validate_local_password;

#[test]
fn password_policy_accepts_letters_digits_and_min_length() {
    let result = validate_local_password("Alpha1234", 8);
    assert!(result.is_ok());
}

#[test]
fn password_policy_rejects_short_password() {
    let error = validate_local_password("Abc123", 8).expect_err("must reject short password");
    assert!(
        error
            .to_string()
            .contains("Password must be at least 8 characters long")
    );
}

#[test]
fn password_policy_rejects_password_without_letter() {
    let error =
        validate_local_password("12345678", 8).expect_err("must reject password without letter");
    assert!(
        error
            .to_string()
            .contains("Password must include at least one letter")
    );
}

#[test]
fn password_policy_rejects_password_without_digit() {
    let error =
        validate_local_password("abcdefgh", 8).expect_err("must reject password without digit");
    assert!(
        error
            .to_string()
            .contains("Password must include at least one digit")
    );
}
