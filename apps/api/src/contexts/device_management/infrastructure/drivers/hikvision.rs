//! Hikvision driver (ISAPI over HTTP on the device's HTTP port, Digest auth).
//!
//! Hikvision's management plane is the ISAPI REST tree (`/ISAPI/...`) answering
//! with flat, known-schema XML. Each operation is one authenticated request via
//! an [`HttpDeviceClient`]; there is no session to hold, so the same Digest
//! client that serves Dahua serves Hikvision. Read operations (`get_info`,
//! `list_users`, `disk_status`), `stream_uri`, and snapshot capture
//! (`Streaming/channels/<id>/picture`) are implemented and match responses
//! verified against a live device (2026-07-17). Recording enumeration is gated to
//! a later phase and returns [`DriverError::Unsupported`].
//!
//! User provisioning and editing (`create_users`/`delete_users`/`update_users`)
//! is implemented against the documented ISAPI `Security/users` write API; an
//! edit is a `PUT /ISAPI/Security/users/{id}` by the numeric id resolved from the
//! current listing (the same resolution the delete path uses). **The
//! salted-SHA256 password wire format for irreversible-encryption firmware
//! (`isIrreversible=true`, firmware >= V4.75) is per-spec and still needs
//! validation against a live device before writes are enabled in production** —
//! see `irreversible_password`. Likewise, whether an edit's partial `<User>`
//! body preserves the fields it omits (versus resetting them) is per-spec and not
//! yet live-verified — see `build_modify_user_body`. The `authorities`
//! facet is **not implemented**: the ISAPI `AuthorityList` write body has not
//! been designed or verified, so a create/update carrying an authorities
//! change is rejected per-item rather than silently dropped — see
//! `authorities_not_implemented`.
//!
//! The XML is parsed with a small, dep-free tag extractor (`xml_tag` /
//! `xml_blocks`) rather than a general XML crate: the ISAPI documents here are
//! a flat, fixed schema, so a structural extractor is sufficient and adds no
//! dependency.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use sha2::{Digest, Sha256};

use crate::contexts::device_management::domain::{
    BulkOutcome, Capabilities, Channel, DeviceDriver, DeviceInfo, DiskState, DiskStatus,
    DownloadStream, DriverError, ExportRequest, LinkFailurePolicy, NewUser, PtzCommand, PtzSupport,
    PtzVector, RecordSpan, RecordingStatus, RemuxHints, Secret, SnapshotRef, StreamTransport,
    StreamUri, UserAccount, UserUpdate,
};
use crate::contexts::device_management::infrastructure::http_client::HttpDeviceClient;
use crate::contexts::device_management::infrastructure::transport::DeviceEndpoint;

/// Device identity/firmware ISAPI resource.
const PATH_DEVICE_INFO: &str = "/ISAPI/System/deviceInfo";
/// User-account listing/provisioning ISAPI resource.
const PATH_USERS: &str = "/ISAPI/Security/users";
/// Per-user security capabilities ISAPI resource (salt, iteration count, and
/// whether the firmware expects an irreversible salted password).
const PATH_SECURITY_CAP: &str = "/ISAPI/Security/capabilities";
/// Storage/disk health ISAPI resource.
const PATH_STORAGE: &str = "/ISAPI/ContentMgmt/Storage/hdd";
/// NVR IP-camera channel list (carries per-channel display names).
const PATH_INPUT_PROXY_CHANNELS: &str = "/ISAPI/ContentMgmt/InputProxy/channels";
/// DVR analog video-input channel list (fallback when there is no InputProxy).
const PATH_VIDEO_INPUT_CHANNELS: &str = "/ISAPI/System/Video/inputs/channels";
/// Recorded-media search ISAPI resource (POST CMSearchDescription).
const PATH_CONTENT_SEARCH: &str = "/ISAPI/ContentMgmt/search";
/// Recorded-media file download ISAPI resource (POST downloadRequest naming a
/// `playbackURI` from a prior search). Live-verified on a DS-N316/2(D):
/// answers `200 Opaque/data` streaming the raw device container.
const PATH_CONTENT_DOWNLOAD: &str = "/ISAPI/ContentMgmt/download";
/// Per-channel PTZ capabilities ISAPI resource (`{channel}` is the same
/// `<id>` `list_channels` reports). A channel with a physical/virtual PTZ head
/// answers a non-empty `controlProtocol`; a plain fixed camera answers the
/// same 200 with an empty one. Live-verified on a TC-R3120 hybrid NVR
/// (2026-07-25): analog channel 1 (`DOME_PLUS`) vs. IP proxy channel 101 (empty).
const PATH_PTZ_CAPABILITIES: &str = "/ISAPI/PTZCtrl/channels";
/// How far back the recording-window search reaches.
const RECORDING_LOOKBACK_DAYS: i64 = 90;
/// Page size for the content search loop.
const SEARCH_PAGE: u32 = 100;
/// Hard ceiling on content-search pages: bounds a device that never stops
/// signalling `MORE`, so `recording_status` cannot grow or loop indefinitely.
/// Each individual request already carries its own timeout in the HTTP
/// client; this bounds the total page count on top of that.
const MAX_SEARCH_PAGES: u32 = 1_000;
/// Main-stream track id for camera 1 (Hik encodes `camera*100 + 1`).
const DEFAULT_TRACK_ID: u32 = 101;
/// Content type ISAPI write bodies are sent with.
const CONTENT_TYPE_XML: &str = "application/xml";
/// Fallback iteration count when the device omits one from its security caps.
const DEFAULT_ITERATIONS: u32 = 1;
/// Ceiling on the device-reported SHA-256 iteration count (`iterateNumber`/
/// `keyIterateNum`). Left unbounded, a hostile or buggy device could report an
/// enormous count and make `irreversible_password`'s synchronous hashing loop
/// run long enough to starve a tokio worker thread.
const MAX_ITERATIONS: u32 = 100_000;
/// RTSP egress port (the near-universal Hikvision default).
const RTSP_PORT: u16 = 554;
/// One mebibyte in bytes; ISAPI reports disk capacity in MiB.
const MIB: u64 = 1024 * 1024;

/// A Hikvision driver bound to one device.
pub struct HikvisionDriver {
    http: Arc<dyn HttpDeviceClient>,
    endpoint: DeviceEndpoint,
    username: String,
    password: Secret,
}

impl HikvisionDriver {
    /// Construct a driver reaching the device via `http`.
    pub fn new(
        http: Arc<dyn HttpDeviceClient>,
        endpoint: DeviceEndpoint,
        username: impl Into<String>,
        password: Secret,
    ) -> Self {
        Self {
            http,
            endpoint,
            username: username.into(),
            password,
        }
    }

    /// Provision a single user: read the device's per-user security capabilities,
    /// encode the password as the firmware expects, and POST the `<User>` block.
    ///
    /// Returns `Ok(())` when the device accepts the user or already has one by
    /// that name (idempotent create). The password never appears in an error or
    /// a log — only the derived hash (or, on legacy plaintext firmware, the value
    /// the device itself requires) is ever placed on the wire.
    ///
    /// A request for a non-empty `authorities` list is rejected up front: the
    /// ISAPI `AuthorityList` write body is not implemented (see the module
    /// doc), and silently dropping the facet while reporting success would
    /// under-grant the account. The group literal is validated before any
    /// network call, so an invalid request makes no device write.
    async fn create_one_user(&self, user: &NewUser) -> Result<(), DriverError> {
        if !user.authorities.is_empty() {
            return Err(authorities_not_implemented());
        }
        let level = hik_user_level(user.group.as_deref())?;
        let password_field = self
            .encode_password_field(&user.name, &user.password)
            .await?;
        let body = build_create_user_body(&user.name, &password_field, level);
        let response = self
            .http
            .post_text(PATH_USERS, &body, CONTENT_TYPE_XML)
            .await?;
        interpret_create(&response)
    }

    /// Encode `secret` as the firmware expects for `username`: the salted,
    /// irreversible SHA-256 hash on firmware that demands it
    /// (`isIrreversible=true`), or the plaintext legacy firmware stores. Reads
    /// `/ISAPI/Security/capabilities` for `username` first, so the create and
    /// update write paths derive the password identically.
    async fn encode_password_field(
        &self,
        username: &str,
        secret: &Secret,
    ) -> Result<String, DriverError> {
        let caps = self.read_security_cap(username).await?;
        Ok(if caps.is_irreversible {
            irreversible_password(username, &caps.salt, secret.expose(), caps.iterations)
        } else {
            // Legacy firmware (`isIrreversible=false`) takes the plaintext value.
            secret.expose().to_owned()
        })
    }

    /// Edit a single existing user via `PUT /ISAPI/Security/users/{id}`, reusing
    /// the same name→id resolution the delete path uses (`ids`, built once from
    /// the current listing) and the same [`encode_password_field`] derivation the
    /// create path uses.
    ///
    /// Unlike a delete — where an absent user is an idempotent success — an update
    /// targets an existing record, so a name the device does not have is this
    /// item's failure. `<password>`/`<userLevel>` are emitted only for the facets
    /// the update changes; whether a partial `<User>` preserves the omitted fields
    /// is per the ISAPI spec and **not yet live-verified** (the same caveat that
    /// gates the salted-SHA256 password wire format — see [`irreversible_password`]).
    ///
    /// An `authorities: Some(_)` request (grant, replace, or clear) is rejected:
    /// the ISAPI `AuthorityList` write body is not implemented (see the module
    /// doc), and silently dropping the facet while reporting success would
    /// leave the account's authorities unchanged from what the caller asked
    /// for. The device-reported `id` is validated as numeric before it is
    /// interpolated into the PUT path.
    async fn update_one_user(
        &self,
        update: &UserUpdate,
        ids: &HashMap<String, String>,
    ) -> Result<(), DriverError> {
        let Some(id) = ids.get(&update.name) else {
            return Err(DriverError::Device(format!(
                "user '{name}' is not present on the device",
                name = update.name,
            )));
        };
        if !is_valid_isapi_id(id) {
            return Err(DriverError::Protocol(format!(
                "device reported a non-numeric user id for '{name}'",
                name = update.name,
            )));
        }
        if update.authorities.is_some() {
            return Err(authorities_not_implemented());
        }
        let password_field = match &update.password {
            Some(secret) => Some(self.encode_password_field(&update.name, secret).await?),
            None => None,
        };
        let level = update
            .group
            .as_deref()
            .map(|group| hik_user_level(Some(group)))
            .transpose()?;
        let body = build_modify_user_body(id, &update.name, password_field.as_deref(), level);
        let path = format!("{PATH_USERS}/{id}");
        let response = self.http.put_text(&path, &body, CONTENT_TYPE_XML).await?;
        interpret_modify(&response)
    }

    /// Read `/ISAPI/Security/capabilities?username=<user>` into the salt and
    /// iteration parameters needed to build an irreversible password.
    async fn read_security_cap(&self, username: &str) -> Result<SecurityCap, DriverError> {
        // Percent-encode the name into the query so an unusual login cannot break
        // out of the `username=` parameter. `NON_ALPHANUMERIC` is conservative and
        // also encodes the unreserved `_ . - ~`; a conformant device decodes those
        // back, but on hardware that does not percent-decode query values the salt
        // lookup could return the wrong salt silently — verify against a live
        // device before enabling writes (see the module-level caveat).
        let encoded = utf8_percent_encode(username, NON_ALPHANUMERIC);
        let path = format!("{PATH_SECURITY_CAP}?username={encoded}");
        let body = self.http.get_text(&path).await?;
        Ok(parse_security_cap(&body))
    }

    /// Resolve each channel's real `ptz_support` via one `PTZCtrl/channels/{id}/
    /// capabilities` read per channel, fanned out concurrently (a hybrid NVR can
    /// list dozens of channels, and these reads are independent). A request that
    /// errors leaves that channel's incoming `Unknown` untouched — this
    /// enrichment is best-effort and must never fail the whole listing.
    async fn attach_ptz_support(&self, channels: Vec<Channel>) -> Vec<Channel> {
        let discoveries = channels
            .iter()
            .map(|channel| self.discover_ptz_support(channel.index));
        let results = futures_util::future::join_all(discoveries).await;
        channels
            .into_iter()
            .zip(results)
            .map(|(channel, discovered)| Channel {
                ptz_support: discovered.unwrap_or(channel.ptz_support),
                ..channel
            })
            .collect()
    }

    /// Read `/ISAPI/PTZCtrl/channels/{channel}/capabilities` and classify the
    /// channel from its `controlProtocol`: non-empty means a PTZ head answers
    /// on that channel, empty (the device still answers 200) means it does not.
    /// `None` on any request/parse failure — the caller keeps whatever
    /// `ptz_support` the channel already had rather than guessing.
    async fn discover_ptz_support(&self, channel: u16) -> Option<PtzSupport> {
        let path = format!("{PATH_PTZ_CAPABILITIES}/{channel}/capabilities");
        let body = self.http.get_text(&path).await.ok()?;
        Some(ptz_support_from_capabilities(&body))
    }
}

/// Classify a `<PTZChannel>` capabilities document by its `controlProtocol`
/// leaf: non-blank means a PTZ head is wired to this channel. `channelID` is
/// always present on a genuine `PTZChannel` document (both a PTZ-capable and a
/// plain fixed channel echo it), so its absence means this was not the
/// expected document at all — `xml_tag` cannot tell "tag present but empty"
/// from "tag absent", and only `channelID` disambiguates a real "no PTZ"
/// answer from a malformed/unexpected one.
fn ptz_support_from_capabilities(body: &str) -> PtzSupport {
    if xml_tag(body, "channelID").is_none() {
        return PtzSupport::Unknown;
    }
    match xml_tag(body, "controlProtocol") {
        Some(_) => PtzSupport::Supported,
        None => PtzSupport::Unsupported,
    }
}

#[async_trait]
impl DeviceDriver for HikvisionDriver {
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
            authority_catalog: false,
            recording_search: true,
        }
    }

    async fn get_info(&self) -> Result<DeviceInfo, DriverError> {
        let body = self.http.get_text(PATH_DEVICE_INFO).await?;
        Ok(parse_device_info(&body))
    }

    async fn list_users(&self) -> Result<Vec<UserAccount>, DriverError> {
        let body = self.http.get_text(PATH_USERS).await?;
        Ok(parse_users(&body))
    }

    async fn create_users(&self, users: &[NewUser]) -> Result<BulkOutcome, DriverError> {
        // One ISAPI client is already held (`self.http`); provisioning is a
        // per-user loop over it. A device-level rejection for one user (e.g. a
        // weak-password policy) is that user's failure and collected into the
        // outcome; an `Auth`/`Transport`/`ClockSkew` error means the credentials
        // or link are broken and no further calls are safe. Keep any earlier
        // acknowledged writes in the returned outcome.
        let mut outcome = BulkOutcome::default();
        for (index, user) in users.iter().enumerate() {
            match self.create_one_user(user).await {
                Ok(()) => outcome.succeeded.push(user.name.clone()),
                Err(error) if is_link_fatal(&error) => {
                    return outcome.abort_after_link_failure(
                        user.name.clone(),
                        error,
                        users[index + 1..].iter().map(|user| user.name.clone()),
                        LinkFailurePolicy::ErrorWithoutAcknowledged,
                    );
                }
                Err(error) => outcome.failed.push((user.name.clone(), error)),
            }
        }
        Ok(outcome)
    }

    async fn delete_users(&self, usernames: &[String]) -> Result<BulkOutcome, DriverError> {
        if usernames.is_empty() {
            return Ok(BulkOutcome::default());
        }
        // ISAPI deletes by numeric id, so resolve names against the current user
        // list once. If that single listing call fails (auth/transport), no
        // deletion can proceed and the whole batch is a top-level `Err`.
        let listing = self.http.get_text(PATH_USERS).await?;
        let ids = user_name_to_id(&listing);

        let mut outcome = BulkOutcome::default();
        for (index, name) in usernames.iter().enumerate() {
            let Some(id) = ids.get(name) else {
                // Not present on the device: an idempotent delete is already
                // satisfied, so treat the absent user as a success.
                outcome.succeeded.push(name.clone());
                continue;
            };
            // The id is read back from the device's own listing; validate it is
            // numeric before it is interpolated into the DELETE path, so a
            // malformed or hostile value cannot inject a path segment.
            if !is_valid_isapi_id(id) {
                outcome.failed.push((
                    name.clone(),
                    DriverError::Protocol(format!(
                        "device reported a non-numeric user id for '{name}'"
                    )),
                ));
                continue;
            }
            let path = format!("{PATH_USERS}/{id}");
            match self.http.delete(&path).await.and_then(interpret_delete) {
                Ok(()) => outcome.succeeded.push(name.clone()),
                Err(error) if is_link_fatal(&error) => {
                    return outcome.abort_after_link_failure(
                        name.clone(),
                        error,
                        usernames[index + 1..].iter().cloned(),
                        LinkFailurePolicy::ErrorWithoutAcknowledged,
                    );
                }
                Err(error) => outcome.failed.push((name.clone(), error)),
            }
        }
        Ok(outcome)
    }

    async fn update_users(&self, updates: &[UserUpdate]) -> Result<BulkOutcome, DriverError> {
        if updates.is_empty() {
            return Ok(BulkOutcome::default());
        }
        // ISAPI edits by numeric id, so resolve names against the current user
        // list once (a fatal read aborts the whole batch, exactly like
        // `delete_users`). A device-level rejection for one edit is that item's
        // failure; an `Auth`/`Transport`/`ClockSkew` error stops further calls
        // without dropping earlier acknowledgements.
        let listing = self.http.get_text(PATH_USERS).await?;
        let ids = user_name_to_id(&listing);

        let mut outcome = BulkOutcome::default();
        for (index, update) in updates.iter().enumerate() {
            match self.update_one_user(update, &ids).await {
                Ok(()) => outcome.succeeded.push(update.name.clone()),
                Err(error) if is_link_fatal(&error) => {
                    return outcome.abort_after_link_failure(
                        update.name.clone(),
                        error,
                        updates[index + 1..]
                            .iter()
                            .map(|update| update.name.clone()),
                        LinkFailurePolicy::ErrorWithoutAcknowledged,
                    );
                }
                Err(error) => outcome.failed.push((update.name.clone(), error)),
            }
        }
        Ok(outcome)
    }

    async fn disk_status(&self) -> Result<Vec<DiskStatus>, DriverError> {
        let body = self.http.get_text(PATH_STORAGE).await?;
        Ok(parse_disks(&body))
    }

    async fn recording_status(&self) -> Result<RecordingStatus, DriverError> {
        // Hik enumerates recordings via `POST /ISAPI/ContentMgmt/search` with a
        // CMSearchDescription; the CMSearchResult carries `searchMatchItem`s with
        // a `timeSpan` (RFC3339, UTC). `responseStatusStrg` is `MORE` while more
        // pages remain — page by advancing `searchResultPosition`. Timestamps are
        // absolute (Z-suffixed), so no zone guess is needed.
        let now = Utc::now();
        let start = now - chrono::Duration::days(RECORDING_LOOKBACK_DAYS);
        let end = now + chrono::Duration::days(1);
        // The device validates `searchID` as a UUID and rejects anything else
        // with `badXmlContent`, which surfaced as HTTP 400 on every recording
        // read — live-verified on a DS-N316/2(D) running V4.75. A fresh id per
        // search also keeps concurrent searches from sharing device-side state.
        let search_id = uuid::Uuid::new_v4().to_string();

        let mut spans: Vec<RecordSpan> = Vec::new();
        let mut position: u32 = 0;
        // Bounded by `MAX_SEARCH_PAGES`, not an unconditional `loop`: a device
        // that never stops signalling `MORE` (buggy or hostile) must not be
        // able to grow `spans` or hold this call open indefinitely.
        for _ in 0..MAX_SEARCH_PAGES {
            let request = content_search_body(&search_id, DEFAULT_TRACK_ID, start, end, position);
            let response = self
                .http
                .post_text(PATH_CONTENT_SEARCH, &request, CONTENT_TYPE_XML)
                .await?;
            let (matched, page) = parse_content_search(&response);
            spans.extend(page);
            // Stop when the device signals no more results (short page) or reports
            // MORE is absent. `matched < page size` ends the loop.
            if matched < SEARCH_PAGE as usize || !response_has_more(&response) {
                break;
            }
            position += matched as u32;
        }

        let oldest = spans.iter().map(|s| s.start).min();
        let newest = spans.iter().map(|s| s.end).max();
        let active = newest.is_some_and(|n| now - n < chrono::Duration::minutes(10));
        Ok(RecordingStatus {
            active,
            oldest,
            newest,
            spans,
        })
    }

    async fn search_recordings(
        &self,
        request: &ExportRequest,
    ) -> Result<Vec<RecordSpan>, DriverError> {
        // Same CMSearchDescription walk as recording_status, keyed to the
        // caller's channel (`camera*100 + 1`, the same main-stream encoding
        // `snapshot` already uses) and window instead of the device-wide
        // lookback, so one search answers a specific question instead of
        // scanning everything.
        let track_id = u32::from(request.channel) * 100 + 1;
        let search_id = uuid::Uuid::new_v4().to_string();

        let mut spans: Vec<RecordSpan> = Vec::new();
        let mut position: u32 = 0;
        for _ in 0..MAX_SEARCH_PAGES {
            let body =
                content_search_body(&search_id, track_id, request.start, request.end, position);
            let response = self
                .http
                .post_text(PATH_CONTENT_SEARCH, &body, CONTENT_TYPE_XML)
                .await?;
            let (matched, page) = parse_content_search(&response);
            spans.extend(page.into_iter().map(|span| RecordSpan {
                channel: Some(request.channel),
                ..span
            }));
            if matched < SEARCH_PAGE as usize || !response_has_more(&response) {
                break;
            }
            position += matched as u32;
        }
        Ok(spans)
    }

    async fn export_download(
        &self,
        request: &ExportRequest,
    ) -> Result<Option<DownloadStream>, DriverError> {
        // ISAPI's own search reply already carries a device-native segment
        // descriptor (`mediaSegmentDescriptor/playbackURI` — the exact
        // `starttime`/`endtime`/`name`/`size` handle `ContentMgmt/download`
        // needs), so no second lookup or guessed path is needed: search once,
        // pick the segment overlapping the window, download it. Live-verified
        // on a DS-N316/2(D): `POST /ISAPI/ContentMgmt/download` with a
        // `<downloadRequest><playbackURI>` body streams the raw device
        // container (`Content-Type: Opaque/data`), which the shared remux
        // plane repackages to MP4 exactly like Dahua's `.dav`.
        let track_id = u32::from(request.channel) * 100 + 1;
        let search_id = uuid::Uuid::new_v4().to_string();
        let body = content_search_body(&search_id, track_id, request.start, request.end, 0);
        let response = self
            .http
            .post_text(PATH_CONTENT_SEARCH, &body, CONTENT_TYPE_XML)
            .await?;
        let Some(playback_uri) =
            find_downloadable_segment_uri(&response, request.start, request.end)
        else {
            return Ok(None);
        };

        let download_body = download_request_body(&playback_uri);
        let bytes = self
            .http
            .download_stream_post(PATH_CONTENT_DOWNLOAD, &download_body, CONTENT_TYPE_XML)
            .await?;
        let filename = format!(
            "hikvision-ch{channel}-{start}",
            channel = request.channel,
            start = request.start.format("%Y%m%dT%H%M%SZ"),
        );
        Ok(Some(DownloadStream {
            bytes,
            filename,
            hints: RemuxHints::default(),
        }))
    }

    async fn snapshot(&self, channel: Option<u16>) -> Result<SnapshotRef, DriverError> {
        // ISAPI encodes the still id as `<camera>*100 + 1` (the camera's main
        // stream): camera 1 -> 101. The endpoint answers with `image/jpeg`, which
        // the HTTP client validates, so the bytes can be wrapped directly.
        let camera = u32::from(channel.unwrap_or(1));
        let stream_id = camera * 100 + 1;
        let path = format!("/ISAPI/Streaming/channels/{stream_id}/picture");
        let jpeg = self.http.get_bytes(&path).await?;
        Ok(SnapshotRef::Bytes(jpeg))
    }

    async fn stream_uri(&self, channel: Option<u16>) -> Result<StreamUri, DriverError> {
        // Hikvision encodes the RTSP stream id as `<camera><stream>`; `<camera>01`
        // is the main stream of that camera (camera 1 -> `101`).
        let camera = channel.unwrap_or(1);
        let uri = format!(
            "rtsp://{user}:{password}@{host}:{RTSP_PORT}/Streaming/Channels/{camera}01",
            user = rtsp_userinfo(&self.username),
            password = rtsp_userinfo(self.password.expose()),
            host = self.endpoint.host,
        );
        Ok(StreamUri {
            uri,
            transport: StreamTransport::Rtsp,
        })
    }

    async fn substream_uri(&self, channel: Option<u16>) -> Result<StreamUri, DriverError> {
        // Same `<camera><stream>` encoding `stream_uri` uses, but `<camera>02`
        // is the device's substream (lower-resolution) rather than the `01`
        // main stream.
        let camera = channel.unwrap_or(1);
        let uri = format!(
            "rtsp://{user}:{password}@{host}:{RTSP_PORT}/Streaming/Channels/{camera}02",
            user = rtsp_userinfo(&self.username),
            password = rtsp_userinfo(self.password.expose()),
            host = self.endpoint.host,
        );
        Ok(StreamUri {
            uri,
            transport: StreamTransport::Rtsp,
        })
    }

    async fn playback_uri(&self, request: &ExportRequest) -> Result<StreamUri, DriverError> {
        // Hikvision RTSP playback: `/Streaming/tracks/<camera>01?starttime=..&
        // endtime=..` with compact `YYYYMMDDTHHMMSSZ` UTC timestamps.
        let uri = format!(
            "rtsp://{user}:{password}@{host}:{RTSP_PORT}/Streaming/tracks/{camera}01?starttime={start}&endtime={end}",
            user = rtsp_userinfo(&self.username),
            password = rtsp_userinfo(self.password.expose()),
            host = self.endpoint.host,
            camera = request.channel,
            start = request.start.format("%Y%m%dT%H%M%SZ"),
            end = request.end.format("%Y%m%dT%H%M%SZ"),
        );
        Ok(StreamUri {
            uri,
            transport: StreamTransport::Rtsp,
        })
    }

    async fn list_channels(&self) -> Result<Vec<Channel>, DriverError> {
        // NVRs expose their IP cameras (with display names) under InputProxy; a
        // DVR exposes analog inputs under System/Video/inputs. A hybrid NVR can
        // have both at once, so both endpoints are probed and merged rather
        // than treated as mutually exclusive — see `merge_channels` for the
        // keying/collision policy this assumes.
        //
        // A link-fatal InputProxy error (`is_link_fatal`: Auth/ClockSkew/
        // Transport) means the credentials or link to the device are broken,
        // so every further probe — including the VideoInput fallback — would
        // fail identically; it propagates immediately instead of masking the
        // real cause behind a fallback that fails the same way. Any other
        // InputProxy error (e.g. 404 on an analog-only DVR) just means that
        // endpoint has nothing to offer here, same as VideoInput being absent.
        // A successful (2xx) InputProxy response is authoritative even when it
        // lists zero channels — that is a legitimately empty NVR, not "wrong
        // endpoint" — so an empty list is never treated as a signal to fall
        // back.
        let primary = match self.http.get_text(PATH_INPUT_PROXY_CHANNELS).await {
            Ok(body) => Some(parse_input_proxy_channels(&body)),
            Err(error) if is_link_fatal(&error) => return Err(error),
            Err(_) => None,
        };
        let secondary = self
            .http
            .get_text(PATH_VIDEO_INPUT_CHANNELS)
            .await
            .ok()
            .map(|body| parse_video_input_channels(&body));

        let channels = match (primary, secondary) {
            (Some(primary), Some(secondary)) => merge_channels(primary, secondary),
            (Some(primary), None) => primary,
            (None, Some(secondary)) if !secondary.is_empty() => secondary,
            (None, _) => return Err(DriverError::Unsupported),
        };
        Ok(self.attach_ptz_support(channels).await)
    }

    async fn ptz(&self, channel: u16, command: PtzCommand) -> Result<(), DriverError> {
        // ISAPI PTZ is a `PUT`: a continuous move is `PTZData` velocities in
        // -100..=100 (0 stops that axis), and a preset recall is an empty-body
        // `.../presets/{id}/goto`.
        match command {
            PtzCommand::Move(vector) => {
                let path = format!("/ISAPI/PTZCtrl/channels/{channel}/continuous");
                self.http
                    .put_text(&path, &ptz_continuous_body(vector), CONTENT_TYPE_XML)
                    .await?;
            }
            PtzCommand::Stop => {
                let path = format!("/ISAPI/PTZCtrl/channels/{channel}/continuous");
                let body = ptz_continuous_body(PtzVector {
                    pan: 0.0,
                    tilt: 0.0,
                    zoom: 0.0,
                });
                self.http.put_text(&path, &body, CONTENT_TYPE_XML).await?;
            }
            PtzCommand::Preset { id } => {
                let path = format!("/ISAPI/PTZCtrl/channels/{channel}/presets/{id}/goto");
                self.http.put_text(&path, "", CONTENT_TYPE_XML).await?;
            }
        }
        Ok(())
    }
}

/// Build the ISAPI `<PTZData>` continuous-move body, mapping the normalized
/// `-1.0..=1.0` velocities onto ISAPI's `-100..=100` integer range.
fn ptz_continuous_body(vector: PtzVector) -> String {
    let vector = vector.clamped();
    format!(
        "<PTZData><pan>{}</pan><tilt>{}</tilt><zoom>{}</zoom></PTZData>",
        to_isapi_velocity(vector.pan),
        to_isapi_velocity(vector.tilt),
        to_isapi_velocity(vector.zoom),
    )
}

/// Scale a normalized `-1.0..=1.0` velocity onto ISAPI's `-100..=100` integers.
fn to_isapi_velocity(value: f32) -> i32 {
    (value * 100.0).round() as i32
}

/// Percent-encode `value` for safe embedding in the RTSP URI's `user:password@`
/// authority segment. [`NON_ALPHANUMERIC`] is the same conservative encoding
/// `read_security_cap` uses for the ISAPI query: a raw `:`/`@`/`/` in a
/// credential could otherwise break out of the userinfo segment and redirect
/// the parsed host, port, or path.
fn rtsp_userinfo(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}

// ── Pure parsers (structure verified against a live device) ─────────────────

/// Parse an `<InputProxyChannelList>` (NVR IP cameras) into [`Channel`]s. Each
/// `<InputProxyChannel>` carries a channel `<id>` and a camera `<name>`; the
/// first `<id>`/`<name>` in the block are the channel's own (a nested source
/// descriptor's tags come later and are ignored).
fn parse_input_proxy_channels(xml: &str) -> Vec<Channel> {
    xml_blocks(xml, "InputProxyChannel")
        .iter()
        .filter_map(|block| channel_from_block(block))
        .collect()
}

/// Build a `CMSearchDescription` request body for one search page.
fn content_search_body(
    search_id: &str,
    track_id: u32,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    position: u32,
) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
<CMSearchDescription>\
<searchID>{search_id}</searchID>\
<trackList><trackID>{track_id}</trackID></trackList>\
<timeSpanList><timeSpan>\
<startTime>{start}</startTime><endTime>{end}</endTime>\
</timeSpan></timeSpanList>\
<maxResults>{SEARCH_PAGE}</maxResults>\
<searchResultPosition>{position}</searchResultPosition>\
</CMSearchDescription>",
        start = start.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        end = end.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    )
}

/// Whether a CMSearchResult signals that more pages remain (`MORE`).
fn response_has_more(xml: &str) -> bool {
    xml_tag(xml, "responseStatusStrg").is_some_and(|s| s.trim().eq_ignore_ascii_case("MORE"))
}

/// Parse a CMSearchResult into `(num_matches, spans)`. Each `searchMatchItem`
/// carries a `timeSpan` with RFC3339 `startTime`/`endTime` (UTC). A span is
/// kept only when both parse and end is after start.
fn parse_content_search(xml: &str) -> (usize, Vec<RecordSpan>) {
    let spans: Vec<RecordSpan> = xml_blocks(xml, "searchMatchItem")
        .iter()
        .filter_map(|item| {
            let span = xml_tag(item, "timeSpan")?;
            let start = parse_isapi_time(xml_tag(span, "startTime")?)?;
            let end = parse_isapi_time(xml_tag(span, "endTime")?)?;
            (end > start).then_some(RecordSpan {
                start,
                end,
                channel: None,
            })
        })
        .collect();
    let matched = xml_tag(xml, "numOfMatches")
        .and_then(|n| n.trim().parse().ok())
        .unwrap_or(spans.len());
    (matched, spans)
}

/// Find the first `searchMatchItem` overlapping `[start, end)` and return its
/// `mediaSegmentDescriptor/playbackURI` verbatim — the device-native handle
/// `ContentMgmt/download` expects. `None` when no segment overlaps the window
/// (no footage) or a matching segment carries no descriptor (audio-only /
/// event-only match, which cannot be downloaded as video).
fn find_downloadable_segment_uri(
    xml: &str,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Option<String> {
    xml_blocks(xml, "searchMatchItem")
        .into_iter()
        .find_map(|item| {
            let span = xml_tag(item, "timeSpan")?;
            let span_start = parse_isapi_time(xml_tag(span, "startTime")?)?;
            let span_end = parse_isapi_time(xml_tag(span, "endTime")?)?;
            // CONTAINMENT, not overlap. `ContentMgmt/download` serves the whole
            // segment file and nothing less: measured against a DS-N316/2(D),
            // a URI narrowed to one minute still answered
            // `Content-Length: 1064320428` — the same 1.06 GB as the untouched
            // segment URI, and dropping the `name` handle to force a pure time
            // range answered 400. The times in a playbackURI are decorative for
            // this endpoint; only `name` selects the file.
            //
            // So the fast path is only honest when the caller wants at least
            // the whole segment. A narrower window falls through to the RTSP
            // clip plane, which cuts precisely with `-t`. Taking the fast path
            // anyway would hand back footage from outside the requested window
            // — a gigabyte for a minute, and time the operator did not ask for.
            if span_start < start || span_end > end {
                return None;
            }
            let descriptor = xml_tag(item, "mediaSegmentDescriptor")?;
            let uri = xml_tag(descriptor, "playbackURI")?;
            (!uri.trim().is_empty()).then(|| xml_unescape(uri.trim()))
        })
}

/// Build the `downloadRequest` body `ContentMgmt/download` expects, naming the
/// segment via its verbatim `playbackURI` from a prior search.
fn download_request_body(playback_uri: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
<downloadRequest version=\"1.0\" xmlns=\"http://www.hikvision.com/ver20/XMLSchema\">\
<playbackURI>{}</playbackURI></downloadRequest>",
        xml_escape(playback_uri),
    )
}

/// Parse an ISAPI RFC3339 timestamp (e.g. `2026-07-01T08:00:00Z`, or with a
/// zone offset) into a UTC instant.
fn parse_isapi_time(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value.trim())
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// Parse a `<VideoInputChannelList>` (DVR analog inputs) into [`Channel`]s. The
/// display name is often absent here, so it falls back to the resolution
/// descriptor when present, else `None`.
fn parse_video_input_channels(xml: &str) -> Vec<Channel> {
    xml_blocks(xml, "VideoInputChannel")
        .iter()
        .filter_map(|block| channel_from_block(block))
        .collect()
}

/// Build a [`Channel`] from one ISAPI channel block: the leading `<id>` is the
/// 1-based channel index; `<name>` (else `<resDesc>`) is the display title.
fn channel_from_block(block: &str) -> Option<Channel> {
    let index: u16 = xml_tag(block, "id")?.trim().parse().ok()?;
    let name = xml_tag(block, "name")
        .or_else(|| xml_tag(block, "resDesc"))
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned);
    let enabled = xml_tag(block, "enabled")
        .or_else(|| xml_tag(block, "online"))
        .map(|value| value.trim().eq_ignore_ascii_case("true"));
    Some(Channel {
        index,
        name,
        enabled,
        ptz_support: PtzSupport::Unknown,
        recording: None,
    })
}

/// Merge Hikvision's two channel-listing endpoints — the IP-camera InputProxy
/// list (`primary`) and the analog/PoE VideoInput list (`secondary`) — into
/// one list keyed by channel index, ascending. `primary` wins on an index
/// collision: its name is the operator-assigned camera title, which is more
/// useful than VideoInput's bare resolution descriptor.
///
/// **VERIFY**: this assumes InputProxy and VideoInput share one coherent
/// per-device index space on a hybrid NVR — not verifiable from source, no
/// hybrid device has been tested live. The driver already makes this same
/// single-index-space assumption everywhere else (`stream_uri`/`snapshot`/
/// `ptz` all key on one channel index), so this surfaces the existing
/// assumption at one more call site rather than introducing a new one.
fn merge_channels(primary: Vec<Channel>, secondary: Vec<Channel>) -> Vec<Channel> {
    let mut by_index: BTreeMap<u16, Channel> = BTreeMap::new();
    for channel in secondary {
        by_index.insert(channel.index, channel);
    }
    for channel in primary {
        by_index.insert(channel.index, channel);
    }
    by_index.into_values().collect()
}

/// Build [`DeviceInfo`] from a `<DeviceInfo>` document.
///
/// `channels` and `device_time` are left `None` — this ISAPI document exposes no
/// field for them.
fn parse_device_info(xml: &str) -> DeviceInfo {
    DeviceInfo {
        model: xml_tag(xml, "model").map(str::to_owned),
        firmware: xml_tag(xml, "firmwareVersion").map(str::to_owned),
        serial: xml_tag(xml, "serialNumber").map(str::to_owned),
        channels: None,
        device_time: None,
    }
}

/// Parse a `<UserList>` document into accounts, in document order.
///
/// Each `<User>` block also carries other nested tags (notably an `<id>`), so
/// name/level are extracted from *within* each block, never globally.
fn parse_users(xml: &str) -> Vec<UserAccount> {
    xml_blocks(xml, "User")
        .into_iter()
        .filter_map(parse_user_block)
        .collect()
}

/// Parse one `<User>...</User>` block into a [`UserAccount`], dropping a block
/// with no login name. `userLevel` maps to the account group; authority tokens
/// are left empty (ISAPI exposes them via a separate permission resource).
fn parse_user_block(block: &str) -> Option<UserAccount> {
    let name = xml_tag(block, "userName")?;
    Some(UserAccount {
        name: name.to_owned(),
        group: xml_tag(block, "userLevel").map(str::to_owned),
        authorities: Vec::new(),
    })
}

/// Parse an `<hddList>` document into per-disk records, keeping only disks that
/// report a positive capacity.
fn parse_disks(xml: &str) -> Vec<DiskStatus> {
    xml_blocks(xml, "hdd")
        .into_iter()
        .filter_map(parse_hdd_block)
        .collect()
}

/// Parse one `<hdd>...</hdd>` block into a [`DiskStatus`].
///
/// Capacity and free space are reported in MiB and converted to bytes.
/// `status == "ok"` is healthy; any other status is flagged as an error.
fn parse_hdd_block(block: &str) -> Option<DiskStatus> {
    let capacity_mib: u64 = xml_tag(block, "capacity")?.parse().ok()?;
    if capacity_mib == 0 {
        return None;
    }
    let free_mib: u64 = xml_tag(block, "freeSpace")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let status = xml_tag(block, "status").unwrap_or_default();
    let healthy = status.eq_ignore_ascii_case("ok");
    let name = xml_tag(block, "hddName")
        .map(str::to_owned)
        .unwrap_or_else(|| "hdd".to_owned());
    Some(DiskStatus {
        name,
        total_bytes: capacity_mib.saturating_mul(MIB),
        used_bytes: capacity_mib.saturating_sub(free_mib).saturating_mul(MIB),
        state: if healthy {
            DiskState::Ok
        } else {
            DiskState::Error
        },
        is_error: !healthy,
    })
}

// ── User provisioning: password derivation and ISAPI write bodies ───────────

/// Whether a driver error means the link or credentials to the device are
/// broken, so every further probe on this device would fail identically —
/// not that one specific resource happens to be unavailable.
///
/// `Auth`/`ClockSkew` and typed transport failures mean the credentials, clock,
/// or link are broken. In a bulk user operation this stops further calls: the
/// acknowledged prefix is returned and, when the failure hits before anything
/// was acknowledged, the batch is a top-level `Err` instead of a
/// successful-looking empty outcome. In `list_channels` it means the InputProxy
/// request failed for the same reason the VideoInput fallback would, so trying
/// the fallback would only mask the real cause. A `Device`/`Protocol`/
/// `Unsupported` error is specific to one item's request (or, for
/// `list_channels`, means only that one resource is unavailable/unsupported) and
/// does not trip this path.
fn is_link_fatal(error: &DriverError) -> bool {
    matches!(
        error,
        DriverError::Auth
            | DriverError::ClockSkew
            | DriverError::Transport(_)
            | DriverError::Timeout(_)
            | DriverError::Unreachable(_)
    )
}

/// The per-user security parameters read from `/ISAPI/Security/capabilities`.
struct SecurityCap {
    /// Whether the firmware expects the salted-SHA256 (irreversible) password.
    is_irreversible: bool,
    /// Per-account salt mixed into the irreversible hash (empty when none).
    salt: String,
    /// SHA-256 iteration count for the irreversible hash.
    iterations: u32,
}

/// Parse a `<SecurityCap>` document into the fields needed to encode a password.
///
/// `isIrreversible` defaults to `false` (legacy plaintext) when absent. The
/// iteration count is read from `iterateNumber`, falling back to `keyIterateNum`
/// (the field name varies across firmware) and then to [`DEFAULT_ITERATIONS`];
/// it is clamped to [`MAX_ITERATIONS`] so a hostile or buggy device cannot make
/// [`irreversible_password`]'s synchronous hashing loop run unbounded.
fn parse_security_cap(xml: &str) -> SecurityCap {
    let is_irreversible =
        xml_tag(xml, "isIrreversible").is_some_and(|value| value.eq_ignore_ascii_case("true"));
    let salt = xml_tag(xml, "salt").unwrap_or_default().to_owned();
    let iterations = xml_tag(xml, "iterateNumber")
        .or_else(|| xml_tag(xml, "keyIterateNum"))
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_ITERATIONS)
        .min(MAX_ITERATIONS);
    SecurityCap {
        is_irreversible,
        salt,
        iterations,
    }
}

/// Derive the Hikvision irreversible-encryption password field.
///
/// Per the ISAPI security spec the device stores a `SHA-256` digest of
/// `userName + salt + password`, iterated `iterations` times over the lowercase
/// hex digest: the first pass hashes the concatenation, then each further pass
/// hashes the previous hex string. `iterations` is clamped to at least one so a
/// missing or zero count still yields a single hash.
///
/// **The exact wire format is taken from the published spec and has not yet been
/// checked against a live device** (no public test vector exists); validate it
/// against real hardware before enabling writes in production. The plaintext
/// `password` is only ever fed into the hash and is never logged.
fn irreversible_password(username: &str, salt: &str, password: &str, iterations: u32) -> String {
    let mut digest = sha256_hex(&format!("{username}{salt}{password}"));
    for _ in 1..iterations.max(1) {
        digest = sha256_hex(&digest);
    }
    digest
}

/// Lowercase-hex SHA-256 of `input`.
fn sha256_hex(input: &str) -> String {
    hex::encode(Sha256::digest(input.as_bytes()))
}

/// Map a requested group to a Hikvision `userLevel`.
///
/// `group` must be one of the literals `list_users` reads back
/// (`Administrator`/`Operator`, case-insensitive) or absent, which defaults to
/// the least-privileged `Operator`. An unrecognized literal is rejected rather
/// than silently mapped to `Operator`, so a typo cannot under-grant an account.
fn hik_user_level(group: Option<&str>) -> Result<&'static str, DriverError> {
    match group {
        None => Ok("Operator"),
        Some(value) if value.eq_ignore_ascii_case("Administrator") => Ok("Administrator"),
        Some(value) if value.eq_ignore_ascii_case("Operator") => Ok("Operator"),
        Some(other) => Err(DriverError::Device(format!(
            "unrecognized Hikvision user group '{other}'; expected 'Administrator' or 'Operator'"
        ))),
    }
}

/// The per-item error a create/update returns when the caller asks to grant,
/// replace, or clear authority tokens: the ISAPI `AuthorityList` write body is
/// not implemented (see the module doc), so silently dropping the facet while
/// reporting success would leave the account's authorities untouched from
/// what the caller asked for.
fn authorities_not_implemented() -> DriverError {
    DriverError::Device(
        "authority/permission-list editing is not implemented for Hikvision; the ISAPI \
         AuthorityList write body has not been verified against a live device"
            .to_owned(),
    )
}

/// Whether `id` is a value ISAPI would emit for a `<User>` block's `<id>` (an
/// unsigned decimal integer). Validated before the value — read back from the
/// device's own listing — is interpolated into a `PUT`/`DELETE` URL path, so a
/// malformed or hostile id cannot inject an unexpected path segment.
fn is_valid_isapi_id(id: &str) -> bool {
    !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit())
}

/// Build the ISAPI `<User>` provisioning body. `name` and `password_field` are
/// XML-escaped; `level` is a fixed protocol literal.
fn build_create_user_body(name: &str, password_field: &str, level: &str) -> String {
    format!(
        "<User><id>0</id><userName>{name}</userName><password>{password}</password><userLevel>{level}</userLevel></User>",
        name = xml_escape(name),
        password = xml_escape(password_field),
    )
}

/// Build the ISAPI `<User>` edit body for a `PUT /ISAPI/Security/users/{id}`.
///
/// `id` (numeric, from the device's own listing) and `userName` (the identity)
/// are always present; `<password>` and `<userLevel>` are emitted only for the
/// facets the update changes. `name`/`password_field` are XML-escaped; `level` is
/// a fixed protocol literal. Whether a partial `<User>` preserves the omitted
/// fields (versus resetting them) is per the ISAPI spec and not yet live-verified.
fn build_modify_user_body(
    id: &str,
    name: &str,
    password_field: Option<&str>,
    level: Option<&str>,
) -> String {
    let mut body = format!(
        "<User><id>{id}</id><userName>{name}</userName>",
        name = xml_escape(name),
    );
    if let Some(password) = password_field {
        body.push_str(&format!(
            "<password>{password}</password>",
            password = xml_escape(password),
        ));
    }
    if let Some(level) = level {
        body.push_str(&format!("<userLevel>{level}</userLevel>"));
    }
    body.push_str("</User>");
    body
}

/// Escape the five XML metacharacters so a value cannot break out of its element.
fn xml_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
    out
}

/// Inverse of [`xml_escape`]: decode the five XML entities back to their
/// literal characters. Needed for `playbackURI`, which the device echoes back
/// XML-escaped (its query string's `&` becomes `&amp;`) but which must be
/// re-escaped verbatim, not double-escaped, when placed in the download
/// request body.
fn xml_unescape(input: &str) -> String {
    input
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

/// Interpret an ISAPI create response: success, an idempotent already-exists, or
/// a device error.
fn interpret_create(xml: &str) -> Result<(), DriverError> {
    let status = ResponseStatus::parse(xml);
    if status.is_success() || status.indicates_already_exists() {
        return Ok(());
    }
    Err(DriverError::Device(status.describe()))
}

/// Interpret an ISAPI delete response: success or a device error. A user that was
/// already gone is handled before the DELETE, by name resolution.
fn interpret_delete(xml: String) -> Result<(), DriverError> {
    let status = ResponseStatus::parse(&xml);
    if status.is_success() {
        return Ok(());
    }
    Err(DriverError::Device(status.describe()))
}

/// Interpret an ISAPI edit (PUT) response: success or a device error. Mirrors
/// [`interpret_delete`]; an absent target is handled before the PUT, by name
/// resolution (an update cannot create a missing user).
fn interpret_modify(xml: &str) -> Result<(), DriverError> {
    let status = ResponseStatus::parse(xml);
    if status.is_success() {
        return Ok(());
    }
    Err(DriverError::Device(status.describe()))
}

/// A parsed ISAPI `<ResponseStatus>` write result.
struct ResponseStatus {
    status_code: Option<u32>,
    status_string: Option<String>,
    sub_status_code: Option<String>,
}

impl ResponseStatus {
    /// Parse the status/sub-status fields from a `<ResponseStatus>` document.
    fn parse(xml: &str) -> Self {
        Self {
            status_code: xml_tag(xml, "statusCode").and_then(|value| value.parse().ok()),
            status_string: xml_tag(xml, "statusString").map(str::to_owned),
            sub_status_code: xml_tag(xml, "subStatusCode").map(str::to_owned),
        }
    }

    /// ISAPI signals success with `statusCode` `1`; some firmware only sets
    /// `subStatusCode` `ok`, so either is accepted.
    fn is_success(&self) -> bool {
        self.status_code == Some(1)
            || self
                .sub_status_code
                .as_deref()
                .is_some_and(|code| code.eq_ignore_ascii_case("ok"))
    }

    /// Whether the device reported that the user already exists, which makes a
    /// create idempotent. Matches the `userAlreadyExist` sub-status literal
    /// case-insensitively, tolerating firmware spelling variants.
    fn indicates_already_exists(&self) -> bool {
        self.sub_status_code
            .as_deref()
            .is_some_and(|code| code.to_ascii_lowercase().contains("exist"))
    }

    /// A log-safe, password-free description for a [`DriverError::Device`].
    fn describe(&self) -> String {
        match (&self.status_string, &self.sub_status_code, self.status_code) {
            (_, Some(sub), _) => format!("ISAPI write rejected: {sub}"),
            (Some(text), None, _) => format!("ISAPI write rejected: {text}"),
            (None, None, Some(code)) => format!("ISAPI write rejected with statusCode {code}"),
            (None, None, None) => "ISAPI write returned an unrecognized response".to_owned(),
        }
    }
}

/// Map each `<User>` block's `userName` to its numeric `id`, for delete-by-id.
fn user_name_to_id(xml: &str) -> HashMap<String, String> {
    xml_blocks(xml, "User")
        .into_iter()
        .filter_map(|block| {
            let name = xml_tag(block, "userName")?;
            let id = xml_tag(block, "id")?;
            Some((name.to_owned(), id.to_owned()))
        })
        .collect()
}

// ── Dep-free XML extraction for the flat ISAPI schema ───────────────────────

/// Return the trimmed text of the first `<tag ...>value</tag>` found in `scope`.
///
/// Handles an optional attribute list on the open tag and surrounding
/// whitespace. Returns `None` for a self-closing tag, an empty value, or a
/// missing tag. This is a structural extractor for a known flat schema, not a
/// general XML parser: it does not descend into nested same-named elements.
fn xml_tag<'a>(scope: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}");
    let mut from = 0;
    while let Some(rel) = scope[from..].find(&open) {
        let after = from + rel + open.len();
        match scope[after..].chars().next() {
            // No attributes: `<tag>value`.
            Some('>') => return tag_value(scope, after + 1, tag),
            // Attribute list: `<tag a="b">value` (or self-closing `<tag .../>`).
            Some(c) if c.is_ascii_whitespace() => {
                let gt = after + scope[after..].find('>')?;
                if scope[..gt].ends_with('/') {
                    return None;
                }
                return tag_value(scope, gt + 1, tag);
            }
            // A longer tag name (e.g. `<id>` matched inside `<identity>`): skip on.
            _ => from = after,
        }
    }
    None
}

/// Slice the value from `value_start` up to the matching `</tag>`, trimmed;
/// `None` if the close tag is absent or the value is empty.
fn tag_value<'a>(scope: &'a str, value_start: usize, tag: &str) -> Option<&'a str> {
    let close = format!("</{tag}>");
    let end = value_start + scope[value_start..].find(&close)?;
    let value = scope[value_start..end].trim();
    (!value.is_empty()).then_some(value)
}

/// Return each top-level `<elem ...>...</elem>` block (tags included) in order.
///
/// The open-tag match requires an exact element name (`<elem>`, `<elem/>`, or
/// `<elem ` followed by attributes), so `<elem>` is not matched inside a longer
/// sibling like `<elemList>`. The close needle carries its `>` so `</elem>` is
/// not matched inside `</elemList>`. Assumes siblings do not nest same-named
/// elements, which holds for this flat schema.
fn xml_blocks<'a>(xml: &'a str, elem: &str) -> Vec<&'a str> {
    let open = format!("<{elem}");
    let close = format!("</{elem}>");
    let mut blocks = Vec::new();
    let mut from = 0;
    while let Some(rel) = xml[from..].find(&open) {
        let start = from + rel;
        let after = start + open.len();
        let is_exact = match xml[after..].chars().next() {
            Some('>' | '/') => true,
            Some(c) => c.is_ascii_whitespace(),
            None => false,
        };
        if !is_exact {
            from = after;
            continue;
        }
        let Some(close_rel) = xml[start..].find(&close) else {
            break;
        };
        let end = start + close_rel + close.len();
        blocks.push(&xml[start..end]);
        from = end;
    }
    blocks
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use super::*;

    const SAMPLE_DEVICE_INFO: &str = "\
<DeviceInfo version=\"2.0\" xmlns=\"http://www.hikvision.com/ver20/XMLSchema\">\
<deviceName>Network Video Recorder</deviceName>\
<deviceID>abc</deviceID>\
<model>DS-N316/2(D)</model>\
<serialNumber>DS-N316/2(D)1620ABCDEF</serialNumber>\
<firmwareVersion>V4.75.000</firmwareVersion>\
<deviceType>NVR</deviceType>\
</DeviceInfo>";

    const SAMPLE_USERS: &str = "\
<UserList version=\"2.0\" xmlns=\"http://www.hikvision.com/ver20/XMLSchema\">\
<User><id>1</id><userName>admin</userName><userLevel>Administrator</userLevel></User>\
<User><id>2</id><userName>it</userName><userLevel>Operator</userLevel></User>\
</UserList>";

    /// A listing where the target user's `<id>` is not a plain decimal integer
    /// (as a compromised/buggy device could report), for the id-validation
    /// tests: the value must never be interpolated into a PUT/DELETE path.
    const SAMPLE_USERS_BAD_ID: &str = "\
<UserList version=\"2.0\" xmlns=\"http://www.hikvision.com/ver20/XMLSchema\">\
<User><id>1</id><userName>admin</userName><userLevel>Administrator</userLevel></User>\
<User><id>2/../evil</id><userName>it</userName><userLevel>Operator</userLevel></User>\
</UserList>";

    const SAMPLE_STORAGE: &str = "\
<hddList version=\"2.0\" xmlns=\"http://www.hikvision.com/ver20/XMLSchema\">\
<hdd><id>1</id><hddName>hdd1</hddName><status>ok</status>\
<capacity>7630885</capacity><freeSpace>0</freeSpace><property>RW</property></hdd>\
<hdd><id>2</id><hddName>hdd2</hddName><status>unformatted</status>\
<capacity>3815442</capacity><freeSpace>3815442</freeSpace><property>RW</property></hdd>\
<hdd><id>3</id><hddName>hdd3</hddName><status>ok</status>\
<capacity>0</capacity><freeSpace>0</freeSpace><property>RW</property></hdd>\
</hddList>";

    /// Security caps for firmware that demands the salted, irreversible password
    /// (`isIrreversible=true`), with a two-round iteration count.
    const SAMPLE_CAP_IRREVERSIBLE: &str = "\
<SecurityCap version=\"2.0\" xmlns=\"http://www.hikvision.com/ver20/XMLSchema\">\
<isIrreversible>true</isIrreversible>\
<salt>0123456789abcdef</salt>\
<iterateNumber>2</iterateNumber>\
</SecurityCap>";

    /// Security caps for legacy firmware that stores the plaintext password.
    const SAMPLE_CAP_PLAINTEXT: &str = "\
<SecurityCap version=\"2.0\" xmlns=\"http://www.hikvision.com/ver20/XMLSchema\">\
<isIrreversible>false</isIrreversible>\
</SecurityCap>";

    /// A successful ISAPI write acknowledgement.
    const RESPONSE_OK: &str = "\
<ResponseStatus version=\"2.0\" xmlns=\"http://www.hikvision.com/ver20/XMLSchema\">\
<requestURL>/ISAPI/Security/users</requestURL>\
<statusCode>1</statusCode><statusString>OK</statusString><subStatusCode>ok</subStatusCode>\
</ResponseStatus>";

    /// A device-level rejection (weak password), which is one user's failure.
    const RESPONSE_RISK_PASSWORD: &str = "\
<ResponseStatus version=\"2.0\" xmlns=\"http://www.hikvision.com/ver20/XMLSchema\">\
<statusCode>6</statusCode><statusString>Invalid Content</statusString>\
<subStatusCode>riskPassword</subStatusCode></ResponseStatus>";

    /// A device-level "already exists", which a create treats as idempotent.
    const RESPONSE_ALREADY_EXISTS: &str = "\
<ResponseStatus version=\"2.0\" xmlns=\"http://www.hikvision.com/ver20/XMLSchema\">\
<statusCode>4</statusCode><statusString>Invalid Operation</statusString>\
<subStatusCode>userAlreadyExist</subStatusCode></ResponseStatus>";

    /// A canned [`HttpDeviceClient`] that replays fixed text (and, for snapshots,
    /// fixed bytes) per request path.
    struct FakeHttpClient {
        responses: HashMap<String, String>,
        byte_responses: HashMap<String, Vec<u8>>,
    }

    impl FakeHttpClient {
        fn new(pairs: &[(&str, &str)]) -> Self {
            Self {
                responses: pairs
                    .iter()
                    .map(|(path, body)| ((*path).to_owned(), (*body).to_owned()))
                    .collect(),
                byte_responses: HashMap::new(),
            }
        }

        /// Attach canned binary bodies keyed by request path, for `get_bytes`.
        fn with_bytes(mut self, pairs: &[(&str, &[u8])]) -> Self {
            self.byte_responses = pairs
                .iter()
                .map(|(path, body)| ((*path).to_owned(), (*body).to_vec()))
                .collect();
            self
        }
    }

    #[async_trait]
    impl HttpDeviceClient for FakeHttpClient {
        async fn get_text(&self, path: &str) -> Result<String, DriverError> {
            self.responses
                .get(path)
                .cloned()
                .ok_or_else(|| DriverError::Device(format!("no canned response for {path}")))
        }

        async fn get_bytes(&self, path: &str) -> Result<Vec<u8>, DriverError> {
            let bytes = self
                .byte_responses
                .get(path)
                .cloned()
                .ok_or_else(|| DriverError::Device(format!("no canned bytes for {path}")))?;
            // Faithfully double the real client's port contract: `get_bytes`
            // yields a validated JPEG, rejecting a non-image body.
            if bytes.starts_with(&[0xFF, 0xD8]) {
                Ok(bytes)
            } else {
                Err(DriverError::Device(
                    "snapshot response was not a JPEG image".to_owned(),
                ))
            }
        }
    }

    fn driver_with(pairs: &[(&str, &str)]) -> HikvisionDriver {
        HikvisionDriver::new(
            Arc::new(FakeHttpClient::new(pairs)),
            DeviceEndpoint::new("device.test", 80),
            "admin",
            Secret::new("secretpw"),
        )
    }

    /// One HTTP call the driver made, captured verbatim (path, and — for a POST —
    /// its body and content type) so tests can assert on the request the driver
    /// *would send* without ever touching a real device.
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Call {
        Get(String),
        Post {
            path: String,
            body: String,
            content_type: String,
        },
        Put {
            path: String,
            body: String,
            content_type: String,
        },
        Delete(String),
    }

    /// A recording [`HttpDeviceClient`]: canned GET responses keyed by path, and
    /// FIFO queues of canned POST/DELETE outcomes (which may be injected errors).
    /// Every call is recorded for assertion. No network is ever touched.
    struct RecordingHttpClient {
        get_responses: HashMap<String, String>,
        get_errors: Mutex<HashMap<String, VecDeque<DriverError>>>,
        post_responses: Mutex<VecDeque<Result<String, DriverError>>>,
        put_responses: Mutex<VecDeque<Result<String, DriverError>>>,
        delete_responses: Mutex<VecDeque<Result<String, DriverError>>>,
        download_post_response: Mutex<Option<Result<Vec<u8>, DriverError>>>,
        calls: Mutex<Vec<Call>>,
    }

    impl RecordingHttpClient {
        fn new(get_responses: &[(&str, &str)]) -> Self {
            Self {
                get_responses: get_responses
                    .iter()
                    .map(|(path, body)| ((*path).to_owned(), (*body).to_owned()))
                    .collect(),
                get_errors: Mutex::new(HashMap::new()),
                post_responses: Mutex::new(VecDeque::new()),
                put_responses: Mutex::new(VecDeque::new()),
                delete_responses: Mutex::new(VecDeque::new()),
                download_post_response: Mutex::new(None),
                calls: Mutex::new(Vec::new()),
            }
        }

        /// Queue the bytes (or error) the next `download_stream_post` call
        /// resolves to.
        fn with_download_post(self, response: Result<Vec<u8>, DriverError>) -> Self {
            *self.download_post_response.lock().unwrap() = Some(response);
            self
        }

        /// Queue an error for the next GET to `path`, ahead of any canned
        /// success body — for tests asserting on GET-level failure handling
        /// (e.g. `is_link_fatal`'s error-class distinction).
        fn with_get_error(self, path: &str, error: DriverError) -> Self {
            self.get_errors
                .lock()
                .unwrap()
                .entry(path.to_owned())
                .or_default()
                .push_back(error);
            self
        }

        fn with_posts(self, responses: Vec<Result<String, DriverError>>) -> Self {
            *self.post_responses.lock().unwrap() = responses.into();
            self
        }

        fn with_puts(self, responses: Vec<Result<String, DriverError>>) -> Self {
            *self.put_responses.lock().unwrap() = responses.into();
            self
        }

        fn with_deletes(self, responses: Vec<Result<String, DriverError>>) -> Self {
            *self.delete_responses.lock().unwrap() = responses.into();
            self
        }

        fn calls(&self) -> Vec<Call> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl HttpDeviceClient for RecordingHttpClient {
        async fn get_text(&self, path: &str) -> Result<String, DriverError> {
            self.calls.lock().unwrap().push(Call::Get(path.to_owned()));
            if let Some(error) = self
                .get_errors
                .lock()
                .unwrap()
                .get_mut(path)
                .and_then(VecDeque::pop_front)
            {
                return Err(error);
            }
            self.get_responses
                .get(path)
                .cloned()
                .ok_or_else(|| DriverError::Device(format!("no canned GET for {path}")))
        }

        async fn post_text(
            &self,
            path: &str,
            body: &str,
            content_type: &str,
        ) -> Result<String, DriverError> {
            self.calls.lock().unwrap().push(Call::Post {
                path: path.to_owned(),
                body: body.to_owned(),
                content_type: content_type.to_owned(),
            });
            self.post_responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Err(DriverError::Device("no canned POST response".to_owned())))
        }

        async fn put_text(
            &self,
            path: &str,
            body: &str,
            content_type: &str,
        ) -> Result<String, DriverError> {
            self.calls.lock().unwrap().push(Call::Put {
                path: path.to_owned(),
                body: body.to_owned(),
                content_type: content_type.to_owned(),
            });
            self.put_responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Err(DriverError::Device("no canned PUT response".to_owned())))
        }

        async fn delete(&self, path: &str) -> Result<String, DriverError> {
            self.calls
                .lock()
                .unwrap()
                .push(Call::Delete(path.to_owned()));
            self.delete_responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Err(DriverError::Device("no canned DELETE response".to_owned())))
        }

        async fn download_stream_post(
            &self,
            path: &str,
            body: &str,
            content_type: &str,
        ) -> Result<
            crate::contexts::device_management::infrastructure::http_client::ByteStream,
            DriverError,
        > {
            self.calls.lock().unwrap().push(Call::Post {
                path: path.to_owned(),
                body: body.to_owned(),
                content_type: content_type.to_owned(),
            });
            let response = self
                .download_post_response
                .lock()
                .unwrap()
                .take()
                .unwrap_or_else(|| {
                    Err(DriverError::Device(
                        "no canned download response".to_owned(),
                    ))
                })?;
            Ok(Box::pin(futures_util::stream::once(async move {
                Ok(bytes::Bytes::from(response))
            })))
        }
    }

    /// Build a driver over `client`, returning the concrete client handle so a
    /// test can inspect the recorded calls after the driver runs.
    fn driver_from(client: RecordingHttpClient) -> (Arc<RecordingHttpClient>, HikvisionDriver) {
        let client = Arc::new(client);
        let driver = HikvisionDriver::new(
            client.clone(),
            DeviceEndpoint::new("device.test", 80),
            "admin",
            Secret::new("secretpw"),
        );
        (client, driver)
    }

    /// Extract the first recorded POST as `(path, body, content_type)`.
    fn first_post(calls: &[Call]) -> (String, String, String) {
        calls
            .iter()
            .find_map(|call| match call {
                Call::Post {
                    path,
                    body,
                    content_type,
                } => Some((path.clone(), body.clone(), content_type.clone())),
                _ => None,
            })
            .expect("a POST call was recorded")
    }

    /// Extract the first recorded PUT as `(path, body, content_type)`.
    fn first_put(calls: &[Call]) -> (String, String, String) {
        calls
            .iter()
            .find_map(|call| match call {
                Call::Put {
                    path,
                    body,
                    content_type,
                } => Some((path.clone(), body.clone(), content_type.clone())),
                _ => None,
            })
            .expect("a PUT call was recorded")
    }

    #[test]
    fn ptz_continuous_body_scales_velocities_onto_isapi_range() {
        let body = ptz_continuous_body(PtzVector {
            pan: 1.0,
            tilt: -0.5,
            zoom: 0.0,
        });
        assert_eq!(
            body,
            "<PTZData><pan>100</pan><tilt>-50</tilt><zoom>0</zoom></PTZData>"
        );
        // Out-of-range velocities are clamped before scaling.
        let clamped = ptz_continuous_body(PtzVector {
            pan: 3.0,
            tilt: 0.0,
            zoom: -9.0,
        });
        assert_eq!(
            clamped,
            "<PTZData><pan>100</pan><tilt>0</tilt><zoom>-100</zoom></PTZData>"
        );
    }

    #[tokio::test]
    async fn ptz_move_puts_continuous_velocities_on_the_channel() {
        let (client, driver) =
            driver_from(RecordingHttpClient::new(&[]).with_puts(vec![Ok(String::new())]));
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
        let (path, body, content_type) = first_put(&client.calls());
        assert_eq!(path, "/ISAPI/PTZCtrl/channels/2/continuous");
        assert!(body.contains("<pan>100</pan>"), "{body}");
        assert_eq!(content_type, CONTENT_TYPE_XML);
    }

    #[tokio::test]
    async fn ptz_stop_puts_zero_velocities() {
        let (client, driver) =
            driver_from(RecordingHttpClient::new(&[]).with_puts(vec![Ok(String::new())]));
        driver.ptz(1, PtzCommand::Stop).await.expect("ptz stop");
        let (path, body, _ct) = first_put(&client.calls());
        assert_eq!(path, "/ISAPI/PTZCtrl/channels/1/continuous");
        assert_eq!(
            body,
            "<PTZData><pan>0</pan><tilt>0</tilt><zoom>0</zoom></PTZData>"
        );
    }

    #[tokio::test]
    async fn ptz_preset_puts_the_goto_path_with_an_empty_body() {
        let (client, driver) =
            driver_from(RecordingHttpClient::new(&[]).with_puts(vec![Ok(String::new())]));
        driver
            .ptz(3, PtzCommand::Preset { id: 7 })
            .await
            .expect("ptz preset");
        let (path, body, _ct) = first_put(&client.calls());
        assert_eq!(path, "/ISAPI/PTZCtrl/channels/3/presets/7/goto");
        assert!(body.is_empty(), "preset goto has no body");
    }

    #[test]
    fn parse_input_proxy_channels_reads_id_and_camera_name() {
        let xml = "\
<InputProxyChannelList>\
  <InputProxyChannel><id>1</id><name>Entrance</name>\
    <sourceInputPortDescriptor><id>1</id><online>true</online></sourceInputPortDescriptor>\
  </InputProxyChannel>\
  <InputProxyChannel><id>2</id><name>Register</name></InputProxyChannel>\
</InputProxyChannelList>";
        let channels = parse_input_proxy_channels(xml);
        assert_eq!(channels.len(), 2);
        // The leading <id> is the channel's own, not the nested source id.
        assert_eq!(channels[0].index, 1);
        assert_eq!(channels[0].name.as_deref(), Some("Entrance"));
        assert_eq!(channels[1].index, 2);
        assert_eq!(channels[1].name.as_deref(), Some("Register"));
    }

    #[test]
    fn parse_video_input_channels_reads_dvr_analog_inputs() {
        let xml = "\
<VideoInputChannelList>\
  <VideoInputChannel><id>1</id><resDesc>1080p</resDesc></VideoInputChannel>\
  <VideoInputChannel><id>2</id></VideoInputChannel>\
</VideoInputChannelList>";
        let channels = parse_video_input_channels(xml);
        assert_eq!(channels.len(), 2);
        assert_eq!(channels[0].index, 1);
        // Name falls back to the resolution descriptor, else None.
        assert_eq!(channels[0].name.as_deref(), Some("1080p"));
        assert_eq!(channels[1].index, 2);
        assert_eq!(channels[1].name, None);
    }

    fn channel(index: u16, name: &str) -> Channel {
        Channel {
            index,
            name: Some(name.to_owned()),
            enabled: None,
            ptz_support: PtzSupport::Unknown,
            recording: None,
        }
    }

    #[test]
    fn merge_channels_prefers_primary_on_index_collision() {
        // Index 1 appears in both lists; the InputProxy (primary) name — the
        // operator-assigned camera title — must win over VideoInput's bare
        // descriptor.
        let primary = vec![channel(1, "Front Door")];
        let secondary = vec![channel(1, "720p")];
        let merged = merge_channels(primary, secondary);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].index, 1);
        assert_eq!(merged[0].name.as_deref(), Some("Front Door"));
    }

    #[test]
    fn merge_channels_unions_disjoint_indices_sorted_ascending() {
        // A hybrid NVR: IP camera on index 3, analog input on index 1 — both
        // must survive the merge, in ascending index order regardless of
        // input order.
        let primary = vec![channel(3, "IP Cam")];
        let secondary = vec![channel(1, "Analog In")];
        let merged = merge_channels(primary, secondary);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].index, 1);
        assert_eq!(merged[0].name.as_deref(), Some("Analog In"));
        assert_eq!(merged[1].index, 3);
        assert_eq!(merged[1].name.as_deref(), Some("IP Cam"));
    }

    #[test]
    fn xml_tag_reads_plain_and_attributed_tags() {
        assert_eq!(xml_tag(SAMPLE_DEVICE_INFO, "model"), Some("DS-N316/2(D)"));
        assert_eq!(
            xml_tag(SAMPLE_DEVICE_INFO, "firmwareVersion"),
            Some("V4.75.000")
        );
        // The open tag carries attributes; the value is still extracted.
        assert_eq!(xml_tag("<root a=\"1\">value</root>", "root"), Some("value"));
        // Surrounding whitespace is trimmed.
        assert_eq!(xml_tag("<x>  spaced  </x>", "x"), Some("spaced"));
    }

    #[test]
    fn xml_tag_returns_none_for_missing_empty_and_self_closing() {
        assert_eq!(xml_tag(SAMPLE_DEVICE_INFO, "absent"), None);
        assert_eq!(xml_tag("<x></x>", "x"), None);
        assert_eq!(xml_tag("<x/>", "x"), None);
        assert_eq!(xml_tag("<x a=\"1\"/>", "x"), None);
    }

    #[test]
    fn xml_tag_does_not_match_a_longer_tag_name() {
        // Searching `id` must not match `deviceID`/`identity`.
        assert_eq!(xml_tag("<identity>no</identity>", "id"), None);
        assert_eq!(
            xml_tag("<identity>no</identity><id>7</id>", "id"),
            Some("7")
        );
    }

    #[test]
    fn xml_blocks_splits_siblings_without_matching_the_container() {
        let blocks = xml_blocks(SAMPLE_USERS, "User");
        // `<UserList>` must not be counted as a `<User>` block.
        assert_eq!(blocks.len(), 2);
        assert!(blocks[0].contains("<userName>admin</userName>"));
        assert!(blocks[1].contains("<userName>it</userName>"));
    }

    #[test]
    fn xml_tag_extracts_from_within_a_user_block_despite_nested_id() {
        let block = xml_blocks(SAMPLE_USERS, "User")[0];
        // The block's own `<id>1</id>` must not shadow the fields we want.
        assert_eq!(xml_tag(block, "userName"), Some("admin"));
        assert_eq!(xml_tag(block, "userLevel"), Some("Administrator"));
        assert_eq!(xml_tag(block, "id"), Some("1"));
    }

    #[test]
    fn parse_device_info_maps_identity_fields() {
        let info = parse_device_info(SAMPLE_DEVICE_INFO);
        assert_eq!(info.model.as_deref(), Some("DS-N316/2(D)"));
        assert_eq!(info.firmware.as_deref(), Some("V4.75.000"));
        assert_eq!(info.serial.as_deref(), Some("DS-N316/2(D)1620ABCDEF"));
        assert_eq!(info.channels, None);
        assert_eq!(info.device_time, None);
    }

    #[test]
    fn parse_users_maps_each_block_and_its_level() {
        let users = parse_users(SAMPLE_USERS);
        assert_eq!(users.len(), 2);
        assert_eq!(users[0].name, "admin");
        assert_eq!(users[0].group.as_deref(), Some("Administrator"));
        assert!(users[0].authorities.is_empty());
        assert_eq!(users[1].name, "it");
        assert_eq!(users[1].group.as_deref(), Some("Operator"));
    }

    #[test]
    fn parse_disks_converts_mib_to_bytes_and_flags_status() {
        let disks = parse_disks(SAMPLE_STORAGE);
        // The zero-capacity third disk is dropped.
        assert_eq!(disks.len(), 2);

        assert_eq!(disks[0].name, "hdd1");
        assert_eq!(disks[0].total_bytes, 7_630_885 * MIB);
        // freeSpace 0 -> the whole disk is used.
        assert_eq!(disks[0].used_bytes, 7_630_885 * MIB);
        assert_eq!(disks[0].state, DiskState::Ok);
        assert!(!disks[0].is_error);

        // A non-"ok" status is flagged as an error.
        assert_eq!(disks[1].name, "hdd2");
        assert_eq!(disks[1].total_bytes, 3_815_442 * MIB);
        // freeSpace == capacity -> nothing used.
        assert_eq!(disks[1].used_bytes, 0);
        assert_eq!(disks[1].state, DiskState::Error);
        assert!(disks[1].is_error);
    }

    #[tokio::test]
    async fn get_info_maps_the_device_info_resource() {
        let driver = driver_with(&[(PATH_DEVICE_INFO, SAMPLE_DEVICE_INFO)]);
        let info = driver.get_info().await.expect("get_info");
        assert_eq!(info.model.as_deref(), Some("DS-N316/2(D)"));
        assert_eq!(info.serial.as_deref(), Some("DS-N316/2(D)1620ABCDEF"));
    }

    #[tokio::test]
    async fn list_users_maps_the_users_resource() {
        let driver = driver_with(&[(PATH_USERS, SAMPLE_USERS)]);
        let users = driver.list_users().await.expect("list_users");
        assert_eq!(users.len(), 2);
        assert_eq!(users[1].name, "it");
    }

    #[tokio::test]
    async fn disk_status_maps_the_storage_resource() {
        let driver = driver_with(&[(PATH_STORAGE, SAMPLE_STORAGE)]);
        let disks = driver.disk_status().await.expect("disk_status");
        assert_eq!(disks.len(), 2);
        assert_eq!(disks[0].total_bytes, 7_630_885 * MIB);
    }

    #[tokio::test]
    async fn recording_status_parses_content_search_matches() {
        // A single search page with two spans and no MORE → one POST, loop ends.
        let result = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
<CMSearchResult>\
<responseStatusStrg>OK</responseStatusStrg>\
<numOfMatches>2</numOfMatches>\
<matchList>\
<searchMatchItem><timeSpan>\
<startTime>2026-07-01T08:00:00Z</startTime><endTime>2026-07-01T09:00:00Z</endTime>\
</timeSpan></searchMatchItem>\
<searchMatchItem><timeSpan>\
<startTime>2026-07-01T09:00:00Z</startTime><endTime>2026-07-01T10:30:00Z</endTime>\
</timeSpan></searchMatchItem>\
</matchList></CMSearchResult>";
        let client =
            Arc::new(RecordingHttpClient::new(&[]).with_posts(vec![Ok(result.to_owned())]));
        let driver = HikvisionDriver::new(
            client.clone(),
            DeviceEndpoint::new("device.test", 80),
            "admin",
            Secret::new("adminpw"),
        );

        let status = driver.recording_status().await.expect("recording status");
        assert_eq!(status.spans.len(), 2);
        assert_eq!(
            status.oldest,
            Some(parse_isapi_time("2026-07-01T08:00:00Z").unwrap())
        );
        assert_eq!(
            status.newest,
            Some(parse_isapi_time("2026-07-01T10:30:00Z").unwrap())
        );
        assert!(!status.active);
        // Exactly one search POST was issued to the content-search resource.
        let posts = client
            .calls()
            .into_iter()
            .filter(|c| matches!(c, Call::Post { path, .. } if path == PATH_CONTENT_SEARCH))
            .count();
        assert_eq!(posts, 1);
        assert!(driver.capabilities().recording);
    }

    /// `search_recordings` must key the ISAPI track id to the requested
    /// channel (`camera*100 + 1`, the same encoding `snapshot` uses) rather
    /// than the device-wide `DEFAULT_TRACK_ID`, and attribute every returned
    /// span back to that channel.
    #[tokio::test]
    async fn search_recordings_uses_the_requested_channels_track_id() {
        let result = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
<CMSearchResult><responseStatusStrg>OK</responseStatusStrg>\
<numOfMatches>1</numOfMatches><matchList>\
<searchMatchItem><timeSpan>\
<startTime>2026-07-01T08:00:00Z</startTime><endTime>2026-07-01T09:00:00Z</endTime>\
</timeSpan></searchMatchItem></matchList></CMSearchResult>";
        let client =
            Arc::new(RecordingHttpClient::new(&[]).with_posts(vec![Ok(result.to_owned())]));
        let driver = HikvisionDriver::new(
            client.clone(),
            DeviceEndpoint::new("device.test", 80),
            "admin",
            Secret::new("adminpw"),
        );

        let request = ExportRequest {
            channel: 4,
            start: "2026-07-01T00:00:00Z".parse().expect("start"),
            end: "2026-07-02T00:00:00Z".parse().expect("end"),
        };
        let spans = driver.search_recordings(&request).await.expect("search");

        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].channel, Some(4));

        let body = client
            .calls()
            .into_iter()
            .find_map(|call| match call {
                Call::Post { path, body, .. } if path == PATH_CONTENT_SEARCH => Some(body),
                _ => None,
            })
            .expect("a content-search POST was issued");
        // camera 4 -> track id 401, not the device-wide 101.
        assert_eq!(xml_tag(&body, "trackID"), Some("401"));
        assert!(driver.capabilities().recording_search);
    }

    /// A search reply carrying one matching segment with a `mediaSegmentDescriptor`.
    fn search_result_with_segment(start: &str, end: &str, playback_uri: &str) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
<CMSearchResult><responseStatusStrg>OK</responseStatusStrg>\
<numOfMatches>1</numOfMatches><matchList>\
<searchMatchItem><timeSpan><startTime>{start}</startTime><endTime>{end}</endTime></timeSpan>\
<mediaSegmentDescriptor><contentType>video</contentType><codecType>H.264-BP</codecType>\
<playbackURI>{playback_uri}</playbackURI></mediaSegmentDescriptor>\
</searchMatchItem></matchList></CMSearchResult>",
        )
    }

    /// `export_download` resolves a segment CONTAINED in the requested window
    /// and downloads it via `ContentMgmt/download` using the search's exact
    /// URI — live-verified on a DS-N316/2(D). The handle must be XML-escaped
    /// exactly once (never double-escaped), or the device rejects it.
    #[tokio::test]
    async fn export_download_resolves_and_downloads_the_matching_segment() {
        let playback_uri = "rtsp://device.test/Streaming/tracks/101/?starttime=20260701T080000Z&endtime=20260701T090000Z&name=x&size=100";
        let search = search_result_with_segment(
            "2026-07-01T08:00:00Z",
            "2026-07-01T09:00:00Z",
            &xml_escape(playback_uri),
        );
        let bytes = b"IMKH-fake-container-bytes".to_vec();
        let client = Arc::new(
            RecordingHttpClient::new(&[])
                .with_posts(vec![Ok(search)])
                .with_download_post(Ok(bytes.clone())),
        );
        let driver = HikvisionDriver::new(
            client.clone(),
            DeviceEndpoint::new("device.test", 80),
            "admin",
            Secret::new("adminpw"),
        );

        // The window CONTAINS the 08:00–09:00 segment: the whole file is what
        // the caller asked for, so the fast path is the right answer.
        let request = ExportRequest {
            channel: 1,
            start: "2026-07-01T07:00:00Z".parse().expect("start"),
            end: "2026-07-01T10:00:00Z".parse().expect("end"),
        };
        let download = driver
            .export_download(&request)
            .await
            .expect("export_download")
            .expect("a segment covers the window");

        use futures_util::StreamExt as _;
        let collected: Vec<u8> = download
            .bytes
            .map(|chunk| chunk.expect("chunk"))
            .collect::<Vec<_>>()
            .await
            .concat();
        assert_eq!(collected, bytes);

        let calls = client.calls();
        let (download_path, download_body, download_content_type) = calls
            .iter()
            .rev()
            .find_map(|call| match call {
                Call::Post {
                    path,
                    body,
                    content_type,
                } if path == PATH_CONTENT_DOWNLOAD => {
                    Some((path.clone(), body.clone(), content_type.clone()))
                }
                _ => None,
            })
            .expect("a download POST was issued");
        assert_eq!(download_path, PATH_CONTENT_DOWNLOAD);
        assert_eq!(download_content_type, CONTENT_TYPE_XML);
        // The search's exact URI: `name` is what selects the file, and the
        // device ignores the times on this endpoint anyway.
        assert!(download_body.contains(&xml_escape(playback_uri)));
        // Escaped exactly once — double-escaping would turn `&amp;` into
        // `&amp;amp;` and the device would reject the handle.
        assert!(!download_body.contains("&amp;amp;"));
    }

    /// A window with no overlapping segment must answer `None`, the same "no
    /// footage" outcome DVRIP and Dahua use — never an error.
    #[tokio::test]
    async fn export_download_returns_none_when_no_segment_overlaps_the_window() {
        let empty = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
<CMSearchResult><responseStatusStrg>OK</responseStatusStrg>\
<numOfMatches>0</numOfMatches><matchList></matchList></CMSearchResult>";
        let client = Arc::new(RecordingHttpClient::new(&[]).with_posts(vec![Ok(empty.to_owned())]));
        let driver = HikvisionDriver::new(
            client,
            DeviceEndpoint::new("device.test", 80),
            "admin",
            Secret::new("adminpw"),
        );

        let request = ExportRequest {
            channel: 1,
            start: "2026-07-01T08:00:00Z".parse().expect("start"),
            end: "2026-07-01T09:00:00Z".parse().expect("end"),
        };
        let download = driver
            .export_download(&request)
            .await
            .expect("export_download");

        assert!(download.is_none());
    }

    /// A returned segment that does not actually overlap the requested window
    /// (a device oddity, or a search reply from a wider query) must not be
    /// downloaded — only a genuinely overlapping match is a real answer.
    #[tokio::test]
    async fn export_download_ignores_a_non_overlapping_segment() {
        let search = search_result_with_segment(
            "2026-07-01T01:00:00Z",
            "2026-07-01T02:00:00Z",
            "rtsp://device.test/x",
        );
        let client = Arc::new(RecordingHttpClient::new(&[]).with_posts(vec![Ok(search)]));
        let driver = HikvisionDriver::new(
            client,
            DeviceEndpoint::new("device.test", 80),
            "admin",
            Secret::new("adminpw"),
        );

        let request = ExportRequest {
            channel: 1,
            start: "2026-07-01T08:00:00Z".parse().expect("start"),
            end: "2026-07-01T09:00:00Z".parse().expect("end"),
        };
        let download = driver
            .export_download(&request)
            .await
            .expect("export_download");

        assert!(download.is_none());
    }

    /// The device validates `searchID` as a UUID and answers `badXmlContent`
    /// (surfacing as HTTP 400) for anything else — live-verified on a
    /// DS-N316/2(D). A fixed literal made every recording read fail.
    #[tokio::test]
    async fn recording_search_sends_a_uuid_search_id() {
        let result = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
<CMSearchResult><responseStatusStrg>OK</responseStatusStrg>\
<numOfMatches>0</numOfMatches><matchList></matchList></CMSearchResult>";
        let client =
            Arc::new(RecordingHttpClient::new(&[]).with_posts(vec![Ok(result.to_owned())]));
        let driver = HikvisionDriver::new(
            client.clone(),
            DeviceEndpoint::new("device.test", 80),
            "admin",
            Secret::new("adminpw"),
        );

        driver.recording_status().await.expect("recording status");

        let body = client
            .calls()
            .into_iter()
            .find_map(|call| match call {
                Call::Post { path, body, .. } if path == PATH_CONTENT_SEARCH => Some(body),
                _ => None,
            })
            .expect("a content-search POST was issued");
        let search_id = xml_tag(&body, "searchID").expect("body carries a searchID");
        assert!(
            uuid::Uuid::parse_str(search_id.trim()).is_ok(),
            "searchID must be a UUID, got {search_id:?}"
        );
    }

    /// Two searches must not share one device-side search id, or concurrent
    /// reads collide on the device's own search state.
    #[tokio::test]
    async fn each_recording_search_uses_a_fresh_search_id() {
        let result = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
<CMSearchResult><responseStatusStrg>OK</responseStatusStrg>\
<numOfMatches>0</numOfMatches><matchList></matchList></CMSearchResult>";
        let client = Arc::new(
            RecordingHttpClient::new(&[])
                .with_posts(vec![Ok(result.to_owned()), Ok(result.to_owned())]),
        );
        let driver = HikvisionDriver::new(
            client.clone(),
            DeviceEndpoint::new("device.test", 80),
            "admin",
            Secret::new("adminpw"),
        );

        driver.recording_status().await.expect("first search");
        driver.recording_status().await.expect("second search");

        let ids: Vec<String> = client
            .calls()
            .into_iter()
            .filter_map(|call| match call {
                Call::Post { path, body, .. } if path == PATH_CONTENT_SEARCH => {
                    xml_tag(&body, "searchID").map(|id| id.trim().to_owned())
                }
                _ => None,
            })
            .collect();
        assert_eq!(ids.len(), 2);
        assert_ne!(ids[0], ids[1], "each search needs its own id");
    }

    #[tokio::test]
    async fn recording_status_stops_at_the_page_cap_even_if_the_device_never_stops_signalling_more()
    {
        // A full page that always claims MORE remains, forcing the loop to hit
        // its hard cap rather than paging (and growing `spans`) forever.
        let always_more = "<CMSearchResult><responseStatusStrg>MORE</responseStatusStrg>\
<numOfMatches>100</numOfMatches><matchList></matchList></CMSearchResult>";
        let responses: Vec<Result<String, DriverError>> = (0..MAX_SEARCH_PAGES + 5)
            .map(|_| Ok(always_more.to_owned()))
            .collect();
        let (client, driver) = driver_from(RecordingHttpClient::new(&[]).with_posts(responses));

        let status = driver.recording_status().await.expect("recording status");
        assert!(status.spans.is_empty());
        let posts = client
            .calls()
            .into_iter()
            .filter(|c| matches!(c, Call::Post { path, .. } if path == PATH_CONTENT_SEARCH))
            .count();
        // Exactly the page cap, not the full canned queue (MAX_SEARCH_PAGES + 5).
        assert_eq!(posts, MAX_SEARCH_PAGES as usize);
    }

    #[test]
    fn parse_content_search_drops_inverted_and_incomplete_spans() {
        let xml = "<CMSearchResult><numOfMatches>2</numOfMatches><matchList>\
<searchMatchItem><timeSpan><startTime>2026-07-01T10:00:00Z</startTime>\
<endTime>2026-07-01T09:00:00Z</endTime></timeSpan></searchMatchItem>\
<searchMatchItem><timeSpan><startTime>2026-07-01T11:00:00Z</startTime></timeSpan></searchMatchItem>\
</matchList></CMSearchResult>";
        let (matched, spans) = parse_content_search(xml);
        assert_eq!(matched, 2);
        assert!(spans.is_empty());
    }

    #[tokio::test]
    async fn snapshot_requests_the_camera_still_and_returns_the_jpeg_bytes() {
        // A minimal but well-formed JPEG (SOI … EOI).
        let jpeg: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46, 0xFF, 0xD9];
        // Camera 2 -> stream id 201.
        let path = "/ISAPI/Streaming/channels/201/picture";
        let driver = HikvisionDriver::new(
            Arc::new(FakeHttpClient::new(&[]).with_bytes(&[(path, jpeg)])),
            DeviceEndpoint::new("device.test", 80),
            "admin",
            Secret::new("secretpw"),
        );

        let snap = driver.snapshot(Some(2)).await.expect("snapshot");
        let SnapshotRef::Bytes(bytes) = &snap else {
            panic!("expected inline bytes, got {snap:?}");
        };
        assert_eq!(bytes.as_slice(), jpeg);
        // Debug reports the size, never the driver password.
        assert!(!format!("{snap:?}").contains("secretpw"));
    }

    #[tokio::test]
    async fn snapshot_defaults_to_camera_one_stream_101() {
        let jpeg: &[u8] = &[0xFF, 0xD8, 0xFF, 0xD9];
        let driver = HikvisionDriver::new(
            Arc::new(
                FakeHttpClient::new(&[])
                    .with_bytes(&[("/ISAPI/Streaming/channels/101/picture", jpeg)]),
            ),
            DeviceEndpoint::new("device.test", 80),
            "admin",
            Secret::new("secretpw"),
        );

        let snap = driver.snapshot(None).await.expect("snapshot");
        assert!(matches!(snap, SnapshotRef::Bytes(bytes) if bytes == jpeg));
    }

    #[tokio::test]
    async fn snapshot_rejects_a_non_jpeg_body() {
        let html: &[u8] = b"<html>401 Unauthorized</html>";
        let driver = HikvisionDriver::new(
            Arc::new(
                FakeHttpClient::new(&[])
                    .with_bytes(&[("/ISAPI/Streaming/channels/101/picture", html)]),
            ),
            DeviceEndpoint::new("device.test", 80),
            "admin",
            Secret::new("secretpw"),
        );

        let error = driver
            .snapshot(None)
            .await
            .expect_err("a non-JPEG body must be rejected");
        assert!(matches!(error, DriverError::Device(_)));
    }

    #[test]
    fn irreversible_password_matches_the_spelled_out_iterated_sha256() {
        let username = "guard1";
        let salt = "0123456789abcdef";
        let password = "s3cr3t-pw";
        let iterations = 3;

        // Independently spell out the documented algorithm: SHA-256 of
        // userName+salt+password, then hash the hex digest (iterations-1) more
        // times.
        let mut expected = hex::encode(Sha256::digest(
            format!("{username}{salt}{password}").as_bytes(),
        ));
        for _ in 1..iterations {
            expected = hex::encode(Sha256::digest(expected.as_bytes()));
        }

        let actual = irreversible_password(username, salt, password, iterations);
        assert_eq!(actual, expected);
        // A SHA-256 hex digest is 64 lowercase hex chars, and never the plaintext.
        assert_eq!(actual.len(), 64);
        assert!(actual.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(actual, password);

        // A single iteration is just SHA-256 of the concatenation; a zero/absent
        // count clamps to that same single hash.
        let single = hex::encode(Sha256::digest(
            format!("{username}{salt}{password}").as_bytes(),
        ));
        assert_eq!(irreversible_password(username, salt, password, 1), single);
        assert_eq!(irreversible_password(username, salt, password, 0), single);

        // NOTE: this is a self-consistent vector. No public Hikvision test vector
        // exists, so the exact wire format still needs live-device validation
        // before writes are enabled.
    }

    #[test]
    fn hik_user_level_maps_known_groups_case_insensitively() {
        assert_eq!(
            hik_user_level(Some("Administrator")).unwrap(),
            "Administrator"
        );
        assert_eq!(
            hik_user_level(Some("administrator")).unwrap(),
            "Administrator"
        );
        assert_eq!(hik_user_level(Some("Operator")).unwrap(), "Operator");
        assert_eq!(hik_user_level(Some("operator")).unwrap(), "Operator");
        assert_eq!(hik_user_level(None).unwrap(), "Operator");
    }

    #[test]
    fn hik_user_level_rejects_an_unrecognized_group_instead_of_under_granting() {
        // A typo/unknown literal must never silently fall back to Operator.
        let error = hik_user_level(Some("guests")).expect_err("unknown group is rejected");
        assert!(matches!(error, DriverError::Device(_)));
    }

    #[test]
    fn parse_security_cap_clamps_an_outrageous_device_reported_iteration_count() {
        let xml = "<SecurityCap><isIrreversible>true</isIrreversible><salt>x</salt>\
<iterateNumber>4294967295</iterateNumber></SecurityCap>";
        let caps = parse_security_cap(xml);
        assert_eq!(caps.iterations, MAX_ITERATIONS);
    }

    fn new_user(name: &str, password: &str, group: Option<&str>) -> NewUser {
        NewUser {
            name: name.to_owned(),
            password: Secret::new(password),
            group: group.map(str::to_owned),
            authorities: Vec::new(),
        }
    }

    #[tokio::test]
    async fn create_user_irreversible_posts_the_salted_hash_not_the_plaintext() {
        let cap_path = format!("{PATH_SECURITY_CAP}?username=guard1");
        let (client, driver) = driver_from(
            RecordingHttpClient::new(&[(&cap_path, SAMPLE_CAP_IRREVERSIBLE)])
                .with_posts(vec![Ok(RESPONSE_OK.to_owned())]),
        );

        let outcome = driver
            .create_users(&[new_user("guard1", "s3cr3t-pw", Some("Operator"))])
            .await
            .expect("create_users");
        assert_eq!(outcome.succeeded, ["guard1"]);
        assert!(outcome.failed.is_empty());

        let calls = client.calls();
        // Capabilities are read before the write.
        assert!(
            calls
                .iter()
                .any(|call| matches!(call, Call::Get(path) if path == &cap_path))
        );

        let (path, body, content_type) = first_post(&calls);
        assert_eq!(path, PATH_USERS);
        assert_eq!(content_type, CONTENT_TYPE_XML);
        // The salted hash (iterateNumber=2) is on the wire; the plaintext is not.
        let expected_hash = irreversible_password("guard1", "0123456789abcdef", "s3cr3t-pw", 2);
        assert!(body.contains(&format!("<password>{expected_hash}</password>")));
        assert!(!body.contains("s3cr3t-pw"));
        // Body shape.
        assert!(body.contains("<id>0</id>"));
        assert!(body.contains("<userName>guard1</userName>"));
        assert!(body.contains("<userLevel>Operator</userLevel>"));
    }

    #[tokio::test]
    async fn create_user_sends_plaintext_when_firmware_is_not_irreversible() {
        let cap_path = format!("{PATH_SECURITY_CAP}?username=it");
        let (client, driver) = driver_from(
            RecordingHttpClient::new(&[(&cap_path, SAMPLE_CAP_PLAINTEXT)])
                .with_posts(vec![Ok(RESPONSE_OK.to_owned())]),
        );

        let outcome = driver
            .create_users(&[new_user("it", "plainpw", None)])
            .await
            .expect("create_users");
        assert_eq!(outcome.succeeded, ["it"]);

        let (_, body, _) = first_post(&client.calls());
        assert!(body.contains("<password>plainpw</password>"));
        // A missing group defaults to the least-privileged Operator.
        assert!(body.contains("<userLevel>Operator</userLevel>"));
    }

    #[tokio::test]
    async fn create_user_xml_escapes_the_body_and_percent_encodes_the_query() {
        // A login carrying an XML/URL metacharacter.
        let cap_path = format!("{PATH_SECURITY_CAP}?username=a%26b");
        let (client, driver) = driver_from(
            RecordingHttpClient::new(&[(&cap_path, SAMPLE_CAP_PLAINTEXT)])
                .with_posts(vec![Ok(RESPONSE_OK.to_owned())]),
        );

        let outcome = driver
            .create_users(&[new_user("a&b", "p<w>&", None)])
            .await
            .expect("create_users");
        assert_eq!(outcome.succeeded, ["a&b"]);

        let calls = client.calls();
        // The capabilities query percent-encoded the name (`&` -> `%26`).
        assert!(
            calls
                .iter()
                .any(|call| matches!(call, Call::Get(path) if path == &cap_path))
        );
        let (_, body, _) = first_post(&calls);
        assert!(body.contains("<userName>a&amp;b</userName>"));
        assert!(body.contains("<password>p&lt;w&gt;&amp;</password>"));
    }

    #[tokio::test]
    async fn create_user_treats_already_exists_as_idempotent_success() {
        let cap_path = format!("{PATH_SECURITY_CAP}?username=admin");
        let (_client, driver) = driver_from(
            RecordingHttpClient::new(&[(&cap_path, SAMPLE_CAP_PLAINTEXT)])
                .with_posts(vec![Ok(RESPONSE_ALREADY_EXISTS.to_owned())]),
        );

        let outcome = driver
            .create_users(&[new_user("admin", "pw", Some("Administrator"))])
            .await
            .expect("create_users");
        assert_eq!(outcome.succeeded, ["admin"]);
        assert!(outcome.failed.is_empty());
    }

    #[tokio::test]
    async fn create_users_records_partial_success() {
        let cap_ok = format!("{PATH_SECURITY_CAP}?username=okuser");
        let cap_bad = format!("{PATH_SECURITY_CAP}?username=baduser");
        let (_client, driver) = driver_from(
            RecordingHttpClient::new(&[
                (&cap_ok, SAMPLE_CAP_PLAINTEXT),
                (&cap_bad, SAMPLE_CAP_PLAINTEXT),
            ])
            .with_posts(vec![
                Ok(RESPONSE_OK.to_owned()),
                Ok(RESPONSE_RISK_PASSWORD.to_owned()),
            ]),
        );

        let outcome = driver
            .create_users(&[
                new_user("okuser", "pw1", None),
                new_user("baduser", "weak", None),
            ])
            .await
            .expect("create_users");
        assert_eq!(outcome.succeeded, ["okuser"]);
        assert_eq!(outcome.failed.len(), 1);
        assert_eq!(outcome.failed[0].0, "baduser");
        assert!(matches!(outcome.failed[0].1, DriverError::Device(_)));
    }

    #[tokio::test]
    async fn create_users_aborts_the_batch_on_a_transport_failure() {
        let cap_path = format!("{PATH_SECURITY_CAP}?username=guard1");
        let (_client, driver) = driver_from(
            RecordingHttpClient::new(&[(&cap_path, SAMPLE_CAP_PLAINTEXT)])
                .with_posts(vec![Err(DriverError::Transport("link down".to_owned()))]),
        );

        let result = driver.create_users(&[new_user("guard1", "pw", None)]).await;
        assert!(matches!(result, Err(DriverError::Transport(_))));
    }

    #[tokio::test]
    async fn bulk_user_writes_keep_committed_prefix_after_transport_abort() {
        let names = ["admin".to_owned(), "it".to_owned(), "third".to_owned()];
        let listing = SAMPLE_USERS.replace("</UserList>", "<User><id>3</id><userName>third</userName><userLevel>Operator</userLevel></User></UserList>");
        for operation in ["create", "delete", "update"] {
            let cap_admin = format!("{PATH_SECURITY_CAP}?username=admin");
            let cap_it = format!("{PATH_SECURITY_CAP}?username=it");
            let (client, driver) = driver_from(
                RecordingHttpClient::new(&[
                    (PATH_USERS, &listing),
                    (&cap_admin, SAMPLE_CAP_PLAINTEXT),
                    (&cap_it, SAMPLE_CAP_PLAINTEXT),
                ])
                .with_posts(vec![
                    Ok(RESPONSE_OK.to_owned()),
                    Err(DriverError::Transport("link down".to_owned())),
                ])
                .with_deletes(vec![
                    Ok(RESPONSE_OK.to_owned()),
                    Err(DriverError::Transport("link down".to_owned())),
                ])
                .with_puts(vec![
                    Ok(RESPONSE_OK.to_owned()),
                    Err(DriverError::Transport("link down".to_owned())),
                ]),
            );
            let outcome = match operation {
                "create" => {
                    driver
                        .create_users(&names.clone().map(|name| new_user(&name, "pw", None)))
                        .await
                }
                "delete" => driver.delete_users(&names).await,
                "update" => {
                    driver
                        .update_users(&names.clone().map(|name| UserUpdate {
                            name,
                            password: None,
                            group: Some("Operator".to_owned()),
                            authorities: None,
                        }))
                        .await
                }
                _ => Err(DriverError::Unsupported),
            }
            .expect("confirmed write survives fatal link error");
            assert_eq!(outcome.succeeded, ["admin"], "{operation}");
            assert_eq!(outcome.failed.len(), 2, "{operation}");
            assert_eq!(outcome.failed[0].0, "it");
            assert!(matches!(outcome.failed[0].1, DriverError::Transport(_)));
            assert_eq!(outcome.failed[1].0, "third");
            assert!(
                matches!(&outcome.failed[1].1, DriverError::Unreachable(reason) if reason == "not attempted after device link failure")
            );
            let writes = client
                .calls()
                .into_iter()
                .filter(|call| match operation {
                    "create" => matches!(call, Call::Post { .. }),
                    "delete" => matches!(call, Call::Delete(_)),
                    _ => matches!(call, Call::Put { .. }),
                })
                .count();
            assert_eq!(writes, 2, "{operation}: tail must not be attempted");
        }
    }

    #[tokio::test]
    async fn bulk_user_writes_without_acknowledgement_keep_top_level_transport_error() {
        for operation in ["create", "delete", "update"] {
            let cap_admin = format!("{PATH_SECURITY_CAP}?username=admin");
            let (client, driver) = driver_from(
                RecordingHttpClient::new(&[
                    (PATH_USERS, SAMPLE_USERS),
                    (&cap_admin, SAMPLE_CAP_PLAINTEXT),
                ])
                .with_posts(vec![Err(DriverError::Transport("link down".to_owned()))])
                .with_deletes(vec![Err(DriverError::Transport("link down".to_owned()))])
                .with_puts(vec![Err(DriverError::Transport("link down".to_owned()))]),
            );
            let names = ["admin".to_owned(), "it".to_owned()];
            let result = match operation {
                "create" => {
                    driver
                        .create_users(&names.map(|name| new_user(&name, "pw", None)))
                        .await
                }
                "delete" => driver.delete_users(&names).await,
                "update" => {
                    driver
                        .update_users(&names.map(|name| UserUpdate {
                            name,
                            password: None,
                            group: Some("Operator".to_owned()),
                            authorities: None,
                        }))
                        .await
                }
                _ => Err(DriverError::Unsupported),
            };
            assert!(
                matches!(result, Err(DriverError::Transport(_))),
                "{operation}"
            );
            let writes = client
                .calls()
                .iter()
                .filter(|call| match operation {
                    "create" => matches!(call, Call::Post { .. }),
                    "delete" => matches!(call, Call::Delete(_)),
                    _ => matches!(call, Call::Put { .. }),
                })
                .count();
            assert_eq!(writes, 1, "{operation}");
        }
    }

    #[tokio::test]
    async fn create_users_with_no_input_makes_no_calls() {
        let (client, driver) = driver_from(RecordingHttpClient::new(&[]));
        let outcome = driver.create_users(&[]).await.expect("create_users");
        assert!(outcome.succeeded.is_empty());
        assert!(outcome.failed.is_empty());
        assert!(client.calls().is_empty());
    }

    #[tokio::test]
    async fn create_users_rejects_a_non_empty_authorities_list_instead_of_dropping_it() {
        // Hikvision's AuthorityList write body is not implemented; a request
        // that carries authorities must fail loudly, never silently succeed
        // without ever transmitting them.
        let (client, driver) = driver_from(RecordingHttpClient::new(&[]));
        let user = NewUser {
            name: "guard1".to_owned(),
            password: Secret::new("pw"),
            group: Some("Operator".to_owned()),
            authorities: vec!["Monitor_01".to_owned()],
        };
        let outcome = driver
            .create_users(std::slice::from_ref(&user))
            .await
            .expect("create_users");
        assert!(outcome.succeeded.is_empty());
        assert_eq!(outcome.failed.len(), 1);
        assert_eq!(outcome.failed[0].0, "guard1");
        assert!(matches!(outcome.failed[0].1, DriverError::Device(_)));
        // Rejected before any network call — no capabilities read, no POST.
        assert!(client.calls().is_empty());
    }

    #[tokio::test]
    async fn delete_user_resolves_the_id_then_deletes() {
        let (client, driver) = driver_from(
            RecordingHttpClient::new(&[(PATH_USERS, SAMPLE_USERS)])
                .with_deletes(vec![Ok(RESPONSE_OK.to_owned())]),
        );

        let outcome = driver
            .delete_users(&["it".to_owned()])
            .await
            .expect("delete_users");
        assert_eq!(outcome.succeeded, ["it"]);
        assert!(outcome.failed.is_empty());

        let calls = client.calls();
        // The listing is fetched first, then the resolved id (2) is DELETEd.
        assert_eq!(calls[0], Call::Get(PATH_USERS.to_owned()));
        assert!(
            calls.iter().any(
                |call| matches!(call, Call::Delete(path) if path == "/ISAPI/Security/users/2")
            )
        );
    }

    #[tokio::test]
    async fn delete_user_treats_a_missing_user_as_success_without_a_delete() {
        let (client, driver) = driver_from(RecordingHttpClient::new(&[(PATH_USERS, SAMPLE_USERS)]));

        let outcome = driver
            .delete_users(&["ghost".to_owned()])
            .await
            .expect("delete_users");
        assert_eq!(outcome.succeeded, ["ghost"]);
        assert!(outcome.failed.is_empty());
        // No DELETE was issued for an absent user.
        assert!(
            !client
                .calls()
                .iter()
                .any(|call| matches!(call, Call::Delete(_)))
        );
    }

    #[tokio::test]
    async fn delete_users_records_partial_success() {
        // admin (id 1) is rejected device-side; it (id 2) succeeds.
        let (_client, driver) = driver_from(
            RecordingHttpClient::new(&[(PATH_USERS, SAMPLE_USERS)]).with_deletes(vec![
                Err(DriverError::Device("account locked".to_owned())),
                Ok(RESPONSE_OK.to_owned()),
            ]),
        );

        let outcome = driver
            .delete_users(&["admin".to_owned(), "it".to_owned()])
            .await
            .expect("delete_users");
        assert_eq!(outcome.succeeded, ["it"]);
        assert_eq!(outcome.failed.len(), 1);
        assert_eq!(outcome.failed[0].0, "admin");
        assert!(matches!(outcome.failed[0].1, DriverError::Device(_)));
    }

    #[tokio::test]
    async fn delete_users_errors_when_the_user_listing_cannot_be_read() {
        // No canned GET for the users path -> the single listing read fails, so
        // the whole batch is a top-level Err (nothing could be deleted).
        let (_client, driver) = driver_from(RecordingHttpClient::new(&[]));
        let result = driver.delete_users(&["it".to_owned()]).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn delete_users_with_no_input_makes_no_calls() {
        let (client, driver) = driver_from(RecordingHttpClient::new(&[]));
        let outcome = driver.delete_users(&[]).await.expect("delete_users");
        assert!(outcome.succeeded.is_empty());
        assert!(outcome.failed.is_empty());
        assert!(client.calls().is_empty());
    }

    #[tokio::test]
    async fn delete_user_rejects_a_non_numeric_device_reported_id_as_a_per_item_failure() {
        let (client, driver) = driver_from(RecordingHttpClient::new(&[(
            PATH_USERS,
            SAMPLE_USERS_BAD_ID,
        )]));

        let outcome = driver
            .delete_users(&["it".to_owned()])
            .await
            .expect("delete_users");
        assert!(outcome.succeeded.is_empty());
        assert_eq!(outcome.failed.len(), 1);
        assert_eq!(outcome.failed[0].0, "it");
        assert!(matches!(outcome.failed[0].1, DriverError::Protocol(_)));
        // The malformed id must never reach a DELETE call.
        assert!(
            !client
                .calls()
                .iter()
                .any(|call| matches!(call, Call::Delete(_)))
        );
    }

    #[tokio::test]
    async fn list_channels_returns_the_input_proxy_list_when_present() {
        let xml = "\
<InputProxyChannelList><InputProxyChannel><id>1</id><name>Entrance</name>\
</InputProxyChannel></InputProxyChannelList>";
        let driver = driver_with(&[(PATH_INPUT_PROXY_CHANNELS, xml)]);
        let channels = driver.list_channels().await.expect("list_channels");
        assert_eq!(channels.len(), 1);
        assert_eq!(channels[0].name.as_deref(), Some("Entrance"));
    }

    #[tokio::test]
    async fn list_channels_returns_empty_for_a_valid_but_empty_input_proxy_list() {
        // A successful (2xx) InputProxy response with zero channels is a
        // legitimately empty NVR, not "wrong endpoint" — an empty primary list
        // is never itself a signal to fall back or error. Both endpoints are
        // still probed (unless InputProxy was link-fatal — see
        // `is_link_fatal`), so the VideoInput endpoint is queried too, but it
        // has nothing to offer here either, so the merged result stays empty.
        let xml = "<InputProxyChannelList></InputProxyChannelList>";
        let (client, driver) = driver_from(RecordingHttpClient::new(&[(
            PATH_INPUT_PROXY_CHANNELS,
            xml,
        )]));
        let channels = driver.list_channels().await.expect("list_channels");
        assert!(channels.is_empty());
        // The analog endpoint was probed (both are queried unless the
        // InputProxy request was link-fatal), even though it had nothing to
        // offer here.
        assert!(
            client
                .calls()
                .iter()
                .any(|call| matches!(call, Call::Get(path) if path == PATH_VIDEO_INPUT_CHANNELS))
        );
    }

    #[tokio::test]
    async fn list_channels_falls_back_to_analog_when_input_proxy_request_fails() {
        // No canned response for InputProxy simulates the request failing
        // (e.g. 404 on an analog-only DVR, a non-link-fatal `Device` error),
        // so the analog endpoint is tried.
        let xml = "\
<VideoInputChannelList><VideoInputChannel><id>1</id><resDesc>1080p</resDesc>\
</VideoInputChannel></VideoInputChannelList>";
        let driver = driver_with(&[(PATH_VIDEO_INPUT_CHANNELS, xml)]);
        let channels = driver.list_channels().await.expect("list_channels");
        assert_eq!(channels.len(), 1);
        assert_eq!(channels[0].name.as_deref(), Some("1080p"));
    }

    #[tokio::test]
    async fn list_channels_merges_input_proxy_and_video_input_on_a_hybrid_nvr() {
        // A device offering both an IP camera (InputProxy) and an analog/PoE
        // input (VideoInput) at once must enumerate both, not just one —
        // see `merge_channels`.
        let proxy_xml = "\
<InputProxyChannelList><InputProxyChannel><id>1</id><name>Front Door</name>\
</InputProxyChannel></InputProxyChannelList>";
        let analog_xml = "\
<VideoInputChannelList><VideoInputChannel><id>2</id><resDesc>720p</resDesc>\
</VideoInputChannel></VideoInputChannelList>";
        let driver = driver_with(&[
            (PATH_INPUT_PROXY_CHANNELS, proxy_xml),
            (PATH_VIDEO_INPUT_CHANNELS, analog_xml),
        ]);
        let channels = driver.list_channels().await.expect("list_channels");
        assert_eq!(channels.len(), 2);
        assert_eq!(channels[0].index, 1);
        assert_eq!(channels[0].name.as_deref(), Some("Front Door"));
        assert_eq!(channels[1].index, 2);
        assert_eq!(channels[1].name.as_deref(), Some("720p"));
    }

    #[tokio::test]
    async fn list_channels_propagates_a_link_fatal_input_proxy_error_without_falling_through() {
        // Auth is link-fatal (`is_link_fatal`): the credential/link is broken,
        // so trying the VideoInput fallback would fail identically and would
        // only mask the real cause behind a generic "no channels" result.
        let (client, driver) = driver_from(
            RecordingHttpClient::new(&[(
                PATH_VIDEO_INPUT_CHANNELS,
                "<VideoInputChannelList></VideoInputChannelList>",
            )])
            .with_get_error(PATH_INPUT_PROXY_CHANNELS, DriverError::Auth),
        );
        let error = driver
            .list_channels()
            .await
            .expect_err("a link-fatal InputProxy error must propagate");
        assert!(matches!(error, DriverError::Auth));
        // No fallback request was made to the analog endpoint.
        assert!(
            !client
                .calls()
                .iter()
                .any(|call| matches!(call, Call::Get(path) if path == PATH_VIDEO_INPUT_CHANNELS))
        );
    }

    #[test]
    fn ptz_support_from_capabilities_reads_the_control_protocol_leaf() {
        let with_ptz = "\
<PTZChannel><portID>2</portID><controlProtocol>DOME_PLUS</controlProtocol>\
<channelID>1</channelID></PTZChannel>";
        assert_eq!(
            ptz_support_from_capabilities(with_ptz),
            PtzSupport::Supported
        );

        let without_ptz = "\
<PTZChannel><portID>0</portID><controlProtocol></controlProtocol>\
<channelID>101</channelID></PTZChannel>";
        assert_eq!(
            ptz_support_from_capabilities(without_ptz),
            PtzSupport::Unsupported
        );

        let malformed = "<Unexpected>not a PTZChannel document</Unexpected>";
        assert_eq!(
            ptz_support_from_capabilities(malformed),
            PtzSupport::Unknown
        );
    }

    #[tokio::test]
    async fn list_channels_attaches_real_ptz_support_per_channel() {
        // Mirrors a live TC-R3120 hybrid NVR: analog channel 1 has a PTZ head
        // wired to it, IP proxy channel 101 does not.
        let proxy_xml = "\
<InputProxyChannelList><InputProxyChannel><id>101</id><name>IP Cam</name>\
</InputProxyChannel></InputProxyChannelList>";
        let analog_xml = "\
<VideoInputChannelList><VideoInputChannel><id>1</id><resDesc>Dome</resDesc>\
</VideoInputChannel></VideoInputChannelList>";
        let with_ptz = "\
<PTZChannel><controlProtocol>DOME_PLUS</controlProtocol><channelID>1</channelID></PTZChannel>";
        let without_ptz = "<PTZChannel><controlProtocol></controlProtocol><channelID>101</channelID></PTZChannel>";
        let driver = driver_with(&[
            (PATH_INPUT_PROXY_CHANNELS, proxy_xml),
            (PATH_VIDEO_INPUT_CHANNELS, analog_xml),
            (&format!("{PATH_PTZ_CAPABILITIES}/1/capabilities"), with_ptz),
            (
                &format!("{PATH_PTZ_CAPABILITIES}/101/capabilities"),
                without_ptz,
            ),
        ]);
        let channels = driver.list_channels().await.expect("list_channels");
        assert_eq!(channels.len(), 2);
        let by_index = |index: u16| channels.iter().find(|c| c.index == index).expect("channel");
        assert_eq!(by_index(1).ptz_support, PtzSupport::Supported);
        assert_eq!(by_index(101).ptz_support, PtzSupport::Unsupported);
    }

    #[tokio::test]
    async fn list_channels_keeps_ptz_support_unknown_when_the_capability_read_fails() {
        let xml = "\
<VideoInputChannelList><VideoInputChannel><id>1</id><resDesc>Dome</resDesc>\
</VideoInputChannel></VideoInputChannelList>";
        // No canned PTZCtrl capabilities response -> the per-channel read fails,
        // and the channel must keep its Unknown default rather than the
        // enrichment failing the whole listing.
        let driver = driver_with(&[(PATH_VIDEO_INPUT_CHANNELS, xml)]);
        let channels = driver.list_channels().await.expect("list_channels");
        assert_eq!(channels[0].ptz_support, PtzSupport::Unknown);
    }

    #[tokio::test]
    async fn stream_uri_builds_a_hikvision_rtsp_url_without_leaking_the_password() {
        let driver = driver_with(&[]);
        let uri = driver.stream_uri(Some(2)).await.expect("stream_uri");
        assert_eq!(uri.transport, StreamTransport::Rtsp);
        assert!(uri.uri.contains("device.test:554/Streaming/Channels/201"));
        // The URL embeds credentials, but Debug must redact them.
        assert!(!format!("{uri:?}").contains("secretpw"));
    }

    #[tokio::test]
    async fn stream_uri_defaults_to_the_first_channel() {
        let driver = driver_with(&[]);
        let uri = driver.stream_uri(None).await.expect("stream_uri");
        assert!(uri.uri.contains("/Streaming/Channels/101"));
    }

    #[tokio::test]
    async fn stream_uri_percent_encodes_credentials_carrying_uri_metacharacters() {
        // A raw ':'/'@' in a credential must not break out of the userinfo
        // segment and redirect the parsed host/port.
        let driver = HikvisionDriver::new(
            Arc::new(FakeHttpClient::new(&[])),
            DeviceEndpoint::new("device.test", 80),
            "ad:min@",
            Secret::new("p@ss:word"),
        );
        let uri = driver.stream_uri(Some(1)).await.expect("stream_uri");
        assert!(
            uri.uri.contains("ad%3Amin%40:p%40ss%3Aword@device.test"),
            "{}",
            uri.uri
        );
    }

    #[tokio::test]
    async fn substream_uri_builds_a_hikvision_rtsp_url_with_the_02_stream_suffix() {
        let driver = driver_with(&[]);
        let uri = driver.substream_uri(Some(2)).await.expect("substream_uri");
        assert_eq!(uri.transport, StreamTransport::Rtsp);
        assert!(uri.uri.contains("device.test:554/Streaming/Channels/202"));
        // The URL embeds credentials, but Debug must redact them.
        assert!(!format!("{uri:?}").contains("secretpw"));
    }

    #[tokio::test]
    async fn playback_uri_builds_a_time_ranged_hik_url() {
        let driver = driver_with(&[]);
        let request = ExportRequest {
            channel: 2,
            start: parse_isapi_time("2026-07-01T08:00:00Z").unwrap(),
            end: parse_isapi_time("2026-07-01T09:00:00Z").unwrap(),
        };
        let uri = driver.playback_uri(&request).await.expect("playback_uri");
        assert_eq!(uri.transport, StreamTransport::Rtsp);
        assert!(uri.uri.contains("/Streaming/tracks/201"));
        assert!(uri.uri.contains("starttime=20260701T080000Z"));
        assert!(uri.uri.contains("endtime=20260701T090000Z"));
        assert!(!format!("{uri:?}").contains("secretpw"));
        assert!(driver.capabilities().export);
    }

    #[tokio::test]
    async fn playback_uri_percent_encodes_credentials_carrying_uri_metacharacters() {
        let driver = HikvisionDriver::new(
            Arc::new(FakeHttpClient::new(&[])),
            DeviceEndpoint::new("device.test", 80),
            "ad:min@",
            Secret::new("p@ss:word"),
        );
        let request = ExportRequest {
            channel: 1,
            start: parse_isapi_time("2026-07-01T08:00:00Z").unwrap(),
            end: parse_isapi_time("2026-07-01T09:00:00Z").unwrap(),
        };
        let uri = driver.playback_uri(&request).await.expect("playback_uri");
        assert!(
            uri.uri.contains("ad%3Amin%40:p%40ss%3Aword@device.test"),
            "{}",
            uri.uri
        );
    }

    #[tokio::test]
    async fn capabilities_reflect_the_implemented_read_plane() {
        let driver = driver_with(&[]);
        let capabilities = driver.capabilities();
        assert!(capabilities.info);
        assert!(capabilities.users);
        assert!(capabilities.disk);
        assert!(capabilities.streaming);
        assert!(capabilities.snapshot);
        assert!(capabilities.recording);
        assert!(capabilities.user_update);
        assert!(!capabilities.authority_catalog);
    }

    #[tokio::test]
    async fn update_user_irreversible_puts_the_salted_hash_by_id_not_the_plaintext() {
        // Edit "it" (id 2) — irreversible firmware, so the PUT carries the salted
        // hash, and the caps are read for the target's own name.
        let cap_path = format!("{PATH_SECURITY_CAP}?username=it");
        let (client, driver) = driver_from(
            RecordingHttpClient::new(&[
                (PATH_USERS, SAMPLE_USERS),
                (&cap_path, SAMPLE_CAP_IRREVERSIBLE),
            ])
            .with_puts(vec![Ok(RESPONSE_OK.to_owned())]),
        );

        let update = UserUpdate {
            name: "it".to_owned(),
            password: Some(Secret::new("new-pw")),
            group: Some("Administrator".to_owned()),
            authorities: None,
        };
        let outcome = driver
            .update_users(std::slice::from_ref(&update))
            .await
            .expect("update_users");
        assert_eq!(outcome.succeeded, ["it"]);
        assert!(outcome.failed.is_empty());

        let calls = client.calls();
        // The listing is read once, then the target's caps, then the PUT by id.
        assert_eq!(calls[0], Call::Get(PATH_USERS.to_owned()));
        assert!(
            calls
                .iter()
                .any(|call| matches!(call, Call::Get(path) if path == &cap_path))
        );

        let (path, body, content_type) = first_put(&calls);
        assert_eq!(path, "/ISAPI/Security/users/2");
        assert_eq!(content_type, CONTENT_TYPE_XML);
        let expected_hash = irreversible_password("it", "0123456789abcdef", "new-pw", 2);
        assert!(body.contains(&format!("<password>{expected_hash}</password>")));
        assert!(!body.contains("new-pw"), "plaintext leaked: {body}");
        assert!(body.contains("<id>2</id>"));
        assert!(body.contains("<userName>it</userName>"));
        assert!(body.contains("<userLevel>Administrator</userLevel>"));
        assert!(driver.capabilities().user_update);
    }

    #[tokio::test]
    async fn update_user_without_a_password_skips_caps_and_omits_the_password_element() {
        let (client, driver) = driver_from(
            RecordingHttpClient::new(&[(PATH_USERS, SAMPLE_USERS)])
                .with_puts(vec![Ok(RESPONSE_OK.to_owned())]),
        );
        // A group-only edit: no password, so no capabilities read is needed.
        let update = UserUpdate {
            name: "it".to_owned(),
            password: None,
            group: Some("Administrator".to_owned()),
            authorities: None,
        };
        let outcome = driver
            .update_users(std::slice::from_ref(&update))
            .await
            .expect("update_users");
        assert_eq!(outcome.succeeded, ["it"]);

        let calls = client.calls();
        assert!(
            !calls
                .iter()
                .any(|call| matches!(call, Call::Get(path) if path.contains("capabilities"))),
            "a password-untouched edit must not read security capabilities"
        );
        let (path, body, _ct) = first_put(&calls);
        assert_eq!(path, "/ISAPI/Security/users/2");
        assert!(body.contains("<userLevel>Administrator</userLevel>"));
        assert!(!body.contains("<password>"), "unexpected password: {body}");
    }

    #[tokio::test]
    async fn update_user_missing_from_the_device_is_a_per_item_failure() {
        let (client, driver) = driver_from(RecordingHttpClient::new(&[(PATH_USERS, SAMPLE_USERS)]));
        let update = UserUpdate {
            name: "ghost".to_owned(),
            password: None,
            group: Some("Operator".to_owned()),
            authorities: None,
        };
        let outcome = driver
            .update_users(std::slice::from_ref(&update))
            .await
            .expect("update_users");
        assert!(outcome.succeeded.is_empty());
        assert_eq!(outcome.failed.len(), 1);
        assert_eq!(outcome.failed[0].0, "ghost");
        assert!(matches!(outcome.failed[0].1, DriverError::Device(_)));
        // An absent user is never PUT — an update cannot create a missing account.
        assert!(
            !client
                .calls()
                .iter()
                .any(|call| matches!(call, Call::Put { .. }))
        );
    }

    #[tokio::test]
    async fn update_users_rejects_an_authorities_change_instead_of_dropping_it() {
        // Hikvision's AuthorityList write body is not implemented; a
        // `Some(_)` authorities facet (grant, replace, or clear) must fail
        // loudly rather than silently leave the account's authorities as-is
        // while reporting success.
        let (client, driver) = driver_from(RecordingHttpClient::new(&[(PATH_USERS, SAMPLE_USERS)]));
        let update = UserUpdate {
            name: "it".to_owned(),
            password: None,
            group: None,
            authorities: Some(Vec::new()),
        };
        let outcome = driver
            .update_users(std::slice::from_ref(&update))
            .await
            .expect("update_users");
        assert!(outcome.succeeded.is_empty());
        assert_eq!(outcome.failed.len(), 1);
        assert_eq!(outcome.failed[0].0, "it");
        assert!(matches!(outcome.failed[0].1, DriverError::Device(_)));
        assert!(
            !client
                .calls()
                .iter()
                .any(|call| matches!(call, Call::Put { .. }))
        );
    }

    #[tokio::test]
    async fn update_users_rejects_an_unrecognized_group_as_a_per_item_failure() {
        let (client, driver) = driver_from(RecordingHttpClient::new(&[(PATH_USERS, SAMPLE_USERS)]));
        let update = UserUpdate {
            name: "it".to_owned(),
            password: None,
            group: Some("Guests".to_owned()),
            authorities: None,
        };
        let outcome = driver
            .update_users(std::slice::from_ref(&update))
            .await
            .expect("update_users");
        assert!(outcome.succeeded.is_empty());
        assert_eq!(outcome.failed.len(), 1);
        assert_eq!(outcome.failed[0].0, "it");
        assert!(matches!(outcome.failed[0].1, DriverError::Device(_)));
        // The listing read happens (to resolve the id), but no PUT is issued
        // for the unrecognized group.
        assert!(
            !client
                .calls()
                .iter()
                .any(|call| matches!(call, Call::Put { .. }))
        );
    }

    #[tokio::test]
    async fn update_user_rejects_a_non_numeric_device_reported_id_as_a_per_item_failure() {
        let (client, driver) = driver_from(RecordingHttpClient::new(&[(
            PATH_USERS,
            SAMPLE_USERS_BAD_ID,
        )]));
        let update = UserUpdate {
            name: "it".to_owned(),
            password: None,
            group: Some("Operator".to_owned()),
            authorities: None,
        };
        let outcome = driver
            .update_users(std::slice::from_ref(&update))
            .await
            .expect("update_users");
        assert!(outcome.succeeded.is_empty());
        assert_eq!(outcome.failed.len(), 1);
        assert_eq!(outcome.failed[0].0, "it");
        assert!(matches!(outcome.failed[0].1, DriverError::Protocol(_)));
        assert!(
            !client
                .calls()
                .iter()
                .any(|call| matches!(call, Call::Put { .. }))
        );
    }

    #[tokio::test]
    async fn update_users_with_no_input_makes_no_calls() {
        let (client, driver) = driver_from(RecordingHttpClient::new(&[]));
        let outcome = driver.update_users(&[]).await.expect("update_users");
        assert!(outcome.succeeded.is_empty());
        assert!(outcome.failed.is_empty());
        assert!(client.calls().is_empty());
    }

    #[tokio::test]
    async fn update_users_errors_when_the_listing_cannot_be_read() {
        // No canned GET for the users listing → the single read fails, aborting all.
        let (_client, driver) = driver_from(RecordingHttpClient::new(&[]));
        let result = driver
            .update_users(&[UserUpdate {
                name: "it".to_owned(),
                password: None,
                group: Some("Operator".to_owned()),
                authorities: None,
            }])
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn update_users_aborts_the_batch_on_a_transport_failure() {
        let (_client, driver) = driver_from(
            RecordingHttpClient::new(&[(PATH_USERS, SAMPLE_USERS)])
                .with_puts(vec![Err(DriverError::Transport("link down".to_owned()))]),
        );
        let result = driver
            .update_users(&[UserUpdate {
                name: "it".to_owned(),
                password: None,
                group: Some("Operator".to_owned()),
                authorities: None,
            }])
            .await;
        assert!(matches!(result, Err(DriverError::Transport(_))));
    }

    /// `ContentMgmt/download` serves the WHOLE segment file — measured on a
    /// DS-N316/2(D), a URI narrowed to one minute still answered
    /// `Content-Length: 1064320428`, the same 1.06 GB as the untouched URI.
    /// A window smaller than the segment must therefore NOT take the fast
    /// path: the caller asked for a minute and would receive 79 of them.
    #[test]
    fn a_window_narrower_than_the_segment_declines_the_download_fast_path() {
        let xml = concat!(
            "<searchMatchItem><timeSpan>",
            "<startTime>2026-08-08T05:31:50Z</startTime>",
            "<endTime>2026-08-08T06:50:45Z</endTime>",
            "</timeSpan><mediaSegmentDescriptor><playbackURI>",
            "rtsp://10.0.0.1/Streaming/tracks/101/?starttime=20260808T053150Z",
            "&amp;endtime=20260808T065045Z&amp;name=00000002866000000&amp;size=1064320388",
            "</playbackURI></mediaSegmentDescriptor></searchMatchItem>",
        );
        let start = "2026-08-08T06:00:00Z"
            .parse::<DateTime<Utc>>()
            .expect("start");
        let end = "2026-08-08T06:01:00Z"
            .parse::<DateTime<Utc>>()
            .expect("end");

        assert!(
            find_downloadable_segment_uri(xml, start, end).is_none(),
            "a one-minute window must fall through to the RTSP plane, which cuts precisely"
        );
    }

    /// When the window does contain the segment, the fast path is exactly
    /// right: the whole file IS what the caller asked for, and it arrives
    /// several times faster than realtime.
    #[test]
    fn a_window_containing_the_segment_takes_the_download_fast_path() {
        let xml = concat!(
            "<searchMatchItem><timeSpan>",
            "<startTime>2026-08-08T06:00:00Z</startTime>",
            "<endTime>2026-08-08T06:10:00Z</endTime>",
            "</timeSpan><mediaSegmentDescriptor><playbackURI>",
            "rtsp://10.0.0.1/Streaming/tracks/101/?starttime=20260808T060000Z",
            "&amp;endtime=20260808T061000Z&amp;name=handle",
            "</playbackURI></mediaSegmentDescriptor></searchMatchItem>",
        );
        let start = "2026-08-08T05:00:00Z"
            .parse::<DateTime<Utc>>()
            .expect("start");
        let end = "2026-08-08T07:00:00Z"
            .parse::<DateTime<Utc>>()
            .expect("end");

        let uri = find_downloadable_segment_uri(xml, start, end).expect("segment is contained");
        // Verbatim: the handle is what selects the file, and the times are
        // decorative for this endpoint.
        assert!(uri.contains("name=handle"), "got {uri}");
    }
}
