//! Grabbing a single still frame from a live RTSP stream.
//!
//! Some device families cannot answer a native still request even though they
//! serve a working video stream — the canonical case is XMeye/Sofia NVR
//! firmware that rejects `OPSNAP` (`Ret 102`) while its `OPMonitor` stream runs
//! fine. For those, the device-management service falls back from the driver's
//! `snapshot()` to a keyframe decoded from the driver's `stream_uri()`.
//!
//! This is the pure port; the ffmpeg-backed adapter lives in
//! `infrastructure::rtsp_still`. It is deliberately separate from the driver
//! byte-transport: a keyframe grab is a media-decode concern (a subprocess or a
//! decoder), not a protocol framing concern.

use async_trait::async_trait;

use crate::contexts::device_management::domain::DriverError;

/// Produces a single JPEG still from a live RTSP URL.
///
/// The URL embeds device credentials (RTSP userinfo or XMeye `password=` path
/// tokens); implementations MUST NOT log it. A failure to reach or decode the
/// stream surfaces as a [`DriverError`] so it maps to the same 502/4xx envelope
/// as any other device-side fault.
#[async_trait]
pub trait RtspStillSource: Send + Sync {
    /// Decode one keyframe from `rtsp_url` and return it as JPEG bytes.
    async fn keyframe(&self, rtsp_url: &str) -> Result<Vec<u8>, DriverError>;
}
