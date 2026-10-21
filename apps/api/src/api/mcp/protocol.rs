//! MCP protocol envelope, rate limiting and transport guards, split out of `mcp.rs`.
//!
//! Moved verbatim; behaviour is unchanged.

use super::*;

pub(super) static MCP_RATE_LIMITER: Lazy<McpRateLimiter> = Lazy::new(McpRateLimiter::new);
#[derive(Clone)]
pub(super) struct McpRateLimiter {
    pub(super) state: Arc<Mutex<HashMap<String, VecDeque<Instant>>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct McpRateLimitDecision {
    pub(super) allowed: bool,
    pub(super) retry_after_seconds: u64,
}

impl McpRateLimiter {
    pub(super) fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub(super) async fn record_attempt(
        &self,
        key: String,
        max_attempts: usize,
    ) -> McpRateLimitDecision {
        self.record_attempt_at(
            key,
            max_attempts,
            Duration::from_secs(MCP_RATE_LIMIT_WINDOW_SECS),
            Instant::now(),
        )
        .await
    }

    pub(super) async fn record_attempt_at(
        &self,
        key: String,
        max_attempts: usize,
        window: Duration,
        now: Instant,
    ) -> McpRateLimitDecision {
        if max_attempts == 0 {
            return McpRateLimitDecision {
                allowed: true,
                retry_after_seconds: 0,
            };
        }

        let mut state = self.state.lock().await;
        let bucket = state.entry(key).or_default();
        let cutoff = now.checked_sub(window).unwrap_or(now);
        while bucket.front().is_some_and(|timestamp| *timestamp <= cutoff) {
            bucket.pop_front();
        }

        if bucket.len() >= max_attempts {
            let retry_after_seconds = bucket
                .front()
                .map(|oldest| {
                    let elapsed = now.saturating_duration_since(*oldest);
                    let remaining = window.saturating_sub(elapsed);
                    remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0)
                })
                .unwrap_or(1);
            return McpRateLimitDecision {
                allowed: false,
                retry_after_seconds,
            };
        }

        bucket.push_back(now);
        McpRateLimitDecision {
            allowed: true,
            retry_after_seconds: 0,
        }
    }
}

#[derive(Debug, Deserialize)]
pub(super) struct JsonRpcRequest {
    #[serde(default)]
    pub(super) jsonrpc: Option<String>,
    pub(super) method: String,
    #[serde(default)]
    pub(super) params: Value,
    #[serde(default)]
    pub(super) id: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ToolCallParams {
    pub(super) name: String,
    #[serde(default)]
    pub(super) arguments: Value,
}
pub(super) async fn mcp_rate_limit_decision(
    state: &AppState,
    auth_context: Option<&AuthContext>,
    request_context: &RequestContext,
) -> Option<McpRateLimitDecision> {
    let max_attempts = state.settings.auth.api_tokens.mcp_rate_limit_per_minute as usize;
    if max_attempts == 0 {
        return None;
    }
    let key = mcp_rate_limit_key(auth_context, request_context);
    Some(MCP_RATE_LIMITER.record_attempt(key, max_attempts).await)
}

pub(super) fn mcp_rate_limit_key(
    auth_context: Option<&AuthContext>,
    request_context: &RequestContext,
) -> String {
    auth_context
        .and_then(|value| value.api_token_id.as_deref())
        .map(|token_id| format!("token:{token_id}"))
        .or_else(|| {
            request_context
                .user_id
                .map(|user_id| format!("user:{user_id}"))
        })
        .or_else(|| {
            request_context
                .request_ip
                .as_deref()
                .map(|request_ip| format!("ip:{request_ip}"))
        })
        .unwrap_or_else(|| "unknown".to_string())
}

pub(super) fn parse_json_rpc_request(payload: Value) -> Result<JsonRpcRequest, String> {
    let request = serde_json::from_value::<JsonRpcRequest>(payload)
        .map_err(|error| format!("Invalid JSON-RPC request: {error}"))?;
    if request.jsonrpc.as_deref() != Some("2.0") {
        return Err("Invalid JSON-RPC version".to_string());
    }
    Ok(request)
}

pub(super) fn validate_mcp_origin(state: &AppState, headers: &HeaderMap) -> Result<(), String> {
    let Some(origin) = headers.get(ORIGIN).and_then(|value| value.to_str().ok()) else {
        return Ok(());
    };
    if mcp_origin_allowed(origin, &state.settings.server.cors_allowed_origins) {
        return Ok(());
    }
    Err("Invalid Origin for MCP endpoint".to_string())
}

pub(super) fn mcp_origin_allowed(origin: &str, allowed_origins: &[String]) -> bool {
    let origin = origin.trim().trim_end_matches('/');
    if origin.is_empty() {
        return false;
    }
    if !allowed_origins.is_empty() {
        return allowed_origins
            .iter()
            .map(|value| value.trim().trim_end_matches('/'))
            .any(|allowed| allowed == origin);
    }

    let Ok(parsed) = url::Url::parse(origin) else {
        return false;
    };
    if !matches!(parsed.scheme(), "http" | "https") {
        return false;
    }
    parsed.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1"
    })
}

pub(super) fn validate_mcp_protocol_version(
    request: &JsonRpcRequest,
    headers: &HeaderMap,
) -> Result<(), String> {
    if request.method == "initialize" {
        return Ok(());
    }
    let Some(protocol_version) = headers
        .get(MCP_PROTOCOL_VERSION_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(());
    };
    if matches!(
        protocol_version,
        MCP_PROTOCOL_VERSION | MCP_PRIOR_PROTOCOL_VERSION
    ) {
        return Ok(());
    }
    Err(format!(
        "Unsupported MCP protocol version '{protocol_version}'"
    ))
}

pub(super) fn notification_response(id: Option<Value>) -> Response {
    if id.is_some() {
        return jsonrpc_result(id, json!({}));
    }
    StatusCode::ACCEPTED.into_response()
}
pub(super) fn tool_success(value: Value) -> Value {
    tool_result(value, false)
}

pub(super) fn tool_error(value: Value) -> Value {
    tool_result(value, true)
}

pub(super) fn mcp_tool_audit_failure(id: Option<Value>) -> Response {
    jsonrpc_result(
        id,
        tool_error(json!({
            "code": MCP_AUDIT_RECORD_FAILED_CODE,
            "message": MCP_AUDIT_RECORD_FAILED_MESSAGE,
        })),
    )
}

pub(super) fn tool_result(value: Value, is_error: bool) -> Value {
    let text = serde_json::to_string(&value).unwrap_or_else(|_| value.to_string());
    json!({
        "structuredContent": value,
        "content": [
            {
                "type": "text",
                "mimeType": "application/json",
                "text": text
            }
        ],
        "isError": is_error
    })
}

pub(super) fn jsonrpc_result(id: Option<Value>, result: Value) -> Response {
    (
        StatusCode::OK,
        Json(json!({
            "jsonrpc": "2.0",
            "id": id.unwrap_or(Value::Null),
            "result": result
        })),
    )
        .into_response()
}

pub(super) fn jsonrpc_error(
    id: Option<Value>,
    code: i64,
    message: &str,
    data: Option<Value>,
) -> Response {
    let mut error = json!({
        "code": code,
        "message": message
    });
    if let Some(data) = data {
        error["data"] = data;
    }
    (
        StatusCode::OK,
        Json(json!({
            "jsonrpc": "2.0",
            "id": id.unwrap_or(Value::Null),
            "error": error
        })),
    )
        .into_response()
}

pub(super) fn mcp_rate_limit_error(id: Option<Value>, retry_after_seconds: u64) -> Response {
    let mut response = (
        StatusCode::TOO_MANY_REQUESTS,
        Json(json!({
            "jsonrpc": "2.0",
            "id": id.unwrap_or(Value::Null),
            "error": {
                "code": -32029,
                "message": "MCP rate limit exceeded",
                "data": {
                    "retry_after_seconds": retry_after_seconds
                }
            }
        })),
    )
        .into_response();
    if let Ok(value) = HeaderValue::from_str(&retry_after_seconds.to_string()) {
        response.headers_mut().insert(RETRY_AFTER, value);
    }
    response
}

pub(super) fn mcp_http_error(status: StatusCode, message: &str) -> Response {
    (
        status,
        Json(json!({
            "error": message
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rate_limiter_zero_max_attempts_allows_without_tracking() {
        let limiter = McpRateLimiter::new();
        let decision = limiter
            .record_attempt_at(
                "token:zero".to_string(),
                0,
                Duration::from_secs(60),
                Instant::now(),
            )
            .await;
        assert!(decision.allowed);
        assert_eq!(decision.retry_after_seconds, 0);
    }

    #[test]
    fn json_rpc_request_version_is_validated() {
        let request = parse_json_rpc_request(json!({"jsonrpc": "2.0", "method": "ping"}));
        assert_eq!(request.expect("valid request").method, "ping");
        let error = parse_json_rpc_request(json!({"jsonrpc": "1.0", "method": "ping"}))
            .expect_err("wrong version");
        assert_eq!(error, "Invalid JSON-RPC version");
        assert!(parse_json_rpc_request(json!({"method": "ping"})).is_err());
    }

    #[test]
    fn origin_allowlist_covers_empty_invalid_and_loopback_origins() {
        assert!(!mcp_origin_allowed("   ", &[]));
        assert!(!mcp_origin_allowed("not a url", &[]));
        assert!(!mcp_origin_allowed("ftp://localhost", &[]));
        assert!(mcp_origin_allowed("http://localhost:3000", &[]));
        assert!(mcp_origin_allowed("http://127.0.0.1", &[]));
        assert!(!mcp_origin_allowed("http://example.com", &[]));
        let allowed = vec!["https://app.example.test/".to_string()];
        assert!(mcp_origin_allowed("https://app.example.test", &allowed));
        assert!(!mcp_origin_allowed("https://other.example.test", &allowed));
    }

    #[test]
    fn protocol_version_header_accepts_known_and_rejects_future_versions() {
        let request = JsonRpcRequest {
            jsonrpc: Some("2.0".to_string()),
            method: "tools/list".to_string(),
            params: json!({}),
            id: None,
        };
        let initialize = JsonRpcRequest {
            jsonrpc: Some("2.0".to_string()),
            method: "initialize".to_string(),
            params: json!({}),
            id: None,
        };
        let mut headers = HeaderMap::new();
        assert!(validate_mcp_protocol_version(&initialize, &headers).is_ok());
        assert!(validate_mcp_protocol_version(&request, &headers).is_ok());
        headers.insert(
            MCP_PROTOCOL_VERSION_HEADER,
            HeaderValue::from_static(MCP_PROTOCOL_VERSION),
        );
        assert!(validate_mcp_protocol_version(&request, &headers).is_ok());
        headers.insert(
            MCP_PROTOCOL_VERSION_HEADER,
            HeaderValue::from_static("2099-01-01"),
        );
        assert!(validate_mcp_protocol_version(&request, &headers).is_err());
    }

    #[test]
    fn response_helpers_build_notification_success_and_error_envelopes() {
        assert_eq!(
            notification_response(Some(json!(1))).status(),
            StatusCode::OK
        );
        assert_eq!(notification_response(None).status(), StatusCode::ACCEPTED);

        let success = tool_success(json!({"ok": true}));
        assert_eq!(success["isError"], json!(false));
        assert_eq!(success["content"][0]["type"], json!("text"));
        let failure = tool_error(json!({"code": "X"}));
        assert_eq!(failure["isError"], json!(true));

        let error = jsonrpc_error(
            Some(json!(7)),
            -32601,
            "Method not found",
            Some(json!({"x": 1})),
        );
        assert_eq!(error.status(), StatusCode::OK);

        let limited = mcp_rate_limit_error(Some(json!(1)), 30);
        assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            limited
                .headers()
                .get(RETRY_AFTER)
                .and_then(|value| value.to_str().ok()),
            Some("30")
        );

        assert_eq!(
            mcp_http_error(StatusCode::BAD_REQUEST, "bad").status(),
            StatusCode::BAD_REQUEST
        );
    }
}
