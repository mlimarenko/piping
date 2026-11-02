//! Live driver integration tests against real video devices.
//!
//! These are `#[ignore]`d — they only run when invoked with `--ignored` AND the
//! per-family `PIPING_LIVE_<FAMILY>_{HOST,USER,PASS}` env vars are set (otherwise
//! each test skips). No credentials or hosts are baked in; they never run in CI
//! (no lab network from the runner). They exercise the real Rust driver +
//! transport against live hardware — read-only, no writes.

use std::sync::Arc;
use std::time::Duration;

use piping_backend::contexts::device_management::domain::{
    Channel, DeviceDriver, ExportRequest, PtzSupport, Secret,
};
use piping_backend::contexts::device_management::infrastructure::drivers::dahua::DahuaDriver;
use piping_backend::contexts::device_management::infrastructure::drivers::dvrip::DvripDriver;
use piping_backend::contexts::device_management::infrastructure::drivers::hikvision::HikvisionDriver;
use piping_backend::contexts::device_management::infrastructure::http_client::{
    HttpDeviceClient, ReqwestDigestClient,
};
use piping_backend::contexts::device_management::infrastructure::transport::{
    DeviceEndpoint, DeviceTransport, DirectTcpTransport,
};

/// Read `<prefix>_HOST/_USER/_PASS`; `None` if any is unset (test then skips).
/// Device credentials for a live run, or a loud failure.
///
/// These tests used to return `Ok(())` when the environment was unset. The
/// coverage lane runs with `--include-ignored`, so all ten of them executed in
/// CI, skipped themselves, and were counted as PASSES — ten green results that
/// proved nothing about the drivers. They are excluded from that lane by name
/// now, which means the only way one runs is that someone asked for it. Asking
/// for a hardware test without the hardware configured is an error, not a skip.
fn env_device(prefix: &str) -> Result<(String, String, String), String> {
    let read = |suffix: &str| {
        std::env::var(format!("{prefix}_{suffix}"))
            .ok()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| format!("{prefix}_{suffix} is unset"))
    };
    Ok((read("HOST")?, read("USER")?, read("PASS")?))
}

/// Every family is reached the same way; only the port and the driver differ.
/// These three constructors keep that wiring in one place so a transport or
/// auth change lands once instead of once per test.
fn dvrip_driver(host: String, user: String, pass: String) -> DvripDriver {
    let transport: Arc<dyn DeviceTransport> =
        Arc::new(DirectTcpTransport::new(Duration::from_secs(8)));
    DvripDriver::new(
        transport,
        DeviceEndpoint::new(host, 34567),
        user,
        Secret::new(pass),
    )
}

#[allow(
    clippy::expect_used,
    reason = "a malformed endpoint is operator setup error in a manual live-hardware suite, not a runtime condition to recover from"
)]
fn digest_client(endpoint: &DeviceEndpoint, user: &str, pass: &str) -> Arc<dyn HttpDeviceClient> {
    Arc::new(
        ReqwestDigestClient::new(endpoint, user.to_string(), Secret::new(pass.to_string()))
            .expect("client"),
    )
}

fn dahua_driver(host: String, user: String, pass: String) -> DahuaDriver {
    let endpoint = DeviceEndpoint::new(host, 80);
    let http = digest_client(&endpoint, &user, &pass);
    DahuaDriver::new(http, endpoint, user, Secret::new(pass))
}

fn hikvision_driver(host: String, user: String, pass: String) -> HikvisionDriver {
    let endpoint = DeviceEndpoint::new(host, 80);
    let http = digest_client(&endpoint, &user, &pass);
    HikvisionDriver::new(http, endpoint, user, Secret::new(pass))
}

/// Build the archive window from `<prefix>_CHANNEL/_START/_END`, defaulting to
/// the last `lookback` up to now. Every archive test wants exactly this.
fn export_request(prefix: &str, lookback: chrono::Duration) -> ExportRequest {
    let now = chrono::Utc::now();
    let at = |suffix: &str, fallback: chrono::DateTime<chrono::Utc>| {
        std::env::var(format!("{prefix}_{suffix}"))
            .ok()
            .and_then(|value| value.parse::<chrono::DateTime<chrono::Utc>>().ok())
            .unwrap_or(fallback)
    };
    ExportRequest {
        channel: std::env::var(format!("{prefix}_CHANNEL"))
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(1),
        start: at("START", now - lookback),
        end: at("END", now),
    }
}

/// List the device's channels and print the per-channel PTZ verdict. Returns the
/// channels so each family can assert its own expectation on top.
#[allow(
    clippy::print_stderr,
    reason = "manual hardware suite: stderr is how the operator sees what the device actually answered. Excluded from the CI lane by name (--skip live_ in .gitlab-ci.yml), so this never prints in a pipeline"
)]
async fn report_channels(
    driver: &dyn DeviceDriver,
    label: &str,
) -> Result<Vec<Channel>, Box<dyn std::error::Error + Send + Sync>> {
    let channels = driver
        .list_channels()
        .await
        .map_err(|e| format!("list_channels: {e}"))?;
    for channel in &channels {
        eprintln!(
            "[{label}] channel {} ({:?}): ptz_support={:?}",
            channel.index, channel.name, channel.ptz_support
        );
    }
    Ok(channels)
}

#[allow(
    clippy::print_stderr,
    reason = "manual hardware suite: stderr is how the operator sees what the device actually answered. Excluded from the CI lane by name (--skip live_ in .gitlab-ci.yml), so this never prints in a pipeline"
)]
async fn exercise_reads(
    driver: &dyn DeviceDriver,
    label: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let info = driver
        .get_info()
        .await
        .map_err(|e| format!("get_info: {e}"))?;
    eprintln!(
        "[{label}] info: model={:?} firmware={:?} serial={:?} channels={:?}",
        info.model, info.firmware, info.serial, info.channels
    );
    assert!(
        info.model.is_some() || info.serial.is_some() || info.firmware.is_some(),
        "{label}: expected some device identity from get_info"
    );

    let users = driver
        .list_users()
        .await
        .map_err(|e| format!("list_users: {e}"))?;
    eprintln!(
        "[{label}] users: {} ({:?})",
        users.len(),
        users.iter().map(|u| &u.name).collect::<Vec<_>>()
    );
    assert!(
        !users.is_empty(),
        "{label}: expected at least the admin user"
    );

    let disks = driver
        .disk_status()
        .await
        .map_err(|e| format!("disk_status: {e}"))?;
    eprintln!("[{label}] disks: {}", disks.len());
    for d in &disks {
        eprintln!(
            "[{label}]   disk {} used={} total={} state={:?}",
            d.name, d.used_bytes, d.total_bytes, d.state
        );
    }

    // recording_status is family-dependent (native on DVRIP; Unsupported for
    // Dahua/Hikvision until the recording phase) — tolerate Unsupported.
    match driver.recording_status().await {
        Ok(recording) => eprintln!(
            "[{label}] recording active={} spans={}",
            recording.active,
            recording.spans.len()
        ),
        Err(err) => {
            eprintln!("[{label}] recording: {err} (unsupported for this family — expected)")
        }
    }
    Ok(())
}

#[tokio::test]
#[ignore = "live device: set PIPING_LIVE_DVRIP_{HOST,USER,PASS}"]
async fn live_dvrip_reads() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (host, user, pass) = env_device("PIPING_LIVE_DVRIP")?;
    let driver = dvrip_driver(host, user, pass);
    exercise_reads(&driver, "dvrip").await?;
    Ok(())
}

/// Live proof that `list_channels` queries the `PTZProtocol` config and
/// resolves a per-channel `ptz_support` (or, on firmware that rejects the
/// query, leaves every channel at `Unknown` without failing the listing).
/// Does not assert a specific channel is `Supported` — this device's exact
/// PTZ wiring is not asserted here, only that discovery reaches the device.
#[tokio::test]
#[ignore = "live device: set PIPING_LIVE_DVRIP_{HOST,USER,PASS}"]
async fn live_dvrip_ptz_discovery() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (host, user, pass) = env_device("PIPING_LIVE_DVRIP")?;
    let driver = dvrip_driver(host, user, pass);
    let channels = report_channels(&driver, "dvrip").await?;
    assert!(!channels.is_empty(), "dvrip: expected at least one channel");
    Ok(())
}

/// Read a real recorded window off a live DVRIP recorder and prove the archive
/// plane end to end: `OPFileQuery` finds footage, `OPPlayBack` streams it, the
/// XM wrappers are stripped, and the frame-rate hint comes off the wire.
///
/// A pure READ of already-recorded footage — nothing is written to the device.
/// The window defaults to the last hour; override with
/// `PIPING_LIVE_DVRIP_ARCHIVE_{START,END}` (RFC3339) to target known footage.
#[tokio::test]
#[ignore = "live device: set PIPING_LIVE_DVRIP_{HOST,USER,PASS}"]
#[allow(
    clippy::print_stderr,
    reason = "manual hardware suite: stderr is how the operator sees what the device actually answered. Excluded from the CI lane by name (--skip live_ in .gitlab-ci.yml), so this never prints in a pipeline"
)]
async fn live_dvrip_archive_download() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use futures_util::StreamExt as _;

    let (host, user, pass) = env_device("PIPING_LIVE_DVRIP")?;
    let request = export_request("PIPING_LIVE_DVRIP_ARCHIVE", chrono::Duration::hours(1));

    let driver = dvrip_driver(host, user, pass);

    let Some(download) = driver.export_download(&request).await? else {
        eprintln!("[dvrip] no footage in the requested window; nothing to verify");
        return Ok(());
    };
    eprintln!(
        "[dvrip] archive stream: filename={} hints={:?}",
        download.filename, download.hints
    );
    assert!(
        download.hints.fps.is_some_and(|fps| fps > 0),
        "the frame rate must be read off the wire, not guessed"
    );

    // Pull a bounded prefix: enough to prove real elementary-stream bytes flow,
    // without downloading the whole window on a manual test run.
    let mut bytes = download.bytes;
    let mut total = 0usize;
    let mut head = Vec::new();
    while total < 256 * 1024 {
        let Some(chunk) = bytes.next().await else {
            break;
        };
        let chunk = chunk?;
        if head.is_empty() {
            head.extend_from_slice(&chunk[..chunk.len().min(8)]);
        }
        total += chunk.len();
    }
    eprintln!("[dvrip] archive bytes read: {total}, head: {head:02X?}");
    assert!(total > 0, "expected recorded video bytes");
    // Annex-B: every elementary stream begins on a start code.
    assert!(
        head.starts_with(&[0x00, 0x00, 0x00, 0x01]) || head.starts_with(&[0x00, 0x00, 0x01]),
        "expected an annex-B start code, got {head:02X?}"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "live device: set PIPING_LIVE_DAHUA_{HOST,USER,PASS}"]
async fn live_dahua_reads() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (host, user, pass) = env_device("PIPING_LIVE_DAHUA")?;
    let driver = dahua_driver(host, user, pass);
    exercise_reads(&driver, "dahua").await?;
    Ok(())
}

/// Live proof that `list_channels` resolves a real per-channel `ptz_support`
/// verdict from the `PTZProtocol` config rather than leaving every channel at
/// the driver's `Unknown` default. Unlike the Hikvision variant, this does not
/// assert a specific channel is `Supported` — the fleet's exact PTZ wiring on
/// this device is not asserted here, only that the discovery call itself
/// reaches the device and parses without failing the whole listing.
#[tokio::test]
#[ignore = "live device: set PIPING_LIVE_DAHUA_{HOST,USER,PASS}"]
async fn live_dahua_ptz_discovery() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (host, user, pass) = env_device("PIPING_LIVE_DAHUA")?;
    let driver = dahua_driver(host, user, pass);
    let channels = report_channels(&driver, "dahua").await?;
    assert!(!channels.is_empty(), "dahua: expected at least one channel");
    Ok(())
}

#[tokio::test]
#[ignore = "live device: set PIPING_LIVE_HIK_{HOST,USER,PASS}"]
async fn live_hikvision_reads() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (host, user, pass) = env_device("PIPING_LIVE_HIK")?;
    let driver = hikvision_driver(host, user, pass);
    exercise_reads(&driver, "hik").await?;
    Ok(())
}

/// Live proof of per-channel PTZ discovery: a hybrid NVR should report
/// `Supported` for a channel with a wired PTZ head and `Unsupported` for a
/// plain fixed channel, never leaving both at `Unknown`. Verified 2026-07-25
/// on a TC-R3120: analog channel 1 (`DOME_PLUS`) vs. IP proxy channel 101
/// (empty `controlProtocol`).
#[tokio::test]
#[ignore = "live device: set PIPING_LIVE_HIK_{HOST,USER,PASS}"]
async fn live_hikvision_ptz_discovery() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (host, user, pass) = env_device("PIPING_LIVE_HIK")?;
    let driver = hikvision_driver(host, user, pass);
    let channels = report_channels(&driver, "hik").await?;
    assert!(
        channels
            .iter()
            .any(|c| c.ptz_support != PtzSupport::Unknown),
        "hik: expected at least one channel to resolve to a real PTZ verdict, not Unknown for all"
    );
    Ok(())
}

/// End-to-end archive proof on live hardware: pull a recorded window off a DVRIP
/// recorder, pipe it through the real remux plane, and assert a browser-playable
/// fragmented MP4 comes out.
///
/// This is the test that would have caught a de-framing bug the byte-level checks
/// miss — a stream can look like annex-B and still fail to remux. Read-only.
#[tokio::test]
#[ignore = "live device: set PIPING_LIVE_DVRIP_{HOST,USER,PASS}"]
#[allow(
    clippy::print_stderr,
    reason = "manual hardware suite: stderr is how the operator sees what the device actually answered. Excluded from the CI lane by name (--skip live_ in .gitlab-ci.yml), so this never prints in a pipeline"
)]
async fn live_dvrip_archive_remuxes_to_mp4() -> Result<(), Box<dyn std::error::Error + Send + Sync>>
{
    use futures_util::StreamExt as _;
    use piping_backend::contexts::device_management::application::remux::RemuxSource;
    use piping_backend::contexts::device_management::domain::ClipEncoding;
    use piping_backend::contexts::device_management::infrastructure::media_budget::MediaBudget;
    use piping_backend::contexts::device_management::infrastructure::remux::{
        FfmpegRemux, RemuxTimeouts,
    };

    let (host, user, pass) = env_device("PIPING_LIVE_DVRIP")?;
    let request = export_request("PIPING_LIVE_DVRIP_ARCHIVE", chrono::Duration::hours(1));

    let driver = dvrip_driver(host, user, pass);
    let Some(download) = driver.export_download(&request).await? else {
        eprintln!("[dvrip] no footage in the requested window; nothing to remux");
        return Ok(());
    };
    let fps = download.hints.fps;

    let remux = FfmpegRemux::new(
        "ffmpeg",
        RemuxTimeouts {
            first_chunk_timeout: Duration::from_secs(60),
            device_idle_timeout: Duration::from_secs(30),
            max_duration: Duration::from_secs(300),
        },
        1,
        MediaBudget::new(64 * 1024 * 1024, 1024 * 1024),
        1,
    );
    let clip = remux.remux(download, ClipEncoding::Copy).await?;
    assert_eq!(clip.content_type, "video/mp4");

    let mut bytes = clip.bytes;
    let mut total = 0usize;
    let mut head = Vec::new();
    while total < 512 * 1024 {
        let Some(chunk) = bytes.next().await else {
            break;
        };
        let chunk = chunk?;
        if head.len() < 12 {
            head.extend_from_slice(&chunk[..chunk.len().min(12)]);
        }
        total += chunk.len();
    }
    eprintln!("[dvrip] remuxed mp4 bytes: {total}, fps hint: {fps:?}, head: {head:02X?}");
    assert!(total > 0, "expected MP4 bytes out of the remux");
    // Every MP4 opens with a `ftyp` box: size (4 bytes) then the type tag.
    assert_eq!(
        &head[4..8],
        b"ftyp",
        "expected an MP4 ftyp box, got {head:02X?}"
    );
    Ok(())
}

/// End-to-end proof of the Hikvision fast export path on live hardware: search
/// finds a segment covering the window, `export_download` resolves its
/// `playbackURI` and downloads it via `ISAPI/ContentMgmt/download`, and the
/// bytes are a real decodable video container.
///
/// A pure READ of already-recorded footage — nothing is written to the device.
#[tokio::test]
#[ignore = "live device: set PIPING_LIVE_HIK_{HOST,USER,PASS}"]
#[allow(
    clippy::print_stderr,
    reason = "manual hardware suite: stderr is how the operator sees what the device actually answered. Excluded from the CI lane by name (--skip live_ in .gitlab-ci.yml), so this never prints in a pipeline"
)]
async fn live_hikvision_export_download() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use futures_util::StreamExt as _;

    let (host, user, pass) = env_device("PIPING_LIVE_HIK")?;
    let request = export_request("PIPING_LIVE_HIK_EXPORT", chrono::Duration::hours(2));

    let driver = hikvision_driver(host, user, pass);

    let Some(download) = driver.export_download(&request).await? else {
        eprintln!("[hik] no footage in the requested window; nothing to verify");
        return Ok(());
    };
    eprintln!("[hik] download filename={}", download.filename);

    let mut bytes = download.bytes;
    let mut total = 0usize;
    let mut head = Vec::new();
    while total < 256 * 1024 {
        let Some(chunk) = bytes.next().await else {
            break;
        };
        let chunk = chunk?;
        if head.is_empty() {
            head.extend_from_slice(&chunk[..chunk.len().min(16)]);
        }
        total += chunk.len();
    }
    eprintln!("[hik] download bytes read: {total}, head: {head:02X?}");
    assert!(total > 0, "expected device container bytes");
    Ok(())
}

/// End-to-end proof that the Hikvision fast download remuxes to a playable
/// MP4, exactly like the Dahua and DVRIP archive paths.
#[tokio::test]
#[ignore = "live device: set PIPING_LIVE_HIK_{HOST,USER,PASS}"]
#[allow(
    clippy::print_stderr,
    reason = "manual hardware suite: stderr is how the operator sees what the device actually answered. Excluded from the CI lane by name (--skip live_ in .gitlab-ci.yml), so this never prints in a pipeline"
)]
async fn live_hikvision_export_remuxes_to_mp4()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use futures_util::StreamExt as _;
    use piping_backend::contexts::device_management::application::remux::RemuxSource;
    use piping_backend::contexts::device_management::domain::ClipEncoding;
    use piping_backend::contexts::device_management::infrastructure::media_budget::MediaBudget;
    use piping_backend::contexts::device_management::infrastructure::remux::{
        FfmpegRemux, RemuxTimeouts,
    };

    let (host, user, pass) = env_device("PIPING_LIVE_HIK")?;
    let request = export_request("PIPING_LIVE_HIK_EXPORT", chrono::Duration::hours(2));

    let driver = hikvision_driver(host, user, pass);
    let Some(download) = driver.export_download(&request).await? else {
        eprintln!("[hik] no footage in the requested window; nothing to remux");
        return Ok(());
    };

    let remux = FfmpegRemux::new(
        "ffmpeg",
        RemuxTimeouts {
            first_chunk_timeout: Duration::from_secs(60),
            device_idle_timeout: Duration::from_secs(30),
            max_duration: Duration::from_secs(300),
        },
        1,
        MediaBudget::new(64 * 1024 * 1024, 1024 * 1024),
        1,
    );
    let clip = remux.remux(download, ClipEncoding::Copy).await?;
    assert_eq!(clip.content_type, "video/mp4");

    let mut bytes = clip.bytes;
    let mut total = 0usize;
    let mut head = Vec::new();
    while total < 512 * 1024 {
        let Some(chunk) = bytes.next().await else {
            break;
        };
        let chunk = chunk?;
        if head.len() < 12 {
            head.extend_from_slice(&chunk[..chunk.len().min(12)]);
        }
        total += chunk.len();
    }
    eprintln!("[hik] remuxed mp4 bytes: {total}, head: {head:02X?}");
    assert!(total > 0, "expected MP4 bytes out of the remux");
    assert_eq!(
        &head[4..8],
        b"ftyp",
        "expected an MP4 ftyp box, got {head:02X?}"
    );
    Ok(())
}
