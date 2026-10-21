//! HTTP transport seam for CGI/ISAPI device families, with RFC 2617 Digest auth.
//!
//! Families like Dahua (`/cgi-bin/...`) and Hikvision (`/ISAPI/...`) speak a
//! plain HTTP request/response management plane behind HTTP Digest
//! authentication. A driver depends only on the [`HttpDeviceClient`] port; the
//! concrete [`ReqwestDigestClient`] performs the two-step Digest handshake
//! (unauthenticated probe → `401` challenge → authenticated retry) and never
//! logs the password.
//!
//! Like [`DirectTcpTransport`](super::transport::DirectTcpTransport), this is the
//! dev/embedded path: it dials the device's HTTP port directly, which is only
//! correct where the backend can reach the device network. The production path
//! routes the same requests through an edge proxy node (as RDP/VNC/SSH do) and
//! lands in a later phase; because drivers depend on the [`HttpDeviceClient`]
//! trait, they will not change when it does.
//!
//! The client follows **no HTTP redirects** ([`reqwest::redirect::Policy::none`],
//! set once in [`ReqwestDigestClient::new`]). This is deliberate and applies to
//! every method, not just the snapshot fetch: a device-management CGI/ISAPI/SOAP
//! endpoint never legitimately 3xx-redirects, and refusing to follow one closes
//! the SSRF-via-`Location` pivot on
//! [`get_image_from_url`](HttpDeviceClient::get_image_from_url), whose target URL is
//! chosen by the device. A device that does redirect a management call surfaces
//! it as a [`DriverError::Device`] rather than being followed to an arbitrary
//! host.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::pin::Pin;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use md5::{Digest, Md5};
use uuid::Uuid;

use crate::contexts::device_management::domain::{DriverError, Secret};
use crate::contexts::device_management::infrastructure::transport::DeviceEndpoint;

/// A streamed byte body — used for large device file downloads (e.g. Dahua
/// `loadfile.cgi`) that must NOT be buffered whole (an hour of footage). Errors
/// are normalized to [`std::io::Error`] so the stream can feed a subprocess.
pub type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>>;

/// Upper bound on one response body, mirroring the DVRIP driver's discipline so a
/// rogue or buggy device cannot force an unbounded allocation.
const MAX_RESPONSE_LEN: usize = 8 * 1024 * 1024;
/// Per-request timeout so a device that stalls cannot hang the calling task.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// Inter-chunk read timeout for the streaming download path. Unlike
/// [`REQUEST_TIMEOUT`] this bounds the gap *between* body chunks, not the total
/// transfer — an hour-long file download is fine as long as bytes keep flowing,
/// but a device that goes silent mid-stream is torn down.
const DOWNLOAD_READ_TIMEOUT: Duration = Duration::from_secs(30);
/// Connect timeout for the streaming download client (matches [`REQUEST_TIMEOUT`]'s
/// conservative dial ceiling; the total transfer is deliberately unbounded).
const DOWNLOAD_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Nonce-count for the single authenticated request we send per operation. We
/// never reuse a client nonce across requests, so the count is always the first.
const NONCE_COUNT: &str = "00000001";
/// JPEG Start-Of-Image marker: every JPEG file begins with these two bytes. Used
/// to reject a non-image body (e.g. an HTML error page) from a snapshot endpoint.
const JPEG_SOI: [u8; 2] = [0xFF, 0xD8];

/// A minimal HTTP client for text CGI/ISAPI endpoints, keyed on the request path.
///
/// `path` is the CGI path plus query, e.g.
/// `/cgi-bin/magicBox.cgi?action=getSystemInfo`. Implementations own the base
/// URL and any authentication.
#[async_trait]
pub trait HttpDeviceClient: Send + Sync {
    /// Perform an HTTP `GET` of `path` and return the response body as text.
    ///
    /// # Errors
    ///
    /// Returns [`DriverError::Auth`] when the device rejects authentication,
    /// [`DriverError::Transport`] on a connection/read failure,
    /// [`DriverError::Protocol`] when the body is oversized or not valid UTF-8,
    /// and [`DriverError::Device`] for other non-success HTTP statuses.
    async fn get_text(&self, path: &str) -> Result<String, DriverError>;

    /// Perform an HTTP `GET` of `path` and return the response body as raw bytes,
    /// validated to be a JPEG still image.
    ///
    /// This is the still-image seam for the CGI/ISAPI snapshot endpoints
    /// (Dahua `snapshot.cgi`, Hikvision `.../picture`). The body is capped like
    /// [`get_text`](HttpDeviceClient::get_text) and must begin with the JPEG
    /// Start-Of-Image marker (`0xFF 0xD8`); a non-image body (e.g. an HTML error
    /// page) is rejected with [`DriverError::Device`] rather than returned as a
    /// bogus image.
    ///
    /// The default rejects the call with [`DriverError::Unsupported`] so that a
    /// read-only fake keeps compiling; a real client overrides it.
    ///
    /// # Errors
    ///
    /// Mirrors [`get_text`](HttpDeviceClient::get_text), plus
    /// [`DriverError::Device`] when the body is not a JPEG; the default always
    /// returns [`DriverError::Unsupported`].
    async fn get_bytes(&self, path: &str) -> Result<Vec<u8>, DriverError> {
        let _ = path;
        Err(DriverError::Unsupported)
    }

    /// Fetch a JPEG still from a **device-provided absolute URL**, returning the
    /// raw bytes so the caller never hands a credential-bearing URI to a client.
    ///
    /// This is the ONVIF snapshot seam: `GetSnapshotUri` yields an absolute URL
    /// (`http(s)://host/...`) that the backend must fetch server-side and proxy as
    /// bytes, exactly as the native families already do via
    /// [`get_bytes`](HttpDeviceClient::get_bytes). Because the URL is chosen by the
    /// device, the implementation MUST constrain egress with an SSRF fence
    /// (http(s) scheme + exact device-host match) before dialing. The body is
    /// capped like [`get_bytes`](HttpDeviceClient::get_bytes) and validated to be a JPEG.
    ///
    /// The default rejects the call with [`DriverError::Unsupported`] so a
    /// read-only fake keeps compiling; a real client overrides it.
    ///
    /// # Errors
    ///
    /// [`DriverError::Protocol`] when the URL is malformed or its scheme is not
    /// http(s); [`DriverError::Device`] when the host is not the device's own host
    /// or the body is not a JPEG; otherwise mirrors
    /// [`get_bytes`](HttpDeviceClient::get_bytes). The default always returns
    /// [`DriverError::Unsupported`].
    async fn get_image_from_url(&self, url: &str) -> Result<Vec<u8>, DriverError> {
        let _ = url;
        Err(DriverError::Unsupported)
    }

    /// Perform an HTTP `POST` of `body` (typed `content_type`) to `path` and
    /// return the response body as text.
    ///
    /// The default rejects the call with [`DriverError::Unsupported`] so that a
    /// read-only fake (e.g. a test double implementing only [`get_text`]) keeps
    /// compiling; a real client overrides it.
    ///
    /// [`get_text`]: HttpDeviceClient::get_text
    ///
    /// # Errors
    ///
    /// Mirrors [`get_text`](HttpDeviceClient::get_text); the default always
    /// returns [`DriverError::Unsupported`].
    async fn post_text(
        &self,
        path: &str,
        body: &str,
        content_type: &str,
    ) -> Result<String, DriverError> {
        let _ = (path, body, content_type);
        Err(DriverError::Unsupported)
    }

    /// Perform an HTTP `DELETE` of `path` and return the response body as text.
    ///
    /// The default rejects the call with [`DriverError::Unsupported`] so that a
    /// read-only fake keeps compiling; a real client overrides it.
    ///
    /// # Errors
    ///
    /// Mirrors [`get_text`](HttpDeviceClient::get_text); the default always
    /// returns [`DriverError::Unsupported`].
    async fn delete(&self, path: &str) -> Result<String, DriverError> {
        let _ = path;
        Err(DriverError::Unsupported)
    }

    /// Perform an HTTP `PUT` of `body` (typed `content_type`) to `path` and
    /// return the response body as text.
    ///
    /// The ISAPI control plane (e.g. Hikvision PTZ `.../continuous`) uses `PUT`.
    /// The default rejects the call with [`DriverError::Unsupported`] so that a
    /// read-only fake keeps compiling; a real client overrides it.
    ///
    /// # Errors
    ///
    /// Mirrors [`get_text`](HttpDeviceClient::get_text); the default always
    /// returns [`DriverError::Unsupported`].
    async fn put_text(
        &self,
        path: &str,
        body: &str,
        content_type: &str,
    ) -> Result<String, DriverError> {
        let _ = (path, body, content_type);
        Err(DriverError::Unsupported)
    }

    /// Perform an HTTP `GET` of `path` and STREAM the response body.
    ///
    /// Unlike [`get_bytes`](HttpDeviceClient::get_bytes) (which buffers a bounded
    /// snapshot), this is for large device file downloads (Dahua `loadfile.cgi`)
    /// that may be gigabytes: the body is streamed, never buffered whole. The
    /// caller is responsible for bounding it (a subprocess `-t`, a permit, and
    /// the transport's inter-chunk read timeout guard against a stalled device).
    ///
    /// # Errors
    ///
    /// Mirrors [`get_bytes`](HttpDeviceClient::get_bytes); the default returns
    /// [`DriverError::Unsupported`] so a read-only fake keeps compiling.
    async fn download_stream(&self, path: &str) -> Result<ByteStream, DriverError> {
        let _ = path;
        Err(DriverError::Unsupported)
    }

    /// Perform an HTTP `POST` of `body` (typed `content_type`) to `path` and
    /// STREAM the response body.
    ///
    /// Some device file downloads are addressed by a request BODY rather than a
    /// query string (Hikvision ISAPI `ContentMgmt/download` takes a
    /// `downloadRequest` XML envelope naming the segment to pull), so this
    /// mirrors [`download_stream`](Self::download_stream) with a POST body
    /// instead of GET. The default rejects the call with
    /// [`DriverError::Unsupported`] so a read-only fake keeps compiling.
    ///
    /// # Errors
    ///
    /// Mirrors [`get_bytes`](HttpDeviceClient::get_bytes); the default returns
    /// [`DriverError::Unsupported`].
    async fn download_stream_post(
        &self,
        path: &str,
        body: &str,
        content_type: &str,
    ) -> Result<ByteStream, DriverError> {
        let _ = (path, body, content_type);
        Err(DriverError::Unsupported)
    }
}

/// An [`HttpDeviceClient`] that talks plain HTTP with RFC 2617 Digest auth.
///
/// The device's HTTP port is taken from the resolved [`DeviceEndpoint`]. Note the
/// CGI plane listens on the device's HTTP port (typically 80/443), which is *not*
/// the same as a family's native binary management port (e.g. Dahua DHIP on
/// 37777); the endpoint must resolve to the HTTP port for this client to work.
#[derive(Debug)]
pub struct ReqwestDigestClient {
    /// Scheme + authority, e.g. `http://192.0.2.10:80`, without a trailing slash.
    base_url: String,
    /// The resolved device host, kept separately as the SSRF fence for
    /// [`get_image_from_url`](HttpDeviceClient::get_image_from_url): a
    /// device-provided snapshot URL may only egress to this exact host.
    host: String,
    username: String,
    password: Secret,
    client: reqwest::Client,
    /// A second client for large file downloads: NO total request timeout (an
    /// hour-long clip must not be cut off), only a connect timeout and an
    /// inter-chunk [`read_timeout`](reqwest::ClientBuilder::read_timeout) stall
    /// guard. Kept separate so the bounded [`client`](Self::client) still caps
    /// every management call at [`REQUEST_TIMEOUT`].
    download_client: reqwest::Client,
}

impl ReqwestDigestClient {
    /// Build a client for `endpoint` authenticating as `username`/`password`.
    ///
    /// # Errors
    ///
    /// Returns [`DriverError::Transport`] if the underlying HTTP client cannot be
    /// constructed (e.g. the TLS backend fails to initialize).
    pub fn new(
        endpoint: &DeviceEndpoint,
        username: impl Into<String>,
        password: Secret,
    ) -> Result<Self, DriverError> {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            // Device management flows never legitimately redirect; refusing to
            // follow 3xx closes the SSRF-via-redirect hole on the device-provided
            // snapshot URL (a device could 302 to an internal address).
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| DriverError::Transport(error.to_string()))?;
        // Same redirect/no-follow discipline, but no *total* timeout — a large
        // file download (an hour of footage) legitimately runs for minutes. The
        // read-timeout bounds a mid-stream stall instead.
        let download_client = reqwest::Client::builder()
            .connect_timeout(DOWNLOAD_CONNECT_TIMEOUT)
            .read_timeout(DOWNLOAD_READ_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| DriverError::Transport(error.to_string()))?;
        Ok(Self {
            base_url: format!("http://{}:{}", endpoint.host, endpoint.port),
            host: endpoint.host.clone(),
            username: username.into(),
            password,
            client,
            download_client,
        })
    }

    /// Build the `Authorization: Digest ...` header value for `challenge`.
    fn authorization_header(&self, challenge: &DigestChallenge, method: &str, uri: &str) -> String {
        let cnonce = Uuid::new_v4().simple().to_string();
        digest_authorization(&DigestAuthorizationInput {
            username: &self.username,
            password: self.password.expose(),
            challenge,
            method,
            uri,
            cnonce: &cnonce,
            nonce_count: NONCE_COUNT,
        })
    }

    /// Run the two-step Digest handshake for one request and return the
    /// successful [`reqwest::Response`], leaving the body read to the caller.
    ///
    /// `build_request` produces a fresh [`reqwest::RequestBuilder`] for the given
    /// method/URL each time it is called (once for the unauthenticated probe,
    /// once for the authenticated retry); `method`/`path` feed the digest `A2`
    /// term and must match the request line. This is the shared plumbing behind
    /// `GET`/`POST`/`DELETE` (text) and `GET` (bytes) so the probe → `401`
    /// challenge → authenticated-retry flow lives in exactly one place; each
    /// caller then applies its own bounded read.
    async fn send_with_digest<F>(
        &self,
        method: &str,
        path: &str,
        build_request: F,
    ) -> Result<reqwest::Response, DriverError>
    where
        F: Fn() -> reqwest::RequestBuilder,
    {
        // First, an unauthenticated probe: most devices answer 401 with a Digest
        // challenge, but some open endpoints answer 200 directly.
        let probe = build_request().send().await.map_err(transport_error)?;
        let probe_status = probe.status();
        if probe_status.is_success() {
            return Ok(probe);
        }
        if probe_status != reqwest::StatusCode::UNAUTHORIZED {
            return Err(status_error(probe_status));
        }

        // The device demanded authentication: parse the Digest challenge and
        // retry once with the computed response.
        let challenge = digest_challenge(&probe).ok_or(DriverError::Auth)?;
        let authorization = self.authorization_header(&challenge, method, path);
        let authenticated = build_request()
            .header(reqwest::header::AUTHORIZATION, authorization)
            .send()
            .await
            .map_err(transport_error)?;
        let status = authenticated.status();
        if status.is_success() {
            return Ok(authenticated);
        }
        Err(status_error(status))
    }

    /// Run the Digest handshake and read the successful response as bounded text.
    async fn request_with_digest<F>(
        &self,
        method: &str,
        path: &str,
        build_request: F,
    ) -> Result<String, DriverError>
    where
        F: Fn() -> reqwest::RequestBuilder,
    {
        let response = self.send_with_digest(method, path, build_request).await?;
        read_bounded_text(response).await
    }
}

#[async_trait]
impl HttpDeviceClient for ReqwestDigestClient {
    async fn get_text(&self, path: &str) -> Result<String, DriverError> {
        let url = format!("{}{}", self.base_url, path);
        self.request_with_digest("GET", path, || self.client.get(&url))
            .await
    }

    async fn get_bytes(&self, path: &str) -> Result<Vec<u8>, DriverError> {
        let url = format!("{}{}", self.base_url, path);
        let response = self
            .send_with_digest("GET", path, || self.client.get(&url))
            .await?;
        let bytes = read_bounded(response).await?;
        ensure_jpeg(&bytes)?;
        Ok(bytes)
    }

    async fn get_image_from_url(&self, url: &str) -> Result<Vec<u8>, DriverError> {
        let parsed = reqwest::Url::parse(url)
            .map_err(|error| DriverError::Protocol(format!("invalid snapshot URL: {error}")))?;
        // SSRF fence FIRST: the URL is chosen by the device, so constrain egress
        // to an http(s) scheme on the device's own host before dialing anything.
        let request_uri = snapshot_request_uri(&parsed, &self.host)?;
        // Resolve exactly once, then override reqwest's resolver with those socket
        // addresses. The Digest probe and retry both use this one client, so a DNS
        // rebinding after the host fence cannot redirect either request elsewhere.
        let snapshot_client = snapshot_client(&parsed).await?;
        let response = self
            .send_with_digest("GET", &request_uri, || snapshot_client.get(parsed.clone()))
            .await?;
        let bytes = read_bounded(response).await?;
        ensure_jpeg(&bytes)?;
        Ok(bytes)
    }

    async fn post_text(
        &self,
        path: &str,
        body: &str,
        content_type: &str,
    ) -> Result<String, DriverError> {
        let url = format!("{}{}", self.base_url, path);
        self.request_with_digest("POST", path, || {
            self.client
                .post(&url)
                .header(reqwest::header::CONTENT_TYPE, content_type)
                .body(body.to_owned())
        })
        .await
    }

    async fn delete(&self, path: &str) -> Result<String, DriverError> {
        let url = format!("{}{}", self.base_url, path);
        self.request_with_digest("DELETE", path, || self.client.delete(&url))
            .await
    }

    async fn put_text(
        &self,
        path: &str,
        body: &str,
        content_type: &str,
    ) -> Result<String, DriverError> {
        let url = format!("{}{}", self.base_url, path);
        self.request_with_digest("PUT", path, || {
            self.client
                .put(&url)
                .header(reqwest::header::CONTENT_TYPE, content_type)
                .body(body.to_owned())
        })
        .await
    }

    async fn download_stream(&self, path: &str) -> Result<ByteStream, DriverError> {
        let url = format!("{}{}", self.base_url, path);
        // Reuse the Digest handshake, but dial with the un-timed download client
        // so the streamed body is not cut off by `REQUEST_TIMEOUT`.
        let response = self
            .send_with_digest("GET", path, || self.download_client.get(&url))
            .await?;
        let stream = response
            .bytes_stream()
            .map(|chunk| chunk.map_err(std::io::Error::other));
        Ok(Box::pin(stream))
    }

    async fn download_stream_post(
        &self,
        path: &str,
        body: &str,
        content_type: &str,
    ) -> Result<ByteStream, DriverError> {
        let url = format!("{}{}", self.base_url, path);
        let response = self
            .send_with_digest("POST", path, || {
                self.download_client
                    .post(&url)
                    .header(reqwest::header::CONTENT_TYPE, content_type)
                    .body(body.to_owned())
            })
            .await?;
        let stream = response
            .bytes_stream()
            .map(|chunk| chunk.map_err(std::io::Error::other));
        Ok(Box::pin(stream))
    }
}

/// Map a transport-level `reqwest` failure to a [`DriverError`].
///
/// NEVER surface `reqwest::Error`'s own string: its `Display` embeds the full
/// request URL, and some device calls (e.g. Dahua `userManager.cgi?action=addUser`)
/// carry a plaintext, trivially-reversible credential in the query string — which
/// would then flow into a client-facing REST error. Build the message from the
/// error's coarse kind + host only, never the path/query.
fn transport_error(error: reqwest::Error) -> DriverError {
    if error.is_timeout() {
        return DriverError::Timeout("device HTTP request deadline exceeded".to_owned());
    }
    if error.is_connect() {
        return DriverError::Unreachable("device HTTP connection failed".to_owned());
    }
    if error.is_body() || error.is_decode() {
        return DriverError::Unreachable("device HTTP response read failed".to_owned());
    }
    DriverError::Unreachable("device HTTP request failed".to_owned())
}

/// Classify a non-success HTTP status: auth failures are distinct from other
/// device errors so callers can react appropriately.
fn status_error(status: reqwest::StatusCode) -> DriverError {
    match status {
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => DriverError::Auth,
        other => DriverError::Device(format!("device returned HTTP {other}")),
    }
}

/// Read a response body as UTF-8 text, rejecting anything over [`MAX_RESPONSE_LEN`].
async fn read_bounded_text(response: reqwest::Response) -> Result<String, DriverError> {
    let bytes = read_bounded(response).await?;
    String::from_utf8(bytes)
        .map_err(|error| DriverError::Protocol(format!("device response was not UTF-8: {error}")))
}

/// Read a response body into memory, capping the total at [`MAX_RESPONSE_LEN`].
///
/// Both the advertised `Content-Length` and the streamed byte count are checked,
/// so a device that lies about its length (or streams chunked) still cannot force
/// an unbounded allocation.
async fn read_bounded(mut response: reqwest::Response) -> Result<Vec<u8>, DriverError> {
    if let Some(len) = response.content_length()
        && len > MAX_RESPONSE_LEN as u64
    {
        return Err(DriverError::Protocol(format!(
            "response body length {len} exceeds maximum {MAX_RESPONSE_LEN}"
        )));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(transport_error)? {
        if body.len() + chunk.len() > MAX_RESPONSE_LEN {
            return Err(DriverError::Protocol(format!(
                "response body exceeds maximum {MAX_RESPONSE_LEN}"
            )));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Validate a device-provided snapshot URL against the SSRF fence and return its
/// request-URI (path + query) for the digest `A2` term.
///
/// The URL comes from the device (ONVIF `GetSnapshotUri`), so this is the egress
/// control: the scheme must be http(s) and the host must be the SAME host the
/// device was resolved on. A hostname that differs from `expected_host` — even a
/// legitimate alias — fails closed; that is the correct trade (a broken snapshot
/// beats a server-side request forgery to an internal address). Purely structural
/// URL parsing, no natural-language matching.
fn snapshot_request_uri(url: &reqwest::Url, expected_host: &str) -> Result<String, DriverError> {
    match url.scheme() {
        "http" | "https" => {}
        other => {
            return Err(DriverError::Protocol(format!(
                "snapshot URL scheme '{other}' is not allowed"
            )));
        }
    }
    let host = url
        .host_str()
        .ok_or_else(|| DriverError::Protocol("snapshot URL has no host".to_owned()))?;
    if !host.eq_ignore_ascii_case(expected_host) {
        return Err(DriverError::Device(format!(
            "snapshot URL host '{host}' does not match the device host"
        )));
    }
    Ok(match url.query() {
        Some(query) => format!("{}?{}", url.path(), query),
        None => url.path().to_owned(),
    })
}

/// Resolve the snapshot host once and build a client pinned to exactly those
/// addresses.
///
/// The URL hostname remains in the request, preserving HTTP `Host` and TLS SNI,
/// while [`reqwest::ClientBuilder::resolve_to_addrs`] prevents a second DNS lookup
/// when the connection is opened. Snapshot URLs may use a different port from the
/// device management endpoint, so resolution uses the URL's effective port.
async fn snapshot_client(url: &reqwest::Url) -> Result<reqwest::Client, DriverError> {
    let host = url
        .host_str()
        .ok_or_else(|| DriverError::Protocol("snapshot URL has no host".to_owned()))?;
    let port = url.port_or_known_default().ok_or_else(|| {
        DriverError::Protocol("snapshot URL has no port for its scheme".to_owned())
    })?;
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|_| DriverError::Transport(format!("snapshot DNS lookup failed (host {host})")))?
        .collect();
    if addrs.is_empty() {
        return Err(DriverError::Transport(format!(
            "snapshot DNS lookup returned no addresses (host {host})"
        )));
    }
    pinned_snapshot_client(host, &addrs)
}

/// Build the no-redirect snapshot client whose resolver is pinned to `addrs`.
fn pinned_snapshot_client(
    host: &str,
    addrs: &[SocketAddr],
) -> Result<reqwest::Client, DriverError> {
    reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .resolve_to_addrs(host, addrs)
        .build()
        .map_err(|_| DriverError::Transport("snapshot HTTP client setup failed".to_owned()))
}

/// Reject a response body that is not a JPEG still image.
///
/// A snapshot endpoint should answer with `image/jpeg`, but a misconfigured or
/// unauthorized device may return an HTML error page with a `200`; validating
/// the JPEG Start-Of-Image marker (`0xFF 0xD8`) stops that page from being
/// surfaced as a bogus image.
fn ensure_jpeg(bytes: &[u8]) -> Result<(), DriverError> {
    if bytes.starts_with(&JPEG_SOI) {
        Ok(())
    } else {
        Err(DriverError::Device(
            "snapshot response was not a JPEG image".to_owned(),
        ))
    }
}

/// A parsed `WWW-Authenticate: Digest ...` challenge.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DigestChallenge {
    realm: String,
    nonce: String,
    /// The `qop` directive verbatim (e.g. `auth` or `auth,auth-int`), if present.
    qop: Option<String>,
    /// The opaque value to echo back, if the device sent one.
    opaque: Option<String>,
    /// The hash algorithm directive (`MD5` or `MD5-sess`), if present.
    algorithm: Option<String>,
}

impl DigestChallenge {
    /// Build from parsed auth-params, requiring the mandatory `realm`/`nonce`.
    fn from_params(params: &HashMap<String, String>) -> Option<Self> {
        Some(Self {
            realm: params.get("realm")?.clone(),
            nonce: params.get("nonce")?.clone(),
            qop: params.get("qop").cloned(),
            opaque: params.get("opaque").cloned(),
            algorithm: params.get("algorithm").cloned(),
        })
    }
}

/// Extract the first parseable Digest challenge from a response's headers.
fn digest_challenge(response: &reqwest::Response) -> Option<DigestChallenge> {
    response
        .headers()
        .get_all(reqwest::header::WWW_AUTHENTICATE)
        .iter()
        .find_map(|value| {
            let params = digest_params(value.to_str().ok()?)?;
            DigestChallenge::from_params(&params)
        })
}

/// Parse a `Digest`-scheme challenge header into its auth-params, or `None` if
/// the header is not a Digest challenge.
fn digest_params(header_value: &str) -> Option<HashMap<String, String>> {
    let trimmed = header_value.trim_start();
    let scheme_len = "digest".len();
    let (scheme, params) = trimmed.split_at_checked(scheme_len)?;
    if !scheme.eq_ignore_ascii_case("digest") {
        return None;
    }
    Some(parse_auth_params(params))
}

/// Parse comma-separated `key=value` auth-params.
///
/// Values may be double-quoted (and then may contain commas); bare values run to
/// the next comma. Keys are lowercased. This is structural HTTP-header parsing —
/// not natural-language matching.
fn parse_auth_params(input: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut rest = input.trim_start();
    while let Some(eq) = rest.find('=') {
        let key = rest[..eq].trim().to_ascii_lowercase();
        let after_key = rest[eq + 1..].trim_start();
        let (value, remainder) = if let Some(quoted) = after_key.strip_prefix('"') {
            match quoted.find('"') {
                Some(end) => (quoted[..end].to_owned(), &quoted[end + 1..]),
                None => (quoted.to_owned(), ""),
            }
        } else {
            match after_key.find(',') {
                Some(end) => (after_key[..end].trim().to_owned(), &after_key[end..]),
                None => (after_key.trim().to_owned(), ""),
            }
        };
        if !key.is_empty() {
            out.insert(key, value);
        }
        rest = match remainder.find(',') {
            Some(comma) => remainder[comma + 1..].trim_start(),
            None => "",
        };
    }
    out
}

/// Inputs to the RFC 2617 digest-response computation.
struct DigestResponseInput<'a> {
    username: &'a str,
    password: &'a str,
    realm: &'a str,
    nonce: &'a str,
    method: &'a str,
    uri: &'a str,
    /// The selected `qop` (`auth`), or `None` for the legacy RFC 2069 form.
    qop: Option<&'a str>,
    nonce_count: &'a str,
    cnonce: &'a str,
    /// The algorithm directive; `MD5-sess` derives a session-scoped `HA1`.
    algorithm: Option<&'a str>,
}

/// Compute the RFC 2617 `response` value (lowercase hex MD5).
fn compute_digest_response(input: &DigestResponseInput<'_>) -> String {
    let ha1_plain = md5_hex(&format!(
        "{}:{}:{}",
        input.username, input.realm, input.password
    ));
    let ha1 = match input.algorithm {
        Some(algorithm) if algorithm.eq_ignore_ascii_case("md5-sess") => {
            md5_hex(&format!("{ha1_plain}:{}:{}", input.nonce, input.cnonce))
        }
        _ => ha1_plain,
    };
    let ha2 = md5_hex(&format!("{}:{}", input.method, input.uri));
    match input.qop {
        Some(qop) => md5_hex(&format!(
            "{ha1}:{}:{}:{}:{qop}:{ha2}",
            input.nonce, input.nonce_count, input.cnonce
        )),
        None => md5_hex(&format!("{ha1}:{}:{ha2}", input.nonce)),
    }
}

/// Lowercase-hex MD5 of `input`.
fn md5_hex(input: &str) -> String {
    hex::encode(Md5::digest(input.as_bytes()))
}

/// Inputs to the `Authorization: Digest ...` header assembly.
struct DigestAuthorizationInput<'a> {
    username: &'a str,
    password: &'a str,
    challenge: &'a DigestChallenge,
    method: &'a str,
    uri: &'a str,
    cnonce: &'a str,
    nonce_count: &'a str,
}

/// Assemble the full `Authorization: Digest ...` header value.
fn digest_authorization(input: &DigestAuthorizationInput<'_>) -> String {
    let challenge = input.challenge;
    let qop = challenge.qop.as_deref().and_then(select_auth_qop);
    let response = compute_digest_response(&DigestResponseInput {
        username: input.username,
        password: input.password,
        realm: &challenge.realm,
        nonce: &challenge.nonce,
        method: input.method,
        uri: input.uri,
        qop,
        nonce_count: input.nonce_count,
        cnonce: input.cnonce,
        algorithm: challenge.algorithm.as_deref(),
    });

    let mut parts = vec![
        format!("username=\"{}\"", input.username),
        format!("realm=\"{}\"", challenge.realm),
        format!("nonce=\"{}\"", challenge.nonce),
        format!("uri=\"{}\"", input.uri),
        format!("response=\"{response}\""),
    ];
    if let Some(algorithm) = challenge.algorithm.as_deref() {
        parts.push(format!("algorithm={algorithm}"));
    }
    if qop.is_some() {
        parts.push("qop=auth".to_owned());
        parts.push(format!("nc={}", input.nonce_count));
        parts.push(format!("cnonce=\"{}\"", input.cnonce));
    }
    if let Some(opaque) = challenge.opaque.as_deref() {
        parts.push(format!("opaque=\"{opaque}\""));
    }
    format!("Digest {}", parts.join(", "))
}

/// Return `Some("auth")` when the device's `qop` directive offers `auth`.
fn select_auth_qop(qop: &str) -> Option<&'static str> {
    qop.split(',')
        .any(|token| token.trim().eq_ignore_ascii_case("auth"))
        .then_some("auth")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    #[derive(Debug)]
    struct CapturedRequest {
        request_line: String,
        headers: HashMap<String, String>,
        body: Vec<u8>,
    }

    impl CapturedRequest {
        fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .get(&name.to_ascii_lowercase())
                .map(String::as_str)
        }
    }

    async fn read_request(stream: &mut TcpStream) -> CapturedRequest {
        let mut bytes = Vec::new();
        let header_end = loop {
            if let Some(position) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                break position;
            }
            let mut chunk = [0_u8; 4096];
            let read = stream.read(&mut chunk).await.expect("read HTTP request");
            assert!(read > 0, "connection closed before the request headers");
            bytes.extend_from_slice(&chunk[..read]);
        };

        let head = std::str::from_utf8(&bytes[..header_end]).expect("request headers are ASCII");
        let mut lines = head.split("\r\n");
        let request_line = lines.next().expect("request line").to_owned();
        let headers: HashMap<String, String> = lines
            .map(|line| {
                let (name, value) = line.split_once(':').expect("well-formed request header");
                (name.to_ascii_lowercase(), value.trim().to_owned())
            })
            .collect();
        let content_length = headers
            .get("content-length")
            .map_or(0, |value| value.parse().expect("numeric Content-Length"));
        let body_start = header_end + 4;
        while bytes.len() < body_start + content_length {
            let mut chunk = [0_u8; 4096];
            let read = stream
                .read(&mut chunk)
                .await
                .expect("read HTTP request body");
            assert!(
                read > 0,
                "connection closed before the complete request body"
            );
            bytes.extend_from_slice(&chunk[..read]);
        }

        CapturedRequest {
            request_line,
            headers,
            body: bytes[body_start..body_start + content_length].to_vec(),
        }
    }

    async fn write_response(
        stream: &mut TcpStream,
        status: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) {
        let mut response = format!("HTTP/1.1 {status}\r\n");
        for (name, value) in headers {
            response.push_str(name);
            response.push_str(": ");
            response.push_str(value);
            response.push_str("\r\n");
        }
        response.push_str(&format!(
            "Content-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        ));
        stream
            .write_all(response.as_bytes())
            .await
            .expect("write HTTP response headers");
        stream
            .write_all(body)
            .await
            .expect("write HTTP response body");
    }

    fn client_for(address: SocketAddr) -> ReqwestDigestClient {
        ReqwestDigestClient::new(
            &DeviceEndpoint::new(address.ip().to_string(), address.port()),
            "operator",
            Secret::new("secret"),
        )
        .expect("build test HTTP client")
    }

    #[tokio::test]
    async fn post_text_replays_the_request_and_signs_its_actual_method_and_uri() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind digest test server");
        let address = listener.local_addr().expect("digest test server address");
        let server = tokio::spawn(async move {
            let (mut probe_stream, _) = listener.accept().await.expect("accept digest probe");
            let probe = read_request(&mut probe_stream).await;
            write_response(
                &mut probe_stream,
                "401 Unauthorized",
                &[
                    ("WWW-Authenticate", r#"Basic realm="fallback""#),
                    (
                        "WWW-Authenticate",
                        r#"Digest realm="camera", nonce="nonce-1", qop="auth-int, auth", opaque="opaque-1", algorithm=MD5-sess"#,
                    ),
                ],
                b"",
            )
            .await;

            let (mut retry_stream, _) = listener.accept().await.expect("accept digest retry");
            let retry = read_request(&mut retry_stream).await;
            write_response(&mut retry_stream, "200 OK", &[], b"configuration applied").await;
            (probe, retry)
        });

        let path = "/cgi-bin/config?action=set";
        let body = r#"<state enabled="true"/>"#;
        let result = client_for(address)
            .post_text(path, body, "application/xml")
            .await
            .expect("the Digest-authenticated POST succeeds");
        let (probe, retry) = server.await.expect("digest test server finishes");

        assert_eq!(result, "configuration applied");
        assert_eq!(probe.request_line, format!("POST {path} HTTP/1.1"));
        assert_eq!(retry.request_line, probe.request_line);
        assert_eq!(probe.header("content-type"), Some("application/xml"));
        assert_eq!(retry.header("content-type"), Some("application/xml"));
        assert_eq!(probe.body, body.as_bytes());
        assert_eq!(retry.body, probe.body);
        assert!(probe.header("authorization").is_none());

        let authorization = retry
            .header("authorization")
            .expect("authenticated retry has Authorization");
        let params = digest_params(authorization).expect("retry uses Digest authentication");
        assert_eq!(params.get("username").map(String::as_str), Some("operator"));
        assert_eq!(params.get("realm").map(String::as_str), Some("camera"));
        assert_eq!(params.get("nonce").map(String::as_str), Some("nonce-1"));
        assert_eq!(params.get("uri").map(String::as_str), Some(path));
        assert_eq!(params.get("qop").map(String::as_str), Some("auth"));
        assert_eq!(params.get("nc").map(String::as_str), Some(NONCE_COUNT));
        assert_eq!(
            params.get("algorithm").map(String::as_str),
            Some("MD5-sess")
        );
        assert_eq!(params.get("opaque").map(String::as_str), Some("opaque-1"));

        let cnonce = params.get("cnonce").expect("a Digest cnonce parameter");
        let ha1_plain = md5_hex("operator:camera:secret");
        let ha1 = md5_hex(&format!("{ha1_plain}:nonce-1:{cnonce}"));
        let ha2 = md5_hex(&format!("POST:{path}"));
        let expected_response =
            md5_hex(&format!("{ha1}:nonce-1:{NONCE_COUNT}:{cnonce}:auth:{ha2}"));
        assert_eq!(
            params.get("response").map(String::as_str),
            Some(expected_response.as_str())
        );
        assert!(!authorization.contains("secret"));
    }

    #[tokio::test]
    async fn get_text_refuses_redirect_instead_of_following_location() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind redirect test server");
        let address = listener.local_addr().expect("redirect test server address");
        let location = format!("http://{address}/redirected");
        let server = tokio::spawn(async move {
            let (mut first_stream, _) = listener.accept().await.expect("accept initial request");
            let first = read_request(&mut first_stream).await;
            write_response(
                &mut first_stream,
                "302 Found",
                &[("Location", &location)],
                b"",
            )
            .await;

            match tokio::time::timeout(Duration::from_millis(500), listener.accept()).await {
                Ok(Ok((mut redirected_stream, _))) => {
                    let redirected = read_request(&mut redirected_stream).await;
                    write_response(&mut redirected_stream, "200 OK", &[], b"redirect followed")
                        .await;
                    (first, Some(redirected))
                }
                Ok(Err(error)) => panic!("redirect listener failed: {error}"),
                Err(_) => (first, None),
            }
        });

        let error = client_for(address)
            .get_text("/management")
            .await
            .expect_err("management redirects must be rejected");
        let (first, redirected) = server.await.expect("redirect test server finishes");

        assert_eq!(first.request_line, "GET /management HTTP/1.1");
        assert!(
            redirected.is_none(),
            "the redirect target must not be dialed"
        );
        assert!(matches!(
            error,
            DriverError::Device(message) if message == "device returned HTTP 302 Found"
        ));
    }

    #[tokio::test]
    async fn get_text_rejects_advertised_body_above_limit_before_reading() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind body-limit test server");
        let address = listener
            .local_addr()
            .expect("body-limit test server address");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept bounded request");
            let request = read_request(&mut stream).await;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                MAX_RESPONSE_LEN + 1
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("write oversized response header");
            request
        });

        let error = client_for(address)
            .get_text("/large")
            .await
            .expect_err("advertised oversized body must be rejected");
        let request = server.await.expect("body-limit test server finishes");

        assert_eq!(request.request_line, "GET /large HTTP/1.1");
        assert!(matches!(
            error,
            DriverError::Protocol(message)
                if message == format!(
                    "response body length {} exceeds maximum {MAX_RESPONSE_LEN}",
                    MAX_RESPONSE_LEN + 1
                )
        ));
    }

    #[tokio::test]
    async fn get_text_rejects_non_utf8_device_response() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind UTF-8 test server");
        let address = listener
            .local_addr()
            .expect("the UTF-8 test server address");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept text request");
            let request = read_request(&mut stream).await;
            write_response(&mut stream, "200 OK", &[], &[0x66, 0x6f, 0x80]).await;
            request
        });

        let error = client_for(address)
            .get_text("/status")
            .await
            .expect_err("invalid UTF-8 must not be silently replaced");
        let request = server.await.expect("the UTF-8 test server finishes");

        assert_eq!(request.request_line, "GET /status HTTP/1.1");
        assert!(matches!(
            error,
            DriverError::Protocol(message) if message.contains("device response was not UTF-8")
        ));
    }

    #[tokio::test]
    async fn download_stream_post_sends_xml_request_and_streams_chunked_response() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind stream test server");
        let address = listener.local_addr().expect("stream test server address");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept stream request");
            let request = read_request(&mut stream).await;
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n3\r\nabc\r\n4\r\ndefg\r\n0\r\n\r\n",
                )
                .await
                .expect("write chunked stream response");
            request
        });

        let path = "/ISAPI/ContentMgmt/download";
        let body = "<downloadRequest><playbackURI>rtsp://clip</playbackURI></downloadRequest>";
        let mut stream = client_for(address)
            .download_stream_post(path, body, "application/xml")
            .await
            .expect("streaming POST starts");
        let mut downloaded = Vec::new();
        while let Some(chunk) = stream.next().await {
            downloaded.extend_from_slice(&chunk.expect("stream chunk is readable"));
        }
        let request = server.await.expect("stream test server finishes");

        assert_eq!(request.request_line, format!("POST {path} HTTP/1.1"));
        assert_eq!(request.header("content-type"), Some("application/xml"));
        assert_eq!(request.body, body.as_bytes());
        assert_eq!(downloaded, b"abcdefg");
    }

    #[tokio::test]
    async fn authenticated_forbidden_response_is_reported_as_auth_failure() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind auth-failure test server");
        let address = listener
            .local_addr()
            .expect("auth-failure test server address");
        let server = tokio::spawn(async move {
            let (mut probe_stream, _) = listener.accept().await.expect("accept auth probe");
            let probe = read_request(&mut probe_stream).await;
            write_response(
                &mut probe_stream,
                "401 Unauthorized",
                &[(
                    "WWW-Authenticate",
                    r#"Digest realm="camera", nonce="nonce-2", qop="auth""#,
                )],
                b"",
            )
            .await;

            let (mut retry_stream, _) = listener.accept().await.expect("accept auth retry");
            let retry = read_request(&mut retry_stream).await;
            write_response(&mut retry_stream, "403 Forbidden", &[], b"").await;
            (probe, retry)
        });

        let error = client_for(address)
            .delete("/cgi-bin/users/locked")
            .await
            .expect_err("authenticated 403 is an authentication failure");
        let (probe, retry) = server.await.expect("auth-failure test server finishes");

        assert_eq!(probe.request_line, "DELETE /cgi-bin/users/locked HTTP/1.1");
        assert_eq!(retry.request_line, probe.request_line);
        assert!(probe.header("authorization").is_none());
        assert!(retry.header("authorization").is_some());
        assert!(matches!(error, DriverError::Auth));
    }

    #[tokio::test]
    async fn connect_error_does_not_expose_credential_bearing_path() {
        // Port 0 is never a listening port, so no descriptor exists that a
        // concurrently running test's forked child could inherit and keep
        // accepting: reserving a port by binding and dropping it would leave
        // the listening fd inheritable until execve closes it via SOCK_CLOEXEC,
        // which makes the connect outcome racy in this parallel test binary.
        let address = SocketAddr::from(([127, 0, 0, 1], 0));

        let secret = "plain-text-device-password";
        let path = format!("/cgi-bin/userManager.cgi?action=addUser&password={secret}");
        let error = client_for(address)
            .get_text(&path)
            .await
            .expect_err("closed port must fail to connect");

        match &error {
            DriverError::Unreachable(message) => {
                assert_eq!(message, "device HTTP connection failed");
            }
            other => panic!("expected unreachable error, got {other:?}"),
        }
        assert!(!error.to_string().contains(secret));
    }

    /// Live diagnostic (ignored): drive the REAL `download_stream` against a
    /// device and count the bytes it yields. Reads `CCTV_DL_HOST`, `CCTV_DL_USER`,
    /// `CCTV_DL_PW`, `CCTV_DL_PATH` from env. Run with:
    /// `CCTV_DL_HOST=... CCTV_DL_USER=... CCTV_DL_PW=... CCTV_DL_PATH='/cgi-bin/...' \
    ///  cargo test --lib download_stream_live -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "hits a real device; set CCTV_DL_* env vars"]
    async fn download_stream_live_counts_bytes() {
        use futures_util::StreamExt;
        let (Ok(host), Ok(user), Ok(pw), Ok(path)) = (
            std::env::var("CCTV_DL_HOST"),
            std::env::var("CCTV_DL_USER"),
            std::env::var("CCTV_DL_PW"),
            std::env::var("CCTV_DL_PATH"),
        ) else {
            tracing::info!("skip: CCTV_DL_* env not set");
            return;
        };
        let endpoint = DeviceEndpoint::new(host, 80);
        let client =
            ReqwestDigestClient::new(&endpoint, user, Secret::new(pw)).expect("client builds");
        let mut stream = client
            .download_stream(&path)
            .await
            .expect("download starts");
        let mut total: usize = 0;
        let mut chunks: usize = 0;
        let mut first_err: Option<String> = None;
        while let Some(item) = stream.next().await {
            match item {
                Ok(bytes) => {
                    total += bytes.len();
                    chunks += 1;
                }
                Err(error) => {
                    first_err = Some(error.to_string());
                    break;
                }
            }
        }
        tracing::info!(
            "download_stream: total_bytes={total} chunks={chunks} first_err={first_err:?}"
        );
        assert!(
            total > 1_000_000,
            "expected a multi-MB download, got {total} bytes; err={first_err:?}"
        );
    }

    #[test]
    fn compute_digest_response_matches_rfc2617_vector() {
        // The canonical RFC 2617 §3.5 example.
        let input = DigestResponseInput {
            username: "Mufasa",
            password: "Circle Of Life",
            realm: "testrealm@host.com",
            nonce: "dcd98b7102dd2f0e8b11d0f600bfb0c093",
            method: "GET",
            uri: "/dir/index.html",
            qop: Some("auth"),
            nonce_count: "00000001",
            cnonce: "0a4f113b",
            algorithm: None,
        };
        assert_eq!(
            compute_digest_response(&input),
            "6629fae49393a05397450978507c4ef1"
        );
    }

    #[test]
    fn digest_params_parses_scheme_and_quoted_values() {
        let header = r#"Digest realm="Login to DVR", qop="auth", nonce="1a2b3c", opaque="ff00", algorithm=MD5"#;
        let params = digest_params(header).expect("digest challenge");
        assert_eq!(
            params.get("realm").map(String::as_str),
            Some("Login to DVR")
        );
        assert_eq!(params.get("qop").map(String::as_str), Some("auth"));
        assert_eq!(params.get("nonce").map(String::as_str), Some("1a2b3c"));
        assert_eq!(params.get("opaque").map(String::as_str), Some("ff00"));
        assert_eq!(params.get("algorithm").map(String::as_str), Some("MD5"));
    }

    #[test]
    fn digest_params_rejects_non_digest_scheme() {
        assert!(digest_params(r#"Basic realm="x""#).is_none());
    }

    #[test]
    fn digest_challenge_requires_realm_and_nonce() {
        let params = parse_auth_params(r#"realm="only realm""#);
        assert!(DigestChallenge::from_params(&params).is_none());
    }

    #[test]
    fn digest_authorization_builds_a_qop_auth_header() {
        let challenge = DigestChallenge {
            realm: "testrealm@host.com".to_owned(),
            nonce: "dcd98b7102dd2f0e8b11d0f600bfb0c093".to_owned(),
            qop: Some("auth".to_owned()),
            opaque: Some("5ccc069c".to_owned()),
            algorithm: None,
        };
        let header = digest_authorization(&DigestAuthorizationInput {
            username: "Mufasa",
            password: "Circle Of Life",
            challenge: &challenge,
            method: "GET",
            uri: "/dir/index.html",
            cnonce: "0a4f113b",
            nonce_count: "00000001",
        });
        assert!(header.starts_with("Digest "));
        assert!(header.contains(r#"username="Mufasa""#));
        assert!(header.contains("qop=auth"));
        assert!(header.contains("nc=00000001"));
        assert!(header.contains(r#"cnonce="0a4f113b""#));
        assert!(header.contains(r#"opaque="5ccc069c""#));
        assert!(header.contains(r#"response="6629fae49393a05397450978507c4ef1""#));
        // The plaintext password must never appear in the header.
        assert!(!header.contains("Circle Of Life"));
    }

    #[test]
    fn digest_authorization_without_qop_uses_the_rfc2069_form() {
        let challenge = DigestChallenge {
            realm: "realm".to_owned(),
            nonce: "abc".to_owned(),
            qop: None,
            opaque: None,
            algorithm: None,
        };
        let header = digest_authorization(&DigestAuthorizationInput {
            username: "user",
            password: "pw",
            challenge: &challenge,
            method: "GET",
            uri: "/x",
            cnonce: "cnonce",
            nonce_count: "00000001",
        });
        assert!(!header.contains("qop="));
        assert!(!header.contains("nc="));
        assert!(header.contains(r#"response=""#));
    }

    #[test]
    fn select_auth_qop_picks_auth_from_a_list() {
        assert_eq!(select_auth_qop("auth"), Some("auth"));
        assert_eq!(select_auth_qop("auth-int,auth"), Some("auth"));
        assert_eq!(select_auth_qop("auth-int"), None);
    }

    #[test]
    fn ensure_jpeg_accepts_soi_and_rejects_other_bodies() {
        // A body starting with the JPEG SOI marker is accepted.
        assert!(ensure_jpeg(&[0xFF, 0xD8, 0xFF, 0xE0, 0x00]).is_ok());
        // An HTML error page (the exact case this guards) is rejected.
        assert!(matches!(
            ensure_jpeg(b"<html>401 Unauthorized</html>"),
            Err(DriverError::Device(_))
        ));
        // A one-byte prefix of the marker, and an empty body, are both rejected.
        assert!(matches!(ensure_jpeg(&[0xFF]), Err(DriverError::Device(_))));
        assert!(matches!(ensure_jpeg(&[]), Err(DriverError::Device(_))));
    }

    #[tokio::test]
    async fn snapshot_client_connects_through_the_supplied_pinned_address_without_dns() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind snapshot test server");
        let address = listener.local_addr().expect("test server address");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept request");
            let mut request = [0_u8; 1024];
            let read = stream.read(&mut request).await.expect("read request");
            assert!(
                std::str::from_utf8(&request[..read])
                    .expect("request is HTTP")
                    .starts_with("GET /still HTTP/1.1")
            );
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\n\xFF\xD8\x00")
                .await
                .expect("write response");
        });

        let client = pinned_snapshot_client("snapshot.invalid", &[address])
            .expect("build client with pinned resolver");
        let response = client
            .get(format!("http://snapshot.invalid:{}/still", address.port()))
            .send()
            .await
            .expect("pinned request succeeds without resolving snapshot.invalid");

        assert_eq!(
            response.bytes().await.expect("read body").as_ref(),
            [0xFF, 0xD8, 0x00]
        );
        server.await.expect("test server finishes");
    }

    #[test]
    fn reqwest_client_new_builds_expected_base_url() {
        let client = ReqwestDigestClient::new(
            &DeviceEndpoint::new("192.0.2.10", 80),
            "admin",
            Secret::new("pw"),
        )
        .expect("client builds");
        assert_eq!(client.base_url, "http://192.0.2.10:80");
        assert_eq!(client.host, "192.0.2.10");
        // Debug must not leak the password.
        assert!(!format!("{client:?}").contains("pw"));
    }

    /// Parse helper for the SSRF-fence tests.
    fn parse(url: &str) -> reqwest::Url {
        reqwest::Url::parse(url).expect("valid url")
    }

    /// The `Ok` request-URI, or a panic naming the unexpected error.
    fn request_uri(url: &str, host: &str) -> String {
        snapshot_request_uri(&parse(url), host).expect("expected an allowed snapshot URL")
    }

    #[test]
    fn snapshot_request_uri_accepts_the_device_host_and_returns_path_and_query() {
        // Same host, http, path + query → returns the request-URI for the digest.
        assert_eq!(
            request_uri(
                "http://192.0.2.10/onvif-http/snapshot?Profile_1",
                "192.0.2.10"
            ),
            "/onvif-http/snapshot?Profile_1"
        );
        // No query → just the path.
        assert_eq!(
            request_uri("http://192.0.2.10:8080/snap.jpg", "192.0.2.10"),
            "/snap.jpg"
        );
        // https on the same host is allowed.
        assert_eq!(
            request_uri("https://cam.local/still", "cam.local"),
            "/still"
        );
        // Host match is case-insensitive.
        assert_eq!(request_uri("http://CAM.local/s", "cam.local"), "/s");
    }

    #[test]
    fn snapshot_request_uri_fences_off_ssrf_pivots() {
        // A different host (the classic SSRF pivot to a metadata endpoint) is a
        // device error — fail closed.
        assert!(matches!(
            snapshot_request_uri(
                &parse("http://169.254.169.254/latest/meta-data/"),
                "192.0.2.10"
            ),
            Err(DriverError::Device(_))
        ));
        // A same-host URL on a non-http(s) scheme is rejected as a protocol error.
        assert!(matches!(
            snapshot_request_uri(&parse("file://192.0.2.10/etc/passwd"), "192.0.2.10"),
            Err(DriverError::Protocol(_))
        ));
        // A gopher/ftp scheme, even on the device host, is refused.
        assert!(matches!(
            snapshot_request_uri(&parse("ftp://192.0.2.10/x"), "192.0.2.10"),
            Err(DriverError::Protocol(_))
        ));
    }
}
