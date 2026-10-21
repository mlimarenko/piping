//! Application error types, SQLx error classification and the mapping onto HTTP responses.
pub mod app_error;
pub mod http_error;

pub use app_error::{
    AppError, DeleteBlockedDetails, DeleteBlockedExample, DeleteBlockedGroup, classify_sqlx_error,
    classify_sqlx_error_owned, normalize_database_error,
};
pub use http_error::map_app_error_to_http;
