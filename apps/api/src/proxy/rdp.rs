use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Map, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::RwLock;
use tokio::time::timeout;
use uuid::Uuid;

use crate::contexts::connections::domain::normalize_rdp_username_domain;
use crate::errors::AppError;
use crate::observability::{EventFields, LogLevel, log_event};

use super::guac::{
    GUAC_STATUS_UPSTREAM_ERROR, GUAC_STATUS_UPSTREAM_TIMEOUT, GUAC_STATUS_UPSTREAM_UNAVAILABLE,
    SliceBuffer, encode_guac_instruction, parse_guac_instruction,
};
use super::metrics::{
    ConnectionMetricsState, LinkType, TrafficDirection, record_connection_traffic,
};

mod desktop;

pub(in crate::proxy) use desktop::{
    DesktopSession, proxy_ws_desktop_loop, proxy_ws_desktop_observe_loop, runtime_presence,
};

#[derive(Debug, Clone)]
struct RdpLaunchConfig {
    host: String,
    port: u16,
    username: Option<String>,
    domain: Option<String>,
    password: Option<String>,
}

#[derive(Debug, Clone)]
struct RdpClientOptions {
    geometry: DesktopGeometry,
    color_depth: Option<u32>,
    ignore_cert: bool,
    disable_auth: bool,
    disable_gfx: bool,
    force_lossless: bool,
    security: Option<String>,
    /// Enables Guacamole RDPDR drive redirection so the session exposes a
    /// virtual disk. guacd then emits a `filesystem` instruction the browser
    /// uses to browse/upload/download files (the SSH file-browser analogue).
    enable_drive: bool,
}

/// Screen geometry advertised to guacd in the `size` handshake capability.
///
/// guacd copies these values directly into the joining user's `info`. VNC's
/// first display resize and RDP's resolution suggestion both divide by values
/// derived from them, so zero width, height or DPI is never a valid handshake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DesktopGeometry {
    width: u32,
    height: u32,
    dpi: u32,
}

impl DesktopGeometry {
    fn new(width: u32, height: u32, dpi: u32) -> Result<Self, AppError> {
        if width == 0 || height == 0 || dpi == 0 {
            return Err(AppError::Validation(
                "Desktop width, height and DPI must be positive".to_string(),
            ));
        }
        Ok(Self { width, height, dpi })
    }

    const fn default_desktop() -> Self {
        Self {
            width: DEFAULT_DESKTOP_WIDTH,
            height: DEFAULT_DESKTOP_HEIGHT,
            dpi: DEFAULT_DESKTOP_DPI,
        }
    }

    fn width(self) -> u32 {
        self.width
    }

    fn height(self) -> u32 {
        self.height
    }

    fn dpi(self) -> u32 {
        self.dpi
    }

    fn components(self) -> (u32, u32, u32) {
        (self.width, self.height, self.dpi)
    }
}

/// Mount name shown for the redirected drive inside the Windows session.
const RDP_DRIVE_NAME: &str = "Piping";
/// Base directory on the guacd host that backs every session's virtual drive.
/// guacd creates the per-session subdirectory (`create-drive-path=true`).
const DEFAULT_RDP_DRIVE_BASE_PATH: &str = "/tmp/guac-drives";
const RDP_DRIVE_BASE_ENV: &str = "PIPING_PROXY__RDP_DRIVE_BASE_PATH";
const EMBEDDED_GUACD_DRIVE_BASE_ENV: &str = "PIPING_PROXY__EMBEDDED_GUACD_DRIVE_BASE";

const GUAC_READ_BUFFER_SIZE: usize = 32 * 1024;
const KEEPALIVE_INTERVAL_SECS: u64 = 5;
const DEFAULT_DESKTOP_WIDTH: u32 = 1_024;
const DEFAULT_DESKTOP_HEIGHT: u32 = 768;
const DEFAULT_DESKTOP_DPI: u32 = 96;

/// Client capability values that advertise no audio or video codecs. guacd
/// still requires the instruction so its mimetype lists are initialized.
const NO_MIMETYPES: &[&str] = &[];
/// Image codecs the browser-backed desktop supports.
const IMAGE_MIMETYPES: &[&str] = &["image/png", "image/jpeg"];

#[derive(Debug)]
enum RdpTargetProbeFailure {
    Connect(std::io::Error),
    Timeout,
}

async fn probe_rdp_target_latency(
    host: &str,
    port: u16,
    connect_timeout: std::time::Duration,
) -> Result<i64, RdpTargetProbeFailure> {
    let started_at = std::time::Instant::now();
    match timeout(connect_timeout, TcpStream::connect((host, port))).await {
        Ok(Ok(stream)) => {
            super::configure_interactive_tcp_stream(&stream);
            drop(stream);
            Ok(super::elapsed_latency_ms_since(started_at))
        }
        Ok(Err(error)) => Err(RdpTargetProbeFailure::Connect(error)),
        Err(_) => Err(RdpTargetProbeFailure::Timeout),
    }
}

async fn send_guac_error_and_close(
    socket: &mut axum::extract::ws::WebSocket,
    status_code: u16,
    reason: &str,
) {
    let elements = vec![
        "error".to_string(),
        reason.trim().to_string(),
        status_code.to_string(),
    ];
    let bytes = encode_guac_instruction(&elements);
    let text = String::from_utf8_lossy(&bytes).to_string();
    let _ = socket
        .send(axum::extract::ws::Message::Text(text.into()))
        .await;
    let _ = socket.send(axum::extract::ws::Message::Close(None)).await;
}

/// guacamole-common-js's WebSocketTunnel sends nothing until it receives at
/// least one server instruction; the first is the internal tunnel UUID.
async fn send_tunnel_uuid(
    socket: &mut axum::extract::ws::WebSocket,
    connection_id: Uuid,
) -> Result<(), AppError> {
    let tunnel_uuid = connection_id.to_string();
    let uuid_bytes = encode_guac_instruction(&["".to_string(), tunnel_uuid]);
    let uuid_text = String::from_utf8_lossy(&uuid_bytes).to_string();
    socket
        .send(axum::extract::ws::Message::Text(uuid_text.into()))
        .await
        .map_err(|error| {
            AppError::External(format!("Client websocket send failed (uuid): {}", error))
        })
}

/// Best-effort reachability probe. Never hard-fails: distributed topologies can
/// give false negatives, and the guacd connect/handshake below is the verdict.
async fn probe_rdp_target_latency_logged(
    connection_id: Uuid,
    launch: &RdpLaunchConfig,
    rdp_target: &str,
    connect_timeout: std::time::Duration,
) -> Option<i64> {
    match probe_rdp_target_latency(&launch.host, launch.port, connect_timeout).await {
        Ok(latency_ms) => Some(latency_ms),
        Err(RdpTargetProbeFailure::Connect(error)) => {
            let (_status_code, reason) = classify_tcp_connect_error(&error);
            log_event(
                LogLevel::Warning,
                "proxy.rdp.target.probe.failed",
                EventFields::default()
                    .with_operation("proxy_rdp_guacd_bridge")
                    .with_resource("proxy_connection", connection_id.to_string())
                    .with_context("rdp_target", rdp_target.to_string())
                    .with_context("reason", reason.to_string())
                    .with_context("error", error.to_string()),
            );
            None
        }
        Err(RdpTargetProbeFailure::Timeout) => {
            log_event(
                LogLevel::Warning,
                "proxy.rdp.target.probe.timed_out",
                EventFields::default()
                    .with_operation("proxy_rdp_guacd_bridge")
                    .with_resource("proxy_connection", connection_id.to_string())
                    .with_context("rdp_target", rdp_target.to_string())
                    .with_context("timeout_ms", connect_timeout.as_millis().to_string()),
            );
            None
        }
    }
}

enum GuacdHandshakeFailure {
    TimedOut,
    Failed(AppError),
}

/// Reads guacd's `args` response, framing at instruction boundaries.
async fn read_guacd_handshake_args<R: AsyncRead + Unpin>(
    tcp_reader: &mut R,
    buffer: &mut [u8],
    stream_buf: &mut SliceBuffer,
    metrics: &Arc<RwLock<ConnectionMetricsState>>,
) -> Result<Vec<String>, AppError> {
    loop {
        let parsed = parse_guac_instruction(stream_buf.available())?;
        if let Some((consumed, elements)) = parsed {
            record_connection_traffic(
                metrics,
                LinkType::ProxyProtocol,
                TrafficDirection::Rx,
                consumed,
            )
            .await;
            stream_buf.consume(consumed);
            if elements.first().map(|s| s.as_str()) == Some("args") {
                let args: Vec<String> = elements.into_iter().skip(1).collect();
                return Ok(args);
            }
            if elements.first().map(|s| s.as_str()) == Some("error") {
                return Err(AppError::External(
                    "guacd returned an error during handshake".to_string(),
                ));
            }
            continue;
        }

        let read = tcp_reader.read(buffer).await.map_err(|error| {
            AppError::External(format!("guacd TCP read failed (handshake): {}", error))
        })?;
        if read == 0 {
            return Err(AppError::External(
                "guacd closed connection during handshake".to_string(),
            ));
        }
        stream_buf.extend_from_slice(&buffer[..read]);
    }
}

/// Performs the proxy-side guacd handshake (`select` then `args`) under the
/// connect timeout, distinguishing a timeout from any other failure so the
/// caller can send the matching close frame.
async fn perform_guacd_handshake<R, W>(
    metrics: &Arc<RwLock<ConnectionMetricsState>>,
    tcp_reader: &mut R,
    tcp_writer: &mut W,
    stream_buf: &mut SliceBuffer,
    buffer: &mut [u8],
    connect_timeout: std::time::Duration,
    selector: &str,
) -> Result<Vec<String>, GuacdHandshakeFailure>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let select = encode_guac_instruction(&["select".to_string(), selector.to_string()]);
    tcp_writer.write_all(&select).await.map_err(|error| {
        GuacdHandshakeFailure::Failed(AppError::External(format!(
            "guacd TCP write failed (select): {}",
            error
        )))
    })?;
    record_connection_traffic(
        metrics,
        LinkType::ProxyProtocol,
        TrafficDirection::Tx,
        select.len(),
    )
    .await;

    match timeout(
        connect_timeout,
        read_guacd_handshake_args(tcp_reader, buffer, stream_buf, metrics),
    )
    .await
    {
        Ok(Ok(args)) => Ok(args),
        Ok(Err(error)) => Err(GuacdHandshakeFailure::Failed(error)),
        Err(_) => Err(GuacdHandshakeFailure::TimedOut),
    }
}

/// Splits guacd's version token from the remaining `connect` parameters.
fn extract_guacd_version(mut args: Vec<String>) -> Result<(String, Vec<String>), AppError> {
    let version = args
        .first()
        .map(|value| value.trim().to_string())
        .filter(|value| value.starts_with("VERSION_"))
        .ok_or_else(|| {
            AppError::External("guacd handshake returned invalid version token".to_string())
        })?;
    args.remove(0);
    Ok((version, args))
}

fn log_empty_rdp_credentials(
    connection_id: Uuid,
    launch: &RdpLaunchConfig,
    options: &RdpClientOptions,
    rdp_target: &str,
) {
    if !options.disable_auth
        && launch
            .username
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
        && launch
            .password
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
    {
        log_event(
            LogLevel::Warning,
            "proxy.rdp.credentials.empty",
            EventFields::default()
                .with_operation("proxy_rdp_guacd_bridge")
                .with_resource("proxy_connection", connection_id.to_string())
                .with_context("rdp_target", rdp_target.to_string())
                .with_context("note", "username/password are empty"),
        );
    }
}

async fn write_guacd_instruction<W: AsyncWrite + Unpin>(
    tcp_writer: &mut W,
    metrics: &Arc<RwLock<ConnectionMetricsState>>,
    elements: &[String],
    label: &str,
) -> Result<(), AppError> {
    let encoded = encode_guac_instruction(elements);
    tcp_writer.write_all(&encoded).await.map_err(|error| {
        AppError::External(format!("guacd TCP write failed ({label}): {error}"))
    })?;
    record_connection_traffic(
        metrics,
        LinkType::ProxyProtocol,
        TrafficDirection::Tx,
        encoded.len(),
    )
    .await;
    Ok(())
}

/// Client capability instructions sent before `connect`. Advertised
/// capabilities determine which media channels guacd opens for this leg.
struct GuacClientCapabilities {
    size: DesktopGeometry,
    audio: &'static [&'static str],
    video: &'static [&'static str],
    image: &'static [&'static str],
    timezone: Option<&'static str>,
}

fn mimetype_capability(opcode: &str, mimetypes: &[&str]) -> Vec<String> {
    let mut elements = Vec::with_capacity(mimetypes.len().saturating_add(1));
    elements.push(opcode.to_string());
    elements.extend(mimetypes.iter().map(|mimetype| (*mimetype).to_string()));
    elements
}

async fn send_client_capabilities<W: AsyncWrite + Unpin>(
    tcp_writer: &mut W,
    metrics: &Arc<RwLock<ConnectionMetricsState>>,
    capabilities: &GuacClientCapabilities,
) -> Result<(), AppError> {
    let (width, height, dpi) = capabilities.size.components();
    let size = vec![
        "size".to_string(),
        width.to_string(),
        height.to_string(),
        dpi.to_string(),
    ];
    write_guacd_instruction(tcp_writer, metrics, &size, "size").await?;

    // guacd initializes the mimetype lists only when it receives these
    // instructions. Omitting an unsupported channel leaves the list NULL and
    // later capability checks dereference it, so "no support" is an empty list.
    let audio = mimetype_capability("audio", capabilities.audio);
    write_guacd_instruction(tcp_writer, metrics, &audio, "audio").await?;

    let video = mimetype_capability("video", capabilities.video);
    write_guacd_instruction(tcp_writer, metrics, &video, "video").await?;

    let image = mimetype_capability("image", capabilities.image);
    write_guacd_instruction(tcp_writer, metrics, &image, "image").await?;

    if let Some(timezone) = capabilities.timezone {
        let timezone = vec!["timezone".to_string(), timezone.to_string()];
        write_guacd_instruction(tcp_writer, metrics, &timezone, "timezone").await?;
    }
    Ok(())
}

/// Sends the client capabilities and `connect` as one ordered handshake.
/// guacd accepts capability instructions only before `connect`.
async fn send_client_handshake<W: AsyncWrite + Unpin>(
    tcp_writer: &mut W,
    metrics: &Arc<RwLock<ConnectionMetricsState>>,
    capabilities: &GuacClientCapabilities,
    connect_elements: &[String],
) -> Result<(), AppError> {
    send_client_capabilities(tcp_writer, metrics, capabilities).await?;
    write_guacd_instruction(tcp_writer, metrics, connect_elements, "connect").await
}

struct RdpConnectContext<'a> {
    resolved_security: &'a str,
    drive_base_path: &'a str,
    connection_id: Uuid,
    version: &'a str,
}

fn build_guacd_connect_elements(
    args: &[String],
    launch: &RdpLaunchConfig,
    options: &RdpClientOptions,
    ctx: &RdpConnectContext<'_>,
) -> Vec<String> {
    let mut connect_elements: Vec<String> = Vec::with_capacity(args.len().saturating_add(2));
    connect_elements.push("connect".to_string());
    connect_elements.push(ctx.version.to_string());
    for arg in args {
        connect_elements.push(rdp_connect_arg(arg, launch, options, ctx));
    }
    connect_elements
}

/// Maps one guacd parameter name to the value the proxy is allowed to supply.
/// Unsupported/unknown keys become empty, matching the original handshake.
fn rdp_connect_arg(
    key: &str,
    launch: &RdpLaunchConfig,
    options: &RdpClientOptions,
    ctx: &RdpConnectContext<'_>,
) -> String {
    match key {
        "hostname" | "host" => launch.host.clone(),
        "port" => launch.port.to_string(),
        "username" => launch.username.clone().unwrap_or_default(),
        "domain" => launch.domain.clone().unwrap_or_default(),
        "password" => launch.password.clone().unwrap_or_default(),
        "width" => options.geometry.width().to_string(),
        "height" => options.geometry.height().to_string(),
        "dpi" => options.geometry.dpi().to_string(),
        "color-depth" => options
            .color_depth
            .map(|value| value.to_string())
            .unwrap_or_default(),
        "security" => ctx.resolved_security.to_string(),
        // RDP servers typically present self-signed TLS certs. Prefer "works by default" and
        // allow callers to override via payload if they require strict validation.
        "ignore-cert" => options.ignore_cert.to_string(),
        "disable-auth" => options.disable_auth.to_string(),
        "disable-gfx" => options.disable_gfx.to_string(),
        "force-lossless" => options.force_lossless.to_string(),
        // Enforce deterministic dynamic-resize behavior for panel resizing.
        "resize-method" => "display-update".to_string(),
        // Some guacd/freeRDP builds expose this explicit toggle instead of resize-method.
        "enable-display-update" => "true".to_string(),
        // RDPDR drive redirection backs the in-session file browser.
        "enable-drive" => options.enable_drive.to_string(),
        "drive-name" if options.enable_drive => RDP_DRIVE_NAME.to_string(),
        "drive-path" if options.enable_drive => {
            rdp_session_drive_path(ctx.drive_base_path, ctx.connection_id)
        }
        "create-drive-path" if options.enable_drive => "true".to_string(),
        _ => String::new(),
    }
}

async fn forward_ready_to_client(
    socket: &mut axum::extract::ws::WebSocket,
    metrics: &Arc<RwLock<ConnectionMetricsState>>,
    ready: &GuacdReadyInstruction,
) -> Result<(), AppError> {
    let ready_text = String::from_utf8_lossy(ready.raw.as_slice()).to_string();
    socket
        .send(axum::extract::ws::Message::Text(ready_text.into()))
        .await
        .map_err(|error| {
            AppError::External(format!("Client websocket send failed (ready): {}", error))
        })?;
    record_connection_traffic(
        metrics,
        LinkType::ClientProxy,
        TrafficDirection::Rx,
        ready.raw.len(),
    )
    .await;
    Ok(())
}

#[derive(Debug)]
struct GuacdReadyInstruction {
    consumed: usize,
    args: Vec<String>,
    raw: Vec<u8>,
}

async fn wait_for_guacd_ready<R: AsyncRead + Unpin>(
    tcp_reader: &mut R,
    stream_buf: &mut SliceBuffer,
    buffer: &mut [u8],
    metrics: &Arc<RwLock<ConnectionMetricsState>>,
) -> Result<GuacdReadyInstruction, AppError> {
    loop {
        if let Some(ready) = consume_guacd_ready_from_buffer(stream_buf)? {
            record_connection_traffic(
                metrics,
                LinkType::ProxyProtocol,
                TrafficDirection::Rx,
                ready.consumed,
            )
            .await;
            return Ok(ready);
        }

        let read = tcp_reader.read(buffer).await.map_err(|error| {
            AppError::External(format!("guacd TCP read failed (ready): {}", error))
        })?;
        if read == 0 {
            return Err(AppError::External(
                "guacd closed connection before ready".to_string(),
            ));
        }
        stream_buf.extend_from_slice(&buffer[..read]);
    }
}

fn consume_guacd_ready_from_buffer(
    stream_buf: &mut SliceBuffer,
) -> Result<Option<GuacdReadyInstruction>, AppError> {
    let Some((consumed, elements)) = parse_guac_instruction(stream_buf.available())? else {
        return Ok(None);
    };

    let opcode = elements.first().map(String::as_str).unwrap_or_default();
    match opcode {
        "ready" => {
            let raw = stream_buf.take_prefix(consumed).to_vec();
            stream_buf.consume(consumed);
            Ok(Some(GuacdReadyInstruction {
                consumed,
                args: elements.into_iter().skip(1).collect(),
                raw,
            }))
        }
        "error" => {
            stream_buf.consume(consumed);
            Err(AppError::External(
                "guacd returned an error during ready handshake".to_string(),
            ))
        }
        _ => Err(AppError::External(format!(
            "guacd returned unexpected instruction before ready: {opcode}"
        ))),
    }
}

fn trimmed_env_value(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn rdp_drive_base_path() -> String {
    trimmed_env_value(RDP_DRIVE_BASE_ENV).unwrap_or_else(|| DEFAULT_RDP_DRIVE_BASE_PATH.to_string())
}

fn rdp_session_drive_path(base_path: &str, connection_id: Uuid) -> String {
    format!("{}/{}", base_path.trim_end_matches('/'), connection_id)
}

fn embedded_rdp_drive_base_path() -> Option<PathBuf> {
    trimmed_env_value(EMBEDDED_GUACD_DRIVE_BASE_ENV).map(PathBuf::from)
}

fn safe_rdp_session_drive_dir(base_path: &Path, connection_id: Uuid) -> Option<PathBuf> {
    if !base_path.is_absolute() {
        return None;
    }
    let session_dir = base_path.join(connection_id.to_string());
    if session_dir.parent() != Some(base_path) || !session_dir.starts_with(base_path) {
        return None;
    }
    Some(session_dir)
}

async fn cleanup_embedded_rdp_drive(connection_id: Uuid, enable_drive: bool) {
    cleanup_embedded_rdp_drive_at(connection_id, enable_drive, embedded_rdp_drive_base_path())
        .await;
}

async fn cleanup_embedded_rdp_drive_at(
    connection_id: Uuid,
    enable_drive: bool,
    base_path: Option<PathBuf>,
) {
    let Some(base_path) = base_path.filter(|_| enable_drive) else {
        return;
    };
    let Some(session_dir) = safe_rdp_session_drive_dir(base_path.as_path(), connection_id) else {
        log_event(
            LogLevel::Warning,
            "proxy.rdp.drive.cleanup.skipped",
            EventFields::default()
                .with_operation("cleanup_embedded_rdp_drive")
                .with_resource("proxy_connection", connection_id.to_string())
                .with_context("reason", "invalid embedded drive base path")
                .with_context("base_path", base_path.display().to_string()),
        );
        return;
    };

    match tokio::fs::remove_dir_all(&session_dir).await {
        Ok(()) => {
            log_event(
                LogLevel::Debug,
                "proxy.rdp.drive.cleanup.removed",
                EventFields::default()
                    .with_operation("cleanup_embedded_rdp_drive")
                    .with_resource("proxy_connection", connection_id.to_string())
                    .with_context("path", session_dir.display().to_string()),
            );
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            log_event(
                LogLevel::Warning,
                "proxy.rdp.drive.cleanup.failed",
                EventFields::default()
                    .with_operation("cleanup_embedded_rdp_drive")
                    .with_resource("proxy_connection", connection_id.to_string())
                    .with_context("path", session_dir.display().to_string())
                    .with_context("error", error.to_string()),
            );
        }
    }
}

fn parse_positive_u32(value: &Value, field: &str) -> Result<Option<u32>, AppError> {
    match value {
        Value::Null => Ok(None),
        Value::Number(number) => number
            .as_u64()
            .and_then(|parsed| u32::try_from(parsed).ok())
            .filter(|parsed| *parsed > 0)
            .map(Some)
            .ok_or_else(|| AppError::Validation(format!("Invalid {field} value"))),
        _ => Err(AppError::Validation(format!("Invalid {field} value"))),
    }
}

fn parse_bool(value: &Value, field: &str) -> Result<Option<bool>, AppError> {
    match value {
        Value::Null => Ok(None),
        Value::Bool(parsed) => Ok(Some(*parsed)),
        _ => Err(AppError::Validation(format!("Invalid {field} value"))),
    }
}

fn parse_rdp_security(value: &Value) -> Result<Option<String>, AppError> {
    match value {
        Value::Null => Ok(None),
        Value::String(raw) => {
            let normalized = raw.trim().to_ascii_lowercase();
            if normalized.is_empty() {
                return Ok(None);
            }
            match normalized.as_str() {
                "any" | "nla" | "rdp" | "tls" => Ok(Some(normalized)),
                _ => Err(AppError::Validation(
                    "Invalid security value; expected any|nla|rdp|tls".to_string(),
                )),
            }
        }
        _ => Err(AppError::Validation(
            "Invalid security value; expected any|nla|rdp|tls".to_string(),
        )),
    }
}

fn resolve_rdp_security_mode(explicit: Option<&str>) -> String {
    explicit.unwrap_or("any").to_string()
}

fn extract_optional_value<'a>(map: Option<&'a Map<String, Value>>, key: &str) -> Option<&'a Value> {
    map.and_then(|parsed| parsed.get(key))
}

fn extract_rdp_client_options(payload: &Value) -> Result<RdpClientOptions, AppError> {
    let payload_map = payload.as_object();
    let options_map = payload_map
        .and_then(|map| map.get("options"))
        .and_then(Value::as_object);

    let width = match extract_optional_value(options_map, "width") {
        Some(value) => parse_positive_u32(value, "options.width")?,
        None => None,
    };
    let height = match extract_optional_value(options_map, "height") {
        Some(value) => parse_positive_u32(value, "options.height")?,
        None => None,
    };
    let width = width.unwrap_or(DEFAULT_DESKTOP_WIDTH);
    let height = height.unwrap_or(DEFAULT_DESKTOP_HEIGHT);
    let dpi = match extract_optional_value(options_map, "dpi") {
        Some(value) => parse_positive_u32(value, "options.dpi")?,
        None => None,
    }
    .unwrap_or(DEFAULT_DESKTOP_DPI);
    let geometry = DesktopGeometry::new(width, height, dpi)?;
    let color_depth = match extract_optional_value(options_map, "color_depth") {
        Some(value) => match parse_positive_u32(value, "options.color_depth")? {
            Some(depth) if matches!(depth, 8 | 16 | 24 | 32) => Some(depth),
            Some(_) => {
                return Err(AppError::Validation(
                    "Invalid options.color_depth value".to_string(),
                ));
            }
            None => None,
        },
        None => None,
    };
    // RDP servers typically present self-signed TLS certs. Prefer "works by default" and
    // allow callers to override via payload if they require strict validation.
    let ignore_cert = match extract_optional_value(options_map, "ignore_cert") {
        Some(value) => parse_bool(value, "options.ignore_cert")?,
        None => None,
    }
    .unwrap_or(true);
    let disable_auth = match extract_optional_value(options_map, "disable_auth") {
        Some(value) => parse_bool(value, "options.disable_auth")?,
        None => None,
    }
    .unwrap_or(false);
    // Some RDP servers terminate sessions when GFX pipeline is negotiated.
    // Default to disabled for stable interoperability unless explicitly enabled.
    let disable_gfx = match extract_optional_value(options_map, "disable_gfx") {
        Some(value) => parse_bool(value, "options.disable_gfx")?,
        None => None,
    }
    .unwrap_or(true);
    let force_lossless = match extract_optional_value(options_map, "force_lossless") {
        Some(value) => parse_bool(value, "options.force_lossless")?,
        None => None,
    }
    .unwrap_or(false);
    // Security mode is optional in payload. If omitted, use `any` to let
    // guacd/FreeRDP negotiate the best mutually supported mode.
    let security = match extract_optional_value(options_map, "security") {
        Some(value) => parse_rdp_security(value)?,
        None => None,
    };
    // RDP file tools are backed by drive redirection. Enable the virtual drive
    // by default so browser sessions expose the file browser; callers can still
    // opt out with `options.enable_drive=false` for problematic servers.
    let enable_drive = match extract_optional_value(options_map, "enable_drive") {
        Some(value) => parse_bool(value, "options.enable_drive")?,
        None => None,
    }
    .unwrap_or(true);

    Ok(RdpClientOptions {
        geometry,
        color_depth,
        ignore_cert,
        disable_auth,
        disable_gfx,
        force_lossless,
        security,
        enable_drive,
    })
}

#[derive(Debug, Clone, Copy, Default)]
struct VncClientOptions {
    color_depth: Option<u32>,
    force_lossless: bool,
}

fn extract_vnc_client_options(payload: &Value) -> Result<VncClientOptions, AppError> {
    let options_map = payload
        .as_object()
        .and_then(|map| map.get("options"))
        .and_then(Value::as_object);
    let color_depth = match extract_optional_value(options_map, "color_depth") {
        Some(value) => match parse_positive_u32(value, "options.color_depth")? {
            Some(depth) if matches!(depth, 8 | 16 | 24 | 32) => Some(depth),
            Some(_) => {
                return Err(AppError::Validation(
                    "Invalid options.color_depth value".to_string(),
                ));
            }
            None => None,
        },
        None => None,
    };
    let force_lossless = match extract_optional_value(options_map, "force_lossless") {
        Some(value) => parse_bool(value, "options.force_lossless")?,
        None => None,
    }
    .unwrap_or(false);
    Ok(VncClientOptions {
        color_depth,
        force_lossless,
    })
}

fn classify_tcp_connect_error(error: &std::io::Error) -> (u16, &'static str) {
    match error.kind() {
        std::io::ErrorKind::ConnectionRefused => {
            return (
                GUAC_STATUS_UPSTREAM_UNAVAILABLE,
                "Target connection refused",
            );
        }
        std::io::ErrorKind::TimedOut => {
            return (GUAC_STATUS_UPSTREAM_TIMEOUT, "Target connection timed out");
        }
        _ => {}
    }

    let message = error.to_string().to_ascii_lowercase();
    if message.contains("connection refused") {
        return (
            GUAC_STATUS_UPSTREAM_UNAVAILABLE,
            "Target connection refused",
        );
    }
    if message.contains("timed out") || message.contains("timeout") {
        return (GUAC_STATUS_UPSTREAM_TIMEOUT, "Target connection timed out");
    }
    if message.contains("no route to host")
        || message.contains("network is unreachable")
        || message.contains("is unreachable")
        || message.contains("could not resolve hostname")
        || message.contains("name or service not known")
        || message.contains("temporary failure in name resolution")
    {
        return (GUAC_STATUS_UPSTREAM_UNAVAILABLE, "Target is unreachable");
    }

    (GUAC_STATUS_UPSTREAM_ERROR, "Target connection failed")
}

fn extract_rdp_launch_config(payload: &Value) -> Result<RdpLaunchConfig, &'static str> {
    let (host, port) = super::extract_host_port(payload)?;
    let credentials = payload
        .as_object()
        .and_then(|map| map.get("credentials"))
        .and_then(Value::as_object);

    // Canonical credentials path (constitution 3.11): credentials are nested under `credentials`.
    let username = credentials
        .and_then(|map| map.get("username"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());

    let domain = credentials
        .and_then(|map| map.get("domain"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());

    let (username, domain) = normalize_rdp_username_domain(username, domain)?;

    let password = credentials
        .and_then(|map| map.get("password"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());

    Ok(RdpLaunchConfig {
        host,
        port,
        username,
        domain,
        password,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        DesktopGeometry, GuacClientCapabilities, IMAGE_MIMETYPES, NO_MIMETYPES,
        RdpTargetProbeFailure, cleanup_embedded_rdp_drive_at, consume_guacd_ready_from_buffer,
        extract_rdp_client_options, extract_rdp_launch_config, probe_rdp_target_latency,
        rdp_session_drive_path, resolve_rdp_security_mode, safe_rdp_session_drive_dir,
        send_client_handshake,
    };
    use crate::errors::AppError;
    use crate::proxy::guac::{SliceBuffer, encode_guac_instruction, parse_guac_instruction};
    use crate::proxy::metrics::ConnectionMetricsState;
    use serde_json::json;
    use std::sync::Arc;
    use std::{fs, time::Duration};
    use tempfile::tempdir;
    use tokio::net::TcpListener;
    use tokio::sync::RwLock;
    use uuid::Uuid;

    const TEST_HOST: &str = "10.10.1.230";
    const TEST_PORT: u16 = 3389;
    const TEST_HD_WIDTH: u32 = 1920;
    const TEST_HD_HEIGHT: u32 = 1080;
    const TEST_HIGH_DPI: u32 = 120;
    const TEST_COLOR_DEPTH: u32 = 24;

    fn assert_validation_error_contains<T: std::fmt::Debug>(
        result: Result<T, AppError>,
        needle: &str,
    ) {
        assert!(
            matches!(
                &result,
                Err(AppError::Validation(message)) if message.contains(needle)
            ),
            "unexpected result: {result:?}"
        );
    }

    #[tokio::test]
    async fn probe_rdp_target_latency_reports_successful_tcp_probe() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test listener");
        let port = listener.local_addr().expect("listener addr").port();
        let accept_task = tokio::spawn(async move {
            let _ = listener.accept().await;
        });

        let latency_ms = probe_rdp_target_latency("127.0.0.1", port, Duration::from_secs(1))
            .await
            .expect("probe should connect");

        assert!(latency_ms >= 1);
        accept_task.await.expect("accept task should finish");
    }

    #[tokio::test]
    async fn probe_rdp_target_latency_reports_connect_failure() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test listener");
        let port = listener.local_addr().expect("listener addr").port();
        drop(listener);

        let result = probe_rdp_target_latency("127.0.0.1", port, Duration::from_secs(1)).await;

        assert!(
            matches!(result, Err(RdpTargetProbeFailure::Connect(_))),
            "unexpected probe result: {result:?}"
        );
    }

    #[test]
    fn extract_rdp_launch_config_reads_nested_credentials() {
        let payload = json!({
            "connection_type": "rdp",
            "host": TEST_HOST,
            "port": TEST_PORT,
            "credentials": {
                "username": "operator",
                "domain": "GRAD",
                "password": "secret"
            }
        });

        let launch = extract_rdp_launch_config(&payload);
        assert!(launch.is_ok(), "launch config should be valid: {launch:?}");
        assert_eq!(
            launch
                .as_ref()
                .ok()
                .and_then(|value| value.username.as_deref()),
            Some("operator")
        );
        assert_eq!(
            launch
                .as_ref()
                .ok()
                .and_then(|value| value.domain.as_deref()),
            Some("GRAD")
        );
        assert_eq!(
            launch
                .as_ref()
                .ok()
                .and_then(|value| value.password.as_deref()),
            Some("secret")
        );
    }

    #[test]
    fn extract_rdp_launch_config_keeps_upn_username_without_domain() {
        let payload = json!({
            "connection_type": "rdp",
            "host": TEST_HOST,
            "port": TEST_PORT,
            "credentials": {
                "username": "operator@example.test"
            }
        });

        let launch = extract_rdp_launch_config(&payload);
        assert!(launch.is_ok(), "launch config should be valid: {launch:?}");
        assert_eq!(
            launch
                .as_ref()
                .ok()
                .and_then(|value| value.username.as_deref()),
            Some("operator@example.test")
        );
        assert_eq!(
            launch
                .as_ref()
                .ok()
                .and_then(|value| value.domain.as_deref()),
            None
        );
    }

    #[test]
    fn extract_rdp_launch_config_splits_backslash_domain_username() {
        let payload = json!({
            "connection_type": "rdp",
            "host": TEST_HOST,
            "port": TEST_PORT,
            "credentials": {
                "username": "GRAD\\operator"
            }
        });

        let launch = extract_rdp_launch_config(&payload);
        assert!(launch.is_ok(), "launch config should be valid: {launch:?}");
        assert_eq!(
            launch
                .as_ref()
                .ok()
                .and_then(|value| value.username.as_deref()),
            Some("operator")
        );
        assert_eq!(
            launch
                .as_ref()
                .ok()
                .and_then(|value| value.domain.as_deref()),
            Some("GRAD")
        );
    }

    #[test]
    fn extract_rdp_launch_config_rejects_conflicting_explicit_domain() {
        let payload = json!({
            "connection_type": "rdp",
            "host": TEST_HOST,
            "port": TEST_PORT,
            "credentials": {
                "username": "GRAD\\operator",
                "domain": "OTHER"
            }
        });

        let launch = extract_rdp_launch_config(&payload);
        assert!(
            matches!(launch, Err(message) if message.contains("conflicts")),
            "conflicting domain should be rejected: {launch:?}"
        );
    }

    #[test]
    fn extract_rdp_launch_config_ignores_removed_aliases() {
        let payload = json!({
            "connection_type": "rdp",
            "host": TEST_HOST,
            "port": TEST_PORT,
            "credentials": {
                "rdp_username": "removed-user"
            },
            "pass": "removed-pass"
        });

        let launch = extract_rdp_launch_config(&payload);
        assert!(launch.is_ok(), "launch config should be valid: {launch:?}");
        assert_eq!(
            launch
                .as_ref()
                .ok()
                .and_then(|value| value.username.clone()),
            None
        );
        assert_eq!(
            launch
                .as_ref()
                .ok()
                .and_then(|value| value.password.clone()),
            None
        );
    }

    #[test]
    fn extract_rdp_client_options_uses_canonical_values() {
        let payload = json!({
            "connection_type": "rdp",
            "host": TEST_HOST,
            "port": TEST_PORT,
            "options": {
                "width": TEST_HD_WIDTH,
                "height": TEST_HD_HEIGHT,
                "dpi": TEST_HIGH_DPI,
                "ignore_cert": false,
                "disable_auth": true,
                "disable_gfx": false,
                "security": "nla",
                "color_depth": TEST_COLOR_DEPTH,
                "force_lossless": true,
                "enable_drive": false
            }
        });

        let options = extract_rdp_client_options(&payload);
        assert!(options.is_ok(), "options should be valid: {options:?}");
        assert_eq!(
            options.as_ref().ok().map(|value| value.geometry.width()),
            Some(TEST_HD_WIDTH)
        );
        assert_eq!(
            options.as_ref().ok().map(|value| value.geometry.height()),
            Some(TEST_HD_HEIGHT)
        );
        assert_eq!(
            options.as_ref().ok().map(|value| value.geometry.dpi()),
            Some(TEST_HIGH_DPI)
        );
        assert_eq!(
            options.as_ref().ok().and_then(|value| value.color_depth),
            Some(TEST_COLOR_DEPTH)
        );
        assert_eq!(
            options.as_ref().ok().map(|value| value.force_lossless),
            Some(true)
        );
        assert_eq!(
            options.as_ref().ok().map(|value| value.enable_drive),
            Some(false)
        );
        assert_eq!(
            options.as_ref().ok().map(|value| value.ignore_cert),
            Some(false)
        );
        assert_eq!(
            options.as_ref().ok().map(|value| value.disable_auth),
            Some(true)
        );
        assert_eq!(
            options.as_ref().ok().map(|value| value.disable_gfx),
            Some(false)
        );
        assert_eq!(
            options
                .as_ref()
                .ok()
                .and_then(|value| value.security.as_deref()),
            Some("nla")
        );
    }

    #[test]
    fn extract_rdp_client_options_uses_defaults_without_explicit_size() {
        let payload = json!({
            "connection_type": "rdp",
            "host": TEST_HOST,
            "port": TEST_PORT
        });

        let options = extract_rdp_client_options(&payload);
        assert!(options.is_ok(), "options should be valid: {options:?}");
        assert_eq!(
            options.as_ref().ok().map(|value| value.geometry.width()),
            Some(super::DEFAULT_DESKTOP_WIDTH)
        );
        assert_eq!(
            options.as_ref().ok().map(|value| value.geometry.height()),
            Some(super::DEFAULT_DESKTOP_HEIGHT)
        );
        assert_eq!(
            options.as_ref().ok().map(|value| value.geometry.dpi()),
            Some(super::DEFAULT_DESKTOP_DPI)
        );
        assert_eq!(
            options.as_ref().ok().and_then(|value| value.color_depth),
            None
        );
        assert_eq!(
            options.as_ref().ok().map(|value| value.ignore_cert),
            Some(true)
        );
        assert_eq!(
            options.as_ref().ok().map(|value| value.disable_auth),
            Some(false)
        );
        assert_eq!(
            options.as_ref().ok().map(|value| value.disable_gfx),
            Some(true)
        );
        assert_eq!(
            options.as_ref().ok().map(|value| value.enable_drive),
            Some(true)
        );
        assert_eq!(
            options
                .as_ref()
                .ok()
                .and_then(|value| value.security.as_deref()),
            None
        );
    }

    #[test]
    fn extract_rdp_client_options_accepts_explicit_drive_enable() {
        let payload = json!({
            "connection_type": "rdp",
            "host": TEST_HOST,
            "port": TEST_PORT,
            "options": {
                "enable_drive": true
            }
        });

        let options = extract_rdp_client_options(&payload);
        assert!(options.is_ok(), "options should be valid: {options:?}");
        assert_eq!(
            options.as_ref().ok().map(|value| value.enable_drive),
            Some(true)
        );
    }

    #[test]
    fn extract_rdp_client_options_rejects_invalid_bool_types() {
        let payload = json!({
            "connection_type": "rdp",
            "host": TEST_HOST,
            "port": TEST_PORT,
            "options": {
                "ignore_cert": "true"
            }
        });

        let result = extract_rdp_client_options(&payload);
        assert_validation_error_contains(result, "options.ignore_cert");
    }

    #[test]
    fn extract_rdp_client_options_rejects_unknown_security_value() {
        let payload = json!({
            "connection_type": "rdp",
            "host": TEST_HOST,
            "port": TEST_PORT,
            "options": {
                "security": "foo"
            }
        });

        let result = extract_rdp_client_options(&payload);
        assert_validation_error_contains(result, "security");
    }

    #[test]
    fn extract_rdp_client_options_accepts_any_security_value() {
        let payload = json!({
            "connection_type": "rdp",
            "host": TEST_HOST,
            "port": TEST_PORT,
            "options": {
                "security": "any"
            }
        });

        let options = extract_rdp_client_options(&payload);
        assert!(options.is_ok(), "security=any should be valid: {options:?}");
        assert_eq!(
            options
                .as_ref()
                .ok()
                .and_then(|value| value.security.as_deref()),
            Some("any")
        );
    }

    #[test]
    fn resolve_rdp_security_mode_defaults_to_any() {
        assert_eq!(resolve_rdp_security_mode(None), "any");
    }

    #[test]
    fn resolve_rdp_security_mode_keeps_explicit_value() {
        assert_eq!(resolve_rdp_security_mode(Some("nla")), "nla");
    }

    #[test]
    fn desktop_geometry_rejects_zero_dimensions_and_dpi() {
        for (width, height, dpi) in [(0, 768, 96), (1024, 0, 96), (1024, 768, 0)] {
            assert!(
                matches!(
                    DesktopGeometry::new(width, height, dpi),
                    Err(AppError::Validation(_))
                ),
                "{width}x{height}@{dpi} must be rejected"
            );
        }
        assert_eq!(
            DesktopGeometry::new(1024, 768, 96).expect("valid geometry"),
            DesktopGeometry::default_desktop()
        );
    }

    #[tokio::test]
    async fn capability_handshake_initializes_media_lists_before_connect() {
        let metrics = Arc::new(RwLock::new(ConnectionMetricsState::default()));
        let capabilities = GuacClientCapabilities {
            size: DesktopGeometry::new(1280, 720, 96).expect("geometry"),
            audio: NO_MIMETYPES,
            video: NO_MIMETYPES,
            image: IMAGE_MIMETYPES,
            timezone: Some("Etc/UTC"),
        };
        let connect = vec!["connect".to_string(), "VERSION_1_6_0".to_string()];
        let mut out = Vec::new();
        send_client_handshake(&mut out, &metrics, &capabilities, &connect)
            .await
            .expect("serialized handshake");

        let instructions = parse_instructions(&out);
        assert_eq!(
            instructions[0],
            vec![
                "size".to_string(),
                "1280".to_string(),
                "720".to_string(),
                "96".to_string()
            ]
        );
        assert_eq!(instructions[1], vec!["audio".to_string()]);
        assert_eq!(instructions[2], vec!["video".to_string()]);
        assert_eq!(
            instructions[3],
            vec![
                "image".to_string(),
                "image/png".to_string(),
                "image/jpeg".to_string()
            ]
        );
        assert_eq!(
            instructions[4],
            vec!["timezone".to_string(), "Etc/UTC".to_string()]
        );
        assert_eq!(instructions[5][0], "connect");
    }

    fn parse_instructions(bytes: &[u8]) -> Vec<Vec<String>> {
        let mut instructions = Vec::new();
        let mut offset = 0;
        while offset < bytes.len() {
            let (consumed, elements) = parse_guac_instruction(&bytes[offset..])
                .expect("valid instruction")
                .expect("complete instruction");
            offset += consumed;
            instructions.push(elements);
        }
        instructions
    }

    #[test]
    fn consume_guacd_ready_returns_raw_and_leaves_post_ready_instruction_buffered() {
        let ready = encode_guac_instruction(&["ready".to_string(), "$connection".to_string()]);
        let filesystem = encode_guac_instruction(&[
            "filesystem".to_string(),
            "0".to_string(),
            "Shared Drive".to_string(),
        ]);
        let mut buffer = SliceBuffer::new();
        buffer.extend_from_slice(&ready);
        buffer.extend_from_slice(&filesystem);

        let result = consume_guacd_ready_from_buffer(&mut buffer);

        assert!(result.is_ok(), "ready should be consumed: {result:?}");
        let Some(consumed) = result.ok().flatten() else {
            panic!("ready instruction should be available");
        };
        assert_eq!(consumed.consumed, ready.len());
        assert_eq!(consumed.args, vec!["$connection".to_string()]);
        assert_eq!(consumed.raw, ready);

        let remaining = parse_guac_instruction(buffer.available());
        assert!(
            matches!(
                remaining,
                Ok(Some((_remaining_consumed, ref elements)))
                    if *elements == vec![
                        "filesystem".to_string(),
                        "0".to_string(),
                        "Shared Drive".to_string()
                    ]
            ),
            "post-ready instruction should remain buffered: {remaining:?}"
        );
    }

    #[test]
    fn consume_guacd_ready_waits_for_partial_instruction() {
        let mut buffer = SliceBuffer::new();
        buffer.extend_from_slice(b"5.read");

        let result = consume_guacd_ready_from_buffer(&mut buffer);

        assert!(matches!(result, Ok(None)));
        assert_eq!(buffer.available(), b"5.read");
    }

    #[test]
    fn consume_guacd_ready_rejects_unexpected_instruction() {
        let sync = encode_guac_instruction(&["sync".to_string(), "1".to_string()]);
        let mut buffer = SliceBuffer::new();
        buffer.extend_from_slice(&sync);

        let result = consume_guacd_ready_from_buffer(&mut buffer);

        assert!(
            matches!(
                result,
                Err(AppError::External(ref message))
                    if message.contains("unexpected instruction before ready")
            ),
            "unexpected instruction should fail the ready handshake: {result:?}"
        );
    }

    #[test]
    fn consume_guacd_ready_rejects_guacd_error() {
        let error = encode_guac_instruction(&[
            "error".to_string(),
            "upstream failed".to_string(),
            "512".to_string(),
        ]);
        let mut buffer = SliceBuffer::new();
        buffer.extend_from_slice(&error);

        let result = consume_guacd_ready_from_buffer(&mut buffer);

        assert!(
            matches!(
                result,
                Err(AppError::External(ref message))
                    if message.contains("error during ready handshake")
            ),
            "guacd error should fail the ready handshake: {result:?}"
        );
        assert!(
            buffer.available().is_empty(),
            "error instruction should be consumed"
        );
    }

    #[test]
    fn rdp_session_drive_path_trims_base_slash() {
        let connection_id = Uuid::now_v7();

        assert_eq!(
            rdp_session_drive_path("/tmp/guac-drives/", connection_id),
            format!("/tmp/guac-drives/{connection_id}")
        );
    }

    #[test]
    fn safe_rdp_session_drive_dir_requires_absolute_base() {
        let connection_id = Uuid::now_v7();

        assert!(
            safe_rdp_session_drive_dir(std::path::Path::new("relative"), connection_id).is_none()
        );
    }

    #[tokio::test]
    async fn cleanup_embedded_rdp_drive_is_noop_without_embedded_base() {
        let temp = tempdir().expect("tempdir");
        let connection_id = Uuid::now_v7();
        let session_dir = temp.path().join(connection_id.to_string());
        fs::create_dir_all(&session_dir).expect("create session dir");

        cleanup_embedded_rdp_drive_at(connection_id, true, None).await;

        assert!(
            session_dir.exists(),
            "non-embedded cleanup must not remove dirs"
        );
    }

    #[tokio::test]
    async fn cleanup_embedded_rdp_drive_removes_only_session_dir() {
        let temp = tempdir().expect("tempdir");
        let connection_id = Uuid::now_v7();
        let session_dir = temp.path().join(connection_id.to_string());
        let other_dir = temp.path().join(Uuid::now_v7().to_string());
        fs::create_dir_all(&session_dir).expect("create session dir");
        fs::write(session_dir.join("file.txt"), b"session").expect("write session file");
        fs::create_dir_all(&other_dir).expect("create other dir");
        fs::write(other_dir.join("file.txt"), b"other").expect("write other file");

        cleanup_embedded_rdp_drive_at(connection_id, true, Some(temp.path().to_path_buf())).await;

        assert!(!session_dir.exists(), "session drive dir should be removed");
        assert!(other_dir.exists(), "other session dirs must be preserved");
    }

    #[tokio::test]
    async fn cleanup_embedded_rdp_drive_is_noop_when_drive_disabled() {
        let temp = tempdir().expect("tempdir");
        let connection_id = Uuid::now_v7();
        let session_dir = temp.path().join(connection_id.to_string());
        fs::create_dir_all(&session_dir).expect("create session dir");

        cleanup_embedded_rdp_drive_at(connection_id, false, Some(temp.path().to_path_buf())).await;

        assert!(
            session_dir.exists(),
            "disabled drive cleanup must not remove dirs"
        );
    }
}
