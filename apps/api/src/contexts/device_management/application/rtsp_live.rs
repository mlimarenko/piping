//! Live MJPEG restreaming of a device's RTSP feed.
//!
//! The still plane ([`rtsp_still`](super::rtsp_still)) decodes ONE keyframe;
//! this port keeps the transcode running and yields a continuous
//! `multipart/x-mixed-replace` MJPEG byte stream a browser `<img>` renders as
//! motion video. Same reachability caveat as the still plane: the media
//! process connects to the device DIRECTLY, so a proxy-node deployment cannot
//! use this path until the proxy grows a media tunnel.
//!
//! This is the pure port; the ffmpeg-backed adapter lives in
//! `infrastructure::rtsp_live`.

use std::pin::Pin;

use async_trait::async_trait;
use bytes::Bytes;
use futures_util::Stream;
use thiserror::Error;

use crate::contexts::device_management::domain::DriverError;

/// Ceilings/defaults for the caller-tunable stream shape. One place so the
/// REST layer and the adapter agree on what "valid" means.
pub const MIN_FPS: u16 = 1;
pub const MAX_FPS: u16 = 15;
pub const DEFAULT_FPS: u16 = 6;
pub const MIN_WIDTH: u16 = 320;
pub const MAX_WIDTH: u16 = 1920;
pub const DEFAULT_WIDTH: u16 = 1280;

/// Caller-tunable shape of the restream.
#[derive(Debug, Clone, Copy)]
pub struct LiveStreamOptions {
    /// Output frame rate, clamped to `MIN_FPS..=MAX_FPS`.
    pub fps: u16,
    /// Output width ceiling (the source is never upscaled), clamped to
    /// `MIN_WIDTH..=MAX_WIDTH`.
    pub max_width: u16,
}

impl Default for LiveStreamOptions {
    fn default() -> Self {
        Self {
            fps: DEFAULT_FPS,
            max_width: DEFAULT_WIDTH,
        }
    }
}

impl LiveStreamOptions {
    /// Clamp arbitrary caller input into the supported envelope.
    pub fn clamped(fps: Option<u16>, max_width: Option<u16>) -> Self {
        Self {
            fps: fps.unwrap_or(DEFAULT_FPS).clamp(MIN_FPS, MAX_FPS),
            max_width: max_width
                .unwrap_or(DEFAULT_WIDTH)
                .clamp(MIN_WIDTH, MAX_WIDTH),
        }
    }
}

/// A running MJPEG restream: the multipart boundary plus the byte stream.
///
/// Dropping the stream MUST tear the underlying media process down — the
/// consumer (an HTTP response body) is the only thing keeping it alive.
pub struct MjpegStream {
    /// The `multipart/x-mixed-replace` boundary token the bytes use.
    pub boundary: &'static str,
    /// The multipart bytes, already boundary-framed by the producer.
    pub bytes: Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>>,
}

impl std::fmt::Debug for MjpegStream {
    /// The byte stream is an opaque trait object with no useful debug
    /// representation; only the boundary is printed.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MjpegStream")
            .field("boundary", &self.boundary)
            .finish_non_exhaustive()
    }
}

/// Why a live restream could not be started.
#[derive(Debug, Error)]
pub enum LiveStreamError {
    /// Every restream slot is taken; retry later. Maps to HTTP 429, not a
    /// device fault.
    #[error("all live-stream slots are busy")]
    Busy,
    /// The device/transcoder failed (unreachable, bad stream, decode error).
    #[error(transparent)]
    Driver(#[from] DriverError),
}

/// Produces a live MJPEG restream from an RTSP URL.
///
/// The URL embeds device credentials; implementations MUST NOT log it.
#[async_trait]
pub trait RtspLiveSource: Send + Sync {
    /// Start restreaming `rtsp_url` as MJPEG with the given shape.
    ///
    /// Implementations must have produced the FIRST body chunk (or failed)
    /// before resolving, so the caller can still answer with a proper error
    /// status instead of a broken 200.
    async fn mjpeg(
        &self,
        rtsp_url: &str,
        options: LiveStreamOptions,
    ) -> Result<MjpegStream, LiveStreamError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_clamp_into_the_supported_envelope() {
        let clamped = LiveStreamOptions::clamped(Some(0), Some(9999));
        assert_eq!(clamped.fps, MIN_FPS);
        assert_eq!(clamped.max_width, MAX_WIDTH);

        let defaults = LiveStreamOptions::clamped(None, None);
        assert_eq!(defaults.fps, DEFAULT_FPS);
        assert_eq!(defaults.max_width, DEFAULT_WIDTH);

        let passthrough = LiveStreamOptions::clamped(Some(10), Some(640));
        assert_eq!(passthrough.fps, 10);
        assert_eq!(passthrough.max_width, 640);
    }
}
