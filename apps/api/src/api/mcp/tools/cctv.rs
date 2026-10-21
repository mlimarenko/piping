use super::super::*;

pub(in crate::api::mcp) async fn handle_cctv_archive_tool_call(
    state: AppState,
    audit_auth_context: Option<AuthContext>,
    request_context: RequestContext,
    id: Option<Value>,
    arguments: Value,
    delivery: ArchiveDelivery,
) -> Response {
    let (action_wire, action) = delivery.action();
    let target_id = match flat_target_id_argument(&arguments) {
        Ok(value) => value,
        Err(message) => {
            return jsonrpc_result(
                id,
                tool_error(json!({ "code": "INVALID_ARGUMENTS", "message": message })),
            );
        }
    };
    let target_uuid = match Uuid::parse_str(&target_id) {
        Ok(value) => value,
        Err(error) => {
            return jsonrpc_result(
                id,
                tool_error(json!({
                    "code": "INVALID_ARGUMENTS",
                    "message": format!("Invalid target_id: {error}")
                })),
            );
        }
    };
    let resource_id = format!("cctv:{target_id}");
    if !request_context.allows_api_token_scope_for_resource(
        action_wire,
        "cctv",
        Some(resource_id.as_str()),
    ) {
        return jsonrpc_result(
            id,
            tool_error(json!({
                "code": "TOKEN_SCOPE_DENIED",
                "message": format!("MCP CCTV archive access requires cctv:{action_wire} for the target")
            })),
        );
    }
    if ensure_target_scoped_access(&state, &request_context, target_uuid, action)
        .await
        .is_err()
    {
        return jsonrpc_result(
            id,
            tool_error(json!({
                "code": "RBAC_DENIED",
                "message": format!("Access denied: missing cctv:{action_wire} for this target")
            })),
        );
    }
    let channel = match arguments.get("channel").and_then(Value::as_u64) {
        Some(value) => match u16::try_from(value) {
            Ok(value) => value,
            Err(_) => {
                return jsonrpc_result(
                    id,
                    tool_error(json!({
                        "code": "INVALID_ARGUMENTS",
                        "message": "channel must be between 0 and 65535"
                    })),
                );
            }
        },
        None => {
            return jsonrpc_result(
                id,
                tool_error(json!({
                    "code": "INVALID_ARGUMENTS",
                    "message": "channel is required"
                })),
            );
        }
    };
    let parse_time = |key: &str| {
        arguments
            .get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{key} is required"))
            .and_then(|value| {
                chrono::DateTime::parse_from_rfc3339(value.trim())
                    .map(|value| value.with_timezone(&Utc))
                    .map_err(|error| format!("invalid {key} timestamp: {error}"))
            })
    };
    let start = match parse_time("start") {
        Ok(value) => value,
        Err(message) => {
            return jsonrpc_result(
                id,
                tool_error(json!({ "code": "INVALID_ARGUMENTS", "message": message })),
            );
        }
    };
    let end = match parse_time("end") {
        Ok(value) => value,
        Err(message) => {
            return jsonrpc_result(
                id,
                tool_error(json!({ "code": "INVALID_ARGUMENTS", "message": message })),
            );
        }
    };
    if end <= start {
        return jsonrpc_result(
            id,
            tool_error(json!({
                "code": "INVALID_ARGUMENTS",
                "message": "export window end must be after start"
            })),
        );
    }
    let max_window = max_export_window_seconds();
    if (end - start).num_seconds() > max_window {
        return jsonrpc_result(
            id,
            tool_error(json!({
                "code": "INVALID_ARGUMENTS",
                "message": format!(
                    "export window exceeds the {max_window}s maximum; request a shorter range"
                )
            })),
        );
    }
    require_mcp_tool_audit!(
        id,
        record_mcp_tool_audit(
            &state,
            audit_auth_context.as_ref(),
            &request_context,
            McpToolAudit {
                tool_name: delivery.tool_name(),
                action: action_wire,
                resource_type: "cctv",
                resource_id: Some(resource_id.as_str()),
                decision: "allowed",
                rest_status: None,
                error_code: None
            }
        )
    );
    jsonrpc_result(
        id,
        tool_success(json!({
            "target_id": target_id,
            "channel": channel,
            "start": start.to_rfc3339(),
            "end": end.to_rfc3339(),
            "download": {
                "method": "GET",
                "path": format!("/api/v1/cctv/{target_uuid}/{}", delivery.path_segment()),
                "query": {
                    "channel": channel,
                    "start": start.to_rfc3339(),
                    "end": end.to_rfc3339()
                },
                "content_type": "video/mp4",
                "authorization": "same MCP API token"
            }
        })),
    )
}

pub(in crate::api::mcp) async fn handle_cctv_snapshot_tool_call(
    state: AppState,
    audit_auth_context: Option<AuthContext>,
    request_context: RequestContext,
    id: Option<Value>,
    arguments: Value,
) -> Response {
    let target_id = match flat_target_id_argument(&arguments) {
        Ok(value) => value,
        Err(message) => {
            return jsonrpc_result(
                id,
                tool_error(json!({ "code": "INVALID_ARGUMENTS", "message": message })),
            );
        }
    };
    let target_uuid = match Uuid::parse_str(&target_id) {
        Ok(value) => value,
        Err(error) => {
            return jsonrpc_result(
                id,
                tool_error(json!({
                    "code": "INVALID_ARGUMENTS",
                    "message": format!("Invalid target_id: {error}")
                })),
            );
        }
    };
    let resource_id = format!("cctv:{target_id}");
    if !request_context.allows_api_token_scope_for_resource(
        "view",
        "cctv",
        Some(resource_id.as_str()),
    ) {
        return jsonrpc_result(
            id,
            tool_error(json!({
                "code": "TOKEN_SCOPE_DENIED",
                "message": "MCP CCTV snapshot requires cctv:view for the target"
            })),
        );
    }
    if ensure_target_scoped_access(&state, &request_context, target_uuid, Action::CctvView)
        .await
        .is_err()
    {
        return jsonrpc_result(
            id,
            tool_error(json!({
                "code": "RBAC_DENIED",
                "message": "Access denied: missing cctv:view for this target"
            })),
        );
    }
    let owner_api_token_id = match request_context.api_token_id.as_deref() {
        Some(value) => value,
        None => {
            return jsonrpc_result(
                id,
                tool_error(json!({
                    "code": "AUTHENTICATION_REQUIRED",
                    "message": "CCTV snapshot resources require an API token credential"
                })),
            );
        }
    };
    let channel = match optional_u64(&arguments, "channel") {
        Ok(value) => match value.map(u16::try_from).transpose() {
            Ok(value) => value,
            Err(_) => {
                return jsonrpc_result(
                    id,
                    tool_error(json!({
                        "code": "INVALID_ARGUMENTS",
                        "message": "channel must be between 0 and 65535"
                    })),
                );
            }
        },
        Err(error) => return jsonrpc_result(id, tool_error(error.to_value())),
    };
    let include_image = match optional_bool(&arguments, "include_image") {
        Ok(value) => value.unwrap_or(false),
        Err(error) => return jsonrpc_result(id, tool_error(error.to_value())),
    };
    let max_inline_bytes = match parse_vnc_inline_max_bytes(&arguments) {
        Ok(value) => value,
        Err(error) => return jsonrpc_result(id, tool_error(error.to_value())),
    };
    let snapshot = match build_device_management_service(&state)
        .snapshot(target_uuid, channel)
        .await
    {
        Ok(SnapshotRef::Bytes(bytes)) if bytes.len() <= MAX_CCTV_SNAPSHOT_BYTES => bytes,
        Ok(SnapshotRef::Bytes(_)) => {
            return jsonrpc_result(
                id,
                tool_error(json!({
                    "code": "PAYLOAD_TOO_LARGE",
                    "message": "Device snapshot exceeds the maximum supported size"
                })),
            );
        }
        Ok(SnapshotRef::Uri(_)) => {
            return jsonrpc_result(
                id,
                tool_error(json!({
                    "code": "UNSUPPORTED",
                    "message": "Snapshot-by-URI is not supported"
                })),
            );
        }
        Err(error) => {
            log_error_event(
                "mcp.cctv.snapshot.failed",
                "mcp_cctv_snapshot_failed",
                error.to_string(),
                EventFields::default()
                    .with_operation("mcp.cctv.snapshot")
                    .with_resource("cctv", target_id.as_str())
                    .with_message("CCTV snapshot failed"),
            );
            return jsonrpc_result(
                id,
                tool_error(json!({
                    "code": "CCTV_SNAPSHOT_FAILED",
                    "message": "Unable to capture a snapshot from this camera"
                })),
            );
        }
    };
    let capture_id = Uuid::new_v4().to_string();
    let resource_uri =
        format!("{MCP_RESOURCE_CONNECTION_TARGET_PREFIX}{target_id}/cctv-snapshot/{capture_id}");
    let created_at = Utc::now();
    let expires_at = created_at + ChronoDuration::seconds(MCP_VNC_CAPTURE_TTL_CHRONO_SECONDS);
    let image_base64 = STANDARD.encode(&snapshot);
    let bytes = snapshot.len();
    let inline_image_included = include_image && bytes <= max_inline_bytes;
    let blob = crate::storage::cache::mcp_capture::McpCaptureBlob {
        capture_id: capture_id.clone(),
        target_id: target_id.clone(),
        session_id: "cctv-snapshot".to_string(),
        connection_id: channel.map_or_else(|| "default".to_string(), |value| value.to_string()),
        owner_api_token_id: owner_api_token_id.to_string(),
        owner_user_id: request_context.user_id.map(|user_id| user_id.to_string()),
        format: "jpg".to_string(),
        mime_type: "image/jpeg".to_string(),
        width: 0,
        height: 0,
        bytes,
        image_base64: image_base64.clone(),
        created_at: created_at.to_rfc3339(),
        expires_at: expires_at.to_rfc3339(),
    };
    let mut redis = state.redis.clone();
    if let Err(error) =
        crate::storage::cache::mcp_capture::put(&mut redis, &blob, MCP_VNC_CAPTURE_TTL_SECONDS)
            .await
    {
        return jsonrpc_result(
            id,
            tool_error(json!({ "code": "CAPTURE_STORE_FAILED", "message": error.to_string() })),
        );
    }
    require_mcp_tool_audit!(
        id,
        record_mcp_tool_audit(
            &state,
            audit_auth_context.as_ref(),
            &request_context,
            McpToolAudit {
                tool_name: CCTV_SNAPSHOT_TOOL_NAME,
                action: "view",
                resource_type: "cctv",
                resource_id: Some(resource_id.as_str()),
                decision: "allowed",
                rest_status: None,
                error_code: None
            }
        )
    );
    let metadata = json!({
        "target_id": target_id,
        "channel": channel,
        "capture_id": capture_id,
        "resource_uri": resource_uri,
        "expires_at": blob.expires_at,
        "mime_type": blob.mime_type,
        "bytes": bytes,
        "inline_image_included": inline_image_included
    });
    jsonrpc_result(
        id,
        vnc_capture_tool_success(VncCaptureToolResult {
            metadata,
            resource_uri,
            resource_name: format!("cctv-snapshot-{}.jpg", blob.capture_id),
            resource_title: "CCTV camera snapshot".to_string(),
            mime_type: blob.mime_type,
            inline_image_base64: inline_image_included.then_some(image_base64),
        }),
    )
}

#[cfg(test)]
mod tests {
    use crate::api::mcp::test_support::*;

    #[test]
    fn cctv_capabilities_manifest_is_visible_for_any_cctv_scope_only() {
        for action in CCTV_SCOPE_ACTIONS {
            let endpoints = visible_scoped_rest_endpoints(&context(action, "cctv", "*"));
            assert!(
                endpoints
                    .iter()
                    .any(|endpoint| endpoint.operation_id == CCTV_CAPABILITIES_OPERATION_ID),
                "{action}:cctv must expose the capabilities manifest"
            );
        }
        let endpoints = visible_scoped_rest_endpoints(&context("read", "connection_target", "*"));
        assert!(
            endpoints
                .iter()
                .all(|endpoint| endpoint.operation_id != CCTV_CAPABILITIES_OPERATION_ID),
            "a broad read:connection_target must not expose CCTV"
        );
    }

    #[test]
    fn cctv_snapshot_tool_requires_only_namespaced_view_scope() {
        let tools = visible_tools(&context("view", "cctv", "*"));
        assert!(tools.iter().any(|tool| {
            tool.get("name").and_then(serde_json::Value::as_str) == Some(CCTV_SNAPSHOT_TOOL_NAME)
        }));

        for (action, resource_type) in [
            ("read", "connection_target"),
            ("recordings", "cctv"),
            ("export", "cctv"),
            ("users", "cctv"),
            ("settings", "cctv"),
            ("ptz", "cctv"),
        ] {
            let tools = visible_tools(&context(action, resource_type, "*"));
            assert!(tools.iter().all(|tool| {
                tool.get("name").and_then(serde_json::Value::as_str)
                    != Some(CCTV_SNAPSHOT_TOOL_NAME)
            }));
        }
    }

    #[test]
    fn cctv_export_tool_requires_only_namespaced_export_scope() {
        let tools = visible_tools(&context("export", "cctv", "*"));
        assert!(tools.iter().any(|tool| {
            tool.get("name").and_then(serde_json::Value::as_str) == Some(CCTV_EXPORT_TOOL_NAME)
        }));

        for (action, resource_type) in [
            ("read", "connection_target"),
            ("view", "cctv"),
            ("recordings", "cctv"),
            ("users", "cctv"),
            ("settings", "cctv"),
            ("ptz", "cctv"),
        ] {
            let tools = visible_tools(&context(action, resource_type, "*"));
            assert!(tools.iter().all(|tool| {
                tool.get("name").and_then(serde_json::Value::as_str) != Some(CCTV_EXPORT_TOOL_NAME)
            }));
        }
    }

    /// Watching recorded footage is a different grant from exporting a copy of
    /// it: a token holding only `cctv:export` must not gain playback, and one
    /// holding only `cctv:recordings` must not gain export.

    #[test]
    fn cctv_playback_tool_requires_only_namespaced_recordings_scope() {
        use super::{CCTV_EXPORT_TOOL_NAME, CCTV_PLAYBACK_TOOL_NAME};

        let tools = visible_tools(&context("recordings", "cctv", "*"));
        assert!(tools.iter().any(|tool| {
            tool.get("name").and_then(serde_json::Value::as_str) == Some(CCTV_PLAYBACK_TOOL_NAME)
        }));
        assert!(tools.iter().all(|tool| {
            tool.get("name").and_then(serde_json::Value::as_str) != Some(CCTV_EXPORT_TOOL_NAME)
        }));

        for (action, resource_type) in [
            ("read", "connection_target"),
            ("view", "cctv"),
            ("export", "cctv"),
            ("users", "cctv"),
            ("settings", "cctv"),
            ("ptz", "cctv"),
        ] {
            let tools = visible_tools(&context(action, resource_type, "*"));
            assert!(
                tools.iter().all(|tool| {
                    tool.get("name").and_then(serde_json::Value::as_str)
                        != Some(CCTV_PLAYBACK_TOOL_NAME)
                }),
                "{action}:{resource_type} must not expose the playback tool"
            );
        }
    }

    /// The two archive tools share one implementation, so their action mapping
    /// is the only thing keeping them apart. Pin it.

    #[test]
    fn archive_delivery_maps_each_tool_to_its_own_action_and_path() {
        use super::{ArchiveDelivery, CCTV_EXPORT_TOOL_NAME, CCTV_PLAYBACK_TOOL_NAME};

        assert_eq!(
            ArchiveDelivery::Watch.action(),
            ("recordings", Action::CctvRecordings)
        );
        assert_eq!(
            ArchiveDelivery::Export.action(),
            ("export", Action::CctvExport)
        );
        assert_eq!(ArchiveDelivery::Watch.path_segment(), "playback");
        assert_eq!(ArchiveDelivery::Export.path_segment(), "export");
        assert_eq!(ArchiveDelivery::Watch.tool_name(), CCTV_PLAYBACK_TOOL_NAME);
        assert_eq!(ArchiveDelivery::Export.tool_name(), CCTV_EXPORT_TOOL_NAME);
    }

    #[test]
    fn cctv_snapshot_resource_uri_is_strict() {
        assert_eq!(
            parse_cctv_snapshot_resource_uri(
                "piping://connection_target/target/cctv-snapshot/capture"
            ),
            Some(("target", "capture"))
        );
        assert!(
            parse_cctv_snapshot_resource_uri(
                "piping://connection_target/target/cctv-snapshot/capture/extra"
            )
            .is_none()
        );
        assert!(
            parse_cctv_snapshot_resource_uri("piping://connection_target/target/capture/capture")
                .is_none()
        );
    }
}
