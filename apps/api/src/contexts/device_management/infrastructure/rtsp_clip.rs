//! ffmpeg-backed [`RtspClipSource`]: copy an RTSP playback window to a
//! fragmented MP4 and STREAM it (no whole-clip buffer).
//!
//! `-c copy` remuxes without transcoding (cheap, exact), and
//! `-movflags frag_keyframe+empty_moov+default_base_moof` makes the MP4
//! streamable from a pipe. The device closes the RTSP session at the requested
//! end time, so `-t` bounds the copy; a semaphore bounds concurrent exports and
//! the child is `kill_on_drop`, so a dropped download tears the transcode down.
//! Nothing is buffered — bytes flow ffmpeg stdout → response as they arrive.
//!
//! Two more fences beyond the semaphore: a per-chunk stall watchdog ends a clip
//! whose source goes quiet mid-export (mirrors `FfmpegBody` in rtsp_live.rs),
//! so a stalled RTSP feed doesn't pin an export slot forever; and an
//! exit-status check on EOF turns a mid-export ffmpeg crash into an unclean
//! stream ending instead of a silently truncated MP4.

use std::future::Future;
use std::pin::Pin;
use std::process::Stdio;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use tokio::process::{Child, Command};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::io::ReaderStream;

use super::media_budget::{MediaBudget, MediaBudgetPermit};
use crate::contexts::device_management::application::rtsp_clip::{ClipStream, RtspClipSource};
use crate::contexts::device_management::domain::{ClipEncoding, DriverError};

/// Build the ffmpeg argument list for one RTSP-playback clip.
///
/// The shape is fixed; only the video codec varies, and only because a browser
/// that cannot decode the device's own encoding still needs to watch the same
/// footage. Callers choose with a typed value, never raw flags.
fn clip_args<'a>(rtsp_url: &'a str, duration: &'a str, encoding: ClipEncoding) -> Vec<&'a str> {
    let mut args = vec![
        "-nostdin",
        "-loglevel",
        "error",
        "-rtsp_transport",
        "tcp",
        "-protocol_whitelist",
        "rtsp,rtp,tcp,udp",
        "-fflags",
        "+genpts",
        "-i",
        rtsp_url,
        "-t",
        duration,
        "-avoid_negative_ts",
        "make_zero",
        "-an",
    ];
    args.extend(match encoding {
        ClipEncoding::Copy => ["-c:v", "copy"].as_slice(),
        ClipEncoding::BrowserCompatible => [
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-tune",
            "zerolatency",
            "-pix_fmt",
            "yuv420p",
            "-profile:v",
            "baseline",
            "-level",
            "3.1",
        ]
        .as_slice(),
    });
    args.extend([
        "-movflags",
        "frag_keyframe+empty_moov+default_base_moof",
        "-f",
        "mp4",
        "-",
    ]);
    args
}

/// Copy an RTSP playback window to a streaming MP4 via an `ffmpeg` subprocess.
#[derive(Clone)]
pub struct FfmpegRtspClip {
    ffmpeg: String,
    /// Cap on how long the FIRST chunk may take (RTSP connect + playback
    /// seek); also reused as the per-chunk stall timeout once streaming (see
    /// `ClipBody::stall_timeout`), so a source that goes quiet mid-export
    /// doesn't pin the slot forever either.
    first_chunk_timeout: Duration,
    /// Concurrent-export ceiling (each copies a whole recorded window).
    slots: Arc<Semaphore>,
    /// Process-wide memory budget shared with the other three CCTV media
    /// planes (still/live/download); see [`MediaBudget`].
    media_budget: MediaBudget,
    /// Chunks of `media_budget` one export reserves for its lifetime.
    media_units: u32,
}

impl FfmpegRtspClip {
    /// Construct the adapter with `max_exports` concurrent slots, drawing
    /// `media_units` chunks from the shared `media_budget` per export.
    pub fn new(
        ffmpeg: impl Into<String>,
        first_chunk_timeout: Duration,
        max_exports: usize,
        media_budget: MediaBudget,
        media_units: u32,
    ) -> Self {
        Self {
            ffmpeg: ffmpeg.into(),
            first_chunk_timeout,
            slots: Arc::new(Semaphore::new(max_exports)),
            media_budget,
            media_units,
        }
    }
}

/// Where the ffmpeg child is, from the stream's point of view.
///
/// EOF on stdout does not mean the process has been reaped: the kernel closes
/// the pipe as the process exits, and a non-blocking `try_wait()` in that
/// window answers "not finished yet" for a child that is about to report a
/// crash. Treating that answer as success is what delivers a truncated MP4 as
/// a complete one, so the stream waits for the real status instead of asking
/// once. The child moves into the wait future, which keeps `kill_on_drop`:
/// dropping the body still tears ffmpeg down.
enum ChildExit {
    Running(Child),
    Waiting(Pin<Box<dyn Future<Output = std::io::Result<std::process::ExitStatus>> + Send>>),
    Reported,
}

/// The clip byte stream: a `ReaderStream` over ffmpeg stdout that OWNS the child
/// and the concurrency permit, so both are released when the download ends,
/// with a stall watchdog and an exit-status check on EOF.
struct ClipBody<R> {
    inner: ReaderStream<R>,
    /// Held so `kill_on_drop` fires when the consumer drops the body, AND
    /// awaited on EOF to catch a mid-export crash.
    child: ChildExit,
    /// Held for the lifetime of the stream; releasing frees an export slot.
    _permit: OwnedSemaphorePermit,
    /// Held for the lifetime of the stream; releasing frees its reservation
    /// from the shared cross-plane [`MediaBudget`].
    _media_permit: MediaBudgetPermit,
    /// Cap on the gap between chunks; expiry ends the clip so a stalled
    /// source can't pin the slot forever (mirrors `stall_timeout` in
    /// `FfmpegBody`, rtsp_live.rs).
    stall_timeout: Duration,
    /// Armed deadline for the next chunk; `None` until first poll, reset on
    /// every chunk of progress.
    deadline: Option<Pin<Box<tokio::time::Sleep>>>,
}

impl<R> ClipBody<R> {
    /// Resolve the end of the stream against ffmpeg's real exit status.
    ///
    /// Returns `Pending` while the status is still unknown — the wait future
    /// registers the waker, so this is a real suspension, not a spin.
    fn poll_child_exit(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Bytes, std::io::Error>>> {
        self.child = match std::mem::replace(&mut self.child, ChildExit::Reported) {
            ChildExit::Running(mut child) => {
                ChildExit::Waiting(Box::pin(async move { child.wait().await }))
            }
            already_started => already_started,
        };
        let ChildExit::Waiting(wait) = &mut self.child else {
            return Poll::Ready(None);
        };
        let status = match wait.as_mut().poll(cx) {
            Poll::Pending => return Poll::Pending,
            // The status is unreadable: report the clean end the pipe showed
            // rather than invent a failure the process never had.
            Poll::Ready(Err(_)) => {
                self.child = ChildExit::Reported;
                return Poll::Ready(None);
            }
            Poll::Ready(Ok(status)) => status,
        };
        self.child = ChildExit::Reported;
        if status.success() {
            return Poll::Ready(None);
        }
        tracing::error!(
            %status,
            "ffmpeg exited non-zero mid clip export; ending transfer uncleanly"
        );
        Poll::Ready(Some(Err(std::io::Error::other(format!(
            "ffmpeg exited with {status}"
        )))))
    }
}

impl<R: tokio::io::AsyncRead + Unpin> Stream for ClipBody<R> {
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.as_mut().get_mut();
        match Pin::new(&mut this.inner).poll_next(cx) {
            Poll::Ready(Some(item)) => {
                // Progress (or a read error): re-arm the watchdog for the next gap.
                this.deadline = None;
                Poll::Ready(Some(item))
            }
            Poll::Ready(None) => {
                this.deadline = None;
                // A clean EOF on the pipe isn't necessarily a clean export: a
                // crashed ffmpeg also closes stdout. Wait for the exit status
                // before telling the client the transfer is complete, so a
                // mid-export crash ends the chunked response uncleanly instead
                // of delivering a silently truncated MP4.
                this.poll_child_exit(cx)
            }
            Poll::Pending => {
                // The device went quiet mid-export: end the body once the
                // stall timeout expires. Drop tears the child + permit down
                // right after the consumer sees the end of the stream.
                let stall = this.stall_timeout;
                let deadline = this
                    .deadline
                    .get_or_insert_with(|| Box::pin(tokio::time::sleep(stall)));
                match deadline.as_mut().poll(cx) {
                    Poll::Ready(()) => Poll::Ready(None),
                    Poll::Pending => Poll::Pending,
                }
            }
        }
    }
}

#[async_trait]
impl RtspClipSource for FfmpegRtspClip {
    async fn export(
        &self,
        rtsp_url: &str,
        filename: &str,
        max_seconds: u64,
        encoding: ClipEncoding,
    ) -> Result<ClipStream, DriverError> {
        if !rtsp_url.starts_with("rtsp://") {
            return Err(DriverError::Unsupported);
        }
        // Reserve from the shared cross-plane memory budget FIRST: it never
        // blocks (see `MediaBudget`'s "Never blocks" note), so an exhausted
        // budget fails this export fast, before this plane's own local slot is
        // even considered.
        let media_permit = self.media_budget.try_reserve(self.media_units, "clip")?;

        let permit = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| DriverError::Busy("all clip-export slots are busy".to_owned()))?;

        let duration = max_seconds.to_string();
        let mut child = super::spawn_media_process(
            Command::new(&self.ffmpeg)
                .args(clip_args(rtsp_url, &duration, encoding))
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true),
        )
        .await?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| DriverError::Transport("ffmpeg produced no stdout".to_string()))?;

        let mut body = ClipBody {
            inner: ReaderStream::with_capacity(stdout, 256 * 1024),
            child: ChildExit::Running(child),
            _permit: permit,
            _media_permit: media_permit,
            // Reuse the connect/seek timeout as the per-chunk stall bound too:
            // both describe how long this source may go quiet, and a second
            // config knob isn't worth it for one more adapter.
            stall_timeout: self.first_chunk_timeout,
            deadline: None,
        };

        // Demand the first chunk before resolving so a device with no footage
        // for the window is a real error, not an empty 200. The first bytes of a
        // fragmented MP4 are the `ftyp` box, a cheap container sanity check.
        let first = match tokio::time::timeout(self.first_chunk_timeout, body.next()).await {
            Err(_) => return Err(DriverError::Transport("clip export timed out".to_string())),
            Ok(None) => {
                return Err(DriverError::Device(
                    "device produced no footage for the requested window".to_string(),
                ));
            }
            Ok(Some(Err(error))) => {
                return Err(DriverError::Transport(format!(
                    "ffmpeg read failed: {error}"
                )));
            }
            Ok(Some(Ok(chunk))) => {
                if chunk.len() < 12 || &chunk[4..8] != b"ftyp" {
                    return Err(DriverError::Protocol(
                        "ffmpeg output was not an MP4 container".to_string(),
                    ));
                }
                chunk
            }
        };

        let bytes: Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>> =
            Box::pin(futures_util::stream::iter([Ok(first)]).chain(body));
        Ok(ClipStream {
            content_type: "video/mp4".to_string(),
            filename: filename.to_string(),
            bytes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adapter(ffmpeg: &str, slots: usize) -> FfmpegRtspClip {
        adapter_with_timeout(ffmpeg, Duration::from_millis(500), slots)
    }

    fn adapter_with_timeout(
        ffmpeg: &str,
        first_chunk_timeout: Duration,
        slots: usize,
    ) -> FfmpegRtspClip {
        // A generous, effectively-unlimited shared budget: these tests exercise
        // the LOCAL semaphore and error paths, not the cross-plane budget (see
        // `shared_media_budget_exhausted_fails_the_export_even_with_a_free_local_slot`
        // below for that).
        FfmpegRtspClip::new(
            ffmpeg,
            first_chunk_timeout,
            slots,
            MediaBudget::new(1024 * 1024 * 1024, 1),
            1,
        )
    }

    /// The RTSP-playback path serves every family without a native download.
    /// A caller asking for a browser-playable clip there must actually get a
    /// re-encode, or `?transcode=true` is a silent no-op on those devices.
    #[test]
    fn clip_args_honour_the_requested_encoding() {
        let copy = clip_args("rtsp://device/playback", "60", ClipEncoding::Copy);
        assert!(copy.windows(2).any(|pair| pair == ["-c:v", "copy"]));
        assert!(!copy.contains(&"libx264"));

        let transcoded = clip_args(
            "rtsp://device/playback",
            "60",
            ClipEncoding::BrowserCompatible,
        );
        assert!(
            transcoded
                .windows(2)
                .any(|pair| pair == ["-c:v", "libx264"])
        );
        assert!(!transcoded.windows(2).any(|pair| pair == ["-c:v", "copy"]));
    }

    /// The window cap must stay on the INPUT side in both encodings, or a long
    /// request would keep the device session open past its bound.
    #[test]
    fn clip_args_keep_the_duration_cap_in_both_encodings() {
        for encoding in [ClipEncoding::Copy, ClipEncoding::BrowserCompatible] {
            let args = clip_args("rtsp://device/playback", "42", encoding);
            let cap = args
                .iter()
                .position(|arg| *arg == "-t")
                .expect("-t present");
            assert_eq!(args[cap + 1], "42");
        }
    }

    #[tokio::test]
    async fn non_rtsp_url_is_rejected() {
        let clip = adapter("/definitely/not/ffmpeg", 1);
        let error = clip
            .export("http://evil.example/x", "c.mp4", 10, ClipEncoding::Copy)
            .await
            .err()
            .expect("non-rtsp rejected");
        assert!(matches!(error, DriverError::Unsupported));
    }

    #[tokio::test]
    async fn missing_binary_is_a_transport_error() {
        let clip = adapter("/definitely/not/ffmpeg", 1);
        let error = clip
            .export(
                "rtsp://device.test/playback",
                "c.mp4",
                10,
                ClipEncoding::Copy,
            )
            .await
            .err()
            .expect("missing binary fails");
        assert!(matches!(error, DriverError::Transport(_)));
    }

    #[tokio::test]
    async fn empty_output_is_a_device_error() {
        // `true` exits 0 with no bytes → "no footage".
        // Use a generous 10s budget so fork/exec under load does not trip a spurious timeout.
        let clip = adapter_with_timeout("true", Duration::from_secs(10), 1);
        let error = clip
            .export(
                "rtsp://device.test/playback",
                "c.mp4",
                10,
                ClipEncoding::Copy,
            )
            .await
            .err()
            .expect("empty output fails");
        assert!(matches!(error, DriverError::Device(_)));
    }

    #[tokio::test]
    async fn zero_slots_reject_as_busy() {
        let clip = adapter("true", 0);
        let error = clip
            .export(
                "rtsp://device.test/playback",
                "c.mp4",
                10,
                ClipEncoding::Copy,
            )
            .await
            .err()
            .expect("no slots busy");
        assert!(matches!(error, DriverError::Busy(_)));
    }

    /// Write an executable shell script that emits a fake `ftyp` header (so
    /// the first-chunk sanity check passes) and then goes quiet for `after`
    /// before exiting with `exit_code`. Used to simulate a stalled source
    /// (long `after`, exit 0) and a mid-export crash (short `after`, non-zero
    /// exit).
    fn header_then_script(after: &str, exit_code: u8) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::atomic::{AtomicU64, Ordering};
        // Process-wide counter: a unique path per call so concurrently
        // running tests never clobber (or delete) each other's script.
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "piping-rtsp-clip-test-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed),
        ));
        // 12 ASCII bytes: `chunk.len() >= 12 && chunk[4..8] == b"ftyp"` is all
        // the sanity check reads, so no real MP4 bytes are needed.
        std::fs::write(
            &path,
            format!("#!/bin/sh\nprintf 'AAAAftypABCD'\nsleep {after}\nexit {exit_code}\n"),
        )
        .expect("script written");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("script chmod");
        path
    }

    #[tokio::test]
    async fn stall_timeout_ends_a_clip_whose_source_goes_quiet_mid_export() {
        // Header immediately, then silence well past the (500ms) stall
        // timeout: the watchdog must end the body on its own instead of
        // pinning the export slot until the process is eventually killed.
        let script = header_then_script("5", 0);
        let clip = adapter(script.to_str().expect("utf-8 path"), 1);
        let stream = clip
            .export(
                "rtsp://device.test/playback",
                "c.mp4",
                10,
                ClipEncoding::Copy,
            )
            .await
            .expect("ftyp header must resolve the start");
        let mut bytes = stream.bytes;
        let first = bytes.next().await.expect("first chunk").expect("ok");
        assert!(!first.is_empty());

        // The next poll hits the stalled source; it must end (not hang) well
        // before the script's 5s sleep would otherwise let it produce more.
        let ended = tokio::time::timeout(Duration::from_secs(2), bytes.next()).await;
        assert!(
            matches!(ended, Ok(None)),
            "stalled source must end the body within the stall timeout"
        );

        // The slot must have been freed by the watchdog ending the stream.
        drop(bytes);
        let second = clip
            .export(
                "rtsp://device.test/playback",
                "c.mp4",
                10,
                ClipEncoding::Copy,
            )
            .await;
        assert!(
            second.is_ok(),
            "slot must be released once the stall ends the body"
        );
        let _ = std::fs::remove_file(&script);
    }

    #[tokio::test]
    async fn ffmpeg_crash_after_first_chunk_ends_the_body_with_an_error() {
        // Header immediately, then a fast non-zero exit: a mid-export crash.
        // The clean EOF on stdout must NOT be reported as a clean end of
        // stream — the client needs to see the transfer fail, not silently
        // receive a truncated MP4.
        let script = header_then_script("0", 7);
        let clip = adapter(script.to_str().expect("utf-8 path"), 1);
        let stream = clip
            .export(
                "rtsp://device.test/playback",
                "c.mp4",
                10,
                ClipEncoding::Copy,
            )
            .await
            .expect("ftyp header must resolve the start");
        let mut bytes = stream.bytes;
        let first = bytes.next().await.expect("first chunk").expect("ok");
        assert!(!first.is_empty());

        let next = tokio::time::timeout(Duration::from_secs(2), bytes.next())
            .await
            .expect("must not hang");
        let error = next
            .expect("stream must not end cleanly on a crash")
            .expect_err("non-zero ffmpeg exit must surface as a stream error");
        assert!(error.to_string().contains("ffmpeg exited"));
        let _ = std::fs::remove_file(&script);
    }

    #[tokio::test]
    async fn eof_long_before_exit_still_surfaces_the_crash() {
        // The same crash as above, but with the race the loaded CI node hits
        // and this machine does not: stdout is closed FIRST and the process
        // keeps living before failing. A single non-blocking status check in
        // that window answers "not finished", which reads as a clean end and
        // hands the client a truncated MP4 that looks complete. Above, the
        // exit is fast enough that the check usually happens to see it — so
        // that test passes with the defect present and only fails under load.
        // Here the window is the point, so the assertion cannot pass by luck.
        let script = closes_stdout_then_fails_script(7);
        let clip = adapter(script.to_str().expect("utf-8 path"), 1);
        let stream = clip
            .export(
                "rtsp://device.test/playback",
                "c.mp4",
                10,
                ClipEncoding::Copy,
            )
            .await
            .expect("ftyp header must resolve the start");
        let mut bytes = stream.bytes;
        let first = bytes.next().await.expect("first chunk").expect("ok");
        assert!(!first.is_empty());

        let next = tokio::time::timeout(Duration::from_secs(5), bytes.next())
            .await
            .expect("must not hang waiting for the exit status");
        let error = next
            .expect("a closed pipe with the process still alive is not a clean end")
            .expect_err("non-zero ffmpeg exit must surface as a stream error");
        assert!(error.to_string().contains("ffmpeg exited"));
        let _ = std::fs::remove_file(&script);
    }

    /// Fake ffmpeg that emits the header, closes stdout, and only then fails.
    /// Closing the pipe while alive is what opens the window between EOF and
    /// the exit status being readable.
    fn closes_stdout_then_fails_script(exit_code: u8) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "piping-rtsp-clip-eof-test-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::write(
            &path,
            format!("#!/bin/sh\nprintf 'AAAAftypABCD'\nexec 1>&-\nsleep 1\nexit {exit_code}\n"),
        )
        .expect("script written");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("script chmod");
        path
    }

    #[tokio::test]
    async fn shared_media_budget_exhausted_fails_the_export_even_with_a_free_local_slot() {
        // The local `slots` semaphore below has a free slot (max_exports = 1,
        // never touched by another caller in this test) — only the exhausted
        // cross-plane shared budget should be why this export fails.
        let budget = MediaBudget::new(1, 1); // exactly 1 unit total
        let _dummy = budget.try_reserve(1, "dummy").expect("dummy reservation");
        let clip = FfmpegRtspClip::new("true", Duration::from_millis(500), 1, budget, 1);
        let error = clip
            .export(
                "rtsp://device.test/playback",
                "c.mp4",
                10,
                ClipEncoding::Copy,
            )
            .await
            .err()
            .expect("exhausted shared budget must fail the export");
        assert!(matches!(error, DriverError::Busy(_)));
    }
}
