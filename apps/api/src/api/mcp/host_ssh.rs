//! SSH terminal and curated host diagnostics MCP tools, split out of `mcp.rs`.
//!
//! Moved verbatim; behaviour is unchanged.

use super::*;

pub(super) static ANSI_ESCAPE_RE: Lazy<Regex> = Lazy::new(|| {
    #[allow(
        clippy::expect_used,
        reason = "the pattern is a compile-time constant, so a parse failure is a programmer error caught deterministically the first time this Lazy is forced, never a runtime condition an operator could hit or recover from"
    )]
    Regex::new(r"\x1B(?:\][^\x07\x1B]*(?:\x07|\x1B\\)|[@-Z\\-_]|\[[0-?]*[ -/]*[@-~])")
        .expect("valid ANSI escape matcher")
});
pub(super) static HOST_SSH_TARGET_LIMITERS: Lazy<Mutex<HashMap<String, Arc<Semaphore>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
pub(super) fn is_host_ssh_tool(name: &str) -> bool {
    matches!(
        name,
        HOST_SSH_OPEN_TOOL_NAME
            | HOST_RESOLVE_CONNECTION_TOOL_NAME
            | HOST_SSH_EXEC_TOOL_NAME
            | HOST_SSH_WRITE_TOOL_NAME
            | HOST_SSH_READ_TOOL_NAME
            | HOST_SSH_READ_UNTIL_TOOL_NAME
            | HOST_SSH_WAIT_FOR_PROMPT_TOOL_NAME
            | HOST_SSH_WAIT_FOR_QUIET_TOOL_NAME
            | HOST_SSH_TAIL_LOG_TOOL_NAME
            | HOST_INSPECT_TOOL_NAME
            | HOST_CHECK_PORT_TOOL_NAME
            | HOST_READ_FILE_TOOL_NAME
            | HOST_FIND_LOGS_TOOL_NAME
            | HOST_SSH_RESIZE_TOOL_NAME
            | HOST_SSH_CLOSE_TOOL_NAME
    )
}

pub(super) fn ssh_tool_creates_connection_session(name: &str) -> bool {
    matches!(
        name,
        HOST_SSH_OPEN_TOOL_NAME
            | HOST_SSH_EXEC_TOOL_NAME
            | HOST_SSH_TAIL_LOG_TOOL_NAME
            | HOST_INSPECT_TOOL_NAME
            | HOST_CHECK_PORT_TOOL_NAME
            | HOST_READ_FILE_TOOL_NAME
            | HOST_FIND_LOGS_TOOL_NAME
    )
}
pub(super) fn host_ssh_tool_descriptors() -> Vec<Value> {
    vec![
        json!({
            "name": HOST_SSH_OPEN_TOOL_NAME,
            "title": "Open SSH terminal",
            "description": "Open a reusable interactive SSH PTY for one connection target through the embedded proxy, returning a session_id. Use ssh_write/ssh_read (which take that flat session_id) for prompts or streaming commands. Arguments are flat: {\"target_id\":\"<uuid>\"}. Requires execute:connection_target and connect:connection_target for the target.",
            "inputSchema": host_ssh_open_schema()
        }),
        json!({
            "name": HOST_RESOLVE_CONNECTION_TOOL_NAME,
            "title": "Resolve connection target plan",
            "description": "Return a secret-free, allowlisted preview of the connection target plan before opening an SSH session. Arguments are flat: {\"target_id\":\"<uuid>\"}. Requires execute:connection_target and connect:connection_target for the target.",
            "inputSchema": host_resolve_connection_schema()
        }),
        json!({
            "name": HOST_SSH_EXEC_TOOL_NAME,
            "title": "Run SSH command",
            "description": "Run one non-interactive SSH command without a PTY and return bounded stdout/stderr after exit or timeout. Best for quick read-only diagnostics; use ssh_open for interactive flows. Arguments are flat: {\"target_id\":\"<uuid>\",\"command\":\"<shell script>\"}. Requires execute:connection_target and connect:connection_target for the target.",
            "inputSchema": host_ssh_exec_schema()
        }),
        json!({
            "name": HOST_SSH_WRITE_TOOL_NAME,
            "title": "Write SSH terminal input",
            "description": "Write text to an open SSH PTY session. Use append_newline to submit a shell command. Requires execute:connection_target and connect:connection_target for the target and verifies session ownership.",
            "inputSchema": host_ssh_write_schema()
        }),
        json!({
            "name": HOST_SSH_READ_TOOL_NAME,
            "title": "Read SSH terminal output",
            "description": "Read buffered SSH PTY output after an optional after_seq cursor. Returns text by default; set include_base64 only for binary output. Requires execute:connection_target and connect:connection_target for the target.",
            "inputSchema": host_ssh_read_schema()
        }),
        json!({
            "name": HOST_SSH_READ_UNTIL_TOOL_NAME,
            "title": "Read SSH output until match",
            "description": "Poll an open SSH PTY until output contains text or matches a regex, returning bounded accumulated output and next_after_seq. Requires execute:connection_target and connect:connection_target for the target.",
            "inputSchema": host_ssh_read_until_schema()
        }),
        json!({
            "name": HOST_SSH_WAIT_FOR_PROMPT_TOOL_NAME,
            "title": "Wait for SSH prompt",
            "description": "Poll an open SSH PTY until a shell prompt regex is seen or timeout/byte limit is reached. Use after_seq from the previous read to avoid old output. Requires execute:connection_target and connect:connection_target for the target.",
            "inputSchema": host_ssh_wait_for_prompt_schema()
        }),
        json!({
            "name": HOST_SSH_WAIT_FOR_QUIET_TOOL_NAME,
            "title": "Wait for SSH quiet output",
            "description": "Poll an open SSH PTY until no new terminal output arrives for quiet_ms or timeout is reached. Requires execute:connection_target and connect:connection_target for the target.",
            "inputSchema": host_ssh_wait_for_quiet_schema()
        }),
        json!({
            "name": HOST_SSH_TAIL_LOG_TOOL_NAME,
            "title": "Tail target log over SSH",
            "description": "Run a bounded remote tail command over one-shot SSH and return captured output. follow_ms uses timeout tail -f; this is not a live stream. Arguments are flat: {\"target_id\":\"<uuid>\",\"path\":\"<file>\"}. Requires execute:connection_target and connect:connection_target for the target.",
            "inputSchema": host_ssh_tail_log_schema()
        }),
        json!({
            "name": HOST_INSPECT_TOOL_NAME,
            "title": "Inspect target basics",
            "description": "Run a fixed, bounded target inspection command set over one-shot SSH: hostname, kernel, uptime, memory, disks, and listening ports when available. Arguments are flat: {\"target_id\":\"<uuid>\"}. Requires execute:connection_target, connect:connection_target, and secret:connection_target for the target.",
            "inputSchema": host_inspect_schema()
        }),
        json!({
            "name": HOST_CHECK_PORT_TOOL_NAME,
            "title": "Check target port",
            "description": "Probe a TCP port from the connection target using a fixed bash /dev/tcp template. Arguments are flat: {\"target_id\":\"<uuid>\",\"port\":<n>}. Requires execute:connection_target, connect:connection_target, and secret:connection_target for the target.",
            "inputSchema": host_check_port_schema()
        }),
        json!({
            "name": HOST_READ_FILE_TOOL_NAME,
            "title": "Read safe target file",
            "description": "Read the first bounded bytes of an allowlisted absolute path over one-shot SSH. Denies dot segments, SSH secret paths, and known sensitive account files. Arguments are flat: {\"target_id\":\"<uuid>\",\"path\":\"<abs path>\"}. Requires execute:connection_target, connect:connection_target, and secret:connection_target for the target.",
            "inputSchema": host_read_file_schema()
        }),
        json!({
            "name": HOST_FIND_LOGS_TOOL_NAME,
            "title": "Find target logs",
            "description": "List likely log files below an absolute directory using a fixed bounded find command. Arguments are flat: {\"target_id\":\"<uuid>\"}. Requires execute:connection_target, connect:connection_target, and secret:connection_target for the target.",
            "inputSchema": host_find_logs_schema()
        }),
        json!({
            "name": HOST_SSH_RESIZE_TOOL_NAME,
            "title": "Resize SSH terminal",
            "description": "Resize an open SSH PTY. Requires execute:connection_target and connect:connection_target for the target and verifies session ownership.",
            "inputSchema": host_ssh_resize_schema()
        }),
        json!({
            "name": HOST_SSH_CLOSE_TOOL_NAME,
            "title": "Close SSH terminal",
            "description": "Terminate an open SSH PTY session. Requires execute:connection_target and connect:connection_target for the target and verifies session ownership.",
            "inputSchema": host_ssh_close_schema()
        }),
    ]
}
#[derive(Debug)]
pub(super) struct HostSshToolError {
    pub(super) code: &'static str,
    pub(super) message: String,
    pub(super) details: Map<String, Value>,
}

impl HostSshToolError {
    pub(super) fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: Map::new(),
        }
    }

    pub(super) fn with_detail(mut self, key: impl Into<String>, value: Value) -> Self {
        self.details.insert(key.into(), value);
        self
    }

    pub(super) fn to_value(&self) -> Value {
        let mut value = json!({
            "code": self.code,
            "message": self.message
        });
        if let Some(object) = value.as_object_mut() {
            object.extend(self.details.clone());
        }
        value
    }
}

pub(super) struct CreatedEmbeddedConnectionSession {
    pub(super) session_id: Uuid,
    pub(super) session_id_text: String,
    pub(super) connection_id: Uuid,
}

pub(super) async fn create_embedded_connection_session(
    context: RestDispatchContext,
    target_id: &str,
    protocol: &str,
    mut body: Map<String, Value>,
) -> Result<CreatedEmbeddedConnectionSession, HostSshToolError> {
    let RestDispatchContext {
        state,
        request_id,
        auth_context,
        headers,
    } = context;
    body.insert("protocol".to_string(), Value::String(protocol.to_string()));
    body.insert(
        "target_id".to_string(),
        Value::String(target_id.to_string()),
    );

    let rest_body = serde_json::to_vec(&Value::Object(body))
        .map(Body::from)
        .map_err(|error| {
            HostSshToolError::new(
                "INVALID_ARGUMENTS",
                format!("Failed to serialize connection session request: {error}"),
            )
        })?;
    let auth_context = auth_context.map(|context| {
        auth_context_with_resource_override(
            context,
            "connect",
            "connection_target",
            format!("connection_target:{target_id}"),
        )
    });
    let rest_response = dispatch_rest_request(
        RestDispatchContext {
            state,
            request_id,
            auth_context,
            headers,
        },
        Method::POST,
        "/api/v1/connections/sessions".to_string(),
        rest_body,
        Some(InternalConnectionSessionCreatePolicy {
            require_embedded_proxy: true,
        }),
    )
    .await
    .map_err(|error| HostSshToolError::new("REST_DISPATCH_FAILED", error))?;

    let status = rest_status(&rest_response).unwrap_or(500);
    if !(200..=399).contains(&status) {
        return Err(HostSshToolError::new(
            "REST_DISPATCH_FAILED",
            serde_json::to_string(&rest_response).unwrap_or_else(|_| {
                format!("Connection session create failed with status {status}")
            }),
        ));
    }

    let session_id_text = rest_response
        .pointer("/body/session/id")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            HostSshToolError::new(
                "REST_DISPATCH_FAILED",
                "Connection session response did not include session.id",
            )
        })?
        .to_string();
    let session_id = parse_uuid_value(session_id_text.as_str(), "session_id")?;
    let connection_id = rest_response
        .pointer("/body/proxy/connection_id")
        .and_then(Value::as_str)
        .or_else(|| {
            rest_response
                .pointer("/body/session/proxy_id")
                .and_then(Value::as_str)
        })
        .unwrap_or(session_id_text.as_str());
    let connection_id = parse_uuid_value(connection_id, "connection_id")?;

    Ok(CreatedEmbeddedConnectionSession {
        session_id,
        session_id_text,
        connection_id,
    })
}

pub(super) async fn handle_host_session_client_credentials_tool_call(
    context: McpToolCallContext,
    arguments: Value,
) -> Response {
    let McpToolCallContext {
        state,
        request_id,
        auth_context,
        audit_auth_context,
        headers,
        request_context,
        id,
    } = context;
    if !has_host_session_client_credentials_scope(&request_context) {
        require_mcp_tool_audit!(
            id,
            record_mcp_tool_audit(
                &state,
                audit_auth_context.as_ref(),
                &request_context,
                McpToolAudit {
                    tool_name: HOST_SESSION_CLIENT_CREDENTIALS_TOOL_NAME,
                    action: "secret",
                    resource_type: "connection_target",
                    resource_id: None,
                    decision: "scope_denied",
                    rest_status: None,
                    error_code: Some("TOKEN_SCOPE_DENIED")
                }
            )
        );
        return jsonrpc_result(
            id,
            tool_error(json!({
                "code": "TOKEN_SCOPE_DENIED",
                "message": "API token scope does not expose VNC/RDP session credentials"
            })),
        );
    }

    let result = host_session_client_credentials(
        state.clone(),
        request_id,
        auth_context,
        headers,
        &request_context,
        &arguments,
    )
    .await;

    match result {
        Ok((resource_id, value)) => {
            let status = rest_status(&value);
            require_mcp_tool_audit!(
                id,
                record_mcp_tool_audit(
                    &state,
                    audit_auth_context.as_ref(),
                    &request_context,
                    McpToolAudit {
                        tool_name: HOST_SESSION_CLIENT_CREDENTIALS_TOOL_NAME,
                        action: "secret",
                        resource_type: "connection_target",
                        resource_id: Some(resource_id.as_str()),
                        decision: rest_decision(status),
                        rest_status: status,
                        error_code: None
                    }
                )
            );
            jsonrpc_result(id, tool_success(value))
        }
        Err((resource_id, error)) => {
            require_mcp_tool_audit!(
                id,
                record_mcp_tool_audit(
                    &state,
                    audit_auth_context.as_ref(),
                    &request_context,
                    McpToolAudit {
                        tool_name: HOST_SESSION_CLIENT_CREDENTIALS_TOOL_NAME,
                        action: "secret",
                        resource_type: "connection_target",
                        resource_id: resource_id.as_deref(),
                        decision: "dispatch_failed",
                        rest_status: None,
                        error_code: Some(error.code)
                    }
                )
            );
            jsonrpc_result(id, tool_error(error.to_value()))
        }
    }
}

pub(super) async fn host_session_client_credentials(
    state: AppState,
    request_id: RequestId,
    auth_context: Option<AuthContext>,
    headers: HeaderMap,
    request_context: &RequestContext,
    arguments: &Value,
) -> Result<(String, Value), (Option<String>, HostSshToolError)> {
    let target_id = required_string(arguments, "target_id")
        .map_err(|error| (None, error))?
        .to_string();
    let resource_id = format!("connection_target:{target_id}");

    if !request_context.allows_api_token_scope_for_resource(
        "connect",
        "connection_target",
        Some(resource_id.as_str()),
    ) {
        return Err((
            Some(resource_id),
            HostSshToolError::new(
                "TOKEN_SCOPE_DENIED",
                "API token scope does not permit connecting to this connection target",
            ),
        ));
    }
    if !request_context.allows_api_token_scope_for_resource(
        "secret",
        "connection_target",
        Some(resource_id.as_str()),
    ) {
        return Err((
            Some(resource_id),
            HostSshToolError::new(
                "TOKEN_SCOPE_DENIED",
                "API token scope does not permit reading session credentials for this connection target",
            ),
        ));
    }

    let actor_id = host_authenticated_user_id(request_context)
        .map_err(|error| (Some(resource_id.clone()), error))?;
    let session_id = required_string(arguments, "session_id")
        .map_err(|error| (Some(resource_id.clone()), error))?;
    let session_id = parse_uuid_value(session_id, "session_id")
        .map_err(|error| (Some(resource_id.clone()), error))?;
    verify_connection_session_for_target(&state, actor_id, &target_id, session_id)
        .await
        .map_err(|error| (Some(resource_id.clone()), error))?;

    let rest_response = dispatch_rest_request(
        RestDispatchContext {
            state,
            request_id,
            auth_context,
            headers,
        },
        Method::GET,
        format!("/api/v1/connections/sessions/{session_id}/client-credentials"),
        Body::empty(),
        None,
    )
    .await
    .map_err(|error| {
        (
            Some(resource_id.clone()),
            HostSshToolError::new("REST_DISPATCH_FAILED", error),
        )
    })?;

    Ok((resource_id, rest_response))
}

/// Which archive operation an archive tool call is performing.
///
/// Watching recorded footage and taking a copy of it are the same bytes over
/// the same media plane, but they answer to different permissions, so they are
/// separate tools sharing one implementation rather than one tool with a flag —
/// a flag would make the permission depend on an argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ArchiveDelivery {
    /// Play recorded footage in place; gated on `cctv:recordings`.
    Watch,
    /// Download recorded footage; gated on `cctv:export`.
    Export,
}

impl ArchiveDelivery {
    /// The MCP tool name for this operation.
    pub(super) fn tool_name(self) -> &'static str {
        match self {
            Self::Watch => CCTV_PLAYBACK_TOOL_NAME,
            Self::Export => CCTV_EXPORT_TOOL_NAME,
        }
    }

    /// The action this operation is authorized under, in both its wire form and
    /// its typed form, so the token-scope check and the RBAC check cannot drift.
    pub(super) fn action(self) -> (&'static str, Action) {
        match self {
            Self::Watch => ("recordings", Action::CctvRecordings),
            Self::Export => ("export", Action::CctvExport),
        }
    }

    /// REST path segment serving this operation.
    pub(super) fn path_segment(self) -> &'static str {
        match self {
            Self::Watch => "playback",
            Self::Export => "export",
        }
    }
}
/// Everything a curated SSH tool needs before it creates a session: the
/// authenticated actor, the parsed target, the embedded proxy handle and the
/// validated launch body.
struct SshLaunchContext {
    actor_id: Uuid,
    target_uuid: Uuid,
    proxy_state: Arc<proxy::ProxyState>,
    body: Map<String, Value>,
}

fn prepare_ssh_launch_context(
    state: &AppState,
    auth_context: Option<&AuthContext>,
    target_id: &str,
    arguments: &Value,
    auth_error_message: &str,
    unsupported_protocol_message: &str,
) -> Result<SshLaunchContext, HostSshToolError> {
    let actor_id = auth_context
        .and_then(|context| context.user_id)
        .ok_or_else(|| HostSshToolError::new("AUTHENTICATION_REQUIRED", auth_error_message))?;
    let target_uuid = parse_uuid_value(target_id, "target_id")?;
    let proxy_state = require_embedded_proxy_state(state)?;
    let body = host_launch_body_from_arguments(arguments)?;
    validate_ssh_open_body(&body)?;
    if let Some(protocol) = body.get("protocol").and_then(Value::as_str)
        && !protocol.eq_ignore_ascii_case("ssh")
    {
        return Err(HostSshToolError::new(
            "INVALID_ARGUMENTS",
            unsupported_protocol_message,
        ));
    }
    Ok(SshLaunchContext {
        actor_id,
        target_uuid,
        proxy_state,
        body,
    })
}

pub(super) async fn host_ssh_open(
    state: AppState,
    request_id: RequestId,
    auth_context: Option<AuthContext>,
    headers: HeaderMap,
    target_id: &str,
    arguments: &Value,
) -> Result<Value, HostSshToolError> {
    let launch = prepare_ssh_launch_context(
        &state,
        auth_context.as_ref(),
        target_id,
        arguments,
        "Authenticated user id is required for SSH terminal tools",
        "piping.connection_target.ssh_open only supports protocol=ssh",
    )?;

    let created = create_embedded_connection_session(
        RestDispatchContext {
            state: state.clone(),
            request_id,
            auth_context,
            headers,
        },
        target_id,
        "ssh",
        launch.body,
    )
    .await?;

    if let Err(error) = verify_created_ssh_session(
        &state,
        created.session_id,
        created.connection_id,
        launch.target_uuid,
        launch.actor_id,
    )
    .await
    {
        cleanup_created_ssh_session(&state, created.session_id, launch.actor_id).await;
        return Err(error);
    }

    if let Err(error) = proxy::agent_ssh_start(launch.proxy_state, created.connection_id).await {
        cleanup_created_ssh_session(&state, created.session_id, launch.actor_id).await;
        return Err(host_ssh_app_error(error));
    }

    Ok(json!({
        "session_id": created.session_id_text,
        "connection_id": created.connection_id.to_string(),
        "target_id": target_id,
        "protocol": "ssh",
        "read_after_seq": null,
        "status": "started"
    }))
}

pub(super) async fn host_ssh_exec(
    state: AppState,
    request_id: RequestId,
    auth_context: Option<AuthContext>,
    headers: HeaderMap,
    target_id: &str,
    arguments: &Value,
) -> Result<Value, HostSshToolError> {
    let command = required_string(arguments, "command")?.to_string();
    host_ssh_exec_command(
        RestDispatchContext {
            state,
            request_id,
            auth_context,
            headers,
        },
        target_id,
        arguments,
        command,
        DEFAULT_SSH_EXEC_TIMEOUT_MS,
    )
    .await
}

pub(super) async fn host_ssh_exec_command(
    context: RestDispatchContext,
    target_id: &str,
    arguments: &Value,
    command: String,
    default_timeout_ms: u64,
) -> Result<Value, HostSshToolError> {
    let launch = prepare_ssh_launch_context(
        &context.state,
        context.auth_context.as_ref(),
        target_id,
        arguments,
        "Authenticated user id is required for SSH exec tools",
        "SSH exec tools only support protocol=ssh",
    )?;
    let _permit = acquire_host_ssh_target_permit(target_id).await?;

    let timeout_ms = optional_u64(arguments, "timeout_ms")?
        .unwrap_or(default_timeout_ms)
        .clamp(1, MAX_SSH_EXEC_TIMEOUT_MS);
    let max_bytes = optional_u64(arguments, "max_bytes")?
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(DEFAULT_SSH_READ_MAX_BYTES)
        .clamp(1, MAX_SSH_READ_MAX_BYTES);
    let include_base64 = optional_bool(arguments, "include_base64")?.unwrap_or(false);
    let include_stderr = optional_bool(arguments, "include_stderr")?.unwrap_or(true);

    let created = create_embedded_connection_session(
        RestDispatchContext {
            state: context.state.clone(),
            request_id: context.request_id,
            auth_context: context.auth_context,
            headers: context.headers,
        },
        target_id,
        "ssh",
        launch.body,
    )
    .await?;

    if let Err(error) = verify_created_ssh_session(
        &context.state,
        created.session_id,
        created.connection_id,
        launch.target_uuid,
        launch.actor_id,
    )
    .await
    {
        cleanup_created_ssh_session(&context.state, created.session_id, launch.actor_id).await;
        return Err(error);
    }

    let exec_result = proxy::agent_ssh_exec(
        launch.proxy_state,
        created.connection_id,
        proxy::AgentSshExecRequest {
            command,
            timeout: Duration::from_millis(timeout_ms),
            max_bytes_per_stream: max_bytes,
            stdin: None,
        },
    )
    .await;
    cleanup_created_ssh_session(&context.state, created.session_id, launch.actor_id).await;
    let exec_result = exec_result.map_err(host_ssh_app_error)?;

    let mut response = json!({
        "session_id": created.session_id_text,
        "connection_id": created.connection_id.to_string(),
        "target_id": target_id,
        "protocol": "ssh",
        "success": exec_result.success,
        "timed_out": exec_result.timed_out,
        "exit_code": exec_result.exit_code,
        "duration_ms": exec_result.duration_ms,
        "stdout": ssh_exec_stream_value(&exec_result.stdout, include_base64),
        "cleanup": {
            "session_terminated": true
        }
    });
    if include_stderr {
        response["stderr"] = ssh_exec_stream_value(&exec_result.stderr, include_base64);
    }
    Ok(response)
}

pub(super) async fn acquire_host_ssh_target_permit(
    target_id: &str,
) -> Result<OwnedSemaphorePermit, HostSshToolError> {
    let semaphore = {
        let mut limiters = HOST_SSH_TARGET_LIMITERS.lock().await;
        limiters
            .entry(target_id.to_string())
            .or_insert_with(|| Arc::new(Semaphore::new(HOST_SSH_PER_TARGET_CONCURRENCY)))
            .clone()
    };
    semaphore.acquire_owned().await.map_err(|_| {
        HostSshToolError::new("HOST_SSH_FAILED", "SSH concurrency limiter is unavailable")
            .with_detail("phase", Value::String("rate_limit".to_string()))
            .with_detail("category", Value::String("internal".to_string()))
            .with_detail("retryable", Value::Bool(true))
            .with_detail("retry_after_ms", Value::from(250_u64))
    })
}

pub(super) fn ssh_exec_stream_value(
    stream: &proxy::AgentSshExecStream,
    include_base64: bool,
) -> Value {
    let mut value = json!({
        "text": String::from_utf8_lossy(&stream.bytes).to_string(),
        "bytes_read": stream.bytes_read,
        "bytes_returned": stream.bytes.len(),
        "truncated": stream.truncated
    });
    if include_base64 {
        value["bytes_base64"] = Value::String(STANDARD.encode(&stream.bytes));
    }
    value
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SshResponseMode {
    Full,
    Combined,
    Chunks,
    Metadata,
}

impl SshResponseMode {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Combined => "combined",
            Self::Chunks => "chunks",
            Self::Metadata => "metadata",
        }
    }
}

pub(super) fn parse_ssh_response_mode(
    arguments: &Value,
) -> Result<SshResponseMode, HostSshToolError> {
    match arguments.get("response_mode") {
        Some(Value::String(value)) => match value.trim().to_ascii_lowercase().as_str() {
            "" | "full" => Ok(SshResponseMode::Full),
            "combined" => Ok(SshResponseMode::Combined),
            "chunks" => Ok(SshResponseMode::Chunks),
            "metadata" => Ok(SshResponseMode::Metadata),
            _ => Err(HostSshToolError::new(
                "INVALID_ARGUMENTS",
                "'response_mode' must be full, combined, chunks, or metadata",
            )),
        },
        Some(Value::Null) | None => Ok(SshResponseMode::Full),
        Some(_) => Err(HostSshToolError::new(
            "INVALID_ARGUMENTS",
            "'response_mode' must be a string",
        )),
    }
}

pub(super) fn strip_ansi_codes(value: &str) -> String {
    ANSI_ESCAPE_RE.replace_all(value, "").to_string()
}

pub(super) fn shape_ssh_terminal_response(
    mut metadata: Map<String, Value>,
    mut chunks: Vec<Value>,
    mut combined_text: String,
    arguments: &Value,
) -> Result<Value, HostSshToolError> {
    let response_mode = parse_ssh_response_mode(arguments)?;
    let strip_ansi = optional_bool(arguments, "strip_ansi")?.unwrap_or(false);
    if strip_ansi {
        combined_text = strip_ansi_codes(combined_text.as_str());
        for chunk in &mut chunks {
            if let Some(text) = chunk.get("text").and_then(Value::as_str) {
                chunk["text"] = Value::String(strip_ansi_codes(text));
            }
        }
    }
    metadata.insert(
        "response_mode".to_string(),
        Value::String(response_mode.as_str().to_string()),
    );
    metadata.insert("strip_ansi".to_string(), Value::Bool(strip_ansi));
    metadata.insert("chunk_count".to_string(), Value::from(chunks.len()));
    metadata.insert(
        "combined_text_bytes".to_string(),
        Value::from(combined_text.len()),
    );

    match response_mode {
        SshResponseMode::Full => {
            metadata.insert("chunks".to_string(), Value::Array(chunks));
            metadata.insert("combined_text".to_string(), Value::String(combined_text));
        }
        SshResponseMode::Combined => {
            metadata.insert("combined_text".to_string(), Value::String(combined_text));
        }
        SshResponseMode::Chunks => {
            metadata.insert("chunks".to_string(), Value::Array(chunks));
        }
        SshResponseMode::Metadata => {}
    }
    Ok(Value::Object(metadata))
}

pub(super) fn validate_ssh_open_body(body: &Map<String, Value>) -> Result<(), HostSshToolError> {
    if body.contains_key("proxy_node_id") {
        return Err(HostSshToolError::new(
            "EXTERNAL_PROXY_UNSUPPORTED",
            "piping.connection_target.ssh_open does not accept proxy_node_id; agent-native SSH currently requires embedded proxy routing",
        ));
    }
    Ok(())
}

pub(super) async fn verify_created_ssh_session(
    state: &AppState,
    session_id: Uuid,
    connection_id: Uuid,
    target_id: Uuid,
    actor_id: Uuid,
) -> Result<(), HostSshToolError> {
    let session = state
        .services
        .connection
        .require_session(session_id)
        .await
        .map_err(host_ssh_app_error)?;
    if session.target_id != target_id {
        return Err(HostSshToolError::new(
            "SESSION_TARGET_MISMATCH",
            "Created SSH session does not belong to the requested target_id",
        ));
    }
    if session.user_id != actor_id {
        return Err(HostSshToolError::new(
            "SESSION_OWNER_MISMATCH",
            "Created SSH session does not belong to the authenticated API token user",
        ));
    }
    if session.protocol != ConnectionProtocol::Ssh {
        return Err(HostSshToolError::new(
            "INVALID_ARGUMENTS",
            "Created terminal session is not SSH",
        ));
    }
    if session.proxy_node_id.is_some() {
        return Err(HostSshToolError::new(
            "EXTERNAL_PROXY_UNSUPPORTED",
            "Agent-native SSH currently requires the embedded proxy route",
        ));
    }
    if session.proxy_id.unwrap_or(session.id) != connection_id {
        return Err(HostSshToolError::new(
            "REST_DISPATCH_FAILED",
            "Connection session response did not match the persisted proxy connection id",
        ));
    }
    if session.status.is_terminal() {
        return Err(HostSshToolError::new(
            "SESSION_CLOSED",
            "Created SSH session is already closed",
        ));
    }
    Ok(())
}

pub(super) async fn verify_connection_session_for_target(
    state: &AppState,
    actor_id: Uuid,
    target_id: &str,
    session_id: Uuid,
) -> Result<(), HostSshToolError> {
    let expected_target_id = parse_uuid_value(target_id, "target_id")?;
    let session = state
        .services
        .connection
        .require_session(session_id)
        .await
        .map_err(host_ssh_app_error)?;
    if session.target_id != expected_target_id {
        return Err(HostSshToolError::new(
            "SESSION_TARGET_MISMATCH",
            "Connection session does not belong to the requested target_id",
        ));
    }
    if session.user_id != actor_id {
        return Err(HostSshToolError::new(
            "SESSION_OWNER_MISMATCH",
            "Connection session does not belong to the authenticated API token user",
        ));
    }
    if !matches!(
        session.protocol,
        ConnectionProtocol::Rdp | ConnectionProtocol::Vnc
    ) {
        return Err(HostSshToolError::new(
            "INVALID_ARGUMENTS",
            "Session client credentials are available only for RDP or VNC sessions",
        ));
    }
    if session.status.is_terminal() {
        return Err(HostSshToolError::new(
            "SESSION_CLOSED",
            "Connection session is no longer active",
        ));
    }
    Ok(())
}

pub(super) async fn cleanup_created_ssh_session(
    state: &AppState,
    session_id: Uuid,
    actor_id: Uuid,
) {
    if let Err(error) = state
        .services
        .connection
        .terminate_session(session_id, actor_id)
        .await
    {
        log_error_event(
            "mcp.host_ssh_open.cleanup.failed",
            "mcp_ssh_session_cleanup_error",
            error.to_string(),
            EventFields::default()
                .with_operation("mcp.host_ssh_open.cleanup")
                .with_resource("connection_session", session_id.to_string())
                .with_message("Failed to terminate MCP-created SSH session after open failure"),
        );
    }
}

pub(super) async fn host_resolve_connection(
    state: AppState,
    request_context: &RequestContext,
    target_id: &str,
    arguments: &Value,
) -> Result<Value, HostSshToolError> {
    let actor_id = host_authenticated_user_id(request_context)?;
    let target_uuid = parse_uuid_value(target_id, "target_id")?;
    let body = host_launch_body_from_arguments(arguments)?;
    validate_ssh_open_body(&body)?;

    if let Some(protocol) = body.get("protocol").and_then(Value::as_str)
        && !protocol.eq_ignore_ascii_case("ssh")
    {
        return Err(HostSshToolError::new(
            "INVALID_ARGUMENTS",
            "piping.connection_target.resolve_connection only supports protocol=ssh",
        ));
    }

    let options = state
        .services
        .connection
        .resolve_launch_options(target_uuid, actor_id, &ConnectionProtocol::Ssh)
        .await
        .map_err(host_ssh_app_error)?;

    let selected_entry_id = requested_launch_entry_id(&body);
    let shared_accounts = options
        .shared_accounts
        .iter()
        .enumerate()
        .map(|(index, account)| resolve_shared_launch_account_plan(index, account))
        .collect::<Vec<_>>();
    let personal_accounts = options
        .personal_credential_sets
        .iter()
        .enumerate()
        .map(|(index, set)| resolve_personal_launch_account_plan(index, set))
        .collect::<Vec<_>>();
    let account_count = shared_accounts.len() + personal_accounts.len();
    let selected_account = selected_entry_id.as_deref().and_then(|entry_id| {
        shared_accounts
            .iter()
            .chain(personal_accounts.iter())
            .find(|account| {
                account
                    .get("entry_id")
                    .and_then(Value::as_str)
                    .is_some_and(|candidate| candidate == entry_id)
            })
            .cloned()
    });

    Ok(json!({
        "target_id": target_id,
        "protocol": "ssh",
        "dry_run": true,
        "safe_for_logging": true,
        "request": {
            "target_id": target_id,
            "template_id": body.get("template_id").cloned().unwrap_or(Value::Null),
            "entry_id": selected_entry_id.clone().map(Value::String).unwrap_or(Value::Null)
        },
        "routing_plan": {
            "mode": "embedded_proxy",
            "proxy_node_id": Value::Null,
            "external_proxy_supported": false,
            "source": "agent_native_mcp"
        },
        "launch_options": {
            "count": account_count,
            "shared_count": shared_accounts.len(),
            "personal_count": personal_accounts.len(),
            "selected_entry_id": selected_entry_id.map(Value::String).unwrap_or(Value::Null),
            "selected_account": selected_account.unwrap_or(Value::Null),
            "shared_accounts": shared_accounts,
            "personal_accounts": personal_accounts
        },
        "security": {
            "secrets_returned": false,
            "secret_values_resolved": false
        }
    }))
}

pub(super) fn requested_launch_entry_id(body: &Map<String, Value>) -> Option<String> {
    nested_launch_string(body, "metadata", "entry_id")
        .or_else(|| nested_launch_string(body, "metadata", "entryId"))
        .or_else(|| nested_launch_string(body, "params", "entry_id"))
        .or_else(|| nested_launch_string(body, "params", "entryId"))
}

pub(super) fn nested_launch_string(
    body: &Map<String, Value>,
    parent: &str,
    key: &str,
) -> Option<String> {
    body.get(parent)
        .and_then(Value::as_object)
        .and_then(|object| object.get(key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub(super) fn resolve_shared_launch_account_plan(index: usize, account: &LaunchAccount) -> Value {
    let entry_id = format!("config:connconf:ssh:{index}");
    let scope_type = account
        .scope_type
        .map(|scope| Value::String(scope.as_str().to_string()))
        .unwrap_or(Value::Null);
    let scope_id = account
        .scope_id
        .map(|id| Value::String(id.to_string()))
        .unwrap_or(Value::Null);
    json!({
        "entry_id": entry_id,
        "kind": "shared",
        "label": launch_account_label(account.params.get("ui"), account.record_name.as_str(), "SSH"),
        "is_default": false,
        "source": {
            "record_id": account.record_id.to_string(),
            "record_name": account.record_name.clone(),
            "scope_type": scope_type,
            "scope_id": scope_id
        },
        "target_plan": launch_target_plan(&account.params),
        "auth_plan": launch_auth_plan(&account.params)
    })
}

pub(super) fn resolve_personal_launch_account_plan(index: usize, set: &UserCredentialSet) -> Value {
    let entry_id = format!("user:connconf:ssh:{index}:{}", set.id);
    json!({
        "entry_id": entry_id,
        "kind": "personal",
        "label": if set.name.trim().is_empty() { "SSH" } else { set.name.as_str() },
        "is_default": set.is_default,
        "source": {
            "credential_set_id": set.id.to_string(),
            "credential_set_name": set.name.clone()
        },
        "target_plan": launch_target_plan(&set.params),
        "auth_plan": launch_auth_plan(&set.params)
    })
}

pub(super) fn launch_account_label(ui: Option<&Value>, fallback: &str, protocol: &str) -> String {
    ui.and_then(Value::as_object)
        .and_then(|ui| ui.get("label"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| {
            let fallback = fallback.trim();
            if fallback.is_empty() {
                protocol.to_string()
            } else {
                fallback.to_string()
            }
        })
}

pub(super) fn launch_target_plan(params: &Value) -> Value {
    let host = params
        .get("host")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let host_source = if host.is_some() {
        "launch_params"
    } else {
        "target_primary_ip"
    };
    let port = params.get("port").and_then(Value::as_u64);
    let timeout = params
        .get("timeout")
        .or_else(|| params.get("timeout_secs"))
        .and_then(Value::as_f64);
    let jumps = params
        .get("jump")
        .and_then(Value::as_array)
        .map(|hops| hops.iter().map(launch_jump_plan).collect::<Vec<_>>())
        .unwrap_or_default();

    json!({
        "host": host.map(Value::String).unwrap_or(Value::Null),
        "host_source": host_source,
        "port": port.map(Value::from).unwrap_or(Value::Null),
        "timeout_seconds": timeout.map(Value::from).unwrap_or(Value::Null),
        "jump_count": jumps.len(),
        "jump": jumps
    })
}

pub(super) fn launch_jump_plan(value: &Value) -> Value {
    let object = value.as_object();
    let host = object
        .and_then(|map| map.get("host"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let port = object
        .and_then(|map| map.get("port"))
        .and_then(Value::as_u64);
    let username = object
        .and_then(|map| map.get("credentials"))
        .and_then(Value::as_object)
        .and_then(|credentials| display_launch_username(credentials.get("username")));

    json!({
        "host": host.map(Value::String).unwrap_or(Value::Null),
        "port": port.map(Value::from).unwrap_or(Value::Null),
        "username": username.map(Value::String).unwrap_or(Value::Null),
        "stored_auth_available": object
            .and_then(|map| map.get("credentials"))
            .and_then(Value::as_object)
            .is_some_and(credentials_have_stored_auth)
    })
}

pub(super) fn launch_auth_plan(params: &Value) -> Value {
    let credentials = params.get("credentials").and_then(Value::as_object);
    let username =
        credentials.and_then(|credentials| display_launch_username(credentials.get("username")));
    let stored_auth_available = credentials.is_some_and(credentials_have_stored_auth);
    json!({
        "username": username.map(Value::String).unwrap_or(Value::Null),
        "stored_auth_available": stored_auth_available,
        "requires_input": !stored_auth_available
    })
}

pub(super) fn display_launch_username(value: Option<&Value>) -> Option<String> {
    let username = value?.as_str()?.trim();
    if username.is_empty() || Uuid::parse_str(username).is_ok() {
        return None;
    }
    Some(username.to_string())
}

pub(super) fn credentials_have_stored_auth(credentials: &Map<String, Value>) -> bool {
    launch_non_empty_leaf(credentials.get("password"))
        || launch_non_empty_leaf(credentials.get("private_key"))
}

pub(super) fn launch_non_empty_leaf(value: Option<&Value>) -> bool {
    match value {
        Some(Value::String(value)) => !value.trim().is_empty(),
        Some(Value::Object(object)) => {
            let unset = object.len() == 1 && object.get("unset") == Some(&Value::Bool(true));
            !object.is_empty() && !unset
        }
        Some(Value::Null) | None => false,
        Some(_) => true,
    }
}

pub(super) async fn host_ssh_write(
    state: AppState,
    request_context: &RequestContext,
    target_id: &str,
    arguments: &Value,
) -> Result<Value, HostSshToolError> {
    let actor_id = host_authenticated_user_id(request_context)?;
    let (session_id, connection_id) =
        require_ssh_session_for_target(&state, actor_id, target_id, arguments).await?;
    let mut bytes = required_string(arguments, "text")?.as_bytes().to_vec();
    if optional_bool(arguments, "append_newline")?.unwrap_or(false) {
        bytes.push(b'\r');
    }
    if bytes.is_empty() {
        return Err(HostSshToolError::new(
            "INVALID_ARGUMENTS",
            "SSH input text must not be empty",
        ));
    }
    if bytes.len() > 64 * 1024 {
        return Err(HostSshToolError::new(
            "INVALID_ARGUMENTS",
            "SSH input text is too large",
        ));
    }

    let proxy_state = require_embedded_proxy_state(&state)?;
    let bytes_written = bytes.len();
    proxy::agent_ssh_write(proxy_state, connection_id, bytes)
        .await
        .map_err(host_ssh_app_error)?;
    state
        .services
        .connection
        .touch_session(session_id)
        .await
        .map_err(host_ssh_app_error)?;

    Ok(json!({
        "session_id": session_id.to_string(),
        "connection_id": connection_id.to_string(),
        "target_id": target_id,
        "bytes_written": bytes_written
    }))
}

pub(super) async fn host_ssh_read(
    state: AppState,
    request_context: &RequestContext,
    target_id: &str,
    arguments: &Value,
) -> Result<Value, HostSshToolError> {
    let actor_id = host_authenticated_user_id(request_context)?;
    let (session_id, connection_id) =
        require_ssh_session_for_target(&state, actor_id, target_id, arguments).await?;
    let after_seq = optional_u64(arguments, "after_seq")?;
    let max_bytes = optional_u64(arguments, "max_bytes")?
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(DEFAULT_SSH_READ_MAX_BYTES)
        .clamp(1, MAX_SSH_READ_MAX_BYTES);
    let include_base64 = optional_bool(arguments, "include_base64")?.unwrap_or(false);

    let proxy_state = require_embedded_proxy_state(&state)?;
    let snapshot = proxy::agent_ssh_read(proxy_state, connection_id, after_seq, max_bytes)
        .await
        .map_err(host_ssh_app_error)?;
    let mut combined_text = String::new();
    let chunks = snapshot
        .chunks
        .into_iter()
        .map(|chunk| {
            let text = String::from_utf8_lossy(&chunk.bytes).to_string();
            combined_text.push_str(text.as_str());
            let mut chunk_value = json!({
                "seq": chunk.seq,
                "text": text
            });
            if include_base64 {
                chunk_value["bytes_base64"] = Value::String(STANDARD.encode(&chunk.bytes));
            }
            chunk_value
        })
        .collect::<Vec<_>>();
    let next_after_seq = chunks
        .last()
        .and_then(|chunk| chunk.get("seq"))
        .and_then(Value::as_u64)
        .or(after_seq)
        .or_else(|| snapshot.next_seq.checked_sub(1));

    let metadata = Map::from_iter([
        (
            "session_id".to_string(),
            Value::String(session_id.to_string()),
        ),
        (
            "connection_id".to_string(),
            Value::String(connection_id.to_string()),
        ),
        (
            "target_id".to_string(),
            Value::String(target_id.to_string()),
        ),
        (
            "after_seq".to_string(),
            after_seq.map(Value::from).unwrap_or(Value::Null),
        ),
        (
            "next_after_seq".to_string(),
            next_after_seq.map(Value::from).unwrap_or(Value::Null),
        ),
        ("next_seq".to_string(), Value::from(snapshot.next_seq)),
        (
            "history_start_seq".to_string(),
            Value::from(snapshot.history_start_seq),
        ),
        ("truncated".to_string(), Value::Bool(snapshot.truncated)),
    ]);

    shape_ssh_terminal_response(metadata, chunks, combined_text, arguments)
}

pub(super) async fn host_ssh_read_until(
    state: AppState,
    request_context: &RequestContext,
    target_id: &str,
    arguments: &Value,
) -> Result<Value, HostSshToolError> {
    let pattern = required_string(arguments, "pattern")?.to_string();
    let regex = optional_bool(arguments, "regex")?.unwrap_or(false);
    host_ssh_read_until_inner(
        state,
        request_context,
        target_id,
        arguments,
        SshMatchSpec {
            pattern,
            regex,
            strip_for_match: false,
        },
    )
    .await
}

pub(super) async fn host_ssh_wait_for_prompt(
    state: AppState,
    request_context: &RequestContext,
    target_id: &str,
    arguments: &Value,
) -> Result<Value, HostSshToolError> {
    let pattern = arguments
        .get("prompt_regex")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(r"(?m)([$#>]\s*)$");
    host_ssh_read_until_inner(
        state,
        request_context,
        target_id,
        arguments,
        SshMatchSpec {
            pattern: pattern.to_string(),
            regex: true,
            strip_for_match: true,
        },
    )
    .await
}

/// The bounded read parameters shared by the polling terminal tools.
struct SshReadLimits {
    max_bytes: usize,
    timeout_ms: u64,
    poll_ms: u64,
}

fn parse_ssh_read_limits(arguments: &Value) -> Result<SshReadLimits, HostSshToolError> {
    let max_bytes = optional_u64(arguments, "max_bytes")?
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(DEFAULT_SSH_READ_MAX_BYTES)
        .clamp(1, MAX_SSH_READ_MAX_BYTES);
    let timeout_ms = optional_u64(arguments, "timeout_ms")?
        .unwrap_or(DEFAULT_SSH_WAIT_TIMEOUT_MS)
        .clamp(1, MAX_SSH_WAIT_TIMEOUT_MS);
    let poll_ms = optional_u64(arguments, "poll_ms")?
        .unwrap_or(DEFAULT_SSH_WAIT_POLL_MS)
        .clamp(25, 5_000);
    Ok(SshReadLimits {
        max_bytes,
        timeout_ms,
        poll_ms,
    })
}

/// Bounded accumulation of terminal output read from an SSH session snapshot
/// by snapshot. It owns the byte budget, the JSON chunk list and the cursor so
/// both polling tools share one implementation of "append until the limit".
struct SshTerminalAccumulator {
    combined_bytes: Vec<u8>,
    chunks: Vec<Value>,
    after_seq: Option<u64>,
    truncated: bool,
}

impl SshTerminalAccumulator {
    fn new(after_seq: Option<u64>) -> Self {
        Self {
            combined_bytes: Vec::new(),
            chunks: Vec::new(),
            after_seq,
            truncated: false,
        }
    }

    /// Absorb one snapshot within `max_bytes`, returning whether the snapshot
    /// carried any chunk at all.
    fn absorb(
        &mut self,
        snapshot: proxy::AgentSshReadSnapshot,
        max_bytes: usize,
        include_base64: bool,
    ) -> bool {
        self.truncated = self.truncated || snapshot.truncated;
        let mut saw_output = false;
        for chunk in snapshot.chunks {
            saw_output = true;
            let available = max_bytes.saturating_sub(self.combined_bytes.len());
            if available == 0 {
                self.truncated = true;
                break;
            }
            let keep = chunk.bytes.len().min(available);
            self.combined_bytes.extend_from_slice(&chunk.bytes[..keep]);
            let text = String::from_utf8_lossy(&chunk.bytes[..keep]).to_string();
            let mut chunk_value = json!({
                "seq": chunk.seq,
                "text": text
            });
            if include_base64 {
                chunk_value["bytes_base64"] = Value::String(STANDARD.encode(&chunk.bytes[..keep]));
            }
            self.chunks.push(chunk_value);
            self.after_seq = Some(chunk.seq);
            if keep < chunk.bytes.len() {
                self.truncated = true;
                break;
            }
        }
        saw_output
    }

    fn combined_text(&self) -> String {
        String::from_utf8_lossy(&self.combined_bytes).to_string()
    }
}

fn compile_ssh_matcher(pattern: &str, regex: bool) -> Result<Option<Regex>, HostSshToolError> {
    if !regex {
        return Ok(None);
    }
    Regex::new(pattern).map(Some).map_err(|error| {
        HostSshToolError::new(
            "INVALID_ARGUMENTS",
            format!("Invalid regex pattern: {error}"),
        )
    })
}

fn ssh_output_matches(
    matcher: Option<&Regex>,
    pattern: &str,
    strip_for_match: bool,
    combined_text: &str,
) -> bool {
    let match_text = if strip_for_match {
        strip_ansi_codes(combined_text)
    } else {
        combined_text.to_string()
    };
    match matcher {
        Some(regex) => regex.is_match(match_text.as_str()),
        None => match_text.contains(pattern),
    }
}

fn ssh_poll_delay(deadline: Instant, poll_ms: u64) -> Duration {
    deadline
        .saturating_duration_since(Instant::now())
        .min(Duration::from_millis(poll_ms))
}

pub(super) async fn host_ssh_wait_for_quiet(
    state: AppState,
    request_context: &RequestContext,
    target_id: &str,
    arguments: &Value,
) -> Result<Value, HostSshToolError> {
    let actor_id = host_authenticated_user_id(request_context)?;
    let (session_id, connection_id) =
        require_ssh_session_for_target(&state, actor_id, target_id, arguments).await?;
    let initial_after_seq = optional_u64(arguments, "after_seq")?;
    let limits = parse_ssh_read_limits(arguments)?;
    let quiet_ms = optional_u64(arguments, "quiet_ms")?
        .unwrap_or(DEFAULT_SSH_QUIET_MS)
        .clamp(25, MAX_SSH_QUIET_MS);
    let include_base64 = optional_bool(arguments, "include_base64")?.unwrap_or(false);

    let proxy_state = require_embedded_proxy_state(&state)?;
    let deadline = Instant::now() + Duration::from_millis(limits.timeout_ms);
    let mut quiet_since = Instant::now();
    let mut terminal = SshTerminalAccumulator::new(initial_after_seq);

    loop {
        let remaining = limits
            .max_bytes
            .saturating_sub(terminal.combined_bytes.len())
            .max(1);
        let snapshot = proxy::agent_ssh_read(
            proxy_state.clone(),
            connection_id,
            terminal.after_seq,
            remaining,
        )
        .await
        .map_err(host_ssh_app_error)?;
        let snapshot_next_seq = snapshot.next_seq;
        let snapshot_history_start_seq = snapshot.history_start_seq;
        let saw_output = terminal.absorb(snapshot, limits.max_bytes, include_base64);

        if saw_output {
            quiet_since = Instant::now();
        }

        let quiet = quiet_since.elapsed() >= Duration::from_millis(quiet_ms);
        let timed_out = Instant::now() >= deadline;
        if quiet || timed_out || terminal.truncated {
            let combined_text = terminal.combined_text();
            let metadata = Map::from_iter([
                (
                    "session_id".to_string(),
                    Value::String(session_id.to_string()),
                ),
                (
                    "connection_id".to_string(),
                    Value::String(connection_id.to_string()),
                ),
                (
                    "target_id".to_string(),
                    Value::String(target_id.to_string()),
                ),
                (
                    "after_seq".to_string(),
                    initial_after_seq.map(Value::from).unwrap_or(Value::Null),
                ),
                (
                    "next_after_seq".to_string(),
                    terminal.after_seq.map(Value::from).unwrap_or(Value::Null),
                ),
                ("next_seq".to_string(), Value::from(snapshot_next_seq)),
                (
                    "history_start_seq".to_string(),
                    snapshot_history_start_seq
                        .map(Value::from)
                        .unwrap_or(Value::Null),
                ),
                ("quiet".to_string(), Value::Bool(quiet)),
                ("settled".to_string(), Value::Bool(quiet && !timed_out)),
                ("quiet_ms".to_string(), Value::from(quiet_ms)),
                ("timed_out".to_string(), Value::Bool(timed_out && !quiet)),
                ("truncated".to_string(), Value::Bool(terminal.truncated)),
            ]);
            return shape_ssh_terminal_response(
                metadata,
                terminal.chunks,
                combined_text,
                arguments,
            );
        }

        let sleep_for = ssh_poll_delay(deadline, limits.poll_ms);
        if !sleep_for.is_zero() {
            tokio::time::sleep(sleep_for).await;
        }
    }
}

/// The match pattern one `ssh_read_until`-family call waits for, and how the
/// accumulated terminal output is compared against it.
pub(super) struct SshMatchSpec {
    pattern: String,
    regex: bool,
    strip_for_match: bool,
}

pub(super) async fn host_ssh_read_until_inner(
    state: AppState,
    request_context: &RequestContext,
    target_id: &str,
    arguments: &Value,
    spec: SshMatchSpec,
) -> Result<Value, HostSshToolError> {
    let actor_id = host_authenticated_user_id(request_context)?;
    let (session_id, connection_id) =
        require_ssh_session_for_target(&state, actor_id, target_id, arguments).await?;
    let after_seq = optional_u64(arguments, "after_seq")?;
    let limits = parse_ssh_read_limits(arguments)?;
    let include_base64 = optional_bool(arguments, "include_base64")?.unwrap_or(false);
    let matcher = compile_ssh_matcher(spec.pattern.as_str(), spec.regex)?;

    let proxy_state = require_embedded_proxy_state(&state)?;
    let deadline = Instant::now() + Duration::from_millis(limits.timeout_ms);
    let mut terminal = SshTerminalAccumulator::new(after_seq);

    loop {
        let remaining = limits
            .max_bytes
            .saturating_sub(terminal.combined_bytes.len())
            .max(1);
        let snapshot = proxy::agent_ssh_read(
            proxy_state.clone(),
            connection_id,
            terminal.after_seq,
            remaining,
        )
        .await
        .map_err(host_ssh_app_error)?;
        let snapshot_next_seq = snapshot.next_seq;
        let snapshot_history_start_seq = snapshot.history_start_seq;
        terminal.absorb(snapshot, limits.max_bytes, include_base64);

        let combined_text = terminal.combined_text();
        let matched = ssh_output_matches(
            matcher.as_ref(),
            spec.pattern.as_str(),
            spec.strip_for_match,
            combined_text.as_str(),
        );
        if matched || terminal.truncated || Instant::now() >= deadline {
            let metadata = Map::from_iter([
                (
                    "session_id".to_string(),
                    Value::String(session_id.to_string()),
                ),
                (
                    "connection_id".to_string(),
                    Value::String(connection_id.to_string()),
                ),
                (
                    "target_id".to_string(),
                    Value::String(target_id.to_string()),
                ),
                ("pattern".to_string(), Value::String(spec.pattern.clone())),
                ("regex".to_string(), Value::Bool(spec.regex)),
                ("matched".to_string(), Value::Bool(matched)),
                (
                    "timed_out".to_string(),
                    Value::Bool(!matched && !terminal.truncated),
                ),
                ("truncated".to_string(), Value::Bool(terminal.truncated)),
                (
                    "after_seq".to_string(),
                    arguments
                        .get("after_seq")
                        .and_then(Value::as_u64)
                        .map(Value::from)
                        .unwrap_or(Value::Null),
                ),
                (
                    "next_after_seq".to_string(),
                    terminal.after_seq.map(Value::from).unwrap_or(Value::Null),
                ),
                ("next_seq".to_string(), Value::from(snapshot_next_seq)),
                (
                    "history_start_seq".to_string(),
                    Value::from(snapshot_history_start_seq),
                ),
            ]);
            return shape_ssh_terminal_response(
                metadata,
                terminal.chunks,
                combined_text,
                arguments,
            );
        }

        let sleep_for = ssh_poll_delay(deadline, limits.poll_ms);
        if sleep_for.is_zero() {
            continue;
        }
        tokio::time::sleep(sleep_for).await;
    }
}

pub(super) async fn host_ssh_tail_log(
    state: AppState,
    request_id: RequestId,
    auth_context: Option<AuthContext>,
    headers: HeaderMap,
    target_id: &str,
    arguments: &Value,
) -> Result<Value, HostSshToolError> {
    let path = required_string(arguments, "path")?;
    if path.as_bytes().contains(&0) {
        return Err(HostSshToolError::new(
            "INVALID_ARGUMENTS",
            "Log path must not contain NUL bytes",
        ));
    }
    let lines = optional_u64(arguments, "lines")?
        .unwrap_or(DEFAULT_SSH_TAIL_LINES)
        .clamp(1, MAX_SSH_TAIL_LINES);
    let follow_ms = optional_u64(arguments, "follow_ms")?
        .unwrap_or(0)
        .min(MAX_SSH_EXEC_TIMEOUT_MS);
    let quoted_path = shell_quote(path);
    let command = if follow_ms == 0 {
        format!("tail -n {lines} -- {quoted_path}")
    } else {
        let seconds = follow_ms.saturating_add(999) / 1000;
        format!("timeout {seconds}s tail -n {lines} -f -- {quoted_path}")
    };
    let default_timeout_ms = if follow_ms == 0 {
        DEFAULT_SSH_EXEC_TIMEOUT_MS
    } else {
        follow_ms.saturating_add(2_000).min(MAX_SSH_EXEC_TIMEOUT_MS)
    };
    let mut response = host_ssh_exec_command(
        RestDispatchContext {
            state,
            request_id,
            auth_context,
            headers,
        },
        target_id,
        arguments,
        command,
        default_timeout_ms,
    )
    .await?;
    response["path"] = Value::String(path.to_string());
    response["lines"] = Value::from(lines);
    response["follow_ms"] = Value::from(follow_ms);
    Ok(response)
}

pub(super) async fn host_inspect(
    state: AppState,
    request_id: RequestId,
    auth_context: Option<AuthContext>,
    headers: HeaderMap,
    target_id: &str,
    arguments: &Value,
) -> Result<Value, HostSshToolError> {
    let command = concat!(
        "printf 'HOST\\n'; (hostname -f 2>/dev/null || hostname 2>/dev/null || true); ",
        "printf '\\nKERNEL\\n'; uname -a 2>/dev/null || true; ",
        "printf '\\nUPTIME\\n'; uptime 2>/dev/null || true; ",
        "printf '\\nCPU\\n'; (nproc 2>/dev/null || grep -c '^processor' /proc/cpuinfo 2>/dev/null || true); ",
        "printf '\\nMEMORY\\n'; (free -m 2>/dev/null || cat /proc/meminfo 2>/dev/null | head -n 20 || true); ",
        "printf '\\nDISK\\n'; df -h -x tmpfs -x devtmpfs 2>/dev/null | head -n 80 || true; ",
        "printf '\\nLISTENING\\n'; (ss -tulpen 2>/dev/null || netstat -tulpen 2>/dev/null || true) | head -n 80"
    )
    .to_string();
    let mut response = host_ssh_exec_command(
        RestDispatchContext {
            state,
            request_id,
            auth_context,
            headers,
        },
        target_id,
        arguments,
        command,
        DEFAULT_SSH_EXEC_TIMEOUT_MS,
    )
    .await?;
    response["diagnostic"] = Value::String("inspect".to_string());
    Ok(response)
}

pub(super) async fn host_check_port(
    state: AppState,
    request_id: RequestId,
    auth_context: Option<AuthContext>,
    headers: HeaderMap,
    target_id: &str,
    arguments: &Value,
) -> Result<Value, HostSshToolError> {
    let host = arguments
        .get("host")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("127.0.0.1");
    validate_probe_host(host)?;
    let port = optional_u64(arguments, "port")?
        .filter(|value| (1..=65535).contains(value))
        .ok_or_else(|| HostSshToolError::new("INVALID_ARGUMENTS", "'port' must be 1..65535"))?;
    let timeout_ms = optional_u64(arguments, "probe_timeout_ms")?
        .unwrap_or(5_000)
        .clamp(100, 30_000);
    let timeout_seconds = timeout_ms.saturating_add(999) / 1000;
    let probe = format!("</dev/tcp/{host}/{port}");
    let command = format!(
        "timeout {timeout_seconds}s bash -lc {} >/dev/null 2>&1 && echo open || echo closed",
        shell_quote(probe.as_str())
    );
    let exec_response = host_ssh_exec_command(
        RestDispatchContext {
            state,
            request_id,
            auth_context,
            headers,
        },
        target_id,
        arguments,
        command,
        timeout_ms.saturating_add(2_000),
    )
    .await;
    let mut response = match exec_response {
        Ok(value) => value,
        Err(error) => {
            return Ok(json!({
                "diagnostic": "check_port",
                "target_id": target_id,
                "host": host,
                "port": port,
                "success": false,
                "phase": "connect_to_target",
                "ssh_connected": false,
                "port_open": Value::Null,
                "error": error.to_value()
            }));
        }
    };
    let port_open = response
        .pointer("/stdout/text")
        .and_then(Value::as_str)
        .is_some_and(|text| text.trim() == "open");
    response["diagnostic"] = Value::String("check_port".to_string());
    response["host"] = Value::String(host.to_string());
    response["port"] = Value::from(port);
    response["phase"] = Value::String("probe_port".to_string());
    response["ssh_connected"] = Value::Bool(true);
    response["port_open"] = Value::Bool(port_open);
    response["success"] = Value::Bool(port_open);
    Ok(response)
}

pub(super) async fn host_read_file(
    state: AppState,
    request_id: RequestId,
    auth_context: Option<AuthContext>,
    headers: HeaderMap,
    target_id: &str,
    arguments: &Value,
) -> Result<Value, HostSshToolError> {
    let path = required_string(arguments, "path")?;
    validate_safe_remote_path(path)?;
    let max_bytes = optional_u64(arguments, "max_bytes")?
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(DEFAULT_HOST_READ_FILE_BYTES)
        .clamp(1, MAX_SSH_READ_MAX_BYTES);
    let command = format!("head -c {max_bytes} -- {}", shell_quote(path));
    let mut response = host_ssh_exec_command(
        RestDispatchContext {
            state,
            request_id,
            auth_context,
            headers,
        },
        target_id,
        arguments,
        command,
        DEFAULT_SSH_EXEC_TIMEOUT_MS,
    )
    .await?;
    response["diagnostic"] = Value::String("read_file".to_string());
    response["path"] = Value::String(path.to_string());
    response["max_bytes"] = Value::from(max_bytes);
    Ok(response)
}

pub(super) async fn host_find_logs(
    state: AppState,
    request_id: RequestId,
    auth_context: Option<AuthContext>,
    headers: HeaderMap,
    target_id: &str,
    arguments: &Value,
) -> Result<Value, HostSshToolError> {
    let directory = arguments
        .get("directory")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("/var/log");
    validate_safe_remote_path(directory)?;
    let pattern = arguments
        .get("pattern")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("*.log");
    validate_find_pattern(pattern)?;
    let max_depth = optional_u64(arguments, "max_depth")?
        .unwrap_or(3)
        .clamp(1, 8);
    let limit = optional_u64(arguments, "limit")?
        .unwrap_or(DEFAULT_HOST_FIND_LOGS_LIMIT)
        .clamp(1, MAX_HOST_FIND_LOGS_LIMIT);
    let command = format!(
        "find {} -maxdepth {max_depth} -type f -name {} -printf '%p\\t%TY-%Tm-%Td %TH:%TM\\t%s bytes\\n' 2>/dev/null | sort | head -n {limit}",
        shell_quote(directory),
        shell_quote(pattern)
    );
    let mut response = host_ssh_exec_command(
        RestDispatchContext {
            state,
            request_id,
            auth_context,
            headers,
        },
        target_id,
        arguments,
        command,
        DEFAULT_SSH_EXEC_TIMEOUT_MS,
    )
    .await?;
    response["diagnostic"] = Value::String("find_logs".to_string());
    response["directory"] = Value::String(directory.to_string());
    response["pattern"] = Value::String(pattern.to_string());
    response["limit"] = Value::from(limit);
    Ok(response)
}

pub(super) fn validate_probe_host(host: &str) -> Result<(), HostSshToolError> {
    if host.len() > 253
        || host
            .chars()
            .any(|value| !(value.is_ascii_alphanumeric() || matches!(value, '.' | '-' | '_' | ':')))
    {
        return Err(HostSshToolError::new(
            "INVALID_ARGUMENTS",
            "'host' must be a DNS name, IPv4, or IPv6 literal without shell characters",
        ));
    }
    Ok(())
}

pub(super) fn validate_safe_remote_path(path: &str) -> Result<(), HostSshToolError> {
    if !path.starts_with('/') || path.as_bytes().contains(&0) || path.contains('\\') {
        return Err(HostSshToolError::new(
            "INVALID_ARGUMENTS",
            "Path must be an absolute Unix path without NUL or backslash",
        ));
    }
    if path
        .split('/')
        .any(|segment| segment == "." || segment == "..")
    {
        return Err(HostSshToolError::new(
            "INVALID_ARGUMENTS",
            "Path must not contain dot segments",
        ));
    }
    let normalized = path.trim_end_matches('/');
    let denied_exact = [
        "/etc/shadow",
        "/etc/gshadow",
        "/etc/sudoers",
        "/etc/security/opasswd",
        "/root/.ssh",
    ];
    if denied_exact.contains(&normalized)
        || normalized.contains("/.ssh/")
        || normalized.ends_with("/.ssh")
    {
        return Err(HostSshToolError::new(
            "INVALID_ARGUMENTS",
            "Path points to a known sensitive credential location",
        ));
    }
    Ok(())
}

pub(super) fn validate_find_pattern(pattern: &str) -> Result<(), HostSshToolError> {
    if pattern.len() > 128
        || pattern.as_bytes().contains(&0)
        || pattern.contains('/')
        || pattern.contains('\\')
    {
        return Err(HostSshToolError::new(
            "INVALID_ARGUMENTS",
            "Pattern must be a single file-name glob without path separators",
        ));
    }
    Ok(())
}

pub(super) fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub(super) async fn host_ssh_resize(
    state: AppState,
    request_context: &RequestContext,
    target_id: &str,
    arguments: &Value,
) -> Result<Value, HostSshToolError> {
    let actor_id = host_authenticated_user_id(request_context)?;
    let (session_id, connection_id) =
        require_ssh_session_for_target(&state, actor_id, target_id, arguments).await?;
    let cols = parse_dimension(arguments, "cols")?;
    let rows = parse_dimension(arguments, "rows")?;

    let proxy_state = require_embedded_proxy_state(&state)?;
    proxy::agent_ssh_resize(proxy_state, connection_id, cols, rows)
        .await
        .map_err(host_ssh_app_error)?;

    Ok(json!({
        "session_id": session_id.to_string(),
        "connection_id": connection_id.to_string(),
        "target_id": target_id,
        "cols": cols,
        "rows": rows
    }))
}

pub(super) async fn host_ssh_close(
    state: AppState,
    request_context: &RequestContext,
    target_id: &str,
    arguments: &Value,
) -> Result<Value, HostSshToolError> {
    let actor_id = host_authenticated_user_id(request_context)?;
    let (session_id, _) =
        require_ssh_session_for_target(&state, actor_id, target_id, arguments).await?;
    let session = state
        .services
        .connection
        .terminate_session(session_id, actor_id)
        .await
        .map_err(host_ssh_app_error)?;

    Ok(json!({
        "session_id": session.id.to_string(),
        "target_id": session.target_id.to_string(),
        "status": session.status.as_str()
    }))
}
pub(super) async fn require_ssh_session_for_target(
    state: &AppState,
    actor_id: Uuid,
    target_id: &str,
    arguments: &Value,
) -> Result<(Uuid, Uuid), HostSshToolError> {
    let session_id = parse_uuid_value(required_string(arguments, "session_id")?, "session_id")?;
    let expected_target_id = parse_uuid_value(target_id, "target_id")?;
    let session = state
        .services
        .connection
        .require_session(session_id)
        .await
        .map_err(host_ssh_app_error)?;
    if session.target_id != expected_target_id {
        return Err(HostSshToolError::new(
            "SESSION_TARGET_MISMATCH",
            "SSH session does not belong to the requested target_id",
        ));
    }
    if session.user_id != actor_id {
        return Err(HostSshToolError::new(
            "SESSION_OWNER_MISMATCH",
            "SSH session does not belong to the authenticated API token user",
        ));
    }
    if session.protocol != ConnectionProtocol::Ssh {
        return Err(HostSshToolError::new(
            "INVALID_ARGUMENTS",
            "Agent-native terminal tools only support SSH sessions",
        ));
    }
    if session.status.is_terminal() {
        return Err(HostSshToolError::new(
            "SESSION_CLOSED",
            "SSH session is no longer active",
        ));
    }
    let connection_id = session.proxy_id.unwrap_or(session.id);
    Ok((session.id, connection_id))
}
pub(super) fn host_authenticated_user_id(
    request_context: &RequestContext,
) -> Result<Uuid, HostSshToolError> {
    request_context.user_id.ok_or_else(|| {
        HostSshToolError::new(
            "AUTHENTICATION_REQUIRED",
            "Authenticated user id is required for SSH terminal tools",
        )
    })
}

pub(super) fn require_embedded_proxy_state(
    state: &AppState,
) -> Result<Arc<proxy::ProxyState>, HostSshToolError> {
    state.proxy_state.clone().ok_or_else(|| {
        HostSshToolError::new(
            "PROXY_DISABLED",
            "Agent-native SSH requires the embedded proxy to be enabled",
        )
    })
}

pub(super) fn host_ssh_target_id(_tool_name: &str, arguments: &Value) -> Result<String, String> {
    flat_target_id_argument(arguments)
}

pub(super) fn host_vnc_target_id(_tool_name: &str, arguments: &Value) -> Result<String, String> {
    flat_target_id_argument(arguments)
}

/// Every curated host tool (SSH, diagnostics, VNC) takes `target_id` as a flat
/// top-level string. There is no `body` wrapper on the public surface — the
/// launch/re-dispatch body is assembled internally from the flat arguments.
pub(super) fn flat_target_id_argument(arguments: &Value) -> Result<String, String> {
    arguments
        .get("target_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| "Missing required target_id".to_string())
}

pub(super) fn auth_context_with_resource_override(
    mut auth_context: AuthContext,
    action: &str,
    resource_type: &str,
    resource_id: String,
) -> AuthContext {
    auth_context.api_token_scope_resource_override = Some(ApiTokenScopeResourceOverride {
        action: action.to_string(),
        resource_type: resource_type.to_string(),
        resource_id,
    });
    auth_context
}

/// Assemble the connection-launch body map (the shape the create-session
/// re-dispatch and the launch/entry-id/validation helpers expect internally)
/// from the FLAT top-level arguments of a curated host tool. `target_id` is a
/// required top-level string; optional `params`/`metadata`/`protocol`/
/// `template_id` are copied through verbatim, and `proxy_node_id` is copied so
/// the open-body validators can still reject it. The public tool surface stays
/// flat — callers never send a `body` wrapper.
pub(super) fn host_launch_body_from_arguments(
    arguments: &Value,
) -> Result<Map<String, Value>, HostSshToolError> {
    let target_id = arguments
        .get("target_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            HostSshToolError::new(
                "INVALID_ARGUMENTS",
                "target_id is required as a top-level string, e.g. \
                 {\"target_id\":\"<uuid>\", ...}",
            )
        })?;
    let mut body = Map::new();
    body.insert(
        "target_id".to_string(),
        Value::String(target_id.to_string()),
    );
    for key in [
        "params",
        "metadata",
        "protocol",
        "template_id",
        "proxy_node_id",
    ] {
        if let Some(value) = arguments.get(key) {
            body.insert(key.to_string(), value.clone());
        }
    }
    Ok(body)
}

pub(super) fn required_string<'a>(
    arguments: &'a Value,
    key: &str,
) -> Result<&'a str, HostSshToolError> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            HostSshToolError::new(
                "INVALID_ARGUMENTS",
                format!("Missing required string '{key}'"),
            )
        })
}

pub(super) fn optional_bool(
    arguments: &Value,
    key: &str,
) -> Result<Option<bool>, HostSshToolError> {
    match arguments.get(key) {
        Some(Value::Bool(value)) => Ok(Some(*value)),
        Some(Value::Null) | None => Ok(None),
        Some(_) => Err(HostSshToolError::new(
            "INVALID_ARGUMENTS",
            format!("'{key}' must be a boolean"),
        )),
    }
}

pub(super) fn optional_u64(arguments: &Value, key: &str) -> Result<Option<u64>, HostSshToolError> {
    match arguments.get(key) {
        Some(Value::Number(value)) => value.as_u64().map(Some).ok_or_else(|| {
            HostSshToolError::new("INVALID_ARGUMENTS", format!("'{key}' must be a u64"))
        }),
        Some(Value::Null) | None => Ok(None),
        Some(_) => Err(HostSshToolError::new(
            "INVALID_ARGUMENTS",
            format!("'{key}' must be a u64"),
        )),
    }
}
pub(super) fn host_ssh_app_error(error: crate::errors::AppError) -> HostSshToolError {
    // The raw text classifies the failure server-side; it is never what the
    // client is told. `client_safe_message` applies the same redaction the REST
    // envelope does, so a `Database` failure reaches an MCP caller as "Database
    // error" instead of the sqlx text naming tables and constraints.
    let raw = error.to_string();
    let (code, category, retryable, retry_after_ms) = match &error {
        crate::errors::AppError::Auth(_) => ("AUTHENTICATION_REQUIRED", "auth_failed", false, None),
        crate::errors::AppError::Forbidden(_) => ("FORBIDDEN", "permission_denied", false, None),
        crate::errors::AppError::NotFound(_) => ("NOT_FOUND", "not_found", false, None),
        crate::errors::AppError::Validation(_)
        | crate::errors::AppError::ValidationJson(_)
        | crate::errors::AppError::ValidationUuid(_) => {
            ("INVALID_ARGUMENTS", "invalid_arguments", false, None)
        }
        crate::errors::AppError::Conflict(_) => ("CONFLICT", "conflict", false, None),
        crate::errors::AppError::PayloadTooLarge(_) => {
            ("PAYLOAD_TOO_LARGE", "payload_too_large", false, None)
        }
        crate::errors::AppError::TooManyRequests(_) => {
            ("TOO_MANY_REQUESTS", "rate_limited", true, Some(1_000))
        }
        crate::errors::AppError::External(_) | crate::errors::AppError::ExternalSource(_) => {
            classify_ssh_external_error(raw.as_str())
        }
        _ => ("HOST_SSH_FAILED", "ssh_failed", false, None),
    };
    let message = crate::api::rest::error_mapping::client_safe_message(error);
    let mut error = HostSshToolError::new(code, message)
        .with_detail("phase", Value::String("connect_to_target".to_string()))
        .with_detail("category", Value::String(category.to_string()))
        .with_detail("retryable", Value::Bool(retryable));
    if let Some(retry_after_ms) = retry_after_ms {
        error = error.with_detail("retry_after_ms", Value::from(retry_after_ms));
    }
    error
}

pub(super) fn classify_ssh_external_error(
    message: &str,
) -> (&'static str, &'static str, bool, Option<u64>) {
    let lower = message.to_ascii_lowercase();
    if lower.contains("timed out") || lower.contains("timeout") {
        return ("SSH_PROXY_ERROR", "connect_timeout", true, Some(1_000));
    }
    if lower.contains("connection refused") {
        return ("SSH_PROXY_ERROR", "connection_refused", true, Some(1_000));
    }
    if lower.contains("unreachable")
        || lower.contains("no route to host")
        || lower.contains("could not resolve hostname")
    {
        return ("SSH_PROXY_ERROR", "network_unreachable", true, Some(2_000));
    }
    if lower.contains("permission denied") || lower.contains("authentication failed") {
        return ("SSH_PROXY_ERROR", "auth_failed", false, None);
    }
    ("SSH_PROXY_ERROR", "ssh_proxy_error", false, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::{AgentSshReadChunk, AgentSshReadSnapshot};

    fn chunk(seq: u64, bytes: &[u8]) -> AgentSshReadChunk {
        AgentSshReadChunk {
            seq,
            bytes: bytes.to_vec(),
        }
    }

    fn snapshot(
        chunks: Vec<AgentSshReadChunk>,
        next_seq: u64,
        history_start_seq: Option<u64>,
        truncated: bool,
    ) -> AgentSshReadSnapshot {
        AgentSshReadSnapshot {
            chunks,
            next_seq,
            history_start_seq,
            truncated,
        }
    }

    #[test]
    fn accumulator_truncates_at_the_byte_budget() {
        let mut accumulator = SshTerminalAccumulator::new(Some(4));
        let saw_output = accumulator.absorb(
            snapshot(vec![chunk(5, b"abc"), chunk(6, b"def")], 7, None, false),
            5,
            false,
        );
        assert!(saw_output);
        assert_eq!(accumulator.after_seq, Some(6));
        assert_eq!(accumulator.combined_text(), "abcde");
        assert!(accumulator.truncated);
        assert_eq!(accumulator.chunks.len(), 2);
        assert_eq!(accumulator.chunks[0]["seq"], json!(5));
        assert_eq!(accumulator.chunks[0]["text"], json!("abc"));
        assert_eq!(accumulator.chunks[1]["text"], json!("de"));
    }

    #[test]
    fn accumulator_stops_when_the_budget_is_already_full() {
        let mut accumulator = SshTerminalAccumulator::new(None);
        accumulator.absorb(snapshot(vec![chunk(1, b"abcd")], 2, None, false), 4, false);
        let saw_output =
            accumulator.absorb(snapshot(vec![chunk(2, b"e")], 3, None, false), 4, false);
        assert!(saw_output);
        assert!(accumulator.truncated);
        assert_eq!(accumulator.combined_text(), "abcd");
        assert_eq!(accumulator.after_seq, Some(1));
        assert_eq!(accumulator.chunks.len(), 1);
    }

    #[test]
    fn accumulator_carries_base64_and_snapshot_truncation() {
        let mut accumulator = SshTerminalAccumulator::new(None);
        accumulator.absorb(snapshot(vec![chunk(9, b"hi")], 10, Some(3), true), 16, true);
        assert!(accumulator.truncated, "snapshot truncation is sticky");
        assert_eq!(accumulator.chunks[0]["bytes_base64"], json!("aGk="));
        assert_eq!(accumulator.chunks[0]["text"], json!("hi"));
    }

    #[test]
    fn read_limits_default_and_clamp() {
        let defaults = parse_ssh_read_limits(&json!({})).expect("defaults");
        assert_eq!(defaults.max_bytes, DEFAULT_SSH_READ_MAX_BYTES);
        assert_eq!(defaults.timeout_ms, DEFAULT_SSH_WAIT_TIMEOUT_MS);
        assert_eq!(defaults.poll_ms, DEFAULT_SSH_WAIT_POLL_MS);

        let clamped = parse_ssh_read_limits(&json!({
            "max_bytes": 0,
            "timeout_ms": 0,
            "poll_ms": 1
        }))
        .expect("clamped");
        assert_eq!(clamped.max_bytes, 1);
        assert_eq!(clamped.timeout_ms, 1);
        assert_eq!(clamped.poll_ms, 25);

        let large =
            parse_ssh_read_limits(&json!({"max_bytes": 10_000_000_000u64})).expect("large bound");
        assert_eq!(large.max_bytes, MAX_SSH_READ_MAX_BYTES);
        assert!(parse_ssh_read_limits(&json!({"poll_ms": "soon"})).is_err());
    }

    #[test]
    fn matcher_compiles_only_for_regex_requests() {
        assert!(
            compile_ssh_matcher("plain", false)
                .expect("plain")
                .is_none()
        );
        assert!(compile_ssh_matcher("^ab+$", true).expect("regex").is_some());
        let error = compile_ssh_matcher("(", true).expect_err("invalid regex");
        assert_eq!(error.code, "INVALID_ARGUMENTS");
    }

    #[test]
    fn output_matching_supports_plain_regex_and_ansi_stripping() {
        assert!(ssh_output_matches(None, "welcome", false, "welcome home"));
        assert!(!ssh_output_matches(None, "absent", false, "welcome home"));
        let regex = Regex::new("^done$").expect("regex");
        assert!(ssh_output_matches(Some(&regex), "", false, "done"));
        let ansi = "\u{1b}[31mrea\u{1b}[0mdy";
        assert!(ssh_output_matches(None, "ready", true, ansi));
        assert!(!ssh_output_matches(None, "ready", false, ansi));
    }

    #[test]
    fn poll_delay_is_bounded_by_poll_ms() {
        let deadline = Instant::now() + Duration::from_secs(30);
        assert_eq!(ssh_poll_delay(deadline, 25), Duration::from_millis(25));
        let past = Instant::now() - Duration::from_secs(1);
        assert!(ssh_poll_delay(past, 25).is_zero());
    }

    #[test]
    fn response_mode_parsing_covers_aliases_and_errors() {
        assert_eq!(
            parse_ssh_response_mode(&json!({})).expect("default"),
            SshResponseMode::Full
        );
        assert_eq!(
            parse_ssh_response_mode(&json!({"response_mode": "chunks"})).expect("chunks"),
            SshResponseMode::Chunks
        );
        assert!(parse_ssh_response_mode(&json!({"response_mode": "nope"})).is_err());
        assert!(parse_ssh_response_mode(&json!({"response_mode": 1})).is_err());
    }

    #[test]
    fn launch_body_copies_flat_keys_and_requires_target_id() {
        let body = host_launch_body_from_arguments(&json!({
            "target_id": "target-1",
            "protocol": "ssh",
            "params": {"a": 1},
            "proxy_node_id": "proxy-1"
        }))
        .expect("body");
        assert_eq!(body["target_id"], json!("target-1"));
        assert_eq!(body["protocol"], json!("ssh"));
        assert_eq!(body["params"], json!({"a": 1}));
        assert_eq!(body["proxy_node_id"], json!("proxy-1"));
        assert!(host_launch_body_from_arguments(&json!({})).is_err());
    }

    #[test]
    fn strip_ansi_removes_escape_sequences() {
        assert_eq!(strip_ansi_codes("\u{1b}[31mred\u{1b}[0m"), "red");
        assert_eq!(strip_ansi_codes("plain"), "plain");
    }

    #[test]
    fn response_mode_labels_cover_every_variant() {
        assert_eq!(SshResponseMode::Full.as_str(), "full");
        assert_eq!(SshResponseMode::Combined.as_str(), "combined");
        assert_eq!(SshResponseMode::Chunks.as_str(), "chunks");
        assert_eq!(SshResponseMode::Metadata.as_str(), "metadata");
    }

    #[test]
    fn exec_stream_value_reports_bytes_and_optional_base64() {
        let stream = proxy::AgentSshExecStream {
            bytes: b"hello".to_vec(),
            bytes_read: 5,
            truncated: false,
        };
        let plain = ssh_exec_stream_value(&stream, false);
        assert_eq!(plain["text"], json!("hello"));
        assert_eq!(plain["bytes_read"], json!(5));
        assert_eq!(plain["bytes_returned"], json!(5));
        assert_eq!(plain["truncated"], json!(false));
        assert!(plain.get("bytes_base64").is_none());

        let encoded = ssh_exec_stream_value(&stream, true);
        assert_eq!(encoded["bytes_base64"], json!("aGVsbG8="));
    }

    #[test]
    fn external_error_classification_maps_proxy_failures() {
        assert_eq!(
            classify_ssh_external_error("Connection timed out"),
            ("SSH_PROXY_ERROR", "connect_timeout", true, Some(1_000))
        );
        assert_eq!(
            classify_ssh_external_error("connection refused by peer"),
            ("SSH_PROXY_ERROR", "connection_refused", true, Some(1_000))
        );
        assert_eq!(
            classify_ssh_external_error("No route to host"),
            ("SSH_PROXY_ERROR", "network_unreachable", true, Some(2_000))
        );
        assert_eq!(
            classify_ssh_external_error("Permission denied (publickey)"),
            ("SSH_PROXY_ERROR", "auth_failed", false, None)
        );
        assert_eq!(
            classify_ssh_external_error("something else"),
            ("SSH_PROXY_ERROR", "ssh_proxy_error", false, None)
        );
    }

    #[test]
    fn scalar_argument_helpers_reject_wrong_types() {
        assert_eq!(optional_u64(&json!({"n": 4}), "n").expect("u64"), Some(4));
        assert_eq!(optional_u64(&json!({}), "n").expect("absent"), None);
        assert!(optional_u64(&json!({"n": "x"}), "n").is_err());
        assert!(optional_u64(&json!({"n": -1}), "n").is_err());

        assert_eq!(
            optional_bool(&json!({"b": true}), "b").expect("bool"),
            Some(true)
        );
        assert_eq!(optional_bool(&json!({}), "b").expect("absent"), None);
        assert!(optional_bool(&json!({"b": "yes"}), "b").is_err());

        assert_eq!(
            required_string(&json!({"s": " value "}), "s").expect("string"),
            "value"
        );
        assert!(required_string(&json!({"s": "   "}), "s").is_err());
        assert!(required_string(&json!({}), "s").is_err());
    }

    #[test]
    fn launch_credential_helpers_recognise_stored_auth() {
        assert_eq!(
            display_launch_username(Some(&json!(" deploy "))),
            Some("deploy".to_string())
        );
        assert_eq!(display_launch_username(Some(&json!("   "))), None);
        assert_eq!(
            display_launch_username(Some(&json!("11111111-1111-1111-1111-111111111111"))),
            None,
            "a raw uuid is not a display username"
        );
        assert_eq!(display_launch_username(None), None);

        assert!(launch_non_empty_leaf(Some(&json!("secret"))));
        assert!(!launch_non_empty_leaf(Some(&json!("  "))));
        assert!(!launch_non_empty_leaf(Some(&json!({"unset": true}))));
        assert!(launch_non_empty_leaf(Some(&json!({"password": "x"}))));
        assert!(!launch_non_empty_leaf(Some(&json!(null))));
        assert!(!launch_non_empty_leaf(None));

        let credentials = json!({"private_key": "-----BEGIN"});
        assert!(credentials_have_stored_auth(
            credentials.as_object().expect("object")
        ));
        let empty = json!({});
        assert!(!credentials_have_stored_auth(
            empty.as_object().expect("object")
        ));
    }

    #[test]
    fn launch_target_plan_summarises_host_port_timeout_and_jumps() {
        let plan = launch_target_plan(&json!({
            "host": " jump.example ",
            "port": 22,
            "timeout": 30.5,
            "jump": [{"host": "bastion", "port": 2222, "credentials": {"username": "ops"}}]
        }));
        assert_eq!(plan["host"], json!("jump.example"));
        assert_eq!(plan["host_source"], json!("launch_params"));
        assert_eq!(plan["port"], json!(22));
        assert_eq!(plan["timeout_seconds"], json!(30.5));
        assert_eq!(plan["jump_count"], json!(1));
        assert_eq!(plan["jump"][0]["host"], json!("bastion"));
        assert_eq!(plan["jump"][0]["port"], json!(2222));
        assert_eq!(plan["jump"][0]["username"], json!("ops"));
        assert_eq!(plan["jump"][0]["stored_auth_available"], json!(false));

        let fallback = launch_target_plan(&json!({}));
        assert_eq!(fallback["host"], json!(null));
        assert_eq!(fallback["host_source"], json!("target_primary_ip"));
        assert_eq!(fallback["jump_count"], json!(0));
    }

    #[test]
    fn auth_plan_requires_input_without_stored_credentials() {
        let with_credentials = launch_auth_plan(&json!({
            "credentials": {"username": "ops", "password": "secret"}
        }));
        assert_eq!(with_credentials["username"], json!("ops"));
        assert_eq!(with_credentials["stored_auth_available"], json!(true));
        assert_eq!(with_credentials["requires_input"], json!(false));

        let without = launch_auth_plan(&json!({}));
        assert_eq!(without["username"], json!(null));
        assert_eq!(without["requires_input"], json!(true));
    }

    #[test]
    fn requested_launch_entry_id_reads_nested_aliases() {
        let body = json!({"metadata": {"entry_id": " config:1 "}});
        assert_eq!(
            requested_launch_entry_id(body.as_object().expect("object")),
            Some("config:1".to_string())
        );
        let camel = json!({"params": {"entryId": "user:2"}});
        assert_eq!(
            requested_launch_entry_id(camel.as_object().expect("object")),
            Some("user:2".to_string())
        );
        let missing = json!({"metadata": {"entry_id": "  "}});
        assert_eq!(
            requested_launch_entry_id(missing.as_object().expect("object")),
            None
        );
        let body_map = json!({"params": {"entry_id": "x"}});
        assert_eq!(
            nested_launch_string(body_map.as_object().expect("object"), "params", "entry_id"),
            Some("x".to_string())
        );
        assert_eq!(
            nested_launch_string(
                body_map.as_object().expect("object"),
                "metadata",
                "entry_id"
            ),
            None
        );
    }

    #[test]
    fn launch_account_label_prefers_label_then_record_then_kind() {
        let labelled = json!({"label": " Primary "});
        assert_eq!(
            launch_account_label(Some(&labelled), "record", "SSH"),
            "Primary"
        );
        assert_eq!(launch_account_label(None, " record ", "SSH"), "record");
        assert_eq!(launch_account_label(None, "  ", "SSH"), "SSH");
        let blank = json!({"label": "   "});
        assert_eq!(
            launch_account_label(Some(&blank), "record-id", "SSH"),
            "record-id"
        );
    }
}
