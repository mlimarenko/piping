use crate::errors::AppError;

pub const INTERNAL_PASSWORD_MIN_LENGTH: usize = 8;

pub fn effective_password_min_length(configured_min_length: usize) -> usize {
    configured_min_length.max(INTERNAL_PASSWORD_MIN_LENGTH)
}

pub fn validate_local_password(
    password: &str,
    configured_min_length: usize,
) -> Result<(), AppError> {
    let effective_min_length = effective_password_min_length(configured_min_length);
    if password.len() < effective_min_length {
        return Err(AppError::Validation(format!(
            "Password must be at least {} characters long",
            effective_min_length
        )));
    }

    if !password.chars().any(|ch| ch.is_ascii_alphabetic()) {
        return Err(AppError::Validation(
            "Password must include at least one letter".to_string(),
        ));
    }

    if !password.chars().any(|ch| ch.is_ascii_digit()) {
        return Err(AppError::Validation(
            "Password must include at least one digit".to_string(),
        ));
    }

    Ok(())
}
