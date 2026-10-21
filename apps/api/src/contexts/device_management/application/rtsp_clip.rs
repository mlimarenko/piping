//! Exporting a recorded clip from an RTSP playback URL to an MP4.
//!
//! The still plane grabs one keyframe and the live plane restreams forever;
//! this plane copies a BOUNDED archive window (the device's RTSP playback
//! session, already time-ranged by the driver's `playback_uri`) into an MP4.
//!
//! The clip is STREAMED, not buffered: the bytes flow ffmpeg stdout → HTTP
//! response as fragments arrive, so an hour-long export costs O(one fragment)
//! of memory, not O(whole clip). This is the load-safety property — buffering a
//! 512 MB clip per concurrent export is what pegs memory; streaming does not.
//!
//! Same reachability caveat as the other media planes: ffmpeg connects to the
//! device directly, so export works only where the backend shares the device
//! network (direct-route deployment).

use std::pin::Pin;

use async_trait::async_trait;
use bytes::Bytes;
use futures_util::Stream;

use crate::contexts::device_management::domain::{ClipEncoding, DriverError};

/// A streaming MP4 clip: the download metadata plus a byte stream fed straight
/// into the HTTP response body. Dropping the stream tears the ffmpeg process
/// down (the consumer is the only thing keeping it alive).
pub struct ClipStream {
    /// MIME type of the clip (`video/mp4`).
    pub content_type: String,
    /// Suggested download filename.
    pub filename: String,
    /// The MP4 bytes, streamed as they are produced.
    pub bytes: Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>>,
}

/// Produces a streaming MP4 clip from an RTSP playback URL.
///
/// The URL embeds device credentials; implementations MUST NOT log it.
#[async_trait]
pub trait RtspClipSource: Send + Sync {
    /// Copy the playback session at `rtsp_url` into an MP4 named `filename`,
    /// capping the transfer at `max_seconds` and streaming the bytes back. The
    /// FIRST chunk (or a failure) is resolved before returning, so the HTTP
    /// layer can still answer a real error status instead of a broken 200.
    async fn export(
        &self,
        rtsp_url: &str,
        filename: &str,
        max_seconds: u64,
        encoding: ClipEncoding,
    ) -> Result<ClipStream, DriverError>;
}
