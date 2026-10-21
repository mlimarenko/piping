use std::sync::Arc;
use std::time::Duration;

use redis::aio::ConnectionManager;

use crate::app::device_ssh_exec::ProxyDeviceSshExec;
use crate::app::{AppRepositories, AppState};
use crate::contexts::access_control::application::{AccessControlService, PolicyEvaluationService};
use crate::contexts::access_control::infrastructure::persistence::{
    SqlxPolicyRepository, SqlxRoleRepository, SqlxUserRepository,
};
use crate::contexts::device_management::application::{
    DeviceDriverRegistry, DeviceManagementService,
};
use crate::contexts::device_management::infrastructure::media_budget::MediaBudget;
use crate::contexts::device_management::infrastructure::remux::{FfmpegRemux, RemuxTimeouts};
use crate::contexts::device_management::infrastructure::resolver::{
    ConfiguredDeviceResolver, ConnConfDeviceLaunchAccounts, ResolverDependencies,
};
use crate::contexts::device_management::infrastructure::rtsp_clip::FfmpegRtspClip;
use crate::contexts::device_management::infrastructure::rtsp_live::{FfmpegRtspLive, LiveTimeouts};
use crate::contexts::device_management::infrastructure::rtsp_still::FfmpegRtspStill;
use crate::contexts::device_management::infrastructure::transport::DirectTcpTransport;

/// TCP connect timeout when a device driver dials a device's management port.
/// Matches the conservative 5s ceiling used across the proxy connect paths.
const DEVICE_MANAGEMENT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// ffmpeg binary used for the RTSP-keyframe still fallback. Overridable via
/// `PIPING_CCTV__FFMPEG_PATH` for images that ship it under a different name.
const CCTV_FFMPEG_PATH_ENV: &str = "PIPING_CCTV__FFMPEG_PATH";

/// Read a `usize` env override (`PIPING_CCTV__<KEY>`), falling back to `default`
/// when unset or unparseable. Lets an operator raise the concurrency ceilings
/// for a bigger host without a rebuild.
fn cctv_env_usize(key: &str, default: usize) -> usize {
    let value = std::env::var(format!("PIPING_CCTV__{key}")).ok();
    parse_positive_usize(value.as_deref(), default)
}

fn parse_positive_usize(value: Option<&str>, default: usize) -> usize {
    value
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(default)
}

/// Read a seconds-valued `Duration` env override, falling back to `default`.
fn cctv_env_secs(key: &str, default: u64) -> Duration {
    let value = std::env::var(format!("PIPING_CCTV__{key}")).ok();
    Duration::from_secs(parse_positive_u64(value.as_deref(), default))
}

fn parse_positive_u64(value: Option<&str>, default: u64) -> u64 {
    value
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(default)
}

/// Read an opt-in boolean override. Unknown values fail closed to `default`.
fn cctv_env_bool(key: &str, default: bool) -> bool {
    let value = std::env::var(format!("PIPING_CCTV__{key}")).ok();
    parse_bool(value.as_deref(), default)
}

fn parse_bool(value: Option<&str>, default: bool) -> bool {
    value
        .and_then(|value| match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            _ => None,
        })
        .unwrap_or(default)
}

/// Upper bound on a decoded keyframe JPEG we will buffer.
const CCTV_RTSP_STILL_MAX_BYTES: usize = 8 * 1024 * 1024;
const CCTV_STILL_MAX_GRABS_DEFAULT: usize = 8;
const CCTV_LIVE_MAX_STREAMS_DEFAULT: usize = 8;
const CCTV_CLIP_MAX_EXPORTS_DEFAULT: usize = 3;
const CCTV_DOWNLOAD_MAX_EXPORTS_DEFAULT: usize = 3;

/// All CCTV media-plane limits, resolved from env with safe defaults. Env keys
/// are `PIPING_CCTV__<FIELD_UPPERCASE>` — e.g. `PIPING_CCTV__LIVE_MAX_STREAMS=32`
/// on a large host. See `docs`/`DRIVER_MATRIX.md` for tuning guidance.
struct CctvLimits {
    still_timeout: Duration,
    still_max_grabs: usize,
    live_first_frame_timeout: Duration,
    live_stall_timeout: Duration,
    live_max_session_duration: Duration,
    live_max_streams: usize,
    clip_first_chunk_timeout: Duration,
    clip_max_exports: usize,
    download_first_chunk_timeout: Duration,
    download_idle_timeout: Duration,
    download_max_duration: Duration,
    download_max_exports: usize,
}

impl CctvLimits {
    fn from_env() -> Self {
        Self {
            still_timeout: cctv_env_secs("STILL_TIMEOUT_SECS", 12),
            still_max_grabs: cctv_env_usize("STILL_MAX_GRABS", CCTV_STILL_MAX_GRABS_DEFAULT),
            live_first_frame_timeout: cctv_env_secs("LIVE_FIRST_FRAME_TIMEOUT_SECS", 15),
            live_stall_timeout: cctv_env_secs("LIVE_STALL_TIMEOUT_SECS", 30),
            // MEMORY-BOUND, not CPU-bound: each ffmpeg H264->MJPEG transcode holds
            // ~300MB, so ~10 is the ceiling on a 4 GiB host — default 8. Raise
            // `PIPING_CCTV__LIVE_MAX_STREAMS` on a bigger host, or move to go2rtc
            // (deferred design: PIP-120).
            // Generous leak-backstop (12h): the common abandoned-stream cases are
            // already handled (backgrounded tabs unmount the stream client-side; a
            // dead client surfaces as a socket write error), so this only bounds a
            // rare lingering half-open connection and essentially never disrupts a
            // legitimate 24/7 wall. Lower it for tighter resource control.
            live_max_session_duration: cctv_env_secs("LIVE_MAX_SESSION_SECS", 12 * 60 * 60),
            live_max_streams: cctv_env_usize("LIVE_MAX_STREAMS", CCTV_LIVE_MAX_STREAMS_DEFAULT),
            clip_first_chunk_timeout: cctv_env_secs("CLIP_FIRST_CHUNK_TIMEOUT_SECS", 90),
            clip_max_exports: cctv_env_usize("CLIP_MAX_EXPORTS", CCTV_CLIP_MAX_EXPORTS_DEFAULT),
            // The native-download path (Dahua `loadfile`) is far cheaper than an
            // RTSP-playback copy — `-c copy` remux, faster than realtime — so it
            // gets its own, independently tunable ceiling. Raise
            // `PIPING_CCTV__DOWNLOAD_MAX_EXPORTS` on a bigger host.
            download_first_chunk_timeout: cctv_env_secs("DOWNLOAD_FIRST_CHUNK_TIMEOUT_SECS", 90),
            download_idle_timeout: cctv_env_secs("DOWNLOAD_IDLE_TIMEOUT_SECS", 90),
            // Overall ceiling on one native download+remux. Generous (a fast
            // loadfile pull of an hour of footage runs ~15-20 min), but bounds a
            // slow-drip device from pinning a slot forever. 1h default.
            download_max_duration: cctv_env_secs("DOWNLOAD_MAX_DURATION_SECS", 3600),
            download_max_exports: cctv_env_usize(
                "DOWNLOAD_MAX_EXPORTS",
                CCTV_DOWNLOAD_MAX_EXPORTS_DEFAULT,
            ),
        }
    }
}

/// Total shared cross-plane media memory budget, in megabytes. Env override:
/// `PIPING_CCTV__MEDIA_BUDGET_TOTAL_MB`.
///
/// Sized so the LIVE plane — by far the most expensive, and the only one whose
/// per-session cost is measured in hundreds of megabytes — can actually reach
/// its own concurrency ceiling: `live_max_streams` (8) × `CCTV_MEDIA_UNITS_LIVE`
/// (30) = 240 units, so the budget must hold at least 240 units. At the 10MB
/// default unit that is 2400MB. The previous 2048MB yielded 204 units, which
/// silently capped live at 6 concurrent streams while advertising 8 and logged
/// the plane-exceeds-budget warning on every boot. `media_budget_is_consistent`
/// below pins this invariant so the two numbers cannot drift apart again.
const CCTV_MEDIA_BUDGET_TOTAL_MB_DEFAULT: usize = 2400;
/// Size of one shared-budget unit, in megabytes. Env override:
/// `PIPING_CCTV__MEDIA_BUDGET_UNIT_MB`.
const CCTV_MEDIA_BUDGET_UNIT_MB_DEFAULT: usize = 10;

/// Per-plane shared-budget unit cost. STILL is a short one-shot JPEG grab
/// (~1 unit); LIVE holds a decode for the whole session (~300MB, matching the
/// documented per-stream ffmpeg cost noted on `CctvLimits::live_max_session_duration`
/// above — ~30 units at the 10MB default); CLIP/DOWNLOAD stream/remux without
/// buffering (~1 unit each).
const CCTV_MEDIA_UNITS_STILL: u32 = 1;
const CCTV_MEDIA_UNITS_LIVE: u32 = 30;
const CCTV_MEDIA_UNITS_CLIP: u32 = 1;
const CCTV_MEDIA_UNITS_DOWNLOAD: u32 = 1;

/// Read the two shared-budget env knobs (in MB), falling back to the safe
/// defaults above.
fn cctv_media_budget_config_mb() -> (usize, usize) {
    (
        cctv_env_usize("MEDIA_BUDGET_TOTAL_MB", CCTV_MEDIA_BUDGET_TOTAL_MB_DEFAULT),
        cctv_env_usize("MEDIA_BUDGET_UNIT_MB", CCTV_MEDIA_BUDGET_UNIT_MB_DEFAULT),
    )
}

/// Warn (never fail) when one CCTV media plane's own concurrency ceiling,
/// multiplied by its `media_units` cost, could ask the shared budget for more
/// units than it holds in total. This is a misconfiguration footgun, not a
/// crash: the plane's own semaphore is unaffected -- the shared-budget gate
/// just starts 429-ing that plane before its own concurrency ceiling is ever
/// reached.
fn warn_if_plane_could_exceed_media_budget(
    plane: &str,
    max_concurrency: usize,
    media_units: u32,
    total_mb: usize,
    unit_mb: usize,
) {
    let total_units = media_budget_units(total_mb, unit_mb);
    if plane_could_exceed_media_budget(max_concurrency, media_units, total_mb, unit_mb) {
        tracing::warn!(
            plane,
            max_concurrency,
            media_units,
            total_units,
            "CCTV media plane concurrency x media_units exceeds the shared media \
             budget; the shared-budget gate will 429 this plane before its own \
             concurrency ceiling is reached -- raise \
             PIPING_CCTV__MEDIA_BUDGET_TOTAL_MB or lower this plane's concurrency \
             ceiling"
        );
    }
}

const BYTES_PER_MEBIBYTE: usize = 1024 * 1024;

fn mebibytes_to_bytes(mebibytes: usize) -> usize {
    mebibytes.saturating_mul(BYTES_PER_MEBIBYTE)
}

fn media_budget_units(total_mb: usize, unit_mb: usize) -> usize {
    mebibytes_to_bytes(total_mb)
        .checked_div(mebibytes_to_bytes(unit_mb).max(1))
        .unwrap_or(0)
        .max(1)
}

fn plane_could_exceed_media_budget(
    max_concurrency: usize,
    media_units: u32,
    total_mb: usize,
    unit_mb: usize,
) -> bool {
    max_concurrency.saturating_mul(media_units as usize) > media_budget_units(total_mb, unit_mb)
}

/// Process-wide memory budget shared by every CCTV media-plane adapter
/// (still/live/clip/download); the ONLY call site for `MediaBudget::new`.
/// Sized from `PIPING_CCTV__MEDIA_BUDGET_TOTAL_MB` /
/// `PIPING_CCTV__MEDIA_BUDGET_UNIT_MB` (defaults: 2048MB total / 10MB units).
fn media_budget() -> MediaBudget {
    static MEDIA_BUDGET: std::sync::OnceLock<MediaBudget> = std::sync::OnceLock::new();
    shared_media_budget(&MEDIA_BUDGET, cctv_media_budget_config_mb)
}

fn shared_media_budget(
    cell: &std::sync::OnceLock<MediaBudget>,
    config: impl FnOnce() -> (usize, usize),
) -> MediaBudget {
    cell.get_or_init(|| {
        let (total_mb, unit_mb) = config();
        MediaBudget::new(mebibytes_to_bytes(total_mb), mebibytes_to_bytes(unit_mb))
    })
    .clone()
}

pub fn build_access_control_service_from_repos(
    repositories: &AppRepositories,
) -> AccessControlService<SqlxUserRepository, SqlxRoleRepository, SqlxPolicyRepository> {
    AccessControlService::new(
        repositories.users.clone(),
        repositories.roles.clone(),
        repositories.policies.clone(),
    )
}

pub fn build_policy_evaluation_service_from_repos(
    repositories: &AppRepositories,
) -> PolicyEvaluationService<SqlxUserRepository, SqlxRoleRepository, SqlxPolicyRepository> {
    PolicyEvaluationService::new(build_access_control_service_from_repos(repositories))
}

pub fn build_access_control_service_with_cache_from_repos(
    repositories: &AppRepositories,
    policy_cache: ConnectionManager,
) -> AccessControlService<SqlxUserRepository, SqlxRoleRepository, SqlxPolicyRepository> {
    AccessControlService::new_with_cache(
        repositories.users.clone(),
        repositories.roles.clone(),
        repositories.policies.clone(),
        policy_cache,
    )
}

pub fn build_policy_evaluation_service_with_cache_from_repos(
    repositories: &AppRepositories,
    policy_cache: ConnectionManager,
) -> PolicyEvaluationService<SqlxUserRepository, SqlxRoleRepository, SqlxPolicyRepository> {
    PolicyEvaluationService::new(build_access_control_service_with_cache_from_repos(
        repositories,
        policy_cache,
    ))
}

pub fn access_control_service(
    state: &AppState,
) -> Arc<AccessControlService<SqlxUserRepository, SqlxRoleRepository, SqlxPolicyRepository>> {
    state.services.access_control.clone()
}

pub fn policy_evaluation_service(
    state: &AppState,
) -> Arc<PolicyEvaluationService<SqlxUserRepository, SqlxRoleRepository, SqlxPolicyRepository>> {
    state.services.policy_evaluation.clone()
}

pub fn build_access_control_service(
    state: &AppState,
) -> Arc<AccessControlService<SqlxUserRepository, SqlxRoleRepository, SqlxPolicyRepository>> {
    access_control_service(state)
}

pub fn build_policy_evaluation_service(
    state: &AppState,
) -> Arc<PolicyEvaluationService<SqlxUserRepository, SqlxRoleRepository, SqlxPolicyRepository>> {
    policy_evaluation_service(state)
}

/// Assemble the read-only device-management service for a request.
///
/// Repositories are cheap `Clone`s, the transport is a `Duration`, and the
/// registry holds an `Arc`, so building this per request is inexpensive — the
/// same shape as [`build_policy_evaluation_service`]. The direct TCP transport
/// is correct for the embedded-proxy / dev path; production proxy-node routing
/// lands with its own transport in a later phase.
#[must_use]
pub fn build_device_management_service(state: &AppState) -> DeviceManagementService {
    let transport = Arc::new(DirectTcpTransport::new(DEVICE_MANAGEMENT_CONNECT_TIMEOUT));
    // The Linux family runs account commands through the embedded proxy: launch
    // material is resolved per target by the shared connection service.
    let registry = DeviceDriverRegistry::new(transport).with_ssh_exec(Arc::new(
        ProxyDeviceSshExec::new(state.services.connection.clone()),
    ));
    let launch_accounts = Arc::new(ConnConfDeviceLaunchAccounts::new(
        Arc::new(state.repositories.connconf_records.clone()),
        Arc::new(state.repositories.connconf_assignments.clone()),
    ));
    let resolver = Arc::new(ConfiguredDeviceResolver::new(ResolverDependencies {
        connection_targets: Arc::new(state.repositories.connection_targets.clone()),
        scopes: Arc::new(state.repositories.connconf_assignments.clone()),
        platforms: Arc::new(state.repositories.platforms.clone()),
        config: Arc::new(state.repositories.config_contexts.clone()),
        secret_bindings: Arc::new(state.repositories.secrets.clone()),
        launch_accounts,
        record_bindings: Arc::new(state.repositories.secrets.clone()),
        secret_store: state.services.secret_store.clone(),
    }));
    // Authorization for device-mutating operations is an RBAC decision made at
    // the route layer (`cctv` resource); the service itself carries no gate.
    //
    // Attach the RTSP-keyframe still fallback so families whose native snapshot
    // is unsupported (e.g. XMeye/Sofia firmware rejecting `OPSNAP`) still yield
    // a picture from their live stream. It is only effective where the backend
    // can reach the device directly (this `DirectTcpTransport` deployment); see
    // `FfmpegRtspStill` for the proxy-node caveat.
    let ffmpeg = std::env::var(CCTV_FFMPEG_PATH_ENV).unwrap_or_else(|_| "ffmpeg".to_string());
    // The service is built per-request, but the ffmpeg adapters must be shared:
    // their semaphores are the process-wide grab/transcode ceilings, so a fresh
    // adapter per request would defeat the cap. One of each, once — the limits
    // are read from env at first init (`CctvLimits::from_env`).
    static RTSP_STILL: std::sync::OnceLock<Arc<FfmpegRtspStill>> = std::sync::OnceLock::new();
    let rtsp_still = RTSP_STILL
        .get_or_init(|| {
            let limits = CctvLimits::from_env();
            let (total_mb, unit_mb) = cctv_media_budget_config_mb();
            warn_if_plane_could_exceed_media_budget(
                "still",
                limits.still_max_grabs,
                CCTV_MEDIA_UNITS_STILL,
                total_mb,
                unit_mb,
            );
            Arc::new(FfmpegRtspStill::new(
                ffmpeg.clone(),
                limits.still_timeout,
                CCTV_RTSP_STILL_MAX_BYTES,
                limits.still_max_grabs,
                media_budget(),
                CCTV_MEDIA_UNITS_STILL,
            ))
        })
        .clone();
    static RTSP_CLIP: std::sync::OnceLock<Arc<FfmpegRtspClip>> = std::sync::OnceLock::new();
    let rtsp_clip = RTSP_CLIP
        .get_or_init(|| {
            let limits = CctvLimits::from_env();
            let (total_mb, unit_mb) = cctv_media_budget_config_mb();
            warn_if_plane_could_exceed_media_budget(
                "clip",
                limits.clip_max_exports,
                CCTV_MEDIA_UNITS_CLIP,
                total_mb,
                unit_mb,
            );
            Arc::new(FfmpegRtspClip::new(
                ffmpeg.clone(),
                limits.clip_first_chunk_timeout,
                limits.clip_max_exports,
                media_budget(),
                CCTV_MEDIA_UNITS_CLIP,
            ))
        })
        .clone();
    static RTSP_LIVE: std::sync::OnceLock<Arc<FfmpegRtspLive>> = std::sync::OnceLock::new();
    let rtsp_live = RTSP_LIVE
        .get_or_init(|| {
            let limits = CctvLimits::from_env();
            let (total_mb, unit_mb) = cctv_media_budget_config_mb();
            warn_if_plane_could_exceed_media_budget(
                "live",
                limits.live_max_streams,
                CCTV_MEDIA_UNITS_LIVE,
                total_mb,
                unit_mb,
            );
            Arc::new(FfmpegRtspLive::new(
                ffmpeg.clone(),
                LiveTimeouts {
                    first_frame_timeout: limits.live_first_frame_timeout,
                    stall_timeout: limits.live_stall_timeout,
                    max_session_duration: limits.live_max_session_duration,
                },
                limits.live_max_streams,
                media_budget(),
                CCTV_MEDIA_UNITS_LIVE,
            ))
        })
        .clone();
    static REMUX: std::sync::OnceLock<Arc<FfmpegRemux>> = std::sync::OnceLock::new();
    let remux = REMUX
        .get_or_init(|| {
            let limits = CctvLimits::from_env();
            let (total_mb, unit_mb) = cctv_media_budget_config_mb();
            warn_if_plane_could_exceed_media_budget(
                "download",
                limits.download_max_exports,
                CCTV_MEDIA_UNITS_DOWNLOAD,
                total_mb,
                unit_mb,
            );
            Arc::new(FfmpegRemux::new(
                ffmpeg,
                RemuxTimeouts {
                    first_chunk_timeout: limits.download_first_chunk_timeout,
                    device_idle_timeout: limits.download_idle_timeout,
                    max_duration: limits.download_max_duration,
                },
                limits.download_max_exports,
                media_budget(),
                CCTV_MEDIA_UNITS_DOWNLOAD,
            ))
        })
        .clone();
    DeviceManagementService::new(resolver, registry)
        // Substreams are an operator-controlled rollout: keep the conservative
        // main stream unless each deployed family was verified read-only.
        .with_live_prefer_substream(cctv_env_bool("LIVE_PREFER_SUBSTREAM", false))
        .with_rtsp_still(rtsp_still)
        .with_rtsp_live(rtsp_live)
        .with_rtsp_clip(rtsp_clip)
        .with_remux(remux)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, OnceLock};

    use super::*;

    #[test]
    fn positive_usize_parser_rejects_empty_zero_malformed_and_overflowing_values() {
        let default = 17;
        assert_eq!(parse_positive_usize(None, default), default);
        for invalid in [
            "",
            "   ",
            "0",
            "-1",
            "not-a-number",
            "184467440737095516160",
        ] {
            assert_eq!(parse_positive_usize(Some(invalid), default), default);
        }
        assert_eq!(parse_positive_usize(Some(" 23 "), default), 23);
        assert_eq!(
            parse_positive_usize(Some(&usize::MAX.to_string()), default),
            usize::MAX
        );
    }

    #[test]
    fn positive_seconds_parser_rejects_zero_invalid_and_u64_overflow() {
        let default = 29;
        assert_eq!(parse_positive_u64(None, default), default);
        for invalid in ["", "0", "-1", "1.5", "18446744073709551616"] {
            assert_eq!(parse_positive_u64(Some(invalid), default), default);
        }
        assert_eq!(parse_positive_u64(Some(" 31 "), default), 31);
        assert_eq!(
            parse_positive_u64(Some(&u64::MAX.to_string()), default),
            u64::MAX
        );
    }

    #[test]
    fn boolean_parser_accepts_documented_forms_and_fails_closed() {
        for enabled in ["1", "true", "TRUE", " yes ", "on"] {
            assert!(parse_bool(Some(enabled), false));
        }
        for disabled in ["0", "false", "FALSE", " no ", "off"] {
            assert!(!parse_bool(Some(disabled), true));
        }
        assert!(!parse_bool(None, false));
        assert!(parse_bool(None, true));
        assert!(!parse_bool(Some("enabled"), false));
        assert!(parse_bool(Some("enabled"), true));
    }

    #[test]
    fn media_budget_math_handles_exact_capacity_zeroes_and_overflow() {
        assert!(!plane_could_exceed_media_budget(8, 30, 2400, 10));
        assert!(plane_could_exceed_media_budget(9, 30, 2400, 10));
        assert_eq!(media_budget_units(0, 0), 1);
        assert_eq!(mebibytes_to_bytes(usize::MAX), usize::MAX);
        assert!(plane_could_exceed_media_budget(
            usize::MAX,
            u32::MAX,
            usize::MAX,
            usize::MAX
        ));
    }

    #[test]
    fn concurrent_initialization_returns_one_shared_media_budget() {
        let cell = Arc::new(OnceLock::new());
        let budgets = std::thread::scope(|scope| {
            let handles = (0..8)
                .map(|_| {
                    let cell = Arc::clone(&cell);
                    scope.spawn(move || shared_media_budget(&cell, || (1, 1)))
                })
                .collect::<Vec<_>>();
            handles
                .into_iter()
                .map(|handle| handle.join().expect("initializer thread must not panic"))
                .collect::<Vec<_>>()
        });

        let permit = budgets[0]
            .try_reserve(1, "test")
            .expect("the shared one-unit budget starts available");
        for budget in &budgets[1..] {
            assert!(
                budget.try_reserve(1, "test").is_err(),
                "all callers must contend for the same process-wide unit"
            );
        }
        drop(permit);
        assert!(budgets[1].try_reserve(1, "test").is_ok());
    }

    /// Every CCTV media plane must be able to reach its OWN concurrency ceiling
    /// against the shipped shared budget. When it cannot, the shared-budget gate
    /// 429s the plane before its local semaphore is ever saturated: the plane
    /// advertises a ceiling it can never reach, and the mismatch surfaces to the
    /// operator as a busy error rather than as a capacity limit.
    ///
    /// `warn_if_plane_could_exceed_media_budget` already reports this at runtime,
    /// but a warning in a boot log is not a gate — the shipped defaults were
    /// inconsistent for the live plane until this test existed.
    #[test]
    fn shipped_media_budget_defaults_admit_every_plane_at_full_concurrency() {
        let total_units =
            (CCTV_MEDIA_BUDGET_TOTAL_MB_DEFAULT / CCTV_MEDIA_BUDGET_UNIT_MB_DEFAULT.max(1)).max(1);

        for (plane, max_concurrency, units) in [
            (
                "still",
                CCTV_STILL_MAX_GRABS_DEFAULT,
                CCTV_MEDIA_UNITS_STILL,
            ),
            ("live", CCTV_LIVE_MAX_STREAMS_DEFAULT, CCTV_MEDIA_UNITS_LIVE),
            ("clip", CCTV_CLIP_MAX_EXPORTS_DEFAULT, CCTV_MEDIA_UNITS_CLIP),
            (
                "download",
                CCTV_DOWNLOAD_MAX_EXPORTS_DEFAULT,
                CCTV_MEDIA_UNITS_DOWNLOAD,
            ),
        ] {
            let requested = max_concurrency * units as usize;
            assert!(
                requested <= total_units,
                "{plane} plane asks for {requested} units at full concurrency \
                 ({max_concurrency} x {units}) but the shared budget holds only \
                 {total_units}; raise CCTV_MEDIA_BUDGET_TOTAL_MB_DEFAULT or lower \
                 this plane's ceiling"
            );
        }
    }
}
