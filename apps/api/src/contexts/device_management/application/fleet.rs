//! Fleet (multi-target) user-management writes over the single-device service.
//!
//! [`FleetWriteService`] wraps a [`DeviceManagementService`] and fans one
//! user-management request (preview / create / delete) out across many
//! connection targets, running them with **bounded concurrency** and collecting
//! each target's result **independently** — one target failing (unreachable,
//! ineligible) never aborts the run, so partial success is the normal outcome.
//!
//! # Auth-agnostic by design
//!
//! This layer knows nothing about RBAC. Per-target permission
//! ([`FleetTargetOutcome::NotPermitted`]) is decided by the REST layer, which
//! holds the auth context: it filters the target list before calling here, and
//! marks the excluded targets `NotPermitted` in the response it builds. The
//! service simply operates on the target list it is handed.
//!
//! # Serialization boundary
//!
//! The preview-side types ([`FleetPreview`], [`FleetPreviewOutcome`]) are
//! `serde`-serializable because [`WritePreview`] is. The write-side types
//! ([`FleetWriteResult`], [`FleetTarget`], [`FleetTargetOutcome`]) are **not**:
//! [`FleetTargetOutcome::Done`] embeds a
//! [`BulkOutcome`], whose per-item failures carry a
//! [`crate::contexts::device_management::domain::DriverError`] — neither is
//! `serde`. The REST layer maps `BulkOutcome` to its own DTO exactly as the
//! single-device write endpoint already does.

use futures_util::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::contexts::device_management::application::device_service::{
    DeviceManagementService, DeviceServiceError,
};
use crate::contexts::device_management::application::write::{WriteOp, WritePreview};
use crate::contexts::device_management::domain::{BulkOutcome, DriverError, NewUser};

/// Maximum number of targets contacted concurrently by one fleet operation.
///
/// A fixed cap bounds the load a single fleet request places on the proxy
/// fabric and the devices behind it, independent of how many targets the caller
/// passed.
const FLEET_CONCURRENCY: usize = 8;

/// What happened to one target in a fleet **write** (create / delete).
///
/// Not `serde` — see the module-level serialization boundary note; the REST
/// layer maps [`BulkOutcome`] to a DTO.
#[derive(Debug)]
pub enum FleetTargetOutcome {
    /// The write reached the device and ran; the [`BulkOutcome`] preserves
    /// per-user partial success.
    Done(BulkOutcome),
    /// The target could not be acted on for a structural reason (it did not
    /// resolve, or its family has no user-management plane) — the device was
    /// never mutated.
    Ineligible {
        /// A short, language-neutral explanation.
        reason: String,
    },
    /// The caller lacks permission for this target. **Never produced by the
    /// service** (it is auth-agnostic); the REST layer sets this for targets it
    /// filtered out of the permitted set.
    NotPermitted,
    /// The target resolved and is eligible, but the device could not be driven
    /// to completion (transport failure or a device-reported error).
    Unreachable {
        /// A short, language-neutral explanation.
        reason: String,
    },
}

/// One target paired with its fleet-write outcome.
#[derive(Debug)]
pub struct FleetTarget {
    /// The connection target this outcome is for.
    pub target_id: Uuid,
    /// What happened to it.
    pub outcome: FleetTargetOutcome,
}

/// The result of a fleet write across every requested target.
#[derive(Debug)]
pub struct FleetWriteResult {
    /// One entry per requested target, in request order.
    pub per_target: Vec<FleetTarget>,
}

/// What a fleet **preview** determined for one target: either a dry-run verdict
/// or a reason the target is ineligible.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FleetPreviewOutcome {
    /// The target is eligible; this is its per-name dry-run verdict.
    Preview(WritePreview),
    /// The target has no user-management plane, or it did not resolve.
    Ineligible {
        /// A short, language-neutral explanation.
        reason: String,
    },
}

/// The result of a fleet preview across every requested target.
///
/// A preview is read-only and never mutates a device.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetPreview {
    /// One `(target, verdict)` pair per requested target, in request order.
    pub per_target: Vec<(Uuid, FleetPreviewOutcome)>,
}

/// Fans user-management operations out across many targets on top of the
/// single-device [`DeviceManagementService`].
#[derive(Clone)]
pub struct FleetWriteService {
    inner: DeviceManagementService,
}

impl FleetWriteService {
    /// Wrap a single-device service to gain fleet operations.
    #[must_use]
    pub fn new(inner: DeviceManagementService) -> Self {
        Self { inner }
    }

    /// Dry-run one user-management op across every target, read-only.
    ///
    /// Each target runs the single-device
    /// [`DeviceManagementService::preview_users`] (never mutating). A target
    /// that does not resolve, whose family lacks a user-management plane, or
    /// whose request is structurally invalid is partitioned into
    /// [`FleetPreviewOutcome::Ineligible`] with a reason; all others carry a
    /// [`WritePreview`]. Targets run with bounded concurrency and results are
    /// returned in request order.
    pub async fn fleet_preview(&self, target_ids: &[Uuid], op: WriteOp) -> FleetPreview {
        // `buffer_unordered` (not a spawn + Semaphore) so each target future can
        // borrow `&self` and `op` directly — no `'static` bound, no per-target
        // Arc cloning. It caps in-flight futures at `FLEET_CONCURRENCY`.
        let per_target = collect_ordered(target_ids.iter().copied().enumerate().map(
            |(idx, target_id)| {
                let op = op.clone();
                async move {
                    let outcome = match self.inner.preview_users(target_id, op).await {
                        Ok(preview) => FleetPreviewOutcome::Preview(preview),
                        Err(error) => FleetPreviewOutcome::Ineligible {
                            reason: error.to_string(),
                        },
                    };
                    (idx, (target_id, outcome))
                }
            },
        ))
        .await;
        FleetPreview { per_target }
    }

    /// Provision the same users across every target.
    ///
    /// The route layer authorizes each target (`Create × cctv`) BEFORE it is
    /// handed to this method. Each target runs the single-device
    /// [`DeviceManagementService::create_users`] with bounded concurrency; a
    /// per-target failure is isolated into that target's
    /// [`FleetTargetOutcome`] and never aborts the others.
    pub async fn fleet_create_users(
        &self,
        target_ids: &[Uuid],
        users: &[NewUser],
    ) -> FleetWriteResult {
        let per_target = collect_ordered(target_ids.iter().copied().enumerate().map(
            |(idx, target_id)| async move {
                let outcome = classify_write(self.inner.create_users(target_id, users).await);
                (idx, FleetTarget { target_id, outcome })
            },
        ))
        .await;
        FleetWriteResult { per_target }
    }

    /// Remove the same users (by login name) across every target.
    ///
    /// Same authorization boundary, concurrency, and per-target isolation as
    /// [`FleetWriteService::fleet_create_users`] (route-layer `Delete × cctv`).
    pub async fn fleet_delete_users(
        &self,
        target_ids: &[Uuid],
        usernames: &[String],
    ) -> FleetWriteResult {
        let per_target = collect_ordered(target_ids.iter().copied().enumerate().map(
            |(idx, target_id)| async move {
                let outcome = classify_write(self.inner.delete_users(target_id, usernames).await);
                (idx, FleetTarget { target_id, outcome })
            },
        ))
        .await;
        FleetWriteResult { per_target }
    }
}

/// Map one target's single-device write result into a [`FleetTargetOutcome`].
///
/// The contract, exhaustive over [`DeviceServiceError`]:
/// - `Ok` → [`FleetTargetOutcome::Done`].
/// - `Resolve` (target misconfigured / does not resolve) → `Ineligible`.
/// - `Driver(Unsupported)` (family has no user plane) → `Ineligible`.
/// - `InvalidRequest` (structurally invalid) → `Ineligible`.
/// - any other `Driver(..)` (auth, clock skew, transport, protocol, device) →
///   `Unreachable`.
fn classify_write(result: Result<BulkOutcome, DeviceServiceError>) -> FleetTargetOutcome {
    match result {
        Ok(outcome) => FleetTargetOutcome::Done(outcome),
        Err(DeviceServiceError::Driver(DriverError::Unsupported)) => {
            FleetTargetOutcome::Ineligible {
                reason: DriverError::Unsupported.to_string(),
            }
        }
        Err(DeviceServiceError::Resolve(error)) => FleetTargetOutcome::Ineligible {
            reason: error.to_string(),
        },
        Err(DeviceServiceError::InvalidRequest(reason)) => {
            FleetTargetOutcome::Ineligible { reason }
        }
        Err(DeviceServiceError::Driver(error)) => FleetTargetOutcome::Unreachable {
            reason: error.to_string(),
        },
        // Fleet writes never start live streams; keep the match exhaustive so a
        // new service error is a compile-time decision, not a silent bucket.
        Err(error @ DeviceServiceError::LiveBusy) => FleetTargetOutcome::Unreachable {
            reason: error.to_string(),
        },
    }
}

/// Run per-target futures with bounded concurrency, returning their outputs in
/// request order.
///
/// Each future yields `(index, value)`; the index restores request order after
/// `buffer_unordered` (which yields in completion order). Concurrency is capped
/// at [`FLEET_CONCURRENCY`], and every result is collected — none is dropped.
async fn collect_ordered<T, Fut, I>(futures: I) -> Vec<T>
where
    I: IntoIterator<Item = Fut>,
    Fut: Future<Output = (usize, T)>,
{
    let mut indexed: Vec<(usize, T)> = stream::iter(futures)
        .buffer_unordered(FLEET_CONCURRENCY)
        .collect()
        .await;
    indexed.sort_by_key(|(idx, _)| *idx);
    indexed.into_iter().map(|(_, value)| value).collect()
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use async_trait::async_trait;
    use serde_json::{Value, json};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;
    use crate::contexts::device_management::application::registry::DeviceDriverRegistry;
    use crate::contexts::device_management::application::resolver::{
        DeviceResolveError, DeviceResolver, ResolvedDevice,
    };
    use crate::contexts::device_management::application::write::PreviewAction;
    use crate::contexts::device_management::domain::{DeviceFamily, Secret};
    use crate::contexts::device_management::infrastructure::transport::{
        DeviceEndpoint, DeviceStream, DeviceTransport,
    };
    use crate::contexts::device_management::protocol::dvrip as codec;

    /// How a fake resolver should map one target id.
    #[derive(Clone)]
    enum TargetPlan {
        /// Resolve to a device of `family` reachable at `host` (port 34567).
        Device { family: DeviceFamily, host: String },
        /// Fail to resolve (a misconfigured target).
        Unresolvable,
    }

    /// Resolver driven by a per-target plan map. Counts resolves and tracks peak
    /// in-flight concurrency, and can delay each resolve to force overlap so a
    /// test can observe the concurrency bound.
    struct FanoutResolver {
        plans: HashMap<Uuid, TargetPlan>,
        resolves: Arc<AtomicUsize>,
        in_flight: Arc<AtomicUsize>,
        peak: Arc<AtomicUsize>,
        delay: Duration,
    }

    impl FanoutResolver {
        fn new(plans: HashMap<Uuid, TargetPlan>) -> Self {
            Self {
                plans,
                resolves: Arc::new(AtomicUsize::new(0)),
                in_flight: Arc::new(AtomicUsize::new(0)),
                peak: Arc::new(AtomicUsize::new(0)),
                delay: Duration::ZERO,
            }
        }

        fn with_delay(mut self, delay: Duration) -> Self {
            self.delay = delay;
            self
        }

        fn peak_in_flight(&self) -> usize {
            self.peak.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl DeviceResolver for FanoutResolver {
        async fn resolve(&self, target_id: Uuid) -> Result<ResolvedDevice, DeviceResolveError> {
            self.resolves.fetch_add(1, Ordering::SeqCst);
            let current = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(current, Ordering::SeqCst);
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            match self.plans.get(&target_id) {
                Some(TargetPlan::Device { family, host }) => Ok(ResolvedDevice {
                    family: *family,
                    endpoint: DeviceEndpoint::new(host.clone(), 34567),
                    username: "admin".to_owned(),
                    password: Some(Secret::new("pw")),
                    become_password: None,
                }),
                Some(TargetPlan::Unresolvable) | None => {
                    Err(DeviceResolveError::NoAddress(target_id))
                }
            }
        }
    }

    /// Byte transport that, per host, either serves a fixed DVRIP response script
    /// on a fresh in-memory stream or fails to connect. Each connection gets its
    /// own script copy, so concurrent targets are isolated. Counts connects.
    struct ScriptedTransport {
        scripts: HashMap<String, Vec<Value>>,
        unreachable: HashSet<String>,
        connects: Arc<AtomicUsize>,
    }

    impl ScriptedTransport {
        fn new(scripts: HashMap<String, Vec<Value>>, unreachable: HashSet<String>) -> Self {
            Self {
                scripts,
                unreachable,
                connects: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    #[async_trait]
    impl DeviceTransport for ScriptedTransport {
        async fn connect(
            &self,
            endpoint: &DeviceEndpoint,
        ) -> Result<Box<dyn DeviceStream>, DriverError> {
            self.connects.fetch_add(1, Ordering::SeqCst);
            if self.unreachable.contains(&endpoint.host) {
                return Err(DriverError::Transport("simulated unreachable".to_owned()));
            }
            let responses = self
                .scripts
                .get(&endpoint.host)
                .cloned()
                .unwrap_or_default();
            let (client, mut server) = tokio::io::duplex(64 * 1024);
            tokio::spawn(async move {
                for response in responses {
                    let mut header = [0u8; codec::HEADER_LEN];
                    if server.read_exact(&mut header).await.is_err() {
                        return;
                    }
                    let declared =
                        u32::from_le_bytes([header[16], header[17], header[18], header[19]])
                            as usize;
                    let mut body = vec![0u8; declared];
                    if server.read_exact(&mut body).await.is_err() {
                        return;
                    }
                    let json = serde_json::to_string(&response).expect("serialize");
                    if server
                        .write_all(&codec::encode(0, 0, 0, &json))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
            });
            Ok(Box::new(client))
        }
    }

    /// A minimal well-formed provisioning request.
    fn new_user(name: &str, password: &str) -> NewUser {
        NewUser {
            name: name.to_owned(),
            password: Secret::new(password),
            group: None,
            authorities: Vec::new(),
        }
    }

    /// The DVRIP login reply every scripted session begins with.
    fn login_reply() -> Value {
        json!({"Ret": 100, "SessionID": "0x00000005"})
    }

    /// Build a fleet service over a caller-supplied resolver and transport.
    fn fleet_service(
        resolver: Arc<FanoutResolver>,
        transport: Arc<ScriptedTransport>,
    ) -> FleetWriteService {
        let registry = DeviceDriverRegistry::new(transport);
        let inner = DeviceManagementService::new(resolver, registry);
        FleetWriteService::new(inner)
    }

    /// Look up a preview entry by target id.
    fn preview_for(preview: &FleetPreview, target_id: Uuid) -> &FleetPreviewOutcome {
        preview
            .per_target
            .iter()
            .find(|(id, _)| *id == target_id)
            .map(|(_, outcome)| outcome)
            .expect("preview entry present")
    }

    /// Look up a write outcome by target id.
    fn outcome_for(result: &FleetWriteResult, target_id: Uuid) -> &FleetTargetOutcome {
        result
            .per_target
            .iter()
            .find(|entry| entry.target_id == target_id)
            .map(|entry| &entry.outcome)
            .expect("write entry present")
    }

    // fleet_preview partitions eligible vs ineligible: a DVRIP target is
    // eligible (carries a WritePreview), an ONVIF target lacks the user plane
    // and a bad target does not resolve — both are Ineligible with a reason.
    #[tokio::test]
    async fn fleet_preview_partitions_eligible_and_ineligible() {
        let dvrip = Uuid::from_u128(1);
        let onvif = Uuid::from_u128(2);
        let unresolved = Uuid::from_u128(3);

        let mut plans = HashMap::new();
        plans.insert(
            dvrip,
            TargetPlan::Device {
                family: DeviceFamily::DvripSofia,
                host: "dvrip.device".to_owned(),
            },
        );
        plans.insert(
            onvif,
            TargetPlan::Device {
                family: DeviceFamily::Onvif,
                host: "onvif.device".to_owned(),
            },
        );
        plans.insert(unresolved, TargetPlan::Unresolvable);

        // The DVRIP preview reads the current user list: login + Users query.
        let mut scripts = HashMap::new();
        scripts.insert(
            "dvrip.device".to_owned(),
            vec![
                login_reply(),
                json!({"Ret": 100, "SessionID": "0x5", "Users": []}),
            ],
        );
        let transport = Arc::new(ScriptedTransport::new(scripts, HashSet::new()));
        let resolver = Arc::new(FanoutResolver::new(plans));
        let service = fleet_service(Arc::clone(&resolver), Arc::clone(&transport));

        let op = WriteOp::CreateUsers(vec![new_user("fresh", "pw")]);
        let preview = service.fleet_preview(&[dvrip, onvif, unresolved], op).await;

        // Request order is preserved.
        assert_eq!(
            preview
                .per_target
                .iter()
                .map(|(id, _)| *id)
                .collect::<Vec<_>>(),
            [dvrip, onvif, unresolved],
        );

        // Eligible: a WritePreview classifying "fresh" as Create.
        match preview_for(&preview, dvrip) {
            FleetPreviewOutcome::Preview(write_preview) => {
                let item = write_preview
                    .per_item
                    .iter()
                    .find(|item| item.name == "fresh")
                    .expect("item for fresh");
                assert_eq!(item.action, PreviewAction::Create);
            }
            FleetPreviewOutcome::Ineligible { reason } => {
                panic!("DVRIP target should be eligible, got: {reason}")
            }
        }

        // Ineligible: ONVIF has no user plane; the bad target does not resolve.
        match preview_for(&preview, onvif) {
            FleetPreviewOutcome::Ineligible { reason } => {
                assert!(reason.contains("not supported"), "reason: {reason}");
            }
            FleetPreviewOutcome::Preview(_) => panic!("ONVIF has no user plane"),
        }
        match preview_for(&preview, unresolved) {
            FleetPreviewOutcome::Ineligible { reason } => {
                assert!(reason.contains("no management address"), "reason: {reason}");
            }
            FleetPreviewOutcome::Preview(_) => panic!("unresolved target is ineligible"),
        }
    }

    // fleet_create_users reaches multiple targets and isolates one target's
    // failure: three succeed (Done) while one unreachable target fails — the
    // failure does not abort the run.
    #[tokio::test]
    async fn fleet_create_isolates_one_failure_from_the_rest() {
        let ok1 = Uuid::from_u128(11);
        let ok2 = Uuid::from_u128(12);
        let ok3 = Uuid::from_u128(13);
        let bad = Uuid::from_u128(14);

        let mut plans = HashMap::new();
        for id in [ok1, ok2, ok3] {
            plans.insert(
                id,
                TargetPlan::Device {
                    family: DeviceFamily::DvripSofia,
                    host: "ok.device".to_owned(),
                },
            );
        }
        plans.insert(
            bad,
            TargetPlan::Device {
                family: DeviceFamily::DvripSofia,
                host: "bad.device".to_owned(),
            },
        );

        // One create of one user per session: login + AddUser ack.
        let mut scripts = HashMap::new();
        scripts.insert(
            "ok.device".to_owned(),
            vec![login_reply(), json!({"Ret": 100, "SessionID": "0x5"})],
        );
        let mut unreachable = HashSet::new();
        unreachable.insert("bad.device".to_owned());

        let transport = Arc::new(ScriptedTransport::new(scripts, unreachable));
        let resolver = Arc::new(FanoutResolver::new(plans));
        let service = fleet_service(Arc::clone(&resolver), Arc::clone(&transport));

        let users = vec![new_user("guard1", "s3cr3t")];
        let result = service
            .fleet_create_users(&[ok1, ok2, ok3, bad], &users)
            .await;

        assert_eq!(result.per_target.len(), 4);

        for id in [ok1, ok2, ok3] {
            match outcome_for(&result, id) {
                FleetTargetOutcome::Done(outcome) => {
                    assert_eq!(outcome.succeeded, ["guard1"]);
                    assert!(outcome.failed.is_empty());
                }
                other => panic!("target {id} should be Done, got: {other:?}"),
            }
        }
        match outcome_for(&result, bad) {
            FleetTargetOutcome::Unreachable { reason } => {
                assert!(reason.contains("simulated unreachable"), "reason: {reason}");
            }
            other => panic!("bad target should be Unreachable, got: {other:?}"),
        }
    }

    // Bounded concurrency: many targets all complete (nothing dropped) and the
    // number contacted at once never exceeds the cap while still overlapping.
    #[tokio::test]
    async fn fleet_create_bounds_concurrency_without_dropping_results() {
        const TARGETS: usize = 20;

        let ids: Vec<Uuid> = (0..TARGETS)
            .map(|n| Uuid::from_u128(100 + n as u128))
            .collect();
        let mut plans = HashMap::new();
        for id in &ids {
            plans.insert(
                *id,
                TargetPlan::Device {
                    family: DeviceFamily::DvripSofia,
                    host: "ok.device".to_owned(),
                },
            );
        }
        let mut scripts = HashMap::new();
        scripts.insert(
            "ok.device".to_owned(),
            vec![login_reply(), json!({"Ret": 100, "SessionID": "0x5"})],
        );

        let transport = Arc::new(ScriptedTransport::new(scripts, HashSet::new()));
        // A small resolve delay forces targets to overlap so peak concurrency is
        // observable.
        let resolver = Arc::new(FanoutResolver::new(plans).with_delay(Duration::from_millis(10)));
        let service = fleet_service(Arc::clone(&resolver), Arc::clone(&transport));

        let users = vec![new_user("guard1", "s3cr3t")];
        let result = service.fleet_create_users(&ids, &users).await;

        // Nothing dropped: every target has a Done outcome.
        assert_eq!(result.per_target.len(), TARGETS);
        assert!(
            result
                .per_target
                .iter()
                .all(|entry| matches!(entry.outcome, FleetTargetOutcome::Done(_))),
            "every target should complete",
        );

        let peak = resolver.peak_in_flight();
        assert!(
            peak <= FLEET_CONCURRENCY,
            "peak concurrency {peak} exceeded the cap {FLEET_CONCURRENCY}",
        );
        assert!(peak >= 2, "expected real overlap, peak was {peak}");
    }
}
