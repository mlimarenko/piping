use std::collections::BTreeMap;

use once_cell::sync::Lazy;
use regex::Regex;

type CompiledRegex = Result<Regex, regex::Error>;

static PASSWORD_RE: Lazy<CompiledRegex> =
    Lazy::new(|| Regex::new(r"(?i)(password\s*[=:]\s*)[^\s,;]+"));
static TOKEN_RE: Lazy<CompiledRegex> = Lazy::new(|| Regex::new(r"(?i)(token\s*[=:]\s*)[^\s,;]+"));
static BEARER_RE: Lazy<CompiledRegex> =
    Lazy::new(|| Regex::new(r"(?i)bearer\s+[A-Za-z0-9\-._~+/]+=*"));
static PAT_TOKEN_RE: Lazy<CompiledRegex> =
    Lazy::new(|| Regex::new(r"pat_[0-9a-fA-F-]{36}_[A-Za-z0-9\-_]+"));
static PLAYBOOK_JOB_TOKEN_RE: Lazy<CompiledRegex> =
    Lazy::new(|| Regex::new(r"pjt_[0-9a-fA-F-]{36}_[A-Za-z0-9\-_]+"));
static API_TOKEN_KEY_RE: Lazy<CompiledRegex> =
    Lazy::new(|| Regex::new(r#"(?i)("?(?:api[_-]?token)"?\s*[:=]\s*"?)([^",;\s}]+)("?)"#));
static TOKEN_VALUE_KEY_RE: Lazy<CompiledRegex> =
    Lazy::new(|| Regex::new(r#"(?i)("?(?:token[_-]?value)"?\s*[:=]\s*"?)([^",;\s}]+)("?)"#));
static SECRET_KEY_RE: Lazy<CompiledRegex> = Lazy::new(|| {
    Regex::new(r#"(?i)("?(?:webhook[_-]?secret|client[_-]?secret)"?\s*[:=]\s*"?)([^",;\s}]+)("?)"#)
});
static SECRET_SCHEME_UUID_RE: Lazy<CompiledRegex> = Lazy::new(|| {
    Regex::new(
        r"(?i)[a-z][a-z0-9+.-]*://[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}",
    )
});
static SECRET_ID_KEY_RE: Lazy<CompiledRegex> = Lazy::new(|| {
    Regex::new(
        r#"(?i)("?(?:secret[_-]?id|secretId)"?\s*[:=]\s*"?)([0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12})("?)"#,
    )
});
static SECRET_FIELD_KEY_RE: Lazy<CompiledRegex> =
    Lazy::new(|| Regex::new(r#"(?i)("?(?:field[_-]?key|fieldKey)"?\s*[:=]\s*"?)([^",;\s}]+)("?)"#));
static CREDENTIAL_URL_RE: Lazy<CompiledRegex> =
    Lazy::new(|| Regex::new(r"([a-zA-Z][a-zA-Z0-9+.-]*://[^\s:@/]+:)[^@\s/]+(@)"));

fn apply_scrub_regex(input: String, regex: &CompiledRegex, replacement: &str) -> String {
    match regex {
        Ok(compiled) => compiled.replace_all(&input, replacement).to_string(),
        // Deterministic secure fallback: if regex failed to compile, scrub whole payload.
        Err(_) => "SCRUBBED".to_string(),
    }
}

/// Scrub sensitive fragments from a free-form string.
pub fn scrub_text(input: &str) -> String {
    let mut output = input.to_string();
    output = apply_scrub_regex(output, &PASSWORD_RE, "${1}SCRUBBED");
    output = apply_scrub_regex(output, &TOKEN_RE, "${1}SCRUBBED");
    output = apply_scrub_regex(output, &BEARER_RE, "Bearer SCRUBBED");
    output = apply_scrub_regex(output, &PAT_TOKEN_RE, "pat_SCRUBBED");
    output = apply_scrub_regex(output, &PLAYBOOK_JOB_TOKEN_RE, "pjt_SCRUBBED");
    output = apply_scrub_regex(output, &API_TOKEN_KEY_RE, "${1}SCRUBBED${3}");
    output = apply_scrub_regex(output, &TOKEN_VALUE_KEY_RE, "${1}SCRUBBED${3}");
    output = apply_scrub_regex(output, &SECRET_KEY_RE, "${1}SCRUBBED${3}");
    output = apply_scrub_regex(output, &SECRET_SCHEME_UUID_RE, "SCHEME://SCRUBBED");
    output = apply_scrub_regex(output, &SECRET_ID_KEY_RE, "${1}SCRUBBED${3}");
    output = apply_scrub_regex(output, &SECRET_FIELD_KEY_RE, "${1}SCRUBBED${3}");
    apply_scrub_regex(output, &CREDENTIAL_URL_RE, "${1}SCRUBBED${2}")
}

fn is_sensitive_context_key(key: &str) -> bool {
    let key = key.trim().to_ascii_lowercase();
    key == "authorization"
        || key == "cookie"
        || key == "set-cookie"
        || key.ends_with("password")
        || key.ends_with("_password")
        || key.ends_with("-password")
        || key.ends_with("token")
        || key.ends_with("_token")
        || key.ends_with("-token")
        || key.ends_with("token_value")
        || key.ends_with("token-value")
        || key.ends_with("secret")
        || key.ends_with("_secret")
        || key.ends_with("-secret")
}

/// Scrub a structured context map.
pub fn scrub_context(context: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    context
        .iter()
        .map(|(key, value)| {
            if is_sensitive_context_key(key) {
                (key.clone(), "SCRUBBED".to_string())
            } else {
                (key.clone(), scrub_text(value))
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{scrub_context, scrub_text};
    use std::collections::BTreeMap;

    #[test]
    fn scrub_text_replaces_password_and_token() {
        let value = "password=supersecret token=abcdef";
        let scrubbed = scrub_text(value);
        assert!(!scrubbed.contains("supersecret"));
        assert!(!scrubbed.contains("abcdef"));
        assert!(scrubbed.contains("SCRUBBED"));
    }

    #[test]
    fn scrub_context_scrubs_each_value() {
        let mut context = BTreeMap::new();
        context.insert("auth".to_string(), "Bearer very-secret-token".to_string());
        let scrubbed = scrub_context(&context);
        assert_eq!(
            scrubbed.get("auth").map(String::as_str),
            Some("Bearer SCRUBBED")
        );
    }

    #[test]
    fn scrub_text_masks_canonical_secret_identifiers() {
        let value = r#"secret_id=019c2f1b-f106-7f91-9697-826530777788 other=http://019c2f1b-f106-7f91-9697-826530777788"#;
        let scrubbed = scrub_text(value);
        assert!(!scrubbed.contains("019c2f1b-f106-7f91-9697-826530777788"));
        assert!(scrubbed.contains("secret_id=SCRUBBED"));
        assert!(scrubbed.contains("other=SCHEME://SCRUBBED"));
    }

    #[test]
    fn scrub_text_masks_secret_field_payload_keys() {
        let value = r#"{"field_key":"piping.connections.ssh.0.password","intent":"set"}"#;
        let scrubbed = scrub_text(value);
        assert!(!scrubbed.contains("piping.connections.ssh.0.password"));
        assert!(scrubbed.contains(r#""field_key":"SCRUBBED""#));
    }

    #[test]
    fn scrub_text_masks_provider_secret_keys_in_json() {
        let value = r#"{"api_token":"abc","webhook_secret":"def","client_secret":"ghi"}"#;
        let scrubbed = scrub_text(value);
        assert!(!scrubbed.contains("abc"));
        assert!(!scrubbed.contains("def"));
        assert!(!scrubbed.contains("ghi"));
        assert!(scrubbed.contains(r#""api_token":"SCRUBBED""#));
        assert!(scrubbed.contains(r#""webhook_secret":"SCRUBBED""#));
        assert!(scrubbed.contains(r#""client_secret":"SCRUBBED""#));
    }

    #[test]
    fn scrub_text_masks_provider_secret_keys_in_key_value_text() {
        let value = "api_token=abc webhook_secret=def client_secret=ghi";
        let scrubbed = scrub_text(value);
        assert!(!scrubbed.contains("abc"));
        assert!(!scrubbed.contains("def"));
        assert!(!scrubbed.contains("ghi"));
        assert!(scrubbed.contains("api_token=SCRUBBED"));
        assert!(scrubbed.contains("webhook_secret=SCRUBBED"));
        assert!(scrubbed.contains("client_secret=SCRUBBED"));
    }

    #[test]
    fn scrub_context_masks_sensitive_keys_even_when_value_has_no_marker() {
        let mut context = BTreeMap::new();
        context.insert("api_token".to_string(), "abc".to_string());
        context.insert("webhook_secret".to_string(), "def".to_string());
        context.insert("note".to_string(), "ok".to_string());
        let scrubbed = scrub_context(&context);
        assert_eq!(
            scrubbed.get("api_token").map(String::as_str),
            Some("SCRUBBED")
        );
        assert_eq!(
            scrubbed.get("webhook_secret").map(String::as_str),
            Some("SCRUBBED")
        );
        assert_eq!(scrubbed.get("note").map(String::as_str), Some("ok"));
    }

    #[test]
    fn scrub_text_masks_pat_token_and_token_value_key() {
        let value = r#"token_value=pat_019c2f1b-f106-7f91-9697-826530777788_AbCdEf123 message=ok"#;
        let scrubbed = scrub_text(value);
        assert!(!scrubbed.contains("AbCdEf123"));
        assert!(scrubbed.contains("token_value=SCRUBBED"));
        assert!(!scrubbed.contains("pat_019c2f1b-f106-7f91-9697-826530777788"));
    }

    #[test]
    fn scrub_text_masks_playbook_job_token() {
        let value = "callback pjt_019c2f1b-f106-7f91-9697-826530777788_AbCdEf123 status=ok";
        let scrubbed = scrub_text(value);
        assert!(!scrubbed.contains("AbCdEf123"));
        assert!(!scrubbed.contains("pjt_019c2f1b-f106-7f91-9697-826530777788"));
        assert!(scrubbed.contains("pjt_SCRUBBED"));
    }

    #[test]
    fn scrub_context_masks_token_value_key() {
        let mut context = BTreeMap::new();
        context.insert(
            "token_value".to_string(),
            "pat_019c2f1b-f106-7f91-9697-826530777788_AbCdEf123".to_string(),
        );
        let scrubbed = scrub_context(&context);
        assert_eq!(
            scrubbed.get("token_value").map(String::as_str),
            Some("SCRUBBED")
        );
    }
}
