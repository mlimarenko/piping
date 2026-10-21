//! VNC and CCTV MCP tools, split out of `mcp.rs`.
//!
//! Moved verbatim; behaviour is unchanged.

use super::*;

pub(super) fn is_host_vnc_tool(name: &str) -> bool {
    matches!(
        name,
        HOST_VNC_OPEN_TOOL_NAME
            | HOST_VNC_CAPTURE_TOOL_NAME
            | HOST_VNC_POINTER_TOOL_NAME
            | HOST_VNC_KEY_TOOL_NAME
            | HOST_VNC_TEXT_TOOL_NAME
            | HOST_VNC_CLOSE_TOOL_NAME
    )
}
pub(super) fn cctv_media_tool_descriptors(request_context: &RequestContext) -> Vec<Value> {
    let mut tools = Vec::new();
    if request_context.has_api_token_scope("view", "cctv") {
        tools.push(json!({
            "name": CCTV_SNAPSHOT_TOOL_NAME,
            "title": "Capture CCTV camera snapshot",
            "description": "Capture a JPEG from one recorder camera and return a short-lived MCP resource_link. Requires cctv:view for the target.",
            "inputSchema": cctv_snapshot_schema()
        }));
    }
    if request_context.has_api_token_scope("recordings", "cctv") {
        tools.push(json!({
            "name": CCTV_PLAYBACK_TOOL_NAME,
            "title": "Play CCTV recorded footage",
            "description": "Resolve a playable stream for one recorder camera over an RFC3339 time window. Returns a descriptor for a streamed fragmented MP4, never JSON/base64. Requires cctv:recordings for the target — watching history, unlike exporting a copy.",
            "inputSchema": cctv_export_schema()
        }));
    }
    if request_context.has_api_token_scope("export", "cctv") {
        tools.push(json!({
            "name": CCTV_EXPORT_TOOL_NAME,
            "title": "Export CCTV archive clip",
            "description": "Export one recorder camera and RFC3339 time window. The result is a streamed binary download, never JSON/base64. Requires cctv:export for the target.",
            "inputSchema": cctv_export_schema()
        }));
    }
    tools
}
pub(super) fn host_vnc_tool_descriptors(request_context: &RequestContext) -> Vec<Value> {
    let mut tools = Vec::new();
    if has_host_visual_scope(request_context) {
        tools.push(json!({
            "name": HOST_VNC_OPEN_TOOL_NAME,
            "title": "Open VNC visual session",
            "description": "Create a VNC visual session through the embedded proxy and UI-equivalent secret resolution, returning a session_id. RDP visual observe-act is not supported by these tools. Arguments are flat: {\"target_id\":\"<uuid>\"}. Requires connect:connection_target and secret:connection_target for the target.",
            "inputSchema": host_vnc_open_schema()
        }));
        tools.push(json!({
            "name": HOST_VNC_CLOSE_TOOL_NAME,
            "title": "Close VNC visual session",
            "description": "Terminate a verified VNC session. Requires connect:connection_target and secret:connection_target for the target.",
            "inputSchema": host_vnc_close_schema()
        }));
    }
    if has_host_visual_observe_scope(request_context) {
        tools.push(json!({
            "name": HOST_VNC_CAPTURE_TOOL_NAME,
            "title": "Capture VNC screen",
            "description": "Capture one VNC framebuffer snapshot and return metadata plus an MCP resource_link. Inline image bytes are opt-in via include_image and are never placed in structuredContent. Requires observe:connection_target, connect:connection_target, and secret:connection_target for the target.",
            "inputSchema": host_vnc_capture_schema()
        }));
    }
    if has_host_visual_control_scope(request_context) {
        tools.push(json!({
            "name": HOST_VNC_POINTER_TOOL_NAME,
            "title": "Send VNC pointer event",
            "description": "Send one VNC pointer event to a verified session. Use button_mask plus release_after=true for a click in one RFB connection. Requires execute:connection_target, connect:connection_target, and secret:connection_target for the target.",
            "inputSchema": host_vnc_pointer_schema()
        }));
        tools.push(json!({
            "name": HOST_VNC_KEY_TOOL_NAME,
            "title": "Send VNC key event",
            "description": "Send VNC keysyms to a verified session. press=true sends down/up in one RFB connection; use press=false with down for modifiers. Requires execute:connection_target, connect:connection_target, and secret:connection_target for the target.",
            "inputSchema": host_vnc_key_schema()
        }));
        tools.push(json!({
            "name": HOST_VNC_TEXT_TOOL_NAME,
            "title": "Type VNC text",
            "description": "Type bounded ASCII text into a verified VNC session as key down/up events over one RFB connection. Requires execute:connection_target, connect:connection_target, and secret:connection_target for the target.",
            "inputSchema": host_vnc_text_schema()
        }));
    }
    tools
}
pub(super) async fn host_vnc_open(
    state: AppState,
    request_id: RequestId,
    auth_context: Option<AuthContext>,
    headers: HeaderMap,
    target_id: &str,
    arguments: &Value,
) -> Result<Value, HostSshToolError> {
    let actor_id = auth_context
        .as_ref()
        .and_then(|context| context.user_id)
        .ok_or_else(|| {
            HostSshToolError::new(
                "AUTHENTICATION_REQUIRED",
                "Authenticated user id is required for VNC visual tools",
            )
        })?;
    let target_uuid = parse_uuid_value(target_id, "target_id")?;
    let _proxy_state = require_embedded_proxy_state(&state)?;
    let body = host_launch_body_from_arguments(arguments)?;
    validate_vnc_open_body(&body)?;
    if let Some(protocol) = body.get("protocol").and_then(Value::as_str)
        && !protocol.eq_ignore_ascii_case("vnc")
    {
        return Err(HostSshToolError::new(
            "INVALID_ARGUMENTS",
            "piping.connection_target.vnc_open only supports protocol=vnc",
        ));
    }

    let created = create_embedded_connection_session(
        RestDispatchContext {
            state: state.clone(),
            request_id,
            auth_context,
            headers,
        },
        target_id,
        "vnc",
        body,
    )
    .await?;
    if let Err(error) = verify_created_visual_session(
        &state,
        created.session_id,
        created.connection_id,
        target_uuid,
        actor_id,
        ConnectionProtocol::Vnc,
    )
    .await
    {
        cleanup_created_ssh_session(&state, created.session_id, actor_id).await;
        return Err(error);
    }

    Ok(json!({
        "session_id": created.session_id_text,
        "connection_id": created.connection_id.to_string(),
        "target_id": target_id,
        "protocol": "vnc",
        "status": "started"
    }))
}

pub(super) fn validate_vnc_open_body(body: &Map<String, Value>) -> Result<(), HostSshToolError> {
    if body.contains_key("proxy_node_id") {
        return Err(HostSshToolError::new(
            "EXTERNAL_PROXY_UNSUPPORTED",
            "piping.connection_target.vnc_open does not accept proxy_node_id; agent-native VNC currently requires embedded proxy routing",
        ));
    }
    Ok(())
}

pub(super) async fn host_vnc_capture(
    state: AppState,
    request_context: &RequestContext,
    target_id: &str,
    arguments: &Value,
) -> Result<VncCaptureToolResult, HostSshToolError> {
    let actor_id = host_authenticated_user_id(request_context)?;
    let owner_api_token_id = request_context.api_token_id.as_deref().ok_or_else(|| {
        HostSshToolError::new(
            "AUTHENTICATION_REQUIRED",
            "VNC capture resources require an API token credential",
        )
    })?;
    let (session_id, connection_id) =
        require_vnc_session_for_target(&state, actor_id, target_id, arguments).await?;
    let format = parse_vnc_capture_format(arguments)?;
    let include_image = optional_bool(arguments, "include_image")?.unwrap_or(false);
    let max_inline_bytes = parse_vnc_inline_max_bytes(arguments)?;
    let proxy_state = require_embedded_proxy_state(&state)?;
    let capture = proxy::agent_vnc_capture(proxy_state, connection_id, format)
        .await
        .map_err(host_visual_app_error)?;
    state
        .services
        .connection
        .touch_session(session_id)
        .await
        .map_err(host_ssh_app_error)?;

    let capture_id = Uuid::new_v4().to_string();
    let resource_uri = vnc_capture_resource_uri(target_id, &capture_id);
    let created_at = Utc::now();
    let expires_at = created_at + ChronoDuration::seconds(MCP_VNC_CAPTURE_TTL_CHRONO_SECONDS);
    let image_base64 = STANDARD.encode(&capture.image_bytes);
    let bytes = capture.image_bytes.len();
    let inline_image_included = include_image && bytes <= max_inline_bytes;

    let blob = crate::storage::cache::mcp_capture::McpCaptureBlob {
        capture_id: capture_id.clone(),
        target_id: target_id.to_string(),
        session_id: session_id.to_string(),
        connection_id: connection_id.to_string(),
        owner_api_token_id: owner_api_token_id.to_string(),
        owner_user_id: request_context.user_id.map(|user_id| user_id.to_string()),
        format: capture.format.to_string(),
        mime_type: capture.mime_type.to_string(),
        width: capture.width,
        height: capture.height,
        bytes,
        image_base64: image_base64.clone(),
        created_at: created_at.to_rfc3339(),
        expires_at: expires_at.to_rfc3339(),
    };
    let mut redis = state.redis.clone();
    crate::storage::cache::mcp_capture::put(&mut redis, &blob, MCP_VNC_CAPTURE_TTL_SECONDS)
        .await
        .map_err(|error| {
            HostSshToolError::new(
                "CAPTURE_STORE_FAILED",
                format!("Failed to persist VNC capture resource: {error}"),
            )
        })?;

    let mut metadata = json!({
        "session_id": session_id.to_string(),
        "connection_id": connection_id.to_string(),
        "target_id": target_id,
        "protocol": "vnc",
        "capture_id": capture_id,
        "resource_uri": resource_uri.clone(),
        "expires_at": blob.expires_at,
        "width": capture.width,
        "height": capture.height,
        "source_width": capture.source_width,
        "source_height": capture.source_height,
        "scaled": capture.scaled,
        "mime_type": capture.mime_type,
        "format": capture.format,
        "bytes": bytes,
        "inline_image_included": inline_image_included
    });
    if include_image && !inline_image_included {
        metadata["inline_image_omitted_reason"] = json!("capture_exceeds_max_inline_bytes");
        metadata["max_inline_bytes"] = json!(max_inline_bytes);
    }

    Ok(VncCaptureToolResult {
        metadata,
        resource_uri,
        resource_name: format!("vnc-capture-{}.{}", blob.capture_id, blob.format),
        resource_title: if capture.scaled {
            format!(
                "VNC framebuffer {}x{} scaled from {}x{}",
                blob.width, blob.height, capture.source_width, capture.source_height
            )
        } else {
            format!("VNC framebuffer {}x{}", blob.width, blob.height)
        },
        mime_type: blob.mime_type,
        inline_image_base64: inline_image_included.then_some(image_base64),
    })
}

#[derive(Debug)]
pub(super) struct VncCaptureToolResult {
    pub(super) metadata: Value,
    pub(super) resource_uri: String,
    pub(super) resource_name: String,
    pub(super) resource_title: String,
    pub(super) mime_type: String,
    pub(super) inline_image_base64: Option<String>,
}

pub(super) fn vnc_capture_resource_uri(target_id: &str, capture_id: &str) -> String {
    format!("{MCP_RESOURCE_CONNECTION_TARGET_PREFIX}{target_id}/capture/{capture_id}")
}

pub(super) fn parse_vnc_inline_max_bytes(arguments: &Value) -> Result<usize, HostSshToolError> {
    let raw = optional_u64(arguments, "max_inline_bytes")?
        .unwrap_or(DEFAULT_VNC_INLINE_IMAGE_MAX_BYTES as u64);
    if raw == 0 || raw > MCP_RESPONSE_BODY_LIMIT_BYTES as u64 {
        return Err(HostSshToolError::new(
            "INVALID_ARGUMENTS",
            format!("'max_inline_bytes' must be between 1 and {MCP_RESPONSE_BODY_LIMIT_BYTES}"),
        ));
    }
    usize::try_from(raw).map_err(|_| {
        HostSshToolError::new(
            "INVALID_ARGUMENTS",
            "'max_inline_bytes' exceeds platform limits",
        )
    })
}

pub(super) async fn host_vnc_pointer(
    state: AppState,
    request_context: &RequestContext,
    target_id: &str,
    arguments: &Value,
) -> Result<Value, HostSshToolError> {
    let actor_id = host_authenticated_user_id(request_context)?;
    let (session_id, connection_id) =
        require_vnc_session_for_target(&state, actor_id, target_id, arguments).await?;
    let event = proxy::AgentVncPointerEvent {
        x: parse_u16_argument(arguments, "x")?,
        y: parse_u16_argument(arguments, "y")?,
        button_mask: parse_u8_argument(arguments, "button_mask")?,
    };
    let release_after = optional_bool(arguments, "release_after")?.unwrap_or(false);
    let proxy_state = require_embedded_proxy_state(&state)?;
    let mut events = vec![event];
    if release_after && event.button_mask != 0 {
        events.push(proxy::AgentVncPointerEvent {
            button_mask: 0,
            ..event
        });
    }
    proxy::agent_vnc_pointer_events(proxy_state, connection_id, events)
        .await
        .map_err(host_visual_app_error)?;
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
        "x": event.x,
        "y": event.y,
        "button_mask": event.button_mask,
        "release_after": release_after
    }))
}

pub(super) async fn host_vnc_key(
    state: AppState,
    request_context: &RequestContext,
    target_id: &str,
    arguments: &Value,
) -> Result<Value, HostSshToolError> {
    let actor_id = host_authenticated_user_id(request_context)?;
    let (session_id, connection_id) =
        require_vnc_session_for_target(&state, actor_id, target_id, arguments).await?;
    let keysym = parse_u32_argument(arguments, "keysym")?;
    let press = optional_bool(arguments, "press")?.unwrap_or(true);
    let proxy_state = require_embedded_proxy_state(&state)?;
    let events = if press {
        vec![
            proxy::AgentVncKeyEvent { keysym, down: true },
            proxy::AgentVncKeyEvent {
                keysym,
                down: false,
            },
        ]
    } else {
        let down = optional_bool(arguments, "down")?.unwrap_or(true);
        vec![proxy::AgentVncKeyEvent { keysym, down }]
    };
    proxy::agent_vnc_key_events(proxy_state, connection_id, events)
        .await
        .map_err(host_visual_app_error)?;
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
        "keysym": keysym,
        "press": press
    }))
}

pub(super) async fn host_vnc_text(
    state: AppState,
    request_context: &RequestContext,
    target_id: &str,
    arguments: &Value,
) -> Result<Value, HostSshToolError> {
    let actor_id = host_authenticated_user_id(request_context)?;
    let (session_id, connection_id) =
        require_vnc_session_for_target(&state, actor_id, target_id, arguments).await?;
    let text = required_string(arguments, "text")?;
    if text.len() > MAX_VNC_TEXT_BYTES {
        return Err(HostSshToolError::new(
            "INVALID_ARGUMENTS",
            "VNC text input is too large",
        ));
    }
    let proxy_state = require_embedded_proxy_state(&state)?;
    proxy::agent_vnc_text(proxy_state, connection_id, text.to_string())
        .await
        .map_err(host_visual_app_error)?;
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
        "bytes_written": text.len()
    }))
}

pub(super) async fn host_vnc_close(
    state: AppState,
    request_context: &RequestContext,
    target_id: &str,
    arguments: &Value,
) -> Result<Value, HostSshToolError> {
    let actor_id = host_authenticated_user_id(request_context)?;
    let (session_id, _) =
        require_vnc_session_for_target(&state, actor_id, target_id, arguments).await?;
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
pub(super) async fn verify_created_visual_session(
    state: &AppState,
    session_id: Uuid,
    connection_id: Uuid,
    target_id: Uuid,
    actor_id: Uuid,
    expected_protocol: ConnectionProtocol,
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
            "Created visual session does not belong to the requested target_id",
        ));
    }
    if session.user_id != actor_id {
        return Err(HostSshToolError::new(
            "SESSION_OWNER_MISMATCH",
            "Created visual session does not belong to the authenticated API token user",
        ));
    }
    if session.protocol != expected_protocol {
        return Err(HostSshToolError::new(
            "INVALID_ARGUMENTS",
            "Created visual session has an unexpected protocol",
        ));
    }
    if session.proxy_node_id.is_some() {
        return Err(HostSshToolError::new(
            "EXTERNAL_PROXY_UNSUPPORTED",
            "Agent-native visual tools currently require the embedded proxy route",
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
            "Created visual session is already closed",
        ));
    }
    Ok(())
}
pub(super) async fn require_vnc_session_for_target(
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
            "VNC session does not belong to the requested target_id",
        ));
    }
    if session.user_id != actor_id {
        return Err(HostSshToolError::new(
            "SESSION_OWNER_MISMATCH",
            "VNC session does not belong to the authenticated API token user",
        ));
    }
    match session.protocol {
        ConnectionProtocol::Vnc => {}
        ConnectionProtocol::Rdp => {
            return Err(HostSshToolError::new(
                "UNSUPPORTED_PROTOCOL",
                "RDP visual observe-act over MCP is not supported yet; use VNC or wait for a guacd capture adapter",
            ));
        }
        _ => {
            return Err(HostSshToolError::new(
                "INVALID_ARGUMENTS",
                "Agent-native visual tools only support VNC sessions",
            ));
        }
    }
    if session.status.is_terminal() {
        return Err(HostSshToolError::new(
            "SESSION_CLOSED",
            "VNC session is no longer active",
        ));
    }
    let connection_id = session.proxy_id.unwrap_or(session.id);
    Ok((session.id, connection_id))
}
pub(super) fn parse_vnc_capture_format(
    arguments: &Value,
) -> Result<proxy::AgentVncCaptureFormat, HostSshToolError> {
    match arguments.get("format") {
        Some(Value::String(value)) => match value.trim().to_ascii_lowercase().as_str() {
            "" | "png" => Ok(proxy::AgentVncCaptureFormat::Png),
            "bmp" => Ok(proxy::AgentVncCaptureFormat::Bmp),
            _ => Err(HostSshToolError::new(
                "INVALID_ARGUMENTS",
                "'format' must be 'png' or 'bmp'",
            )),
        },
        Some(Value::Null) | None => Ok(proxy::AgentVncCaptureFormat::Png),
        Some(_) => Err(HostSshToolError::new(
            "INVALID_ARGUMENTS",
            "'format' must be a string",
        )),
    }
}

pub(super) fn parse_dimension(arguments: &Value, key: &str) -> Result<u16, HostSshToolError> {
    let value = optional_u64(arguments, key)?.ok_or_else(|| {
        HostSshToolError::new(
            "INVALID_ARGUMENTS",
            format!("Missing required terminal dimension '{key}'"),
        )
    })?;
    u16::try_from(value)
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| {
            HostSshToolError::new(
                "INVALID_ARGUMENTS",
                format!("'{key}' must be between 1 and {}", u16::MAX),
            )
        })
}

pub(super) fn parse_u8_argument(arguments: &Value, key: &str) -> Result<u8, HostSshToolError> {
    let value = optional_u64(arguments, key)?.ok_or_else(|| {
        HostSshToolError::new("INVALID_ARGUMENTS", format!("Missing required '{key}'"))
    })?;
    u8::try_from(value).map_err(|_| {
        HostSshToolError::new(
            "INVALID_ARGUMENTS",
            format!("'{key}' must be between 0 and {}", u8::MAX),
        )
    })
}

pub(super) fn parse_u16_argument(arguments: &Value, key: &str) -> Result<u16, HostSshToolError> {
    let value = optional_u64(arguments, key)?.ok_or_else(|| {
        HostSshToolError::new("INVALID_ARGUMENTS", format!("Missing required '{key}'"))
    })?;
    u16::try_from(value).map_err(|_| {
        HostSshToolError::new(
            "INVALID_ARGUMENTS",
            format!("'{key}' must be between 0 and {}", u16::MAX),
        )
    })
}

pub(super) fn parse_u32_argument(arguments: &Value, key: &str) -> Result<u32, HostSshToolError> {
    let value = optional_u64(arguments, key)?.ok_or_else(|| {
        HostSshToolError::new("INVALID_ARGUMENTS", format!("Missing required '{key}'"))
    })?;
    u32::try_from(value).map_err(|_| {
        HostSshToolError::new(
            "INVALID_ARGUMENTS",
            format!("'{key}' must be between 0 and {}", u32::MAX),
        )
    })
}

pub(super) fn parse_uuid_value(value: &str, field: &str) -> Result<Uuid, HostSshToolError> {
    Uuid::parse_str(value).map_err(|error| {
        HostSshToolError::new("INVALID_ARGUMENTS", format!("Invalid {field}: {error}"))
    })
}
pub(super) fn host_visual_app_error(error: crate::errors::AppError) -> HostSshToolError {
    // See `host_ssh_app_error`: raw text classifies, `client_safe_message`
    // decides what leaves the process.
    let raw = error.to_string();
    let (code, category, retryable, retry_after_ms) = match &error {
        crate::errors::AppError::Validation(message)
            if message.contains("RDP visual observe-act") =>
        {
            return HostSshToolError::new("UNSUPPORTED_PROTOCOL", message.clone());
        }
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
            classify_visual_external_error(raw.as_str())
        }
        _ => ("HOST_VISUAL_FAILED", "visual_failed", false, None),
    };
    let message = crate::api::rest::error_mapping::client_safe_message(error);
    let mut error = HostSshToolError::new(code, message)
        .with_detail("phase", Value::String("visual_session".to_string()))
        .with_detail("category", Value::String(category.to_string()))
        .with_detail("retryable", Value::Bool(retryable));
    if let Some(retry_after_ms) = retry_after_ms {
        error = error.with_detail("retry_after_ms", Value::from(retry_after_ms));
    }
    error
}

pub(super) fn classify_visual_external_error(
    message: &str,
) -> (&'static str, &'static str, bool, Option<u64>) {
    let lower = message.to_ascii_lowercase();
    if lower.contains("timed out") || lower.contains("timeout") {
        return ("VNC_PROXY_ERROR", "vnc_timeout", true, Some(2_000));
    }
    if lower.contains("connection refused") {
        return ("VNC_PROXY_ERROR", "connection_refused", true, Some(1_000));
    }
    if lower.contains("unreachable")
        || lower.contains("no route to host")
        || lower.contains("could not resolve hostname")
    {
        return ("VNC_PROXY_ERROR", "network_unreachable", true, Some(2_000));
    }
    if lower.contains("permission denied") || lower.contains("authentication failed") {
        return ("VNC_PROXY_ERROR", "auth_failed", false, None);
    }
    ("VNC_PROXY_ERROR", "vnc_proxy_error", false, None)
}
pub(super) fn vnc_capture_tool_success(value: VncCaptureToolResult) -> Value {
    let text =
        serde_json::to_string(&value.metadata).unwrap_or_else(|_| value.metadata.to_string());
    let mime_type = value.mime_type;
    let mut content = vec![
        json!({
            "type": "text",
            "mimeType": "application/json",
            "text": text
        }),
        json!({
            "type": "resource_link",
            "uri": value.resource_uri,
            "name": value.resource_name,
            "title": value.resource_title,
            "mimeType": mime_type.clone()
        }),
    ];
    if let Some(image_base64) = value.inline_image_base64 {
        content.push(json!({
            "type": "image",
            "mimeType": mime_type,
            "data": image_base64
        }));
    }

    json!({
        "structuredContent": value.metadata,
        "content": content,
        "isError": false
    })
}
