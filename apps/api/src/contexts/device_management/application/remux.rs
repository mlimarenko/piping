//! [`RemuxSource`]: repackage a raw device-file download (e.g. Dahua `.dav`)
//! into a streaming, browser-playable MP4 — without transcoding.
//!
//! The fast export path pulls recorded footage as a native device file
//! ([`DownloadStream`]), which is a vendor container the browser cannot play.
//! A `RemuxSource` pipes those bytes through `ffmpeg -c copy` to a fragmented
//! MP4: no re-encode (cheap, exact), streamed end-to-end so an hour-long clip
//! stays O(chunk) in memory. The output is a [`ClipStream`], identical in shape
//! to the RTSP-playback export, so the service returns one uniform type.

use async_trait::async_trait;

use crate::contexts::device_management::application::rtsp_clip::ClipStream;
use crate::contexts::device_management::domain::{ClipEncoding, DownloadStream, DriverError};

/// An opaque concurrency-slot reservation from [`RemuxSource::try_reserve`].
/// Carries no data the caller needs to inspect — it exists only so the
/// remux-slot cap can be enforced BEFORE the caller opens the (expensive,
/// device-side) download connection, rather than after. Dropping it without
/// calling [`RemuxSource::remux_reserved`] releases the slot immediately.
pub trait RemuxReservation: Send {}

/// Remux a raw device container stream into a streaming MP4 [`ClipStream`].
#[async_trait]
pub trait RemuxSource: Send + Sync {
    /// Reserve a concurrency slot without touching any device. Callers that
    /// open an expensive device-side download (e.g. Dahua `loadfile`) should
    /// reserve FIRST and only then dial the device — otherwise an
    /// already-over-the-cap request still opens (and must tear down) a device
    /// connection before finding out it was going to be rejected anyway, and
    /// the configured concurrency cap fails to bound device connections at all.
    ///
    /// # Errors
    ///
    /// [`DriverError::Busy`] when every slot is taken.
    fn try_reserve(&self) -> Result<Box<dyn RemuxReservation>, DriverError>;

    /// Remux `download` using a slot already reserved via [`Self::try_reserve`].
    /// The input bytes flow device → ffmpeg stdin → MP4 stdout → response as
    /// they arrive; backpressure propagates the whole way, so nothing is
    /// buffered whole.
    async fn remux_reserved(
        &self,
        download: DownloadStream,
        reservation: Box<dyn RemuxReservation>,
        encoding: ClipEncoding,
    ) -> Result<ClipStream, DriverError>;

    /// Convenience for a caller with no reservation-ordering requirement:
    /// reserve a slot, then remux. Equivalent to
    /// `try_reserve()` immediately followed by [`Self::remux_reserved`].
    async fn remux(
        &self,
        download: DownloadStream,
        encoding: ClipEncoding,
    ) -> Result<ClipStream, DriverError> {
        let reservation = self.try_reserve()?;
        self.remux_reserved(download, reservation, encoding).await
    }
}
