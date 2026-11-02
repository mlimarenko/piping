use super::{
    AppendRunGroupEventBatchOutcome, CallbackTokenAuth, CompleteRunGroupBatchRecord,
    CompleteRunRecord, CompletedRunGroupBatchRecord, CompletedRunRecord, CreateArtifactRecord,
    CreateAssignmentRecord, CreateExecutionEnvironmentRecord, CreateJobTemplateRecord,
    CreateJobTokenRecord, CreatePlaybookRecord, CreatePlaybookVersionRecord, CreateProjectRecord,
    CreateRunEventRecord, CreateRunGroupEventRecord, CreateRunGroupRecord, CreateScheduleRecord,
    CreateServicePrincipalRecord, CreateTargetSetRecord, CreatedRunGroupRecord, DueScheduleRecord,
    FinalizeRunGroupCancelRecord, FinalizedRunGroupCancelRecord, LeaseQueuedRunsRecord,
    LeaseScheduleOccurrencesRecord, LeasedScheduleOccurrenceRecord, MarkRunRunningRecord,
    MarkRunsRunningRecord, PlaybookAssignmentListFilter, PlaybookRunListFilter,
    PlaybookScheduleListFilter, PlaybookServicePrincipalListFilter, PlaybookTargetSetListFilter,
    ReleaseScheduleOccurrenceRecord, RequeueStartedRunsRecord, RevokeServicePrincipalRecord,
    RunGroupWithRuns, SetActiveRunTimeoutsRecord, StartLeasedRunRecord, StartLeasedRunsRecord,
    StartedRunRecord, TimedOutRunsRecord, TimeoutStuckRunsRecord, UpdateAssignmentRecord,
    UpdatePlaybookRecord, UpdateScheduleRecord, UpdateTargetSetRecord,
};
use crate::contexts::playbooks::application::playbook_service::PlaybookRunGroupEventListFilter;
use crate::contexts::playbooks::application::{PlaybookCatalogItem, PlaybookTargetAssignments};
use crate::contexts::playbooks::domain::{
    PlaybookArtifact, PlaybookAssignment, PlaybookExecutionEnvironment, PlaybookJobTemplate,
    PlaybookJobToken, PlaybookProject, PlaybookRun, PlaybookRunEvent, PlaybookRunGroup,
    PlaybookSchedule, PlaybookServicePrincipal, PlaybookTargetSet, PlaybookTargetSetPreview,
    PlaybookVersion,
};
use crate::contexts::shared::pagination::PageResult;
use crate::errors::AppError;
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;

#[async_trait]
pub trait PlaybookRepository: Send + Sync {
    async fn list_catalog(
        &self,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<PlaybookCatalogItem>, AppError>;

    async fn get_catalog_item(&self, id: Uuid) -> Result<Option<PlaybookCatalogItem>, AppError>;

    async fn create_playbook(
        &self,
        record: CreatePlaybookRecord,
    ) -> Result<PlaybookCatalogItem, AppError>;

    async fn update_playbook(
        &self,
        record: UpdatePlaybookRecord,
    ) -> Result<Option<PlaybookCatalogItem>, AppError>;

    async fn delete_playbook(&self, id: Uuid) -> Result<bool, AppError>;

    async fn list_versions(
        &self,
        playbook_id: Uuid,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<PlaybookVersion>, AppError>;

    async fn create_version(
        &self,
        record: CreatePlaybookVersionRecord,
    ) -> Result<Option<PlaybookVersion>, AppError>;

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

    async fn list_projects(
        &self,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<PlaybookProject>, AppError>;

    async fn get_project(&self, id: Uuid) -> Result<Option<PlaybookProject>, AppError>;

    async fn create_project(
        &self,
        record: CreateProjectRecord,
    ) -> Result<PlaybookProject, AppError>;

    async fn list_job_templates(
        &self,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<PlaybookJobTemplate>, AppError>;

    async fn get_job_template(&self, id: Uuid) -> Result<Option<PlaybookJobTemplate>, AppError>;

    async fn create_job_template(
        &self,
        record: CreateJobTemplateRecord,
    ) -> Result<PlaybookJobTemplate, AppError>;

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
    ) -> Result<Option<PlaybookProject>, AppError>;

    /// Replaces `metadata.project_files` for a manual project without disturbing
    /// any other metadata keys, returning the refreshed row.
    async fn set_project_files(
        &self,
        id: Uuid,
        project_files: Value,
        user_id: Option<Uuid>,
    ) -> Result<Option<PlaybookProject>, AppError>;

    async fn delete_project(&self, id: Uuid) -> Result<bool, AppError>;

    /// Number of job templates that still reference this project. Used to refuse
    /// a delete before the `ON DELETE CASCADE` FK would silently remove them.
    async fn count_job_templates_for_project(&self, id: Uuid) -> Result<i64, AppError>;

    async fn update_job_template(
        &self,
        id: Uuid,
        record: CreateJobTemplateRecord,
    ) -> Result<Option<PlaybookJobTemplate>, AppError>;

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
    /// can report a page count (see `list_schedules`).
    async fn list_target_sets(
        &self,
        filter: &PlaybookTargetSetListFilter,
        limit: i64,
        offset: i64,
    ) -> Result<(Vec<PlaybookTargetSet>, i64), AppError>;

    /// One page of assignments plus the TOTAL number of matches, so callers
    /// can report a page count (see `list_schedules`).
    async fn list_assignments(
        &self,
        filter: &PlaybookAssignmentListFilter,
        limit: i64,
        offset: i64,
    ) -> Result<(Vec<PlaybookAssignment>, i64), AppError>;

    async fn get_target_set(&self, id: Uuid) -> Result<Option<PlaybookTargetSet>, AppError>;

    async fn preview_target_set(
        &self,
        id: Uuid,
        limit: i64,
    ) -> Result<Option<PlaybookTargetSetPreview>, AppError>;

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
    ) -> Result<Option<PlaybookTargetAssignments>, AppError>;

    async fn get_service_principal(
        &self,
        id: Uuid,
    ) -> Result<Option<PlaybookServicePrincipal>, AppError>;

    /// One page of service principals plus the TOTAL number of matches, so
    /// callers can report a page count (see `list_schedules`).
    async fn list_service_principals(
        &self,
        filter: &PlaybookServicePrincipalListFilter,
        limit: i64,
        offset: i64,
    ) -> Result<(Vec<PlaybookServicePrincipal>, i64), AppError>;

    async fn create_service_principal(
        &self,
        record: CreateServicePrincipalRecord,
    ) -> Result<PlaybookServicePrincipal, AppError>;

    async fn revoke_service_principal(
        &self,
        record: RevokeServicePrincipalRecord,
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

    async fn list_due_schedules(&self, limit: i64) -> Result<Vec<DueScheduleRecord>, AppError>;

    async fn lease_schedule_occurrences(
        &self,
        record: LeaseScheduleOccurrencesRecord,
    ) -> Result<Vec<LeasedScheduleOccurrenceRecord>, AppError>;

    async fn release_schedule_occurrence(
        &self,
        record: ReleaseScheduleOccurrenceRecord,
    ) -> Result<bool, AppError>;

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
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<PlaybookJobToken, AppError>;

    async fn create_artifact_with_callback(
        &self,
        auth: CallbackTokenAuth,
        record: CreateArtifactRecord,
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

    async fn lease_queued_runs(
        &self,
        record: LeaseQueuedRunsRecord,
    ) -> Result<Vec<PlaybookRun>, AppError>;

    async fn start_leased_run(
        &self,
        record: StartLeasedRunRecord,
    ) -> Result<StartedRunRecord, AppError>;

    async fn start_leased_runs(
        &self,
        record: StartLeasedRunsRecord,
    ) -> Result<Vec<StartedRunRecord>, AppError>;

    /// Transition a durably accepted dispatch from `dispatching` to `running`.
    async fn mark_run_running(&self, record: MarkRunRunningRecord)
    -> Result<PlaybookRun, AppError>;

    async fn mark_runs_running(
        &self,
        record: MarkRunsRunningRecord,
    ) -> Result<Vec<PlaybookRun>, AppError>;

    /// Update the execution timeout for already-started runs owned by the
    /// worker before dispatching a grouped runner job. Grouped Ansible execution
    /// uses a single runner process for many run rows, so the runner job timeout,
    /// per-run timeout reaper, and run-group lease must be extended together.
    async fn set_active_run_timeouts(
        &self,
        record: SetActiveRunTimeoutsRecord,
    ) -> Result<Vec<PlaybookRun>, AppError>;

    /// Park a `dispatching` group only after receipt lookup affirmatively proves
    /// that the runner never accepted its dispatch identity.
    async fn requeue_started_runs(
        &self,
        record: RequeueStartedRunsRecord,
    ) -> Result<Vec<PlaybookRun>, AppError>;

    async fn complete_run(&self, record: CompleteRunRecord)
    -> Result<CompletedRunRecord, AppError>;

    async fn complete_run_with_callback(
        &self,
        auth: CallbackTokenAuth,
        record: CompleteRunRecord,
    ) -> Result<CompletedRunRecord, AppError>;

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
