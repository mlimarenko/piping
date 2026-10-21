//! Generic ONVIF driver (SOAP/XML over HTTP, WS-Security UsernameToken auth).
//!
//! ONVIF is the standardized long-tail fallback: when a device is not one of the
//! native families (DVRIP/Sofia, Dahua, Hikvision) but speaks the ONVIF profile,
//! this driver covers the *view* plane — device identity, per-profile channel
//! enumeration, live-stream/substream URI, snapshot URI — plus Profile S
//! pan/tilt/zoom control. The management plane (users / disk / recording) is
//! intentionally [`DriverError::Unsupported`]: ONVIF's user, disk, and
//! recording services are optional and unreliable across the long tail, so
//! this driver never pretends to offer them.
//!
//! Each operation is one SOAP `POST` to the device service endpoint
//! (`DEVICE_SERVICE_PATH`) carrying a fresh WS-Security `UsernameToken` in the
//! SOAP header (a `PasswordDigest`, **not** HTTP Digest). There is no session; the
//! same request/response [`HttpDeviceClient`] the CGI/ISAPI families use serves
//! ONVIF too.
//!
//! Responses are read with small, dep-free structural extractors that match
//! elements by *local* name, ignoring the namespace prefix — ONVIF devices
//! disagree on which prefix (`tds:`, `trt:`, `tt:`, a
//! default namespace, or a device alias) they emit, so matching a fixed
//! `prefix:Name` would be brittle. This is a structural reader for a flat, known
//! response shape, not a general XML parser.
//!
//! Media-service address: the ONVIF media operations (`GetProfiles`,
//! `GetStreamUri`, `GetSnapshotUri`) are formally addressed by the media `XAddr`
//! that `GetCapabilities` reports, which *may* differ from the device service
//! path. Many devices accept the media calls on `/onvif/device_service`, so this
//! driver posts them there directly; per-device media-`XAddr` discovery via
//! `GetCapabilities` is a documented refinement, not implemented here.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::{SecondsFormat, Utc};
use sha1::{Digest, Sha1};

use crate::contexts::device_management::domain::{
    BulkOutcome, Capabilities, Channel, DeviceDriver, DeviceInfo, DiskStatus, DriverError, NewUser,
    PtzCommand, PtzSupport, PtzVector, RecordingStatus, Secret, SnapshotRef, StreamTransport,
    StreamUri, UserAccount,
};
use crate::contexts::device_management::infrastructure::http_client::HttpDeviceClient;
use crate::contexts::device_management::infrastructure::transport::DeviceEndpoint;

/// Path the device and (by default) media SOAP services answer on.
const DEVICE_SERVICE_PATH: &str = "/onvif/device_service";
/// SOAP 1.2 content type ONVIF request envelopes are sent with.
const CONTENT_TYPE_SOAP: &str = "application/soap+xml; charset=utf-8";
/// Length of the WS-Security nonce, in bytes.
const NONCE_LEN: usize = 16;

/// SOAP 1.2 envelope namespace.
const SOAP_ENV_NS: &str = "http://www.w3.org/2003/05/soap-envelope";
/// WS-Security secext namespace (the `wsse:` elements).
const WSSE_NS: &str =
    "http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-secext-1.0.xsd";
/// WS-Security utility namespace (the `wsu:Created` element).
const WSU_NS: &str =
    "http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-utility-1.0.xsd";
/// `Type` attribute marking the password as a digest (not a plaintext) token.
const PASSWORD_DIGEST_TYPE: &str = "http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-username-token-profile-1.0#PasswordDigest";
/// `EncodingType` attribute marking the nonce as base64 binary.
const NONCE_ENCODING_TYPE: &str = "http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-soap-message-security-1.0#Base64Binary";
/// ONVIF device-service WSDL namespace (the `tds:` operations).
const DEVICE_WSDL_NS: &str = "http://www.onvif.org/ver10/device/wsdl";
/// ONVIF media WSDL namespace (the `trt:` operations).
const MEDIA_WSDL_NS: &str = "http://www.onvif.org/ver10/media/wsdl";
/// ONVIF common schema namespace (the `tt:` types inside media requests).
const SCHEMA_NS: &str = "http://www.onvif.org/ver10/schema";
/// ONVIF PTZ WSDL namespace (the `tptz:` operations, Profile S PTZ control).
const PTZ_WSDL_NS: &str = "http://www.onvif.org/ver20/ptz/wsdl";

/// A generic-ONVIF driver bound to one device.
pub struct OnvifDriver {
    http: Arc<dyn HttpDeviceClient>,
    #[expect(
        dead_code,
        reason = "kept for parity with native drivers and future \
        media-XAddr discovery; the SOAP client owns the base URL today"
    )]
    endpoint: DeviceEndpoint,
    username: String,
    password: Secret,
    nonce: Arc<dyn WsseTokenSource>,
}

impl OnvifDriver {
    /// Construct a driver reaching the device via `http`, using a non-cryptographic
    /// per-call nonce source (see [`CounterNonceSource`]).
    pub fn new(
        http: Arc<dyn HttpDeviceClient>,
        endpoint: DeviceEndpoint,
        username: impl Into<String>,
        password: Secret,
    ) -> Self {
        Self::with_token_source(
            http,
            endpoint,
            username,
            password,
            Arc::new(CounterNonceSource::default()),
        )
    }

    /// Construct a driver with an explicit WS-Security token source.
    ///
    /// This seam lets tests pin an exact `(nonce, created)` pair so the derived
    /// `PasswordDigest` is deterministic.
    fn with_token_source(
        http: Arc<dyn HttpDeviceClient>,
        endpoint: DeviceEndpoint,
        username: impl Into<String>,
        password: Secret,
        nonce: Arc<dyn WsseTokenSource>,
    ) -> Self {
        Self {
            http,
            endpoint,
            username: username.into(),
            password,
            nonce,
        }
    }

    /// Wrap `body` in a WS-Security-authenticated SOAP envelope and `POST` it to the
    /// device service, returning the response text.
    ///
    /// A fresh token is minted per call; the plaintext password is only fed into
    /// the digest and never placed on the wire or into an error.
    ///
    /// The response is checked for a SOAP `<Fault>` before it is handed back:
    /// embedded ONVIF devices routinely answer a bad password with HTTP 200 and
    /// a fault body rather than an HTTP 401/403, so treating every 200 as
    /// success would read back a blank-success [`DeviceInfo`] and later 502 out
    /// of `stream_uri`/`snapshot` instead of surfacing the credential problem.
    /// See [`soap_fault`].
    async fn soap_call(&self, body: &str) -> Result<String, DriverError> {
        let token = self.nonce.token();
        let digest = password_digest(&token.nonce, &token.created, self.password.expose());
        let nonce_b64 = STANDARD.encode(&token.nonce);
        let security = security_header(&self.username, &digest, &nonce_b64, &token.created);
        let envelope = envelope(&security, body);
        let response = self
            .http
            .post_text(DEVICE_SERVICE_PATH, &envelope, CONTENT_TYPE_SOAP)
            .await?;
        if let Some(err) = soap_fault(&response) {
            return Err(err);
        }
        Ok(response)
    }

    /// Resolve one media profile by 1-based `channel` index (default the
    /// first advertised profile) via `GetProfiles`.
    ///
    /// ONVIF addresses a stream/snapshot/PTZ target by media *profile*, not by
    /// a channel index, so this maps the normalized 1-based channel number
    /// onto the profile at that position in the `GetProfiles` response order.
    ///
    /// VERIFY: not confirmed against a live device — see [`parse_profiles`]'s
    /// profile-order-stability caveat, which this inherits.
    ///
    /// # Errors
    ///
    /// [`DriverError::Unsupported`] when `channel` is `Some(0)`, out of range,
    /// or the device advertises no profiles at all.
    async fn resolve_profile(&self, channel: Option<u16>) -> Result<OnvifProfile, DriverError> {
        let body = format!("<trt:GetProfiles xmlns:trt=\"{MEDIA_WSDL_NS}\"/>");
        let response = self.soap_call(&body).await?;
        let profiles = parse_profiles(&response);
        let index = channel.unwrap_or(1);
        if index == 0 {
            return Err(DriverError::Unsupported);
        }
        profiles
            .into_iter()
            .nth(usize::from(index - 1))
            .ok_or(DriverError::Unsupported)
    }

    /// Resolve the live-stream `Uri` for an already-known media-profile
    /// `token` via `GetStreamUri`.
    ///
    /// Shared by [`DeviceDriver::stream_uri`] (resolves the default profile
    /// first) and [`DeviceDriver::substream_uri`] (resolves a positional
    /// second profile first) — both then address the same operation by token.
    async fn stream_uri_for_profile(&self, token: &str) -> Result<StreamUri, DriverError> {
        let body = format!(
            "<trt:GetStreamUri xmlns:trt=\"{MEDIA_WSDL_NS}\" xmlns:tt=\"{SCHEMA_NS}\">\
<trt:StreamSetup>\
<tt:Stream>RTP-Unicast</tt:Stream>\
<tt:Transport><tt:Protocol>RTSP</tt:Protocol></tt:Transport>\
</trt:StreamSetup>\
<trt:ProfileToken>{token}</trt:ProfileToken>\
</trt:GetStreamUri>",
            token = xml_escape(token),
        );
        let response = self.soap_call(&body).await?;
        let uri = soap_text(&response, "Uri").ok_or_else(|| {
            DriverError::Protocol("GetStreamUri response carried no <Uri>".to_owned())
        })?;
        Ok(StreamUri {
            uri: uri.to_owned(),
            transport: StreamTransport::Rtsp,
        })
    }

    /// Read the PTZ configuration catalog without issuing any control operation.
    async fn ptz_configuration_catalog(&self) -> Result<PtzConfigurationCatalog, DriverError> {
        let body = format!("<tptz:GetConfigurations xmlns:tptz=\"{PTZ_WSDL_NS}\"/>");
        let response = self.soap_call(&body).await?;
        Ok(parse_ptz_configuration_catalog(&response))
    }

    /// Read the PTZ service's declared default continuous-move velocity
    /// spaces via `GetConfigurations`.
    ///
    /// This doubles as the PTZ-presence preflight [`Self::ptz_preflight`]
    /// uses before a mutating PTZ call: a device with no `PTZConfiguration`
    /// at all (a fixed camera, or a PTZ service the device does not
    /// implement) answers [`DriverError::Protocol`] here, which the caller
    /// folds into [`DriverError::Unsupported`].
    ///
    /// # Errors
    ///
    /// [`DriverError::Protocol`] when the response carries no
    /// `PTZConfiguration` element at all; otherwise whatever
    /// [`Self::soap_call`] returns (a SOAP fault or a transport error). A
    /// `PTZConfiguration` that simply omits a velocity-space URI is not an
    /// error — that axis group is `None` in the returned
    /// [`PtzVelocitySpaces`].
    async fn ptz_velocity_spaces(&self) -> Result<PtzVelocitySpaces, DriverError> {
        let body = format!("<tptz:GetConfigurations xmlns:tptz=\"{PTZ_WSDL_NS}\"/>");
        let response = self.soap_call(&body).await?;
        if next_open_tag(&response, "PTZConfiguration", 0).is_none() {
            return Err(DriverError::Protocol(
                "GetConfigurations response carried no PTZConfiguration".to_owned(),
            ));
        }
        Ok(PtzVelocitySpaces {
            pan_tilt: soap_text(&response, "DefaultContinuousPanTiltVelocitySpace")
                .map(str::to_owned),
            zoom: soap_text(&response, "DefaultContinuousZoomVelocitySpace").map(str::to_owned),
        })
    }

    /// Presence-check the PTZ service before a mutating `Move`/`Preset` call.
    ///
    /// Mirrors the discrimination `application/device_service.rs`'s snapshot
    /// fallback uses: [`DriverError::Unsupported`]/[`DriverError::Device`]/
    /// [`DriverError::Protocol`] from [`Self::ptz_velocity_spaces`] all mean
    /// "this device has no usable PTZ service" and fold into
    /// [`DriverError::Unsupported`]; `Auth`/`Transport`/`ClockSkew`/`Busy` are
    /// real failures and propagate unchanged, never masked.
    async fn ptz_preflight(&self) -> Result<PtzVelocitySpaces, DriverError> {
        match self.ptz_velocity_spaces().await {
            Ok(spaces) => Ok(spaces),
            Err(DriverError::Unsupported | DriverError::Device(_) | DriverError::Protocol(_)) => {
                Err(DriverError::Unsupported)
            }
            Err(other) => Err(other),
        }
    }
}

#[async_trait]
impl DeviceDriver for OnvifDriver {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            info: true,
            users: false,
            disk: false,
            recording: false,
            snapshot: true,
            streaming: true,
            channels: true,
            ptz: true,
            export: false,
            // User editing is not a base-profile ONVIF capability; the default
            // `update_users` stays `Unsupported` (see `create_users` below).
            user_update: false,
            // ONVIF has no discoverable authority/permission-token catalog in
            // the base profile; the default `list_authorities` stays
            // `Unsupported`.
            authority_catalog: false,
            recording_search: false,
        }
    }

    async fn get_info(&self) -> Result<DeviceInfo, DriverError> {
        let body = format!("<tds:GetDeviceInformation xmlns:tds=\"{DEVICE_WSDL_NS}\"/>");
        let response = self.soap_call(&body).await?;
        Ok(parse_device_info(&response))
    }

    async fn list_users(&self) -> Result<Vec<UserAccount>, DriverError> {
        // ONVIF user management is optional and unreliable across the long tail;
        // this is the view-only fallback, so account listing is not offered.
        Err(DriverError::Unsupported)
    }

    async fn create_users(&self, _users: &[NewUser]) -> Result<BulkOutcome, DriverError> {
        Err(DriverError::Unsupported)
    }

    async fn delete_users(&self, _usernames: &[String]) -> Result<BulkOutcome, DriverError> {
        Err(DriverError::Unsupported)
    }

    // `update_users` is intentionally left as the trait default (`Unsupported`):
    // user editing is not a base-profile ONVIF capability, so this view-only
    // fallback never pretends to offer it.

    async fn disk_status(&self) -> Result<Vec<DiskStatus>, DriverError> {
        Err(DriverError::Unsupported)
    }

    async fn recording_status(&self) -> Result<RecordingStatus, DriverError> {
        Err(DriverError::Unsupported)
    }

    async fn snapshot(&self, channel: Option<u16>) -> Result<SnapshotRef, DriverError> {
        // Resolve the snapshot URI via SOAP, then fetch the still bytes
        // server-side: the URI is credential-bearing and must never reach a
        // client. The HTTP client applies the SSRF fence (http(s) + device-host)
        // and JPEG validation, so this returns proxied bytes like the native
        // families, not a bare URI.
        let profile = self.resolve_profile(channel).await?;
        let body = format!(
            "<trt:GetSnapshotUri xmlns:trt=\"{MEDIA_WSDL_NS}\">\
<trt:ProfileToken>{token}</trt:ProfileToken>\
</trt:GetSnapshotUri>",
            token = xml_escape(&profile.token),
        );
        let response = self.soap_call(&body).await?;
        let uri = soap_text(&response, "Uri").ok_or_else(|| {
            DriverError::Protocol("GetSnapshotUri response carried no <Uri>".to_owned())
        })?;
        let bytes = self.http.get_image_from_url(uri).await?;
        Ok(SnapshotRef::Bytes(bytes))
    }

    async fn stream_uri(&self, channel: Option<u16>) -> Result<StreamUri, DriverError> {
        let profile = self.resolve_profile(channel).await?;
        self.stream_uri_for_profile(&profile.token).await
    }

    async fn substream_uri(&self, _channel: Option<u16>) -> Result<StreamUri, DriverError> {
        // ONVIF has no index-based substream: streams are addressed by media
        // *profile*, not a channel/substream index. VERIFY: ONVIF does not
        // mandate profile ordering, so picking the second advertised profile
        // is a POSITIONAL convention only — it mirrors the index-based
        // substream forms the other three families use (first configured
        // stream is main, second is sub), and is deliberately NOT a
        // name/keyword heuristic: the profile name is never inspected for
        // words like "sub"/"main", even though device-advertised profile
        // names often literally contain them (see the module-level
        // domain-neutrality note against natural-language keyword routing).
        // A bad pick just fails its playback probe, which the caller
        // (`device_service.rs`) already handles via the documented
        // bounded-probe fallback to the main stream.
        let body = format!("<trt:GetProfiles xmlns:trt=\"{MEDIA_WSDL_NS}\"/>");
        let response = self.soap_call(&body).await?;
        let profiles = parse_profiles(&response);
        let profile = profiles
            .get(1)
            .or_else(|| profiles.first())
            .ok_or(DriverError::Unsupported)?;
        self.stream_uri_for_profile(&profile.token).await
    }

    async fn list_channels(&self) -> Result<Vec<Channel>, DriverError> {
        let body = format!("<trt:GetProfiles xmlns:trt=\"{MEDIA_WSDL_NS}\"/>");
        let response = self.soap_call(&body).await?;
        let profiles = parse_profiles(&response);
        let configuration_catalog = self.ptz_configuration_catalog().await.ok();
        Ok(profiles
            .into_iter()
            .enumerate()
            .map(|(zero_based, profile)| Channel {
                index: u16::try_from(zero_based + 1).unwrap_or(u16::MAX),
                name: profile.name,
                enabled: None,
                ptz_support: classify_ptz_support(
                    &profile.ptz_configuration,
                    configuration_catalog.as_ref(),
                ),
                recording: None,
            })
            .collect())
    }

    async fn ptz(&self, channel: u16, command: PtzCommand) -> Result<(), DriverError> {
        // Profile S PTZ: a continuous move is `tptz:ContinuousMove` addressed
        // by media profile, carrying the device's own declared default
        // velocity-space URIs (read via `GetConfigurations`); `Stop` and
        // `GotoPreset` are the matching profile-addressed operations.
        match command {
            PtzCommand::Move(vector) => {
                let spaces = self.ptz_preflight().await?;
                let profile = self.resolve_profile(Some(channel)).await?;
                let body = continuous_move_body(&profile.token, vector, &spaces);
                self.soap_call(&body).await?;
            }
            PtzCommand::Stop => {
                // Halting motion must never be gated behind an extra fallible
                // round trip (mirrors the Dahua driver's "stop must be
                // maximally robust" stance): no `GetConfigurations`
                // preflight, just resolve the profile and send `Stop`.
                let profile = self.resolve_profile(Some(channel)).await?;
                let body = format!(
                    "<tptz:Stop xmlns:tptz=\"{PTZ_WSDL_NS}\">\
<tptz:ProfileToken>{token}</tptz:ProfileToken>\
<tptz:PanTilt>true</tptz:PanTilt>\
<tptz:Zoom>true</tptz:Zoom>\
</tptz:Stop>",
                    token = xml_escape(&profile.token),
                );
                self.soap_call(&body).await?;
            }
            PtzCommand::Preset { id } => {
                self.ptz_preflight().await?;
                let profile = self.resolve_profile(Some(channel)).await?;
                let body = format!(
                    "<tptz:GotoPreset xmlns:tptz=\"{PTZ_WSDL_NS}\">\
<tptz:ProfileToken>{token}</tptz:ProfileToken>\
<tptz:PresetToken>{id}</tptz:PresetToken>\
</tptz:GotoPreset>",
                    token = xml_escape(&profile.token),
                );
                self.soap_call(&body).await?;
            }
        }
        Ok(())
    }
}

// ── WS-Security token derivation ────────────────────────────────────────────

/// A per-call WS-Security freshness token: raw nonce bytes and their ISO 8601-UTC
/// `Created` timestamp.
struct WsseToken {
    /// Raw (un-encoded) nonce bytes.
    nonce: Vec<u8>,
    /// ISO 8601 UTC timestamp string placed in `<wsu:Created>`.
    created: String,
}

/// Source of the per-call [`WsseToken`].
///
/// A trait so the production, non-cryptographic source can be swapped for a fixed
/// one in tests, making the derived `PasswordDigest` deterministic.
trait WsseTokenSource: Send + Sync {
    /// Produce a token for one SOAP call.
    fn token(&self) -> WsseToken;
}

/// A non-cryptographic per-call token source.
///
/// A true CSPRNG nonce is not available here, so the nonce is derived from inputs
/// this process controls: a monotonically increasing per-call counter (bytes
/// 0..8) concatenated with the current nanosecond epoch (bytes 8..16). This
/// guarantees a distinct nonce per call — which is all the `UsernameToken`
/// freshness check needs — but is **not** cryptographically unpredictable; do not
/// rely on it for secrecy.
#[derive(Default)]
struct CounterNonceSource {
    counter: AtomicU64,
}

impl WsseTokenSource for CounterNonceSource {
    fn token(&self) -> WsseToken {
        let now = Utc::now();
        let created = now.to_rfc3339_opts(SecondsFormat::Secs, true);
        let count = self.counter.fetch_add(1, Ordering::Relaxed);
        let epoch_nanos = now.timestamp_nanos_opt().unwrap_or_default();
        let mut nonce = [0u8; NONCE_LEN];
        nonce[..8].copy_from_slice(&count.to_be_bytes());
        nonce[8..].copy_from_slice(&epoch_nanos.to_be_bytes());
        WsseToken {
            nonce: nonce.to_vec(),
            created,
        }
    }
}

/// Compute the WS-Security `PasswordDigest`: `Base64(SHA1(nonce ++ created ++
/// password))`.
///
/// The plaintext `password` is only ever hashed here and never returned.
fn password_digest(nonce: &[u8], created: &str, password: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(nonce);
    hasher.update(created.as_bytes());
    hasher.update(password.as_bytes());
    STANDARD.encode(hasher.finalize())
}

/// Build the `<wsse:Security>` SOAP header carrying the `UsernameToken`.
///
/// `username` is XML-escaped; `digest`, `nonce_b64`, and `created` are base64 /
/// ISO 8601 and structurally safe to inline.
fn security_header(username: &str, digest: &str, nonce_b64: &str, created: &str) -> String {
    format!(
        "<wsse:Security xmlns:wsse=\"{WSSE_NS}\" xmlns:wsu=\"{WSU_NS}\">\
<wsse:UsernameToken>\
<wsse:Username>{user}</wsse:Username>\
<wsse:Password Type=\"{PASSWORD_DIGEST_TYPE}\">{digest}</wsse:Password>\
<wsse:Nonce EncodingType=\"{NONCE_ENCODING_TYPE}\">{nonce_b64}</wsse:Nonce>\
<wsu:Created>{created}</wsu:Created>\
</wsse:UsernameToken>\
</wsse:Security>",
        user = xml_escape(username),
    )
}

/// Wrap a SOAP header and body in a SOAP 1.2 envelope.
fn envelope(security: &str, body: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
<s:Envelope xmlns:s=\"{SOAP_ENV_NS}\">\
<s:Header>{security}</s:Header>\
<s:Body>{body}</s:Body>\
</s:Envelope>"
    )
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

// ── Response parsing (dep-free, local-name matching) ────────────────────────

/// Build [`DeviceInfo`] from a `GetDeviceInformationResponse`.
///
/// `Manufacturer` and `Model` are folded into `model` (`"<mfr> <model>"`) so the
/// device identity is not lost; `channels` and `device_time` are `None` (this
/// response exposes neither).
fn parse_device_info(xml: &str) -> DeviceInfo {
    let manufacturer = soap_text(xml, "Manufacturer");
    let model = soap_text(xml, "Model");
    let identity = match (manufacturer, model) {
        (Some(mfr), Some(mdl)) => Some(format!("{mfr} {mdl}")),
        (Some(only), None) | (None, Some(only)) => Some(only.to_owned()),
        (None, None) => None,
    };
    DeviceInfo {
        model: identity,
        firmware: soap_text(xml, "FirmwareVersion").map(str::to_owned),
        serial: soap_text(xml, "SerialNumber").map(str::to_owned),
        channels: None,
        device_time: None,
    }
}

/// A located open tag `<[prefix:]local ...>`, described by the byte offsets of its
/// attribute region and closing `>` plus whether it is self-closing.
struct OpenTag {
    /// Byte index just after the local name — the start of the attribute region.
    attrs_start: usize,
    /// Byte index of the tag-closing `>`.
    gt: usize,
    /// Whether the tag closes itself (`<x/>`), so it has no text content.
    self_closing: bool,
}

/// Find the next element whose *local* name is `local`, at or after byte `from`.
///
/// Namespace prefixes are ignored: `tds:Model`, `Model`, and `x:Model` all match
/// `local = "Model"`. A match must be a real element name — preceded by `<` (or a
/// `<prefix:`) and followed by `>`, whitespace, or `/` — so a longer sibling name
/// (`ModelName`) or a mid-name substring (`MediaUri` for `Uri`) is not matched.
fn next_open_tag(scope: &str, local: &str, from: usize) -> Option<OpenTag> {
    let mut cursor = from;
    while let Some(rel) = scope[cursor..].find(local) {
        let name_start = cursor + rel;
        let name_end = name_start + local.len();
        cursor = name_end;
        if !preceded_by_element_open(scope, name_start) {
            continue;
        }
        let tag = match scope[name_end..].chars().next() {
            Some('>') => OpenTag {
                attrs_start: name_end,
                gt: name_end,
                self_closing: false,
            },
            Some('/') => {
                let gt = name_end + scope[name_end..].find('>')?;
                OpenTag {
                    attrs_start: name_end,
                    gt,
                    self_closing: true,
                }
            }
            Some(c) if c.is_ascii_whitespace() => {
                let gt = name_end + scope[name_end..].find('>')?;
                OpenTag {
                    attrs_start: name_end,
                    gt,
                    self_closing: scope[..gt].ends_with('/'),
                }
            }
            // A longer element name (e.g. `Uri` inside `UriList`): keep scanning.
            _ => continue,
        };
        return Some(tag);
    }
    None
}

/// Find the next closing element whose local name is `local`.
fn next_close_tag(scope: &str, local: &str, from: usize) -> Option<usize> {
    let mut cursor = from;
    while let Some(relative_start) = scope[cursor..].find("</") {
        let start = cursor + relative_start;
        let name_start = start + 2;
        let name_end = name_start
            + scope[name_start..]
                .find(|character: char| character == '>' || character.is_ascii_whitespace())?;
        let qualified_name = &scope[name_start..name_end];
        if local_name(qualified_name) == local {
            return Some(start);
        }
        cursor = name_end;
    }
    None
}

/// Return the inner XML of one non-self-closing element.
fn element_inner_scope<'a>(scope: &'a str, tag: &OpenTag, local: &str) -> Option<&'a str> {
    if tag.self_closing {
        return Some("");
    }
    let content_start = tag.gt + 1;
    let content_end = next_close_tag(scope, local, content_start)?;
    Some(&scope[content_start..content_end])
}

/// Whether the byte just before `name_start` opens an element: either a `<`, or a
/// `:` whose namespace prefix runs back to a `<` (so `<tds:Model` qualifies but
/// the `Model` inside `SubModel` or `</tds:Model>` does not).
fn preceded_by_element_open(scope: &str, name_start: usize) -> bool {
    let before = &scope.as_bytes()[..name_start];
    match before.last() {
        Some(b'<') => true,
        Some(b':') => {
            // Walk back over ASCII prefix bytes; the prefix must reach a `<` with
            // no `/` (which would make this a close tag) in between.
            let mut idx = before.len() - 1;
            while idx > 0 {
                match before[idx - 1] {
                    b'<' => return true,
                    b if is_name_byte(b) => idx -= 1,
                    _ => return false,
                }
            }
            false
        }
        _ => false,
    }
}

/// Whether `b` is an ASCII byte allowed in an XML name / namespace prefix.
fn is_name_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.')
}

/// Return the trimmed text of the first element with local name `local`.
///
/// A self-closing or empty-valued match is skipped and the scan continues, so a
/// container that merely shares the local name does not shadow the leaf element.
fn soap_text<'a>(scope: &'a str, local: &str) -> Option<&'a str> {
    let mut from = 0;
    while let Some(tag) = next_open_tag(scope, local, from) {
        if tag.self_closing {
            from = tag.gt + 1;
            continue;
        }
        let value_start = tag.gt + 1;
        let end = value_start + scope[value_start..].find('<')?;
        let value = scope[value_start..end].trim();
        if !value.is_empty() {
            return Some(value);
        }
        from = end;
    }
    None
}

/// Locate every matching open tag, preserving document order.
fn element_tags(scope: &str, local: &str) -> Vec<OpenTag> {
    let mut tags = Vec::new();
    let mut from = 0;
    while let Some(tag) = next_open_tag(scope, local, from) {
        from = tag.gt + 1;
        tags.push(tag);
    }
    tags
}

/// Extract an attribute from every matching element, preserving document order.
fn element_attribute_values(scope: &str, local: &str, attr: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut from = 0;
    while let Some(tag) = next_open_tag(scope, local, from) {
        if let Some(value) = attr_value(&scope[tag.attrs_start..tag.gt], attr) {
            values.push(value.to_owned());
        }
        from = tag.gt + 1;
    }
    values
}

/// Extract `attr="value"` from an open tag's attribute region.
///
/// The name must be delimited (preceded by whitespace or the region start) so a
/// suffix match (`xtoken` for `token`) is rejected; the value is the content of
/// the following double-quoted string.
fn attr_value<'a>(attrs: &'a str, attr: &str) -> Option<&'a str> {
    let mut from = 0;
    while let Some(rel) = attrs[from..].find(attr) {
        let start = from + rel;
        let end = start + attr.len();
        from = end;
        let delimited = start == 0 || attrs.as_bytes()[start - 1].is_ascii_whitespace();
        if !delimited {
            continue;
        }
        let rest = attrs[end..].trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        let rest = rest.trim_start();
        let Some(rest) = rest.strip_prefix('"') else {
            continue;
        };
        let close = rest.find('"')?;
        return Some(&rest[..close]);
    }
    None
}

/// One ONVIF media profile: the address `GetStreamUri`/`GetSnapshotUri`/PTZ
/// operations key on.
struct OnvifProfile {
    /// The profile's own `token` attribute — carried as
    /// `<trt:ProfileToken>`/`<tptz:ProfileToken>` in every profile-addressed
    /// SOAP call.
    token: String,
    /// The profile's own `<tt:Name>`, if present.
    name: Option<String>,
    /// Structural PTZ configuration reference carried by this profile.
    ptz_configuration: ProfilePtzConfiguration,
}

/// Structural state of a profile's PTZ configuration reference.
enum ProfilePtzConfiguration {
    /// The profile carries no PTZ configuration element.
    Absent,
    /// The profile carries exactly one configuration token.
    Token(String),
    /// The profile carries multiple configuration elements and is invalid/ambiguous.
    Ambiguous,
}

/// Parse every advertised media profile, in document order, from a
/// `GetProfilesResponse`.
///
/// Each profile is one `<trt:Profiles token="...">...</trt:Profiles>`
/// element (ONVIF names the per-profile element `Profiles`, not `Profile`).
/// A profile's scope is bounded by the *next* profile's own opening tag (or
/// the end of the document for the last one), so [`soap_text`] scanning for
/// `Name` inside that scope cannot spill into a sibling profile.
///
/// VERIFY against a live device / the ONVIF profile XSD: assumes a profile's
/// own `Name` element is the FIRST `Name`-local child inside its scope. This
/// is true per the ONVIF profile complex-type sequence (`Name` is listed
/// before `VideoSourceConfiguration`/`VideoEncoderConfiguration`, each of
/// which may carry its own nested `Name`), but has not been confirmed live —
/// a device violating the XSD element order would read the wrong name (the
/// `token`, which addresses every downstream call, is unaffected).
fn parse_profiles(xml: &str) -> Vec<OnvifProfile> {
    let Some(response_tag) = next_open_tag(xml, "GetProfilesResponse", 0) else {
        return Vec::new();
    };
    let Some(response_scope) = element_inner_scope(xml, &response_tag, "GetProfilesResponse")
    else {
        return Vec::new();
    };

    let mut profiles = Vec::new();
    let mut from = 0;
    while let Some(tag) = next_open_tag(response_scope, "Profiles", from) {
        from = tag.gt + 1;
        let Some(token) = attr_value(&response_scope[tag.attrs_start..tag.gt], "token") else {
            continue;
        };
        let Some(profile_scope) = element_inner_scope(response_scope, &tag, "Profiles") else {
            continue;
        };
        let name = soap_text(profile_scope, "Name").map(str::to_owned);
        let profile_ptz_tokens =
            element_attribute_values(profile_scope, "PTZConfiguration", "token");
        let ptz_configuration = match profile_ptz_tokens.as_slice() {
            [] => ProfilePtzConfiguration::Absent,
            [configuration_token] => ProfilePtzConfiguration::Token(configuration_token.clone()),
            _ => ProfilePtzConfiguration::Ambiguous,
        };
        profiles.push(OnvifProfile {
            token: token.to_owned(),
            name,
            ptz_configuration,
        });
    }
    profiles
}

/// Read-only PTZ configuration discovery result.
enum PtzConfigurationCatalog {
    /// A successful, structurally valid response with these unique tokens.
    Valid(Vec<String>),
    /// The response was malformed or ambiguous, so no support claim is safe.
    Ambiguous,
}

fn parse_ptz_configuration_catalog(xml: &str) -> PtzConfigurationCatalog {
    if next_open_tag(xml, "GetConfigurationsResponse", 0).is_none() {
        return PtzConfigurationCatalog::Ambiguous;
    }

    let tags = element_tags(xml, "PTZConfiguration");
    if tags.is_empty() {
        return PtzConfigurationCatalog::Valid(Vec::new());
    }
    let mut tokens = Vec::with_capacity(tags.len());
    for tag in tags {
        let Some(token) = attr_value(&xml[tag.attrs_start..tag.gt], "token") else {
            return PtzConfigurationCatalog::Ambiguous;
        };
        if tokens.iter().any(|existing| existing == token) {
            return PtzConfigurationCatalog::Ambiguous;
        }
        tokens.push(token.to_owned());
    }
    PtzConfigurationCatalog::Valid(tokens)
}

fn classify_ptz_support(
    profile_configuration: &ProfilePtzConfiguration,
    discovered_catalog: Option<&PtzConfigurationCatalog>,
) -> PtzSupport {
    let Some(PtzConfigurationCatalog::Valid(tokens)) = discovered_catalog else {
        return PtzSupport::Unknown;
    };
    match profile_configuration {
        ProfilePtzConfiguration::Absent => PtzSupport::Unsupported,
        ProfilePtzConfiguration::Token(profile_token)
            if tokens.iter().any(|token| token == profile_token) =>
        {
            PtzSupport::Supported
        }
        ProfilePtzConfiguration::Token(_) | ProfilePtzConfiguration::Ambiguous => {
            PtzSupport::Unknown
        }
    }
}

// ── SOAP Fault detection ─────────────────────────────────────────────────

/// Inspect a SOAP response body for a `<Fault>` and, if present, map it to a
/// [`DriverError`].
///
/// Embedded ONVIF devices routinely answer a bad `UsernameToken` with HTTP 200
/// and a SOAP `<s:Fault>` body (a `wsse:FailedAuthentication` or the
/// ONVIF-specific `ter:NotAuthorized` subcode) rather than an HTTP 401/403.
/// Left undetected, that 200 read back as a blank-success [`DeviceInfo`] and
/// pushed a 502 out of `stream_uri`/`snapshot` instead of the auth error the
/// credential-recovery path needs. This is parsed structurally with the same
/// local-name [`next_open_tag`]/[`soap_text`] reader used for a normal
/// response, not by scanning the body for keywords.
///
/// `None` means the response carried no `<Fault>` (the common case). A fault
/// whose subcode is `NotAuthorized`/`FailedAuthentication` (any namespace
/// prefix) maps to [`DriverError::Auth`]; every other fault maps to
/// [`DriverError::Protocol`] carrying the device's fault reason.
fn soap_fault(response: &str) -> Option<DriverError> {
    let fault = next_open_tag(response, "Fault", 0)?;
    if fault.self_closing {
        return Some(DriverError::Protocol(
            "device returned an empty SOAP Fault".to_owned(),
        ));
    }
    let scope = &response[fault.gt + 1..];

    // SOAP 1.2 nests the specific subcode inside `<Code><Subcode><Value>`; fall
    // back to the top-level `<Code><Value>` (SOAP 1.2) or `<faultcode>` (SOAP
    // 1.1) when no `Subcode` is present.
    let subcode_value =
        next_open_tag(scope, "Subcode", 0).and_then(|tag| soap_text(&scope[tag.gt + 1..], "Value"));
    let code_value = subcode_value
        .or_else(|| soap_text(scope, "Value"))
        .or_else(|| soap_text(scope, "faultcode"));

    let reason = soap_text(scope, "Text").or_else(|| soap_text(scope, "faultstring"));

    let is_auth_fault = code_value.map(local_name).is_some_and(|local| {
        local.eq_ignore_ascii_case("NotAuthorized")
            || local.eq_ignore_ascii_case("FailedAuthentication")
    });
    if is_auth_fault {
        return Some(DriverError::Auth);
    }
    Some(DriverError::Protocol(format!(
        "device returned a SOAP Fault: {}",
        reason.unwrap_or("no reason given")
    )))
}

/// The local (namespace-stripped) part of a qualified-name *text* value like
/// `"ter:NotAuthorized"`.
///
/// This is distinct from [`preceded_by_element_open`]'s prefix stripping,
/// which matches *element* names structurally; a fault subcode's namespace
/// prefix is carried as plain text content, not markup.
fn local_name(qname: &str) -> &str {
    qname.rsplit(':').next().unwrap_or(qname)
}

// ── PTZ (Profile S) request building ─────────────────────────────────────

/// The PTZ service's declared default continuous-move velocity space URIs,
/// as read from `GetConfigurations`.
///
/// A `None` field means the device did not declare a default for that axis
/// group; [`continuous_move_body`] then omits the `space` attribute entirely,
/// which the ONVIF spec defines as "use the device's own default space" for
/// that axis group — so a missing declaration is not an error, only a
/// missing (optional) hint.
struct PtzVelocitySpaces {
    /// Default continuous pan/tilt velocity space URI, if declared.
    pan_tilt: Option<String>,
    /// Default continuous zoom velocity space URI, if declared.
    zoom: Option<String>,
}

/// Build the ONVIF `<tptz:ContinuousMove>` request body.
///
/// `vector` is clamped into `-1.0..=1.0` and each axis formatted to 6
/// decimal places; the declared default velocity-space URI is echoed per
/// axis group when `spaces` reports one ([`space_attr`] omits the attribute
/// entirely when there is none, letting the device fall back to its own
/// default space for that axis group).
fn continuous_move_body(token: &str, vector: PtzVector, spaces: &PtzVelocitySpaces) -> String {
    let vector = vector.clamped();
    format!(
        "<tptz:ContinuousMove xmlns:tptz=\"{PTZ_WSDL_NS}\" xmlns:tt=\"{SCHEMA_NS}\">\
<tptz:ProfileToken>{token}</tptz:ProfileToken>\
<tptz:Velocity>\
<tt:PanTilt x=\"{pan:.6}\" y=\"{tilt:.6}\"{pan_tilt_space}/>\
<tt:Zoom x=\"{zoom:.6}\"{zoom_space}/>\
</tptz:Velocity>\
</tptz:ContinuousMove>",
        token = xml_escape(token),
        pan = vector.pan,
        tilt = vector.tilt,
        zoom = vector.zoom,
        pan_tilt_space = space_attr(spaces.pan_tilt.as_deref()),
        zoom_space = space_attr(spaces.zoom.as_deref()),
    )
}

/// Render a `space="..."` XML attribute (XML-escaped), or an empty string
/// when `space` is `None` (the ONVIF-spec meaning: use the device's own
/// default space for that axis group).
fn space_attr(space: Option<&str>) -> String {
    match space {
        Some(space) => format!(" space=\"{}\"", xml_escape(space)),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use super::*;

    /// A fixed nonce+created token source, so a test asserts a deterministic
    /// `PasswordDigest` vector.
    struct FixedTokenSource {
        nonce: Vec<u8>,
        created: String,
    }

    impl WsseTokenSource for FixedTokenSource {
        fn token(&self) -> WsseToken {
            WsseToken {
                nonce: self.nonce.clone(),
                created: self.created.clone(),
            }
        }
    }

    /// One recorded HTTP call: `(path, body, content_type)`.
    type Call = (String, String, String);

    /// A recording [`HttpDeviceClient`] that replays a FIFO queue of canned POST
    /// responses (and, separately, canned snapshot-image results) and records
    /// every call. No network is touched.
    struct RecordingSoapClient {
        responses: Mutex<VecDeque<Result<String, DriverError>>>,
        images: Mutex<VecDeque<Result<Vec<u8>, DriverError>>>,
        calls: Mutex<Vec<Call>>,
        image_urls: Mutex<Vec<String>>,
    }

    impl RecordingSoapClient {
        fn new(responses: Vec<Result<String, DriverError>>) -> Self {
            Self {
                responses: Mutex::new(responses.into()),
                images: Mutex::new(VecDeque::new()),
                calls: Mutex::new(Vec::new()),
                image_urls: Mutex::new(Vec::new()),
            }
        }

        /// Queue the bytes `get_image_from_url` will return, in call order.
        fn with_images(self, images: Vec<Result<Vec<u8>, DriverError>>) -> Self {
            *self.images.lock().unwrap() = images.into();
            self
        }

        fn calls(&self) -> Vec<Call> {
            self.calls.lock().unwrap().clone()
        }

        fn image_urls(&self) -> Vec<String> {
            self.image_urls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl HttpDeviceClient for RecordingSoapClient {
        async fn get_text(&self, path: &str) -> Result<String, DriverError> {
            Err(DriverError::Device(format!("unexpected GET {path}")))
        }

        async fn post_text(
            &self,
            path: &str,
            body: &str,
            content_type: &str,
        ) -> Result<String, DriverError> {
            self.calls.lock().unwrap().push((
                path.to_owned(),
                body.to_owned(),
                content_type.to_owned(),
            ));
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Err(DriverError::Device("no canned POST response".to_owned())))
        }

        async fn get_image_from_url(&self, url: &str) -> Result<Vec<u8>, DriverError> {
            self.image_urls.lock().unwrap().push(url.to_owned());
            self.images
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Err(DriverError::Device("no canned image response".to_owned())))
        }
    }

    const FIXED_NONCE: [u8; NONCE_LEN] = [
        0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
        0x10,
    ];
    const FIXED_CREATED: &str = "2026-07-17T12:34:56Z";
    const PASSWORD: &str = "s3cr3t-pw";

    const DEVICE_INFO_RESPONSE: &str = "\
<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
<SOAP-ENV:Envelope xmlns:SOAP-ENV=\"http://www.w3.org/2003/05/soap-envelope\" \
xmlns:tds=\"http://www.onvif.org/ver10/device/wsdl\">\
<SOAP-ENV:Body>\
<tds:GetDeviceInformationResponse>\
<tds:Manufacturer>ACME</tds:Manufacturer>\
<tds:Model>NVR-8</tds:Model>\
<tds:FirmwareVersion>V5.1.2</tds:FirmwareVersion>\
<tds:SerialNumber>SN-0001-ABCD</tds:SerialNumber>\
<tds:HardwareId>HW-77</tds:HardwareId>\
</tds:GetDeviceInformationResponse>\
</SOAP-ENV:Body>\
</SOAP-ENV:Envelope>";

    const PROFILES_RESPONSE: &str = "\
<s:Envelope xmlns:s=\"http://www.w3.org/2003/05/soap-envelope\" \
xmlns:trt=\"http://www.onvif.org/ver10/media/wsdl\" \
xmlns:tt=\"http://www.onvif.org/ver10/schema\">\
<s:Body>\
<trt:GetProfilesResponse>\
<trt:Profiles token=\"Profile_1\" fixed=\"true\"><tt:Name>mainstream</tt:Name><tt:PTZConfiguration token=\"PTZ_1\"/></trt:Profiles>\
<trt:Profiles token=\"Profile_2\" fixed=\"true\"><tt:Name>substream</tt:Name></trt:Profiles>\
</trt:GetProfilesResponse>\
</s:Body>\
</s:Envelope>";

    const STREAM_URI_RESPONSE: &str = "\
<s:Envelope xmlns:s=\"http://www.w3.org/2003/05/soap-envelope\" \
xmlns:trt=\"http://www.onvif.org/ver10/media/wsdl\" \
xmlns:tt=\"http://www.onvif.org/ver10/schema\">\
<s:Body>\
<trt:GetStreamUriResponse>\
<trt:MediaUri>\
<tt:Uri>rtsp://device.test:554/Streaming/Channels/101</tt:Uri>\
<tt:InvalidAfterConnect>false</tt:InvalidAfterConnect>\
<tt:Timeout>PT60S</tt:Timeout>\
</trt:MediaUri>\
</trt:GetStreamUriResponse>\
</s:Body>\
</s:Envelope>";

    const SNAPSHOT_URI_RESPONSE: &str = "\
<s:Envelope xmlns:s=\"http://www.w3.org/2003/05/soap-envelope\" \
xmlns:trt=\"http://www.onvif.org/ver10/media/wsdl\" \
xmlns:tt=\"http://www.onvif.org/ver10/schema\">\
<s:Body>\
<trt:GetSnapshotUriResponse>\
<trt:MediaUri>\
<tt:Uri>http://device.test/onvif-http/snapshot?Profile_1</tt:Uri>\
</trt:MediaUri>\
</trt:GetSnapshotUriResponse>\
</s:Body>\
</s:Envelope>";

    // A device answering HTTP 200 with a SOAP 1.2 Fault carrying the
    // ONVIF-specific `ter:NotAuthorized` subcode (a bad password or nonce).
    const NOT_AUTHORIZED_FAULT_RESPONSE: &str = "\
<s:Envelope xmlns:s=\"http://www.w3.org/2003/05/soap-envelope\" \
xmlns:ter=\"http://www.onvif.org/ver10/error\">\
<s:Body>\
<s:Fault>\
<s:Code>\
<s:Value>s:Sender</s:Value>\
<s:Subcode><s:Value>ter:NotAuthorized</s:Value></s:Subcode>\
</s:Code>\
<s:Reason><s:Text xml:lang=\"en\">Sender not authorized</s:Text></s:Reason>\
</s:Fault>\
</s:Body>\
</s:Envelope>";

    // A device answering with the WS-Security `wsse:FailedAuthentication`
    // subcode instead of the ONVIF-specific one.
    const FAILED_AUTHENTICATION_FAULT_RESPONSE: &str = "\
<s:Envelope xmlns:s=\"http://www.w3.org/2003/05/soap-envelope\" \
xmlns:wsse=\"http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-secext-1.0.xsd\">\
<s:Body>\
<s:Fault>\
<s:Code>\
<s:Value>s:Sender</s:Value>\
<s:Subcode><s:Value>wsse:FailedAuthentication</s:Value></s:Subcode>\
</s:Code>\
<s:Reason><s:Text xml:lang=\"en\">The security token could not be authenticated</s:Text></s:Reason>\
</s:Fault>\
</s:Body>\
</s:Envelope>";

    // A non-auth fault (e.g. an unsupported action): must not be mistaken for
    // an auth failure.
    const ACTION_NOT_SUPPORTED_FAULT_RESPONSE: &str = "\
<s:Envelope xmlns:s=\"http://www.w3.org/2003/05/soap-envelope\" \
xmlns:ter=\"http://www.onvif.org/ver10/error\">\
<s:Body>\
<s:Fault>\
<s:Code>\
<s:Value>s:Sender</s:Value>\
<s:Subcode><s:Value>ter:ActionNotSupported</s:Value></s:Subcode>\
</s:Code>\
<s:Reason><s:Text xml:lang=\"en\">Action not supported</s:Text></s:Reason>\
</s:Fault>\
</s:Body>\
</s:Envelope>";

    /// Build a driver over a recording client with a fixed WS-Security token,
    /// returning the client handle so recorded calls can be asserted.
    fn driver_with(
        responses: Vec<Result<String, DriverError>>,
    ) -> (Arc<RecordingSoapClient>, OnvifDriver) {
        driver_over(RecordingSoapClient::new(responses))
    }

    /// Like [`driver_with`], but the client also has canned snapshot-image results.
    fn driver_with_images(
        responses: Vec<Result<String, DriverError>>,
        images: Vec<Result<Vec<u8>, DriverError>>,
    ) -> (Arc<RecordingSoapClient>, OnvifDriver) {
        driver_over(RecordingSoapClient::new(responses).with_images(images))
    }

    /// Wrap a prepared recording client in a fixed-token driver.
    fn driver_over(client: RecordingSoapClient) -> (Arc<RecordingSoapClient>, OnvifDriver) {
        let client = Arc::new(client);
        let driver = OnvifDriver::with_token_source(
            client.clone(),
            DeviceEndpoint::new("device.test", 80),
            "admin",
            Secret::new(PASSWORD),
            Arc::new(FixedTokenSource {
                nonce: FIXED_NONCE.to_vec(),
                created: FIXED_CREATED.to_owned(),
            }),
        );
        (client, driver)
    }

    // ── (a) get_info parses identity fields ─────────────────────────────────
    #[tokio::test]
    async fn get_info_parses_manufacturer_model_firmware_and_serial() {
        let (_client, driver) = driver_with(vec![Ok(DEVICE_INFO_RESPONSE.to_owned())]);
        let info = driver.get_info().await.expect("get_info");
        // Manufacturer + Model are folded into the identity string.
        assert_eq!(info.model.as_deref(), Some("ACME NVR-8"));
        assert_eq!(info.firmware.as_deref(), Some("V5.1.2"));
        assert_eq!(info.serial.as_deref(), Some("SN-0001-ABCD"));
        assert_eq!(info.channels, None);
        assert_eq!(info.device_time, None);
    }

    // ── (b) the request carries a WS-Security UsernameToken, no plaintext pw ──
    #[tokio::test]
    async fn request_carries_a_username_token_digest_and_hides_the_plaintext() {
        let (client, driver) = driver_with(vec![Ok(DEVICE_INFO_RESPONSE.to_owned())]);
        driver.get_info().await.expect("get_info");

        let calls = client.calls();
        assert_eq!(calls.len(), 1);
        let (path, body, content_type) = &calls[0];
        assert_eq!(path, DEVICE_SERVICE_PATH);
        assert!(content_type.starts_with("application/soap+xml"));

        assert!(body.contains("<wsse:Security"));
        assert!(body.contains("<wsse:UsernameToken>"));
        assert!(body.contains("<wsse:Username>admin</wsse:Username>"));
        assert!(body.contains("#PasswordDigest"));
        assert!(body.contains("<wsu:Created>"));
        // The plaintext password must never appear on the wire.
        assert!(
            !body.contains(PASSWORD),
            "plaintext password leaked: {body}"
        );
    }

    // ── (c) the digest equals Base64(SHA1(nonce ++ created ++ password)) ─────
    #[tokio::test]
    async fn password_digest_matches_the_ws_security_reference_vector() {
        let (client, driver) = driver_with(vec![Ok(DEVICE_INFO_RESPONSE.to_owned())]);
        driver.get_info().await.expect("get_info");
        let (_path, body, _ct) = client.calls().remove(0);

        // Independently derive the expected digest and nonce from the primitives.
        let mut hasher = Sha1::new();
        hasher.update(FIXED_NONCE);
        hasher.update(FIXED_CREATED.as_bytes());
        hasher.update(PASSWORD.as_bytes());
        let expected_digest = STANDARD.encode(hasher.finalize());
        let expected_nonce = STANDARD.encode(FIXED_NONCE);

        assert!(
            body.contains(&format!(
                "<wsse:Password Type=\"{PASSWORD_DIGEST_TYPE}\">{expected_digest}</wsse:Password>"
            )),
            "digest mismatch in body: {body}"
        );
        assert!(body.contains(&format!(">{expected_nonce}</wsse:Nonce>")));
        assert!(body.contains(&format!("<wsu:Created>{FIXED_CREATED}</wsu:Created>")));
        // A SHA-1 digest base64-encodes to 28 chars, and is never the plaintext.
        assert_eq!(expected_digest.len(), 28);
        assert_ne!(expected_digest, PASSWORD);
    }

    // ── (d) stream_uri: GetProfiles then GetStreamUri, returns the RTSP Uri ──
    #[tokio::test]
    async fn stream_uri_fetches_the_profile_then_resolves_the_rtsp_uri() {
        let (client, driver) = driver_with(vec![
            Ok(PROFILES_RESPONSE.to_owned()),
            Ok(STREAM_URI_RESPONSE.to_owned()),
        ]);
        let stream = driver.stream_uri(None).await.expect("stream_uri");
        assert_eq!(stream.transport, StreamTransport::Rtsp);
        assert_eq!(stream.uri, "rtsp://device.test:554/Streaming/Channels/101");

        let calls = client.calls();
        assert_eq!(calls.len(), 2);
        assert!(calls[0].1.contains("GetProfiles"));
        assert!(calls[1].1.contains("GetStreamUri"));
        // The resolved profile token is carried into GetStreamUri.
        assert!(
            calls[1]
                .1
                .contains("<trt:ProfileToken>Profile_1</trt:ProfileToken>")
        );
        assert!(calls[1].1.contains("RTP-Unicast"));
        assert!(calls[1].1.contains("RTSP"));
    }

    // ── (e) snapshot resolves the URI then proxies the fetched JPEG bytes ─────
    #[tokio::test]
    async fn snapshot_resolves_the_uri_then_returns_the_fetched_bytes() {
        let jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        let (client, driver) = driver_with_images(
            vec![
                Ok(PROFILES_RESPONSE.to_owned()),
                Ok(SNAPSHOT_URI_RESPONSE.to_owned()),
            ],
            vec![Ok(jpeg.clone())],
        );
        let snapshot = driver.snapshot(None).await.expect("snapshot");
        match snapshot {
            // The still is proxied as bytes, never a bare URI.
            SnapshotRef::Bytes(bytes) => assert_eq!(bytes, jpeg),
            other => panic!("expected SnapshotRef::Bytes, got {other:?}"),
        }
        let calls = client.calls();
        assert_eq!(calls.len(), 2);
        assert!(calls[0].1.contains("GetProfiles"));
        assert!(calls[1].1.contains("GetSnapshotUri"));
        assert!(
            calls[1]
                .1
                .contains("<trt:ProfileToken>Profile_1</trt:ProfileToken>")
        );
        // The URI resolved from GetSnapshotUri is exactly what the fetch dialed.
        assert_eq!(
            client.image_urls(),
            vec!["http://device.test/onvif-http/snapshot?Profile_1".to_owned()]
        );
    }

    // The snapshot URI is resolved but the server-side fetch is refused (e.g. the
    // SSRF fence): the driver surfaces the fetch error, never a bare URI.
    #[tokio::test]
    async fn snapshot_propagates_a_fetch_failure() {
        let (_client, driver) = driver_with_images(
            vec![
                Ok(PROFILES_RESPONSE.to_owned()),
                Ok(SNAPSHOT_URI_RESPONSE.to_owned()),
            ],
            vec![Err(DriverError::Device(
                "snapshot URL host mismatch".to_owned(),
            ))],
        );
        assert!(matches!(
            driver.snapshot(None).await,
            Err(DriverError::Device(_))
        ));
    }

    // ── (f) the management plane is Unsupported ──────────────────────────────
    #[tokio::test]
    async fn management_operations_are_unsupported() {
        let (_client, driver) = driver_with(vec![]);
        assert!(matches!(
            driver.list_users().await,
            Err(DriverError::Unsupported)
        ));
        assert!(matches!(
            driver.disk_status().await,
            Err(DriverError::Unsupported)
        ));
        assert!(matches!(
            driver.recording_status().await,
            Err(DriverError::Unsupported)
        ));
        assert!(matches!(
            driver.create_users(&[]).await,
            Err(DriverError::Unsupported)
        ));
        assert!(matches!(
            driver.update_users(&[]).await,
            Err(DriverError::Unsupported)
        ));
        assert!(matches!(
            driver.delete_users(&[]).await,
            Err(DriverError::Unsupported)
        ));
    }

    // ── (g) capabilities are view-only ───────────────────────────────────────
    #[test]
    fn capabilities_are_view_only() {
        let (_client, driver) = driver_with(vec![]);
        let caps = driver.capabilities();
        assert!(caps.info);
        assert!(caps.snapshot);
        assert!(caps.streaming);
        assert!(caps.channels);
        assert!(caps.ptz);
        assert!(!caps.users);
        assert!(!caps.disk);
        assert!(!caps.recording);
        assert!(!caps.user_update);
    }

    // ── (h) an HTTP-200 SOAP Fault is detected, not read back as success ─────
    #[tokio::test]
    async fn get_info_maps_a_not_authorized_fault_to_auth_error() {
        let (_client, driver) = driver_with(vec![Ok(NOT_AUTHORIZED_FAULT_RESPONSE.to_owned())]);
        assert!(matches!(driver.get_info().await, Err(DriverError::Auth)));
    }

    #[tokio::test]
    async fn get_info_maps_a_failed_authentication_fault_to_auth_error() {
        let (_client, driver) =
            driver_with(vec![Ok(FAILED_AUTHENTICATION_FAULT_RESPONSE.to_owned())]);
        assert!(matches!(driver.get_info().await, Err(DriverError::Auth)));
    }

    #[tokio::test]
    async fn get_info_maps_a_non_auth_fault_to_protocol_error() {
        let (_client, driver) =
            driver_with(vec![Ok(ACTION_NOT_SUPPORTED_FAULT_RESPONSE.to_owned())]);
        match driver.get_info().await {
            Err(DriverError::Protocol(message)) => {
                assert!(
                    message.contains("Action not supported"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected DriverError::Protocol, got {other:?}"),
        }
    }

    // A fault surfaced while resolving the profile (before the media call) must
    // still stop the chain with the mapped error, not a confusing downstream
    // "no profile token" protocol error.
    #[tokio::test]
    async fn stream_uri_propagates_an_auth_fault_from_get_profiles() {
        let (_client, driver) = driver_with(vec![Ok(NOT_AUTHORIZED_FAULT_RESPONSE.to_owned())]);
        assert!(matches!(
            driver.stream_uri(None).await,
            Err(DriverError::Auth)
        ));
    }

    // ── Pure parser unit tests ───────────────────────────────────────────────
    #[test]
    fn soap_text_matches_local_name_ignoring_namespace_prefix() {
        assert_eq!(
            soap_text("<tds:Model>X100</tds:Model>", "Model"),
            Some("X100")
        );
        assert_eq!(soap_text("<Model>Y</Model>", "Model"), Some("Y"));
        // Surrounding whitespace is trimmed.
        assert_eq!(
            soap_text("<tt:Uri>  rtsp://x  </tt:Uri>", "Uri"),
            Some("rtsp://x")
        );
    }

    #[test]
    fn soap_text_does_not_match_a_longer_name_or_a_container_suffix() {
        // A longer sibling element name must not match.
        assert_eq!(soap_text("<tt:ModelName>no</tt:ModelName>", "Model"), None);
        // A container sharing the suffix must not shadow the leaf `<tt:Uri>`.
        assert_eq!(
            soap_text(
                "<trt:MediaUri><tt:Uri>rtsp://x</tt:Uri></trt:MediaUri>",
                "Uri"
            ),
            Some("rtsp://x")
        );
        // A self-closing/empty element yields no text.
        assert_eq!(soap_text("<tds:Model/>", "Model"), None);
        assert_eq!(soap_text("<tds:Model></tds:Model>", "Model"), None);
    }

    #[test]
    fn soap_fault_is_none_for_a_normal_response() {
        assert!(soap_fault(DEVICE_INFO_RESPONSE).is_none());
        assert!(soap_fault(PROFILES_RESPONSE).is_none());
    }

    #[test]
    fn soap_fault_maps_the_onvif_not_authorized_subcode_to_auth() {
        assert!(matches!(
            soap_fault(NOT_AUTHORIZED_FAULT_RESPONSE),
            Some(DriverError::Auth)
        ));
    }

    #[test]
    fn soap_fault_maps_the_wsse_failed_authentication_subcode_to_auth() {
        assert!(matches!(
            soap_fault(FAILED_AUTHENTICATION_FAULT_RESPONSE),
            Some(DriverError::Auth)
        ));
    }

    #[test]
    fn soap_fault_maps_a_non_auth_subcode_to_protocol_with_the_device_reason() {
        match soap_fault(ACTION_NOT_SUPPORTED_FAULT_RESPONSE) {
            Some(DriverError::Protocol(message)) => {
                assert!(
                    message.contains("Action not supported"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected Some(DriverError::Protocol(_)), got {other:?}"),
        }
    }

    #[test]
    fn soap_fault_handles_a_self_closing_fault_element() {
        let xml = "<s:Envelope xmlns:s=\"http://www.w3.org/2003/05/soap-envelope\">\
<s:Body><s:Fault/></s:Body></s:Envelope>";
        assert!(matches!(soap_fault(xml), Some(DriverError::Protocol(_))));
    }

    #[test]
    fn local_name_strips_the_namespace_prefix() {
        assert_eq!(local_name("ter:NotAuthorized"), "NotAuthorized");
        assert_eq!(
            local_name("wsse:FailedAuthentication"),
            "FailedAuthentication"
        );
        // No prefix: the whole text is the local name.
        assert_eq!(local_name("NotAuthorized"), "NotAuthorized");
    }

    // ── list_channels: every profile, 1-based index, one GetProfiles call ───
    #[tokio::test]
    async fn list_channels_lists_every_profile_with_its_ordinal_index_and_name() {
        let (client, driver) = driver_with(vec![
            Ok(PROFILES_RESPONSE.to_owned()),
            Ok(PTZ_CONFIGURATIONS_RESPONSE.to_owned()),
        ]);
        let channels = driver.list_channels().await.expect("list_channels");
        assert_eq!(
            channels,
            vec![
                Channel {
                    index: 1,
                    name: Some("mainstream".to_owned()),
                    enabled: None,
                    ptz_support: PtzSupport::Supported,
                    recording: None,
                },
                Channel {
                    index: 2,
                    name: Some("substream".to_owned()),
                    enabled: None,
                    ptz_support: PtzSupport::Unsupported,
                    recording: None,
                },
            ]
        );
        // One GetProfiles plus one read-only GetConfigurations resolves every channel.
        let calls = client.calls();
        assert_eq!(calls.len(), 2);
        assert!(calls[0].1.contains("GetProfiles"));
        assert!(calls[1].1.contains("GetConfigurations"));
    }

    #[tokio::test]
    async fn list_channels_returns_unknown_when_ptz_configuration_read_fails() {
        let (_client, driver) = driver_with(vec![
            Ok(PROFILES_RESPONSE.to_owned()),
            Err(DriverError::Transport("read failed".to_owned())),
        ]);
        let channels = driver
            .list_channels()
            .await
            .expect("profiles remain usable");
        assert!(
            channels
                .iter()
                .all(|channel| channel.ptz_support == PtzSupport::Unknown)
        );
    }

    #[test]
    fn ptz_support_is_unknown_for_ambiguous_or_dangling_configuration_links() {
        let catalog = PtzConfigurationCatalog::Valid(vec!["PTZ_1".to_owned()]);
        assert_eq!(
            classify_ptz_support(&ProfilePtzConfiguration::Ambiguous, Some(&catalog)),
            PtzSupport::Unknown
        );
        assert_eq!(
            classify_ptz_support(
                &ProfilePtzConfiguration::Token("missing".to_owned()),
                Some(&catalog),
            ),
            PtzSupport::Unknown
        );
        assert_eq!(
            classify_ptz_support(
                &ProfilePtzConfiguration::Token("PTZ_1".to_owned()),
                Some(&PtzConfigurationCatalog::Ambiguous),
            ),
            PtzSupport::Unknown
        );
    }

    // ── substream_uri: positional convention picks the second profile ───────
    #[tokio::test]
    async fn substream_uri_prefers_the_second_profile_when_two_are_advertised() {
        let (client, driver) = driver_with(vec![
            Ok(PROFILES_RESPONSE.to_owned()),
            Ok(STREAM_URI_RESPONSE.to_owned()),
        ]);
        let stream = driver.substream_uri(None).await.expect("substream_uri");
        assert_eq!(stream.uri, "rtsp://device.test:554/Streaming/Channels/101");

        let calls = client.calls();
        assert_eq!(calls.len(), 2);
        assert!(calls[0].1.contains("GetProfiles"));
        assert!(
            calls[1]
                .1
                .contains("<trt:ProfileToken>Profile_2</trt:ProfileToken>")
        );
    }

    #[tokio::test]
    async fn substream_uri_falls_back_to_the_first_profile_when_only_one_is_advertised() {
        const SINGLE_PROFILE_RESPONSE: &str = "\
<s:Envelope xmlns:s=\"http://www.w3.org/2003/05/soap-envelope\" \
xmlns:trt=\"http://www.onvif.org/ver10/media/wsdl\" \
xmlns:tt=\"http://www.onvif.org/ver10/schema\">\
<s:Body>\
<trt:GetProfilesResponse>\
<trt:Profiles token=\"Profile_1\" fixed=\"true\"><tt:Name>mainstream</tt:Name></trt:Profiles>\
</trt:GetProfilesResponse>\
</s:Body>\
</s:Envelope>";
        let (client, driver) = driver_with(vec![
            Ok(SINGLE_PROFILE_RESPONSE.to_owned()),
            Ok(STREAM_URI_RESPONSE.to_owned()),
        ]);
        driver.substream_uri(None).await.expect("substream_uri");

        let calls = client.calls();
        assert!(
            calls[1]
                .1
                .contains("<trt:ProfileToken>Profile_1</trt:ProfileToken>")
        );
    }

    // ── resolve_profile: out-of-range / empty list both surface Unsupported ─
    #[tokio::test]
    async fn resolve_profile_is_unsupported_for_an_out_of_range_channel() {
        let (_client, driver) = driver_with(vec![Ok(PROFILES_RESPONSE.to_owned())]);
        assert!(matches!(
            driver.resolve_profile(Some(3)).await,
            Err(DriverError::Unsupported)
        ));
    }

    #[tokio::test]
    async fn resolve_profile_is_unsupported_when_the_device_advertises_no_profiles() {
        const EMPTY_PROFILES_RESPONSE: &str = "\
<s:Envelope xmlns:s=\"http://www.w3.org/2003/05/soap-envelope\" \
xmlns:trt=\"http://www.onvif.org/ver10/media/wsdl\">\
<s:Body><trt:GetProfilesResponse/></s:Body>\
</s:Envelope>";
        let (_client, driver) = driver_with(vec![Ok(EMPTY_PROFILES_RESPONSE.to_owned())]);
        assert!(matches!(
            driver.resolve_profile(None).await,
            Err(DriverError::Unsupported)
        ));
    }

    #[test]
    fn ptz_configuration_catalog_is_ambiguous_without_expected_response_envelope() {
        assert!(matches!(
            parse_ptz_configuration_catalog("<s:Envelope><s:Body/></s:Envelope>"),
            PtzConfigurationCatalog::Ambiguous
        ));
        assert!(matches!(
            parse_ptz_configuration_catalog(
                "<s:Envelope><s:Body><tptz:Ack/></s:Body></s:Envelope>"
            ),
            PtzConfigurationCatalog::Ambiguous
        ));
    }

    #[test]
    fn ptz_configuration_catalog_accepts_verified_empty_response() {
        assert!(matches!(
            parse_ptz_configuration_catalog(PTZ_CONFIGURATIONS_EMPTY_RESPONSE),
            PtzConfigurationCatalog::Valid(tokens) if tokens.is_empty()
        ));
    }

    // ── PTZ fixtures ──────────────────────────────────────────────────────
    const PTZ_CONFIGURATIONS_RESPONSE: &str = "\
<s:Envelope xmlns:s=\"http://www.w3.org/2003/05/soap-envelope\" \
xmlns:tptz=\"http://www.onvif.org/ver20/ptz/wsdl\" \
xmlns:tt=\"http://www.onvif.org/ver10/schema\">\
<s:Body>\
<tptz:GetConfigurationsResponse>\
<tptz:PTZConfiguration token=\"PTZ_1\">\
<tt:Name>PTZConfig</tt:Name>\
<tt:DefaultContinuousPanTiltVelocitySpace>\
http://www.onvif.org/ver10/tptz/PanTiltSpaces/ContinuousVelocityGenericSpace\
</tt:DefaultContinuousPanTiltVelocitySpace>\
<tt:DefaultContinuousZoomVelocitySpace>\
http://www.onvif.org/ver10/tptz/ZoomSpaces/ContinuousVelocityGenericSpace\
</tt:DefaultContinuousZoomVelocitySpace>\
</tptz:PTZConfiguration>\
</tptz:GetConfigurationsResponse>\
</s:Body>\
</s:Envelope>";

    const PTZ_CONFIGURATIONS_NO_SPACES_RESPONSE: &str = "\
<s:Envelope xmlns:s=\"http://www.w3.org/2003/05/soap-envelope\" \
xmlns:tptz=\"http://www.onvif.org/ver20/ptz/wsdl\">\
<s:Body>\
<tptz:GetConfigurationsResponse>\
<tptz:PTZConfiguration token=\"PTZ_1\"/>\
</tptz:GetConfigurationsResponse>\
</s:Body>\
</s:Envelope>";

    const PTZ_CONFIGURATIONS_EMPTY_RESPONSE: &str = "\
<s:Envelope xmlns:s=\"http://www.w3.org/2003/05/soap-envelope\" \
xmlns:tptz=\"http://www.onvif.org/ver20/ptz/wsdl\">\
<s:Body>\
<tptz:GetConfigurationsResponse/>\
</s:Body>\
</s:Envelope>";

    const PTZ_ACK_RESPONSE: &str = "\
<s:Envelope xmlns:s=\"http://www.w3.org/2003/05/soap-envelope\" \
xmlns:tptz=\"http://www.onvif.org/ver20/ptz/wsdl\">\
<s:Body><tptz:Ack/></s:Body>\
</s:Envelope>";

    fn move_vector() -> PtzVector {
        PtzVector {
            pan: 0.5,
            tilt: -1.5,
            zoom: 0.25,
        }
    }

    // ── PTZ: Move happy path — velocity formatting + echoed space URIs ──────
    #[tokio::test]
    async fn ptz_move_sends_continuous_move_with_formatted_velocities_and_declared_spaces() {
        let (client, driver) = driver_with(vec![
            Ok(PTZ_CONFIGURATIONS_RESPONSE.to_owned()),
            Ok(PROFILES_RESPONSE.to_owned()),
            Ok(PTZ_ACK_RESPONSE.to_owned()),
        ]);
        driver
            .ptz(1, PtzCommand::Move(move_vector()))
            .await
            .expect("ptz move");

        let calls = client.calls();
        assert_eq!(calls.len(), 3);
        assert!(calls[0].1.contains("GetConfigurations"));
        assert!(calls[1].1.contains("GetProfiles"));
        let body = &calls[2].1;
        assert!(body.contains("ContinuousMove"));
        assert!(body.contains("<tptz:ProfileToken>Profile_1</tptz:ProfileToken>"));
        // pan 0.5 (unclamped), tilt -1.5 clamped to -1.0, zoom 0.25, all to 6 decimals.
        assert!(body.contains("x=\"0.500000\" y=\"-1.000000\""));
        assert!(body.contains("<tt:Zoom x=\"0.250000\""));
        assert!(body.contains(
            "space=\"http://www.onvif.org/ver10/tptz/PanTiltSpaces/ContinuousVelocityGenericSpace\""
        ));
        assert!(body.contains(
            "space=\"http://www.onvif.org/ver10/tptz/ZoomSpaces/ContinuousVelocityGenericSpace\""
        ));
    }

    // ── PTZ: Move with no declared space omits the space attribute ──────────
    #[tokio::test]
    async fn ptz_move_omits_the_space_attribute_when_the_device_declares_none() {
        let (client, driver) = driver_with(vec![
            Ok(PTZ_CONFIGURATIONS_NO_SPACES_RESPONSE.to_owned()),
            Ok(PROFILES_RESPONSE.to_owned()),
            Ok(PTZ_ACK_RESPONSE.to_owned()),
        ]);
        driver
            .ptz(1, PtzCommand::Move(move_vector()))
            .await
            .expect("ptz move");
        let body = &client.calls()[2].1;
        assert!(!body.contains("space=\""));
    }

    // ── PTZ: Stop skips the GetConfigurations preflight entirely ────────────
    #[tokio::test]
    async fn ptz_stop_skips_the_get_configurations_preflight() {
        let (client, driver) = driver_with(vec![
            Ok(PROFILES_RESPONSE.to_owned()),
            Ok(PTZ_ACK_RESPONSE.to_owned()),
        ]);
        driver.ptz(1, PtzCommand::Stop).await.expect("ptz stop");

        let calls = client.calls();
        assert_eq!(calls.len(), 2);
        assert!(calls[0].1.contains("GetProfiles"));
        assert!(!calls[0].1.contains("GetConfigurations"));
        assert!(!calls[1].1.contains("GetConfigurations"));
        assert!(calls[1].1.contains("<tptz:Stop"));
        assert!(
            calls[1]
                .1
                .contains("<tptz:ProfileToken>Profile_1</tptz:ProfileToken>")
        );
    }

    // ── PTZ: Preset sends the stringified preset id ──────────────────────────
    #[tokio::test]
    async fn ptz_preset_sends_the_stringified_preset_id() {
        let (client, driver) = driver_with(vec![
            Ok(PTZ_CONFIGURATIONS_RESPONSE.to_owned()),
            Ok(PROFILES_RESPONSE.to_owned()),
            Ok(PTZ_ACK_RESPONSE.to_owned()),
        ]);
        driver
            .ptz(1, PtzCommand::Preset { id: 7 })
            .await
            .expect("ptz preset");

        let calls = client.calls();
        assert_eq!(calls.len(), 3);
        assert!(
            calls[2]
                .1
                .contains("<tptz:PresetToken>7</tptz:PresetToken>")
        );
    }

    // ── PTZ: Move/Preset fold "no PTZ" into Unsupported ──────────────────────
    #[tokio::test]
    async fn ptz_move_is_unsupported_when_no_ptz_configuration_is_declared() {
        let (_client, driver) = driver_with(vec![Ok(PTZ_CONFIGURATIONS_EMPTY_RESPONSE.to_owned())]);
        assert!(matches!(
            driver.ptz(1, PtzCommand::Move(move_vector())).await,
            Err(DriverError::Unsupported)
        ));
    }

    #[tokio::test]
    async fn ptz_move_is_unsupported_when_get_configurations_faults_action_not_supported() {
        let (_client, driver) =
            driver_with(vec![Ok(ACTION_NOT_SUPPORTED_FAULT_RESPONSE.to_owned())]);
        assert!(matches!(
            driver.ptz(1, PtzCommand::Move(move_vector())).await,
            Err(DriverError::Unsupported)
        ));
    }

    // A non-SOAP-fault `Device` error (e.g. the PTZ service path 404s) folds
    // into `Unsupported` exactly like a `Protocol` SOAP fault does.
    #[tokio::test]
    async fn ptz_move_is_unsupported_when_get_configurations_returns_a_device_error() {
        let (_client, driver) = driver_with(vec![Err(DriverError::Device("HTTP 404".to_owned()))]);
        assert!(matches!(
            driver.ptz(1, PtzCommand::Move(move_vector())).await,
            Err(DriverError::Unsupported)
        ));
    }

    #[tokio::test]
    async fn ptz_preset_is_unsupported_when_no_ptz_configuration_is_declared() {
        let (_client, driver) = driver_with(vec![Ok(PTZ_CONFIGURATIONS_EMPTY_RESPONSE.to_owned())]);
        assert!(matches!(
            driver.ptz(1, PtzCommand::Preset { id: 1 }).await,
            Err(DriverError::Unsupported)
        ));
    }

    // ── PTZ: Auth/Transport from GetConfigurations propagate UNCHANGED ──────
    #[tokio::test]
    async fn ptz_move_propagates_an_auth_fault_from_get_configurations_unchanged() {
        let (_client, driver) = driver_with(vec![Ok(NOT_AUTHORIZED_FAULT_RESPONSE.to_owned())]);
        assert!(matches!(
            driver.ptz(1, PtzCommand::Move(move_vector())).await,
            Err(DriverError::Auth)
        ));
    }

    #[tokio::test]
    async fn ptz_move_propagates_a_transport_error_from_get_configurations_unchanged() {
        let (_client, driver) = driver_with(vec![Err(DriverError::Transport(
            "connection reset".to_owned(),
        ))]);
        assert!(matches!(
            driver.ptz(1, PtzCommand::Move(move_vector())).await,
            Err(DriverError::Transport(_))
        ));
    }

    // ── Pure parser/builder unit tests ───────────────────────────────────────
    #[test]
    fn continuous_move_body_clamps_velocities_and_formats_six_decimals() {
        let spaces = PtzVelocitySpaces {
            pan_tilt: Some("space://pan-tilt".to_owned()),
            zoom: None,
        };
        let body = continuous_move_body(
            "Profile_1",
            PtzVector {
                pan: 2.0,
                tilt: -2.0,
                zoom: 0.123_456_79,
            },
            &spaces,
        );
        assert!(body.contains("<tptz:ProfileToken>Profile_1</tptz:ProfileToken>"));
        // pan/tilt clamped to +-1.0; the declared pan/tilt space is echoed.
        assert!(body.contains("x=\"1.000000\" y=\"-1.000000\" space=\"space://pan-tilt\""));
        // zoom is within range and rounds to 6 decimals; no declared zoom space.
        assert!(body.contains("<tt:Zoom x=\"0.123457\"/>"));
    }

    #[test]
    fn space_attr_is_empty_for_none_and_escaped_for_some() {
        assert_eq!(space_attr(None), "");
        assert_eq!(
            space_attr(Some("http://x/y?a=1&b=2")),
            " space=\"http://x/y?a=1&amp;b=2\""
        );
    }

    #[test]
    fn parse_profiles_rejects_a_wrong_response_wrapper() {
        let xml = "<s:Envelope><s:Body><trt:UnrelatedResponse><trt:Profiles token=\"Profile_1\"><tt:PTZConfiguration token=\"PTZ_1\"/></trt:Profiles></trt:UnrelatedResponse></s:Body></s:Envelope>";
        assert!(parse_profiles(xml).is_empty());
    }

    #[test]
    fn parse_profiles_does_not_attach_a_sibling_ptz_configuration() {
        let xml = "<s:Envelope><s:Body><trt:GetProfilesResponse><trt:Profiles token=\"Profile_1\"><tt:Name>fixed</tt:Name></trt:Profiles><tt:PTZConfiguration token=\"PTZ_1\"/></trt:GetProfilesResponse></s:Body></s:Envelope>";
        let profiles = parse_profiles(xml);
        assert_eq!(profiles.len(), 1);
        assert!(matches!(
            profiles[0].ptz_configuration,
            ProfilePtzConfiguration::Absent
        ));
    }

    #[test]
    fn parse_profiles_extracts_every_profile_token_and_name_in_order() {
        let profiles = parse_profiles(PROFILES_RESPONSE);
        assert_eq!(profiles.len(), 2);
        assert_eq!(profiles[0].token, "Profile_1");
        assert_eq!(profiles[0].name.as_deref(), Some("mainstream"));
        assert_eq!(profiles[1].token, "Profile_2");
        assert_eq!(profiles[1].name.as_deref(), Some("substream"));
    }
}
