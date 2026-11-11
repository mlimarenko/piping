//! ffmpeg-backed [`RemuxSource`]: pipe a raw device container (Dahua `.dav`)
//! through `ffmpeg -c copy` into a streaming fragmented MP4.
//!
//! The device bytes are fed to `ffmpeg` **stdin** (the device file plane needs
//! Digest auth that ffmpeg's own HTTP input cannot perform, so the backend does
//! the download and pipes it) while the MP4 is read from **stdout** and streamed
//! to the client. `tokio::io::copy` over a [`StreamReader`] propagates
//! backpressure device → ffmpeg → response, so nothing is buffered whole; a
//! semaphore bounds concurrent remuxes and `kill_on_drop` + task-abort tear the
//! whole pipeline down when the download is dropped.
//!
//! Three more fences beyond the semaphore: [`RemuxSource::try_reserve`] lets a
//! caller (see `application::device_service::export_clip`) acquire the slot
//! BEFORE opening the device-side download, so an over-the-cap request never
//! dials the device at all; a device-read INACTIVITY timeout on the stdin pump
//! (reset on every chunk actually read from the device, not a flat wall-clock)
//! ends a clip whose device source goes quiet, without being tripped by a slow
//! or backgrounded CLIENT pacing the response; and an exit-status check on EOF
//! turns a mid-remux ffmpeg crash into an unclean stream ending instead of a
//! silently truncated MP4.

use std::future::Future;
use std::pin::Pin;
use std::process::Stdio;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinHandle;
use tokio_util::io::{ReaderStream, StreamReader};

use super::media_budget::{MediaBudget, MediaBudgetPermit};
use crate::contexts::device_management::application::remux::{RemuxReservation, RemuxSource};
use crate::contexts::device_management::application::rtsp_clip::ClipStream;
use crate::contexts::device_management::domain::{
    ClipEncoding, DownloadStream, DriverError, RemuxHints,
};

/// Build the ffmpeg argument list for one remux.
///
/// The shape is fixed — stdin in, `-c copy` out, fragmented MP4 — and the only
/// thing a driver can influence is the input frame rate, and only by supplying a
/// typed hint. Everything else stays out of driver reach on purpose.
fn remux_args(hints: RemuxHints, encoding: ClipEncoding) -> Vec<String> {
    let mut args: Vec<String> = ["-loglevel", "error", "-fflags", "+genpts"]
        .iter()
        .map(|arg| (*arg).to_owned())
        .collect();
    // A bare elementary stream carries no timing, so ffmpeg would assume 25 fps
    // and play a 12 fps recorder's archive at roughly double speed. The rate
    // must precede `-i` to apply to the INPUT rather than re-time the output.
    if let Some(fps) = hints.fps.filter(|fps| *fps > 0) {
        args.push("-r".to_owned());
        args.push(fps.to_string());
    }
    args.extend(
        [
            // Read the device container from stdin (backend digest-downloads it
            // and pumps it in; ffmpeg cannot do the Digest auth itself).
            "-i", "pipe:0", "-an",
        ]
        .iter()
        .map(|arg| (*arg).to_owned()),
    );
    args.extend(
        match encoding {
            // Remux only — no re-encode. Cheap, exact, ~0 CPU.
            ClipEncoding::Copy => ["-c:v", "copy"].as_slice(),
            // Re-encode to H.264 baseline. These recorders commonly write
            // H.265, which most browsers refuse to decode, so a faithful copy
            // is unwatchable for the operator who asked to watch. `ultrafast`
            // and `zerolatency` keep the first frames flowing quickly, since
            // the response streams as it is produced.
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
        }
        .iter()
        .map(|arg| (*arg).to_owned()),
    );
    args.extend(
        [
            "-movflags",
            "frag_keyframe+empty_moov+default_base_moof",
            "-f",
            "mp4",
            "-",
        ]
        .iter()
        .map(|arg| (*arg).to_owned()),
    );
    args
}

/// Remux a device container stream to a streaming MP4 via an `ffmpeg` subprocess.
#[derive(Clone)]
pub struct FfmpegRemux {
    ffmpeg: String,
    /// Timing limits for one native download/remux.
    timeouts: RemuxTimeouts,
    /// Concurrent-remux ceiling. Separate from the RTSP-clip ceiling: a native
    /// file download is far cheaper (no transcode, faster than realtime), so it
    /// gets its own, independently tunable budget.
    slots: Arc<Semaphore>,
    /// Process-wide memory budget shared with the other three CCTV media
    /// planes (still/live/clip); see [`MediaBudget`].
    media_budget: MediaBudget,
    /// Chunks of `media_budget` one remux reserves for its lifetime.
    media_units: u32,
}

/// Timing limits for one native download/remux.
#[derive(Clone, Copy)]
pub struct RemuxTimeouts {
    /// Cap on how long the FIRST MP4 chunk may take (device download start +
    /// ffmpeg container probe).
    pub first_chunk_timeout: Duration,
    /// Cap on the gap between successive DEVICE reads on the stdin pump.
    pub device_idle_timeout: Duration,
    /// Absolute wall-clock cap for one native download/remux, regardless of
    /// continuous progress.
    pub max_duration: Duration,
}

impl FfmpegRemux {
    /// Construct the adapter with `max_exports` concurrent slots, drawing
    /// `media_units` chunks from the shared `media_budget` per remux.
    pub fn new(
        ffmpeg: impl Into<String>,
        timeouts: RemuxTimeouts,
        max_exports: usize,
        media_budget: MediaBudget,
        media_units: u32,
    ) -> Self {
        Self {
            ffmpeg: ffmpeg.into(),
            timeouts,
            slots: Arc::new(Semaphore::new(max_exports)),
            media_budget,
            media_units,
        }
    }
}

/// [`RemuxReservation`] impl: the local semaphore permit plus the reservation
/// from the shared cross-plane [`MediaBudget`]. Dropping it (without ever
/// reaching [`FfmpegRemux::remux_reserved`]) releases both.
#[allow(
    dead_code,
    reason = "held only for its Drop effect (releases the local slot and the shared media-budget reservation)"
)]
struct Reservation {
    _local: OwnedSemaphorePermit,
    _media: MediaBudgetPermit,
}
impl RemuxReservation for Reservation {}

/// Copy `reader` into `writer` until EOF, bounding the gap between successive
/// DEVICE reads (not the total transfer time, and not writer-side backpressure)
/// by `idle_timeout`. Returns `Ok(())` on a clean EOF; an inactivity timeout or
/// I/O failure is returned as an error so the caller can log/signal truncation.
async fn copy_with_timeouts<R, W>(
    reader: &mut R,
    writer: &mut W,
    idle_timeout: Duration,
    max_duration: Duration,
) -> std::io::Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut buf = vec![0u8; 256 * 1024];
    let deadline = tokio::time::Instant::now() + max_duration;
    loop {
        let read_timeout =
            idle_timeout.min(deadline.saturating_duration_since(tokio::time::Instant::now()));
        if read_timeout.is_zero() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!(
                    "download exceeded maximum duration of {}s",
                    max_duration.as_secs()
                ),
            ));
        }
        let read = match tokio::time::timeout(read_timeout, reader.read(&mut buf)).await {
            Ok(result) => result?,
            Err(_) if tokio::time::Instant::now() >= deadline => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!(
                        "download exceeded maximum duration of {}s",
                        max_duration.as_secs()
                    ),
                ));
            }
            Err(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!("device produced no bytes for {}s", idle_timeout.as_secs()),
                ));
            }
        };
        if read == 0 {
            return Ok(());
        }
        // Writer-side backpressure (ffmpeg's stdin buffer draining slowly
        // because a client is reading the response slowly) is deliberately
        // NOT time-bounded here — only the device read above is.
        match tokio::time::timeout_at(deadline, writer.write_all(&buf[..read])).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) if error.kind() == std::io::ErrorKind::BrokenPipe => {
                // A broken pipe (ffmpeg exited / body dropped) just ends the copy,
                // matching the documented pump task contract.
                return Ok(());
            }
            Ok(Err(error)) => return Err(error),
            Err(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!(
                        "download exceeded maximum duration of {}s",
                        max_duration.as_secs()
                    ),
                ));
            }
        }
    }
}

/// Aborts the stdin-pump task when the response body is dropped, so a client
/// disconnect stops the device download instead of draining it in the void.
struct PumpGuard(Option<JoinHandle<std::io::Result<()>>>);

impl Drop for PumpGuard {
    fn drop(&mut self) {
        if let Some(task) = self.0.as_ref() {
            task.abort();
        }
    }
}

/// Owns the ffmpeg child wait task. Aborting the task drops the child, whose
/// `kill_on_drop` setting then terminates ffmpeg on a client disconnect.
struct ProcessGuard(JoinHandle<std::io::Result<std::process::ExitStatus>>);

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// The MP4 byte stream: a `ReaderStream` over ffmpeg stdout that OWNS the
/// process wait task, the concurrency permit, and the stdin-pump guard, so all
/// three are released when the download ends, with an exit-status check on EOF.
struct RemuxBody<R> {
    inner: ReaderStream<R>,
    pump: PumpGuard,
    process: Option<ProcessGuard>,
    process_succeeded: bool,
    deadline: Pin<Box<tokio::time::Sleep>>,
    _permit: Box<dyn RemuxReservation>,
}

impl<R: tokio::io::AsyncRead + Unpin> Stream for RemuxBody<R> {
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.as_mut().get_mut();
        if this.deadline.as_mut().poll(cx).is_ready() {
            if let Some(process) = this.process.as_mut() {
                process.0.abort();
            }
            this.process.take();
            return Poll::Ready(Some(Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "remux exceeded maximum duration",
            ))));
        }
        match Pin::new(&mut this.inner).poll_next(cx) {
            Poll::Ready(None) => {
                // A clean EOF on the pipe isn't necessarily a clean remux: a
                // crashed ffmpeg also closes stdout. Poll the child status before
                // telling the client the transfer is complete. `try_wait()` alone
                // is racy here because stdout can close just before the process is
                // reaped; polling `wait()` registers the task's waker until the
                // authoritative exit status is available.
                if !this.process_succeeded {
                    let Some(process) = this.process.as_mut() else {
                        return Poll::Ready(None);
                    };
                    match Pin::new(&mut process.0).poll(cx) {
                        Poll::Ready(Ok(Ok(status))) if !status.success() => {
                            this.process.take();
                            tracing::error!(
                                %status,
                                "ffmpeg exited non-zero mid clip remux; ending transfer uncleanly"
                            );
                            return Poll::Ready(Some(Err(std::io::Error::other(format!(
                                "ffmpeg exited with {status}"
                            )))));
                        }
                        Poll::Ready(Ok(Ok(_))) => {
                            this.process.take();
                            this.process_succeeded = true;
                        }
                        Poll::Ready(Ok(Err(error))) => {
                            this.process.take();
                            return Poll::Ready(Some(Err(std::io::Error::other(format!(
                                "failed to read ffmpeg exit status: {error}"
                            )))));
                        }
                        Poll::Ready(Err(error)) => {
                            this.process.take();
                            return Poll::Ready(Some(Err(std::io::Error::other(format!(
                                "ffmpeg wait task failed: {error}"
                            )))));
                        }
                        Poll::Pending => return Poll::Pending,
                    }
                }

                let Some(pump) = this.pump.0.as_mut() else {
                    return Poll::Ready(None);
                };
                match Pin::new(pump).poll(cx) {
                    Poll::Ready(Ok(Ok(()))) => {
                        this.pump.0.take();
                        Poll::Ready(None)
                    }
                    Poll::Ready(Ok(Err(error))) => {
                        this.pump.0.take();
                        Poll::Ready(Some(Err(error)))
                    }
                    Poll::Ready(Err(error)) => {
                        this.pump.0.take();
                        Poll::Ready(Some(Err(std::io::Error::other(format!(
                            "device download pump task failed: {error}"
                        )))))
                    }
                    Poll::Pending => Poll::Pending,
                }
            }
            other => other,
        }
    }
}

#[async_trait]
impl RemuxSource for FfmpegRemux {
    fn try_reserve(&self) -> Result<Box<dyn RemuxReservation>, DriverError> {
        // Reserve from the shared cross-plane memory budget FIRST: it never
        // blocks (see `MediaBudget`'s "Never blocks" note), so an exhausted
        // budget fails fast here -- before this plane's own local slot, and
        // before the caller ever dials the device -- are even considered.
        let media = self
            .media_budget
            .try_reserve(self.media_units, "download")?;
        let local = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| DriverError::Busy("all clip-download slots are busy".to_owned()))?;
        Ok(Box::new(Reservation {
            _local: local,
            _media: media,
        }))
    }

    async fn remux_reserved(
        &self,
        download: DownloadStream,
        reservation: Box<dyn RemuxReservation>,
        encoding: ClipEncoding,
    ) -> Result<ClipStream, DriverError> {
        let filename = format!("{}.mp4", download.filename);

        let mut child = super::spawn_media_process(
            Command::new(&self.ffmpeg)
                .args(remux_args(download.hints, encoding))
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true),
        )
        .await?;

        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| DriverError::Transport("ffmpeg produced no stdin".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| DriverError::Transport("ffmpeg produced no stdout".to_string()))?;

        // Pump the device bytes into ffmpeg stdin in the background. Backpressure
        // flows device → ffmpeg → this task; a broken pipe (ffmpeg exited / body
        // dropped) just ends the copy. Dropping `stdin` at task end signals EOF
        // so ffmpeg finalizes the MP4.
        let device_idle_timeout = self.timeouts.device_idle_timeout;
        let max_duration = self.timeouts.max_duration;
        let pump = tokio::spawn(async move {
            let mut reader = StreamReader::new(download.bytes);
            let result =
                copy_with_timeouts(&mut reader, &mut stdin, device_idle_timeout, max_duration)
                    .await;
            drop(stdin);
            result
        });

        let max_duration = self.timeouts.max_duration;
        let process = tokio::spawn(async move {
            match tokio::time::timeout(max_duration, child.wait()).await {
                Ok(result) => result,
                Err(_) => {
                    child.start_kill()?;
                    Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        format!(
                            "remux exceeded maximum duration of {}s",
                            max_duration.as_secs()
                        ),
                    ))
                }
            }
        });
        let mut body = RemuxBody {
            inner: ReaderStream::with_capacity(stdout, 256 * 1024),
            pump: PumpGuard(Some(pump)),
            process: Some(ProcessGuard(process)),
            process_succeeded: false,
            deadline: Box::pin(tokio::time::sleep(self.timeouts.max_duration)),
            _permit: reservation,
        };

        // Demand the first chunk before resolving so a window with no footage is a
        // real error, not an empty 200. A fragmented MP4 begins with the `ftyp`
        // box — a cheap container sanity check that the remux actually produced MP4.
        let first_chunk_timeout = self.timeouts.first_chunk_timeout;
        let first = match tokio::time::timeout(first_chunk_timeout, body.next()).await {
            Err(_) => {
                return Err(DriverError::Transport(
                    "clip download timed out".to_string(),
                ));
            }
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
            filename,
            bytes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adapter(ffmpeg: &str, slots: usize) -> FfmpegRemux {
        // A generous, effectively-unlimited shared budget: these tests exercise
        // the LOCAL semaphore and error paths, not the cross-plane budget (see
        // `shared_media_budget_exhausted_fails_the_reservation_even_with_a_free_local_slot`
        // below for that).
        //
        // The first-chunk budget is 10s, not the 500ms it used to be. No test
        // built from this adapter asserts a first-chunk timeout - they assert
        // error paths and the local semaphore - so the only thing a short
        // budget could do here was lose a race on a busy machine and report
        // `Transport("clip download timed out")` where the test expected the
        // real outcome. `empty_output_is_a_device_error` failed exactly that
        // way in CI: `true` exits at once, and forking it, reaping it and
        // observing the EOF took longer than half a second under load.
        FfmpegRemux::new(
            ffmpeg,
            RemuxTimeouts {
                first_chunk_timeout: Duration::from_secs(10),
                device_idle_timeout: Duration::from_secs(60),
                max_duration: Duration::from_secs(3600),
            },
            slots,
            MediaBudget::new(1024 * 1024 * 1024, 1),
            1,
        )
    }

    fn download() -> DownloadStream {
        DownloadStream {
            bytes: Box::pin(futures_util::stream::iter([Ok(Bytes::from_static(
                b"dav-bytes",
            ))])),
            filename: "clip".to_string(),
            hints: RemuxHints::default(),
        }
    }

    /// Without a rate hint the argument list stays exactly what a real container
    /// needs — the prober reads its own timing.
    #[test]
    fn remux_args_omit_the_rate_when_no_hint_is_supplied() {
        let args = remux_args(RemuxHints::default(), ClipEncoding::Copy);

        assert!(!args.contains(&"-r".to_owned()));
        assert_eq!(args.iter().position(|arg| arg == "-i"), Some(4));
    }

    /// A bare elementary stream needs its rate declared, and it must land BEFORE
    /// `-i` or ffmpeg re-times the output instead of interpreting the input —
    /// the difference between a correct clip and one playing at ~2x.
    #[test]
    fn remux_args_place_the_rate_hint_before_the_input() {
        let args = remux_args(RemuxHints { fps: Some(12) }, ClipEncoding::Copy);

        let rate = args
            .iter()
            .position(|arg| arg == "-r")
            .expect("rate hint present");
        let input = args
            .iter()
            .position(|arg| arg == "-i")
            .expect("input present");
        assert_eq!(args[rate + 1], "12");
        assert!(rate < input, "the rate must qualify the input: {args:?}");
    }

    /// A zero rate is not a rate; passing it through would make ffmpeg fail.
    #[test]
    fn remux_args_ignore_a_zero_rate_hint() {
        let args = remux_args(RemuxHints { fps: Some(0) }, ClipEncoding::Copy);

        assert!(!args.contains(&"-r".to_owned()));
    }

    /// A faithful copy keeps the device's own codec — that is the whole point of
    /// the cheap path, and it must never silently start re-encoding.
    #[test]
    fn remux_args_copy_the_video_stream_by_default() {
        let args = remux_args(RemuxHints::default(), ClipEncoding::Copy);

        assert!(args.windows(2).any(|pair| pair == ["-c:v", "copy"]));
        assert!(!args.iter().any(|arg| arg == "libx264"));
    }

    /// Asking for a browser-compatible clip must actually re-encode to H.264:
    /// these recorders commonly write H.265, which most browsers refuse.
    #[test]
    fn remux_args_transcode_to_h264_when_the_caller_asks_for_it() {
        let args = remux_args(RemuxHints::default(), ClipEncoding::BrowserCompatible);

        assert!(args.windows(2).any(|pair| pair == ["-c:v", "libx264"]));
        assert!(
            args.windows(2)
                .any(|pair| pair == ["-profile:v", "baseline"])
        );
        assert!(!args.windows(2).any(|pair| pair == ["-c:v", "copy"]));
    }

    /// The rate hint applies to the input in both encodings, or a 12 fps
    /// recorder's archive plays at roughly double speed either way.
    #[test]
    fn remux_args_keep_the_rate_hint_before_the_input_when_transcoding() {
        let args = remux_args(
            RemuxHints { fps: Some(12) },
            ClipEncoding::BrowserCompatible,
        );

        let rate = args.iter().position(|arg| arg == "-r").expect("rate hint");
        let input = args.iter().position(|arg| arg == "-i").expect("input");
        assert!(rate < input, "{args:?}");
    }

    #[tokio::test]
    async fn missing_binary_is_a_transport_error() {
        let remux = adapter("/definitely/not/ffmpeg", 1);
        let error = remux
            .remux(download(), ClipEncoding::Copy)
            .await
            .err()
            .expect("missing binary fails");
        assert!(matches!(error, DriverError::Transport(_)));
    }

    #[tokio::test]
    async fn empty_output_is_a_device_error() {
        // `true` exits 0 with no bytes → "no footage".
        let remux = adapter("true", 1);
        let error = remux
            .remux(download(), ClipEncoding::Copy)
            .await
            .err()
            .expect("empty output fails");
        assert!(matches!(error, DriverError::Device(_)));
    }

    #[tokio::test]
    async fn zero_slots_reject_as_busy() {
        let remux = adapter("true", 0);
        let error = remux
            .remux(download(), ClipEncoding::Copy)
            .await
            .err()
            .expect("no slots busy");
        assert!(matches!(error, DriverError::Busy(_)));
    }

    #[tokio::test]
    async fn try_reserve_acquires_before_any_device_io_and_reports_busy_when_exhausted() {
        let remux = adapter("true", 1);
        let reservation = remux.try_reserve().expect("first reservation succeeds");
        // The single slot is held: a second reservation attempt (which a caller
        // would make BEFORE dialing the device) must fail immediately.
        let error = remux
            .try_reserve()
            .err()
            .expect("second reservation must be busy");
        assert!(matches!(error, DriverError::Busy(_)));
        // Dropping the first reservation without ever remuxing releases the slot.
        drop(reservation);
        assert!(remux.try_reserve().is_ok());
    }

    #[tokio::test]
    async fn remux_reserved_uses_a_reservation_acquired_up_front() {
        let remux = adapter("true", 1);
        let reservation = remux.try_reserve().expect("reservation succeeds");
        // `true` produces no output, so this still resolves to the same
        // "no footage" error as the `remux()` convenience path — proving
        // `remux_reserved` runs the identical pipeline, just with an
        // externally-supplied reservation instead of acquiring its own.
        let error = remux
            .remux_reserved(download(), reservation, ClipEncoding::Copy)
            .await
            .err()
            .expect("empty output still fails the same way");
        assert!(matches!(error, DriverError::Device(_)));
    }

    /// Write an executable shell script that reads all of stdin (so the pump
    /// task's copy completes) then emits a fake `ftyp` header and exits.
    /// `exit_code` lets a test simulate a mid-remux crash.
    fn echo_then_exit_script(exit_code: u8) -> std::path::PathBuf {
        executable_script(&format!(
            "cat >/dev/null\nprintf 'AAAAftypABCD'\nexit {exit_code}"
        ))
    }

    fn executable_script(body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "piping-remux-test-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("script written");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("script chmod");
        path
    }

    #[tokio::test]
    async fn ffmpeg_crash_after_first_chunk_ends_the_body_with_an_error() {
        let script = echo_then_exit_script(7);
        let remux = adapter(script.to_str().expect("utf-8 path"), 1);
        let stream = remux
            .remux(download(), ClipEncoding::Copy)
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
    async fn waits_for_non_zero_exit_after_stdout_closes_before_process_exit() {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::atomic::{AtomicU64, Ordering};

        static SEQ: AtomicU64 = AtomicU64::new(0);
        let script = std::env::temp_dir().join(format!(
            "piping-remux-delayed-exit-test-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::write(
            &script,
            "#!/bin/sh\ncat >/dev/null\nprintf 'AAAAftypABCD'\nexec 1>&-\nsleep 0.1\nexit 9\n",
        )
        .expect("script written");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("script chmod");

        let remux = adapter(script.to_str().expect("utf-8 path"), 1);
        let stream = remux
            .remux(download(), ClipEncoding::Copy)
            .await
            .expect("ftyp header must resolve the start");
        let mut bytes = stream.bytes;
        bytes.next().await.expect("first chunk").expect("ok");

        let next = tokio::time::timeout(Duration::from_secs(2), bytes.next())
            .await
            .expect("must wait for the child exit without hanging");
        let error = next
            .expect("stream must not end cleanly while child status is pending")
            .expect_err("delayed non-zero exit must surface as a stream error");
        assert!(error.to_string().contains("ffmpeg exited"));
        let _ = std::fs::remove_file(&script);
    }

    // `copy_with_timeouts` is tested directly (not through a spawned
    // ffmpeg/cat subprocess): subprocess stdout buffering/scheduling is
    // nondeterministic under load and would make a timing assertion flaky at
    // this granularity. The function is a plain async helper with no process
    // dependency, so testing it in isolation is both deterministic and exactly
    // matches the unit under test.

    #[tokio::test]
    async fn device_idle_timeout_ends_the_copy_when_the_device_goes_silent() {
        use futures_util::stream;
        use tokio::io::duplex;

        // A device stream that emits one chunk, then never produces another
        // (and never closes) — simulates a device that goes silent mid-download.
        let head = stream::iter([Ok::<_, std::io::Error>(Bytes::from_static(b"partial"))]);
        let tail = stream::pending::<Result<Bytes, std::io::Error>>();
        let mut reader = StreamReader::new(Box::pin(head.chain(tail)));

        // A duplex pipe stands in for ffmpeg's stdin: bytes written to `writer`
        // land in `sink`, which this test never reads from (mirrors ffmpeg
        // consuming stdin without backpressure for a 7-byte write).
        let (mut writer, mut sink) = duplex(64 * 1024);
        let drain = tokio::spawn(async move {
            let mut buf = [0u8; 64];
            while sink.read(&mut buf).await.unwrap_or(0) > 0 {}
        });

        // A short idle timeout + a device that goes silent after one chunk: the
        // copy must give up well before any generous wall-clock cap would ever
        // have fired, regardless of how slowly a CLIENT paces the response (this
        // helper only bounds the reader side, never the writer side).
        let started = tokio::time::Instant::now();
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            copy_with_timeouts(
                &mut reader,
                &mut writer,
                Duration::from_millis(100),
                Duration::from_secs(5),
            ),
        )
        .await
        .expect("must not hang past the outer safety timeout");

        let error = result.expect_err("a stalled device must surface as an error, not clean EOF");
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert!(error.to_string().contains("no bytes"));
        // The bound that fired was the 100ms idle timeout, not the 5s cap
        // behind it. 2s discriminates between those two while leaving room for
        // a loaded scheduler; a tighter number would be measuring the machine.
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "idle timeout took {:?}, expected ~100ms",
            started.elapsed()
        );

        drop(writer);
        let _ = drain.await;
    }

    #[tokio::test]
    async fn device_idle_timeout_does_not_fire_while_the_device_keeps_sending() {
        use futures_util::stream;
        use tokio::io::duplex;

        // A steady trickle of chunks, each well inside the idle window — the
        // copy must run to completion (clean EOF) rather than timing out just
        // because the TOTAL transfer exceeds one idle-timeout interval. This is
        // the exact scenario the wall-clock-cap bug used to break: a slow but
        // continuously-progressing transfer must not be punished.
        let chunks: Vec<Result<Bytes, std::io::Error>> =
            (0..5).map(|_| Ok(Bytes::from_static(b"chunk-"))).collect();
        let mut reader = StreamReader::new(Box::pin(stream::iter(chunks).then(|item| async {
            tokio::time::sleep(Duration::from_millis(30)).await;
            item
        })));

        let (mut writer, mut sink) = duplex(64 * 1024);
        let mut received = Vec::new();
        let drain = tokio::spawn(async move {
            let mut buf = [0u8; 64];
            loop {
                match sink.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => received.extend_from_slice(&buf[..n]),
                }
            }
            received
        });

        // Idle timeout (100ms) is comfortably larger than the 30ms inter-chunk
        // gap, so five chunks spanning ~150ms total must complete cleanly.
        copy_with_timeouts(
            &mut reader,
            &mut writer,
            Duration::from_millis(100),
            Duration::from_secs(5),
        )
        .await
        .expect("steady progress within the idle window must not time out");
        drop(writer);

        let received = drain.await.expect("drain task completes");
        assert_eq!(received, b"chunk-chunk-chunk-chunk-chunk-".to_vec());
    }

    #[tokio::test]
    async fn absolute_timeout_ends_a_copy_despite_continuous_device_progress() {
        use futures_util::stream;
        use tokio::io::duplex;

        let chunks = stream::unfold((), |()| async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            Some((Ok::<_, std::io::Error>(Bytes::from_static(b"chunk")), ()))
        });
        let mut reader = StreamReader::new(Box::pin(chunks));
        let (mut writer, mut sink) = duplex(64 * 1024);
        let drain = tokio::spawn(async move {
            let mut buf = [0u8; 64];
            while sink.read(&mut buf).await.unwrap_or(0) > 0 {}
        });

        let started = tokio::time::Instant::now();
        let result = copy_with_timeouts(
            &mut reader,
            &mut writer,
            Duration::from_millis(100),
            Duration::from_millis(250),
        )
        .await;

        let error = result.expect_err("the absolute cap must end a continuously progressing copy");
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert!(error.to_string().contains("maximum duration"));
        // Discriminates the 250ms cap from no cap at all, without asking the
        // scheduler to be prompt.
        assert!(started.elapsed() < Duration::from_secs(2));
        drop(writer);
        let _ = drain.await;
    }

    #[tokio::test]
    async fn absolute_timeout_ends_ffmpeg_after_the_pump_finishes() {
        // The invariant: once the pump has finished and the child keeps
        // running, the ABSOLUTE cap is what ends it - not the idle timeout,
        // and not a clean EOF.
        //
        // The timings are chosen so the machine cannot decide the outcome. The
        // absolute cap used to be 100ms, which asked the host to fork a shell,
        // drain the download and print the header inside a tenth of a second;
        // on a loaded CI runner it lost that race and the cap fired before the
        // header, failing at `remux()` with the very error the test was
        // waiting for further down. The cap is now 5s (50x the old budget) and
        // the idle timeout is far beyond it, so idle can never be the cause of
        // the error being asserted.
        //
        // The script records its own pid and then `exec`s, so the long sleep
        // IS the child rather than a grandchild of a shell. That is what lets
        // the test check the process is gone afterwards - and it is also what
        // stops the test leaking: `kill_on_drop` reaps the direct child only,
        // so the previous `sleep 3600` outlived every run by an hour, six of
        // them at once on this machine.
        let pid_file = std::env::temp_dir().join(format!(
            "piping-remux-deadline-pid-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        let script = executable_script(&format!(
            "cat >/dev/null\nprintf 'AAAAftypABCD'\nprintf '%s' \"$$\" > {}\nexec sleep 3600",
            pid_file.display()
        ));
        let first_chunk_timeout = Duration::from_secs(10);
        let device_idle_timeout = Duration::from_secs(120);
        let max_duration = Duration::from_secs(5);
        let remux = FfmpegRemux::new(
            script.to_str().expect("utf-8 path"),
            RemuxTimeouts {
                first_chunk_timeout,
                device_idle_timeout,
                max_duration,
            },
            1,
            MediaBudget::new(1024 * 1024 * 1024, 1),
            1,
        );
        let stream = remux
            .remux(download(), ClipEncoding::Copy)
            .await
            .expect("ftyp header must resolve the start");
        let mut bytes = stream.bytes;
        bytes.next().await.expect("first chunk").expect("ok");

        // Generous relative to `max_duration`: this bound exists to stop a
        // hung test, not to measure anything.
        let error = tokio::time::timeout(max_duration * 6, bytes.next())
            .await
            .expect("absolute deadline must end the child")
            .expect("deadline must surface as stream error")
            .expect_err("deadline must not become clean EOF");

        assert!(error.to_string().contains("maximum duration"));

        // The name of this test is a claim about the CHILD, not just about the
        // stream. Without this the test passed with the child's own deadline
        // disabled, because the body's deadline reported the same message.
        let recorded = std::fs::read_to_string(&pid_file).expect("the child recorded its pid");
        let pid: i32 = recorded.trim().parse().expect("the pid is a number");
        let mut alive_after = true;
        for _ in 0..100 {
            if !std::path::Path::new(&format!("/proc/{pid}")).exists() {
                alive_after = false;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(
            !alive_after,
            "the deadline must end ffmpeg itself, but pid {pid} was still running"
        );

        let _ = std::fs::remove_file(&script);
        let _ = std::fs::remove_file(&pid_file);
    }

    #[tokio::test]
    async fn pump_failure_surfaces_even_when_ffmpeg_exits_successfully() {
        let script = echo_then_exit_script(0);
        let remux = adapter(script.to_str().expect("utf-8 path"), 1);
        let download = DownloadStream {
            bytes: Box::pin(futures_util::stream::iter([
                Ok(Bytes::from_static(b"dav-bytes")),
                Err(std::io::Error::other("device download failed")),
            ])),
            filename: "clip".to_owned(),
            hints: RemuxHints::default(),
        };

        let stream = remux
            .remux(download, ClipEncoding::Copy)
            .await
            .expect("ffmpeg emits a valid first chunk");
        let mut bytes = stream.bytes;
        bytes.next().await.expect("first chunk").expect("ok");
        let error = bytes
            .next()
            .await
            .expect("pump failure must not become clean EOF")
            .expect_err("pump failure must surface on the response stream");

        assert!(error.to_string().contains("device download failed"));
        let _ = std::fs::remove_file(&script);
    }

    #[tokio::test]
    async fn shared_media_budget_exhausted_fails_the_reservation_even_with_a_free_local_slot() {
        // The local `slots` semaphore below has a free slot (max_exports = 1,
        // never touched by another caller in this test) — only the exhausted
        // cross-plane shared budget should be why `try_reserve` fails.
        let budget = MediaBudget::new(1, 1); // exactly 1 unit total
        let _dummy = budget.try_reserve(1, "dummy").expect("dummy reservation");
        let remux = FfmpegRemux::new(
            "true",
            RemuxTimeouts {
                first_chunk_timeout: Duration::from_millis(500),
                device_idle_timeout: Duration::from_secs(60),
                max_duration: Duration::from_secs(3600),
            },
            1,
            budget,
            1,
        );
        let error = remux
            .try_reserve()
            .err()
            .expect("exhausted shared budget must fail the reservation");
        assert!(matches!(error, DriverError::Busy(_)));
    }
}
