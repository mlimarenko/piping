use serde::{Deserialize, Serialize};

use crate::contexts::connections::application::connection_params_schema::ConnectionParamValidationError;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
pub struct ValidationErrorDetail {
    pub field_path: String,
    pub constraint: String,
    pub message: String,
    pub suggestion: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
pub struct ValidationErrorResponse {
    pub error: String,
    pub message: String,
    pub details: Vec<ValidationErrorDetail>,
}

impl From<Vec<ConnectionParamValidationError>> for ValidationErrorResponse {
    fn from(errors: Vec<ConnectionParamValidationError>) -> Self {
        Self {
            error: "VALIDATION_ERROR".to_string(),
            message: "Connection parameters validation failed".to_string(),
            details: errors
                .into_iter()
                .map(|error| ValidationErrorDetail {
                    field_path: error.field_path,
                    constraint: error.constraint,
                    message: error.message,
                    suggestion: error.suggestion,
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ValidationErrorResponse;
    use crate::contexts::connections::application::connection_params_schema::ConnectionParamValidationError;

    #[test]
    fn maps_domain_validation_errors_to_rest_shape() {
        let source = vec![ConnectionParamValidationError {
            field_path: "piping.connections.ssh[0].host".to_string(),
            constraint: "required".to_string(),
            message: "Field is required".to_string(),
            suggestion: Some("Set host".to_string()),
        }];

        let response = ValidationErrorResponse::from(source);
        assert_eq!(response.error, "VALIDATION_ERROR");
        assert_eq!(response.details.len(), 1);
        assert_eq!(
            response.details[0].field_path,
            "piping.connections.ssh[0].host"
        );
    }
}
