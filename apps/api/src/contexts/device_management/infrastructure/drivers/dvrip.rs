//! DVRIP / Sofia driver (Xiongmai family, TCP 34567).
//!
//! Opens a fresh session per operation (connect → login → query → drop), which
//! matches the stateless request/response nature of the management plane. Read
//! operations (`get_info`, `list_users`, `disk_status`, `recording_status`) and
//! `stream_uri` are implemented and verified against live devices. Provisioning
//! (`create_users` via `AddUser`/1482, `delete_users` via `DelUser`/1486) opens
//! one authenticated session and attempts each item, collecting a per-item
//! [`BulkOutcome`]. `snapshot` opens a session and sends `OPSNAP`/1560, whose
//! reply is a binary media frame (not JSON) carrying the JPEG still.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use futures_util::StreamExt as _;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::{Value, json};

use crate::contexts::device_management::domain::{
    BulkOutcome, Capabilities, Channel, DeviceDriver, DeviceInfo, DiskState, DiskStatus,
    DownloadStream, DriverError, ExportRequest, LinkFailurePolicy, NewUser, PtzCommand, PtzSupport,
    PtzVector, RecordSpan, RecordingStatus, RemuxHints, Secret, SnapshotRef, StreamTransport,
    StreamUri, UserAccount, UserUpdate,
};
use crate::contexts::device_management::infrastructure::transport::{
    DeviceEndpoint, DeviceStream, DeviceTransport,
};
use crate::contexts::device_management::protocol::dvrip as codec;

/// One mebibyte, the unit DVRIP reports storage space in.
const MIB: u64 = 1024 * 1024;
/// The `Ret` code a DVRIP device returns on success.
const RET_OK: i64 = 100;
/// Group assigned to a provisioned user when the caller specifies none. DVRIP
/// devices ship a built-in non-privileged `user` group alongside `admin`.
const DEFAULT_USER_GROUP: &str = "user";
/// Upper bound on one response body. Far above any real config reply, but small
/// enough that a rogue or buggy device cannot force a huge allocation from the
/// device-controlled length field.
const MAX_RESPONSE_LEN: usize = 8 * 1024 * 1024;
/// Per-read timeout so a device that under-delivers cannot hang the task.
const READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
/// How long an archive download waits for the next media frame before treating
/// the silence as the end of the window.
///
/// Shorter than [`READ_TIMEOUT`] because silence is the protocol's only end
/// marker: the reader validates timestamp coverage before accepting it as a
/// clean end. Long enough to ride out an ordinary stall between frames.
const ARCHIVE_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
/// How close the first/last timestamped I-frame must be to the claimed window.
///
/// XM timestamps have one-second precision and only I-frames carry one, so an
/// exact boundary comparison would reject a complete clip by up to one GOP.
/// Five seconds covers the observed GOPs without allowing a mid-window stall to
/// masquerade as successful delivery of the rest of a segment.
const ARCHIVE_COVERAGE_TOLERANCE: chrono::TimeDelta = chrono::TimeDelta::seconds(5);
/// JPEG Start-Of-Image marker; an `OPSNAP` still begins here inside the frame.
const JPEG_SOI: [u8; 2] = [0xFF, 0xD8];
/// JPEG End-Of-Image marker; the still ends here, before any device padding.
const JPEG_EOI: [u8; 2] = [0xFF, 0xD9];

/// A DVRIP/Sofia driver bound to one device.
pub struct DvripDriver {
    transport: Arc<dyn DeviceTransport>,
    endpoint: DeviceEndpoint,
    username: String,
    password: Secret,
}

impl DvripDriver {
    /// Construct a driver for a device reachable via `transport`.
    pub fn new(
        transport: Arc<dyn DeviceTransport>,
        endpoint: DeviceEndpoint,
        username: impl Into<String>,
        password: Secret,
    ) -> Self {
        Self {
            transport,
            endpoint,
            username: username.into(),
            password,
        }
    }

    /// Open a transport stream and authenticate, returning a live session.
    async fn open(&self) -> Result<DvripSession, DriverError> {
        let stream = self.transport.connect(&self.endpoint).await?;
        let mut session = DvripSession {
            stream,
            session_id: 0,
            sequence: 0,
        };
        session.login(&self.username, &self.password).await?;
        Ok(session)
    }
}

/// An authenticated DVRIP session over one transport stream.
struct DvripSession {
    stream: Box<dyn DeviceStream>,
    session_id: u32,
    sequence: u32,
}

impl DvripSession {
    /// Send one request frame and read one response frame, returning the full
    /// frame bytes (header + body) verbatim.
    ///
    /// This is the shared framing behind [`request`](Self::request) (which
    /// JSON-decodes the reply) and [`request_raw`](Self::request_raw) (which
    /// returns the binary body): it applies the [`MAX_RESPONSE_LEN`] cap and the
    /// bounded read timeout in exactly one place.
    async fn exchange(&mut self, message_id: u16, payload: &Value) -> Result<Vec<u8>, DriverError> {
        let json =
            serde_json::to_string(payload).map_err(|e| DriverError::Protocol(e.to_string()))?;
        let frame = codec::encode(message_id, self.session_id, self.sequence, &json);
        self.sequence = self.sequence.wrapping_add(1);
        self.stream.send(&frame).await?;

        let mut header = [0u8; codec::HEADER_LEN];
        recv_exact_timed(self.stream.as_mut(), &mut header).await?;
        let declared =
            u32::from_le_bytes([header[16], header[17], header[18], header[19]]) as usize;
        if declared > MAX_RESPONSE_LEN {
            return Err(DriverError::Protocol(format!(
                "response body length {declared} exceeds maximum {MAX_RESPONSE_LEN}"
            )));
        }
        let mut body = vec![0u8; declared];
        recv_exact_timed(self.stream.as_mut(), &mut body).await?;

        let mut full = Vec::with_capacity(codec::HEADER_LEN + declared);
        full.extend_from_slice(&header);
        full.extend_from_slice(&body);
        Ok(full)
    }

    /// Send one request frame and read one response frame, parsing the JSON body.
    async fn request(&mut self, message_id: u16, payload: &Value) -> Result<Value, DriverError> {
        let full = self.exchange(message_id, payload).await?;
        let frame = codec::decode(&full).map_err(|e| DriverError::Protocol(e.to_string()))?;
        serde_json::from_slice(&frame.payload).map_err(|e| DriverError::Protocol(e.to_string()))
    }

    /// Send one request frame and return the raw response *body* bytes, without
    /// JSON decoding or terminator stripping.
    ///
    /// Used for `OPSNAP`, whose reply is a binary media frame rather than JSON.
    /// The bytes preceding the JPEG (a small device preamble) and any trailing
    /// padding are left for the caller to strip by locating the JPEG markers.
    async fn request_raw(
        &mut self,
        message_id: u16,
        payload: &Value,
    ) -> Result<Vec<u8>, DriverError> {
        let full = self.exchange(message_id, payload).await?;
        Ok(full[codec::HEADER_LEN..].to_vec())
    }

    /// Authenticate, capturing the session id the device assigns.
    async fn login(&mut self, username: &str, password: &Secret) -> Result<(), DriverError> {
        let payload = json!({
            "EncryptType": "MD5",
            "LoginType": "DVRIP-Web",
            "UserName": username,
            "PassWord": codec::sofia_hash(password.expose()),
        });
        let response = self.request(codec::command::LOGIN, &payload).await?;
        if response_ret(&response) != Some(RET_OK) {
            return Err(DriverError::Auth);
        }
        let session_hex = response
            .get("SessionID")
            .and_then(Value::as_str)
            .ok_or_else(|| DriverError::Protocol("login response missing SessionID".to_owned()))?;
        self.session_id = parse_session_id(session_hex)
            .ok_or_else(|| DriverError::Protocol(format!("invalid SessionID {session_hex:?}")))?;
        Ok(())
    }

    /// Issue a named config/system query and require a success `Ret`.
    async fn query(&mut self, message_id: u16, name: &str) -> Result<Value, DriverError> {
        let session_hex = format!("0x{:08X}", self.session_id);
        let payload = json!({ "Name": name, "SessionID": session_hex });
        let response = self.request(message_id, &payload).await?;
        if response_ret(&response) != Some(RET_OK) {
            return Err(DriverError::Device(format!(
                "query {name} returned Ret {:?}",
                response_ret(&response)
            )));
        }
        Ok(response)
    }

    /// The device-assigned session id in the `"0x00000005"` form the write
    /// payloads carry.
    fn session_hex(&self) -> String {
        format!("0x{:08X}", self.session_id)
    }

    /// Provision one user via `AddUser` (1482). The password is hashed with the
    /// Sofia digest before it leaves this process; the plaintext is never placed
    /// on the wire.
    async fn add_user(&mut self, user: &NewUser) -> Result<(), DriverError> {
        let group = user.group.as_deref().unwrap_or(DEFAULT_USER_GROUP);
        let payload = json!({
            "Name": "AddUser",
            "SessionID": self.session_hex(),
            "User": {
                "AuthorityList": user.authorities,
                "Group": group,
                "Memo": "",
                "Name": user.name,
                "Password": codec::sofia_hash(user.password.expose()),
                "Reserved": false,
                "Sharable": true,
            },
        });
        let response = self.request(codec::command::ADD_USER, &payload).await?;
        ret_ok_or_device(&response, "AddUser")
    }

    /// Remove one user via `DelUser` (1486). Note the username is keyed under
    /// `"Name"`, unlike the `AddUser` envelope which nests it under `"User"`.
    async fn delete_user(&mut self, username: &str) -> Result<(), DriverError> {
        let payload = json!({ "Name": username, "SessionID": self.session_hex() });
        let response = self.request(codec::command::DELETE_USER, &payload).await?;
        ret_ok_or_device(&response, "DelUser")
    }

    /// Edit one existing user. A password change goes through the dedicated
    /// `ModifyPassword` (1488); group/authority changes go through `ModifyUser`
    /// (1484), whose `User` envelope mirrors `AddUser`'s. A single [`UserUpdate`]
    /// can carry both, so up to two frames may be sent. The device offers no
    /// transactional envelope spanning both writes, so this method cannot make
    /// the pair atomic — instead it orders them to bound what a rejection can do:
    /// `ModifyPassword` is sent FIRST, `ModifyUser` (the group/authority —
    /// privilege — change) second. Per the HARD SAFETY note below, `ModifyPassword`
    /// is the frame more likely to be rejected by a given firmware, so trying it
    /// first means a rejection aborts *before* any group/authority change reaches
    /// the device — a rejected password reset can never leave a privilege
    /// escalation committed behind it. If `ModifyPassword` instead succeeds and
    /// the subsequent `ModifyUser` is rejected, the password change has already
    /// landed on-device; that partial state is never folded into a plain
    /// "ModifyUser failed" — the per-item failure message says so explicitly (see
    /// [`annotate_partial_apply`]). The caller guarantees the update is
    /// non-vacuous, so at least one frame is always sent.
    ///
    /// The password is Sofia-hashed before it leaves this process; the plaintext
    /// is never placed on the wire, exactly as in [`add_user`](Self::add_user).
    ///
    /// HARD SAFETY — like `AddUser`/`DelUser`, the `ModifyUser`/`ModifyPassword`
    /// `Ret` success/failure catalogue is taken from reference implementations and
    /// is **not dual-source verified**, and this driver must never touch a live
    /// device to discover it. Two consequences are documented rather than guessed:
    /// (1) every non-OK `Ret` is surfaced verbatim in `failed` (see
    /// [`ret_ok_or_device`]), never folded into a fake success; and (2) some
    /// firmware's `ModifyPassword` requires the account's *current* password as an
    /// additional field — [`UserUpdate`] does not carry one, so an admin-set reset
    /// is assumed. Both must be verified against real traffic before writes are
    /// enabled in production.
    async fn modify_user(&mut self, update: &UserUpdate) -> Result<(), DriverError> {
        let password_committed = if let Some(password) = &update.password {
            let payload = json!({
                "Name": "ModifyPassword",
                "SessionID": self.session_hex(),
                "User": {
                    "Name": update.name,
                    "Password": codec::sofia_hash(password.expose()),
                },
            });
            let response = self
                .request(codec::command::MODIFY_PASSWORD, &payload)
                .await?;
            ret_ok_or_device(&response, "ModifyPassword")?;
            true
        } else {
            false
        };
        if update.group.is_some() || update.authorities.is_some() {
            let mut user = serde_json::Map::new();
            user.insert("Name".to_owned(), Value::String(update.name.clone()));
            if let Some(group) = &update.group {
                user.insert("Group".to_owned(), Value::String(group.clone()));
            }
            if let Some(authorities) = &update.authorities {
                user.insert("AuthorityList".to_owned(), json!(authorities));
            }
            let payload = json!({
                "Name": "ModifyUser",
                "SessionID": self.session_hex(),
                "User": Value::Object(user),
            });
            let response = self.request(codec::command::MODIFY_USER, &payload).await?;
            if let Err(error) = ret_ok_or_device(&response, "ModifyUser") {
                return Err(annotate_partial_apply(error, password_committed));
            }
        }
        Ok(())
    }
}

/// Read exactly `buf.len()` bytes with a bounded timeout, so an under-delivering
/// device cannot hang the calling task forever.
async fn recv_exact_timed(
    stream: &mut dyn DeviceStream,
    buf: &mut [u8],
) -> Result<(), DriverError> {
    recv_exact_within(stream, buf, READ_TIMEOUT).await
}

/// [`recv_exact_timed`] with an explicit deadline, for callers whose idea of
/// "too long" differs from the control plane's.
async fn recv_exact_within(
    stream: &mut dyn DeviceStream,
    buf: &mut [u8],
    deadline: std::time::Duration,
) -> Result<(), DriverError> {
    tokio::time::timeout(deadline, stream.recv_exact(buf))
        .await
        .map_err(|_elapsed| DriverError::Timeout("device read deadline exceeded".to_owned()))?
}

/// One recorded segment as the device enumerates it.
///
/// The archive is stored as per-hour files and playback is addressed BY NAME:
/// `PlayMode: "ByTime"` is accepted by the device and then delivers nothing, so
/// a window is always resolved to concrete segments first.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ArchiveSegment {
    /// Device-side path, replayed verbatim.
    name: String,
    /// Segment start, device-local wall clock read as UTC.
    start: DateTime<Utc>,
    /// Segment end, device-local wall clock read as UTC.
    end: DateTime<Utc>,
}

impl DvripSession {
    /// Enumerate recorded segments for one channel and window (`OPFileQuery`).
    ///
    /// A pure metadata read. An empty list means the device has no footage for
    /// the window, which is a normal answer rather than an error.
    async fn query_archive_segments(
        &mut self,
        channel: u16,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<ArchiveSegment>, DriverError> {
        let payload = json!({
            "Name": "OPFileQuery",
            "SessionID": self.session_hex(),
            "OPFileQuery": {
                "BeginTime": format_dvr_time(start),
                "EndTime": format_dvr_time(end),
                "Channel": channel,
                // Every storage driver type; the device masks what it has.
                "DriverTypeMask": "0x0000FFFF",
                "Event": "*",
                // The query's type token, not the payload codec: these devices
                // answer it with HEVC recordings just as readily as H.264.
                "Type": "h264",
            },
        });
        let response = self
            .request(codec::command::FILE_QUERY, &payload)
            .await
            .map_err(annotate_archive_request_error)?;
        match response_ret(&response) {
            Some(RET_OK) => {}
            Some(ret) => {
                return Err(DriverError::Device(format!(
                    "OPFileQuery returned Ret {ret}"
                )));
            }
            None => {
                return Err(DriverError::Protocol(
                    "OPFileQuery response missing integer Ret".to_owned(),
                ));
            }
        }
        Ok(parse_archive_segments(&response))
    }

    /// Claim and start an archive download for one segment, bounded to the
    /// caller's sub-window.
    ///
    /// The device honours a sub-window inside a segment and closes the stream
    /// itself at the end of it. `Claim` and `DownloadStart` deliberately use
    /// different message ids, and the device does not answer the latter — it
    /// simply begins pushing media frames.
    async fn start_archive_download(
        &mut self,
        segment: &ArchiveSegment,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<(), DriverError> {
        let body = json!({
            "Name": "OPPlayBack",
            "SessionID": self.session_hex(),
            "OPPlayBack": {
                "Action": "Claim",
                "Parameter": {
                    "PlayMode": "ByName",
                    "FileName": segment.name,
                    "StreamType": 0,
                    "Value": 0,
                    "TransMode": "TCP",
                },
                "StartTime": format_dvr_time(start),
                "EndTime": format_dvr_time(end),
            },
        });
        let claim = self
            .request(codec::command::PLAYBACK_CLAIM, &body)
            .await
            .map_err(annotate_archive_request_error)?;
        ret_ok_or_device(&claim, "OPPlayBack Claim")?;

        let mut start_body = body;
        start_body["OPPlayBack"]["Action"] = Value::String("DownloadStart".to_owned());
        let json = serde_json::to_string(&start_body)
            .map_err(|error| DriverError::Protocol(error.to_string()))?;
        let frame = codec::encode(
            codec::command::PLAYBACK_START,
            self.session_id,
            self.sequence,
            &json,
        );
        self.sequence = self.sequence.wrapping_add(1);
        self.stream.send(&frame).await
    }

    /// Read the next raw archive frame body, or `None` at end of stream.
    ///
    /// Frames other than [`codec::command::PLAYBACK_MEDIA`] (keepalive
    /// acknowledgements, status frames) are consumed and skipped so a control
    /// frame in the middle of a download cannot corrupt the media stream.
    ///
    /// End of stream has TWO shapes on this protocol, and only one of them is a
    /// closed socket. Live capture shows the device finishing a window by simply
    /// going quiet while holding the connection open — no FIN, no status frame.
    /// A read timeout is therefore a *candidate* end. [`ArchiveReader`] checks
    /// the timestamped media coverage before accepting it as clean; silence in
    /// the middle of the claimed window remains a visible stream error.
    async fn next_archive_frame(
        &mut self,
        idle_timeout: std::time::Duration,
    ) -> Result<Option<Vec<u8>>, DriverError> {
        loop {
            let mut header = [0u8; codec::HEADER_LEN];
            match recv_exact_within(self.stream.as_mut(), &mut header, idle_timeout).await {
                Ok(()) => {}
                // A closed socket, or a device that has simply stopped sending:
                // both end this read attempt; the reader decides from timestamp
                // coverage whether that end was complete.
                Err(
                    DriverError::Transport(_)
                    | DriverError::Unreachable(_)
                    | DriverError::Timeout(_),
                ) => return Ok(None),
                Err(error) => return Err(error),
            }
            let declared =
                u32::from_le_bytes([header[16], header[17], header[18], header[19]]) as usize;
            if declared > MAX_RESPONSE_LEN {
                return Err(DriverError::Protocol(format!(
                    "archive frame length {declared} exceeds maximum {MAX_RESPONSE_LEN}"
                )));
            }
            let mut body = vec![0u8; declared];
            if declared > 0 {
                recv_exact_timed(self.stream.as_mut(), &mut body).await?;
            }
            let message_id = u16::from_le_bytes([header[14], header[15]]);
            if message_id == codec::command::PLAYBACK_MEDIA {
                return Ok(Some(body));
            }
        }
    }

    /// Send one keepalive so a long download is not dropped mid-window.
    async fn send_keepalive(&mut self) -> Result<(), DriverError> {
        let payload = json!({ "Name": "KeepAlive", "SessionID": self.session_hex() });
        let json =
            serde_json::to_string(&payload).map_err(|e| DriverError::Protocol(e.to_string()))?;
        let frame = codec::encode(
            codec::command::KEEPALIVE,
            self.session_id,
            self.sequence,
            &json,
        );
        self.sequence = self.sequence.wrapping_add(1);
        self.stream.send(&frame).await
    }
}

/// Format a timestamp the way this protocol's time fields expect.
///
/// The device speaks local wall clock; [`parse_dvr_time`] already reads its
/// values as UTC, so this is the exact inverse of that convention.
fn format_dvr_time(value: DateTime<Utc>) -> String {
    value.format("%Y-%m-%d %H:%M:%S").to_string()
}

/// Add archive-request context to a device error, leaving every other failure
/// class untouched.
fn annotate_archive_request_error(error: DriverError) -> DriverError {
    match error {
        DriverError::Device(message) => {
            DriverError::Device(format!("archive request rejected: {message}"))
        }
        other => other,
    }
}

/// Parse an `OPFileQuery` reply into segments, skipping malformed entries.
fn parse_archive_segments(response: &Value) -> Vec<ArchiveSegment> {
    response
        .get("OPFileQuery")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| {
                    Some(ArchiveSegment {
                        name: entry.get("FileName")?.as_str()?.to_owned(),
                        start: parse_dvr_time(entry.get("BeginTime")?.as_str()?)?,
                        end: parse_dvr_time(entry.get("EndTime")?.as_str()?)?,
                    })
                })
                .take(MAX_ARCHIVE_SEGMENTS)
                .collect()
        })
        .unwrap_or_default()
}

#[async_trait]
impl DeviceDriver for DvripDriver {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            info: true,
            users: true,
            disk: true,
            recording: true,
            snapshot: true,
            streaming: true,
            channels: true,
            ptz: true,
            export: true,
            user_update: true,
            authority_catalog: true,
            recording_search: true,
        }
    }

    async fn get_info(&self) -> Result<DeviceInfo, DriverError> {
        let mut session = self.open().await?;
        let response = session
            .query(codec::command::CONFIG_QUERY, "SystemInfo")
            .await?;
        Ok(parse_system_info(&response))
    }

    async fn list_users(&self) -> Result<Vec<UserAccount>, DriverError> {
        let mut session = self.open().await?;
        let response = session.query(codec::command::USERS, "Users").await?;
        Ok(parse_users(&response))
    }

    async fn list_authorities(&self) -> Result<Vec<String>, DriverError> {
        let mut session = self.open().await?;
        let response = session
            .query(codec::command::AUTHORITY_LIST, "AuthorityList")
            .await?;
        Ok(parse_authority_list(&response))
    }

    async fn create_users(&self, users: &[NewUser]) -> Result<BulkOutcome, DriverError> {
        // One login serves the whole batch. A connect/login failure prevents all
        // work → top-level Err; a device-reported per-user failure lands in
        // `failed`. A dead stream stops the batch: the attempted account carries
        // the error, the tail is recorded as unattempted, and even with no
        // acknowledgement the outcome stays per-item because the login already
        // proved the link (see `LinkFailurePolicy::PreservePartialOutcome`).
        let mut session = self.open().await?;
        let mut outcome = BulkOutcome::default();
        for (index, user) in users.iter().enumerate() {
            match session.add_user(user).await {
                Ok(()) => outcome.succeeded.push(user.name.clone()),
                Err(error) => {
                    let stream_dead = matches!(
                        &error,
                        DriverError::Transport(_)
                            | DriverError::Timeout(_)
                            | DriverError::Unreachable(_)
                    );
                    if stream_dead {
                        return outcome.abort_after_link_failure(
                            user.name.clone(),
                            error,
                            users[index + 1..].iter().map(|u| u.name.clone()),
                            LinkFailurePolicy::PreservePartialOutcome,
                        );
                    }
                    outcome.failed.push((user.name.clone(), error));
                }
            }
        }
        Ok(outcome)
    }

    async fn delete_users(&self, usernames: &[String]) -> Result<BulkOutcome, DriverError> {
        let mut session = self.open().await?;
        let mut outcome = BulkOutcome::default();
        for (index, username) in usernames.iter().enumerate() {
            match session.delete_user(username).await {
                Ok(()) => outcome.succeeded.push(username.clone()),
                Err(error) => {
                    let stream_dead = matches!(
                        &error,
                        DriverError::Transport(_)
                            | DriverError::Timeout(_)
                            | DriverError::Unreachable(_)
                    );
                    if stream_dead {
                        return outcome.abort_after_link_failure(
                            username.clone(),
                            error,
                            usernames[index + 1..].iter().cloned(),
                            LinkFailurePolicy::PreservePartialOutcome,
                        );
                    }
                    outcome.failed.push((username.clone(), error));
                }
            }
        }
        Ok(outcome)
    }

    async fn update_users(&self, updates: &[UserUpdate]) -> Result<BulkOutcome, DriverError> {
        // One login serves the whole batch (as create/delete do). A connect/login
        // failure prevents all work → top-level Err; a device-reported per-user
        // failure lands in `failed` without aborting the batch. A dead shared
        // stream stops further calls: the attempted edit carries the error, the
        // tail is recorded as unattempted, and the login already proved the link
        // so the outcome stays per-item even without an acknowledgement.
        let mut session = self.open().await?;
        let mut outcome = BulkOutcome::default();
        for (index, update) in updates.iter().enumerate() {
            match session.modify_user(update).await {
                Ok(()) => outcome.succeeded.push(update.name.clone()),
                Err(error) => {
                    let stream_dead = matches!(
                        &error,
                        DriverError::Transport(_)
                            | DriverError::Timeout(_)
                            | DriverError::Unreachable(_)
                    );
                    if stream_dead {
                        return outcome.abort_after_link_failure(
                            update.name.clone(),
                            error,
                            updates[index + 1..].iter().map(|u| u.name.clone()),
                            LinkFailurePolicy::PreservePartialOutcome,
                        );
                    }
                    outcome.failed.push((update.name.clone(), error));
                }
            }
        }
        Ok(outcome)
    }

    async fn disk_status(&self) -> Result<Vec<DiskStatus>, DriverError> {
        let mut session = self.open().await?;
        let response = session
            .query(codec::command::CONFIG_QUERY, "StorageInfo")
            .await?;
        Ok(parse_storage(&response).0)
    }

    async fn recording_status(&self) -> Result<RecordingStatus, DriverError> {
        let mut session = self.open().await?;
        let response = session
            .query(codec::command::CONFIG_QUERY, "StorageInfo")
            .await?;
        Ok(parse_storage(&response).1)
    }

    async fn search_recordings(
        &self,
        request: &ExportRequest,
    ) -> Result<Vec<RecordSpan>, DriverError> {
        // The exact per-channel primitive the archive plane already uses
        // (`export_download` -> `query_archive_segments`) to find which hour
        // files to claim. Reused as-is: `OPFileQuery` IS a per-channel,
        // time-windowed search, so no new device call is needed.
        let mut session = self.open().await?;
        let segments = session
            .query_archive_segments(request.channel, request.start, request.end)
            .await?;
        Ok(segments
            .into_iter()
            .map(|segment| RecordSpan {
                start: segment.start,
                end: segment.end,
                channel: Some(request.channel),
            })
            .collect())
    }

    async fn snapshot(&self, channel: Option<u16>) -> Result<SnapshotRef, DriverError> {
        // OPSNAP channels are 0-based on the wire, while the public `channel` is
        // 1-based (matching `stream_uri`), so the first channel (1, the default)
        // maps to wire 0. This 0-based OPSNAP convention follows the reference
        // implementation and is not separately live-verified; the read plane is.
        let wire_channel = channel.unwrap_or(1).saturating_sub(1);
        let mut session = self.open().await?;
        let payload = json!({
            "Name": "OPSNAP",
            "SessionID": session.session_hex(),
            "OPSNAP": { "Channel": wire_channel },
        });
        let body = session
            .request_raw(codec::command::SNAPSHOT, &payload)
            .await?;
        // Some firmwares answer OPSNAP with a JSON status frame (msgid 1561)
        // carrying a non-OK `Ret` and no image, instead of a JPEG media frame —
        // e.g. XMeye NVRs whose firmware lacks still capture reply `Ret 102`.
        // Verified live on an AHB8016T-MH: OPSNAP is rejected while the OPMonitor
        // video stream works, so a still needs a stream keyframe decode, not this
        // path. Surface that as an unsupported operation (a clean, client-actionable
        // "no still available") rather than a JPEG-decode failure. A JSON object
        // reply never carries an inline JPEG, so any status frame ends the path.
        if opsnap_status_ret(&body).is_some() {
            return Err(DriverError::Unsupported);
        }
        let jpeg = extract_jpeg(&body)?;
        Ok(SnapshotRef::Bytes(jpeg))
    }

    async fn stream_uri(&self, channel: Option<u16>) -> Result<StreamUri, DriverError> {
        let channel = channel.unwrap_or(1);
        // This is a bespoke XMeye `user=..&password=..` query form, not the
        // standard `user:pass@host` RTSP userinfo — so an unencoded `& = / ? #`
        // or whitespace in either credential would break the query structure
        // (and, downstream, defeat `StreamUri`'s `Debug` redaction, which expects
        // each credential to render as one opaque `key=value` token). Percent-
        // encode both values at construction time rather than patching the
        // redaction tokenizer.
        let uri = format!(
            "rtsp://{host}:554/user={user}&password={password}&channel={channel}&stream=0.sdp?",
            host = self.endpoint.host,
            user = encode_query_value(&self.username),
            password = encode_query_value(self.password.expose()),
        );
        Ok(StreamUri {
            uri,
            transport: StreamTransport::Rtsp,
        })
    }

    async fn substream_uri(&self, channel: Option<u16>) -> Result<StreamUri, DriverError> {
        let channel = channel.unwrap_or(1);
        // Same XMeye credential-path query form as `stream_uri`, with only the
        // stream index literal changed: `stream=1.sdp` selects the substream,
        // `stream=0.sdp` (what `stream_uri` builds) the mainstream. The
        // mainstream form is the one live-verified against real devices; the
        // substream index follows the same documented XMeye convention but has
        // not itself been exercised against a device.
        let uri = format!(
            "rtsp://{host}:554/user={user}&password={password}&channel={channel}&stream=1.sdp?",
            host = self.endpoint.host,
            user = encode_query_value(&self.username),
            password = encode_query_value(self.password.expose()),
        );
        Ok(StreamUri {
            uri,
            transport: StreamTransport::Rtsp,
        })
    }

    async fn list_channels(&self) -> Result<Vec<Channel>, DriverError> {
        let mut session = self.open().await?;
        // Prefer per-channel display names from the `ChannelTitle` config. Some
        // firmware rejects that query (`Ret 102`, verified live on an AHB8016T-MH,
        // the same firmware that rejects `OPSNAP`), so fall back to the SystemInfo
        // channel count and synthesize unnamed channels — enough for the operator
        // to select a camera. The failed query leaves the session usable.
        let named = match session
            .query(codec::command::CONFIG_QUERY, "ChannelTitle")
            .await
        {
            Ok(response) => {
                let named = parse_channel_titles(&response);
                (!named.is_empty()).then_some(named)
            }
            Err(_) => None,
        };
        let mut channels = match named {
            Some(named) => named,
            None => {
                let info = session
                    .query(codec::command::CONFIG_QUERY, "SystemInfo")
                    .await?;
                let count = parse_system_info(&info).channels.unwrap_or(0);
                if count == 0 {
                    return Err(DriverError::Unsupported);
                }
                (1..=count)
                    .map(|index| Channel {
                        index,
                        name: None,
                        enabled: None,
                        ptz_support: PtzSupport::Unknown,
                        recording: None,
                    })
                    .collect()
            }
        };
        // Best-effort enrichment: firmware that rejects this config query (or
        // has no `PTZProtocol` entries) leaves every channel at its
        // already-`Unknown` default rather than failing the whole listing.
        if let Ok(response) = session
            .query(codec::command::CONFIG_QUERY, "PTZProtocol")
            .await
        {
            apply_ptz_support(&mut channels, &response);
        }
        Ok(channels)
    }

    async fn ptz(&self, channel: u16, command: PtzCommand) -> Result<(), DriverError> {
        // DVRIP/Sofia PTZ is `OPPTZControl` (msgid 1400): a `Command` string
        // (DirectionUp/…/ZoomTile/GotoPreset) plus a `Parameter` block with the
        // 0-based channel (matching `snapshot`) and a `Step` speed. A continuous
        // move sends the dominant axis with `Pattern: "Start"`; `Stop` halts
        // every direction a move could have started (there is no single "stop"
        // opcode), mirroring the Dahua per-code stop; a preset uses
        // `Command: "GotoPreset"` with the preset id.
        let wire_channel = channel.saturating_sub(1);
        let mut session = self.open().await?;
        match command {
            PtzCommand::Move(vector) => {
                let Some((cmd, step)) = dvrip_move_command(vector.clamped()) else {
                    return Ok(()); // all-zero move is a no-op
                };
                let payload =
                    ptz_payload(session.session_hex(), cmd, wire_channel, step, -1, "Start");
                session.request(codec::command::PTZ, &payload).await?;
            }
            PtzCommand::Stop => {
                for cmd in PTZ_DIRECTIONS {
                    let payload =
                        ptz_payload(session.session_hex(), cmd, wire_channel, 0, -1, "Stop");
                    // Best-effort: one direction failing must not block the rest.
                    let _ = session.request(codec::command::PTZ, &payload).await;
                }
            }
            PtzCommand::Preset { id } => {
                let payload = ptz_payload(
                    session.session_hex(),
                    "GotoPreset",
                    wire_channel,
                    0,
                    i32::from(id),
                    "Start",
                );
                session.request(codec::command::PTZ, &payload).await?;
            }
        }
        Ok(())
    }

    async fn export_download(
        &self,
        request: &ExportRequest,
    ) -> Result<Option<DownloadStream>, DriverError> {
        // This family's archive is not RTSP: it is served over the same :34567
        // control connection via `OPFileQuery` + `OPPlayBack`, and delivers a
        // bare elementary stream wrapped in XM media packets rather than a
        // container. The driver therefore owns the whole read: enumerate the
        // segments covering the window, replay each one bounded to the caller's
        // sub-window, de-frame the wrappers, and emit annex-B bytes the shared
        // remux plane turns into a fragmented MP4 with `-c copy`.
        //
        // Stitching happens HERE rather than in the service on purpose: each
        // remux produces its own `ftyp`/`moov`, so concatenating per-segment MP4s
        // would not yield a playable stream. Elementary-stream bytes concatenate
        // cleanly, which is exactly why the join belongs below the remux.
        let mut session = self.open().await?;
        let segments = session
            .query_archive_segments(request.channel, request.start, request.end)
            .await?;
        if segments.is_empty() {
            return Ok(None);
        }

        // Read the FIRST video packet before returning so the frame rate is
        // known up front: the remux must receive `-r` before it starts, and a
        // bare elementary stream would otherwise be assumed to be 25 fps.
        // Reopening needs the same transport and credentials this driver holds.
        let transport = Arc::clone(&self.transport);
        let endpoint = self.endpoint.clone();
        let username = self.username.clone();
        let password = self.password.clone();
        let reconnect: SessionFactory = Arc::new(move || {
            let transport = Arc::clone(&transport);
            let endpoint = endpoint.clone();
            let username = username.clone();
            let password = password.clone();
            Box::pin(async move {
                let stream = transport.connect(&endpoint).await?;
                let mut session = DvripSession {
                    stream,
                    session_id: 0,
                    sequence: 0,
                };
                session.login(&username, &password).await?;
                Ok(session)
            })
        });
        let mut reader =
            ArchiveReader::new(session, reconnect, segments, request.start, request.end);
        let Some(prelude) = reader.read_until_hinted().await? else {
            return Ok(None);
        };
        let hints = RemuxHints {
            fps: reader.deframer.hints().map(|hints| hints.fps),
        };

        let filename = format!(
            "dvrip-ch{channel}-{start}",
            channel = request.channel,
            start = request.start.format("%Y%m%dT%H%M%SZ"),
        );
        // Read on a task rather than on the consumer's pull.
        //
        // The device drops an idle session after the interval it reports at
        // login (~21 s observed). A pull-driven reader only advances its
        // keepalive while the consumer is actually pulling — which is exactly
        // when the session is NOT idle. Under HTTP backpressure (a slow client,
        // or a transcode that encodes slower than the device sends) the pull
        // stops, the keepalive stops with it, and the device tears the session
        // down mid-window. This task waits for a channel slot and the keepalive
        // deadline together, so the session stays alive while the consumer is
        // behind.
        let (sender, receiver) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(4);
        tokio::spawn(pump_archive(reader, sender));
        let bytes = futures_util::stream::once(async move { Ok(Bytes::from(prelude)) }).chain(
            futures_util::stream::unfold(receiver, |mut receiver| async move {
                receiver.recv().await.map(|chunk| (chunk, receiver))
            }),
        );

        Ok(Some(DownloadStream {
            bytes: Box::pin(bytes),
            filename,
            hints,
        }))
    }
}

/// Read archive chunks ahead of the HTTP consumer while keeping the DVR session
/// alive under backpressure.
async fn pump_archive(
    mut reader: ArchiveReader,
    sender: tokio::sync::mpsc::Sender<Result<Bytes, std::io::Error>>,
) {
    loop {
        let permit = tokio::select! {
            permit = sender.reserve() => match permit {
                Ok(permit) => permit,
                // Consumer dropped: end the read and close the session.
                Err(_) => return,
            },
            () = tokio::time::sleep_until(reader.next_keepalive) => {
                if let Err(error) = reader.session.send_keepalive().await {
                    // No channel permit exists in this branch. Reserve one now
                    // so a failed keepalive is an item in the stream, not a
                    // clean EOF caused by silently dropping the sender.
                    let _ = sender
                        .send(Err(std::io::Error::other(error.to_string())))
                        .await;
                    return;
                }
                reader.arm_keepalive();
                continue;
            }
        };
        match reader.next_video_chunk().await {
            Ok(Some(chunk)) => permit.send(Ok(Bytes::from(chunk))),
            Ok(None) => return,
            // A device failure mid-window must truncate the clip visibly, not
            // end it as a clean EOF that looks complete.
            Err(error) => {
                permit.send(Err(std::io::Error::other(error.to_string())));
                return;
            }
        }
    }
}

/// Timestamp coverage observed inside one claimed segment sub-window.
///
/// XM I-frame headers carry a packed wall-clock timestamp. Tracking the first
/// and last one lets a silence-only protocol ending distinguish a completed
/// claim from a device that stalled halfway through it.
struct ArchiveWindowCoverage {
    expected_start: DateTime<Utc>,
    expected_end: DateTime<Utc>,
    first: Option<DateTime<Utc>>,
    last: Option<DateTime<Utc>>,
    /// Tail retained so an I-frame header split across DVRIP frames is visible.
    scan_tail: Vec<u8>,
}

impl ArchiveWindowCoverage {
    fn new(expected_start: DateTime<Utc>, expected_end: DateTime<Utc>) -> Self {
        Self {
            expected_start,
            expected_end,
            first: None,
            last: None,
            scan_tail: Vec::new(),
        }
    }

    /// Observe every timestamped I-frame header in the joined XM byte stream.
    fn observe(&mut self, chunk: &[u8]) {
        const I_FRAME_MAGIC: [u8; 4] = [0x00, 0x00, 0x01, codec::media::KIND_VIDEO_I];
        const TIMESTAMP_END: usize = 12;

        self.scan_tail.extend_from_slice(chunk);
        let mut cursor = 0;
        while let Some(relative) = self.scan_tail[cursor..]
            .windows(I_FRAME_MAGIC.len())
            .position(|window| window == I_FRAME_MAGIC)
        {
            let position = cursor + relative;
            if position + TIMESTAMP_END > self.scan_tail.len() {
                break;
            }
            let offset = position + 8;
            let packed = u32::from_le_bytes([
                self.scan_tail[offset],
                self.scan_tail[offset + 1],
                self.scan_tail[offset + 2],
                self.scan_tail[offset + 3],
            ]);
            if let Some(timestamp) = parse_archive_media_timestamp(packed) {
                self.first.get_or_insert(timestamp);
                self.last = Some(timestamp);
            }
            cursor = position + I_FRAME_MAGIC.len();
        }

        // Eleven bytes are enough to retain a split four-byte magic plus the
        // seven timestamp-prefix bytes before the final timestamp byte.
        let consumed = self.scan_tail.len().saturating_sub(TIMESTAMP_END - 1);
        self.scan_tail.drain(..consumed);
    }

    fn verify(self) -> Result<(), DriverError> {
        let Some((first, last)) = self.first.zip(self.last) else {
            return Err(DriverError::Timeout(format!(
                "archive playback ended before timestamp coverage of {}..{} could be verified",
                self.expected_start, self.expected_end
            )));
        };
        let started_in_time = first <= self.expected_start + ARCHIVE_COVERAGE_TOLERANCE;
        let reached_end = last >= self.expected_end - ARCHIVE_COVERAGE_TOLERANCE;
        if started_in_time && reached_end {
            return Ok(());
        }
        Err(DriverError::Timeout(format!(
            "archive playback ended with incomplete coverage {first}..{last}; expected {}..{}",
            self.expected_start, self.expected_end
        )))
    }
}

/// Decode Xiongmai's packed `year/month/day/hour/minute/second` I-frame field.
fn parse_archive_media_timestamp(packed: u32) -> Option<DateTime<Utc>> {
    let year = 2000 + i32::try_from((packed >> 26) & 0x3F).ok()?;
    let month = (packed >> 22) & 0x0F;
    let day = (packed >> 17) & 0x1F;
    let hour = (packed >> 12) & 0x1F;
    let minute = (packed >> 6) & 0x3F;
    let second = packed & 0x3F;
    Some(
        NaiveDate::from_ymd_opt(year, month, day)?
            .and_hms_opt(hour, minute, second)?
            .and_utc(),
    )
}

/// Drives archive playback across the segments covering one window.
///
/// Owns the authenticated session for the whole read: the stream returned to the
/// media plane borrows nothing, so dropping it drops the session and closes the
/// device connection.
struct ArchiveReader {
    session: DvripSession,
    /// How to open a replacement session: the device refuses a second claim on
    /// a socket that has already served one (`Ret 103`), so every segment after
    /// the first needs a fresh, freshly-authenticated connection.
    reconnect: SessionFactory,
    segments: std::collections::VecDeque<ArchiveSegment>,
    /// Caller's requested window; each segment is clamped into it.
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    deframer: codec::MediaDeframer,
    /// Whether a download is currently running on the session.
    streaming: bool,
    /// Timestamp coverage for the sub-window currently being downloaded.
    current_coverage: Option<ArchiveWindowCoverage>,
    /// Maximum silence before the current segment is considered ended.
    frame_idle_timeout: std::time::Duration,
    /// When the next keepalive is due, so a long window is not dropped.
    next_keepalive: tokio::time::Instant,
    /// Absolute ceiling for the whole read.
    ///
    /// The media plane's own idle and duration timeouts only start once it is
    /// pumping, so everything up to the first frame — segment enumeration and
    /// every claim — would otherwise be bounded only by a per-read deadline that
    /// any device byte resets. A half-open connection could then burn one read
    /// timeout per segment while holding a bounded download slot.
    deadline: tokio::time::Instant,
    /// Consecutive segments the device refused, so a systematically failing
    /// device ends the read instead of walking the whole list.
    consecutive_skips: usize,
}

/// Opens a fresh authenticated session to the same device.
type SessionFactory = Arc<
    dyn Fn() -> Pin<Box<dyn Future<Output = Result<DvripSession, DriverError>> + Send>>
        + Send
        + Sync,
>;

/// How often to send a keepalive during a long archive download. Devices report
/// their own interval at login (~21 s observed); this stays comfortably under
/// the smallest value seen.
const ARCHIVE_KEEPALIVE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);

/// Absolute ceiling on one archive read, covering the phase before the media
/// plane's own timeouts take effect.
const ARCHIVE_MAX_DURATION: std::time::Duration = std::time::Duration::from_secs(600);

/// How many segments in a row the device may refuse before the read gives up.
const MAX_CONSECUTIVE_SEGMENT_SKIPS: usize = 3;

/// Upper bound on segments honoured from one `OPFileQuery` reply. A window is
/// already capped at the REST layer, so a list longer than this is a device
/// fault rather than a legitimate archive.
const MAX_ARCHIVE_SEGMENTS: usize = 512;

impl ArchiveReader {
    fn new(
        session: DvripSession,
        reconnect: SessionFactory,
        segments: Vec<ArchiveSegment>,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Self {
        let now = tokio::time::Instant::now();
        Self {
            session,
            reconnect,
            segments: segments.into(),
            start,
            end,
            deframer: codec::MediaDeframer::new(),
            streaming: false,
            current_coverage: None,
            frame_idle_timeout: ARCHIVE_IDLE_TIMEOUT,
            next_keepalive: now + ARCHIVE_KEEPALIVE_INTERVAL,
            deadline: now + ARCHIVE_MAX_DURATION,
            consecutive_skips: 0,
        }
    }

    /// Pull chunks until the de-framer has seen an I-frame header (and therefore
    /// knows the frame rate), returning everything read so far.
    async fn read_until_hinted(&mut self) -> Result<Option<Vec<u8>>, DriverError> {
        let mut prelude = Vec::new();
        while self.deframer.hints().is_none() {
            match self.next_video_chunk().await? {
                Some(chunk) => prelude.extend_from_slice(&chunk),
                // Stream ended before any I-frame: whatever arrived cannot be
                // decoded on its own, so report "no footage" rather than emit it.
                None => return Ok((!prelude.is_empty()).then_some(prelude)),
            }
        }
        Ok(Some(prelude))
    }

    /// Next batch of elementary-stream bytes, or `None` when every segment in
    /// the window has been replayed.
    async fn next_video_chunk(&mut self) -> Result<Option<Vec<u8>>, DriverError> {
        loop {
            self.check_deadline()?;
            if !self.streaming && !self.start_next_segment().await? {
                return Ok(None);
            }
            self.send_keepalive_if_due().await?;
            match self
                .session
                .next_archive_frame(self.frame_idle_timeout)
                .await?
            {
                Some(frame) => {
                    let Some(coverage) = self.current_coverage.as_mut() else {
                        return Err(DriverError::Protocol(
                            "archive media arrived without an active segment".to_owned(),
                        ));
                    };
                    coverage.observe(&frame);
                    let video = self.deframer.push(&frame);
                    if !video.is_empty() {
                        return Ok(Some(video));
                    }
                    // Frame carried only audio or a partial packet: keep reading.
                }
                // End of this segment's window.
                None => {
                    self.verify_current_coverage()?;
                    if !self.roll_over_to_next_segment().await? {
                        return Ok(None);
                    }
                }
            }
        }
    }

    /// A silent or disconnected device is a clean segment end only after the
    /// timestamped media proves the whole claimed sub-window was delivered.
    fn verify_current_coverage(&mut self) -> Result<(), DriverError> {
        let coverage = self.current_coverage.take().ok_or_else(|| {
            DriverError::Protocol("archive segment ended without coverage state".to_owned())
        })?;
        coverage.verify()
    }

    /// Fail the read once the absolute ceiling has passed.
    ///
    /// Every step that can block — segment enumeration, each claim, each frame —
    /// is gated on this, because the media plane's own idle/duration timeouts
    /// only start once bytes are flowing (see [`ArchiveReader::deadline`]).
    fn check_deadline(&self) -> Result<(), DriverError> {
        if tokio::time::Instant::now() >= self.deadline {
            return Err(DriverError::Timeout(
                "archive read exceeded its maximum duration".to_owned(),
            ));
        }
        Ok(())
    }

    /// Keep a long window alive, so the device does not drop a slow transfer.
    async fn send_keepalive_if_due(&mut self) -> Result<(), DriverError> {
        if tokio::time::Instant::now() >= self.next_keepalive {
            self.session.send_keepalive().await?;
            self.arm_keepalive();
        }
        Ok(())
    }

    /// Finish the current segment and open a session for the next one, reporting
    /// whether any segment remains.
    ///
    /// A remaining segment needs a FRESH session: live capture shows the device
    /// answering a second claim on the same socket with `Ret 103` and sending
    /// nothing, so reusing the connection would silently truncate a
    /// multi-segment window to its first segment and still look like a
    /// successful transfer.
    async fn roll_over_to_next_segment(&mut self) -> Result<bool, DriverError> {
        self.streaming = false;
        if self.segments.is_empty() {
            return Ok(false);
        }
        self.session = (self.reconnect)().await?;
        self.arm_keepalive();
        Ok(true)
    }

    /// Schedule the next keepalive one interval out from now.
    fn arm_keepalive(&mut self) {
        self.next_keepalive = tokio::time::Instant::now() + ARCHIVE_KEEPALIVE_INTERVAL;
    }

    /// Claim and start the next segment, clamped to the requested window.
    ///
    /// Returns `false` when no segments remain. A segment the device refuses is
    /// skipped rather than failing the whole export: a partially readable
    /// archive is more useful than none, and the refusal is per-file.
    async fn start_next_segment(&mut self) -> Result<bool, DriverError> {
        while let Some(segment) = self.segments.pop_front() {
            self.check_deadline()?;
            let start = segment.start.max(self.start);
            let end = segment.end.min(self.end);
            if end <= start {
                continue;
            }
            match self
                .session
                .start_archive_download(&segment, start, end)
                .await
            {
                Ok(()) => {
                    self.streaming = true;
                    self.current_coverage = Some(ArchiveWindowCoverage::new(start, end));
                    self.consecutive_skips = 0;
                    return Ok(true);
                }
                // A device that is gone, or that rejects our credentials, will
                // reject every remaining segment too — failing now beats paying
                // a read timeout per file while holding a bounded media slot.
                Err(
                    error @ (DriverError::Auth
                    | DriverError::Transport(_)
                    | DriverError::Unreachable(_)
                    | DriverError::Timeout(_)),
                ) => return Err(error),
                // A per-file rejection is survivable: a partially readable
                // archive beats none. But a run of them means the device is
                // refusing everything, not that one file is bad.
                Err(error) => {
                    self.consecutive_skips += 1;
                    if self.consecutive_skips >= MAX_CONSECUTIVE_SEGMENT_SKIPS {
                        return Err(error);
                    }
                }
            }
        }
        Ok(false)
    }
}

/// The six directional/zoom `OPPTZControl` commands a continuous move can
/// start — used to halt every axis on `Stop`.
const PTZ_DIRECTIONS: [&str; 6] = [
    "DirectionUp",
    "DirectionDown",
    "DirectionLeft",
    "DirectionRight",
    "ZoomTile",
    "ZoomWide",
];

/// Map a clamped PTZ vector to the single dominant `OPPTZControl` command and a
/// 1..=8 step speed. Zoom outranks tilt outranks pan only when it is the
/// largest-magnitude axis; an all-zero vector yields `None` (a no-op move).
fn dvrip_move_command(vector: PtzVector) -> Option<(&'static str, u8)> {
    let axes = [
        (
            vector.pan.abs(),
            if vector.pan >= 0.0 {
                "DirectionRight"
            } else {
                "DirectionLeft"
            },
        ),
        (
            vector.tilt.abs(),
            if vector.tilt >= 0.0 {
                "DirectionUp"
            } else {
                "DirectionDown"
            },
        ),
        (
            vector.zoom.abs(),
            if vector.zoom >= 0.0 {
                "ZoomTile"
            } else {
                "ZoomWide"
            },
        ),
    ];
    // Pick the dominant axis directly from the fixed three-element array; ties
    // resolve to the later axis (zoom > tilt > pan), matching `max_by`'s
    // last-maximal semantics without an infallible unwrap.
    let [pan_axis, tilt_axis, zoom_axis] = axes;
    let dominant = if tilt_axis.0.total_cmp(&pan_axis.0).is_ge() {
        tilt_axis
    } else {
        pan_axis
    };
    let (magnitude, cmd) = if zoom_axis.0.total_cmp(&dominant.0).is_ge() {
        zoom_axis
    } else {
        dominant
    };
    if magnitude <= f32::EPSILON {
        return None;
    }
    // Map the 0..1 magnitude onto the device's 1..=8 step scale.
    let step = (magnitude * 8.0).ceil().clamp(1.0, 8.0) as u8;
    Some((cmd, step))
}

/// Build the `OPPTZControl` request payload for one command.
fn ptz_payload(
    session_hex: String,
    command: &str,
    wire_channel: u16,
    step: u8,
    preset: i32,
    pattern: &str,
) -> Value {
    json!({
        "Name": "OPPTZControl",
        "SessionID": session_hex,
        "OPPTZControl": {
            "Command": command,
            "Parameter": {
                "AUX": { "Number": 0, "Status": "On" },
                "Channel": wire_channel,
                "MenuOpts": "Enter",
                "POINT": { "bottom": 0, "left": 0, "right": 0, "top": 0 },
                "Pattern": pattern,
                "Preset": preset,
                "Step": step,
                "Tour": 0
            }
        }
    })
}

// ── Pure parsers (structure verified against live devices) ──────────────────

/// Set each channel's `ptz_support` from a `PTZProtocol` config response (a
/// flat, 0-based array of `{"Enable": bool, ...}` entries, the same wire
/// indexing `ChannelTitle` uses). A channel with no matching entry (index out
/// of range, or the response carries no `PTZProtocol` array at all) keeps its
/// existing `Unknown` — the device either said nothing about that channel or
/// rejected the whole query, neither of which is a confirmed negative.
fn apply_ptz_support(channels: &mut [Channel], response: &Value) {
    let Some(entries) = response.get("PTZProtocol").and_then(Value::as_array) else {
        return;
    };
    for channel in channels.iter_mut() {
        let wire_index = usize::from(channel.index.saturating_sub(1));
        if let Some(enabled) = entries
            .get(wire_index)
            .and_then(|entry| entry.get("Enable"))
            .and_then(Value::as_bool)
        {
            channel.ptz_support = if enabled {
                PtzSupport::Supported
            } else {
                PtzSupport::Unsupported
            };
        }
    }
}

/// Parse the `ChannelTitle` config (a flat array of per-channel display names,
/// 0-based on the wire) into 1-based [`Channel`]s. A blank title yields a
/// nameless channel (the index still identifies it). Non-array/absent → empty.
fn parse_channel_titles(response: &Value) -> Vec<Channel> {
    response
        .get("ChannelTitle")
        .and_then(Value::as_array)
        .map(|titles| {
            titles
                .iter()
                .enumerate()
                .map(|(wire_index, title)| Channel {
                    index: u16::try_from(wire_index)
                        .unwrap_or(u16::MAX)
                        .saturating_add(1),
                    name: title
                        .as_str()
                        .map(str::trim)
                        .filter(|name| !name.is_empty())
                        .map(str::to_owned),
                    enabled: None,
                    ptz_support: PtzSupport::Unknown,
                    recording: None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Parse the `AuthorityList` response (a flat array of authority/permission
/// tokens the device accepts) into owned strings. Non-array/absent → empty.
///
/// VERIFY, UNCONFIRMED AGAINST A LIVE DEVICE: the response envelope shape
/// (`{"AuthorityList": [...], "Ret": 100, ...}`) is inferred by analogy with
/// this session's `Users`/`ChannelTitle` queries (same envelope shape, see
/// [`parse_users`]/[`parse_channel_titles`]) and with the per-user
/// `AuthorityList` array this file already parses inside [`parse_users`] —
/// but no code has ever issued this specific query against a real device.
/// Defensive by design: any shape mismatch yields an empty `Vec` (never a
/// panic or fabricated data), and a device-level rejection (non-100 `Ret`)
/// surfaces as [`DriverError::Device`] via the [`DvripSession::query`]
/// wrapper, never silently swallowed.
fn parse_authority_list(response: &Value) -> Vec<String> {
    response
        .get("AuthorityList")
        .and_then(Value::as_array)
        .map(|tokens| {
            tokens
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Read the `Ret` status code from a response.
fn response_ret(value: &Value) -> Option<i64> {
    value.get("Ret").and_then(Value::as_i64)
}

/// Map a write response to a per-item result: [`RET_OK`] succeeds, anything else
/// is a device-reported failure carrying the `Ret` for diagnosis.
///
/// Idempotency note: the spec allows treating "user already exists" (on create)
/// or "user does not exist" (on delete) as a success/skip *if the device signals
/// it*. We deliberately do **not** special-case any non-OK `Ret` here. The
/// per-operation `Ret` catalogue for `AddUser`/`DelUser` is not dual-source
/// verified, and this driver must never touch a live device to discover it
/// (HARD SAFETY). Guessing a code would risk silently swallowing a genuine
/// failure — insufficient permission, name-too-long, quota — as "already done".
/// Every non-OK `Ret` is therefore surfaced in `failed` *with its code*, so the
/// existence codes can be identified from real traffic and folded in later.
fn ret_ok_or_device(response: &Value, operation: &str) -> Result<(), DriverError> {
    match response_ret(response) {
        Some(RET_OK) => Ok(()),
        other => Err(DriverError::Device(format!(
            "{operation} returned Ret {other:?}"
        ))),
    }
}

/// Fold a `ModifyUser` device rejection with the fact that this same edit
/// already committed a `ModifyPassword` change on-device, so
/// [`DvripSession::modify_user`] never reports a clean per-item failure that
/// silently hides a partial write. `password_committed` is `true` only when the
/// preceding `ModifyPassword` frame in the same edit was accepted by the device;
/// when `false` this is a no-op passthrough. Only the device-reported rejection
/// (from [`ret_ok_or_device`]) is annotated here — a transport/protocol error
/// from the `ModifyUser` request itself is left untouched so its variant (in
/// particular [`DriverError::Transport`]) still drives the caller's mid-batch
/// stream-death detection.
fn annotate_partial_apply(error: DriverError, password_committed: bool) -> DriverError {
    if !password_committed {
        return error;
    }
    let DriverError::Device(message) = &error else {
        return error;
    };
    DriverError::Device(format!(
        "password change already committed on-device before this failure: {message}"
    ))
}

/// Percent-encode a value for the `stream_uri` XMeye credential query.
///
/// [`NON_ALPHANUMERIC`] is deliberately aggressive: it encodes every reserved
/// character (`&`, `=`, `+`, `%`, space, `#`, `?`, …), so a password like
/// `a&b=c` travels as `a%26b%3Dc` and can never be mistaken for query
/// structure or truncate the URI early. Mirrors `dahua.rs`'s
/// `encode_query_value` (same crate, same `NON_ALPHANUMERIC` set).
fn encode_query_value(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}

/// Extract the JPEG still from an `OPSNAP` media frame.
///
/// The device prefixes the image with a small binary preamble, so scan for the
/// first SOI (`0xFF 0xD8`) and take from there to the EOI (`0xFF 0xD9`)
/// inclusive when one is cleanly present — dropping any trailing padding — or to
/// the end otherwise. A single `OPSNAP` reply is one chunk (as the reference
/// implementation returns); a multi-frame reassembly for a very large image is a
/// future refinement. Returns [`DriverError::Device`] when no SOI is present
/// (e.g. an error reply rather than an image).
/// If an OPSNAP reply body is a JSON status frame (an object carrying `Ret`),
/// return that `Ret`. A JSON status reply never carries an inline JPEG — it means
/// the device acknowledged (or rejected) the request without an image — so its
/// presence lets [`DvripDriver::snapshot`] distinguish an unsupported still from
/// a genuine binary media frame. Binary JPEG frames are not valid JSON objects,
/// so they return `None` here and fall through to marker extraction.
fn opsnap_status_ret(body: &[u8]) -> Option<i64> {
    let trimmed = body
        .iter()
        .rposition(|&b| b != 0x00 && b != 0x0A)
        .map_or(&body[..0], |end| &body[..=end]);
    if trimmed.first() != Some(&b'{') {
        return None;
    }
    let value: Value = serde_json::from_slice(trimmed).ok()?;
    response_ret(&value)
}

fn extract_jpeg(body: &[u8]) -> Result<Vec<u8>, DriverError> {
    let soi = find_marker(body, &JPEG_SOI)
        .ok_or_else(|| DriverError::Device("OPSNAP reply contained no JPEG image".to_owned()))?;
    let image = &body[soi..];
    let end = find_marker(image, &JPEG_EOI).map_or(image.len(), |pos| pos + JPEG_EOI.len());
    Ok(image[..end].to_vec())
}

/// Byte offset of the first occurrence of the two-byte `marker` in `haystack`.
fn find_marker(haystack: &[u8], marker: &[u8]) -> Option<usize> {
    haystack
        .windows(marker.len())
        .position(|window| window == marker)
}

/// Parse a `"0x3"` / `"0x00000005"` session id.
fn parse_session_id(value: &str) -> Option<u32> {
    let hex = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))?;
    u32::from_str_radix(hex, 16).ok()
}

/// Parse a hex space value like `"0x001D1C11"` (in mebibytes).
fn parse_hex_mib(value: &str) -> Option<u64> {
    let hex = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    u64::from_str_radix(hex, 16).ok()
}

/// Parse a DVRIP `"YYYY-MM-DD HH:MM:SS"` timestamp; the all-zero sentinel and
/// unparseable values become `None`. The device reports local wall-clock time
/// with no zone, which we treat as UTC.
fn parse_dvr_time(value: &str) -> Option<DateTime<Utc>> {
    if value.starts_with("0000") {
        return None;
    }
    NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S")
        .ok()
        .map(|naive| naive.and_utc())
}

/// Non-empty string field, or `None`.
fn non_empty_str(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn parse_system_info(value: &Value) -> DeviceInfo {
    let info = value.get("SystemInfo");
    let field = |key: &str| info.and_then(|i| non_empty_str(i, key));
    DeviceInfo {
        model: field("HardWare"),
        firmware: field("SoftWareVersion"),
        serial: field("SerialNo"),
        channels: info
            .and_then(|i| i.get("VideoInChannel"))
            .and_then(Value::as_u64)
            .and_then(|n| u16::try_from(n).ok()),
        device_time: None,
    }
}

fn parse_users(value: &Value) -> Vec<UserAccount> {
    let Some(users) = value.get("Users").and_then(Value::as_array) else {
        return Vec::new();
    };
    users
        .iter()
        .filter_map(|user| {
            let name = non_empty_str(user, "Name")?;
            let authorities = user
                .get("AuthorityList")
                .and_then(Value::as_array)
                .map(|list| {
                    list.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            Some(UserAccount {
                name,
                group: non_empty_str(user, "Group"),
                authorities,
            })
        })
        .collect()
}

/// One parsed storage partition: its disk status plus, when the partition
/// carries a recorded segment, the closed span and whether it is the
/// actively-recording partition.
struct PartitionInfo {
    disk: DiskStatus,
    span: Option<RecordSpan>,
    active: bool,
}

/// Parse one storage partition into its disk status and recording contribution.
/// Empty/absent slots (`TotalSpace == 0`) yield `None` so the caller skips them.
fn parse_partition(partition: &Value, name: String) -> Option<PartitionInfo> {
    let total_mib = partition
        .get("TotalSpace")
        .and_then(Value::as_str)
        .and_then(parse_hex_mib)
        .unwrap_or(0);
    if total_mib == 0 {
        return None; // empty/absent slot
    }
    let remain_mib = partition
        .get("RemainSpace")
        .and_then(Value::as_str)
        .and_then(parse_hex_mib)
        .unwrap_or(0);
    let status = partition.get("Status").and_then(Value::as_i64).unwrap_or(0);
    let (state, is_error) = match status {
        0 => (DiskState::Ok, false),
        _ => (DiskState::Unknown, false),
    };
    let disk = DiskStatus {
        name,
        total_bytes: total_mib.saturating_mul(MIB),
        used_bytes: total_mib.saturating_sub(remain_mib).saturating_mul(MIB),
        state,
        is_error,
    };

    let start = partition
        .get("NewStartTime")
        .and_then(Value::as_str)
        .and_then(parse_dvr_time);
    let end = partition
        .get("NewEndTime")
        .and_then(Value::as_str)
        .and_then(parse_dvr_time);
    let is_current = partition
        .get("IsCurrent")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let span = start.zip(end).map(|(start, end)| RecordSpan {
        start,
        end,
        channel: None,
    });
    // A current partition is actively recording even when its end time is still
    // open (the device reports "0000-00-00 ..." until the segment closes), so key
    // `active` on IsCurrent + a valid start, not a closed span.
    let active = is_current && start.is_some();
    Some(PartitionInfo { disk, span, active })
}

fn parse_storage(value: &Value) -> (Vec<DiskStatus>, RecordingStatus) {
    let mut disks = Vec::new();
    let mut spans = Vec::new();
    let mut active = false;

    let storage = value.get("StorageInfo").and_then(Value::as_array);
    for (disk_index, disk) in storage.into_iter().flatten().enumerate() {
        let partitions = disk.get("Partition").and_then(Value::as_array);
        for (part_index, partition) in partitions.into_iter().flatten().enumerate() {
            let name = format!("disk{}-{}", disk_index + 1, part_index + 1);
            let Some(info) = parse_partition(partition, name) else {
                continue;
            };
            disks.push(info.disk);
            if let Some(span) = info.span {
                spans.push(span);
            }
            if info.active {
                active = true;
            }
        }
    }

    let oldest = spans.iter().map(|span| span.start).min();
    let newest = spans.iter().map(|span| span.end).max();
    (
        disks,
        RecordingStatus {
            active,
            oldest,
            newest,
            spans,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Datelike as _, Timelike as _};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn parse_session_id_handles_short_and_padded_forms() {
        assert_eq!(parse_session_id("0x3"), Some(3));
        assert_eq!(parse_session_id("0x00000005"), Some(5));
        assert_eq!(parse_session_id("nope"), None);
    }

    #[test]
    fn parse_hex_mib_reads_space_values() {
        assert_eq!(parse_hex_mib("0x001D1C11"), Some(0x001D_1C11));
        assert_eq!(parse_hex_mib("0x00000000"), Some(0));
    }

    #[test]
    fn parse_dvr_time_rejects_zero_sentinel() {
        assert!(parse_dvr_time("0000-00-00 00:00:00").is_none());
        assert!(parse_dvr_time("2026-06-18 19:02:25").is_some());
    }

    #[test]
    fn parse_system_info_maps_device_fields() {
        let value = json!({"SystemInfo": {
            "HardWare": "AHB8016T-MH",
            "SoftWareVersion": "V4.03.R11",
            "SerialNo": "0123456789abcdef",
            "VideoInChannel": 16,
        }});
        let info = parse_system_info(&value);
        assert_eq!(info.model.as_deref(), Some("AHB8016T-MH"));
        assert_eq!(info.firmware.as_deref(), Some("V4.03.R11"));
        assert_eq!(info.serial.as_deref(), Some("0123456789abcdef"));
        assert_eq!(info.channels, Some(16));
    }

    #[test]
    fn parse_users_extracts_name_group_and_authorities() {
        let value = json!({"Users": [
            {"Name": "admin", "Group": "admin", "AuthorityList": ["Monitor_01", "Replay_01"]},
            {"Name": "", "Group": "user"},
        ]});
        let users = parse_users(&value);
        assert_eq!(users.len(), 1); // the empty-name entry is dropped
        assert_eq!(users[0].name, "admin");
        assert_eq!(users[0].group.as_deref(), Some("admin"));
        assert_eq!(users[0].authorities, ["Monitor_01", "Replay_01"]);
    }

    #[test]
    fn parse_channel_titles_indexes_from_one_and_drops_blank_names() {
        let value = json!({"ChannelTitle": ["CAM01", "", "Entrance"], "Ret": 100});
        let channels = parse_channel_titles(&value);
        assert_eq!(channels.len(), 3);
        assert_eq!(channels[0].index, 1);
        assert_eq!(channels[0].name.as_deref(), Some("CAM01"));
        // A blank title yields a nameless channel; its index still identifies it.
        assert_eq!(channels[1].index, 2);
        assert_eq!(channels[1].name, None);
        assert_eq!(channels[2].index, 3);
        assert_eq!(channels[2].name.as_deref(), Some("Entrance"));
    }

    #[test]
    fn parse_channel_titles_is_empty_without_the_array() {
        assert!(parse_channel_titles(&json!({"Ret": 100})).is_empty());
    }

    #[test]
    fn parse_authority_list_extracts_tokens() {
        let value = json!({"AuthorityList": ["Monitor_01", "Replay_01"], "Ret": 100});
        assert_eq!(
            parse_authority_list(&value),
            vec!["Monitor_01".to_owned(), "Replay_01".to_owned()]
        );
    }

    #[test]
    fn parse_authority_list_is_empty_without_the_array() {
        assert!(parse_authority_list(&json!({"Ret": 100})).is_empty());
    }

    #[test]
    fn parse_storage_filters_empty_slots_and_computes_bytes() {
        let value = json!({"StorageInfo": [{
            "Partition": [
                {"TotalSpace": "0x00000100", "RemainSpace": "0x00000040", "Status": 0,
                 "IsCurrent": true, "NewStartTime": "2026-06-18 19:02:25",
                 "NewEndTime": "2026-07-17 10:32:28"},
                {"TotalSpace": "0x00000000", "RemainSpace": "0x00000000", "Status": 0,
                 "IsCurrent": false, "NewStartTime": "0000-00-00 00:00:00",
                 "NewEndTime": "0000-00-00 00:00:00"},
            ],
        }]});
        let (disks, recording) = parse_storage(&value);
        assert_eq!(disks.len(), 1); // empty slot filtered
        assert_eq!(disks[0].total_bytes, 0x100 * MIB);
        assert_eq!(disks[0].used_bytes, (0x100 - 0x40) * MIB);
        assert_eq!(disks[0].state, DiskState::Ok);
        assert!(recording.active);
        assert_eq!(recording.spans.len(), 1);
        assert!(recording.oldest.is_some() && recording.newest.is_some());
    }

    #[test]
    fn recording_is_active_when_current_partition_has_open_end() {
        let value = json!({"StorageInfo": [{
            "Partition": [
                {"TotalSpace": "0x00000100", "RemainSpace": "0x00000000", "Status": 0,
                 "IsCurrent": true, "NewStartTime": "2026-07-01 00:00:00",
                 "NewEndTime": "0000-00-00 00:00:00"},
            ],
        }]});
        let (_disks, recording) = parse_storage(&value);
        assert!(recording.active); // current partition is recording, end still open
        assert!(recording.spans.is_empty()); // no closed span yet
    }

    #[test]
    fn stream_uri_uses_xmeye_credential_path() {
        // Build a driver with a never-connecting transport; stream_uri is offline.
        struct DeadTransport;
        #[async_trait]
        impl DeviceTransport for DeadTransport {
            async fn connect(
                &self,
                _endpoint: &DeviceEndpoint,
            ) -> Result<Box<dyn DeviceStream>, DriverError> {
                Err(DriverError::Transport("unused".to_owned()))
            }
        }
        let driver = DvripDriver::new(
            Arc::new(DeadTransport),
            DeviceEndpoint::new("cam.test", 34567),
            "admin",
            Secret::new("secretpw"),
        );
        let uri = tokio_block(driver.stream_uri(Some(2))).expect("stream_uri");
        assert_eq!(uri.transport, StreamTransport::Rtsp);
        assert!(uri.uri.contains("channel=2"));
        assert!(uri.uri.contains("cam.test:554"));
        // Debug must not leak the password even though the field carries it.
        assert!(!format!("{uri:?}").contains("secretpw"));
    }

    #[test]
    fn stream_uri_percent_encodes_credentials_that_would_break_the_query_or_redaction() {
        // Build a driver with a never-connecting transport; stream_uri is offline.
        struct DeadTransport;
        #[async_trait]
        impl DeviceTransport for DeadTransport {
            async fn connect(
                &self,
                _endpoint: &DeviceEndpoint,
            ) -> Result<Box<dyn DeviceStream>, DriverError> {
                Err(DriverError::Transport("unused".to_owned()))
            }
        }
        // A username/password containing the query form's own separators (`&`
        // `=`) plus other reserved/whitespace characters (`?` `#` ` `), which
        // would otherwise break the bespoke `user=..&password=..` structure
        // and, as a side effect, defeat `StreamUri`'s `Debug` redaction (it
        // tokenizes on `&`/`?`/`;`).
        let raw_user = "ad&min";
        let raw_password = "pa&ss=w /d#1";
        let driver = DvripDriver::new(
            Arc::new(DeadTransport),
            DeviceEndpoint::new("cam.test", 34567),
            raw_user,
            Secret::new(raw_password),
        );
        let uri = tokio_block(driver.stream_uri(Some(1))).expect("stream_uri");

        // The URI is built from exactly the percent-encoded credentials — no
        // other construction detail changes.
        let expected = format!(
            "rtsp://cam.test:554/user={user}&password={password}&channel=1&stream=0.sdp?",
            user = encode_query_value(raw_user),
            password = encode_query_value(raw_password),
        );
        assert_eq!(uri.uri, expected);

        // The raw separator characters from the credentials never survive
        // unencoded, and the encoded octets are present.
        assert!(uri.uri.contains("%26")); // encoded '&'
        assert!(uri.uri.contains("%3D")); // encoded '='
        assert!(!uri.uri.contains(raw_password));
        assert!(!uri.uri.contains(raw_user));

        // Exactly one `user=`/`password=` token each: encoding preserved the
        // query structure instead of the credential splintering it into extra
        // tokens.
        assert_eq!(uri.uri.matches("user=").count(), 1);
        assert_eq!(uri.uri.matches("password=").count(), 1);

        // Debug redaction now masks the whole credential cleanly — no raw
        // fragment of either credential leaks through a broken token boundary.
        let debug = format!("{uri:?}");
        assert!(!debug.contains(raw_password));
        assert!(!debug.contains(raw_user));
        assert!(debug.contains("password=***"));
    }

    #[test]
    fn substream_uri_uses_stream_1_in_the_xmeye_credential_path() {
        // Build a driver with a never-connecting transport; substream_uri is
        // offline, just like stream_uri.
        struct DeadTransport;
        #[async_trait]
        impl DeviceTransport for DeadTransport {
            async fn connect(
                &self,
                _endpoint: &DeviceEndpoint,
            ) -> Result<Box<dyn DeviceStream>, DriverError> {
                Err(DriverError::Transport("unused".to_owned()))
            }
        }
        let driver = DvripDriver::new(
            Arc::new(DeadTransport),
            DeviceEndpoint::new("cam.test", 34567),
            "admin",
            Secret::new("secretpw"),
        );
        let uri = tokio_block(driver.substream_uri(Some(2))).expect("substream_uri");
        assert_eq!(uri.transport, StreamTransport::Rtsp);
        assert!(uri.uri.contains("channel=2"));
        assert!(uri.uri.contains("cam.test:554"));
        // Only the stream index literal differs from stream_uri.
        assert!(uri.uri.contains("stream=1.sdp"));
        assert!(!uri.uri.contains("stream=0.sdp"));
        // Debug must not leak the password even though the field carries it.
        assert!(!format!("{uri:?}").contains("secretpw"));
    }

    /// A transport that replays a fixed script of JSON responses, one per request.
    struct ScriptedTransport {
        responses: Vec<Value>,
    }

    #[async_trait]
    impl DeviceTransport for ScriptedTransport {
        async fn connect(
            &self,
            _endpoint: &DeviceEndpoint,
        ) -> Result<Box<dyn DeviceStream>, DriverError> {
            let (client, mut server) = tokio::io::duplex(64 * 1024);
            let responses = self.responses.clone();
            tokio::spawn(async move {
                for response in responses {
                    let mut header = [0u8; codec::HEADER_LEN];
                    if server.read_exact(&mut header).await.is_err() {
                        return;
                    }
                    let declared =
                        u32::from_le_bytes([header[16], header[17], header[18], header[19]])
                            as usize;
                    let mut body = vec![0u8; declared];
                    if server.read_exact(&mut body).await.is_err() {
                        return;
                    }
                    let json = serde_json::to_string(&response).expect("serialize response");
                    let frame = codec::encode(0, 0, 0, &json);
                    if server.write_all(&frame).await.is_err() {
                        return;
                    }
                }
            });
            Ok(Box::new(client))
        }
    }

    #[tokio::test]
    async fn get_info_logs_in_then_queries_and_parses() {
        let login = json!({"Name": "", "Ret": 100, "SessionID": "0x00000005"});
        let system_info = json!({"Name": "SystemInfo", "Ret": 100, "SessionID": "0x5",
            "SystemInfo": {"HardWare": "AHB8016T-MH", "SoftWareVersion": "V4.03",
                           "SerialNo": "0123456789abcdef", "VideoInChannel": 16}});
        let transport = Arc::new(ScriptedTransport {
            responses: vec![login, system_info],
        });
        let driver = DvripDriver::new(
            transport,
            DeviceEndpoint::new("device.test", 34567),
            "admin",
            Secret::new("pw"),
        );

        let info = driver.get_info().await.expect("get_info");
        assert_eq!(info.model.as_deref(), Some("AHB8016T-MH"));
        assert_eq!(info.channels, Some(16));
    }

    #[test]
    fn apply_ptz_support_marks_supported_and_unsupported_from_enable_flag() {
        let response = json!({"Name": "PTZProtocol", "Ret": 100, "SessionID": "0x5",
            "PTZProtocol": [{"Enable": true}, {"Enable": false}]});
        let mut channels = vec![
            Channel {
                index: 1,
                name: None,
                enabled: None,
                ptz_support: PtzSupport::Unknown,
                recording: None,
            },
            Channel {
                index: 2,
                name: None,
                enabled: None,
                ptz_support: PtzSupport::Unknown,
                recording: None,
            },
            Channel {
                index: 3,
                name: None,
                enabled: None,
                ptz_support: PtzSupport::Unknown,
                recording: None,
            },
        ];
        apply_ptz_support(&mut channels, &response);
        assert_eq!(channels[0].ptz_support, PtzSupport::Supported);
        assert_eq!(channels[1].ptz_support, PtzSupport::Unsupported);
        // No PTZProtocol[2] entry in the response -> stays Unknown, not
        // downgraded to Unsupported.
        assert_eq!(channels[2].ptz_support, PtzSupport::Unknown);
    }

    #[tokio::test]
    async fn list_channels_enriches_with_ptz_support_from_the_protocol_config() {
        let login = json!({"Name": "", "Ret": 100, "SessionID": "0x00000005"});
        let channel_titles = json!({"Name": "ChannelTitle", "Ret": 100, "SessionID": "0x5",
            "ChannelTitle": ["Entrance", "Back"]});
        let ptz_protocol = json!({"Name": "PTZProtocol", "Ret": 100, "SessionID": "0x5",
            "PTZProtocol": [{"Enable": true}, {"Enable": false}]});
        let transport = Arc::new(ScriptedTransport {
            responses: vec![login, channel_titles, ptz_protocol],
        });
        let driver = DvripDriver::new(
            transport,
            DeviceEndpoint::new("device.test", 34567),
            "admin",
            Secret::new("pw"),
        );

        let channels = driver.list_channels().await.expect("list_channels");
        assert_eq!(channels[0].ptz_support, PtzSupport::Supported);
        assert_eq!(channels[1].ptz_support, PtzSupport::Unsupported);
    }

    #[tokio::test]
    async fn list_channels_leaves_ptz_support_unknown_when_the_protocol_query_is_rejected() {
        // Firmware that has no PTZProtocol config query answers with a
        // non-OK Ret (mirrors the already-handled ChannelTitle/OPSNAP `Ret
        // 102` rejection case); every channel keeps its Unknown default
        // instead of the whole listing failing.
        let login = json!({"Name": "", "Ret": 100, "SessionID": "0x00000005"});
        let channel_titles = json!({"Name": "ChannelTitle", "Ret": 100, "SessionID": "0x5",
            "ChannelTitle": ["Entrance"]});
        let rejected = json!({"Name": "PTZProtocol", "Ret": 102, "SessionID": "0x5"});
        let transport = Arc::new(ScriptedTransport {
            responses: vec![login, channel_titles, rejected],
        });
        let driver = DvripDriver::new(
            transport,
            DeviceEndpoint::new("device.test", 34567),
            "admin",
            Secret::new("pw"),
        );

        let channels = driver.list_channels().await.expect("list_channels");
        assert_eq!(channels[0].ptz_support, PtzSupport::Unknown);
    }

    #[tokio::test]
    async fn list_authorities_logs_in_then_queries_and_parses() {
        let login = json!({"Name": "", "Ret": 100, "SessionID": "0x00000005"});
        let authority_list = json!({"Name": "AuthorityList", "Ret": 100, "SessionID": "0x5",
            "AuthorityList": ["Monitor_01", "Replay_01", "SystemInfo_01"]});
        let transport = Arc::new(ScriptedTransport {
            responses: vec![login, authority_list],
        });
        let driver = DvripDriver::new(
            transport,
            DeviceEndpoint::new("device.test", 34567),
            "admin",
            Secret::new("pw"),
        );

        let authorities = driver.list_authorities().await.expect("list_authorities");
        assert_eq!(
            authorities,
            vec![
                "Monitor_01".to_owned(),
                "Replay_01".to_owned(),
                "SystemInfo_01".to_owned(),
            ]
        );
    }

    #[tokio::test]
    async fn login_failure_maps_to_auth_error() {
        let login = json!({"Name": "", "Ret": 205, "SessionID": "0x0"});
        let transport = Arc::new(ScriptedTransport {
            responses: vec![login],
        });
        let driver = DvripDriver::new(
            transport,
            DeviceEndpoint::new("device.test", 34567),
            "admin",
            Secret::new("wrong"),
        );
        let error = driver.get_info().await.expect_err("should fail auth");
        assert!(matches!(error, DriverError::Auth));
    }

    /// A transport that replies with a header claiming a 4 GiB body it never sends.
    struct MalformedHeaderTransport;

    #[async_trait]
    impl DeviceTransport for MalformedHeaderTransport {
        async fn connect(
            &self,
            _endpoint: &DeviceEndpoint,
        ) -> Result<Box<dyn DeviceStream>, DriverError> {
            let (client, mut server) = tokio::io::duplex(64 * 1024);
            tokio::spawn(async move {
                // Drain the one request the driver sends, then reply with a header
                // whose length field is u32::MAX and deliver no body.
                let mut request_header = [0u8; codec::HEADER_LEN];
                if server.read_exact(&mut request_header).await.is_err() {
                    return;
                }
                let request_len = u32::from_le_bytes([
                    request_header[16],
                    request_header[17],
                    request_header[18],
                    request_header[19],
                ]) as usize;
                let mut request_body = vec![0u8; request_len];
                let _ = server.read_exact(&mut request_body).await;

                let mut header = [0u8; codec::HEADER_LEN];
                header[0] = 0xFF;
                header[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
                let _ = server.write_all(&header).await;
            });
            Ok(Box::new(client))
        }
    }

    #[tokio::test]
    async fn oversized_response_length_is_rejected_without_hanging() {
        let driver = DvripDriver::new(
            Arc::new(MalformedHeaderTransport),
            DeviceEndpoint::new("device.test", 34567),
            "admin",
            Secret::new("pw"),
        );
        // The cap rejects the response before allocating or blocking on the body,
        // so this resolves promptly rather than hanging on a 4 GiB read.
        let error = tokio::time::timeout(std::time::Duration::from_secs(5), driver.get_info())
            .await
            .expect("driver must not hang on an oversized length")
            .expect_err("oversized response length must be rejected");
        assert!(matches!(error, DriverError::Protocol(_)));
    }

    /// A transport that captures every request frame the driver emits and, per
    /// request, replies with the next scripted response. `close_after` bounds how
    /// many responses it serves; once exhausted it drops the stream so the next
    /// request the driver sends fails at the transport (letting us exercise
    /// mid-batch stream death without waiting on a read timeout). `connects`
    /// counts `connect` calls so a test can assert the whole batch used one
    /// session, and `sends` counts client frames the driver *attempted*, which
    /// stays observable even after the peer has gone away.
    #[derive(Clone)]
    struct CapturingTransport {
        responses: Vec<Value>,
        captured: Arc<std::sync::Mutex<Vec<Vec<u8>>>>,
        connects: Arc<std::sync::atomic::AtomicUsize>,
        sends: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl CapturingTransport {
        fn new(responses: Vec<Value>) -> Self {
            Self {
                responses,
                captured: Arc::new(std::sync::Mutex::new(Vec::new())),
                connects: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                sends: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            }
        }

        /// Decode captured request `index` into its command id and JSON payload.
        fn request(&self, index: usize) -> (u16, Value) {
            let frames = self.captured.lock().expect("capture lock");
            let frame = codec::decode(&frames[index]).expect("decode captured frame");
            let payload: Value =
                serde_json::from_slice(&frame.payload).expect("captured payload is JSON");
            (frame.message_id, payload)
        }

        /// Raw bytes of captured request `index`, terminators and all.
        fn raw(&self, index: usize) -> Vec<u8> {
            self.captured.lock().expect("capture lock")[index].clone()
        }

        /// Frames the server actually read. Flat after the peer disconnects, so
        /// it cannot by itself prove whether a tail request was attempted.
        fn request_count(&self) -> usize {
            self.captured.lock().expect("capture lock").len()
        }

        fn connect_count(&self) -> usize {
            self.connects.load(std::sync::atomic::Ordering::SeqCst)
        }

        /// Frames the driver attempted to send. Incremented before the write, so
        /// it counts a request even when the closed peer makes that write fail.
        fn send_count(&self) -> usize {
            self.sends.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    /// Client stream wrapper counting `send` attempts.
    ///
    /// The server-side read counter cannot observe a request sent after the peer
    /// disconnected: nobody reads it, so the count stays flat whether or not the
    /// driver wrote. Counting the client's own sends makes "the tail was never
    /// attempted" a real assertion.
    struct SendCountingStream {
        inner: tokio::io::DuplexStream,
        sends: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait]
    impl DeviceStream for SendCountingStream {
        async fn send(&mut self, bytes: &[u8]) -> Result<(), DriverError> {
            self.sends.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.inner
                .write_all(bytes)
                .await
                .map_err(|error| DriverError::Unreachable(error.to_string()))
        }

        async fn recv_exact(&mut self, buf: &mut [u8]) -> Result<(), DriverError> {
            self.inner
                .read_exact(buf)
                .await
                .map(|_read| ())
                .map_err(|error| DriverError::Unreachable(error.to_string()))
        }
    }

    #[async_trait]
    impl DeviceTransport for CapturingTransport {
        async fn connect(
            &self,
            _endpoint: &DeviceEndpoint,
        ) -> Result<Box<dyn DeviceStream>, DriverError> {
            self.connects
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let (client, mut server) = tokio::io::duplex(64 * 1024);
            let responses = self.responses.clone();
            let captured = Arc::clone(&self.captured);
            tokio::spawn(async move {
                for response in responses {
                    let mut header = [0u8; codec::HEADER_LEN];
                    if server.read_exact(&mut header).await.is_err() {
                        return;
                    }
                    let declared =
                        u32::from_le_bytes([header[16], header[17], header[18], header[19]])
                            as usize;
                    let mut body = vec![0u8; declared];
                    if server.read_exact(&mut body).await.is_err() {
                        return;
                    }
                    let mut full = Vec::with_capacity(codec::HEADER_LEN + declared);
                    full.extend_from_slice(&header);
                    full.extend_from_slice(&body);
                    captured.lock().expect("capture lock").push(full);

                    let json = serde_json::to_string(&response).expect("serialize response");
                    let frame = codec::encode(0, 0, 0, &json);
                    if server.write_all(&frame).await.is_err() {
                        return;
                    }
                }
                // Responses exhausted: dropping `server` closes the stream, so a
                // further request from the driver fails at the transport.
            });
            Ok(Box::new(SendCountingStream {
                inner: client,
                sends: Arc::clone(&self.sends),
            }))
        }
    }

    /// True if `needle` occurs anywhere in `haystack`.
    fn frame_contains(haystack: &[u8], needle: &[u8]) -> bool {
        !needle.is_empty()
            && haystack
                .windows(needle.len())
                .any(|window| window == needle)
    }

    #[tokio::test]
    async fn create_users_emits_adduser_frames_over_one_session_with_partial_success() {
        let login = json!({"Name": "", "Ret": 100, "SessionID": "0x00000005"});
        // user1 accepted; user2 rejected by the device (e.g. insufficient authority).
        let add_ok = json!({"Name": "AddUser", "Ret": 100, "SessionID": "0x5"});
        let add_denied = json!({"Name": "AddUser", "Ret": 107, "SessionID": "0x5"});
        let transport = Arc::new(CapturingTransport::new(vec![login, add_ok, add_denied]));
        let driver = DvripDriver::new(
            Arc::clone(&transport) as Arc<dyn DeviceTransport>,
            DeviceEndpoint::new("device.test", 34567),
            "admin",
            Secret::new("login-pw"),
        );

        let users = vec![
            NewUser {
                name: "guard1".to_owned(),
                password: Secret::new("s3cr3t-pw-one"),
                group: Some("operators".to_owned()),
                authorities: vec!["Monitor_01".to_owned(), "Replay_01".to_owned()],
            },
            NewUser {
                name: "guard2".to_owned(),
                password: Secret::new("s3cr3t-pw-two"),
                group: None, // exercises the "user" default
                authorities: vec![],
            },
        ];

        let outcome = driver.create_users(&users).await.expect("create_users");

        // Partial success: user1 succeeded, user2 is a per-user device failure
        // (NOT a top-level Err), and the Ret is surfaced for diagnosis.
        assert_eq!(outcome.succeeded, ["guard1"]);
        assert_eq!(outcome.failed.len(), 1);
        assert_eq!(outcome.failed[0].0, "guard2");
        let reason = outcome.failed[0].1.to_string();
        assert!(
            reason.contains("107"),
            "reason should carry the Ret: {reason}"
        );

        // One login served the whole batch: exactly one connect, and the frame
        // sequence is [login, add guard1, add guard2] (not login/add/login/add).
        assert_eq!(transport.connect_count(), 1);
        assert_eq!(transport.request_count(), 3);

        // Frame 1: AddUser for guard1 — exact envelope the driver emits.
        let (msg1, add1) = transport.request(1);
        assert_eq!(msg1, codec::command::ADD_USER);
        assert_eq!(add1["Name"], "AddUser");
        assert_eq!(add1["SessionID"], "0x00000005");
        let user1 = &add1["User"];
        assert_eq!(user1["Name"], "guard1");
        assert_eq!(user1["Group"], "operators");
        assert_eq!(user1["Reserved"], json!(false));
        assert_eq!(user1["Sharable"], json!(true));
        assert_eq!(user1["Memo"], "");
        assert_eq!(user1["AuthorityList"], json!(["Monitor_01", "Replay_01"]));
        // The password is Sofia-hashed, never plaintext, on the wire.
        assert_eq!(user1["Password"], codec::sofia_hash("s3cr3t-pw-one"));
        assert!(
            !frame_contains(&transport.raw(1), b"s3cr3t-pw-one"),
            "plaintext password leaked into the AddUser frame"
        );
        assert!(frame_contains(
            &transport.raw(1),
            codec::sofia_hash("s3cr3t-pw-one").as_bytes()
        ));

        // Frame 2: AddUser for guard2 — the None group defaults to "user".
        let (msg2, add2) = transport.request(2);
        assert_eq!(msg2, codec::command::ADD_USER);
        assert_eq!(add2["User"]["Name"], "guard2");
        assert_eq!(add2["User"]["Group"], "user");
        assert_eq!(add2["User"]["AuthorityList"], json!([]));
        assert!(!frame_contains(&transport.raw(2), b"s3cr3t-pw-two"));
    }

    #[tokio::test]
    async fn delete_users_emits_deluser_frame_keyed_by_name() {
        let login = json!({"Name": "", "Ret": 100, "SessionID": "0x00000005"});
        let del_ok = json!({"Name": "guard1", "Ret": 100, "SessionID": "0x5"});
        let transport = Arc::new(CapturingTransport::new(vec![login, del_ok]));
        let driver = DvripDriver::new(
            Arc::clone(&transport) as Arc<dyn DeviceTransport>,
            DeviceEndpoint::new("device.test", 34567),
            "admin",
            Secret::new("login-pw"),
        );

        let outcome = driver
            .delete_users(&["guard1".to_owned()])
            .await
            .expect("delete_users");
        assert_eq!(outcome.succeeded, ["guard1"]);
        assert!(outcome.failed.is_empty());

        assert_eq!(transport.connect_count(), 1);
        let (msg, del) = transport.request(1);
        assert_eq!(msg, codec::command::DELETE_USER);
        // DelUser keys the username under "Name" directly (no nested "User").
        assert_eq!(del["Name"], "guard1");
        assert_eq!(del["SessionID"], "0x00000005");
        assert!(del.get("User").is_none());
    }

    #[tokio::test]
    async fn update_users_emits_modifypassword_then_modifyuser_over_one_session() {
        let login = json!({"Name": "", "Ret": 100, "SessionID": "0x00000005"});
        let password_ok = json!({"Name": "ModifyPassword", "Ret": 100, "SessionID": "0x5"});
        let modify_ok = json!({"Name": "ModifyUser", "Ret": 100, "SessionID": "0x5"});
        let transport = Arc::new(CapturingTransport::new(vec![login, password_ok, modify_ok]));
        let driver = DvripDriver::new(
            Arc::clone(&transport) as Arc<dyn DeviceTransport>,
            DeviceEndpoint::new("device.test", 34567),
            "admin",
            Secret::new("login-pw"),
        );

        let updates = vec![UserUpdate {
            name: "guard1".to_owned(),
            password: Some(Secret::new("new-s3cr3t")),
            group: Some("operators".to_owned()),
            authorities: Some(vec!["Monitor_01".to_owned()]),
        }];
        let outcome = driver.update_users(&updates).await.expect("update_users");
        assert_eq!(outcome.succeeded, ["guard1"]);
        assert!(outcome.failed.is_empty());

        // One login served the edit: [login, ModifyPassword, ModifyUser].
        // ModifyPassword goes FIRST — it is the frame more likely to be
        // rejected (see the HARD SAFETY note on `modify_user`), so a rejection
        // aborts before any group/authority (privilege) change reaches the
        // device.
        assert_eq!(transport.connect_count(), 1);
        assert_eq!(transport.request_count(), 3);

        // Frame 1: ModifyPassword carries the Sofia-hashed new password; the
        // plaintext is never on the wire.
        let (msg1, password) = transport.request(1);
        assert_eq!(msg1, codec::command::MODIFY_PASSWORD);
        assert_eq!(password["Name"], "ModifyPassword");
        assert_eq!(password["User"]["Name"], "guard1");
        assert_eq!(
            password["User"]["Password"],
            codec::sofia_hash("new-s3cr3t")
        );
        assert!(
            !frame_contains(&transport.raw(1), b"new-s3cr3t"),
            "plaintext password leaked into the ModifyPassword frame"
        );

        // Frame 2: ModifyUser carries only the changed record fields — Name (the
        // identity), Group, AuthorityList — and never a Password.
        let (msg2, modify) = transport.request(2);
        assert_eq!(msg2, codec::command::MODIFY_USER);
        assert_eq!(modify["Name"], "ModifyUser");
        assert_eq!(modify["SessionID"], "0x00000005");
        assert_eq!(modify["User"]["Name"], "guard1");
        assert_eq!(modify["User"]["Group"], "operators");
        assert_eq!(modify["User"]["AuthorityList"], json!(["Monitor_01"]));
        assert!(modify["User"].get("Password").is_none());

        assert!(driver.capabilities().user_update);
    }

    #[tokio::test]
    async fn update_users_aborts_group_change_when_modifypassword_is_rejected_first() {
        // The order-of-operations fix: a rejected password reset must never let a
        // subsequent group/authority (privilege) change reach the device.
        let login = json!({"Name": "", "Ret": 100, "SessionID": "0x00000005"});
        let password_denied = json!({"Name": "ModifyPassword", "Ret": 107, "SessionID": "0x5"});
        let transport = Arc::new(CapturingTransport::new(vec![login, password_denied]));
        let driver = DvripDriver::new(
            Arc::clone(&transport) as Arc<dyn DeviceTransport>,
            DeviceEndpoint::new("device.test", 34567),
            "admin",
            Secret::new("login-pw"),
        );

        let updates = vec![UserUpdate {
            name: "guard1".to_owned(),
            password: Some(Secret::new("new-s3cr3t")),
            group: Some("admin".to_owned()),
            authorities: None,
        }];
        let outcome = driver.update_users(&updates).await.expect("update_users");
        assert!(outcome.succeeded.is_empty());
        assert_eq!(outcome.failed.len(), 1);
        assert_eq!(outcome.failed[0].0, "guard1");
        assert!(outcome.failed[0].1.to_string().contains("107"));

        // Only ModifyPassword was ever sent — the ModifyUser (group=admin
        // escalation) frame was never emitted, because the password rejection
        // aborted the edit before it.
        assert_eq!(transport.request_count(), 2);
        let (msg, _payload) = transport.request(1);
        assert_eq!(msg, codec::command::MODIFY_PASSWORD);
    }

    #[tokio::test]
    async fn update_users_reports_partial_apply_when_modifyuser_rejects_after_modifypassword_commits()
     {
        // ModifyPassword succeeds (committed on-device), then ModifyUser is
        // rejected: the per-item failure must say so honestly instead of
        // reading as a clean, no-effect failure.
        let login = json!({"Name": "", "Ret": 100, "SessionID": "0x00000005"});
        let password_ok = json!({"Name": "ModifyPassword", "Ret": 100, "SessionID": "0x5"});
        let modify_denied = json!({"Name": "ModifyUser", "Ret": 107, "SessionID": "0x5"});
        let transport = Arc::new(CapturingTransport::new(vec![
            login,
            password_ok,
            modify_denied,
        ]));
        let driver = DvripDriver::new(
            Arc::clone(&transport) as Arc<dyn DeviceTransport>,
            DeviceEndpoint::new("device.test", 34567),
            "admin",
            Secret::new("login-pw"),
        );

        let updates = vec![UserUpdate {
            name: "guard1".to_owned(),
            password: Some(Secret::new("new-s3cr3t")),
            group: Some("operators".to_owned()),
            authorities: None,
        }];
        let outcome = driver.update_users(&updates).await.expect("update_users");
        assert!(outcome.succeeded.is_empty());
        assert_eq!(outcome.failed.len(), 1);
        assert_eq!(outcome.failed[0].0, "guard1");
        let reason = outcome.failed[0].1.to_string();
        assert!(
            reason.contains("already committed"),
            "failure message must surface the partial write honestly: {reason}"
        );
        assert!(
            reason.contains("107"),
            "failure message should still carry the ModifyUser Ret: {reason}"
        );

        // Both frames were sent — the password did land on-device even though
        // the overall edit is reported as failed.
        assert_eq!(transport.request_count(), 3);
    }

    #[tokio::test]
    async fn update_users_password_only_skips_modifyuser() {
        let login = json!({"Name": "", "Ret": 100, "SessionID": "0x00000005"});
        let password_ok = json!({"Name": "ModifyPassword", "Ret": 100, "SessionID": "0x5"});
        let transport = Arc::new(CapturingTransport::new(vec![login, password_ok]));
        let driver = DvripDriver::new(
            Arc::clone(&transport) as Arc<dyn DeviceTransport>,
            DeviceEndpoint::new("device.test", 34567),
            "admin",
            Secret::new("login-pw"),
        );

        let updates = vec![UserUpdate {
            name: "guard1".to_owned(),
            password: Some(Secret::new("reset-pw")),
            group: None,
            authorities: None,
        }];
        let outcome = driver.update_users(&updates).await.expect("update_users");
        assert_eq!(outcome.succeeded, ["guard1"]);

        // A password-only edit sends exactly one write frame (ModifyPassword),
        // never a ModifyUser record edit.
        assert_eq!(transport.request_count(), 2);
        let (msg, _payload) = transport.request(1);
        assert_eq!(msg, codec::command::MODIFY_PASSWORD);
    }

    #[tokio::test]
    async fn update_users_reports_a_device_rejection_as_partial_failure() {
        let login = json!({"Name": "", "Ret": 100, "SessionID": "0x00000005"});
        // A group-only edit whose ModifyUser the device rejects (e.g. insufficient
        // authority) is one user's failure carrying the Ret, NOT a top-level Err.
        let denied = json!({"Name": "ModifyUser", "Ret": 107, "SessionID": "0x5"});
        let transport = Arc::new(CapturingTransport::new(vec![login, denied]));
        let driver = DvripDriver::new(
            Arc::clone(&transport) as Arc<dyn DeviceTransport>,
            DeviceEndpoint::new("device.test", 34567),
            "admin",
            Secret::new("login-pw"),
        );

        let updates = vec![UserUpdate {
            name: "guard1".to_owned(),
            password: None,
            group: Some("admin".to_owned()),
            authorities: None,
        }];
        let outcome = driver.update_users(&updates).await.expect("update_users");
        assert!(outcome.succeeded.is_empty());
        assert_eq!(outcome.failed.len(), 1);
        assert_eq!(outcome.failed[0].0, "guard1");
        assert!(
            outcome.failed[0].1.to_string().contains("107"),
            "reason should carry the Ret: {}",
            outcome.failed[0].1
        );
    }

    #[tokio::test]
    async fn create_users_stops_at_stream_death_and_keeps_earlier_success() {
        let login = json!({"Name": "", "Ret": 100, "SessionID": "0x00000005"});
        let add_ok = json!({"Name": "AddUser", "Ret": 100, "SessionID": "0x5"});
        // Only two responses (login + one AddUser): the second request fails at
        // the transport and the third must not be attempted.
        let transport = Arc::new(CapturingTransport::new(vec![login, add_ok]));
        let driver = DvripDriver::new(
            Arc::clone(&transport) as Arc<dyn DeviceTransport>,
            DeviceEndpoint::new("device.test", 34567),
            "admin",
            Secret::new("login-pw"),
        );
        let users = vec![
            NewUser {
                name: "guard1".to_owned(),
                password: Secret::new("pw-one"),
                group: None,
                authorities: vec![],
            },
            NewUser {
                name: "guard2".to_owned(),
                password: Secret::new("pw-two"),
                group: None,
                authorities: vec![],
            },
            NewUser {
                name: "guard3".to_owned(),
                password: Secret::new("pw-three"),
                group: None,
                authorities: vec![],
            },
        ];

        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            driver.create_users(&users),
        )
        .await
        .expect("must not hang on stream death")
        .expect("an acknowledged write requires a partial outcome");

        assert_eq!(outcome.succeeded, ["guard1"]);
        assert_eq!(outcome.failed.len(), 2);
        assert_eq!(outcome.failed[0].0, "guard2");
        assert!(matches!(outcome.failed[0].1, DriverError::Unreachable(_)));
        assert_eq!(outcome.failed[1].0, "guard3");
        assert!(
            matches!(&outcome.failed[1].1, DriverError::Unreachable(reason) if reason == "not attempted after device link failure")
        );
        assert_eq!(
            transport.send_count(),
            3,
            "login plus two attempted writes; the third account was never sent"
        );
        assert_eq!(
            transport.request_count(),
            2,
            "the server only read the login and the acknowledged write"
        );
    }

    #[tokio::test]
    async fn bulk_user_writes_with_first_stream_failure_keep_full_partial_outcome() {
        for operation in ["create", "delete", "update"] {
            let login = json!({"Name": "", "Ret": 100, "SessionID": "0x00000005"});
            let transport = Arc::new(CapturingTransport::new(vec![login]));
            let driver = DvripDriver::new(
                Arc::clone(&transport) as Arc<dyn DeviceTransport>,
                DeviceEndpoint::new("device.test", 34567),
                "admin",
                Secret::new("login-pw"),
            );
            let outcome = match operation {
                "create" => {
                    driver
                        .create_users(&[
                            NewUser {
                                name: "first".to_owned(),
                                password: Secret::new("pw"),
                                group: None,
                                authorities: vec![],
                            },
                            NewUser {
                                name: "second".to_owned(),
                                password: Secret::new("pw"),
                                group: None,
                                authorities: vec![],
                            },
                        ])
                        .await
                }
                "delete" => {
                    driver
                        .delete_users(&["first".to_owned(), "second".to_owned()])
                        .await
                }
                "update" => {
                    driver
                        .update_users(&[
                            UserUpdate {
                                name: "first".to_owned(),
                                password: None,
                                group: Some("user".to_owned()),
                                authorities: None,
                            },
                            UserUpdate {
                                name: "second".to_owned(),
                                password: None,
                                group: Some("user".to_owned()),
                                authorities: None,
                            },
                        ])
                        .await
                }
                _ => Err(DriverError::Unsupported),
            }
            .expect("login proved the link, so a stream death is a partial outcome");
            assert!(outcome.succeeded.is_empty(), "{operation}");
            assert_eq!(outcome.failed.len(), 2, "{operation}");
            assert_eq!(outcome.failed[0].0, "first", "{operation}");
            assert!(
                matches!(outcome.failed[0].1, DriverError::Unreachable(_)),
                "{operation}"
            );
            assert_eq!(outcome.failed[1].0, "second", "{operation}");
            assert!(
                matches!(&outcome.failed[1].1, DriverError::Unreachable(reason) if reason == "not attempted after device link failure"),
                "{operation}"
            );
            assert_eq!(transport.connect_count(), 1, "{operation}");
            assert_eq!(
                transport.send_count(),
                2,
                "{operation}: only login and the first write were attempted"
            );
        }
    }

    #[tokio::test]
    async fn create_users_connect_failure_is_top_level_err() {
        struct DeadTransport;
        #[async_trait]
        impl DeviceTransport for DeadTransport {
            async fn connect(
                &self,
                _endpoint: &DeviceEndpoint,
            ) -> Result<Box<dyn DeviceStream>, DriverError> {
                Err(DriverError::Transport("connect refused".to_owned()))
            }
        }
        let driver = DvripDriver::new(
            Arc::new(DeadTransport),
            DeviceEndpoint::new("device.test", 34567),
            "admin",
            Secret::new("pw"),
        );
        let users = vec![NewUser {
            name: "guard1".to_owned(),
            password: Secret::new("pw"),
            group: None,
            authorities: vec![],
        }];
        // A failure that prevents ALL work is a top-level Err, not a BulkOutcome.
        let error = driver
            .create_users(&users)
            .await
            .expect_err("connect failure must be a top-level Err");
        assert!(matches!(error, DriverError::Transport(_)));
    }

    /// A transport that captures each request frame and replies with a caller-
    /// supplied *raw* response frame (already framed), so a test can script a
    /// binary (non-JSON) reply such as an `OPSNAP` media frame. Captured requests
    /// are decodable so a test can assert on the frame the driver emitted.
    struct RawReplyTransport {
        /// Scripted replies per connection: the first `connect` gets script 0,
        /// the second script 1, and so on. A driver that must reopen a session
        /// (archive segments) therefore sees a genuinely fresh conversation.
        scripts: Vec<Vec<Vec<u8>>>,
        captured: Arc<std::sync::Mutex<Vec<Vec<u8>>>>,
        connects: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl RawReplyTransport {
        fn new(replies: Vec<Vec<u8>>) -> Self {
            Self::new_per_connection(vec![replies])
        }

        /// One reply script per connection, in order.
        fn new_per_connection(scripts: Vec<Vec<Vec<u8>>>) -> Self {
            Self {
                scripts,
                captured: Arc::new(std::sync::Mutex::new(Vec::new())),
                connects: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            }
        }

        /// Decode captured request `index` into its command id and JSON payload.
        fn request(&self, index: usize) -> (u16, Value) {
            let frames = self.captured.lock().expect("capture lock");
            let frame = codec::decode(&frames[index]).expect("decode captured frame");
            let payload: Value =
                serde_json::from_slice(&frame.payload).expect("captured payload is JSON");
            (frame.message_id, payload)
        }

        /// Raw bytes of captured request `index`.
        fn raw(&self, index: usize) -> Vec<u8> {
            self.captured.lock().expect("capture lock")[index].clone()
        }

        fn connect_count(&self) -> usize {
            self.connects.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl DeviceTransport for RawReplyTransport {
        async fn connect(
            &self,
            _endpoint: &DeviceEndpoint,
        ) -> Result<Box<dyn DeviceStream>, DriverError> {
            let index = self
                .connects
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let (client, mut server) = tokio::io::duplex(64 * 1024);
            // Past the end of the script the connection simply stays quiet,
            // which is how a real device signals the end of a window.
            let replies = self.scripts.get(index).cloned().unwrap_or_default();
            let captured = Arc::clone(&self.captured);
            tokio::spawn(async move {
                for reply in replies {
                    let mut header = [0u8; codec::HEADER_LEN];
                    if server.read_exact(&mut header).await.is_err() {
                        return;
                    }
                    let declared =
                        u32::from_le_bytes([header[16], header[17], header[18], header[19]])
                            as usize;
                    let mut body = vec![0u8; declared];
                    if server.read_exact(&mut body).await.is_err() {
                        return;
                    }
                    let mut full = Vec::with_capacity(codec::HEADER_LEN + declared);
                    full.extend_from_slice(&header);
                    full.extend_from_slice(&body);
                    captured.lock().expect("capture lock").push(full);

                    if server.write_all(&reply).await.is_err() {
                        return;
                    }
                }
            });
            Ok(Box::new(client))
        }
    }

    /// Frame arbitrary `body` bytes as a DVRIP response: a 20-byte header carrying
    /// the body length and message id, then the body verbatim. Unlike
    /// [`codec::encode`], the body is not treated as text and no `\n\0` terminator
    /// is appended, so it can carry a binary media frame.
    fn raw_frame(message_id: u16, body: &[u8]) -> Vec<u8> {
        let mut frame = vec![0u8; codec::HEADER_LEN];
        frame[0] = 0xFF; // head flag
        frame[14..16].copy_from_slice(&message_id.to_le_bytes());
        frame[16..20].copy_from_slice(&(body.len() as u32).to_le_bytes());
        frame.extend_from_slice(body);
        frame
    }

    // ── Archive plane (`OPFileQuery` + `OPPlayBack`) ────────────────────────

    /// A login reply frame with a fixed session id.
    fn login_frame() -> Vec<u8> {
        let login = json!({"Name": "", "Ret": 100, "SessionID": "0x00000005"});
        codec::encode(0, 0, 0, &serde_json::to_string(&login).expect("login json"))
    }

    /// A JSON reply frame carrying `value` under `message_id`.
    fn json_frame(message_id: u16, value: &Value) -> Vec<u8> {
        raw_frame(
            message_id,
            serde_json::to_string(value).expect("json").as_bytes(),
        )
    }

    /// An `OPFileQuery` reply listing one recorded segment.
    fn file_query_reply(name: &str, begin: &str, end: &str) -> Vec<u8> {
        json_frame(
            codec::command::FILE_QUERY + 1,
            &json!({
                "Name": "OPFileQuery",
                "Ret": 100,
                "SessionID": "0x00000005",
                "OPFileQuery": [{
                    "FileName": name,
                    "BeginTime": begin,
                    "EndTime": end,
                    "DiskNo": 0,
                    "SerialNo": 0,
                    "FileLength": "0x00024650",
                }],
            }),
        )
    }

    /// A successful `OPPlayBack` claim reply.
    fn claim_reply() -> Vec<u8> {
        json_frame(
            codec::command::PLAYBACK_CLAIM + 1,
            &json!({"Name": "OPPlayBack", "Ret": 100, "SessionID": "0x00000005"}),
        )
    }

    /// One XM video I-frame packet carrying `payload` at `fps`.
    fn media_i_packet_at(fps: u8, timestamp: &str, payload: &[u8]) -> Vec<u8> {
        let timestamp = parse_dvr_time(timestamp).expect("valid DVR timestamp");
        let year = u32::try_from(timestamp.year() - 2000).expect("year fits packed timestamp");
        let packed = (year << 26)
            | (timestamp.month() << 22)
            | (timestamp.day() << 17)
            | (timestamp.hour() << 12)
            | (timestamp.minute() << 6)
            | timestamp.second();
        let mut packet = vec![0x00, 0x00, 0x01, 0xFC, 0x03, fps, 116, 72];
        packet.extend_from_slice(&packed.to_le_bytes());
        packet.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        packet.extend_from_slice(payload);
        packet
    }

    fn media_i_frame_at(fps: u8, timestamp: &str, payload: &[u8]) -> Vec<u8> {
        raw_frame(
            codec::command::PLAYBACK_MEDIA,
            &media_i_packet_at(fps, timestamp, payload),
        )
    }

    fn media_i_frame_pair_at(
        fps: u8,
        first_timestamp: &str,
        first_payload: &[u8],
        last_timestamp: &str,
        last_payload: &[u8],
    ) -> Vec<u8> {
        let mut body = media_i_packet_at(fps, first_timestamp, first_payload);
        body.extend(media_i_packet_at(fps, last_timestamp, last_payload));
        raw_frame(codec::command::PLAYBACK_MEDIA, &body)
    }

    fn media_i_frame(fps: u8, payload: &[u8]) -> Vec<u8> {
        media_i_frame_at(fps, "2026-07-25 04:00:00", payload)
    }

    /// A stream that serves one finite byte script, then remains connected and
    /// silent. It can independently reject writes to make keepalive failure
    /// deterministic without racing a socket close against the producer task.
    struct ArchiveTestStream {
        bytes: std::collections::VecDeque<u8>,
        failed_send: Option<Arc<tokio::sync::Notify>>,
    }

    #[async_trait]
    impl DeviceStream for ArchiveTestStream {
        async fn send(&mut self, _bytes: &[u8]) -> Result<(), DriverError> {
            if let Some(attempted) = &self.failed_send {
                attempted.notify_one();
                Err(DriverError::Unreachable(
                    "scripted keepalive failure".to_owned(),
                ))
            } else {
                Ok(())
            }
        }

        async fn recv_exact(&mut self, buf: &mut [u8]) -> Result<(), DriverError> {
            if self.bytes.len() < buf.len() {
                return std::future::pending().await;
            }
            for byte in buf {
                *byte = self.bytes.pop_front().expect("length checked");
            }
            Ok(())
        }
    }

    fn started_archive_reader(
        frame: Vec<u8>,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        failed_send: Option<Arc<tokio::sync::Notify>>,
    ) -> ArchiveReader {
        let session = DvripSession {
            stream: Box::new(ArchiveTestStream {
                bytes: frame.into(),
                failed_send,
            }),
            session_id: 5,
            sequence: 0,
        };
        let reconnect: SessionFactory = Arc::new(|| {
            Box::pin(async {
                Err(DriverError::Unreachable(
                    "test reader must not reconnect".to_owned(),
                ))
            })
        });
        let mut reader = ArchiveReader::new(session, reconnect, Vec::new(), start, end);
        reader.streaming = true;
        reader.current_coverage = Some(ArchiveWindowCoverage::new(start, end));
        reader
    }

    fn archive_request() -> ExportRequest {
        ExportRequest {
            channel: 1,
            start: "2026-07-25T04:00:00Z".parse().expect("start"),
            end: "2026-07-25T05:00:00Z".parse().expect("end"),
        }
    }

    fn archive_driver(transport: &Arc<RawReplyTransport>) -> DvripDriver {
        DvripDriver::new(
            Arc::clone(transport) as Arc<dyn DeviceTransport>,
            DeviceEndpoint::new("device.test", 34567),
            "admin",
            Secret::new("login-pw"),
        )
    }

    /// The backpressure branch owns no reserved permit when its keepalive
    /// fails. The failure still has to become a stream item before the sender is
    /// dropped; otherwise an HTTP client observes an indistinguishable EOF.
    #[tokio::test]
    async fn archive_keepalive_failure_is_reported_as_stream_error() {
        const NAL: &[u8] = &[0x00, 0x00, 0x00, 0x01, 0x40, 0x01];
        let start = "2026-07-25T04:00:00Z".parse().expect("start");
        let end = "2026-07-25T05:00:00Z".parse().expect("end");
        let keepalive_attempted = Arc::new(tokio::sync::Notify::new());
        let mut reader = started_archive_reader(
            media_i_frame(12, NAL),
            start,
            end,
            Some(Arc::clone(&keepalive_attempted)),
        );
        let keepalive_due = tokio::time::Instant::now() + std::time::Duration::from_secs(1);
        reader.next_keepalive = keepalive_due;

        let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
        let capacity_probe = sender.clone();
        let task = tokio::spawn(pump_archive(reader, sender));

        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while capacity_probe.capacity() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the first media chunk must fill the channel");
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            keepalive_attempted.notified(),
        )
        .await
        .expect("keepalive must be attempted while the channel is full");
        drop(capacity_probe);

        let first = receiver.recv().await.expect("first media chunk");
        assert_eq!(first.expect("media must precede the failure"), NAL);
        let failure = receiver
            .recv()
            .await
            .expect("keepalive failure must be a stream item")
            .expect_err("keepalive must fail");
        assert!(failure.to_string().contains("scripted keepalive failure"));
        task.await.expect("archive pump task");
    }

    /// Silence is only a protocol terminator after timestamped media reaches
    /// the claim's end. A deterministic never-ready read inside the segment
    /// must therefore become an error rather than a clean `None`.
    #[tokio::test]
    async fn archive_silence_before_the_claimed_end_is_a_stream_error() {
        const NAL: &[u8] = &[0x00, 0x00, 0x00, 0x01, 0x40, 0x01];
        let start = "2026-07-25T04:00:00Z".parse().expect("start");
        let end = "2026-07-25T05:00:00Z".parse().expect("end");
        let mut reader = started_archive_reader(media_i_frame(12, NAL), start, end, None);
        reader.frame_idle_timeout = std::time::Duration::from_millis(20);

        assert_eq!(
            reader.next_video_chunk().await.expect("first chunk"),
            Some(NAL.to_vec())
        );
        let error =
            tokio::time::timeout(std::time::Duration::from_secs(1), reader.next_video_chunk())
                .await
                .expect("silence timeout must be bounded")
                .expect_err("incomplete timestamp coverage must fail");

        assert!(
            error.to_string().contains("incomplete coverage"),
            "unexpected error: {error}"
        );
    }

    /// The archive read emits the two-step handshake, strips the XM wrappers,
    /// and reports the device's own frame rate so the remux is not left to guess.
    #[tokio::test]
    async fn export_download_streams_deframed_archive_video() {
        const NAL: &[u8] = &[0x00, 0x00, 0x00, 0x01, 0x40, 0x01, 0x0C, 0x01];
        let transport = Arc::new(RawReplyTransport::new(vec![
            login_frame(),
            file_query_reply(
                "/idea0/2026-07-25/002/04.00.00-05.00.00[R][@e2863][9].h264",
                "2026-07-25 04:00:00",
                "2026-07-25 05:00:00",
            ),
            claim_reply(),
            media_i_frame_pair_at(12, "2026-07-25 04:00:00", NAL, "2026-07-25 04:59:59", NAL),
        ]));
        let driver = archive_driver(&transport);

        let download = driver
            .export_download(&archive_request())
            .await
            .expect("archive read")
            .expect("a segment covers the window");

        assert_eq!(download.hints, RemuxHints { fps: Some(12) });
        assert!(download.filename.starts_with("dvrip-ch1-"));

        let bytes: Vec<u8> = download
            .bytes
            .map(|chunk| chunk.expect("chunk"))
            .collect::<Vec<_>>()
            .await
            .concat();
        assert_eq!(
            bytes,
            [NAL, NAL].concat(),
            "wrappers must be stripped, payload preserved"
        );

        let (search_id, search) = transport.request(1);
        assert_eq!(search_id, codec::command::FILE_QUERY);
        assert_eq!(search["OPFileQuery"]["Channel"], json!(1));
        assert_eq!(search["OPFileQuery"]["BeginTime"], "2026-07-25 04:00:00");

        let (claim_id, claim) = transport.request(2);
        assert_eq!(claim_id, codec::command::PLAYBACK_CLAIM);
        assert_eq!(claim["OPPlayBack"]["Action"], "Claim");
        // `ByTime` is accepted by these devices and then delivers nothing, so
        // playback must always be addressed by the enumerated file name.
        assert_eq!(claim["OPPlayBack"]["Parameter"]["PlayMode"], "ByName");

        let (start_id, start) = transport.request(3);
        assert_eq!(start_id, codec::command::PLAYBACK_START);
        assert_eq!(start["OPPlayBack"]["Action"], "DownloadStart");
    }

    /// An `OPFileQuery` reply listing two consecutive hour-segments.
    fn two_segment_file_query_reply() -> Vec<u8> {
        json_frame(
            codec::command::FILE_QUERY + 1,
            &json!({
                "Name": "OPFileQuery",
                "Ret": 100,
                "SessionID": "0x00000005",
                "OPFileQuery": [
                    {"FileName": "/idea0/2026-07-25/002/04.00.00-05.00.00[R][@a][1].h264",
                     "BeginTime": "2026-07-25 04:00:00", "EndTime": "2026-07-25 05:00:00",
                     "DiskNo": 0, "SerialNo": 0, "FileLength": "0x00024650"},
                    {"FileName": "/idea0/2026-07-25/002/05.00.00-06.00.00[R][@b][2].h264",
                     "BeginTime": "2026-07-25 05:00:00", "EndTime": "2026-07-25 06:00:00",
                     "DiskNo": 0, "SerialNo": 0, "FileLength": "0x00024650"},
                ],
            }),
        )
    }

    /// A window spanning two segments must deliver BOTH, on two sessions.
    ///
    /// The device answers a second claim on an already-used socket with `Ret
    /// 103` and then sends nothing (live-verified), so reusing the connection
    /// truncated the window to its first segment while still reporting success —
    /// the operator got a short clip with no indication anything was missing.
    #[tokio::test]
    async fn export_download_reopens_a_session_for_each_segment() {
        const FIRST: &[u8] = &[0x00, 0x00, 0x00, 0x01, 0x40, 0x01, 0x0C, 0x01];
        const SECOND: &[u8] = &[0x00, 0x00, 0x00, 0x01, 0x26, 0x01, 0xAF, 0x22];

        // Session A: login, search, claim, one frame, then silence (end of the
        // first segment). Session B: login, claim, one frame, silence.
        let transport = Arc::new(RawReplyTransport::new_per_connection(vec![
            vec![
                login_frame(),
                two_segment_file_query_reply(),
                claim_reply(),
                media_i_frame_pair_at(
                    12,
                    "2026-07-25 04:30:00",
                    FIRST,
                    "2026-07-25 04:59:59",
                    FIRST,
                ),
            ],
            vec![
                login_frame(),
                claim_reply(),
                media_i_frame_pair_at(
                    12,
                    "2026-07-25 05:00:00",
                    SECOND,
                    "2026-07-25 05:29:59",
                    SECOND,
                ),
            ],
        ]));
        let driver = archive_driver(&transport);

        let request = ExportRequest {
            channel: 1,
            start: "2026-07-25T04:30:00Z".parse().expect("start"),
            end: "2026-07-25T05:30:00Z".parse().expect("end"),
        };
        let download = driver
            .export_download(&request)
            .await
            .expect("archive read")
            .expect("segments cover the window");

        let bytes: Vec<u8> = download
            .bytes
            .map(|chunk| chunk.expect("chunk"))
            .collect::<Vec<_>>()
            .await
            .concat();

        assert_eq!(
            bytes,
            [FIRST, FIRST, SECOND, SECOND].concat(),
            "both segments must reach the client, not just the first"
        );
        assert_eq!(
            transport.connect_count(),
            2,
            "the second segment needs its own session"
        );
    }

    /// The device ends a window by going quiet while holding the connection
    /// open — no FIN, no status frame. Once timestamped I-frames prove that the
    /// claimed end was reached, that silence is a clean terminating condition.
    #[tokio::test]
    async fn archive_stream_ends_cleanly_when_the_device_falls_silent() {
        const NAL: &[u8] = &[0x00, 0x00, 0x00, 0x01, 0x40, 0x01, 0x0C, 0x01];
        // The scripted transport answers login, search and claim, then stops —
        // exactly the silence a real device leaves at the end of a window.
        let transport = Arc::new(RawReplyTransport::new(vec![
            login_frame(),
            file_query_reply(
                "/idea0/2026-07-25/002/04.00.00-05.00.00[R][@e2863][9].h264",
                "2026-07-25 04:00:00",
                "2026-07-25 05:00:00",
            ),
            claim_reply(),
            media_i_frame_pair_at(12, "2026-07-25 04:00:00", NAL, "2026-07-25 04:59:59", NAL),
        ]));
        let driver = archive_driver(&transport);

        let download = driver
            .export_download(&archive_request())
            .await
            .expect("archive read")
            .expect("a segment covers the window");

        let chunks: Vec<_> = download.bytes.collect().await;

        assert!(
            chunks.iter().all(Result::is_ok),
            "silence must end the stream cleanly, not as an error: {chunks:?}"
        );
        let bytes: Vec<u8> = chunks
            .into_iter()
            .map(|c| c.expect("ok"))
            .collect::<Vec<_>>()
            .concat();
        assert_eq!(bytes, [NAL, NAL].concat());
    }

    /// A window the device has no footage for is a normal answer, and the
    /// service falls through to another path rather than surfacing an error.
    #[tokio::test]
    async fn export_download_reports_no_footage_as_none() {
        let empty = json_frame(
            codec::command::FILE_QUERY + 1,
            &json!({"Name": "OPFileQuery", "Ret": 100, "SessionID": "0x00000005",
                    "OPFileQuery": []}),
        );
        let transport = Arc::new(RawReplyTransport::new(vec![login_frame(), empty]));
        let driver = archive_driver(&transport);

        let download = driver
            .export_download(&archive_request())
            .await
            .expect("archive read");

        assert!(download.is_none());
    }

    /// The device honours a sub-window inside a segment, so a request narrower
    /// than the segment must be clamped to what the caller asked for — not
    /// widened to the whole hour the segment covers.
    #[tokio::test]
    async fn export_download_clamps_the_claim_to_the_requested_window() {
        const NAL: &[u8] = &[0x00, 0x00, 0x00, 0x01, 0x40, 0x01];
        let transport = Arc::new(RawReplyTransport::new(vec![
            login_frame(),
            file_query_reply(
                "/idea0/2026-07-25/002/04.00.00-05.00.00[R][@e2863][9].h264",
                "2026-07-25 04:00:00",
                "2026-07-25 05:00:00",
            ),
            claim_reply(),
            media_i_frame(12, NAL),
        ]));
        let driver = archive_driver(&transport);

        let request = ExportRequest {
            channel: 1,
            start: "2026-07-25T04:20:00Z".parse().expect("start"),
            end: "2026-07-25T04:21:00Z".parse().expect("end"),
        };
        driver
            .export_download(&request)
            .await
            .expect("archive read")
            .expect("segment covers the window");

        let (_, claim) = transport.request(2);
        assert_eq!(claim["OPPlayBack"]["StartTime"], "2026-07-25 04:20:00");
        assert_eq!(claim["OPPlayBack"]["EndTime"], "2026-07-25 04:21:00");
    }

    /// The plaintext password must never reach the wire on the archive path
    /// either — login carries only the Sofia hash.
    #[tokio::test]
    async fn export_download_never_puts_the_password_on_the_wire() {
        let transport = Arc::new(RawReplyTransport::new(vec![
            login_frame(),
            file_query_reply(
                "/idea0/2026-07-25/002/04.00.00-05.00.00[R][@e2863][9].h264",
                "2026-07-25 04:00:00",
                "2026-07-25 05:00:00",
            ),
            claim_reply(),
            media_i_frame(12, &[0x00, 0x00, 0x00, 0x01, 0x40]),
        ]));
        let driver = archive_driver(&transport);

        driver
            .export_download(&archive_request())
            .await
            .expect("archive read");

        for index in 0..3 {
            assert!(
                !frame_contains(&transport.raw(index), b"login-pw"),
                "frame {index} leaked the plaintext password"
            );
        }
    }

    /// The archive capability must be advertised, since the driver now
    /// implements it — the matrix and the runtime have to agree.
    #[test]
    fn capabilities_advertise_archive_export() {
        let transport = Arc::new(RawReplyTransport::new(vec![]));
        let driver = archive_driver(&transport);

        assert!(driver.capabilities().export);
        assert!(driver.capabilities().recording_search);
    }

    /// `search_recordings` reuses `OPFileQuery` verbatim (no new device call)
    /// and attributes every span back to the requested channel — the field
    /// the device-wide `recording_status` read cannot fill in.
    #[tokio::test]
    async fn search_recordings_attributes_spans_to_the_requested_channel() {
        let transport = Arc::new(RawReplyTransport::new(vec![
            login_frame(),
            file_query_reply(
                "/idea0/2026-07-25/002/04.00.00-05.00.00[R][@e2863][9].h264",
                "2026-07-25 04:00:00",
                "2026-07-25 05:00:00",
            ),
        ]));
        let driver = archive_driver(&transport);

        let spans = driver
            .search_recordings(&archive_request())
            .await
            .expect("search");

        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].channel, Some(1));
        assert_eq!(
            spans[0].start,
            "2026-07-25T04:00:00Z".parse::<DateTime<Utc>>().unwrap()
        );
    }

    /// A window with no footage answers with an empty list, not an error —
    /// the same "no footage" outcome `export_download` treats as `None`.
    #[tokio::test]
    async fn search_recordings_returns_empty_for_a_window_with_no_footage() {
        let empty = json_frame(
            codec::command::FILE_QUERY + 1,
            &json!({"Name": "OPFileQuery", "Ret": 100, "SessionID": "0x00000005",
                    "OPFileQuery": []}),
        );
        let transport = Arc::new(RawReplyTransport::new(vec![login_frame(), empty]));
        let driver = archive_driver(&transport);

        let spans = driver
            .search_recordings(&archive_request())
            .await
            .expect("search");

        assert!(spans.is_empty());
    }

    /// A rejected or malformed archive query is not evidence that the archive
    /// is empty. Both archive consumers must preserve the failure so callers
    /// can distinguish "could not query footage" from "no footage".
    #[tokio::test]
    async fn archive_operations_surface_file_query_failures() {
        enum ExpectedError {
            Device(&'static str),
            Protocol(&'static str),
        }

        for (reply, expected_error) in [
            (
                json!({
                    "Name": "OPFileQuery",
                    "Ret": 107,
                    "SessionID": "0x00000005",
                }),
                ExpectedError::Device("OPFileQuery returned Ret 107"),
            ),
            (
                json!({
                    "Name": "OPFileQuery",
                    "SessionID": "0x00000005",
                }),
                ExpectedError::Protocol("OPFileQuery response missing integer Ret"),
            ),
        ] {
            let assert_expected_error = |error| match (error, &expected_error) {
                (DriverError::Device(actual), ExpectedError::Device(expected))
                | (DriverError::Protocol(actual), ExpectedError::Protocol(expected)) => {
                    assert_eq!(actual, *expected);
                }
                (other, _) => panic!("unexpected archive query error: {other:?}"),
            };

            let rejected = json_frame(codec::command::FILE_QUERY + 1, &reply);
            let transport = Arc::new(RawReplyTransport::new(vec![login_frame(), rejected]));
            let driver = archive_driver(&transport);

            let export_error = match driver.export_download(&archive_request()).await {
                Err(error) => error,
                Ok(_) => panic!("export must surface the file-query failure"),
            };
            assert_expected_error(export_error);

            let rejected = json_frame(codec::command::FILE_QUERY + 1, &reply);
            let transport = Arc::new(RawReplyTransport::new(vec![login_frame(), rejected]));
            let driver = archive_driver(&transport);

            let search_error = match driver.search_recordings(&archive_request()).await {
                Err(error) => error,
                Ok(_) => panic!("search must surface the file-query failure"),
            };
            assert_expected_error(search_error);
        }
    }

    #[tokio::test]
    async fn snapshot_emits_opsnap_and_returns_the_jpeg_bytes() {
        let login = json!({"Name": "", "Ret": 100, "SessionID": "0x00000005"});
        let login_frame =
            codec::encode(0, 0, 0, &serde_json::to_string(&login).expect("login json"));

        // A well-formed JPEG (SOI … EOI); the driver must return exactly these.
        let jpeg: Vec<u8> = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46, 0xFF, 0xD9];
        // The OPSNAP reply is a binary media frame: a device preamble, the JPEG,
        // then trailing padding — extraction must isolate exactly the JPEG.
        let mut media = vec![0x00, 0x01, 0x02];
        media.extend_from_slice(&jpeg);
        media.push(0x00);
        let snap_frame = raw_frame(codec::command::SNAPSHOT, &media);

        let transport = Arc::new(RawReplyTransport::new(vec![login_frame, snap_frame]));
        let driver = DvripDriver::new(
            Arc::clone(&transport) as Arc<dyn DeviceTransport>,
            DeviceEndpoint::new("device.test", 34567),
            "admin",
            Secret::new("login-pw"),
        );

        let snap = driver.snapshot(Some(1)).await.expect("snapshot");
        let SnapshotRef::Bytes(bytes) = &snap else {
            panic!("expected inline bytes, got {snap:?}");
        };
        assert_eq!(bytes, &jpeg);

        // One session served the operation: a single connect, and frame 1 (after
        // the login) is the OPSNAP request the driver emitted.
        assert_eq!(transport.connect_count(), 1);
        let (msg, payload) = transport.request(1);
        assert_eq!(msg, codec::command::SNAPSHOT);
        assert_eq!(payload["Name"], "OPSNAP");
        assert_eq!(payload["SessionID"], "0x00000005");
        // The 1-based public channel 1 maps to the 0-based wire channel 0.
        assert_eq!(payload["OPSNAP"]["Channel"], json!(0));

        // No plaintext login secret leaks into any emitted frame (login carries
        // only the Sofia hash).
        assert!(!frame_contains(&transport.raw(0), b"login-pw"));
        assert!(!frame_contains(&transport.raw(1), b"login-pw"));

        assert!(driver.capabilities().snapshot);
    }

    #[tokio::test]
    async fn snapshot_errors_when_the_reply_has_no_jpeg() {
        let login = json!({"Name": "", "Ret": 100, "SessionID": "0x00000005"});
        let login_frame =
            codec::encode(0, 0, 0, &serde_json::to_string(&login).expect("login json"));
        // A reply with no SOI marker at all (e.g. a device error frame).
        let snap_frame = raw_frame(codec::command::SNAPSHOT, &[0x00, 0x11, 0x22, 0x33]);
        let transport = Arc::new(RawReplyTransport::new(vec![login_frame, snap_frame]));
        let driver = DvripDriver::new(
            Arc::clone(&transport) as Arc<dyn DeviceTransport>,
            DeviceEndpoint::new("device.test", 34567),
            "admin",
            Secret::new("login-pw"),
        );

        let error = driver
            .snapshot(None)
            .await
            .expect_err("a reply without a JPEG must error");
        assert!(matches!(error, DriverError::Device(_)));
    }

    #[tokio::test]
    async fn snapshot_reports_unsupported_when_the_device_rejects_opsnap() {
        // Firmware that lacks still capture answers OPSNAP with a JSON status
        // frame (non-OK `Ret`, no image) instead of a JPEG media frame. Verified
        // live on an AHB8016T-MH (Ret 102). That must surface as `Unsupported`
        // (a clean "no still available") — never a JPEG-decode failure.
        let login = json!({"Name": "", "Ret": 100, "SessionID": "0x00000005"});
        let login_frame =
            codec::encode(0, 0, 0, &serde_json::to_string(&login).expect("login json"));
        let reject = json!({"Name": "OPSNAP", "Ret": 102, "SessionID": "0x00000005"});
        let snap_frame = codec::encode(
            codec::command::SNAPSHOT,
            0,
            0,
            &serde_json::to_string(&reject).expect("reject json"),
        );
        let transport = Arc::new(RawReplyTransport::new(vec![login_frame, snap_frame]));
        let driver = DvripDriver::new(
            Arc::clone(&transport) as Arc<dyn DeviceTransport>,
            DeviceEndpoint::new("device.test", 34567),
            "admin",
            Secret::new("login-pw"),
        );

        let error = driver
            .snapshot(None)
            .await
            .expect_err("OPSNAP rejection must error");
        assert!(matches!(error, DriverError::Unsupported));
    }

    #[test]
    fn ptz_move_maps_the_dominant_axis_to_a_command_and_step() {
        // Pan right dominates → DirectionRight; magnitude maps onto 1..=8.
        assert_eq!(
            dvrip_move_command(PtzVector {
                pan: 1.0,
                tilt: 0.1,
                zoom: 0.0
            }),
            Some(("DirectionRight", 8))
        );
        assert_eq!(
            dvrip_move_command(PtzVector {
                pan: -1.0,
                tilt: 0.0,
                zoom: 0.0
            }),
            Some(("DirectionLeft", 8))
        );
        // Tilt up dominates; a small magnitude still yields at least step 1.
        assert_eq!(
            dvrip_move_command(PtzVector {
                pan: 0.0,
                tilt: 0.1,
                zoom: 0.0
            }),
            Some(("DirectionUp", 1))
        );
        // Zoom wide (negative) dominates.
        assert_eq!(
            dvrip_move_command(PtzVector {
                pan: 0.0,
                tilt: 0.0,
                zoom: -0.5
            }),
            Some(("ZoomWide", 4))
        );
        // An all-zero vector is a no-op (the client sends Stop to halt).
        assert_eq!(
            dvrip_move_command(PtzVector {
                pan: 0.0,
                tilt: 0.0,
                zoom: 0.0
            }),
            None
        );
    }

    #[tokio::test]
    async fn ptz_move_emits_an_opptzcontrol_frame_for_the_wire_channel() {
        let login = json!({"Name": "", "Ret": 100, "SessionID": "0x00000005"});
        let login_frame =
            codec::encode(0, 0, 0, &serde_json::to_string(&login).expect("login json"));
        let ack = json!({"Name": "OPPTZControl", "Ret": 100, "SessionID": "0x00000005"});
        let ack_frame = codec::encode(
            codec::command::PTZ,
            0,
            0,
            &serde_json::to_string(&ack).expect("ack json"),
        );
        let transport = Arc::new(RawReplyTransport::new(vec![login_frame, ack_frame]));
        let driver = DvripDriver::new(
            Arc::clone(&transport) as Arc<dyn DeviceTransport>,
            DeviceEndpoint::new("device.test", 34567),
            "admin",
            Secret::new("login-pw"),
        );

        driver
            .ptz(
                2,
                PtzCommand::Move(PtzVector {
                    pan: 1.0,
                    tilt: 0.0,
                    zoom: 0.0,
                }),
            )
            .await
            .expect("ptz move");

        let (msg, payload) = transport.request(1);
        assert_eq!(msg, codec::command::PTZ);
        assert_eq!(payload["Name"], "OPPTZControl");
        assert_eq!(payload["OPPTZControl"]["Command"], "DirectionRight");
        // Public channel 2 → 0-based wire channel 1.
        assert_eq!(payload["OPPTZControl"]["Parameter"]["Channel"], json!(1));
        assert_eq!(payload["OPPTZControl"]["Parameter"]["Pattern"], "Start");
        assert!(driver.capabilities().ptz);
    }

    #[tokio::test]
    async fn ptz_preset_recall_emits_gotopreset() {
        let login = json!({"Name": "", "Ret": 100, "SessionID": "0x00000005"});
        let login_frame =
            codec::encode(0, 0, 0, &serde_json::to_string(&login).expect("login json"));
        let ack = json!({"Name": "OPPTZControl", "Ret": 100, "SessionID": "0x00000005"});
        let ack_frame = codec::encode(
            codec::command::PTZ,
            0,
            0,
            &serde_json::to_string(&ack).expect("ack"),
        );
        let transport = Arc::new(RawReplyTransport::new(vec![login_frame, ack_frame]));
        let driver = DvripDriver::new(
            Arc::clone(&transport) as Arc<dyn DeviceTransport>,
            DeviceEndpoint::new("device.test", 34567),
            "admin",
            Secret::new("login-pw"),
        );

        driver
            .ptz(1, PtzCommand::Preset { id: 7 })
            .await
            .expect("preset");
        let (_msg, payload) = transport.request(1);
        assert_eq!(payload["OPPTZControl"]["Command"], "GotoPreset");
        assert_eq!(payload["OPPTZControl"]["Parameter"]["Preset"], json!(7));
    }

    /// Minimal blocking bridge for a non-async test asserting on an async method
    /// that never actually awaits I/O.
    fn tokio_block<F: Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime")
            .block_on(future)
    }
}
