use std::{sync::Arc, time::Duration};

use des::Des;
use des::cipher::{Block, BlockEncrypt, KeyInit};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{Mutex, RwLock};
use tokio::time::timeout;
use uuid::Uuid;

use crate::errors::AppError;

use super::metrics::ConnectionMetricsState;
use super::{
    AgentVncCapture, AgentVncCaptureFormat, AgentVncKeyEvent, AgentVncPointerEvent,
    ProxyConnectionType, ProxyState,
};

const RFB_VERSION_LEN: usize = 12;
const RFB_VERSION_38: &[u8; RFB_VERSION_LEN] = b"RFB 003.008\n";
const RFB_SECURITY_NONE: u8 = 1;
const RFB_SECURITY_VNC_AUTH: u8 = 2;
const RFB_ENCODING_RAW: i32 = 0;
const RFB_IO_TIMEOUT: Duration = Duration::from_secs(30);
const RFB_MAX_CAPTURE_PIXELS: usize = 1920 * 1200;
const RFB_MAX_SOURCE_CAPTURE_PIXELS: usize = 4096 * 4096;
const RFB_MAX_CAPTURE_IMAGE_BYTES: usize = 8 * 1024 * 1024;
const RFB_MAX_NAME_BYTES: usize = 64 * 1024;
const VNC_TEXT_MAX_BYTES: usize = 4096;

pub(super) async fn proxy_ws_vnc_loop(
    connection_id: Uuid,
    socket: axum::extract::ws::WebSocket,
    payload: &Value,
    metrics: Arc<RwLock<ConnectionMetricsState>>,
) {
    super::tcp_relay::proxy_ws_tcp_relay_loop(connection_id, socket, payload, metrics).await;
}

pub(super) async fn agent_vnc_capture(
    state: Arc<ProxyState>,
    connection_id: Uuid,
    format: AgentVncCaptureFormat,
) -> Result<AgentVncCapture, AppError> {
    let session = agent_vnc_session(state.clone(), connection_id).await?;
    let result = {
        let mut client = session.lock().await;
        async {
            client.set_pixel_format_raw_bgra().await?;
            client.set_encodings_raw().await?;
            client.capture(format).await
        }
        .await
    };
    if should_drop_cached_vnc_session(&result) {
        drop_cached_vnc_session(&state, connection_id).await;
    }
    result
}

pub(super) async fn agent_vnc_pointer_events(
    state: Arc<ProxyState>,
    connection_id: Uuid,
    events: Vec<AgentVncPointerEvent>,
) -> Result<(), AppError> {
    if events.is_empty() {
        return Err(AppError::Validation(
            "VNC pointer event list must not be empty".to_string(),
        ));
    }
    let session = agent_vnc_session(state.clone(), connection_id).await?;
    let result = {
        let mut client = session.lock().await;
        async {
            for event in events {
                client.pointer_event(event).await?;
            }
            Ok(())
        }
        .await
    };
    if should_drop_cached_vnc_session(&result) {
        drop_cached_vnc_session(&state, connection_id).await;
    }
    result
}

pub(super) async fn agent_vnc_key_events(
    state: Arc<ProxyState>,
    connection_id: Uuid,
    events: Vec<AgentVncKeyEvent>,
) -> Result<(), AppError> {
    if events.is_empty() {
        return Err(AppError::Validation(
            "VNC key event list must not be empty".to_string(),
        ));
    }
    let session = agent_vnc_session(state.clone(), connection_id).await?;
    let result = {
        let mut client = session.lock().await;
        async {
            for event in events {
                client.key_event(event).await?;
            }
            Ok(())
        }
        .await
    };
    if should_drop_cached_vnc_session(&result) {
        drop_cached_vnc_session(&state, connection_id).await;
    }
    result
}

pub(super) async fn agent_vnc_text(
    state: Arc<ProxyState>,
    connection_id: Uuid,
    text: String,
) -> Result<(), AppError> {
    if text.len() > VNC_TEXT_MAX_BYTES {
        return Err(AppError::Validation(
            "VNC text input is too large".to_string(),
        ));
    }
    if !text.is_ascii() {
        return Err(AppError::Validation(
            "VNC text input currently supports ASCII only".to_string(),
        ));
    }

    let session = agent_vnc_session(state.clone(), connection_id).await?;
    let result = {
        let mut client = session.lock().await;
        async {
            for ch in text.chars() {
                let keysym = keysym_for_ascii(ch)?;
                client
                    .key_event(AgentVncKeyEvent { keysym, down: true })
                    .await?;
                client
                    .key_event(AgentVncKeyEvent {
                        keysym,
                        down: false,
                    })
                    .await?;
            }
            Ok(())
        }
        .await
    };
    if should_drop_cached_vnc_session(&result) {
        drop_cached_vnc_session(&state, connection_id).await;
    }
    result
}

pub(super) async fn agent_vnc_session(
    state: Arc<ProxyState>,
    connection_id: Uuid,
) -> Result<Arc<Mutex<RfbClient>>, AppError> {
    if let Some(session) = state.vnc_sessions.read().await.get(&connection_id).cloned() {
        return Ok(session);
    }

    let init_lock = vnc_session_init_lock(&state, connection_id).await;
    let _init_guard = init_lock.lock().await;

    if let Some(session) = state.vnc_sessions.read().await.get(&connection_id).cloned() {
        return Ok(session);
    }

    let client = RfbClient::connect_from_proxy_state(state.clone(), connection_id).await?;
    let session = Arc::new(Mutex::new(client));
    state
        .vnc_sessions
        .write()
        .await
        .insert(connection_id, session.clone());
    Ok(session)
}

pub(super) async fn vnc_session_init_lock(
    state: &Arc<ProxyState>,
    connection_id: Uuid,
) -> Arc<Mutex<()>> {
    let mut guard = state.vnc_session_init_locks.write().await;
    guard
        .entry(connection_id)
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

async fn drop_cached_vnc_session(state: &Arc<ProxyState>, connection_id: Uuid) {
    state.vnc_sessions.write().await.remove(&connection_id);
}

fn should_drop_cached_vnc_session<T>(result: &Result<T, AppError>) -> bool {
    matches!(
        result,
        Err(AppError::External(_)) | Err(AppError::NotFound(_))
    )
}

pub(super) struct RfbClient {
    stream: TcpStream,
    width: u16,
    height: u16,
    io_timeout: Duration,
}

impl RfbClient {
    async fn connect_from_proxy_state(
        state: Arc<ProxyState>,
        connection_id: Uuid,
    ) -> Result<Self, AppError> {
        let entry = {
            let guard = state.connections.read().await;
            guard.get(&connection_id).cloned()
        }
        .ok_or_else(|| AppError::NotFound("Proxy connection not found".to_string()))?;

        match super::parse_connection_type(&entry.payload) {
            Ok(ProxyConnectionType::Vnc) => {}
            Ok(ProxyConnectionType::Rdp) => {
                return Err(AppError::Validation(
                    "RDP visual observe-act over MCP is not supported yet; use the VNC adapter or wait for a guacd capture adapter".to_string(),
                ));
            }
            Ok(_) => {
                return Err(AppError::Validation(
                    "MCP visual observe-act is only supported for VNC sessions".to_string(),
                ));
            }
            Err(error) => return Err(AppError::Validation(error.to_string())),
        }

        let (host, port) = super::extract_host_port(&entry.payload)
            .map_err(|error| AppError::Validation(error.to_string()))?;
        let connect_timeout = super::extract_connect_timeout_duration(&entry.payload)
            .map_err(|error| AppError::Validation(error.to_string()))?;
        let password = vnc_password(&entry.payload);
        let stream = timeout(connect_timeout, TcpStream::connect((host.as_str(), port)))
            .await
            .map_err(|_| {
                AppError::External(format!("Timed out connecting to VNC target {host}:{port}"))
            })?
            .map_err(|error| {
                AppError::External(format!(
                    "Failed to connect to VNC target {host}:{port}: {error}"
                ))
            })?;
        super::configure_interactive_tcp_stream(&stream);

        let mut client = Self {
            stream,
            width: 0,
            height: 0,
            io_timeout: RFB_IO_TIMEOUT,
        };
        client.handshake(password.as_deref()).await?;
        Ok(client)
    }

    async fn handshake(&mut self, password: Option<&str>) -> Result<(), AppError> {
        let mut server_version = [0_u8; RFB_VERSION_LEN];
        self.read_exact_timeout(&mut server_version, "VNC protocol version")
            .await?;
        if !server_version.starts_with(b"RFB ") || server_version[RFB_VERSION_LEN - 1] != b'\n' {
            return Err(AppError::External(
                "VNC server returned an invalid RFB protocol version".to_string(),
            ));
        }
        self.write_all_timeout(RFB_VERSION_38, "VNC protocol version")
            .await?;

        let mut count = [0_u8; 1];
        self.read_exact_timeout(&mut count, "VNC security type count")
            .await?;
        if count[0] == 0 {
            let reason = self.read_security_failure_reason().await?;
            return Err(AppError::External(format!(
                "VNC server reported no security types: {reason}"
            )));
        }

        let mut security_types = vec![0_u8; usize::from(count[0])];
        self.read_exact_timeout(&mut security_types, "VNC security types")
            .await?;
        let selected = select_security_type(&security_types, password)?;
        self.write_all_timeout(&[selected], "VNC security type selection")
            .await?;

        if selected == RFB_SECURITY_VNC_AUTH {
            let pass = password.ok_or_else(|| {
                AppError::Validation("VNC password is required for VNC authentication".to_string())
            })?;
            let mut challenge = [0_u8; 16];
            self.read_exact_timeout(&mut challenge, "VNC auth challenge")
                .await?;
            let response = vnc_auth_response(&challenge, pass)?;
            self.write_all_timeout(&response, "VNC auth response")
                .await?;
        }

        let security_result = self.read_u32().await?;
        if security_result != 0 {
            let reason = self.read_security_failure_reason().await?;
            return Err(AppError::External(format!(
                "VNC authentication failed: {reason}"
            )));
        }

        self.write_all_timeout(&[1], "VNC ClientInit").await?;
        self.read_server_init().await
    }

    async fn read_server_init(&mut self) -> Result<(), AppError> {
        self.width = self.read_u16().await?;
        self.height = self.read_u16().await?;
        let mut pixel_format = [0_u8; 16];
        self.read_exact_timeout(&mut pixel_format, "VNC pixel format")
            .await?;
        let name_len = self.read_u32().await? as usize;
        if name_len > RFB_MAX_NAME_BYTES {
            return Err(AppError::External(
                "VNC server name is too large".to_string(),
            ));
        }
        let mut name = vec![0_u8; name_len];
        self.read_exact_timeout(&mut name, "VNC server name")
            .await?;
        if self.width == 0 || self.height == 0 {
            return Err(AppError::External(
                "VNC server reported an empty framebuffer".to_string(),
            ));
        }
        Ok(())
    }

    async fn set_pixel_format_raw_bgra(&mut self) -> Result<(), AppError> {
        let mut message = Vec::with_capacity(20);
        message.extend_from_slice(&[0, 0, 0, 0]);
        message.push(32);
        message.push(24);
        message.push(0);
        message.push(1);
        message.extend_from_slice(&255_u16.to_be_bytes());
        message.extend_from_slice(&255_u16.to_be_bytes());
        message.extend_from_slice(&255_u16.to_be_bytes());
        message.push(16);
        message.push(8);
        message.push(0);
        message.extend_from_slice(&[0, 0, 0]);
        self.write_all_timeout(&message, "VNC SetPixelFormat").await
    }

    async fn set_encodings_raw(&mut self) -> Result<(), AppError> {
        let mut message = Vec::with_capacity(8);
        message.push(2);
        message.push(0);
        message.extend_from_slice(&1_u16.to_be_bytes());
        message.extend_from_slice(&RFB_ENCODING_RAW.to_be_bytes());
        self.write_all_timeout(&message, "VNC SetEncodings").await
    }

    async fn capture(
        &mut self,
        format: AgentVncCaptureFormat,
    ) -> Result<AgentVncCapture, AppError> {
        let rgba = self.capture_framebuffer_rgba().await?;
        let source_width = self.width;
        let source_height = self.height;
        let frame = fit_framebuffer_for_mcp(source_width, source_height, rgba)?;
        let (mime_type, format_label, image_bytes) = match format {
            AgentVncCaptureFormat::Png => (
                "image/png",
                "png",
                encode_png_from_rgba(frame.width, frame.height, &frame.rgba)?,
            ),
            AgentVncCaptureFormat::Bmp => (
                "image/bmp",
                "bmp",
                encode_bmp_from_rgba(frame.width, frame.height, &frame.rgba)?,
            ),
        };
        if image_bytes.len() > RFB_MAX_CAPTURE_IMAGE_BYTES {
            return Err(AppError::External(format!(
                "VNC capture response is too large for MCP: {} bytes exceeds the {RFB_MAX_CAPTURE_IMAGE_BYTES} byte limit",
                image_bytes.len()
            )));
        }
        Ok(AgentVncCapture {
            width: frame.width,
            height: frame.height,
            source_width,
            source_height,
            scaled: frame.scaled,
            mime_type,
            image_bytes,
            format: format_label,
        })
    }

    async fn capture_framebuffer_rgba(&mut self) -> Result<Vec<u8>, AppError> {
        self.request_framebuffer_update(false).await?;
        let pixel_count = validate_source_framebuffer_size(self.width, self.height)?;
        let mut rgba = vec![0_u8; pixel_count * 4];

        loop {
            let message_type = self.read_u8().await?;
            match message_type {
                0 => {
                    let _padding = self.read_u8().await?;
                    let rect_count = self.read_u16().await?;
                    for _ in 0..rect_count {
                        self.read_raw_rect_into(&mut rgba).await?;
                    }
                    return Ok(rgba);
                }
                2 => {}
                3 => {
                    let mut padding = [0_u8; 3];
                    self.read_exact_timeout(&mut padding, "VNC ServerCutText padding")
                        .await?;
                    let len = self.read_u32().await? as usize;
                    if len > RFB_MAX_NAME_BYTES {
                        return Err(AppError::External(
                            "VNC cut-text message is too large".to_string(),
                        ));
                    }
                    let mut buffer = vec![0_u8; len];
                    self.read_exact_timeout(&mut buffer, "VNC ServerCutText")
                        .await?;
                }
                _ => {
                    return Err(AppError::External(format!(
                        "Unsupported VNC server message type {message_type}"
                    )));
                }
            }
        }
    }

    async fn read_raw_rect_into(&mut self, framebuffer: &mut [u8]) -> Result<(), AppError> {
        let x = self.read_u16().await?;
        let y = self.read_u16().await?;
        let width = self.read_u16().await?;
        let height = self.read_u16().await?;
        let encoding = self.read_i32().await?;
        if encoding != RFB_ENCODING_RAW {
            return Err(AppError::External(format!(
                "Unsupported VNC framebuffer encoding {encoding}"
            )));
        }
        if x.saturating_add(width) > self.width || y.saturating_add(height) > self.height {
            return Err(AppError::External(
                "VNC framebuffer rectangle is out of bounds".to_string(),
            ));
        }
        let rect_bytes = usize::from(width)
            .saturating_mul(usize::from(height))
            .saturating_mul(4);
        let mut rect = vec![0_u8; rect_bytes];
        self.read_exact_timeout(&mut rect, "VNC raw framebuffer")
            .await?;

        let screen_width = usize::from(self.width);
        for row in 0..usize::from(height) {
            for col in 0..usize::from(width) {
                let src = (row * usize::from(width) + col) * 4;
                let dst = ((usize::from(y) + row) * screen_width + usize::from(x) + col) * 4;
                framebuffer[dst] = rect[src + 2];
                framebuffer[dst + 1] = rect[src + 1];
                framebuffer[dst + 2] = rect[src];
                framebuffer[dst + 3] = 255;
            }
        }
        Ok(())
    }

    async fn request_framebuffer_update(&mut self, incremental: bool) -> Result<(), AppError> {
        let mut message = Vec::with_capacity(10);
        message.push(3);
        message.push(u8::from(incremental));
        message.extend_from_slice(&0_u16.to_be_bytes());
        message.extend_from_slice(&0_u16.to_be_bytes());
        message.extend_from_slice(&self.width.to_be_bytes());
        message.extend_from_slice(&self.height.to_be_bytes());
        self.write_all_timeout(&message, "VNC FramebufferUpdateRequest")
            .await
    }

    async fn pointer_event(&mut self, event: AgentVncPointerEvent) -> Result<(), AppError> {
        if event.x >= self.width || event.y >= self.height {
            return Err(AppError::Validation(
                "VNC pointer coordinates are outside the framebuffer".to_string(),
            ));
        }
        let mut message = Vec::with_capacity(6);
        message.push(5);
        message.push(event.button_mask);
        message.extend_from_slice(&event.x.to_be_bytes());
        message.extend_from_slice(&event.y.to_be_bytes());
        self.write_all_timeout(&message, "VNC pointer event").await
    }

    async fn key_event(&mut self, event: AgentVncKeyEvent) -> Result<(), AppError> {
        let mut message = Vec::with_capacity(8);
        message.push(4);
        message.push(u8::from(event.down));
        message.extend_from_slice(&[0, 0]);
        message.extend_from_slice(&event.keysym.to_be_bytes());
        self.write_all_timeout(&message, "VNC key event").await
    }

    async fn read_security_failure_reason(&mut self) -> Result<String, AppError> {
        let len = self.read_u32().await.unwrap_or(0) as usize;
        if len == 0 || len > RFB_MAX_NAME_BYTES {
            return Ok("no reason supplied".to_string());
        }
        let mut buffer = vec![0_u8; len];
        self.read_exact_timeout(&mut buffer, "VNC failure reason")
            .await?;
        Ok(String::from_utf8_lossy(&buffer).to_string())
    }

    async fn read_u8(&mut self) -> Result<u8, AppError> {
        let mut buffer = [0_u8; 1];
        self.read_exact_timeout(&mut buffer, "VNC byte").await?;
        Ok(buffer[0])
    }

    async fn read_u16(&mut self) -> Result<u16, AppError> {
        let mut buffer = [0_u8; 2];
        self.read_exact_timeout(&mut buffer, "VNC u16").await?;
        Ok(u16::from_be_bytes(buffer))
    }

    async fn read_u32(&mut self) -> Result<u32, AppError> {
        let mut buffer = [0_u8; 4];
        self.read_exact_timeout(&mut buffer, "VNC u32").await?;
        Ok(u32::from_be_bytes(buffer))
    }

    async fn read_i32(&mut self) -> Result<i32, AppError> {
        let mut buffer = [0_u8; 4];
        self.read_exact_timeout(&mut buffer, "VNC i32").await?;
        Ok(i32::from_be_bytes(buffer))
    }

    async fn read_exact_timeout(
        &mut self,
        buffer: &mut [u8],
        operation: &str,
    ) -> Result<(), AppError> {
        let mut read = 0;
        while read < buffer.len() {
            let bytes = timeout(self.io_timeout, self.stream.read(&mut buffer[read..]))
                .await
                .map_err(|_| AppError::External(format!("Timed out reading {operation}")))?
                .map_err(|error| {
                    AppError::External(format!("Failed to read {operation}: {error}"))
                })?;
            if bytes == 0 {
                return Err(AppError::External(format!(
                    "Unexpected EOF reading {operation}"
                )));
            }
            read += bytes;
        }
        Ok(())
    }

    async fn write_all_timeout(&mut self, buffer: &[u8], operation: &str) -> Result<(), AppError> {
        timeout(self.io_timeout, self.stream.write_all(buffer))
            .await
            .map_err(|_| AppError::External(format!("Timed out writing {operation}")))?
            .map_err(|error| AppError::External(format!("Failed to write {operation}: {error}")))
    }
}

fn vnc_password(payload: &Value) -> Option<String> {
    payload
        .as_object()
        .and_then(|map| map.get("credentials"))
        .and_then(Value::as_object)
        .and_then(|credentials| credentials.get("password"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn select_security_type(security_types: &[u8], password: Option<&str>) -> Result<u8, AppError> {
    if password.is_some() && security_types.contains(&RFB_SECURITY_VNC_AUTH) {
        return Ok(RFB_SECURITY_VNC_AUTH);
    }
    if security_types.contains(&RFB_SECURITY_NONE) {
        return Ok(RFB_SECURITY_NONE);
    }
    if security_types.contains(&RFB_SECURITY_VNC_AUTH) && password.is_none() {
        return Err(AppError::Validation(
            "VNC password is required for VNC authentication".to_string(),
        ));
    }
    Err(AppError::External(format!(
        "Unsupported VNC security types: {security_types:?}"
    )))
}

fn vnc_auth_response(challenge: &[u8; 16], password: &str) -> Result<[u8; 16], AppError> {
    let mut key = [0_u8; 8];
    for (index, byte) in password.as_bytes().iter().take(8).enumerate() {
        key[index] = reverse_bits(*byte);
    }

    let cipher = Des::new_from_slice(&key)
        .map_err(|error| AppError::External(format!("Failed to initialize VNC auth: {error}")))?;
    let mut response = [0_u8; 16];
    response.copy_from_slice(challenge);
    let (blocks, remainder) = response.as_chunks_mut::<8>();
    debug_assert!(
        remainder.is_empty(),
        "the 16-byte VNC challenge is 8-byte aligned"
    );
    for block in blocks {
        let mut encrypted_block = Block::<Des>::default();
        encrypted_block.copy_from_slice(block);
        cipher.encrypt_block(&mut encrypted_block);
        block.copy_from_slice(&encrypted_block);
    }
    Ok(response)
}

fn reverse_bits(byte: u8) -> u8 {
    byte.reverse_bits()
}

fn keysym_for_ascii(ch: char) -> Result<u32, AppError> {
    match ch {
        '\n' | '\r' => Ok(0xff0d),
        '\t' => Ok(0xff09),
        '\u{0008}' => Ok(0xff08),
        value if value.is_ascii() => Ok(value as u32),
        _ => Err(AppError::Validation(
            "VNC text input currently supports ASCII only".to_string(),
        )),
    }
}

fn validate_source_framebuffer_size(width: u16, height: u16) -> Result<usize, AppError> {
    let pixel_count = usize::from(width)
        .checked_mul(usize::from(height))
        .ok_or_else(|| AppError::External("VNC framebuffer size overflow".to_string()))?;
    if pixel_count > RFB_MAX_SOURCE_CAPTURE_PIXELS {
        return Err(AppError::External(format!(
            "VNC framebuffer is too large for MCP capture: {width}x{height} exceeds the {} source pixel limit",
            RFB_MAX_SOURCE_CAPTURE_PIXELS
        )));
    }
    Ok(pixel_count)
}

struct FittedFramebuffer {
    width: u16,
    height: u16,
    rgba: Vec<u8>,
    scaled: bool,
}

fn fit_framebuffer_for_mcp(
    width: u16,
    height: u16,
    rgba: Vec<u8>,
) -> Result<FittedFramebuffer, AppError> {
    let pixel_count = usize::from(width)
        .checked_mul(usize::from(height))
        .ok_or_else(|| AppError::External("VNC framebuffer size overflow".to_string()))?;
    if pixel_count <= RFB_MAX_CAPTURE_PIXELS {
        validate_capture_output_size(width, height)?;
        return Ok(FittedFramebuffer {
            width,
            height,
            rgba,
            scaled: false,
        });
    }

    let (scaled_width, scaled_height) = scaled_dimensions(width, height, RFB_MAX_CAPTURE_PIXELS)?;
    let scaled_rgba = scale_rgba_nearest(width, height, &rgba, scaled_width, scaled_height)?;
    validate_capture_output_size(scaled_width, scaled_height)?;
    Ok(FittedFramebuffer {
        width: scaled_width,
        height: scaled_height,
        rgba: scaled_rgba,
        scaled: true,
    })
}

fn validate_capture_output_size(width: u16, height: u16) -> Result<(), AppError> {
    let (_row_stride, _image_size, file_size) = bmp_layout(width, height)?;
    if file_size > RFB_MAX_CAPTURE_IMAGE_BYTES {
        return Err(AppError::External(format!(
            "VNC framebuffer response is too large for MCP capture: {file_size} bytes exceeds the {RFB_MAX_CAPTURE_IMAGE_BYTES} byte limit"
        )));
    }
    Ok(())
}

fn scaled_dimensions(width: u16, height: u16, max_pixels: usize) -> Result<(u16, u16), AppError> {
    let pixel_count = usize::from(width)
        .checked_mul(usize::from(height))
        .ok_or_else(|| AppError::External("VNC framebuffer size overflow".to_string()))?;
    if pixel_count <= max_pixels {
        return Ok((width, height));
    }

    let scale = (max_pixels as f64 / pixel_count as f64).sqrt();
    let scaled_width = ((f64::from(width) * scale).floor() as u16).max(1);
    let scaled_height = ((f64::from(height) * scale).floor() as u16).max(1);
    if usize::from(scaled_width) * usize::from(scaled_height) > max_pixels {
        return Err(AppError::External(
            "VNC framebuffer scaling exceeded MCP pixel limit".to_string(),
        ));
    }
    Ok((scaled_width, scaled_height))
}

fn scale_rgba_nearest(
    source_width: u16,
    source_height: u16,
    rgba: &[u8],
    target_width: u16,
    target_height: u16,
) -> Result<Vec<u8>, AppError> {
    let source_width_usize = usize::from(source_width);
    let source_height_usize = usize::from(source_height);
    let target_width_usize = usize::from(target_width);
    let target_height_usize = usize::from(target_height);
    let expected = source_width_usize
        .checked_mul(source_height_usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| AppError::External("VNC framebuffer size overflow".to_string()))?;
    if rgba.len() != expected {
        return Err(AppError::External(
            "VNC framebuffer size did not match dimensions".to_string(),
        ));
    }

    let mut scaled = vec![0_u8; target_width_usize * target_height_usize * 4];
    for target_y in 0..target_height_usize {
        let source_y = target_y * source_height_usize / target_height_usize;
        for target_x in 0..target_width_usize {
            let source_x = target_x * source_width_usize / target_width_usize;
            let source = (source_y * source_width_usize + source_x) * 4;
            let target = (target_y * target_width_usize + target_x) * 4;
            scaled[target..target + 4].copy_from_slice(&rgba[source..source + 4]);
        }
    }
    Ok(scaled)
}

fn bmp_layout(width: u16, height: u16) -> Result<(usize, usize, usize), AppError> {
    let width_usize = usize::from(width);
    let height_usize = usize::from(height);
    let row_stride = width_usize
        .checked_mul(3)
        .and_then(|bytes| bytes.checked_add(3))
        .map(|bytes| bytes & !3)
        .ok_or_else(|| AppError::External("BMP row size overflow".to_string()))?;
    let image_size = row_stride
        .checked_mul(height_usize)
        .ok_or_else(|| AppError::External("BMP image size overflow".to_string()))?;
    let file_size = 14usize
        .checked_add(40)
        .and_then(|header| header.checked_add(image_size))
        .ok_or_else(|| AppError::External("BMP file size overflow".to_string()))?;
    Ok((row_stride, image_size, file_size))
}

fn encode_bmp_from_rgba(width: u16, height: u16, rgba: &[u8]) -> Result<Vec<u8>, AppError> {
    let width_usize = usize::from(width);
    let height_usize = usize::from(height);
    let expected = width_usize
        .checked_mul(height_usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| AppError::External("VNC framebuffer size overflow".to_string()))?;
    if rgba.len() != expected {
        return Err(AppError::External(
            "VNC framebuffer size did not match dimensions".to_string(),
        ));
    }
    let (row_stride, image_size, file_size) = bmp_layout(width, height)?;
    let file_size = u32::try_from(file_size)
        .map_err(|_| AppError::External("BMP file is too large".to_string()))?;
    let image_size = u32::try_from(image_size)
        .map_err(|_| AppError::External("BMP image is too large".to_string()))?;

    let mut bmp = Vec::with_capacity(file_size as usize);
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&file_size.to_le_bytes());
    bmp.extend_from_slice(&[0, 0, 0, 0]);
    bmp.extend_from_slice(&54_u32.to_le_bytes());
    bmp.extend_from_slice(&40_u32.to_le_bytes());
    bmp.extend_from_slice(&(i32::from(width)).to_le_bytes());
    bmp.extend_from_slice(&(i32::from(height)).to_le_bytes());
    bmp.extend_from_slice(&1_u16.to_le_bytes());
    bmp.extend_from_slice(&24_u16.to_le_bytes());
    bmp.extend_from_slice(&0_u32.to_le_bytes());
    bmp.extend_from_slice(&image_size.to_le_bytes());
    bmp.extend_from_slice(&0_i32.to_le_bytes());
    bmp.extend_from_slice(&0_i32.to_le_bytes());
    bmp.extend_from_slice(&0_u32.to_le_bytes());
    bmp.extend_from_slice(&0_u32.to_le_bytes());

    let padding = row_stride - width_usize * 3;
    for row in (0..height_usize).rev() {
        for col in 0..width_usize {
            let src = (row * width_usize + col) * 4;
            bmp.push(rgba[src + 2]);
            bmp.push(rgba[src + 1]);
            bmp.push(rgba[src]);
        }
        bmp.extend(std::iter::repeat_n(0, padding));
    }
    Ok(bmp)
}

fn encode_png_from_rgba(width: u16, height: u16, rgba: &[u8]) -> Result<Vec<u8>, AppError> {
    let width_usize = usize::from(width);
    let height_usize = usize::from(height);
    let expected = width_usize
        .checked_mul(height_usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| AppError::External("VNC framebuffer size overflow".to_string()))?;
    if rgba.len() != expected {
        return Err(AppError::External(
            "VNC framebuffer size did not match dimensions".to_string(),
        ));
    }

    let mut png_bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png_bytes, u32::from(width), u32::from(height));
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|error| AppError::External(format!("Failed to write PNG header: {error}")))?;
        writer
            .write_image_data(rgba)
            .map_err(|error| AppError::External(format!("Failed to write PNG data: {error}")))?;
    }
    Ok(png_bytes)
}

#[cfg(test)]
mod tests {
    use super::{
        RFB_MAX_CAPTURE_PIXELS, RFB_SECURITY_NONE, RFB_SECURITY_VNC_AUTH, RFB_VERSION_38,
        RfbClient, agent_vnc_session, encode_bmp_from_rgba, encode_png_from_rgba,
        fit_framebuffer_for_mcp, keysym_for_ascii, reverse_bits, scale_rgba_nearest,
        select_security_type, validate_source_framebuffer_size, vnc_auth_response,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use crate::config::ProxyConfig;
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::RwLock;
    use tokio::time::Duration;
    use uuid::Uuid;

    use super::super::metrics::ConnectionMetricsState;

    #[test]
    fn reverses_vnc_des_key_bits() {
        assert_eq!(reverse_bits(0b0001_0010), 0b0100_1000);
    }

    #[test]
    fn selects_vnc_auth_when_password_available() {
        let selected =
            select_security_type(&[RFB_SECURITY_NONE, RFB_SECURITY_VNC_AUTH], Some("secret"))
                .expect("auth security type");
        assert_eq!(selected, RFB_SECURITY_VNC_AUTH);
    }

    #[test]
    fn rejects_vnc_auth_without_password() {
        let error =
            select_security_type(&[RFB_SECURITY_VNC_AUTH], None).expect_err("password is required");
        assert!(error.to_string().contains("password"));
    }

    #[test]
    fn vnc_auth_response_is_deterministic_and_sized() {
        let challenge = *b"0123456789abcdef";
        let response = vnc_auth_response(&challenge, "passw0rd").expect("auth response");
        assert_eq!(response.len(), 16);
        assert_eq!(response, vnc_auth_response(&challenge, "passw0rd").unwrap());
        assert_ne!(response, challenge);
    }

    #[test]
    fn maps_ascii_control_keysyms() {
        assert_eq!(keysym_for_ascii('\n').unwrap(), 0xff0d);
        assert_eq!(keysym_for_ascii('\t').unwrap(), 0xff09);
        assert_eq!(keysym_for_ascii('A').unwrap(), 65);
    }

    #[test]
    fn encodes_bmp_header_and_bottom_up_pixels() {
        let rgba = vec![
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ];
        let bmp = encode_bmp_from_rgba(2, 2, &rgba).expect("bmp");
        assert_eq!(&bmp[..2], b"BM");
        assert_eq!(u32::from_le_bytes([bmp[10], bmp[11], bmp[12], bmp[13]]), 54);
        assert_eq!(bmp[54], 255);
        assert_eq!(bmp[55], 0);
        assert_eq!(bmp[56], 0);
    }

    #[test]
    fn encodes_png_header_and_rgba_pixels() {
        let rgba = vec![255, 0, 0, 255, 0, 255, 0, 255];
        let png = encode_png_from_rgba(2, 1, &rgba).expect("png");
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn rejects_extreme_source_framebuffer_capture_before_allocation() {
        let error =
            validate_source_framebuffer_size(8192, 8192).expect_err("framebuffer too large");
        assert!(error.to_string().contains("too large"));
    }

    #[test]
    fn scales_dual_monitor_framebuffer_to_mcp_pixel_budget() {
        let source_width = 1880;
        let source_height = 1920;
        let rgba = vec![127_u8; source_width * source_height * 4];

        let frame = fit_framebuffer_for_mcp(source_width as u16, source_height as u16, rgba)
            .expect("scaled framebuffer");

        assert!(frame.scaled);
        assert!(frame.width < source_width as u16);
        assert!(frame.height < source_height as u16);
        assert!(usize::from(frame.width) * usize::from(frame.height) <= RFB_MAX_CAPTURE_PIXELS);
        assert_eq!(
            frame.rgba.len(),
            usize::from(frame.width) * usize::from(frame.height) * 4
        );
    }

    #[test]
    fn scales_rgba_with_nearest_neighbor_sampling() {
        let rgba = vec![
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ];
        let scaled = scale_rgba_nearest(2, 2, &rgba, 1, 1).expect("scale");
        assert_eq!(scaled, vec![255, 0, 0, 255]);
    }

    #[tokio::test]
    async fn handshake_accepts_none_security_fake_server() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            stream
                .write_all(RFB_VERSION_38)
                .await
                .expect("server version");
            let mut client_version = [0_u8; 12];
            stream
                .read_exact(&mut client_version)
                .await
                .expect("client version");
            assert_eq!(&client_version, RFB_VERSION_38);
            stream
                .write_all(&[1, RFB_SECURITY_NONE])
                .await
                .expect("security list");
            let mut selected = [0_u8; 1];
            stream.read_exact(&mut selected).await.expect("selection");
            assert_eq!(selected[0], RFB_SECURITY_NONE);
            stream
                .write_all(&0_u32.to_be_bytes())
                .await
                .expect("result");
            let mut client_init = [0_u8; 1];
            stream
                .read_exact(&mut client_init)
                .await
                .expect("client init");
            assert_eq!(client_init[0], 1);
            write_server_init(&mut stream, 2, 1).await;
        });

        let stream = TcpStream::connect(addr).await.expect("connect");
        let mut client = test_client(stream, 0, 0);
        client.handshake(None).await.expect("handshake");
        assert_eq!(client.width, 2);
        assert_eq!(client.height, 1);
        server.await.expect("server task");
    }

    #[tokio::test]
    async fn handshake_times_out_when_server_stalls_on_version() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.expect("accept");
            tokio::time::sleep(Duration::from_secs(1)).await;
        });

        let stream = TcpStream::connect(addr).await.expect("connect");
        let mut client = test_client(stream, 0, 0);
        client.io_timeout = Duration::from_millis(25);
        let error = client.handshake(None).await.expect_err("timeout");
        assert!(
            error
                .to_string()
                .contains("Timed out reading VNC protocol version")
        );
        server.abort();
    }

    #[tokio::test]
    async fn handshake_sends_vnc_auth_response_fake_server() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let challenge = *b"0123456789abcdef";
        let expected = vnc_auth_response(&challenge, "secret").expect("expected response");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            stream
                .write_all(RFB_VERSION_38)
                .await
                .expect("server version");
            let mut client_version = [0_u8; 12];
            stream
                .read_exact(&mut client_version)
                .await
                .expect("client version");
            stream
                .write_all(&[1, RFB_SECURITY_VNC_AUTH])
                .await
                .expect("security list");
            let mut selected = [0_u8; 1];
            stream.read_exact(&mut selected).await.expect("selection");
            assert_eq!(selected[0], RFB_SECURITY_VNC_AUTH);
            stream.write_all(&challenge).await.expect("challenge");
            let mut response = [0_u8; 16];
            stream.read_exact(&mut response).await.expect("response");
            assert_eq!(response, expected);
            stream
                .write_all(&0_u32.to_be_bytes())
                .await
                .expect("result");
            let mut client_init = [0_u8; 1];
            stream
                .read_exact(&mut client_init)
                .await
                .expect("client init");
            write_server_init(&mut stream, 1, 1).await;
        });

        let stream = TcpStream::connect(addr).await.expect("connect");
        let mut client = test_client(stream, 0, 0);
        client.handshake(Some("secret")).await.expect("handshake");
        assert_eq!(client.width, 1);
        assert_eq!(client.height, 1);
        server.await.expect("server task");
    }

    #[tokio::test]
    async fn agent_vnc_session_reuses_cached_rfb_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let accept_count = Arc::new(AtomicUsize::new(0));
        let accept_count_for_server = accept_count.clone();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            accept_count_for_server.fetch_add(1, Ordering::SeqCst);
            stream
                .write_all(RFB_VERSION_38)
                .await
                .expect("server version");
            let mut client_version = [0_u8; 12];
            stream
                .read_exact(&mut client_version)
                .await
                .expect("client version");
            stream
                .write_all(&[1, RFB_SECURITY_NONE])
                .await
                .expect("security list");
            let mut selected = [0_u8; 1];
            stream.read_exact(&mut selected).await.expect("selection");
            stream
                .write_all(&0_u32.to_be_bytes())
                .await
                .expect("result");
            let mut client_init = [0_u8; 1];
            stream
                .read_exact(&mut client_init)
                .await
                .expect("client init");
            write_server_init(&mut stream, 2, 1).await;
            tokio::time::sleep(Duration::from_millis(100)).await;
        });

        let state = Arc::new(super::super::ProxyState::new(&ProxyConfig::default()).unwrap());
        let connection_id = Uuid::now_v7();
        state.connections.write().await.insert(
            connection_id,
            super::super::ProxyConnection {
                payload: json!({
                    "connection_type": "vnc",
                    "host": addr.ip().to_string(),
                    "port": addr.port(),
                    "timeout": 1
                }),
                metrics: Arc::new(RwLock::new(ConnectionMetricsState::default())),
            },
        );

        let first = agent_vnc_session(state.clone(), connection_id)
            .await
            .expect("first session");
        let second = agent_vnc_session(state, connection_id)
            .await
            .expect("cached session");

        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(accept_count.load(Ordering::SeqCst), 1);
        server.await.expect("server task");
    }

    #[tokio::test]
    async fn capture_skips_server_cut_text_padding_before_framebuffer_update() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut request = [0_u8; 10];
            stream
                .read_exact(&mut request)
                .await
                .expect("framebuffer request");
            assert_eq!(request[0], 3);

            stream
                .write_all(&[3, 0, 0, 0])
                .await
                .expect("cut text header");
            stream
                .write_all(&4_u32.to_be_bytes())
                .await
                .expect("cut text length");
            stream.write_all(b"clip").await.expect("cut text");

            let mut update = Vec::new();
            update.extend_from_slice(&[0, 0]);
            update.extend_from_slice(&1_u16.to_be_bytes());
            update.extend_from_slice(&0_u16.to_be_bytes());
            update.extend_from_slice(&0_u16.to_be_bytes());
            update.extend_from_slice(&1_u16.to_be_bytes());
            update.extend_from_slice(&1_u16.to_be_bytes());
            update.extend_from_slice(&0_i32.to_be_bytes());
            update.extend_from_slice(&[0, 0, 255, 255]);
            stream.write_all(&update).await.expect("framebuffer update");
        });

        let stream = TcpStream::connect(addr).await.expect("connect");
        let mut client = test_client(stream, 1, 1);
        let capture = client
            .capture(crate::proxy::AgentVncCaptureFormat::Bmp)
            .await
            .expect("capture");
        assert_eq!(capture.width, 1);
        assert_eq!(capture.height, 1);
        assert_eq!(&capture.image_bytes[..2], b"BM");
        server.await.expect("server task");
    }

    #[tokio::test]
    async fn capture_times_out_when_server_stalls_after_request() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut request = [0_u8; 10];
            stream
                .read_exact(&mut request)
                .await
                .expect("framebuffer request");
            tokio::time::sleep(Duration::from_secs(1)).await;
        });

        let stream = TcpStream::connect(addr).await.expect("connect");
        let mut client = test_client(stream, 1, 1);
        client.io_timeout = Duration::from_millis(25);
        let error = client
            .capture(crate::proxy::AgentVncCaptureFormat::Bmp)
            .await
            .expect_err("timeout");
        assert!(error.to_string().contains("Timed out reading VNC byte"));
        server.abort();
    }

    #[tokio::test]
    async fn capture_accepts_slow_raw_framebuffer_when_bytes_keep_arriving() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut request = [0_u8; 10];
            stream
                .read_exact(&mut request)
                .await
                .expect("framebuffer request");

            let mut update = Vec::new();
            update.extend_from_slice(&[0, 0]);
            update.extend_from_slice(&1_u16.to_be_bytes());
            update.extend_from_slice(&0_u16.to_be_bytes());
            update.extend_from_slice(&0_u16.to_be_bytes());
            update.extend_from_slice(&3_u16.to_be_bytes());
            update.extend_from_slice(&1_u16.to_be_bytes());
            update.extend_from_slice(&0_i32.to_be_bytes());
            stream.write_all(&update).await.expect("framebuffer header");
            for chunk in [[0, 0, 255, 255], [0, 255, 0, 255], [255, 0, 0, 255]] {
                stream.write_all(&chunk).await.expect("framebuffer chunk");
                tokio::time::sleep(Duration::from_millis(35)).await;
            }
        });

        let stream = TcpStream::connect(addr).await.expect("connect");
        let mut client = test_client(stream, 3, 1);
        client.io_timeout = Duration::from_millis(50);
        let capture = client
            .capture(crate::proxy::AgentVncCaptureFormat::Bmp)
            .await
            .expect("capture");
        assert_eq!(capture.width, 3);
        assert_eq!(capture.height, 1);
        server.await.expect("server task");
    }

    async fn write_server_init(stream: &mut TcpStream, width: u16, height: u16) {
        stream.write_all(&width.to_be_bytes()).await.expect("width");
        stream
            .write_all(&height.to_be_bytes())
            .await
            .expect("height");
        stream.write_all(&[0; 16]).await.expect("pixel format");
        stream
            .write_all(&0_u32.to_be_bytes())
            .await
            .expect("name length");
    }

    fn test_client(stream: TcpStream, width: u16, height: u16) -> RfbClient {
        RfbClient {
            stream,
            width,
            height,
            io_timeout: Duration::from_millis(250),
        }
    }
}
