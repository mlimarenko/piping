//! MCP tool JSON-schema registry, split out of `mcp.rs`.
//!
//! Moved verbatim; the schema contents are unchanged.

use super::*;

/// Attach an optional `description` to a property schema. The value is left
/// untouched when no description is documented for that tool.
fn with_optional_description(mut schema: Value, description: Option<&str>) -> Value {
    if let Some(description) = description {
        schema["description"] = Value::String(description.to_string());
    }
    schema
}

/// Insert the `id` + `path_params` property pair shared by every id-addressed
/// inventory and access-control operation schema.
fn insert_resource_id_path_params(properties: &mut Map<String, Value>, id_description: &str) {
    properties.insert(
        "id".to_string(),
        json!({
            "type": "string",
            "description": id_description
        }),
    );
    properties.insert(
        "path_params".to_string(),
        json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string"
                }
            },
            "additionalProperties": false
        }),
    );
}

/// Per-tool descriptions for the terminal output arguments every SSH read
/// schema shares; unset fields stay undocumented.
#[derive(Default)]
struct SshOutputSchemaDescriptions {
    after_seq: Option<&'static str>,
    timeout_ms: Option<&'static str>,
    poll_ms: Option<&'static str>,
    max_bytes: Option<&'static str>,
    include_base64: Option<&'static str>,
    response_mode: Option<&'static str>,
    strip_ansi: Option<&'static str>,
}

/// The property set common to `ssh_read`/`ssh_read_until`/`ssh_wait_for_prompt`/
/// `ssh_wait_for_quiet`.
fn ssh_output_schema_properties(descriptions: SshOutputSchemaDescriptions) -> Map<String, Value> {
    let mut properties = Map::new();
    properties.insert("target_id".to_string(), json!({ "type": "string" }));
    properties.insert("session_id".to_string(), json!({ "type": "string" }));
    properties.insert(
        "after_seq".to_string(),
        with_optional_description(
            json!({
                "type": "integer",
                "minimum": 0
            }),
            descriptions.after_seq,
        ),
    );
    properties.insert(
        "timeout_ms".to_string(),
        with_optional_description(
            json!({
                "type": "integer",
                "minimum": 1,
                "maximum": MAX_SSH_WAIT_TIMEOUT_MS,
                "default": DEFAULT_SSH_WAIT_TIMEOUT_MS
            }),
            descriptions.timeout_ms,
        ),
    );
    properties.insert(
        "poll_ms".to_string(),
        with_optional_description(
            json!({
                "type": "integer",
                "minimum": 25,
                "maximum": 5000,
                "default": DEFAULT_SSH_WAIT_POLL_MS
            }),
            descriptions.poll_ms,
        ),
    );
    properties.insert(
        "max_bytes".to_string(),
        with_optional_description(
            json!({
                "type": "integer",
                "minimum": 1,
                "maximum": MAX_SSH_READ_MAX_BYTES,
                "default": DEFAULT_SSH_READ_MAX_BYTES
            }),
            descriptions.max_bytes,
        ),
    );
    properties.insert(
        "include_base64".to_string(),
        with_optional_description(
            json!({
                "type": "boolean",
                "default": false
            }),
            descriptions.include_base64,
        ),
    );
    properties.insert(
        "response_mode".to_string(),
        with_optional_description(
            json!({
                "type": "string",
                "enum": ["full", "combined", "chunks", "metadata"],
                "default": "full"
            }),
            descriptions.response_mode,
        ),
    );
    properties.insert(
        "strip_ansi".to_string(),
        with_optional_description(
            json!({
                "type": "boolean",
                "default": false
            }),
            descriptions.strip_ansi,
        ),
    );
    properties
}

pub(super) fn generic_rest_call_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "method": {
                "type": "string",
                "enum": ["GET", "POST", "PUT", "PATCH", "DELETE"]
            },
            "path": {
                "type": "string",
                "description": "Absolute REST path under /api/v1, for example /api/v1/inventory/devices. Query strings are not accepted here; use the query object."
            },
            "query": {
                "type": "object",
                "description": "Optional REST query parameters.",
                "additionalProperties": true
            },
            "body": {
                "description": "Optional JSON request body."
            }
        },
        "required": ["method", "path"],
        "additionalProperties": false
    })
}

pub(super) fn scoped_rest_list_endpoints_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": 1000,
                "default": 50
            },
            "offset": {
                "type": "integer",
                "minimum": 0,
                "default": 0
            },
            "fields": {
                "type": "array",
                "items": {
                    "type": "string",
                    "enum": [
                        "operation_id",
                        "title",
                        "description",
                        "method",
                        "path_template",
                        "action",
                        "resource_type",
                        "static_resource_id",
                        "resource_id_source"
                    ]
                },
                "description": "Optional endpoint fields to return."
            },
            "summary": {
                "type": "boolean",
                "default": false
            },
            "full": {
                "type": "boolean",
                "default": false,
                "description": "Explicitly return all filtered endpoints; otherwise responses are paginated."
            },
            "action": {
                "type": "string",
                "description": "Optional action filter, for example read, create, update, delete, connect, execute, secret."
            },
            "resource_type": {
                "type": "string",
                "description": "Optional resource type filter, for example inventory, connection_target, config_context, webhook."
            },
            "operation_id": {
                "type": "string",
                "description": "Optional exact operation id filter."
            },
            "method": {
                "type": "string",
                "description": "Optional HTTP method filter."
            },
            "path_contains": {
                "type": "string",
                "description": "Optional case-sensitive substring filter for path_template."
            }
        },
        "additionalProperties": false
    })
}

pub(super) fn scoped_rest_call_endpoint_schema(request_context: &RequestContext) -> Value {
    let operation_ids = visible_scoped_rest_endpoints(request_context)
        .into_iter()
        .map(|endpoint| Value::String(endpoint.operation_id))
        .collect::<Vec<_>>();
    json!({
        "type": "object",
        "properties": {
            "operation_id": {
                "type": "string",
                "enum": operation_ids,
                "description": "Operation id returned by piping.rest.list_allowed_endpoints."
            },
            "path_params": {
                "type": "object",
                "description": "Path parameters for templated endpoints, for example {\"id\":\"...\"}. Top-level parameter aliases are also accepted.",
                "additionalProperties": {
                    "type": "string"
                }
            },
            "query": {
                "type": "object",
                "description": "Optional REST query parameters.",
                "additionalProperties": true
            },
            "body": {
                "description": "Optional JSON request body."
            }
        },
        "required": ["operation_id"],
        "additionalProperties": false
    })
}

pub(super) fn inventory_tool_schema(
    request_context: &RequestContext,
    operation: InventoryToolOperation,
) -> Value {
    let mut required = vec!["entity_type"];
    if operation.requires_id() {
        required.push("id");
    }
    if operation.requires_body() {
        required.push("body");
    }
    let mut properties = Map::new();
    properties.insert(
        "entity_type".to_string(),
        json!({
            "type": "string",
            "enum": allowed_inventory_entity_types(request_context, operation),
            "description": "Inventory REST entity path segment, for example devices, sites, or virtual_machines."
        }),
    );
    if operation.requires_id() {
        insert_resource_id_path_params(&mut properties, "Inventory resource id for the REST path.");
    }
    if operation == InventoryToolOperation::List {
        properties.insert(
            "query".to_string(),
            json!({
                "type": "object",
                "description": "Optional REST query parameters such as limit, offset, search, or filters supported by the entity endpoint.",
                "additionalProperties": true
            }),
        );
    }
    if operation.requires_body() {
        properties.insert("body".to_string(), inventory_tool_body_schema(operation));
    }

    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
}

pub(super) fn inventory_tool_body_schema(operation: InventoryToolOperation) -> Value {
    if operation == InventoryToolOperation::BulkMutate {
        return json!({
            "type": "object",
            "description": "Bulk mutation body. Execution is per-item and non-atomic; each result reports success or error independently.",
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": [
                        "delete",
                        "set_status",
                        "assign_tenant",
                        "set_role",
                        "set_site",
                        "set_cluster",
                        "set_device_type"
                    ],
                    "description": "Bulk operation. Supported entity/operation combinations are advertised in piping://inventory/entities/{entity_type}."
                },
                "items": {
                    "type": "array",
                    "minItems": 1,
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": {
                                "type": "string",
                                "description": "Inventory resource UUID."
                            }
                        },
                        "required": ["id"],
                        "additionalProperties": true
                    }
                },
                "params": {
                    "type": ["object", "null"],
                    "description": "Operation parameters, for example {\"status\":\"active\"}, {\"tenant_id\":\"...\"}, {\"role_id\":\"...\"}, {\"site_id\":\"...\"}, {\"cluster_id\":\"...\"}, or {\"device_type_id\":\"...\"}. Omit or null for delete."
                }
            },
            "required": ["operation", "items"],
            "additionalProperties": false
        });
    }

    json!({
        "description": "JSON body accepted by the underlying inventory REST endpoint."
    })
}

pub(super) fn allowed_inventory_entity_types(
    request_context: &RequestContext,
    operation: InventoryToolOperation,
) -> Vec<&'static str> {
    INVENTORY_ENTITY_ROUTES
        .iter()
        .filter(|route| inventory_operation_visible(request_context, operation, route))
        .map(|route| route.entity_type)
        .collect()
}

pub(super) fn access_control_tool_schema(
    request_context: &RequestContext,
    operation: AccessControlToolOperation,
) -> Value {
    let mut required = vec!["entity_type"];
    if operation.requires_id() {
        required.push("id");
    }
    if operation.requires_body() {
        required.push("body");
    }

    let mut properties = Map::new();
    properties.insert(
        "entity_type".to_string(),
        json!({
            "type": "string",
            "enum": allowed_access_control_entity_types(request_context, operation),
            "description": "Access-control REST entity path segment: users, groups, roles, or policies."
        }),
    );
    if operation.requires_id() {
        insert_resource_id_path_params(
            &mut properties,
            "Access-control resource id for the REST path.",
        );
    }
    if operation == AccessControlToolOperation::List {
        properties.insert(
            "query".to_string(),
            json!({
                "type": "object",
                "description": "Optional REST query parameters such as limit, offset, search, or policy filters.",
                "additionalProperties": true
            }),
        );
    }
    if operation.requires_body() {
        properties.insert(
            "body".to_string(),
            json!({
                "description": "JSON body accepted by the underlying access-control REST endpoint."
            }),
        );
    }

    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
}

pub(super) fn allowed_access_control_entity_types(
    request_context: &RequestContext,
    operation: AccessControlToolOperation,
) -> Vec<&'static str> {
    ACCESS_CONTROL_ENTITY_ROUTES
        .iter()
        .filter(|route| access_control_route_visible(request_context, operation, route))
        .map(|route| route.entity_type)
        .collect()
}

pub(super) fn list_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "query": {
                "type": "object",
                "description": "REST query parameters.",
                "additionalProperties": true
            }
        },
        "additionalProperties": false
    })
}

pub(super) fn connection_target_list_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "query": {
                "type": "object",
                "properties": {
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 1000,
                        "default": 50
                    },
                    "offset": {
                        "type": "integer",
                        "minimum": 0,
                        "default": 0
                    },
                    "search": {
                        "type": "string"
                    },
                    "name": {
                        "type": "string"
                    },
                    "status": {
                        "type": "string"
                    },
                    "address": {
                        "type": "string"
                    },
                    "site_id": {
                        "type": "string"
                    },
                    "role_id": {
                        "type": "string"
                    },
                    "target_kind": {
                        "type": "string",
                        "enum": ["device", "virtual_machine"]
                    }
                },
                "additionalProperties": false
            }
        },
        "additionalProperties": false
    })
}

pub(super) fn path_id_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": {
                "type": "string",
                "description": "Resource id for the REST path."
            },
            "path_params": {
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string"
                    }
                },
                "additionalProperties": false
            },
            "query": {
                "type": "object",
                "description": "Optional REST query parameters.",
                "additionalProperties": true
            }
        },
        "additionalProperties": false
    })
}

pub(super) fn host_ssh_open_schema() -> Value {
    json!({
        "type": "object",
        "required": ["target_id"],
        "properties": {
            "target_id": {
                "type": "string",
                "description": "Target device or virtual machine id."
            },
            "params": {
                "type": "object",
                "description": "Optional SSH launch params in the same canonical shape as create_connection_session.",
                "additionalProperties": true
            },
            "metadata": {
                "type": "object",
                "description": "Optional connection session metadata, including entryId selection.",
                "additionalProperties": true
            }
        },
        "additionalProperties": false
    })
}

pub(super) fn host_ssh_write_schema() -> Value {
    json!({
        "type": "object",
        "required": ["target_id", "session_id", "text"],
        "properties": {
            "target_id": {
                "type": "string"
            },
            "session_id": {
                "type": "string"
            },
            "text": {
                "type": "string"
            },
            "append_newline": {
                "type": "boolean",
                "default": false
            }
        },
        "additionalProperties": false
    })
}

pub(super) fn host_ssh_exec_schema() -> Value {
    json!({
        "type": "object",
        "required": ["target_id", "command"],
        "properties": {
            "target_id": {
                "type": "string",
                "description": "Target device or virtual machine id."
            },
            "command": {
                "type": "string",
                "description": "Remote shell command to execute non-interactively."
            },
            "params": {
                "type": "object",
                "description": "Optional SSH launch params in the same canonical shape as create_connection_session.",
                "additionalProperties": true
            },
            "metadata": {
                "type": "object",
                "description": "Optional connection session metadata, including entryId selection.",
                "additionalProperties": true
            },
            "timeout_ms": {
                "type": "integer",
                "minimum": 1,
                "maximum": MAX_SSH_EXEC_TIMEOUT_MS,
                "default": DEFAULT_SSH_EXEC_TIMEOUT_MS,
                "description": "Maximum wall-clock time to wait for the remote command before killing it."
            },
            "max_bytes": {
                "type": "integer",
                "minimum": 1,
                "maximum": MAX_SSH_READ_MAX_BYTES,
                "default": DEFAULT_SSH_READ_MAX_BYTES,
                "description": "Maximum bytes retained per stdout/stderr stream. Extra output is drained and reported as truncated."
            },
            "include_stderr": {
                "type": "boolean",
                "default": true,
                "description": "Include stderr in the response."
            },
            "include_base64": {
                "type": "boolean",
                "default": false,
                "description": "Include base64 bytes alongside text. Leave false for normal terminal text."
            }
        },
        "additionalProperties": false
    })
}

pub(super) fn host_resolve_connection_schema() -> Value {
    json!({
        "type": "object",
        "required": ["target_id"],
        "properties": {
            "target_id": {
                "type": "string",
                "description": "Target device or virtual machine id."
            },
            "protocol": {
                "type": "string",
                "enum": ["ssh"],
                "default": "ssh"
            },
            "template_id": {
                "type": "string"
            },
            "params": {
                "type": "object",
                "additionalProperties": true
            },
            "metadata": {
                "type": "object",
                "additionalProperties": true
            }
        },
        "additionalProperties": false
    })
}

pub(super) fn host_ssh_read_schema() -> Value {
    json!({
        "type": "object",
        "required": ["target_id", "session_id"],
        "properties": {
            "target_id": {
                "type": "string",
                "description": "Target device or virtual machine id."
            },
            "session_id": {
                "type": "string",
                "description": "Open SSH session id returned by ssh_open."
            },
            "after_seq": {
                "type": "integer",
                "minimum": 0,
                "description": "Read only chunks with sequence numbers greater than this cursor."
            },
            "max_bytes": {
                "type": "integer",
                "minimum": 1,
                "maximum": MAX_SSH_READ_MAX_BYTES,
                "description": "Maximum bytes to return from buffered terminal history."
            },
            "include_base64": {
                "type": "boolean",
                "default": false,
                "description": "Include base64 bytes alongside text. Leave false for normal terminal text."
            },
            "response_mode": {
                "type": "string",
                "enum": ["full", "combined", "chunks", "metadata"],
                "default": "full",
                "description": "full returns both chunks and combined_text; combined is compact for agent text loops; chunks omits combined_text; metadata returns only cursors and status."
            },
            "strip_ansi": {
                "type": "boolean",
                "default": false,
                "description": "Remove ANSI escape sequences from text fields."
            }
        },
        "additionalProperties": false
    })
}

pub(super) fn host_ssh_read_until_schema() -> Value {
    let mut properties = ssh_output_schema_properties(SshOutputSchemaDescriptions {
        after_seq: Some("Start reading after this sequence cursor."),
        timeout_ms: Some("Maximum polling time."),
        poll_ms: Some("Delay between terminal buffer polls."),
        max_bytes: Some("Maximum accumulated output bytes before returning truncated=true."),
        ..SshOutputSchemaDescriptions::default()
    });
    properties.insert(
        "pattern".to_string(),
        json!({
            "type": "string",
            "description": "Text or regex pattern to wait for in accumulated terminal output."
        }),
    );
    properties.insert(
        "regex".to_string(),
        json!({
            "type": "boolean",
            "default": false,
            "description": "Treat pattern as a Rust regex."
        }),
    );
    json!({
        "type": "object",
        "required": ["target_id", "session_id", "pattern"],
        "properties": properties,
        "additionalProperties": false
    })
}

pub(super) fn host_ssh_wait_for_prompt_schema() -> Value {
    let mut properties = ssh_output_schema_properties(SshOutputSchemaDescriptions::default());
    properties.insert(
        "prompt_regex".to_string(),
        json!({
            "type": "string",
            "default": "(?m)([$#>]\\s*)$",
            "description": "Regex used to detect the shell prompt at the end of accumulated output."
        }),
    );
    json!({
        "type": "object",
        "required": ["target_id", "session_id"],
        "properties": properties,
        "additionalProperties": false
    })
}

pub(super) fn host_ssh_wait_for_quiet_schema() -> Value {
    let mut properties = ssh_output_schema_properties(SshOutputSchemaDescriptions::default());
    properties.insert(
        "quiet_ms".to_string(),
        json!({
            "type": "integer",
            "minimum": 25,
            "maximum": MAX_SSH_QUIET_MS,
            "default": DEFAULT_SSH_QUIET_MS
        }),
    );
    json!({
        "type": "object",
        "required": ["target_id", "session_id"],
        "properties": properties,
        "additionalProperties": false
    })
}

pub(super) fn host_ssh_tail_log_schema() -> Value {
    json!({
        "type": "object",
        "required": ["target_id", "path"],
        "properties": {
            "target_id": {
                "type": "string",
                "description": "Target device or virtual machine id."
            },
            "path": {
                "type": "string",
                "description": "Remote log file path passed to tail. Shell quoting is handled by the backend."
            },
            "params": {
                "type": "object",
                "additionalProperties": true
            },
            "metadata": {
                "type": "object",
                "additionalProperties": true
            },
            "lines": {
                "type": "integer",
                "minimum": 1,
                "maximum": MAX_SSH_TAIL_LINES,
                "default": DEFAULT_SSH_TAIL_LINES
            },
            "follow_ms": {
                "type": "integer",
                "minimum": 0,
                "maximum": MAX_SSH_EXEC_TIMEOUT_MS,
                "default": 0,
                "description": "When >0, run timeout Ns tail -f and return captured output after that window."
            },
            "timeout_ms": {
                "type": "integer",
                "minimum": 1,
                "maximum": MAX_SSH_EXEC_TIMEOUT_MS
            },
            "max_bytes": {
                "type": "integer",
                "minimum": 1,
                "maximum": MAX_SSH_READ_MAX_BYTES,
                "default": DEFAULT_SSH_READ_MAX_BYTES
            },
            "include_stderr": {
                "type": "boolean",
                "default": true
            },
            "include_base64": {
                "type": "boolean",
                "default": false
            }
        },
        "additionalProperties": false
    })
}

pub(super) fn host_ssh_resize_schema() -> Value {
    json!({
        "type": "object",
        "required": ["target_id", "session_id", "cols", "rows"],
        "properties": {
            "target_id": {
                "type": "string"
            },
            "session_id": {
                "type": "string"
            },
            "cols": {
                "type": "integer",
                "minimum": 1,
                "maximum": u16::MAX
            },
            "rows": {
                "type": "integer",
                "minimum": 1,
                "maximum": u16::MAX
            }
        },
        "additionalProperties": false
    })
}

/// Build a curated-host diagnostic tool schema with a FLAT top-level
/// `target_id` (required) plus optional `params`/`metadata`, merged with the
/// tool's own `extra_properties` and `extra_required`. No `body` wrapper.
pub(super) fn host_diagnostic_schema(extra_properties: Value, extra_required: &[&str]) -> Value {
    let mut properties = Map::new();
    properties.insert(
        "target_id".to_string(),
        json!({
            "type": "string",
            "description": "Target device or virtual machine id."
        }),
    );
    properties.insert(
        "params".to_string(),
        json!({
            "type": "object",
            "description": "Optional SSH launch params in the same canonical shape as create_connection_session.",
            "additionalProperties": true
        }),
    );
    properties.insert(
        "metadata".to_string(),
        json!({
            "type": "object",
            "additionalProperties": true
        }),
    );
    if let Value::Object(extra) = extra_properties {
        for (key, value) in extra {
            properties.insert(key, value);
        }
    }
    let mut required = vec![Value::String("target_id".to_string())];
    required.extend(
        extra_required
            .iter()
            .map(|key| Value::String((*key).to_string())),
    );
    json!({
        "type": "object",
        "required": required,
        "properties": Value::Object(properties),
        "additionalProperties": false
    })
}

pub(super) fn host_inspect_schema() -> Value {
    host_diagnostic_schema(
        json!({
            "timeout_ms": {
                "type": "integer",
                "minimum": 1,
                "maximum": MAX_SSH_EXEC_TIMEOUT_MS
            },
            "max_bytes": {
                "type": "integer",
                "minimum": 1,
                "maximum": MAX_SSH_READ_MAX_BYTES,
                "default": DEFAULT_SSH_READ_MAX_BYTES
            }
        }),
        &[],
    )
}

pub(super) fn host_check_port_schema() -> Value {
    host_diagnostic_schema(
        json!({
            "host": {
                "type": "string",
                "default": "127.0.0.1",
                "description": "Host or IP to probe from the connection target."
            },
            "port": {
                "type": "integer",
                "minimum": 1,
                "maximum": 65535
            },
            "probe_timeout_ms": {
                "type": "integer",
                "minimum": 100,
                "maximum": 30000,
                "default": 5000
            },
            "max_bytes": {
                "type": "integer",
                "minimum": 1,
                "maximum": MAX_SSH_READ_MAX_BYTES,
                "default": 4096
            }
        }),
        &["port"],
    )
}

pub(super) fn host_read_file_schema() -> Value {
    host_diagnostic_schema(
        json!({
            "path": {
                "type": "string",
                "description": "Absolute Unix path. Known secret paths and dot segments are rejected."
            },
            "max_bytes": {
                "type": "integer",
                "minimum": 1,
                "maximum": MAX_SSH_READ_MAX_BYTES,
                "default": DEFAULT_HOST_READ_FILE_BYTES
            },
            "include_stderr": {
                "type": "boolean",
                "default": true
            }
        }),
        &["path"],
    )
}

pub(super) fn host_find_logs_schema() -> Value {
    host_diagnostic_schema(
        json!({
            "directory": {
                "type": "string",
                "default": "/var/log",
                "description": "Absolute directory to search."
            },
            "pattern": {
                "type": "string",
                "default": "*.log",
                "description": "Single file-name glob without path separators."
            },
            "max_depth": {
                "type": "integer",
                "minimum": 1,
                "maximum": 8,
                "default": 3
            },
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": MAX_HOST_FIND_LOGS_LIMIT,
                "default": DEFAULT_HOST_FIND_LOGS_LIMIT
            },
            "max_bytes": {
                "type": "integer",
                "minimum": 1,
                "maximum": MAX_SSH_READ_MAX_BYTES,
                "default": DEFAULT_SSH_READ_MAX_BYTES
            }
        }),
        &[],
    )
}

pub(super) fn host_ssh_close_schema() -> Value {
    json!({
        "type": "object",
        "required": ["target_id", "session_id"],
        "properties": {
            "target_id": {
                "type": "string",
                "description": "Target device or virtual machine id."
            },
            "session_id": {
                "type": "string",
                "description": "Open VNC session id returned by vnc_open."
            }
        },
        "additionalProperties": false
    })
}

pub(super) fn host_vnc_open_schema() -> Value {
    json!({
        "type": "object",
        "required": ["target_id"],
        "properties": {
            "target_id": {
                "type": "string",
                "description": "Target device or virtual machine id."
            },
            "params": {
                "type": "object",
                "additionalProperties": true
            },
            "metadata": {
                "type": "object",
                "additionalProperties": true
            }
        },
        "additionalProperties": false
    })
}

pub(super) fn cctv_export_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "target_id": { "type": "string", "format": "uuid" },
            "channel": { "type": "integer", "minimum": 0, "maximum": 65535 },
            "start": { "type": "string", "format": "date-time" },
            "end": { "type": "string", "format": "date-time" }
        },
        "required": ["target_id", "channel", "start", "end"],
        "additionalProperties": false
    })
}

pub(super) fn cctv_snapshot_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "target_id": { "type": "string", "format": "uuid" },
            "channel": { "type": "integer", "minimum": 0, "maximum": 65535 },
            "include_image": { "type": "boolean", "default": false },
            "max_inline_bytes": {
                "type": "integer",
                "minimum": 1,
                "maximum": MCP_RESPONSE_BODY_LIMIT_BYTES,
                "default": DEFAULT_VNC_INLINE_IMAGE_MAX_BYTES
            }
        },
        "required": ["target_id"],
        "additionalProperties": false
    })
}

pub(super) fn host_vnc_capture_schema() -> Value {
    json!({
        "type": "object",
        "required": ["target_id", "session_id"],
        "properties": {
            "target_id": {
                "type": "string",
                "description": "Target device or virtual machine id."
            },
            "session_id": {
                "type": "string",
                "description": "Open VNC session id returned by vnc_open."
            },
            "format": {
                "type": "string",
                "enum": ["png", "bmp"],
                "default": "png",
                "description": "PNG is the smaller default for agent observe-act loops; BMP is kept for raw compatibility."
            },
            "include_image": {
                "type": "boolean",
                "default": false,
                "description": "Also include the framebuffer as an MCP image content block when it fits max_inline_bytes. Leave false for normal agent loops; use the returned resource_link instead."
            },
            "max_inline_bytes": {
                "type": "integer",
                "minimum": 1,
                "maximum": MCP_RESPONSE_BODY_LIMIT_BYTES,
                "default": DEFAULT_VNC_INLINE_IMAGE_MAX_BYTES,
                "description": "Maximum raw capture bytes allowed for include_image. Larger captures return metadata and resource_link only."
            }
        },
        "additionalProperties": false
    })
}

pub(super) fn host_vnc_pointer_schema() -> Value {
    json!({
        "type": "object",
        "required": ["target_id", "session_id", "x", "y", "button_mask"],
        "properties": {
            "target_id": {
                "type": "string",
                "description": "Target device or virtual machine id."
            },
            "session_id": {
                "type": "string",
                "description": "Open VNC session id returned by vnc_open."
            },
            "x": {
                "type": "integer",
                "minimum": 0,
                "maximum": u16::MAX,
                "description": "Pointer x coordinate in framebuffer pixels."
            },
            "y": {
                "type": "integer",
                "minimum": 0,
                "maximum": u16::MAX,
                "description": "Pointer y coordinate in framebuffer pixels."
            },
            "button_mask": {
                "type": "integer",
                "minimum": 0,
                "maximum": u8::MAX,
                "description": "RFB button mask: 1 left, 2 middle, 4 right, 8/16 wheel."
            },
            "release_after": {
                "type": "boolean",
                "default": false,
                "description": "Send a second pointer event with button_mask=0 in the same RFB connection."
            }
        },
        "additionalProperties": false
    })
}

pub(super) fn host_vnc_key_schema() -> Value {
    json!({
        "type": "object",
        "required": ["target_id", "session_id", "keysym"],
        "properties": {
            "target_id": {
                "type": "string"
            },
            "session_id": {
                "type": "string"
            },
            "keysym": {
                "type": "integer",
                "minimum": 0,
                "maximum": u32::MAX,
                "description": "X11 keysym value, for example 65293 for Enter."
            },
            "press": {
                "type": "boolean",
                "default": true,
                "description": "When true, send key down and key up in one RFB connection."
            },
            "down": {
                "type": "boolean",
                "default": true,
                "description": "Used only when press=false; true sends key down, false sends key up."
            }
        },
        "additionalProperties": false
    })
}

pub(super) fn host_vnc_text_schema() -> Value {
    json!({
        "type": "object",
        "required": ["target_id", "session_id", "text"],
        "properties": {
            "target_id": {
                "type": "string"
            },
            "session_id": {
                "type": "string"
            },
            "text": {
                "type": "string",
                "maxLength": MAX_VNC_TEXT_BYTES,
                "description": "ASCII text typed as key down/up events. Non-ASCII input is rejected."
            }
        },
        "additionalProperties": false
    })
}

pub(super) fn host_vnc_close_schema() -> Value {
    json!({
        "type": "object",
        "required": ["target_id", "session_id"],
        "properties": {
            "target_id": {
                "type": "string"
            },
            "session_id": {
                "type": "string"
            }
        },
        "additionalProperties": false
    })
}

pub(super) fn host_session_client_credentials_schema() -> Value {
    json!({
        "type": "object",
        "required": ["target_id", "session_id"],
        "properties": {
            "target_id": {
                "type": "string",
                "description": "Target device or virtual machine id for the session."
            },
            "session_id": {
                "type": "string",
                "description": "VNC or RDP connection session id."
            }
        },
        "additionalProperties": false
    })
}

pub(super) fn create_connection_session_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "body": {
                "type": "object",
                "required": ["target_id", "protocol"],
                "properties": {
                    "target_id": {
                        "type": "string",
                        "description": "Target device or virtual machine id."
                    },
                    "protocol": {
                        "type": "string",
                        "description": "Connection protocol, for example ssh, vnc, or rdp."
                    },
                    "template_id": {
                        "type": "string"
                    },
                    "params": {
                        "type": "object",
                        "additionalProperties": true
                    },
                    "metadata": {
                        "type": "object",
                        "additionalProperties": true
                    },
                    "proxy_node_id": {
                        "type": "string"
                    }
                },
                "additionalProperties": true
            }
        },
        "required": ["body"],
        "additionalProperties": false
    })
}

pub(super) fn proxy_node_payload_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "body": proxy_node_body_schema(true)
        },
        "required": ["body"],
        "additionalProperties": false
    })
}

pub(super) fn path_id_proxy_node_payload_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": {
                "type": "string",
                "description": "Proxy node id for the REST path."
            },
            "path_params": {
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string"
                    }
                },
                "additionalProperties": false
            },
            "body": proxy_node_body_schema(false)
        },
        "required": ["body"],
        "additionalProperties": false
    })
}

pub(super) fn proxy_node_body_schema(create: bool) -> Value {
    let required = if create {
        vec!["name", "slug", "base_url"]
    } else {
        vec!["name", "slug", "base_url", "enabled"]
    };
    json!({
        "type": "object",
        "required": required,
        "properties": {
            "name": {
                "type": "string"
            },
            "slug": {
                "type": "string"
            },
            "base_url": {
                "type": "string"
            },
            "enabled": {
                "type": "boolean"
            }
        },
        "additionalProperties": true
    })
}

pub(super) fn path_id_target_proxy_assignment_payload_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": {
                "type": "string",
                "description": "Target device or virtual machine id for the REST path."
            },
            "path_params": {
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string"
                    }
                },
                "additionalProperties": false
            },
            "body": {
                "type": "object",
                "required": ["proxy_node_id"],
                "properties": {
                    "proxy_node_id": {
                        "type": ["string", "null"],
                        "description": "Proxy node id to assign, or null to clear the assignment."
                    }
                },
                "additionalProperties": false
            }
        },
        "required": ["body"],
        "additionalProperties": false
    })
}

pub(super) fn config_context_payload_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "body": config_context_body_schema()
        },
        "required": ["body"],
        "additionalProperties": false
    })
}

pub(super) fn path_id_config_context_payload_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": {
                "type": "string",
                "description": "Configuration context id for the REST path."
            },
            "path_params": {
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string"
                    }
                },
                "additionalProperties": false
            },
            "body": config_context_body_schema()
        },
        "required": ["body"],
        "additionalProperties": false
    })
}

pub(super) fn config_context_body_schema() -> Value {
    json!({
        "type": "object",
        "required": ["name", "weight", "data"],
        "properties": {
            "name": {
                "type": "string"
            },
            "weight": {
                "type": "integer"
            },
            "data": {
                "type": "object",
                "additionalProperties": true
            },
            "is_active": {
                "type": "boolean"
            }
        },
        "additionalProperties": true
    })
}

pub(super) fn config_context_binding_list_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "query": {
                "type": "object",
                "required": ["context_id"],
                "properties": {
                    "context_id": {
                        "type": "string"
                    },
                    "limit": {
                        "type": "integer"
                    },
                    "offset": {
                        "type": "integer"
                    }
                },
                "additionalProperties": true
            }
        },
        "required": ["query"],
        "additionalProperties": false
    })
}

pub(super) fn config_context_binding_payload_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "body": {
                "type": "object",
                "required": ["context_id", "scope_type", "scope_id"],
                "properties": {
                    "context_id": {
                        "type": "string"
                    },
                    "scope_type": {
                        "type": "string"
                    },
                    "scope_id": {
                        "type": "string"
                    }
                },
                "additionalProperties": true
            }
        },
        "required": ["body"],
        "additionalProperties": false
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::mcp::test_support::*;

    #[test]
    fn ssh_wait_schemas_share_the_terminal_argument_set() {
        let until = host_ssh_read_until_schema();
        assert_eq!(
            until
                .pointer("/properties/after_seq/description")
                .and_then(Value::as_str),
            Some("Start reading after this sequence cursor.")
        );
        assert!(until.pointer("/properties/pattern").is_some());
        assert!(until.pointer("/properties/regex").is_some());

        let prompt = host_ssh_wait_for_prompt_schema();
        assert_eq!(
            prompt
                .pointer("/properties/prompt_regex/default")
                .and_then(Value::as_str),
            Some("(?m)([$#>]\\s*)$")
        );
        assert!(prompt.pointer("/properties/pattern").is_none());
        assert!(
            prompt
                .pointer("/properties/after_seq/description")
                .is_none()
        );

        let quiet = host_ssh_wait_for_quiet_schema();
        assert_eq!(
            quiet
                .pointer("/properties/quiet_ms/maximum")
                .and_then(Value::as_u64),
            Some(MAX_SSH_QUIET_MS)
        );
        assert!(quiet.pointer("/properties/quiet_ms/default").is_some());

        for schema in [&until, &prompt, &quiet] {
            assert_eq!(
                schema
                    .pointer("/properties/response_mode/enum")
                    .and_then(Value::as_array)
                    .map(Vec::len),
                Some(4),
                "every terminal read schema advertises the four response modes"
            );
            assert!(schema.pointer("/properties/max_bytes/default").is_some());
            assert!(schema.pointer("/properties/strip_ansi").is_some());
        }
        assert_eq!(
            until
                .pointer("/required")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(3)
        );
        assert_eq!(
            prompt
                .pointer("/required")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(2)
        );
    }

    #[test]
    fn id_addressed_schemas_advertise_path_params() {
        let context = full_context();
        let inventory = inventory_tool_schema(&context, InventoryToolOperation::Get);
        assert!(inventory.pointer("/properties/id").is_some());
        assert_eq!(
            inventory
                .pointer("/properties/path_params/properties/id/type")
                .and_then(Value::as_str),
            Some("string")
        );
        let access = access_control_tool_schema(&context, AccessControlToolOperation::Get);
        assert!(access.pointer("/properties/id").is_some());
        assert_eq!(
            access
                .pointer("/properties/path_params/properties/id/type")
                .and_then(Value::as_str),
            Some("string")
        );
    }
}
