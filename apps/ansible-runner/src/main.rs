//! Trusted supervisor. Ansible and Git execute only through disposable OCI jobs.
mod acquisition;
mod console;
mod kubernetes;
mod launch;
mod native;
mod notification;
mod route;
mod sandbox;
mod store;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, bail};
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use piping_contracts::playbook_runner::{
    AcquireFailure, AcquireFailureCode, AcquireProjectRequest, AcquireProjectResponse, ConsoleAck,
    RUNNER_CONTRACT_VERSION, RunnerConsoleBatch, RunnerDispatchReceipt, RunnerDispatchStatus,
    RunnerGroupCompletionBatch, RunnerGroupCompletionItem, RunnerGroupEventBatch, RunnerJobRequest,
    RunnerJobState, RunnerRunStatus, SupervisorCallbacks,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::sync::{Mutex, Semaphore, watch};
use tokio::time::{Instant, interval};
use tracing::{error, warn};
use uuid::Uuid;

use console::{Collector, Stream};
use sandbox::{FrameKind, Sandbox, SandboxConfig, SandboxHandle, SandboxLimits, SandboxState};
use store::SupervisorStore;

const OUTPUT_LIMIT: usize = 16 * 1024 * 1024;
const CHUNK_LIMIT: usize = 8192;

struct AppState {
    secret: String,
    store: SupervisorStore,
    sandbox: Sandbox,
    client: reqwest::Client,
    jobs: Mutex<HashMap<Uuid, Arc<JobControl>>>,
    acquisitions: Mutex<HashMap<Uuid, AcquisitionRecord>>,
    slots: Arc<Semaphore>,
    shutdown: watch::Sender<bool>,
}

struct JobControl {
    data: Mutex<PersistedDispatch>,
    cancel: watch::Sender<bool>,
}

#[derive(Clone, Serialize, Deserialize)]
struct PersistedDispatch {
    receipt: RunnerDispatchReceipt,
    fingerprint: String,
    status: RunnerJobState,
    rc: Option<i32>,
    cancel_requested: bool,
    sandbox: Option<SandboxHandle>,
    callbacks: SupervisorCallbacks,
    callback_base_url: String,
    lease_owner: String,
    targets: BTreeMap<String, Uuid>,
    next_sequence: u64,
    delivered_sequence: u64,
    completion: Option<RunnerGroupCompletionBatch>,
    completion_delivered: bool,
    #[serde(default)]
    notification: Option<RunnerGroupEventBatch>,
    #[serde(default)]
    notification_delivered: bool,
    #[serde(default)]
    notification_dropped: bool,
    terminal_at: Option<u64>,
}

#[derive(Clone, Serialize, Deserialize)]
struct AcquisitionRecord {
    fingerprint: String,
    response_available: bool,
    interrupted: bool,
    created_at: u64,
}

struct ApiError(StatusCode, &'static str);

#[derive(Debug, thiserror::Error)]
#[error("Git acquisition failed")]
struct AcquisitionFailure(AcquireFailureCode);

enum AcquireApiError {
    Api(ApiError),
    Acquisition(AcquireFailureCode),
}
impl From<ApiError> for AcquireApiError {
    fn from(error: ApiError) -> Self {
        Self::Api(error)
    }
}
impl IntoResponse for AcquireApiError {
    fn into_response(self) -> Response {
        match self {
            Self::Api(error) => error.into_response(),
            Self::Acquisition(code) => {
                (StatusCode::BAD_GATEWAY, Json(AcquireFailure { code })).into_response()
            }
        }
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"detail":self.1}))).into_response()
    }
}
fn internal(_: impl std::fmt::Display) -> ApiError {
    ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "runner operation could not be completed",
    )
}
fn auth(headers: &HeaderMap, secret: &str) -> Result<(), ApiError> {
    let expected = format!("Bearer {secret}");
    let provided = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if !bool::from(provided.as_bytes().ct_eq(expected.as_bytes())) {
        return Err(ApiError(
            StatusCode::UNAUTHORIZED,
            "missing or invalid runner authorization",
        ));
    }
    Ok(())
}
fn terminal(state: RunnerJobState) -> bool {
    matches!(
        state,
        RunnerJobState::Succeeded
            | RunnerJobState::Failed
            | RunnerJobState::Canceled
            | RunnerJobState::TimedOut
    )
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn limits(sandbox: &Sandbox, seconds: u32) -> SandboxLimits {
    SandboxLimits {
        memory_bytes: 1024 * 1024 * 1024,
        pids: sandbox.process_limit(),
        cpus_millis: 1000,
        timeout: Duration::from_secs(u64::from(seconds.clamp(1, 86400))),
        output_bytes: OUTPUT_LIMIT,
    }
}
fn fingerprint(value: &impl Serialize) -> anyhow::Result<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}
fn endpoint(base: &str, group: Uuid, suffix: &str) -> anyhow::Result<reqwest::Url> {
    let mut url = reqwest::Url::parse(base)?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        bail!("invalid callback origin");
    }
    url.set_path(&format!("/api/v1/playbooks/run-groups/{group}/{suffix}"));
    Ok(url)
}

/// Sandbox writeback tokens are delivered into the job, so the API origin must
/// be TLS-protected. Certificates are always verified against the system trust
/// store; there is deliberately no "ignore certificate" path.
const SANDBOX_TOKEN_API_NOT_HTTPS: &str = "sandbox token delivery requires an https API base URL";
/// An unreachable API would start a job that can never write its result back.
const SANDBOX_TOKEN_API_UNREACHABLE: &str =
    "callback API is unreachable over TLS; refusing a job that cannot receive sandbox tokens";

#[derive(Debug, PartialEq, Eq)]
enum SandboxTokenApiError {
    NotHttps,
    Unreachable,
}

impl SandboxTokenApiError {
    fn api_error(self) -> ApiError {
        match self {
            Self::NotHttps => ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                SANDBOX_TOKEN_API_NOT_HTTPS,
            ),
            Self::Unreachable => ApiError(
                StatusCode::SERVICE_UNAVAILABLE,
                SANDBOX_TOKEN_API_UNREACHABLE,
            ),
        }
    }
}

fn sandbox_token_api_url(base: &str, group: Uuid) -> Result<reqwest::Url, SandboxTokenApiError> {
    let parsed = reqwest::Url::parse(base).map_err(|_| SandboxTokenApiError::NotHttps)?;
    if parsed.scheme() != "https" {
        return Err(SandboxTokenApiError::NotHttps);
    }
    endpoint(base, group, "events/batch").map_err(|_| SandboxTokenApiError::NotHttps)
}

/// Bounded reachability probe: any HTTP answer proves the TLS origin is up,
/// while a transport error or the short timeout refuses the dispatch.
async fn ensure_sandbox_token_api(
    client: &reqwest::Client,
    base: &str,
    group: Uuid,
) -> Result<(), SandboxTokenApiError> {
    let url = sandbox_token_api_url(base, group)?;
    client
        .head(url)
        .timeout(Duration::from_secs(3))
        .send()
        .await
        .map_err(|_| SandboxTokenApiError::Unreachable)?;
    Ok(())
}
async fn save(state: &AppState, data: &PersistedDispatch) -> anyhow::Result<()> {
    let store = state.store.clone();
    let data = data.clone();
    tokio::task::spawn_blocking(move || store.save_dispatch(data.receipt.dispatch_id, &data))
        .await??;
    Ok(())
}
async fn save_acquisition(
    state: &AppState,
    id: Uuid,
    record: &AcquisitionRecord,
) -> anyhow::Result<()> {
    let store = state.store.clone();
    let record = record.clone();
    tokio::task::spawn_blocking(move || store.save_acquisition(id, &record)).await??;
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if std::env::args().any(|arg| arg == "--health-check") {
        let address =
            std::env::var("PIPING_ANSIBLE_RUNNER__BIND").unwrap_or_else(|_| "0.0.0.0:8080".into());
        reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()?
            .get(format!(
                "http://{}/health",
                address.replace("0.0.0.0", "127.0.0.1")
            ))
            .bearer_auth(
                std::env::var("PIPING_ANSIBLE_RUNNER__SHARED_SECRET")
                    .context("runner shared secret is required")?,
            )
            .send()
            .await?
            .error_for_status()?;
        return Ok(());
    }
    tracing_subscriber::fmt()
        .with_env_filter("piping_ansible_runner=info")
        .init();
    let secret = std::env::var("PIPING_ANSIBLE_RUNNER__SHARED_SECRET")
        .context("runner shared secret is required")?;
    if secret.is_empty() {
        bail!("runner shared secret is required");
    }
    let root = PathBuf::from(
        std::env::var("PIPING_ANSIBLE_RUNNER__DATA_DIR")
            .unwrap_or_else(|_| "/var/lib/piping-runner".into()),
    );
    for name in [
        "PIPING_SANDBOX_WORKSPACE_ROOT",
        "PIPING_SANDBOX_RUNTIME_DIR",
    ] {
        store::private_directory(&PathBuf::from(
            std::env::var_os(name).with_context(|| format!("{name} is required"))?,
        ))?;
    }
    let store = SupervisorStore::open(&root)?;
    let sandbox = Sandbox::new(SandboxConfig::from_env()?)?;
    if store.root().starts_with(sandbox.workspace_root())
        || sandbox.workspace_root().starts_with(store.root())
    {
        bail!("supervisor state and job workspaces must not contain each other");
    }
    // Fail closed on unavailable engines. Never replay an interrupted launch with
    // credentials that were deliberately not retained in the durable receipt.
    for handle in sandbox.reconcile().await? {
        sandbox.terminate(&handle).await?;
    }
    // Only UUID-named private children belong to this runtime. Reconciliation
    // above has confirmed that no container can still mutate them.
    for entry in std::fs::read_dir(sandbox.workspace_root())? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_str().context("invalid job workspace name")?;
        let id = name
            .strip_suffix("-execute")
            .or_else(|| name.strip_suffix("-acquire"))
            .context("unknown entry in job workspace root")?;
        Uuid::parse_str(id).context("invalid job workspace identity")?;
        cleanup_workspace(&sandbox, &entry.path()).await?;
    }
    let mut jobs = HashMap::new();
    for (id, mut data) in store.load_dispatches::<PersistedDispatch>()? {
        if id != data.receipt.dispatch_id {
            bail!("dispatch receipt identity mismatch");
        }
        data.next_sequence = store.console_count(id)?;
        if data.delivered_sequence > data.next_sequence {
            bail!("console acknowledgement exceeds persisted output");
        }
        if !terminal(data.status) {
            data.status = if data.cancel_requested {
                RunnerJobState::Canceled
            } else {
                RunnerJobState::Failed
            };
            data.sandbox = None;
            data.terminal_at = Some(now());
            data.completion = Some(completion(
                &data,
                &BTreeMap::new(),
                Some("supervisor restarted; sandbox stopped"),
            ));
            store.save_dispatch(id, &data)?;
        }
        let (cancel, _) = watch::channel(data.cancel_requested);
        jobs.insert(
            id,
            Arc::new(JobControl {
                data: Mutex::new(data),
                cancel,
            }),
        );
    }
    let mut acquisitions = HashMap::new();
    for (id, mut record) in store.load_acquisitions::<AcquisitionRecord>()? {
        if !record.response_available {
            record.interrupted = true;
            store.save_acquisition(id, &record)?;
        }
        acquisitions.insert(id, record);
    }
    let state = Arc::new(AppState {
        secret,
        store,
        sandbox,
        client: reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(15))
            .build()?,
        jobs: Mutex::new(jobs),
        acquisitions: Mutex::new(acquisitions),
        slots: Arc::new(Semaphore::new(4)),
        shutdown: watch::channel(false).0,
    });
    let callbacks = tokio::spawn(deliver_callbacks(Arc::clone(&state)));
    let app = Router::new()
        .route("/health", get(health))
        .route("/jobs", post(create_job))
        .route("/projects/acquire", post(acquire_project))
        .route("/dispatches/{id}", get(get_dispatch))
        .route("/dispatches/{id}/cancel", post(cancel_dispatch))
        .layer(DefaultBodyLimit::max(96 * 1024 * 1024))
        .with_state(Arc::clone(&state));
    let bind =
        std::env::var("PIPING_ANSIBLE_RUNNER__BIND").unwrap_or_else(|_| "0.0.0.0:8080".into());
    let listener = tokio::net::TcpListener::bind(bind).await?;
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let shutdown = state.shutdown.clone();
    let result = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            tokio::select! { _=tokio::signal::ctrl_c()=>{}, _=terminate.recv()=>{} }
            shutdown.send_replace(true);
        })
        .await;
    state.shutdown.send_replace(true);
    let _drained = tokio::time::timeout(
        Duration::from_secs(600),
        state.slots.clone().acquire_many_owned(4),
    )
    .await
    .context("supervisor shutdown did not drain; startup reconciliation required")??;
    for handle in state.sandbox.reconcile().await? {
        state.sandbox.terminate(&handle).await?;
    }
    callbacks.abort();
    result.context("serve runner HTTP")
}
async fn health(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    auth(&headers, &state.secret)?;
    Ok(Json(
        json!({"status":"ok","protocol_version":RUNNER_CONTRACT_VERSION,"resources":state.sandbox.resource_contract()}),
    ))
}

async fn create_job(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(job): Json<RunnerJobRequest>,
) -> Result<(StatusCode, Json<RunnerDispatchReceipt>), ApiError> {
    auth(&headers, &state.secret)?;
    // An HTTP disconnect must not cancel the acceptance transaction between
    // its durable write and its in-memory registration/worker start.
    tokio::spawn(create_job_owned(state, job))
        .await
        .map_err(internal)?
}

async fn create_job_owned(
    state: Arc<AppState>,
    job: RunnerJobRequest,
) -> Result<(StatusCode, Json<RunnerDispatchReceipt>), ApiError> {
    if job.protocol_version != RUNNER_CONTRACT_VERSION || job.dispatch_attempt < 1 {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid runner protocol or dispatch attempt",
        ));
    }
    endpoint(&job.callback_base_url, job.run_group_id, "events/batch")
        .map_err(|_| ApiError(StatusCode::UNPROCESSABLE_ENTITY, "invalid callback origin"))?;
    let job = Arc::new(job);
    let validated = Arc::clone(&job);
    let digest = tokio::task::spawn_blocking(move || {
        launch::validate(&validated)?;
        fingerprint(validated.as_ref())
    })
    .await
    .map_err(internal)?
    .map_err(|_| {
        ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid resolved launch or unsupported input; manual prompts require needs_input",
        )
    })?;
    let mut jobs = state.jobs.lock().await;
    if let Some(existing) = jobs.get(&job.dispatch_id) {
        let existing = existing.data.lock().await;
        if existing.fingerprint != digest {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "dispatch identity already binds different inputs",
            ));
        }
        return Ok((StatusCode::ACCEPTED, Json(existing.receipt.clone())));
    }
    for existing in jobs.values() {
        let existing = existing.data.lock().await;
        if existing.receipt.run_group_id == job.run_group_id && !terminal(existing.status) {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "run group has an active dispatch",
            ));
        }
    }
    let store = state.store.clone();
    let id = job.dispatch_id;
    if tokio::task::spawn_blocking(move || store.dispatch_exists(id))
        .await
        .map_err(internal)?
        .map_err(internal)?
    {
        return Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "durable acceptance requires startup reconciliation",
        ));
    }
    ensure_sandbox_token_api(&state.client, &job.callback_base_url, job.run_group_id)
        .await
        .map_err(SandboxTokenApiError::api_error)?;
    let permit = state.slots.clone().try_acquire_owned().map_err(|_| {
        ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "runner capacity is exhausted",
        )
    })?;
    let receipt = RunnerDispatchReceipt {
        dispatch_id: job.dispatch_id,
        run_group_id: job.run_group_id,
        dispatch_attempt: job.dispatch_attempt,
    };
    let data = PersistedDispatch {
        receipt: receipt.clone(),
        fingerprint: digest,
        status: RunnerJobState::Accepted,
        rc: None,
        cancel_requested: false,
        sandbox: None,
        callbacks: job.callbacks.clone(),
        callback_base_url: job.callback_base_url.clone(),
        lease_owner: job.lease_owner.clone(),
        targets: job
            .launch
            .targets
            .iter()
            .filter_map(|target| target.run_id.map(|id| (target.hostname.clone(), id)))
            .collect(),
        next_sequence: 0,
        delivered_sequence: 0,
        completion: None,
        completion_delivered: false,
        notification: None,
        notification_delivered: false,
        notification_dropped: false,
        terminal_at: None,
    };
    save(&state, &data).await.map_err(internal)?;
    let (cancel, _) = watch::channel(false);
    let control = Arc::new(JobControl {
        data: Mutex::new(data),
        cancel,
    });
    jobs.insert(job.dispatch_id, Arc::clone(&control));
    drop(jobs);
    tokio::spawn(async move {
        let _permit = permit;
        if execute_job(&state, job, &control).await.is_err() {
            error!(
                "runner execution stopped without a durable terminal receipt; reconciliation required"
            );
        }
    });
    Ok((StatusCode::ACCEPTED, Json(receipt)))
}
async fn get_control(state: &AppState, id: Uuid) -> Result<Arc<JobControl>, ApiError> {
    if let Some(control) = state.jobs.lock().await.get(&id).cloned() {
        return Ok(control);
    }
    let store = state.store.clone();
    if tokio::task::spawn_blocking(move || store.dispatch_exists(id))
        .await
        .map_err(internal)?
        .map_err(internal)?
    {
        return Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "durable acceptance requires startup reconciliation",
        ));
    }
    Err(ApiError(
        StatusCode::NOT_FOUND,
        "dispatch was never accepted",
    ))
}
async fn get_dispatch(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<RunnerDispatchStatus>, ApiError> {
    auth(&headers, &state.secret)?;
    let control = get_control(&state, id).await?;
    let data = control.data.lock().await;
    Ok(Json(RunnerDispatchStatus {
        receipt: data.receipt.clone(),
        status: data.status,
        rc: data.rc,
    }))
}
async fn cancel_dispatch(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<RunnerDispatchStatus>, ApiError> {
    auth(&headers, &state.secret)?;
    let control = get_control(&state, id).await?;
    let mut data = control.data.lock().await;
    if !terminal(data.status) {
        data.cancel_requested = true;
        data.status = RunnerJobState::Canceling;
        save(&state, &data).await.map_err(internal)?;
        control.cancel.send_replace(true);
    }
    Ok(Json(RunnerDispatchStatus {
        receipt: data.receipt.clone(),
        status: data.status,
        rc: data.rc,
    }))
}

async fn persist_chunks(
    state: &AppState,
    control: &JobControl,
    chunks: Vec<console::Chunk>,
) -> anyhow::Result<()> {
    for chunk in chunks {
        let mut data = control.data.lock().await;
        if chunk.sequence != data.next_sequence {
            bail!("console sequence mismatch");
        }
        let sequence = chunk.sequence;
        let body = RunnerConsoleBatch {
            dispatch_id: data.receipt.dispatch_id,
            dispatch_attempt: data.receipt.dispatch_attempt,
            chunks: vec![chunk],
        };
        let store = state.store.clone();
        let id = data.receipt.dispatch_id;
        tokio::task::spawn_blocking(move || store.save_console(id, sequence, &body)).await??;
        data.next_sequence += 1;
        save(state, &data).await?;
    }
    Ok(())
}

async fn execute_job(
    state: &AppState,
    job: Arc<RunnerJobRequest>,
    control: &JobControl,
) -> anyhow::Result<()> {
    let workspace = state
        .sandbox
        .workspace_root()
        .join(format!("{}-execute", job.dispatch_id));
    let mut handle = None;
    let mut notification_bytes = Vec::new();
    let mut notification_overflow = false;
    let mut collector = None;
    let mut console_eof = false;
    let mut shutdown = state.shutdown.subscribe();
    let job_limits = limits(&state.sandbox, job.launch.timeout_seconds);
    let mut known_secrets = Vec::new();
    let result=async {
        if *shutdown.borrow() {bail!("supervisor is stopping");}
        let prepare_job=Arc::clone(&job); let directory=workspace.clone();
        let prepared=tokio::task::spawn_blocking(move || {
            std::fs::create_dir(&directory)?; store::private_directory(&directory)?;
            launch::prepare(&prepare_job,&directory,job_limits)
        }).await??;
        known_secrets = prepared.known_secrets.clone();
        let mut cancellation=control.cancel.subscribe();
        if *shutdown.borrow() {bail!("supervisor is stopping");}
        if *cancellation.borrow() { return Ok((RunnerJobState::Canceled,None,Some("run canceled"))); }
        let created=state.sandbox.create(&prepared.spec).await?; handle=Some(created.clone());
        {let mut data=control.data.lock().await; data.sandbox=Some(created.clone()); save(state,&data).await?;}
        let mut stream=if state.sandbox.attach_before_start() {
            let stream=state.sandbox.stream(&created,OUTPUT_LIMIT).await?; state.sandbox.start(&created).await?; stream
        } else { state.sandbox.start(&created).await?; state.sandbox.stream(&created,OUTPUT_LIMIT).await? };
        {let mut data=control.data.lock().await; if !data.cancel_requested {data.status=RunnerJobState::Running;} save(state,&data).await?;}
        collector=Some(Collector::new(prepared.known_secrets,OUTPUT_LIMIT,CHUNK_LIMIT).map_err(|_|anyhow::anyhow!("invalid redaction inputs"))?);
        let collector=collector.as_mut().context("console was not initialized")?;
        let deadline=Instant::now()+prepared.spec.limits.timeout;
        let mut observe=interval(Duration::from_secs(1));
        let mut exited=None; let mut eof=false; let mut drain_deadline=None;
        let outcome=loop {
            if *cancellation.borrow() {break (RunnerJobState::Canceled,None,Some("run canceled"));}
            if *shutdown.borrow() {break (RunnerJobState::Failed,None,Some("supervisor stopped"));}
            if let Some(rc)=exited && eof {break (if rc==0 {RunnerJobState::Succeeded}else{RunnerJobState::Failed},Some(rc),if rc==78 {Some("needs_input: unattended execution cannot answer prompts")}else if rc!=0 {Some("native Ansible exited with failure")}else{None});}
            tokio::select! {
                _=cancellation.changed()=>{},
                _=shutdown.changed()=>{},
                _=tokio::time::sleep_until(deadline)=>break (RunnerJobState::TimedOut,None,Some("job timeout")),
                _=observe.tick()=>{
                    if let SandboxState::Exited(rc)=state.sandbox.observe(&created).await? { exited=Some(rc); drain_deadline.get_or_insert(Instant::now()+Duration::from_secs(5)); }
                    if drain_deadline.is_some_and(|until|Instant::now()>=until) && !eof {break (RunnerJobState::Failed,exited,Some("descendant kept output open after native exit"));}
                },
                frame=stream.next(),if !eof=>match frame {
                    Some(Ok(frame))=>match frame.kind {
                        FrameKind::Notification=>{
                            if notification_bytes.len().saturating_add(frame.bytes.len())<=notification::MAX_NOTIFICATION_BYTES {
                                notification_bytes.extend_from_slice(&frame.bytes);
                            } else {
                                notification_overflow=true;
                            }
                        },
                        FrameKind::Output(stream)=>{
                            let chunks=collector.push(stream,&frame.bytes).map_err(|_|anyhow::anyhow!("console output limit exceeded"))?;
                            persist_chunks(state,control,chunks).await?;
                        },
                    },
                    Some(Err(error))=>return Err(error.into()),
                    None=>{eof=true; console_eof=true; for stream in [Stream::Stdout,Stream::Stderr] {
                        let chunks=collector.eof(stream).map_err(|_|anyhow::anyhow!("console output limit exceeded"))?;
                        persist_chunks(state,control,chunks).await?;
                    }}
                }
            }
        };
        Ok::<_,anyhow::Error>(outcome)
    }.await;
    // No terminal success/cancel acknowledgement until the whole sandbox is gone.
    if let Some(handle) = handle {
        state.sandbox.terminate(&handle).await?;
    } else {
        state
            .sandbox
            .cleanup_dispatch(&job.dispatch_id.to_string(), "execution")
            .await?;
    }
    if !console_eof && let Some(collector) = collector.as_mut() {
        for stream in [Stream::Stdout, Stream::Stderr] {
            // A failed collector discards its pending bytes by contract.
            if let Ok(chunks) = collector.eof(stream) {
                persist_chunks(state, control, chunks).await?;
            }
        }
    }
    let (status, rc, message) = result.unwrap_or((
        RunnerJobState::Failed,
        None,
        Some("sandbox preparation, execution or output failed"),
    ));
    let (spool, unavailable) = if state.sandbox.reads_workspace_notification() {
        // Termination above is fail-closed (`?`), so the writable Docker spool
        // is read only after no job process can mutate it. Do not move this read
        // before terminate or tolerate a failed termination.
        match notification::read_spool(&workspace) {
            Ok(Some(bytes)) => (bytes, None),
            Ok(None) => (Vec::new(), Some("absent")),
            Err(error) => (Vec::new(), Some(error.reason())),
        }
    } else if notification_overflow {
        (Vec::new(), Some("oversized"))
    } else if notification_bytes.is_empty() {
        (Vec::new(), Some("absent"))
    } else {
        (notification_bytes, None)
    };
    let mut data = control.data.lock().await;
    data.status = if data.cancel_requested {
        RunnerJobState::Canceled
    } else {
        status
    };
    data.rc = rc;
    data.sandbox = None;
    data.terminal_at = Some(now());
    let parsed = notification::parse(&spool, &data.targets, &known_secrets);
    let projection = notification::project(&parsed, &data.targets, Uuid::now_v7(), unavailable);
    data.notification = projection.batch;
    data.completion = Some(completion(&data, &projection.host_verdicts, message));
    save(state, &data).await?;
    drop(data);
    cleanup_workspace(&state.sandbox, &workspace).await?;
    Ok(())
}

const NOTIFICATION_REFUSED_DIAGNOSTIC: &str =
    "structured notification was refused by the backend; required host/task events are missing";

/// A permanently refused event batch is an operator-visible data-loss fact.
/// It must not rewrite a host verdict or the group process outcome. The batch
/// completion only persists per-run `error_message` (the group row has no
/// message column), so the diagnostic is attached to every run item as well as
/// to the completion-level message.
fn annotate_refused_completion(
    mut completion: RunnerGroupCompletionBatch,
) -> RunnerGroupCompletionBatch {
    let diagnostic = NOTIFICATION_REFUSED_DIAGNOSTIC;
    let existing = completion.error_message.take().unwrap_or_default();
    completion.error_message = Some(merge_diagnostic(&existing, diagnostic));
    for run in &mut completion.runs {
        let existing = run.error_message.take().unwrap_or_default();
        run.error_message = Some(merge_diagnostic(&existing, diagnostic));
    }
    completion
}

fn merge_diagnostic(existing: &str, diagnostic: &str) -> String {
    // Mirrors the backend's PLAYBOOK_RUN_ERROR_MAX_CHARS; the backend rejects
    // an over-long message, so keep the diagnostic and trim the old prefix.
    const MAX_CHARS: usize = 8_000;
    let existing = existing.trim_end();
    if existing.is_empty() {
        diagnostic.to_string()
    } else if existing.chars().count() + 1 + diagnostic.chars().count() <= MAX_CHARS {
        format!("{existing} {diagnostic}")
    } else {
        let keep = MAX_CHARS
            .saturating_sub(diagnostic.chars().count())
            .saturating_sub(1);
        let prefix: String = existing.chars().take(keep).collect();
        format!("{} {diagnostic}", prefix.trim_end())
    }
}

fn completion(
    data: &PersistedDispatch,
    verdicts: &BTreeMap<String, bool>,
    message: Option<&str>,
) -> RunnerGroupCompletionBatch {
    let status = match data.status {
        RunnerJobState::Succeeded => RunnerRunStatus::Succeeded,
        RunnerJobState::Canceled => RunnerRunStatus::Canceled,
        RunnerJobState::TimedOut => RunnerRunStatus::TimedOut,
        _ => RunnerRunStatus::Failed,
    };
    let runs = data
        .targets
        .iter()
        .map(|(host, id)| {
            // A valid terminal host summary is never overwritten by the group
            // process verdict. Cancellation/deadline are lifecycle facts only
            // for targets that never produced one; otherwise the honest value
            // is Unreported, never a fabricated success or failure.
            let (target_status, error, exit_code) = match verdicts.get(host) {
                Some(true) => (RunnerRunStatus::Succeeded, None, data.rc),
                Some(false) => (
                    RunnerRunStatus::Failed,
                    Some("structured host result reports failure".into()),
                    data.rc,
                ),
                None => match status {
                    RunnerRunStatus::Canceled => {
                        (RunnerRunStatus::Canceled, message.map(str::to_owned), None)
                    }
                    RunnerRunStatus::TimedOut => {
                        (RunnerRunStatus::TimedOut, message.map(str::to_owned), None)
                    }
                    _ => (
                        RunnerRunStatus::Unreported,
                        Some("host outcome unreported by structured callback".into()),
                        None,
                    ),
                },
            };
            RunnerGroupCompletionItem {
                run_id: *id,
                status: target_status,
                exit_code,
                error_message: error,
            }
        })
        .collect();
    RunnerGroupCompletionBatch {
        dispatch_id: data.receipt.dispatch_id,
        dispatch_attempt: data.receipt.dispatch_attempt,
        console_next_sequence: data.next_sequence,
        lease_owner: data.lease_owner.clone(),
        group_status: status,
        exit_code: data.rc,
        error_message: message.map(str::to_owned),
        runs,
    }
}
async fn cleanup_workspace(sandbox: &Sandbox, path: &std::path::Path) -> anyhow::Result<()> {
    if path.parent() != Some(sandbox.workspace_root()) {
        bail!("invalid cleanup boundary");
    }
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        if let Ok(metadata) = std::fs::symlink_metadata(&path) {
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                bail!("invalid job workspace");
            }
            std::fs::remove_dir_all(path)?;
        }
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    Ok(())
}

async fn deliver_callbacks(state: Arc<AppState>) {
    let mut tick = interval(Duration::from_secs(1));
    loop {
        tick.tick().await;
        let jobs: Vec<_> = state.jobs.lock().await.values().cloned().collect();
        for control in jobs {
            if deliver_one(&state, &control).await.is_err() {
                warn!("callback remains in supervisor outbox");
            }
        }
    }
}

fn acknowledged_prefix(
    data: &PersistedDispatch,
    sequence: u64,
    ack: &ConsoleAck,
) -> anyhow::Result<u64> {
    if ack.dispatch_id != data.receipt.dispatch_id
        || ack.dispatch_attempt != data.receipt.dispatch_attempt
        || ack.next_sequence <= sequence
        || ack.next_sequence > data.next_sequence
    {
        bail!("invalid console acknowledgement");
    }
    Ok(ack.next_sequence)
}

async fn deliver_one(state: &AppState, control: &JobControl) -> anyhow::Result<()> {
    for _ in 0..32 {
        let data = control.data.lock().await.clone();
        if data.delivered_sequence == data.next_sequence {
            break;
        }
        let store = state.store.clone();
        let id = data.receipt.dispatch_id;
        let sequence = data.delivered_sequence;
        let batch: RunnerConsoleBatch =
            tokio::task::spawn_blocking(move || store.load_console(id, sequence)).await??;
        let ack: ConsoleAck = state
            .client
            .post(endpoint(
                &data.callback_base_url,
                data.receipt.run_group_id,
                "console/batch",
            )?)
            .bearer_auth(&data.callbacks.group_token)
            .json(&batch)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let next = acknowledged_prefix(&data, sequence, &ack)?;
        let mut current = control.data.lock().await;
        current.delivered_sequence = next;
        save(state, &current).await?;
    }
    let data = control.data.lock().await.clone();
    if data.completion_delivered || data.delivered_sequence != data.next_sequence {
        return Ok(());
    }
    if let Some(batch) = data
        .notification
        .as_ref()
        .filter(|_| !data.notification_delivered && !data.notification_dropped)
    {
        let response = state
            .client
            .post(endpoint(
                &data.callback_base_url,
                data.receipt.run_group_id,
                "events/batch",
            )?)
            .bearer_auth(&data.callbacks.group_token)
            .json(batch)
            .send()
            .await;
        match response {
            Ok(response) if response.status().is_success() => {
                let mut current = control.data.lock().await;
                current.notification_delivered = true;
                save(state, &current).await?;
            }
            Ok(response)
                if response.status() == StatusCode::TOO_MANY_REQUESTS
                    || response.status().is_server_error() =>
            {
                bail!("structured notification delivery is retryable");
            }
            Ok(response)
                if response.status() == StatusCode::UNAUTHORIZED
                    || response.status() == StatusCode::FORBIDDEN =>
            {
                // Authentication/identity refusals are never blessed by
                // completing the run without the required data. Keep the
                // outbox pending and visible; completion stays blocked.
                bail!("structured notification authentication or identity was refused");
            }
            Ok(_) => {
                // Retrying identical bytes cannot succeed. Drop the batch
                // explicitly, keep every observed host verdict and the group
                // process outcome, and attach the loss to the completion's
                // operator-facing error message.
                let mut current = control.data.lock().await;
                current.notification_dropped = true;
                if let Some(completion) = current.completion.take() {
                    current.completion = Some(annotate_refused_completion(completion));
                }
                save(state, &current).await?;
                warn!("structured notification was refused by the backend");
            }
            Err(_) => bail!("structured notification delivery failed"),
        }
    }
    let data = control.data.lock().await.clone();
    if data.notification.is_some() && !data.notification_delivered && !data.notification_dropped {
        // Console is complete but the structured batch is still pending; never
        // send completion (which revokes the token) ahead of it.
        return Ok(());
    }
    if let Some(body) = data.completion {
        let response = state
            .client
            .post(endpoint(
                &data.callback_base_url,
                data.receipt.run_group_id,
                "completion-batch",
            )?)
            .bearer_auth(&data.callbacks.group_token)
            .json(&body)
            .send()
            .await?;
        if response.status() == StatusCode::CONFLICT {
            // Completion requires the exact durable prefix. Replaying it is
            // idempotent; a lost backend prefix must not strand completion.
            let mut current = control.data.lock().await;
            current.delivered_sequence = 0;
            save(state, &current).await?;
        }
        response.error_for_status()?;
        let mut current = control.data.lock().await;
        current.completion_delivered = true;
        current.callbacks.group_token.clear();
        current.callbacks.run_tokens.clear();
        save(state, &current).await?;
    }
    Ok(())
}

async fn acquire_project(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<AcquireProjectRequest>,
) -> Result<Json<AcquireProjectResponse>, AcquireApiError> {
    auth(&headers, &state.secret)?;
    tokio::spawn(acquire_project_owned(state, request))
        .await
        .map_err(internal)?
}

async fn acquire_project_owned(
    state: Arc<AppState>,
    request: AcquireProjectRequest,
) -> Result<Json<AcquireProjectResponse>, AcquireApiError> {
    acquisition::validate(&request).map_err(|_| {
        ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid or unsupported acquisition inputs",
        )
    })?;
    let digest = fingerprint(&request).map_err(internal)?;
    let mut records = state.acquisitions.lock().await;
    if let Some(record) = records.get(&request.acquisition_id) {
        if record.fingerprint != digest {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "acquisition identity binds different inputs",
            )
            .into());
        }
        if !record.response_available {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "acquisition is active or interrupted; use its recorded result or a new attempt identity",
            ).into());
        }
        let store = state.store.clone();
        let id = request.acquisition_id;
        drop(records);
        return tokio::task::spawn_blocking(move || {
            store.load_acquisition_result::<AcquireProjectResponse>(id)
        })
        .await
        .map_err(internal)?
        .map(Json)
        .map_err(|error| internal(error).into());
    }
    let permit = state.slots.clone().try_acquire_owned().map_err(|_| {
        ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "runner capacity is exhausted",
        )
    })?;
    let record = AcquisitionRecord {
        fingerprint: digest,
        response_available: false,
        interrupted: false,
        created_at: now(),
    };
    let store = state.store.clone();
    let id = request.acquisition_id;
    if tokio::task::spawn_blocking(move || store.acquisition_exists(id))
        .await
        .map_err(internal)?
        .map_err(internal)?
    {
        return Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "durable acquisition requires startup reconciliation",
        )
        .into());
    }
    save_acquisition(&state, request.acquisition_id, &record)
        .await
        .map_err(internal)?;
    records.insert(request.acquisition_id, record);
    drop(records);
    let task = tokio::spawn(async move {
        let _permit = permit;
        let result = execute_acquisition(&state, &request).await;
        let mut records = state.acquisitions.lock().await;
        if let Some(record) = records.get_mut(&request.acquisition_id) {
            if let Ok(response) = &result {
                let store = state.store.clone();
                let response = response.clone();
                let id = request.acquisition_id;
                tokio::task::spawn_blocking(move || store.save_acquisition_result(id, &response))
                    .await??;
                record.response_available = true;
            }
            record.interrupted = result.is_err();
            save_acquisition(&state, request.acquisition_id, record).await?;
        }
        result
    });
    task.await.map_err(internal)?.map(Json).map_err(|error| {
        if let Some(failure) = error.downcast_ref::<AcquisitionFailure>() {
            AcquireApiError::Acquisition(failure.0)
        } else {
            AcquireApiError::Api(internal(error))
        }
    })
}
async fn execute_acquisition(
    state: &AppState,
    request: &AcquireProjectRequest,
) -> anyhow::Result<AcquireProjectResponse> {
    let workspace = state
        .sandbox
        .workspace_root()
        .join(format!("{}-acquire", request.acquisition_id));
    let prepare = request.clone();
    let directory = workspace.clone();
    let job_limits = limits(&state.sandbox, 600);
    let mut shutdown = state.shutdown.subscribe();
    if *shutdown.borrow() {
        bail!("supervisor is stopping");
    }
    let prepared = tokio::task::spawn_blocking(move || {
        acquisition::prepare_request(&prepare, &directory, job_limits)
    })
    .await?;
    let spec = match prepared {
        Ok(spec) => spec,
        Err(error) => {
            cleanup_workspace(&state.sandbox, &workspace).await?;
            return Err(error);
        }
    };
    let handle = match state.sandbox.create(&spec).await {
        Ok(handle) => handle,
        Err(error) => {
            state
                .sandbox
                .cleanup_dispatch(&request.acquisition_id.to_string(), "acquire")
                .await?;
            cleanup_workspace(&state.sandbox, &workspace).await?;
            return Err(error.into());
        }
    };
    let operation = async {
        let mut stream = if state.sandbox.attach_before_start() {
            let stream = state.sandbox.stream(&handle, OUTPUT_LIMIT).await?;
            state.sandbox.start(&handle).await?;
            stream
        } else {
            state.sandbox.start(&handle).await?;
            state.sandbox.stream(&handle, OUTPUT_LIMIT).await?
        };
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut total = 0usize;
        while let Some(frame) = stream.next().await {
            let frame = frame?;
            total = total
                .checked_add(frame.bytes.len())
                .context("acquisition output overflow")?;
            if total > OUTPUT_LIMIT {
                bail!("acquisition result exceeds output bound");
            }
            if let FrameKind::Output(Stream::Stdout) = frame.kind {
                stdout.extend(frame.bytes);
            } else if let FrameKind::Output(Stream::Stderr) = frame.kind {
                stderr.extend(frame.bytes);
            }
        }
        let mut observe = interval(Duration::from_millis(250));
        loop {
            match state.sandbox.observe(&handle).await? {
                SandboxState::Exited(0) => break,
                SandboxState::Exited(_) => {
                    return Err(AcquisitionFailure(acquisition::classify_failure(&stderr)).into());
                }
                _ => observe.tick().await,
            };
        }
        Ok::<_, anyhow::Error>(stdout)
    };
    let deadline = spec.limits.timeout;
    let stopping = *shutdown.borrow_and_update();
    let result = if stopping {
        Err(anyhow::anyhow!("supervisor stopped"))
    } else {
        tokio::select! {
            result=tokio::time::timeout(deadline,operation)=>result.context("acquisition timeout").and_then(|result|result),
            _=shutdown.changed()=>Err(anyhow::anyhow!("supervisor stopped")),
        }
    };
    state.sandbox.terminate(&handle).await?;
    cleanup_workspace(&state.sandbox, &workspace).await?;
    let bytes = result?;
    let request = request.clone();
    tokio::task::spawn_blocking(move || acquisition::collect_response(&request, &bytes)).await?
}

#[cfg(test)]
mod tests {
    use super::*;
    use piping_contracts::playbook_runner::{RunnerEventKind, RunnerGroupEvent};

    #[tokio::test]
    async fn acquisition_failure_response_contains_only_fixed_category() {
        let response =
            AcquireApiError::Acquisition(AcquireFailureCode::AuthenticationFailed).into_response();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        let failure: AcquireFailure = serde_json::from_slice(&body).unwrap();
        assert_eq!(failure.code, AcquireFailureCode::AuthenticationFailed);
        assert_eq!(body.as_ref(), br#"{"code":"authentication_failed"}"#);
    }

    fn dispatch_data() -> PersistedDispatch {
        PersistedDispatch {
            receipt: RunnerDispatchReceipt {
                dispatch_id: Uuid::new_v4(),
                run_group_id: Uuid::new_v4(),
                dispatch_attempt: 1,
            },
            fingerprint: "test".into(),
            status: RunnerJobState::Accepted,
            rc: None,
            cancel_requested: false,
            sandbox: None,
            callbacks: SupervisorCallbacks {
                group_token: "fixture".into(),
                run_tokens: BTreeMap::new(),
            },
            callback_base_url: "http://example.invalid".into(),
            lease_owner: "fixture".into(),
            targets: BTreeMap::new(),
            next_sequence: 0,
            delivered_sequence: 0,
            completion: None,
            completion_delivered: false,
            notification: None,
            notification_delivered: false,
            notification_dropped: false,
            terminal_at: None,
        }
    }

    #[test]
    fn native_group_success_keeps_empty_target_rows_and_sealed_console_prefix() {
        let mut data = dispatch_data();
        data.status = RunnerJobState::Succeeded;
        data.rc = Some(0);
        data.next_sequence = 7;
        let completed = completion(&data, &BTreeMap::new(), None);
        assert_eq!(completed.group_status, RunnerRunStatus::Succeeded);
        assert!(completed.runs.is_empty());
        assert_eq!(completed.console_next_sequence, 7);
        assert_eq!(completed.dispatch_id, data.receipt.dispatch_id);
    }

    #[test]
    fn replay_ack_uses_the_durable_backend_prefix_but_cannot_skip_unknown_bytes() {
        let mut data = dispatch_data();
        data.next_sequence = 7;
        let mut ack = ConsoleAck {
            dispatch_id: data.receipt.dispatch_id,
            dispatch_attempt: 1,
            next_sequence: 5,
        };
        assert_eq!(acknowledged_prefix(&data, 0, &ack).unwrap(), 5);
        ack.next_sequence = 8;
        assert!(acknowledged_prefix(&data, 0, &ack).is_err());
        ack.next_sequence = 0;
        assert!(acknowledged_prefix(&data, 0, &ack).is_err());
        ack.next_sequence = 1;
        ack.dispatch_id = Uuid::new_v4();
        assert!(acknowledged_prefix(&data, 0, &ack).is_err());
    }
    #[test]
    fn callback_origin_rejects_credentials_and_non_http_schemes() {
        for url in [
            "file:///etc/passwd",
            "https://user:secret@example.test",
            "https://example.test/?token=x",
        ] {
            assert!(endpoint(url, Uuid::nil(), "events/batch").is_err());
        }
    }
    #[test]
    fn sandbox_token_delivery_requires_an_https_api_origin() {
        let error = sandbox_token_api_url("http://api.example.test", Uuid::nil())
            .expect_err("plaintext API origin must be refused");
        assert_eq!(error, SandboxTokenApiError::NotHttps);
        let response = error.api_error();
        assert_eq!(response.0, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(response.1, SANDBOX_TOKEN_API_NOT_HTTPS);
        assert!(sandbox_token_api_url("https://api.example.test", Uuid::nil()).is_ok());
    }
    #[test]
    fn missing_host_summary_is_unreported_and_never_invents_a_verdict() {
        let mut data = dispatch_data();
        data.targets.insert("web".into(), Uuid::new_v4());
        data.status = RunnerJobState::Succeeded;
        data.rc = Some(0);
        let completed = completion(&data, &BTreeMap::new(), None);
        assert_eq!(completed.group_status, RunnerRunStatus::Succeeded);
        assert_eq!(completed.runs[0].status, RunnerRunStatus::Unreported);
        assert_eq!(completed.runs[0].exit_code, None);
        assert_eq!(
            completed.runs[0].error_message.as_deref(),
            Some("host outcome unreported by structured callback")
        );

        data.status = RunnerJobState::Failed;
        data.rc = Some(2);
        let completed = completion(
            &data,
            &BTreeMap::new(),
            Some("native Ansible exited with failure"),
        );
        assert_eq!(completed.group_status, RunnerRunStatus::Failed);
        assert_eq!(
            completed.runs[0].status,
            RunnerRunStatus::Unreported,
            "a nonzero process exit is not per-host evidence"
        );

        let completed = completion(
            &data,
            &BTreeMap::new(),
            Some("supervisor restarted; sandbox stopped"),
        );
        assert_eq!(completed.runs[0].status, RunnerRunStatus::Unreported);
    }

    #[test]
    fn recorded_host_verdicts_survive_group_failure_and_cancellation() {
        let mut data = dispatch_data();
        let web = Uuid::new_v4();
        let db = Uuid::new_v4();
        data.targets.insert("web".into(), web);
        data.targets.insert("db".into(), db);
        data.rc = Some(2);
        let verdicts = BTreeMap::from([("web".to_string(), true), ("db".to_string(), false)]);
        let status = |batch: &RunnerGroupCompletionBatch, run_id: Uuid| {
            batch
                .runs
                .iter()
                .find(|run| run.run_id == run_id)
                .expect("run is present")
                .status
        };

        data.status = RunnerJobState::Failed;
        let completed = completion(&data, &verdicts, Some("native Ansible exited with failure"));
        assert_eq!(status(&completed, web), RunnerRunStatus::Succeeded);
        assert_eq!(status(&completed, db), RunnerRunStatus::Failed);
        let db_error = completed
            .runs
            .iter()
            .find(|run| run.run_id == db)
            .and_then(|run| run.error_message.as_deref());
        assert_eq!(db_error, Some("structured host result reports failure"));

        data.status = RunnerJobState::Canceled;
        data.rc = None;
        let completed = completion(&data, &verdicts, Some("run canceled"));
        assert_eq!(
            status(&completed, web),
            RunnerRunStatus::Succeeded,
            "cancellation must not overwrite a recorded verdict"
        );
        assert_eq!(status(&completed, db), RunnerRunStatus::Failed);

        let completed = completion(&data, &BTreeMap::new(), Some("run canceled"));
        assert_eq!(completed.runs[0].status, RunnerRunStatus::Canceled);

        data.status = RunnerJobState::TimedOut;
        let completed = completion(&data, &BTreeMap::new(), Some("job timeout"));
        assert_eq!(completed.runs[0].status, RunnerRunStatus::TimedOut);
    }

    fn delivery_state(
        root: &tempfile::TempDir,
        id: Uuid,
        control: &Arc<JobControl>,
    ) -> anyhow::Result<AppState> {
        let workspace_root = root.path().join("jobs");
        let runtime_dir = root.path().join("runtime");
        store::private_directory(&workspace_root)?;
        store::private_directory(&runtime_dir)?;
        let sandbox = Sandbox::new(SandboxConfig {
            runtime: sandbox::Runtime::Docker,
            workspace_root: workspace_root.clone(),
            host_workspace_root: workspace_root,
            runtime_dir,
            installation_id: "test".into(),
            task_label: None,
            network: None,
        })?;
        let store = SupervisorStore::open(&root.path().join("state"))?;
        Ok(AppState {
            secret: "fixture".into(),
            store,
            sandbox,
            client: reqwest::Client::builder().no_proxy().build()?,
            jobs: Mutex::new(HashMap::from([(id, Arc::clone(control))])),
            acquisitions: Mutex::new(HashMap::new()),
            slots: Arc::new(Semaphore::new(1)),
            shutdown: watch::channel(false).0,
        })
    }

    async fn notification_control(origin: String) -> anyhow::Result<(Uuid, Arc<JobControl>)> {
        let id = Uuid::new_v4();
        let mut data = dispatch_data();
        data.receipt.dispatch_id = id;
        data.callbacks.group_token = "fixture".into();
        data.callback_base_url = origin;
        data.status = RunnerJobState::Succeeded;
        let completed = completion(&data, &BTreeMap::new(), None);
        data.completion = Some(completed);
        data.notification = Some(RunnerGroupEventBatch {
            batch_id: Uuid::new_v4(),
            events: vec![RunnerGroupEvent {
                run_id: None,
                host: None,
                event_kind: RunnerEventKind::System,
                event_type: "notification_unavailable".into(),
                message: None,
                payload: Some(json!({"reason": "absent"})),
            }],
        });
        let control = Arc::new(JobControl {
            data: Mutex::new(data),
            cancel: watch::channel(false).0,
        });
        Ok((id, control))
    }

    /// A succeeded group with a valid `web` verdict and a pending notification.
    async fn verdict_control(origin: String) -> anyhow::Result<(Uuid, Arc<JobControl>, Uuid)> {
        let id = Uuid::new_v4();
        let run_id = Uuid::new_v4();
        let mut data = dispatch_data();
        data.receipt.dispatch_id = id;
        data.callbacks.group_token = "fixture".into();
        data.callback_base_url = origin;
        data.status = RunnerJobState::Succeeded;
        data.rc = Some(0);
        data.targets.insert("web".into(), run_id);
        let verdicts = BTreeMap::from([("web".to_string(), true)]);
        let completed = completion(&data, &verdicts, None);
        data.completion = Some(completed);
        let batch_id = Uuid::new_v4();
        data.notification = Some(RunnerGroupEventBatch {
            batch_id,
            events: vec![RunnerGroupEvent {
                run_id: Some(run_id),
                host: Some("web".into()),
                event_kind: RunnerEventKind::Result,
                event_type: "host_summary".into(),
                message: None,
                payload: Some(json!({"ok": 1, "failed": 0, "unreachable": 0})),
            }],
        });
        let control = Arc::new(JobControl {
            data: Mutex::new(data),
            cancel: watch::channel(false).0,
        });
        Ok((id, control, batch_id))
    }

    #[tokio::test]
    async fn structured_events_are_delivered_once_before_completion() -> anyhow::Result<()> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let origin = format!("http://{}", listener.local_addr()?);
        let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let log = Arc::clone(&seen);
        let server = tokio::spawn(async move {
            let app = Router::new().fallback(move |request: axum::extract::Request| {
                let log = Arc::clone(&log);
                async move {
                    log.lock().unwrap().push(request.uri().path().to_string());
                    (StatusCode::OK, Json(Vec::<Value>::new()))
                }
            });
            let _ = axum::serve(listener, app).await;
        });
        let (id, control) = notification_control(origin).await?;
        let root = tempfile::tempdir()?;
        let state = delivery_state(&root, id, &control)?;
        deliver_one(&state, &control).await?;
        let paths = seen.lock().unwrap().clone();
        assert_eq!(paths.len(), 2);
        assert!(paths[0].ends_with("/events/batch"), "{paths:?}");
        assert!(paths[1].ends_with("/completion-batch"), "{paths:?}");
        let data = control.data.lock().await;
        assert!(data.notification_delivered && data.completion_delivered);
        drop(data);

        deliver_one(&state, &control).await?;
        assert_eq!(seen.lock().unwrap().len(), 2, "replay must not resend");
        server.abort();
        Ok(())
    }

    #[tokio::test]
    async fn a_refused_notification_is_visible_in_the_completion_body() -> anyhow::Result<()> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let origin = format!("http://{}", listener.local_addr()?);
        let seen = Arc::new(std::sync::Mutex::new(Vec::<(String, Vec<u8>)>::new()));
        let log = Arc::clone(&seen);
        let server = tokio::spawn(async move {
            let app = Router::new().fallback(move |request: axum::extract::Request| {
                let log = Arc::clone(&log);
                async move {
                    let path = request.uri().path().to_string();
                    let body = axum::body::to_bytes(request.into_body(), 64 * 1024)
                        .await
                        .unwrap_or_default()
                        .to_vec();
                    log.lock().unwrap().push((path.clone(), body));
                    let status = if path.ends_with("/events/batch") {
                        StatusCode::BAD_REQUEST
                    } else {
                        StatusCode::OK
                    };
                    (status, Json(Vec::<Value>::new()))
                }
            });
            let _ = axum::serve(listener, app).await;
        });
        let (id, control, batch_id) = verdict_control(origin).await?;
        let root = tempfile::tempdir()?;
        let state = delivery_state(&root, id, &control)?;
        deliver_one(&state, &control).await?;
        let requests = seen.lock().unwrap().clone();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].0.ends_with("/events/batch"));
        assert!(requests[1].0.ends_with("/completion-batch"));
        let event_body: Value = serde_json::from_slice(&requests[0].1)?;
        let expected_batch = batch_id.to_string();
        assert_eq!(event_body["batch_id"], serde_json::json!(expected_batch));
        let completion: Value = serde_json::from_slice(&requests[1].1)?;
        assert_eq!(completion["group_status"], "succeeded");
        assert_eq!(completion["runs"][0]["status"], "succeeded");
        assert!(
            completion["error_message"]
                .as_str()
                .is_some_and(|message| message.contains("structured notification was refused")),
            "{completion}"
        );
        assert!(
            completion["runs"][0]["error_message"]
                .as_str()
                .is_some_and(|message| message.contains("structured notification was refused")),
            "the persisted per-run message must carry the diagnostic: {completion}"
        );
        let data = control.data.lock().await;
        assert!(data.notification_dropped && data.completion_delivered);
        drop(data);
        server.abort();
        Ok(())
    }

    #[tokio::test]
    async fn an_authentication_refusal_fails_closed_and_reuses_the_batch_id() -> anyhow::Result<()>
    {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let origin = format!("http://{}", listener.local_addr()?);
        let seen = Arc::new(std::sync::Mutex::new(Vec::<(String, Vec<u8>)>::new()));
        let log = Arc::clone(&seen);
        let server = tokio::spawn(async move {
            let app = Router::new().fallback(move |request: axum::extract::Request| {
                let log = Arc::clone(&log);
                async move {
                    let path = request.uri().path().to_string();
                    let body = axum::body::to_bytes(request.into_body(), 64 * 1024)
                        .await
                        .unwrap_or_default()
                        .to_vec();
                    log.lock().unwrap().push((path.clone(), body));
                    let status = if path.ends_with("/events/batch") {
                        StatusCode::UNAUTHORIZED
                    } else {
                        StatusCode::OK
                    };
                    (status, Json(Vec::<Value>::new()))
                }
            });
            let _ = axum::serve(listener, app).await;
        });
        let (id, control, batch_id) = verdict_control(origin).await?;
        let root = tempfile::tempdir()?;
        let state = delivery_state(&root, id, &control)?;
        assert!(deliver_one(&state, &control).await.is_err());
        assert!(deliver_one(&state, &control).await.is_err());
        let requests = seen.lock().unwrap().clone();
        assert_eq!(requests.len(), 2, "completion must stay blocked");
        assert!(
            requests
                .iter()
                .all(|(path, _)| path.ends_with("/events/batch"))
        );
        for (_, body) in &requests {
            let event: Value = serde_json::from_slice(body)?;
            let expected_batch = batch_id.to_string();
            assert_eq!(event["batch_id"], serde_json::json!(expected_batch));
        }
        let data = control.data.lock().await;
        assert!(!data.notification_dropped && !data.completion_delivered);
        drop(data);
        server.abort();
        Ok(())
    }

    #[tokio::test]
    async fn accepted_cancel_survives_before_worker_subscribes() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let workspace_root = root.path().join("jobs");
        let runtime_dir = root.path().join("runtime");
        store::private_directory(&workspace_root)?;
        store::private_directory(&runtime_dir)?;
        let sandbox = Sandbox::new(SandboxConfig {
            runtime: sandbox::Runtime::Docker,
            workspace_root: workspace_root.clone(),
            host_workspace_root: workspace_root,
            runtime_dir,
            installation_id: "test".into(),
            task_label: None,
            network: None,
        })?;
        let store = SupervisorStore::open(&root.path().join("state"))?;
        let id = Uuid::new_v4();
        let mut data = dispatch_data();
        data.receipt.dispatch_id = id;
        let (cancel, receiver) = watch::channel(false);
        drop(receiver);
        let control = Arc::new(JobControl {
            data: Mutex::new(data),
            cancel,
        });
        let state = Arc::new(AppState {
            secret: "fixture".into(),
            store,
            sandbox,
            client: reqwest::Client::new(),
            jobs: Mutex::new(HashMap::from([(id, Arc::clone(&control))])),
            acquisitions: Mutex::new(HashMap::new()),
            slots: Arc::new(Semaphore::new(1)),
            shutdown: watch::channel(false).0,
        });
        let mut headers = HeaderMap::new();
        headers.insert(axum::http::header::AUTHORIZATION, "Bearer fixture".parse()?);
        let result = cancel_dispatch(State(Arc::clone(&state)), headers, Path(id))
            .await
            .map_err(|_| anyhow::anyhow!("cancel failed"))?;
        assert_eq!(result.0.status, RunnerJobState::Canceling);
        assert!(*control.cancel.subscribe().borrow());
        let persisted = state.store.load_dispatches::<PersistedDispatch>()?;
        assert!(persisted[0].1.cancel_requested);
        assert_eq!(persisted[0].1.status, RunnerJobState::Canceling);
        Ok(())
    }
}
