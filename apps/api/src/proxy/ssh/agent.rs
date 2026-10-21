use super::*;
pub(in crate::proxy) async fn agent_ssh_session(
    state: Arc<ProxyState>,
    connection_id: Uuid,
) -> Result<Arc<SshSession>, AppError> {
    let entry = {
        let guard = state.connections.read().await;
        guard.get(&connection_id).cloned()
    }
    .ok_or_else(|| AppError::NotFound("Proxy connection not found".to_string()))?;

    match super::super::parse_connection_type(&entry.payload) {
        Ok(ProxyConnectionType::Ssh) => {}
        Ok(_) => {
            return Err(AppError::Validation(
                "Agent-native terminal access is only supported for SSH sessions".to_string(),
            ));
        }
        Err(error) => return Err(AppError::Validation(error.to_string())),
    }

    let payload = entry.payload.clone();
    let metrics = entry.metrics.clone();
    get_or_start_ssh_session(state.clone(), connection_id, move || async move {
        start_new_ssh_session_runtime(connection_id, &payload, metrics).await
    })
    .await
}

pub(in crate::proxy) async fn agent_ssh_exec(
    state: Arc<ProxyState>,
    connection_id: Uuid,
    request: AgentSshExecRequest,
) -> Result<AgentSshExecResult, AppError> {
    if request.command.trim().is_empty() {
        return Err(AppError::Validation(
            "SSH exec command must not be empty".to_string(),
        ));
    }
    if request.timeout.is_zero() {
        return Err(AppError::Validation(
            "SSH exec timeout must be positive".to_string(),
        ));
    }

    let entry = {
        let guard = state.connections.read().await;
        guard.get(&connection_id).cloned()
    }
    .ok_or_else(|| AppError::NotFound("Proxy connection not found".to_string()))?;

    match super::super::parse_connection_type(&entry.payload) {
        Ok(ProxyConnectionType::Ssh) => {}
        Ok(_) => {
            return Err(AppError::Validation(
                "Agent-native command execution is only supported for SSH sessions".to_string(),
            ));
        }
        Err(error) => return Err(AppError::Validation(error.to_string())),
    }

    run_ssh_exec_runtime(
        connection_id,
        &entry.payload,
        entry.metrics,
        request,
        SshProcessMetricsMode::Lifecycle,
        None,
    )
    .await
}

pub(in crate::proxy) async fn agent_ssh_list_dir(
    state: Arc<ProxyState>,
    connection_id: Uuid,
    path: String,
) -> Result<AgentSshFileList, AppError> {
    let entry = require_ssh_proxy_entry(state.clone(), connection_id, "SSH file browser").await?;
    let remote_path = validate_remote_file_path(&path)?;
    let _sidecar_permit = state.acquire_ssh_file_sidecar(connection_id).await?;
    let control_path = active_ssh_control_path(&state, connection_id).await?;
    let result = run_ssh_exec_runtime(
        connection_id,
        &entry.payload,
        entry.metrics,
        AgentSshExecRequest {
            stdin: None,
            command: build_remote_list_command(remote_path),
            timeout: SSH_FILE_LIST_TIMEOUT,
            max_bytes_per_stream: SSH_FILE_LIST_MAX_BYTES,
        },
        SshProcessMetricsMode::Sidecar,
        Some(control_path),
    )
    .await?;

    if !result.success {
        return Err(AppError::External(format!(
            "Failed to list remote directory: {}",
            ssh_stream_excerpt(&result.stderr.bytes)
        )));
    }

    parse_remote_list_output(&result.stdout.bytes)
}

pub(in crate::proxy) async fn agent_ssh_upload_file(
    state: Arc<ProxyState>,
    connection_id: Uuid,
    local_path: PathBuf,
    remote_path: String,
) -> Result<AgentSshFileTransferResult, AppError> {
    let entry = require_ssh_proxy_entry(state.clone(), connection_id, "SSH file upload").await?;
    let remote_path = validate_remote_file_path(&remote_path)?.to_string();
    let local_size = tokio::fs::metadata(&local_path)
        .await
        .map_err(|error| {
            AppError::Validation(format!("Upload temp file is not readable: {error}"))
        })?
        .len();
    let _sidecar_permit = state.acquire_ssh_file_sidecar(connection_id).await?;
    let control_path = active_ssh_control_path(&state, connection_id).await?;
    let prepared = prepare_scp_process(connection_id, &entry.payload, control_path).await?;
    run_scp_transfer(
        connection_id,
        entry.metrics,
        prepared,
        ScpDirection::Upload {
            local_path,
            remote_path,
            bytes: local_size,
        },
        SshProcessMetricsMode::Sidecar,
    )
    .await
}

pub(in crate::proxy) async fn agent_ssh_ensure_dir(
    state: Arc<ProxyState>,
    connection_id: Uuid,
    remote_path: String,
) -> Result<AgentSshFileTransferResult, AppError> {
    let entry = require_ssh_proxy_entry(state.clone(), connection_id, "SSH file browser").await?;
    let remote_path = validate_remote_file_path(&remote_path)?.to_string();
    let _sidecar_permit = state.acquire_ssh_file_sidecar(connection_id).await?;
    let control_path = active_ssh_control_path(&state, connection_id).await?;
    let result = run_ssh_exec_runtime(
        connection_id,
        &entry.payload,
        entry.metrics,
        AgentSshExecRequest {
            stdin: None,
            command: build_remote_mkdir_command(&remote_path),
            timeout: SSH_FILE_LIST_TIMEOUT,
            max_bytes_per_stream: 1024,
        },
        SshProcessMetricsMode::Sidecar,
        Some(control_path),
    )
    .await?;

    if !result.success {
        return Err(AppError::External(format!(
            "Failed to create remote directory: {}",
            ssh_stream_excerpt(&result.stderr.bytes)
        )));
    }

    Ok(AgentSshFileTransferResult {
        bytes: 0,
        duration_ms: result.duration_ms,
    })
}

pub(in crate::proxy) async fn agent_ssh_preflight_download_file(
    state: Arc<ProxyState>,
    connection_id: Uuid,
    remote_path: String,
    max_bytes: u64,
    archive: bool,
) -> Result<AgentSshFileDownloadPreflight, AppError> {
    let entry = require_ssh_proxy_entry(state.clone(), connection_id, "SSH file download").await?;
    let remote_path = validate_remote_file_path(&remote_path)?.to_string();
    let _sidecar_permit = state.acquire_ssh_file_sidecar(connection_id).await?;
    let control_path = active_ssh_control_path(&state, connection_id).await?;
    let command = if archive {
        build_remote_archive_preflight_command(&remote_path)
    } else {
        build_remote_download_preflight_command(&remote_path)
    };
    let result = run_ssh_exec_runtime(
        connection_id,
        &entry.payload,
        entry.metrics,
        AgentSshExecRequest {
            stdin: None,
            command,
            timeout: SSH_FILE_LIST_TIMEOUT,
            max_bytes_per_stream: 1024,
        },
        SshProcessMetricsMode::Sidecar,
        Some(control_path),
    )
    .await?;

    if !result.success {
        return Err(AppError::External(format!(
            "Failed to inspect remote file: {}",
            ssh_stream_excerpt(&result.stderr.bytes)
        )));
    }

    parse_download_preflight_output(&remote_path, archive, &result.stdout.bytes, max_bytes)
}

pub(in crate::proxy) async fn agent_ssh_preview_file(
    state: Arc<ProxyState>,
    connection_id: Uuid,
    remote_path: String,
) -> Result<AgentSshFilePreview, AppError> {
    let entry = require_ssh_proxy_entry(state.clone(), connection_id, "SSH file preview").await?;
    let remote_path = validate_remote_file_path(&remote_path)?.to_string();
    let _sidecar_permit = state.acquire_ssh_file_sidecar(connection_id).await?;
    let control_path = active_ssh_control_path(&state, connection_id).await?;
    let result = run_ssh_exec_runtime(
        connection_id,
        &entry.payload,
        entry.metrics,
        AgentSshExecRequest {
            stdin: None,
            command: build_remote_preview_command(&remote_path, SSH_FILE_PREVIEW_MAX_BYTES),
            timeout: SSH_FILE_LIST_TIMEOUT,
            max_bytes_per_stream: SSH_FILE_PREVIEW_OUTPUT_MAX_BYTES,
        },
        SshProcessMetricsMode::Sidecar,
        Some(control_path),
    )
    .await?;

    if !result.success {
        return Err(AppError::External(format!(
            "Failed to preview remote file: {}",
            ssh_stream_excerpt(&result.stderr.bytes)
        )));
    }

    parse_remote_preview_output(
        &remote_path,
        &result.stdout.bytes,
        SSH_FILE_PREVIEW_MAX_BYTES,
        result.stdout.truncated,
    )
}

pub(in crate::proxy) async fn agent_ssh_delete_files(
    state: Arc<ProxyState>,
    connection_id: Uuid,
    remote_paths: Vec<String>,
) -> Result<AgentSshFileDeleteResult, AppError> {
    let entry = require_ssh_proxy_entry(state.clone(), connection_id, "SSH file delete").await?;
    let paths = validate_remote_delete_paths(remote_paths)?;
    let _sidecar_permit = state.acquire_ssh_file_sidecar(connection_id).await?;
    let control_path = active_ssh_control_path(&state, connection_id).await?;
    let started_at = Instant::now();
    let result = run_ssh_exec_runtime(
        connection_id,
        &entry.payload,
        entry.metrics,
        AgentSshExecRequest {
            stdin: None,
            command: build_remote_delete_command(&paths),
            timeout: SSH_FILE_TRANSFER_TIMEOUT,
            max_bytes_per_stream: 4096,
        },
        SshProcessMetricsMode::Sidecar,
        Some(control_path),
    )
    .await?;

    if !result.success {
        return Err(AppError::External(format!(
            "Failed to delete remote paths: {}",
            ssh_stream_excerpt(&result.stderr.bytes)
        )));
    }

    Ok(AgentSshFileDeleteResult {
        paths,
        duration_ms: started_at.elapsed().as_millis(),
    })
}

pub(in crate::proxy) async fn agent_ssh_move_files(
    state: Arc<ProxyState>,
    connection_id: Uuid,
    remote_paths: Vec<String>,
    destination: String,
) -> Result<AgentSshFileMoveResult, AppError> {
    let entry = require_ssh_proxy_entry(state.clone(), connection_id, "SSH file move").await?;
    let paths = validate_remote_delete_paths(remote_paths)?;
    let destination = validate_remote_file_path(&destination)?.to_string();
    let _sidecar_permit = state.acquire_ssh_file_sidecar(connection_id).await?;
    let control_path = active_ssh_control_path(&state, connection_id).await?;
    let started_at = Instant::now();
    let result = run_ssh_exec_runtime(
        connection_id,
        &entry.payload,
        entry.metrics,
        AgentSshExecRequest {
            stdin: None,
            command: build_remote_move_command(&paths, &destination),
            timeout: SSH_FILE_TRANSFER_TIMEOUT,
            max_bytes_per_stream: 4096,
        },
        SshProcessMetricsMode::Sidecar,
        Some(control_path),
    )
    .await?;

    if !result.success {
        return Err(AppError::External(format!(
            "Failed to move remote paths: {}",
            ssh_stream_excerpt(&result.stderr.bytes)
        )));
    }

    Ok(AgentSshFileMoveResult {
        paths,
        destination,
        duration_ms: started_at.elapsed().as_millis(),
    })
}

pub(in crate::proxy) async fn agent_ssh_download_file(
    state: Arc<ProxyState>,
    connection_id: Uuid,
    remote_path: String,
    local_path: PathBuf,
    max_bytes: u64,
    archive: bool,
) -> Result<AgentSshFileTransferResult, AppError> {
    let entry = require_ssh_proxy_entry(state.clone(), connection_id, "SSH file download").await?;
    let remote_path = validate_remote_file_path(&remote_path)?.to_string();
    let _sidecar_permit = state.acquire_ssh_file_sidecar(connection_id).await?;
    let control_path = active_ssh_control_path(&state, connection_id).await?;
    if archive {
        let result = run_ssh_exec_runtime(
            connection_id,
            &entry.payload,
            entry.metrics.clone(),
            AgentSshExecRequest {
                stdin: None,
                command: build_remote_archive_preflight_command(&remote_path),
                timeout: SSH_FILE_LIST_TIMEOUT,
                max_bytes_per_stream: 1024,
            },
            SshProcessMetricsMode::Sidecar,
            Some(control_path.clone()),
        )
        .await?;
        if !result.success {
            return Err(AppError::External(format!(
                "Failed to inspect remote directory: {}",
                ssh_stream_excerpt(&result.stderr.bytes)
            )));
        }
        let _ =
            parse_download_preflight_output(&remote_path, true, &result.stdout.bytes, max_bytes)?;
    }
    run_ssh_download_to_file(SshDownloadRequest {
        connection_id,
        payload: &entry.payload,
        metrics: entry.metrics,
        remote_path,
        local_path,
        max_bytes,
        archive,
        metrics_mode: SshProcessMetricsMode::Sidecar,
        control_path,
    })
    .await
}

pub(super) async fn get_or_start_ssh_session<F, Fut>(
    state: Arc<ProxyState>,
    connection_id: Uuid,
    create: F,
) -> Result<Arc<SshSession>, AppError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Arc<SshSession>, AppError>>,
{
    if let Some(existing) = cached_live_ssh_session(&state, connection_id).await {
        return Ok(existing);
    }

    let init_lock = ssh_session_init_lock(&state, connection_id).await;
    let _init_guard = init_lock.lock().await;

    if let Some(existing) = cached_live_ssh_session(&state, connection_id).await {
        return Ok(existing);
    }

    if !state.connections.read().await.contains_key(&connection_id) {
        return Err(AppError::NotFound("Proxy connection not found".to_string()));
    }

    // Guard against unbounded PTY + file-descriptor growth. Re-attaches
    // (which hit the registry above and return early) are not affected.
    if state.ssh_sessions.read().await.len() >= MAX_CONCURRENT_SSH_SESSIONS {
        return Err(AppError::TooManyRequests(
            "Too many active SSH sessions; try again later".to_string(),
        ));
    }

    let created = create().await?;
    let connections_guard = state.connections.read().await;
    if !connections_guard.contains_key(&connection_id) {
        created.shutdown().await;
        return Err(AppError::NotFound("Proxy connection not found".to_string()));
    }

    let mut guard = state.ssh_sessions.write().await;
    Ok(guard
        .entry(connection_id)
        .or_insert_with(|| created.clone())
        .clone())
}

/// Returns a cached SSH runtime only while it is live. A terminal runtime is
/// removed before a new attach/agent request can reuse its closed input
/// channel; the next caller starts a fresh SSH process under the init lock.
pub(super) async fn cached_live_ssh_session(
    state: &Arc<ProxyState>,
    connection_id: Uuid,
) -> Option<Arc<SshSession>> {
    let existing = state
        .ssh_sessions
        .read()
        .await
        .get(&connection_id)
        .cloned()?;
    if existing.terminal_state().is_none() {
        return Some(existing);
    }

    let mut sessions = state.ssh_sessions.write().await;
    if sessions
        .get(&connection_id)
        .is_some_and(|current| Arc::ptr_eq(current, &existing))
        && existing.terminal_state().is_some()
    {
        sessions.remove(&connection_id);
    }
    None
}
