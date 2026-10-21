use axum::http::HeaderMap;

use crate::errors::AppError;

pub const IDEMPOTENCY_KEY_HEADER: &str = "x-idempotency-key";
const MAX_IDEMPOTENCY_KEY_LEN: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdempotencyKeyContext {
    pub value: String,
}

pub fn extract_idempotency_key(
    headers: &HeaderMap,
) -> Result<Option<IdempotencyKeyContext>, AppError> {
    let Some(value) = headers.get(IDEMPOTENCY_KEY_HEADER) else {
        return Ok(None);
    };
    let value = value
        .to_str()
        .map_err(|_| AppError::Validation("x-idempotency-key must be valid ASCII".to_string()))?;
    let value = value.trim();
    if value.is_empty() {
        return Err(AppError::Validation(
            "x-idempotency-key must not be empty".to_string(),
        ));
    }
    if value.len() > MAX_IDEMPOTENCY_KEY_LEN {
        return Err(AppError::Validation(format!(
            "x-idempotency-key must be at most {MAX_IDEMPOTENCY_KEY_LEN} characters"
        )));
    }

    Ok(Some(IdempotencyKeyContext {
        value: value.to_string(),
    }))
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue};

    use super::{IDEMPOTENCY_KEY_HEADER, MAX_IDEMPOTENCY_KEY_LEN, extract_idempotency_key};
    use crate::errors::AppError;

    #[test]
    fn extracts_valid_header() {
        let mut headers = HeaderMap::new();
        headers.insert(
            IDEMPOTENCY_KEY_HEADER,
            HeaderValue::from_static("license-refresh-1"),
        );

        let context = extract_idempotency_key(&headers)
            .expect("header should parse")
            .expect("header should exist");
        assert_eq!(context.value, "license-refresh-1");
    }

    #[test]
    fn rejects_empty_header() {
        let mut headers = HeaderMap::new();
        headers.insert(IDEMPOTENCY_KEY_HEADER, HeaderValue::from_static(""));

        let error = extract_idempotency_key(&headers).expect_err("empty key must fail");
        assert!(matches!(error, AppError::Validation(_)));
    }

    #[test]
    fn rejects_overlong_header() {
        let mut headers = HeaderMap::new();
        let value = "x".repeat(MAX_IDEMPOTENCY_KEY_LEN + 1);
        headers.insert(
            IDEMPOTENCY_KEY_HEADER,
            HeaderValue::from_str(&value).expect("header should be valid"),
        );

        let error = extract_idempotency_key(&headers).expect_err("overlong key must fail");
        assert!(matches!(error, AppError::Validation(_)));
    }
}
