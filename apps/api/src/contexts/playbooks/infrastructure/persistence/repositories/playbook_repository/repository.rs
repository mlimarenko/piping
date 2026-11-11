use super::{
    AppendRunGroupEventBatchOutcome, CallbackTokenAuth, CompleteRunGroupBatchRecord,
    CompleteRunRecord, CompletedRunGroupBatchRecord, CompletedRunRecord, CreateArtifactRecord,
    CreateAssignmentRecord, CreateExecutionEnvironmentRecord, CreateJobTemplateRecord,
    CreateJobTokenRecord, CreateProjectRecord, CreateRunEventRecord, CreateRunGroupEventRecord,
    CreateRunGroupRecord, CreateScheduleRecord,
    CreateTargetSetRecord, CreatedRunGroupRecord, DueScheduleRecord, FinalizeRunGroupCancelRecord,
    FinalizedRunGroupCancelRecord, LeaseQueuedRunsRecord, LeaseScheduleOccurrencesRecord,
    LeasedScheduleOccurrenceRecord, PlaybookAssignmentListFilter, PlaybookRunListFilter,
    PlaybookScheduleListFilter, PlaybookTargetSetListFilter,
    ReleaseScheduleOccurrenceRecord, RunGroupWithRuns,
    SeedSystemCatalogRecord, SeededSystemCatalog,
    SetActiveRunTimeoutsRecord, StartLeasedRunsRecord, StartedRunRecord, TargetAssignmentsRecord,
    TimedOutRunsRecord, TimeoutStuckRunsRecord, UpdateAssignmentRecord, UpdateScheduleRecord,
    UpdateTargetSetRecord,
};
use crate::contexts::playbooks::application::playbook_service::PlaybookRunGroupEventListFilter;
use crate::contexts::playbooks::domain::{
    JobTemplate, JobTokenAudience, PlaybookArtifact, PlaybookAssignment,
    PlaybookExecutionEnvironment, PlaybookJobToken, PlaybookRun, PlaybookRunEvent,
    PlaybookRunGroup, PlaybookSchedule, PlaybookServicePrincipal, PlaybookTargetPreviewItem,
    PlaybookTargetSelectorPreview, PlaybookTargetSet, PlaybookTargetSetListItem,
    PlaybookTargetSetMember, PlaybookTargetSetPreview, Project, ProjectEntry, ProjectRevision,
    RevisionChange,
};
use crate::contexts::shared::pagination::PageResult;
use crate::errors::AppError;
use async_trait::async_trait;
use piping_contracts::playbook_runner::{ConsoleAck, ConsolePage, RunnerConsoleBatch};
use serde_json::Value;
use uuid::Uuid;

#[async_trait]
pub trait PlaybookRepository: Send + Sync {
    /// Idempotent, atomic write of the bundled catalog: the single system
    /// project, one immutable revision per catalog tree hash and the system job
    /// definitions. A repeated seed with unchanged inputs changes nothing.
    async fn seed_system_catalog(
        &self,
        record: SeedSystemCatalogRecord,
    ) -> Result<SeededSystemCatalog, AppError>;
    /// Atomically applies all changes against the current base revision. A stale
    /// base is a conflict and Git-backed projects reject edits.
    async fn change_project_revision(
        &self,
        project_id: Uuid,
        base_revision_id: Uuid,
        changes: Vec<RevisionChange>,
        created_by: Option<Uuid>,
    ) -> Result<ProjectRevision, AppError>;
    async fn get_project_revision(
        &self,
        project_id: Uuid,
        revision_id: Uuid,
    ) -> Result<Option<ProjectRevision>, AppError>;
    async fn fork_project(
        &self,
        project_id: Uuid,
        revision_id: Uuid,
        name: String,
        created_by: Option<Uuid>,
    ) -> Result<Project, AppError>;
    async fn import_project_revision(
        &self,
        project_id: Uuid,
        base_revision_id: Option<Uuid>,
        expected_updated_at: chrono::DateTime<chrono::Utc>,
        source_commit_oid: String,
        entries: Vec<ProjectEntry>,
        created_by: Option<Uuid>,
    ) -> Result<ProjectRevision, AppError>;
    async fn list_execution_environments(
        &self,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<PlaybookExecutionEnvironment>, AppError>;

    async fn get_execution_environment(
        &self,
        id: Uuid,
    ) -> Result<Option<PlaybookExecutionEnvironment>, AppError>;

    async fn create_execution_environment(
        &self,
        record: CreateExecutionEnvironmentRecord,
    ) -> Result<PlaybookExecutionEnvironment, AppError>;

    async fn list_projects(&self, limit: u32, offset: u32)
    -> Result<PageResult<Project>, AppError>;

    async fn get_project(&self, id: Uuid) -> Result<Option<Project>, AppError>;

    async fn create_project(&self, record: CreateProjectRecord) -> Result<Project, AppError>;

    async fn list_job_templates(
        &self,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<JobTemplate>, AppError>;

    async fn get_job_template(&self, id: Uuid) -> Result<Option<JobTemplate>, AppError>;

    /// Every job template of one project. Used to validate a candidate revision
    /// against the schema files the project's templates pin.
    async fn list_job_templates_for_project(
        &self,
        project_id: Uuid,
    ) -> Result<Vec<JobTemplate>, AppError>;

    async fn create_job_template(
        &self,
        record: CreateJobTemplateRecord,
    ) -> Result<JobTemplate, AppError>;

    async fn update_execution_environment(
        &self,
        id: Uuid,
        record: CreateExecutionEnvironmentRecord,
    ) -> Result<Option<PlaybookExecutionEnvironment>, AppError>;

    async fn delete_execution_environment(&self, id: Uuid) -> Result<bool, AppError>;

    /// Number of projects and job templates that still reference this execution
    /// environment. Used to refuse a delete before the `ON DELETE SET NULL` FKs
    /// would silently null those references out.
    async fn count_execution_environment_references(&self, id: Uuid) -> Result<i64, AppError>;

    async fn update_project(
        &self,
        id: Uuid,
        record: CreateProjectRecord,
    ) -> Result<Option<Project>, AppError>;

    async fn delete_project(&self, id: Uuid) -> Result<bool, AppError>;

    /// Number of job templates that still reference this project. Used to refuse
    /// a delete before the `ON DELETE CASCADE` FK would silently remove them.
    async fn count_job_templates_for_project(&self, id: Uuid) -> Result<i64, AppError>;

    async fn update_job_template(
        &self,
        id: Uuid,
        record: CreateJobTemplateRecord,
    ) -> Result<Option<JobTemplate>, AppError>;

    async fn delete_job_template(&self, id: Uuid) -> Result<bool, AppError>;

    async fn create_target_set(
        &self,
        record: CreateTargetSetRecord,
        preview_limit: i64,
    ) -> Result<PlaybookTargetSetPreview, AppError>;

    async fn update_target_set(
        &self,
        record: UpdateTargetSetRecord,
    ) -> Result<Option<PlaybookTargetSet>, AppError>;

    async fn delete_target_set(&self, id: Uuid) -> Result<bool, AppError>;

    async fn prune_deleted_role_from_target_sets(
        &self,
        role_id: Uuid,
        role_name: &str,
    ) -> Result<u64, AppError>;

    /// One page of target sets plus the TOTAL number of matches, so callers
    /// can report a page count (see `list_schedules`). Each row carries the
    /// resolved host count and usage the list shows.
    async fn list_target_sets(
        &self,
        filter: &PlaybookTargetSetListFilter,
        limit: i64,
        offset: i64,
    ) -> Result<(Vec<PlaybookTargetSetListItem>, i64), AppError>;

    /// One page of assignments plus the TOTAL number of matches, so callers
    /// can report a page count (see `list_schedules`).
    async fn list_assignments(
        &self,
        filter: &PlaybookAssignmentListFilter,
        limit: i64,
        offset: i64,
    ) -> Result<(Vec<PlaybookAssignment>, i64), AppError>;

    async fn get_target_set(&self, id: Uuid) -> Result<Option<PlaybookTargetSet>, AppError>;

    async fn list_target_set_members(
        &self,
        id: Uuid,
    ) -> Result<Vec<PlaybookTargetSetMember>, AppError>;

    async fn preview_target_set(
        &self,
        id: Uuid,
        limit: i64,
    ) -> Result<Option<PlaybookTargetSetPreview>, AppError>;

    async fn list_target_set_facets(
        &self,
        kind: crate::contexts::playbooks::domain::TargetSetFacetKind,
        selector: &Value,
        search: &str,
        limit: i64,
        offset: i64,
    ) -> Result<crate::contexts::playbooks::domain::TargetSetFacetPage, AppError>;

    /// Resolve an unsaved dynamic selector into the same rows, total and
    /// truncation flag a saved set with that selector reports. Nothing is
    /// persisted, so an editor can show the result before saving.
    async fn preview_target_selector(
        &self,
        selector: &Value,
        limit: i64,
    ) -> Result<PlaybookTargetSelectorPreview, AppError>;

    /// Resolve an ad-hoc dynamic selector into complete target rows. The
    /// implementation fetches `limit + 1` in one SELECT so an oversized result
    /// is a validation error rather than a silent truncation or a second count
    /// racing a different snapshot.
    async fn resolve_target_selector(
        &self,
        selector: &Value,
        limit: i64,
    ) -> Result<Vec<PlaybookTargetPreviewItem>, AppError>;

    async fn create_assignment(
        &self,
        record: CreateAssignmentRecord,
    ) -> Result<PlaybookAssignment, AppError>;

    async fn get_assignment(&self, id: Uuid) -> Result<Option<PlaybookAssignment>, AppError>;

    async fn update_assignment(
        &self,
        record: UpdateAssignmentRecord,
    ) -> Result<Option<PlaybookAssignment>, AppError>;

    async fn delete_assignment(&self, id: Uuid) -> Result<bool, AppError>;

    async fn list_target_assignments(
        &self,
        target_id: Uuid,
        limit: i64,
    ) -> Result<Option<TargetAssignmentsRecord>, AppError>;

    async fn get_service_principal(
        &self,
        id: Uuid,
    ) -> Result<Option<PlaybookServicePrincipal>, AppError>;

    async fn create_schedule(
        &self,
        record: CreateScheduleRecord,
    ) -> Result<PlaybookSchedule, AppError>;

    async fn get_schedule(&self, id: Uuid) -> Result<Option<PlaybookSchedule>, AppError>;

    /// One page of schedules plus the TOTAL number of matches, so callers can
    /// report a page count. Returning only the page made `total` the page
    /// length, which capped every UI at one page.
    async fn list_schedules(
        &self,
        filter: &PlaybookScheduleListFilter,
        limit: i64,
        offset: i64,
    ) -> Result<(Vec<PlaybookSchedule>, i64), AppError>;

    async fn update_schedule(
        &self,
        record: UpdateScheduleRecord,
    ) -> Result<Option<PlaybookSchedule>, AppError>;

    async fn delete_schedule(&self, id: Uuid) -> Result<bool, AppError>;

    /// Stop a schedule whose owner no longer holds launch authority.
    ///
    /// Returns `true` only for the transition that disabled it, so the caller
    /// records one lifecycle event and a repeated failure stays silent.
    async fn pause_schedule_owner_authority(
        &self,
        id: Uuid,
        reason: &str,
    ) -> Result<bool, AppError>;

    async fn list_due_schedules(&self, limit: i64) -> Result<Vec<DueScheduleRecord>, AppError>;

    async fn lease_schedule_occurrences(
        &self,
        record: LeaseScheduleOccurrencesRecord,
    ) -> Result<Vec<LeasedScheduleOccurrenceRecord>, AppError>;

    async fn release_schedule_occurrence(
        &self,
        record: ReleaseScheduleOccurrenceRecord,
    ) -> Result<bool, AppError>;

    /// Persist one immutable resolved launch and its exact host rows. Target
    /// sets are never previewed or re-expanded here: `launch_spec.targets` is
    /// the authoritative, already-resolved host list.
    async fn create_run_group(
        &self,
        record: CreateRunGroupRecord,
    ) -> Result<CreatedRunGroupRecord, AppError>;

    async fn get_run(&self, run_id: Uuid) -> Result<Option<PlaybookRun>, AppError>;

    async fn list_runs(
        &self,
        filter: &PlaybookRunListFilter,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<PlaybookRun>, AppError>;

    /// One page of run groups plus the TOTAL number of matches, so callers
    /// can report a page count (see `list_schedules`).
    async fn list_run_groups(
        &self,
        filter: &PlaybookRunListFilter,
        limit: i64,
        offset: i64,
    ) -> Result<(Vec<PlaybookRunGroup>, i64), AppError>;

    async fn get_run_group_with_runs(
        &self,
        run_group_id: Uuid,
        limit: i64,
    ) -> Result<Option<RunGroupWithRuns>, AppError>;

    /// Persist a cancellation request for the run's execution group. Work that
    /// has no dispatch identity terminalizes immediately; dispatched work stays
    /// active until `finalize_run_group_cancel` records confirmation or expiry.
    async fn request_run_cancel(
        &self,
        run_id: Uuid,
        now: chrono::DateTime<chrono::Utc>,
        deadline_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<Option<PlaybookRun>, AppError>;

    /// Persist one durable group cancellation command. The `limit` caps only
    /// the returned member set, never the update itself.
    async fn request_run_group_cancel(
        &self,
        run_group_id: Uuid,
        limit: i64,
        now: chrono::DateTime<chrono::Utc>,
        deadline_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<Option<RunGroupWithRuns>, AppError>;

    async fn list_pending_run_group_cancels(
        &self,
        limit: i64,
    ) -> Result<Vec<PlaybookRunGroup>, AppError>;

    async fn finalize_run_group_cancel(
        &self,
        record: FinalizeRunGroupCancelRecord,
    ) -> Result<Option<FinalizedRunGroupCancelRecord>, AppError>;

    async fn create_job_token(
        &self,
        record: CreateJobTokenRecord,
    ) -> Result<PlaybookJobToken, AppError>;

    async fn validate_active_job_token(
        &self,
        token_id: Uuid,
        token_hash: &str,
        audience: JobTokenAudience,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<PlaybookJobToken, AppError>;

    async fn create_artifact_with_callback(
        &self,
        auth: CallbackTokenAuth,
        record: CreateArtifactRecord,
        audience: JobTokenAudience,
    ) -> Result<PlaybookArtifact, AppError>;

    async fn list_artifacts(
        &self,
        run_id: Uuid,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<PlaybookArtifact>, AppError>;

    async fn append_run_event_with_callback(
        &self,
        auth: CallbackTokenAuth,
        record: CreateRunEventRecord,
        content_size_bytes: i64,
    ) -> Result<PlaybookRunEvent, AppError>;

    async fn append_run_group_event_with_callback(
        &self,
        auth: CallbackTokenAuth,
        record: CreateRunGroupEventRecord,
        content_size_bytes: i64,
    ) -> Result<PlaybookRunEvent, AppError>;

    async fn append_run_group_event_batch(
        &self,
        batch_id: Uuid,
        records: Vec<CreateRunGroupEventRecord>,
    ) -> Result<AppendRunGroupEventBatchOutcome, AppError>;

    async fn append_run_group_event_batch_with_callback(
        &self,
        auth: CallbackTokenAuth,
        batch_id: Uuid,
        records: Vec<(CreateRunGroupEventRecord, i64)>,
    ) -> Result<AppendRunGroupEventBatchOutcome, AppError>;

    async fn count_run_group_runs(
        &self,
        run_group_id: Uuid,
        run_ids: &[Uuid],
    ) -> Result<i64, AppError>;

    async fn list_run_group_events(
        &self,
        run_group_id: Uuid,
        filter: PlaybookRunGroupEventListFilter,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<PlaybookRunEvent>, AppError>;

    async fn list_run_events(
        &self,
        run_id: Uuid,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<PlaybookRunEvent>, AppError>;

    /// Append a supervisor console batch using the group callback token. The
    /// durable prefix is contiguous across stdout/stderr for one dispatch;
    /// exact retries succeed, gaps or changed bytes conflict, and a sealed
    /// prefix rejects new chunks.
    async fn append_console_batch_with_callback(
        &self,
        group_id: Uuid,
        batch: RunnerConsoleBatch,
        auth: CallbackTokenAuth,
    ) -> Result<ConsoleAck, AppError>;

    /// Read the durable console from an inclusive sequence cursor. `complete`
    /// only asserts that the page reached the supervisor's sealed prefix.
    async fn list_console(
        &self,
        group_id: Uuid,
        from_sequence: u64,
        limit: i64,
        dispatch_id: Option<Uuid>,
    ) -> Result<ConsolePage, AppError>;

    /// Lease up to `limit` queued run groups, including groups whose resolved
    /// host list is empty. Member runs are leased alongside their group.
    async fn lease_queued_groups(
        &self,
        record: LeaseQueuedRunsRecord,
    ) -> Result<Vec<PlaybookRunGroup>, AppError>;

    /// Durably accept the group dispatch. Works with an empty `run_ids` list so
    /// a zero-host launch still receives its group callback token.
    async fn start_leased_runs(
        &self,
        record: StartLeasedRunsRecord,
    ) -> Result<Vec<StartedRunRecord>, AppError>;

    /// Transition the group and its dispatching members to `running`.
    async fn mark_run_group_running(
        &self,
        run_group_id: Uuid,
        lease_owner: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Option<PlaybookRunGroup>, AppError>;

    /// Update the execution timeout for already-started runs owned by the
    /// worker before dispatching a grouped runner job. Grouped Ansible execution
    /// uses a single runner process for many run rows, so the runner job timeout,
    /// per-run timeout reaper, and run-group lease must be extended together.
    async fn set_active_run_timeouts(
        &self,
        record: SetActiveRunTimeoutsRecord,
    ) -> Result<Vec<PlaybookRun>, AppError>;

    /// Park a `dispatching` group only after receipt lookup affirmatively proves
    /// that the runner never accepted its dispatch identity. The whole group
    /// returns to the lease queue; no sibling is requeued independently.
    async fn requeue_run_group(
        &self,
        run_group_id: Uuid,
        lease_owner: &str,
        retry_at: chrono::DateTime<chrono::Utc>,
        reason: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, AppError>;

    async fn complete_run(&self, record: CompleteRunRecord)
    -> Result<CompletedRunRecord, AppError>;

    async fn complete_run_with_callback(
        &self,
        auth: CallbackTokenAuth,
        record: CompleteRunRecord,
    ) -> Result<CompletedRunRecord, AppError>;

    /// Complete a group batch without callback authorization (server-internal
    /// failure/terminal path). A zero-row batch terminalizes the group from the
    /// explicit `group_status`.
    async fn complete_run_group_batch(
        &self,
        record: CompleteRunGroupBatchRecord,
    ) -> Result<CompletedRunGroupBatchRecord, AppError>;

    async fn complete_run_group_batch_with_callback(
        &self,
        auth: CallbackTokenAuth,
        record: CompleteRunGroupBatchRecord,
    ) -> Result<CompletedRunGroupBatchRecord, AppError>;

    async fn timeout_stuck_runs(
        &self,
        record: TimeoutStuckRunsRecord,
    ) -> Result<TimedOutRunsRecord, AppError>;
}
