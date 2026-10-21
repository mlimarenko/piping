use crate::errors::AppError;

/// A custom-field domain validation failure. Carries the offending field name
/// (when known) and a human-readable message. Converts into the app-wide
/// [`AppError::Validation`] at the service boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomFieldError {
    pub field: Option<String>,
    pub message: String,
}

impl CustomFieldError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            field: None,
            message: message.into(),
        }
    }

    pub fn for_field(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            field: Some(field.into()),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for CustomFieldError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.field {
            Some(field) => write!(f, "{field}: {}", self.message),
            None => write!(f, "{}", self.message),
        }
    }
}

impl std::error::Error for CustomFieldError {}

impl From<CustomFieldError> for AppError {
    fn from(error: CustomFieldError) -> Self {
        AppError::Validation(error.to_string())
    }
}
