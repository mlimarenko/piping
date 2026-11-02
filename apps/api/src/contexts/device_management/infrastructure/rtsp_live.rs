//! ffmpeg-backed [`RtspLiveSource`]: restream an RTSP feed as multipart MJPEG
//! by keeping a bounded `ffmpeg` transcode running for the lifetime of the
//! consumer.
//!
//! Same reachability caveat as [`rtsp_still`](super::rtsp_still): ffmpeg
//! connects to the device DIRECTLY, so this only works where the backend
//! shares a network route with the device.
//!
//! Resource fencing:
//! - a semaphore caps concurrent transcodes ([`LiveStreamError::Busy`] beyond);
//! - the FIRST chunk must arrive within `first_frame_timeout` or the start
//!   fails (so the HTTP layer can still answer with a real error status);
//! - an inter-chunk `stall_timeout` ends a stream whose device went quiet;
//! - the child is `kill_on_drop`, and the stream owns it — the consumer
//!   dropping the HTTP response body is what tears the transcode down.

use std::pin::Pin;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use tokio::process::Command;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};
use tokio::task::JoinHandle;
use tokio_util::io::ReaderStream;

use super::media_budget::{MediaBudget, MediaBudgetPermit};
use crate::contexts::device_management::application::rtsp_live::{
    LiveStreamError, LiveStreamOptions, MjpegStream, RtspLiveSource,
};
use crate::contexts::device_management::domain::DriverError;

/// The multipart boundary ffmpeg's `mpjpeg` muxer emits.
const FFMPEG_MPJPEG_BOUNDARY: &str = "ffmpeg";

/// Restream RTSP as multipart MJPEG via an `ffmpeg` subprocess.
#[derive(Clone)]
pub struct FfmpegRtspLive {
    /// Path or name of the ffmpeg binary (resolved on `PATH` when bare).
    ffmpeg: String,
    /// Timing limits for one live session.
    timeouts: LiveTimeouts,
    /// Concurrent-transcode slots; a full house answers `Busy` (HTTP 429).
    slots: Arc<Semaphore>,
    /// Process-wide memory budget shared with the other three CCTV media
    /// planes (still/clip/download); see [`MediaBudget`].
    media_budget: MediaBudget,
    /// Chunks of `media_budget` one live session reserves for its whole
    /// (potentially many-hour) lifetime.
    media_units: u32,
}

/// Timing limits for one live MJPEG session.
#[derive(Clone, Copy)]
pub struct LiveTimeouts {
    /// Cap on how long the first chunk may take (covers RTSP connect+decode).
    pub first_frame_timeout: Duration,
    /// Cap on the gap between chunks once streaming; expiry ends the stream.
    pub stall_timeout: Duration,
    /// Absolute wall-clock cap on ONE live session. `stall_timeout` only catches
    /// a device that goes *quiet*; a healthy stream that nobody closes (an
    /// abandoned tab, a half-open TCP) would otherwise pin a transcode slot and
    /// ~300 MB forever. This bounds that leak — the session ends gracefully when
    /// it expires (the viewer reconnects if still watching).
    pub max_session_duration: Duration,
}

impl FfmpegRtspLive {
    /// Construct the adapter with `max_streams` concurrent transcode slots,
    /// drawing `media_units` chunks from the shared `media_budget` per stream.
    pub fn new(
        ffmpeg: impl Into<String>,
        timeouts: LiveTimeouts,
        max_streams: usize,
        media_budget: MediaBudget,
        media_units: u32,
    ) -> Self {
        Self {
            ffmpeg: ffmpeg.into(),
            timeouts,
            slots: Arc::new(Semaphore::new(max_streams)),
            media_budget,
            media_units,
        }
    }
}

/// Aborts the independently-driven ffmpeg lifecycle task when the response body
/// is dropped. The task owns the child and permits, so its stall/session timers
/// continue to fire even while downstream backpressure stops polling the body.
struct LiveTaskGuard {
    task: JoinHandle<()>,
    permits: Arc<Mutex<Option<(OwnedSemaphorePermit, MediaBudgetPermit)>>>,
}

impl Drop for LiveTaskGuard {
    fn drop(&mut self) {
        self.task.abort();
        self.permits
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
    }
}

struct FfmpegBody {
    receiver: mpsc::Receiver<Result<Bytes, std::io::Error>>,
    _task: LiveTaskGuard,
}

impl Stream for FfmpegBody {
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.receiver.poll_recv(cx)
    }
}

/// Whether the live pump should keep running after one step.
enum PumpStep {
    /// The chunk was forwarded; keep pumping.
    Continue,
    /// The session is over (deadline, stall, or the consumer went away).
    Stop,
}

/// Log that a live session hit its absolute wall-clock cap. Kept in one place so
/// both deadline branches share a single `tracing` call site.
fn log_session_deadline_reached() {
    tracing::debug!("cctv live session reached max duration; ending stream");
}

/// Await the next device chunk, bounded by the inter-chunk `stall_timeout` and
/// raced against the absolute `session_deadline`. `None` means the session is
/// over: the deadline fired, the device stalled, or it reached EOF.
async fn next_chunk<R>(
    chunks: &mut ReaderStream<R>,
    stall_timeout: Duration,
    session_deadline: &mut Pin<&mut tokio::time::Sleep>,
) -> Option<Result<Bytes, std::io::Error>>
where
    R: tokio::io::AsyncRead + Unpin,
{
    tokio::select! {
        () = session_deadline.as_mut() => {
            log_session_deadline_reached();
            None
        }
        next = tokio::time::timeout(stall_timeout, chunks.next()) => next.ok().flatten(),
    }
}

/// Forward one device chunk to the consumer, racing the absolute
/// `session_deadline` and a fresh inter-chunk `stall_timeout` against the send
/// (which applies backpressure). `Stop` when any of them ends the session or the
/// consumer is gone.
async fn forward_chunk(
    item: Result<Bytes, std::io::Error>,
    sender: &mpsc::Sender<Result<Bytes, std::io::Error>>,
    stall_timeout: Duration,
    session_deadline: &mut Pin<&mut tokio::time::Sleep>,
) -> PumpStep {
    tokio::select! {
        () = session_deadline.as_mut() => {
            log_session_deadline_reached();
            PumpStep::Stop
        }
        () = tokio::time::sleep(stall_timeout) => PumpStep::Stop,
        result = sender.send(item) => {
            if result.is_err() {
                PumpStep::Stop
            } else {
                PumpStep::Continue
            }
        }
    }
}

async fn drive_live_stream<R>(
    stdout: R,
    mut child: tokio::process::Child,
    permits: Arc<Mutex<Option<(OwnedSemaphorePermit, MediaBudgetPermit)>>>,
    stall_timeout: Duration,
    max_session_duration: Duration,
    sender: mpsc::Sender<Result<Bytes, std::io::Error>>,
) where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut chunks = ReaderStream::with_capacity(stdout, 64 * 1024);
    let session_deadline = tokio::time::sleep(max_session_duration);
    tokio::pin!(session_deadline);
    loop {
        let Some(item) = next_chunk(&mut chunks, stall_timeout, &mut session_deadline).await else {
            break;
        };
        let step = forward_chunk(item, &sender, stall_timeout, &mut session_deadline).await;
        if let PumpStep::Stop = step {
            break;
        }
    }
    let _ = child.kill().await;
    permits
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .take();
}

#[async_trait]
impl RtspLiveSource for FfmpegRtspLive {
    async fn mjpeg(
        &self,
        rtsp_url: &str,
        options: LiveStreamOptions,
    ) -> Result<MjpegStream, LiveStreamError> {
        // Scheme pin: mirror the still plane — never let a regressed driver
        // steer ffmpeg at file:/http: inputs.
        if !rtsp_url.starts_with("rtsp://") {
            return Err(DriverError::Unsupported.into());
        }

        // Reserve from the shared cross-plane memory budget FIRST: it never
        // blocks (see `MediaBudget`'s "Never blocks" note), so an exhausted
        // budget fails this start fast, before this plane's own local slot is
        // even considered.
        let media_permit = self.media_budget.try_reserve(self.media_units, "live")?;

        let permit = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| LiveStreamError::Busy)?;

        // `-f mpjpeg` writes multipart parts delimited by `--ffmpeg`, exactly
        // what a `multipart/x-mixed-replace; boundary=ffmpeg` response needs.
        // The filter derives output shape from the clamped options; `-2` keeps
        // the scaled height codec-friendly (even), and `min(w,iw)` never
        // upscales.
        let filter = format!(
            "fps={fps},scale='min({width},iw)':-2",
            fps = options.fps,
            width = options.max_width,
        );
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
                    "-an",
                    "-vf",
                    &filter,
                    "-c:v",
                    "mjpeg",
                    "-q:v",
                    "6",
                    "-f",
                    "mpjpeg",
                    "-",
                ])
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

        let permits = Arc::new(Mutex::new(Some((permit, media_permit))));
        let (sender, receiver) = mpsc::channel(1);
        let task = tokio::spawn(drive_live_stream(
            stdout,
            child,
            Arc::clone(&permits),
            self.timeouts.stall_timeout,
            self.timeouts.max_session_duration,
            sender,
        ));
        let mut body = FfmpegBody {
            receiver,
            _task: LiveTaskGuard { task, permits },
        };

        // Demand the first chunk before resolving, so a dead device is a real
        // error response instead of a 200 that immediately ends.
        use futures_util::StreamExt;
        let first_frame_timeout = self.timeouts.first_frame_timeout;
        let first = match tokio::time::timeout(first_frame_timeout, body.next()).await {
            Err(_) => {
                return Err(DriverError::Transport("device read timed out".to_string()).into());
            }
            Ok(None) => {
                return Err(
                    DriverError::Device("device stream produced no video".to_string()).into(),
                );
            }
            Ok(Some(Err(error))) => {
                return Err(DriverError::Transport(format!("ffmpeg read failed: {error}")).into());
            }
            Ok(Some(Ok(chunk))) => chunk,
        };

        let bytes: Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>> =
            Box::pin(futures_util::stream::iter([Ok(first)]).chain(body));
        Ok(MjpegStream {
            boundary: FFMPEG_MPJPEG_BOUNDARY,
            bytes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;

    fn adapter(ffmpeg: &str, slots: usize) -> FfmpegRtspLive {
        // A generous, effectively-unlimited shared budget: these tests exercise
        // the LOCAL semaphore and error paths, not the cross-plane budget (see
        // `shared_media_budget_exhausted_fails_the_start_even_with_a_free_local_slot`
        // below for that).
        FfmpegRtspLive::new(
            ffmpeg,
            LiveTimeouts {
                first_frame_timeout: Duration::from_millis(500),
                stall_timeout: Duration::from_millis(500),
                max_session_duration: Duration::from_secs(3600),
            },
            slots,
            MediaBudget::new(1024 * 1024 * 1024, 1),
            1,
        )
    }

    #[tokio::test]
    async fn session_deadline_ends_a_healthy_stream_that_nobody_closes() {
        // A producer that streams forever + a tiny session cap: the stream must
        // END on its own (Poll::Ready(None)) rather than run indefinitely.
        let script = endless_producer_script();
        let live = FfmpegRtspLive::new(
            script.to_str().expect("utf-8 path"),
            LiveTimeouts {
                first_frame_timeout: Duration::from_millis(500),
                stall_timeout: Duration::from_secs(3600),
                max_session_duration: Duration::from_millis(200),
            },
            1,
            MediaBudget::new(1024 * 1024 * 1024, 1),
            1,
        );
        let stream = live
            .mjpeg("rtsp://device.test/stream", LiveStreamOptions::default())
            .await
            .expect("healthy producer must start");
        let mut bytes = stream.bytes;
        // Drain until the session deadline ends the stream; must terminate.
        let ended = tokio::time::timeout(Duration::from_secs(5), async {
            while bytes.next().await.is_some() {}
        })
        .await;
        assert!(
            ended.is_ok(),
            "session must end within the deadline, not hang"
        );
        let _ = std::fs::remove_file(&script);
    }

    #[tokio::test]
    async fn session_deadline_releases_capacity_without_downstream_polling() {
        let script = endless_producer_script();
        let live = FfmpegRtspLive::new(
            script.to_str().expect("utf-8 path"),
            LiveTimeouts {
                first_frame_timeout: Duration::from_millis(500),
                stall_timeout: Duration::from_secs(3600),
                max_session_duration: Duration::from_millis(100),
            },
            1,
            MediaBudget::new(1024 * 1024 * 1024, 1),
            1,
        );
        let first = live
            .mjpeg("rtsp://device.test/stream", LiveStreamOptions::default())
            .await
            .expect("healthy producer must start");

        tokio::time::sleep(Duration::from_millis(250)).await;
        let second = live
            .mjpeg("rtsp://device.test/stream", LiveStreamOptions::default())
            .await;

        assert!(
            second.is_ok(),
            "the absolute watchdog must release the slot even while the first body is backpressured"
        );
        drop(first);
        let _ = std::fs::remove_file(&script);
    }

    #[tokio::test]
    async fn stall_deadline_releases_capacity_without_downstream_polling() {
        let script = endless_producer_script();
        let live = FfmpegRtspLive::new(
            script.to_str().expect("utf-8 path"),
            LiveTimeouts {
                first_frame_timeout: Duration::from_millis(500),
                stall_timeout: Duration::from_millis(100),
                max_session_duration: Duration::from_secs(3600),
            },
            1,
            MediaBudget::new(1024 * 1024 * 1024, 1),
            1,
        );
        let first = live
            .mjpeg("rtsp://device.test/stream", LiveStreamOptions::default())
            .await
            .expect("producer must emit its initial chunk");

        tokio::time::sleep(Duration::from_millis(250)).await;
        let second = live
            .mjpeg("rtsp://device.test/stream", LiveStreamOptions::default())
            .await;

        assert!(
            second.is_ok(),
            "the stall watchdog must release the slot even while the first body is backpressured"
        );
        drop(first);
        let _ = std::fs::remove_file(&script);
    }

    #[tokio::test]
    async fn non_rtsp_url_is_rejected_before_spawning_anything() {
        let live = adapter("/definitely/not/ffmpeg", 1);
        let error = live
            .mjpeg("http://evil.example/file", LiveStreamOptions::default())
            .await
            .expect_err("non-rtsp must be rejected");
        assert!(matches!(
            error,
            LiveStreamError::Driver(DriverError::Unsupported)
        ));
    }

    #[tokio::test]
    async fn missing_binary_is_a_transport_error() {
        let live = adapter("/definitely/not/ffmpeg", 1);
        let error = live
            .mjpeg("rtsp://device.test/stream", LiveStreamOptions::default())
            .await
            .expect_err("missing binary must fail");
        assert!(matches!(
            error,
            LiveStreamError::Driver(DriverError::Transport(_))
        ));
    }

    #[tokio::test]
    async fn instant_eof_is_a_device_error_not_an_empty_200() {
        // `true` exits immediately with no output: the "device produced no
        // video" path.
        let live = adapter("true", 1);
        let error = live
            .mjpeg("rtsp://device.test/stream", LiveStreamOptions::default())
            .await
            .expect_err("instant EOF must fail the start");
        assert!(matches!(
            error,
            LiveStreamError::Driver(DriverError::Device(_))
        ));
    }

    #[tokio::test]
    async fn zero_slots_answer_busy() {
        let live = adapter("true", 0);
        let error = live
            .mjpeg("rtsp://device.test/stream", LiveStreamOptions::default())
            .await
            .expect_err("no slots must be Busy");
        assert!(matches!(error, LiveStreamError::Busy));
    }

    /// Write an executable shell script that ignores the ffmpeg argv and just
    /// streams zeros forever — a stand-in for a healthy transcode. (Real
    /// binaries like `yes` choke on the `-nostdin`-style args they receive.)
    fn endless_producer_script() -> std::path::PathBuf {
        executable_script("exec cat /dev/zero")
    }

    fn executable_script(body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::atomic::{AtomicU64, Ordering};
        // A process-wide counter guarantees a unique path per call, so tests
        // running concurrently never clobber (or delete) each other's script.
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "piping-rtsp-live-test-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("script written");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("script chmod");
        path
    }

    #[tokio::test]
    async fn producing_binary_streams_its_output_and_frees_the_slot() {
        // The first chunk resolves the start; dropping the stream kills the
        // child and releases the slot for the next caller.
        let script = endless_producer_script();
        let live = adapter(script.to_str().expect("utf-8 path"), 1);
        let stream = live
            .mjpeg("rtsp://device.test/stream", LiveStreamOptions::default())
            .await
            .expect("healthy producer must start");
        assert_eq!(stream.boundary, "ffmpeg");
        let mut bytes = stream.bytes;
        let first = bytes.next().await.expect("first chunk").expect("ok");
        assert!(!first.is_empty());
        drop(bytes);

        // The permit must come back — the next start succeeds (not Busy).
        let second = live
            .mjpeg("rtsp://device.test/stream", LiveStreamOptions::default())
            .await;
        assert!(second.is_ok(), "slot must be released on drop");
        let _ = std::fs::remove_file(&script);
    }

    #[tokio::test]
    async fn shared_media_budget_exhausted_fails_the_start_even_with_a_free_local_slot() {
        // The local `slots` semaphore below has a free slot (max_streams = 1,
        // never touched by another caller in this test) — only the exhausted
        // cross-plane shared budget should be why this start fails.
        let budget = MediaBudget::new(1, 1); // exactly 1 unit total
        let _dummy = budget.try_reserve(1, "dummy").expect("dummy reservation");
        let live = FfmpegRtspLive::new(
            "true",
            LiveTimeouts {
                first_frame_timeout: Duration::from_millis(500),
                stall_timeout: Duration::from_millis(500),
                max_session_duration: Duration::from_secs(3600),
            },
            1,
            budget,
            1,
        );
        let error = live
            .mjpeg("rtsp://device.test/stream", LiveStreamOptions::default())
            .await
            .expect_err("exhausted shared budget must fail the start");
        assert!(matches!(
            error,
            LiveStreamError::Driver(DriverError::Busy(_))
        ));
    }
}
