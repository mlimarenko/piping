//! ffmpeg-backed [`RtspStillSource`]: decode one keyframe from an RTSP URL to
//! JPEG by driving a bounded `ffmpeg` subprocess.
//!
//! Reachability note: ffmpeg connects to the RTSP endpoint DIRECTLY, so this
//! adapter only produces a still when the backend itself has a network route to
//! the device (the [`DirectTcpTransport`](super::transport::DirectTcpTransport)
//! deployment — dev, and any install where the backend shares the device
//! network). A proxy-node deployment, where the driver byte-transport is tunnelled
//! through an edge proxy, cannot use this path until the proxy grows a media
//! tunnel; there the native driver `snapshot()` (which IS proxy-aware) remains
//! the only still source, and families without a native still simply report no
//! picture. This asymmetry is intentional and fail-safe, not a regression.

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::Semaphore;

use super::media_budget::{MediaBudget, MediaBudgetPermit};
use crate::contexts::device_management::application::rtsp_still::RtspStillSource;
use crate::contexts::device_management::domain::DriverError;

/// Decode one RTSP keyframe to JPEG via an `ffmpeg` subprocess.
#[derive(Clone)]
pub struct FfmpegRtspStill {
    /// Path or name of the ffmpeg binary (resolved on `PATH` when bare).
    ffmpeg: String,
    /// Hard wall-clock cap on a single grab; the child is killed on expiry.
    timeout: Duration,
    /// Upper bound on the decoded JPEG we will buffer (defence against a
    /// runaway encoder), in bytes.
    max_bytes: usize,
    /// Concurrent-grab ceiling. A stills-mode grid over a half-populated NVR
    /// would otherwise spawn one doomed ~timeout-long ffmpeg per empty channel;
    /// the semaphore bounds how many run at once (excess grabs queue, they do
    /// not fail). Shared across clones so the cap is process-wide.
    slots: Arc<Semaphore>,
    /// Process-wide memory budget shared with the other three CCTV media
    /// planes (live/clip/download); see [`MediaBudget`].
    media_budget: MediaBudget,
    /// Chunks of `media_budget` one grab reserves for its lifetime.
    media_units: u32,
}

impl FfmpegRtspStill {
    /// Construct the adapter with `max_grabs` concurrent decode slots, drawing
    /// `media_units` chunks from the shared `media_budget` per grab.
    pub fn new(
        ffmpeg: impl Into<String>,
        timeout: Duration,
        max_bytes: usize,
        max_grabs: usize,
        media_budget: MediaBudget,
        media_units: u32,
    ) -> Self {
        Self {
            ffmpeg: ffmpeg.into(),
            timeout,
            max_bytes,
            slots: Arc::new(Semaphore::new(max_grabs)),
            media_budget,
            media_units,
        }
    }
}

#[async_trait]
impl RtspStillSource for FfmpegRtspStill {
    async fn keyframe(&self, rtsp_url: &str) -> Result<Vec<u8>, DriverError> {
        // Only ever hand ffmpeg an rtsp URL. The URL comes from a driver's
        // `stream_uri()` (built from an operator-configured host/credentials),
        // never from client input, but pinning the scheme keeps ffmpeg from
        // being steered at `file:`/`http:`/etc. if a driver ever regresses.
        if !rtsp_url.starts_with("rtsp://") {
            return Err(DriverError::Unsupported);
        }

        // Reserve from the shared cross-plane memory budget FIRST: it never
        // blocks (see `MediaBudget`'s "Never blocks" note), so an exhausted
        // budget fails this grab fast instead of first waiting behind the
        // (potentially queued) local semaphore below for nothing.
        let _media_permit: MediaBudgetPermit =
            self.media_budget.try_reserve(self.media_units, "still")?;

        // Bound concurrent ffmpeg grabs. Unlike the live plane (which rejects
        // with Busy when full), a still queues for a slot — a snapshot is a
        // one-shot request the caller is waiting on, so a brief wait beats a
        // spurious failure. The permit is held for the whole grab.
        let _permit = self
            .slots
            .acquire()
            .await
            .map_err(|_| DriverError::Transport("still grab pool closed".to_owned()))?;

        // No shell: the URL is a single argv element, so its `&`/`?` tokens
        // cannot inject. `-protocol_whitelist` further fences ffmpeg to the
        // RTSP/RTP family; `-nostdin` stops it from consuming our stdin; a
        // single mjpeg frame is written to stdout.
        let mut child = super::spawn_media_process(
            Command::new(&self.ffmpeg)
                .args([
                    "-nostdin",
                    "-loglevel",
                    "error",
                    "-rtsp_transport",
                    "tcp",
                    "-protocol_whitelist",
                    "rtsp,rtp,tcp,udp",
                    "-i",
                    rtsp_url,
                    "-frames:v",
                    "1",
                    "-f",
                    "image2pipe",
                    "-c:v",
                    "mjpeg",
                    "-q:v",
                    "3",
                    "-",
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true),
        )
        .await?;

        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| DriverError::Transport("ffmpeg produced no stdout".to_string()))?;

        // Read at most `max_bytes + 1` so an over-cap stream is detected without
        // buffering it whole.
        let cap = self.max_bytes.saturating_add(1);
        let mut buf = Vec::with_capacity(64 * 1024);
        let grab = async {
            (&mut stdout)
                .take(cap as u64)
                .read_to_end(&mut buf)
                .await
                .map_err(|error| DriverError::Transport(format!("ffmpeg read failed: {error}")))?;
            if buf.len() > self.max_bytes {
                // The bounded `Take` above stopped draining stdout once the cap
                // was hit, so ffmpeg is now blocked writing further frame bytes
                // into a pipe nobody is reading; `wait()` would hang until
                // `timeout` expires. Kill it now instead of burning a shared
                // still slot for the whole timeout.
                let _ = child.start_kill();
                return Err(DriverError::Transport(
                    "snapshot exceeded the size limit".to_string(),
                ));
            }
            child
                .wait()
                .await
                .map_err(|error| DriverError::Transport(format!("ffmpeg wait failed: {error}")))
        };

        let status = match tokio::time::timeout(self.timeout, grab).await {
            Ok(result) => result?,
            Err(_) => {
                let _ = child.start_kill();
                return Err(DriverError::Transport("device read timed out".to_string()));
            }
        };

        if !status.success() {
            // The device was reachable but produced no decodable frame; report
            // it as an upstream/device fault, never echoing the credential URL.
            return Err(DriverError::Device(
                "device stream produced no still frame".to_string(),
            ));
        }
        // A valid JPEG starts with the SOI marker (0xFF 0xD8); anything else is
        // not an image we should return.
        if buf.len() < 2 || buf[0] != 0xFF || buf[1] != 0xD8 {
            return Err(DriverError::Protocol(
                "ffmpeg output was not a JPEG frame".to_string(),
            ));
        }
        Ok(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adapter(ffmpeg: &str, timeout: Duration, max_bytes: usize, slots: usize) -> FfmpegRtspStill {
        // A generous, effectively-unlimited shared budget: these tests exercise
        // the LOCAL semaphore and error paths, not the cross-plane budget (see
        // `shared_media_budget_exhausted_fails_the_grab_even_with_a_free_local_slot`
        // below for that).
        FfmpegRtspStill::new(
            ffmpeg,
            timeout,
            max_bytes,
            slots,
            MediaBudget::new(1024 * 1024 * 1024, 1),
            1,
        )
    }

    #[tokio::test]
    async fn non_rtsp_url_is_rejected_before_spawning_anything() {
        let still = adapter(
            "/definitely/not/ffmpeg",
            Duration::from_millis(500),
            1024,
            1,
        );
        let error = still
            .keyframe("http://evil.example/file")
            .await
            .expect_err("non-rtsp must be rejected");
        assert!(matches!(error, DriverError::Unsupported));
    }

    #[tokio::test]
    async fn missing_binary_is_a_transport_error() {
        let still = adapter(
            "/definitely/not/ffmpeg",
            Duration::from_millis(500),
            1024,
            1,
        );
        let error = still
            .keyframe("rtsp://device.test/stream")
            .await
            .expect_err("missing binary must fail");
        assert!(matches!(error, DriverError::Transport(_)));
    }

    #[tokio::test]
    async fn nonzero_exit_with_no_output_is_a_device_error() {
        // `false` exits 1 immediately with no stdout: the device-fault path.
        let still = adapter("false", Duration::from_secs(10), 1024, 1);
        let error = still
            .keyframe("rtsp://device.test/stream")
            .await
            .expect_err("nonzero exit must fail the grab");
        assert!(matches!(error, DriverError::Device(_)));
    }

    #[tokio::test]
    async fn successful_exit_with_no_frame_is_a_protocol_error() {
        // `true` exits 0 immediately with no stdout: not a device fault, but
        // also not a JPEG — surfaces as a protocol error.
        let still = adapter("true", Duration::from_secs(10), 1024, 1);
        let error = still
            .keyframe("rtsp://device.test/stream")
            .await
            .expect_err("empty successful exit must fail the grab");
        assert!(matches!(error, DriverError::Protocol(_)));
    }

    /// Write an executable shell script that ignores the ffmpeg argv and just
    /// streams zeros forever — a stand-in for an encoder that keeps writing
    /// past the size cap. (Mirrors rtsp_live's `endless_producer_script`.)
    fn endless_producer_script() -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::atomic::{AtomicU64, Ordering};
        // A process-wide counter guarantees a unique path per call, so tests
        // running concurrently never clobber (or delete) each other's script.
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "piping-rtsp-still-test-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::write(&path, "#!/bin/sh\nexec cat /dev/zero\n").expect("script written");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("script chmod");
        path
    }

    #[tokio::test]
    async fn oversized_frame_kills_the_child_fast_instead_of_waiting_out_the_timeout() {
        // Regression test for the hang: once the bounded read hits `max_bytes`,
        // the encoder is left blocked writing to a stdout pipe nobody drains
        // any more. `wait()`-ing on it would burn the whole adapter timeout
        // (and hold a shared still slot) for nothing; the fix kills the child
        // immediately instead.
        let script = endless_producer_script();
        // Deliberately long adapter timeout: a pass that only holds because
        // the timeout eventually fired would prove nothing about the fix, so
        // the outer bound below (2s) must be what actually ends the grab.
        let still = adapter(
            script.to_str().expect("utf-8 path"),
            Duration::from_secs(10),
            4,
            1,
        );
        let outcome = tokio::time::timeout(
            Duration::from_secs(2),
            still.keyframe("rtsp://device.test/stream"),
        )
        .await;
        let error = outcome
            .expect("oversize grab must fail fast, not hang out the adapter timeout")
            .expect_err("oversize stream must be rejected");
        match error {
            DriverError::Transport(message) => {
                assert!(
                    message.contains("size limit"),
                    "unexpected transport error: {message}"
                );
            }
            other => panic!("expected a Transport size-limit error, got {other:?}"),
        }

        // The slot must have been released promptly (not held for the whole
        // 10s adapter timeout) — a second grab on the same 1-slot adapter
        // must be able to start right away.
        let second = tokio::time::timeout(
            Duration::from_secs(2),
            still.keyframe("rtsp://device.test/stream"),
        )
        .await;
        assert!(
            second.is_ok(),
            "still slot must be freed promptly after the oversize grab"
        );
        let _ = std::fs::remove_file(&script);
    }

    #[tokio::test]
    async fn shared_media_budget_exhausted_fails_the_grab_even_with_a_free_local_slot() {
        // The local `slots` semaphore below has a free slot (max_grabs = 1,
        // never touched by another caller in this test) — only the exhausted
        // cross-plane shared budget should be why this grab fails.
        let budget = MediaBudget::new(1, 1); // exactly 1 unit total
        let _dummy = budget.try_reserve(1, "dummy").expect("dummy reservation");
        let still = FfmpegRtspStill::new(
            "/definitely/not/ffmpeg",
            Duration::from_millis(500),
            1024,
            1,
            budget,
            1,
        );
        let error = still
            .keyframe("rtsp://device.test/stream")
            .await
            .expect_err("exhausted shared budget must fail the grab");
        assert!(matches!(error, DriverError::Busy(_)));
    }
}
