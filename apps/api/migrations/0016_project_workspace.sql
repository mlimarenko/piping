-- The previous launch model has no lossless automatic mapping. Lock first so
-- the emptiness check and schema change cannot race an old application writer.
LOCK TABLE playbook, playbook_version, playbook_assignment, playbook_project,
    playbook_job_template, playbook_job_template_survey, playbook_run_group,
    playbook_run, playbook_run_event, playbook_artifact, playbook_job_token,
    playbook_run_group_callback_token, playbook_run_group_event_batch,
    playbook_schedule, playbook_schedule_occurrence IN ACCESS EXCLUSIVE MODE;
LOCK TABLE secret_usage_binding IN SHARE ROW EXCLUSIVE MODE;

DO $$
DECLARE table_name text; has_rows boolean;
BEGIN
    FOREACH table_name IN ARRAY ARRAY[
        'playbook', 'playbook_version', 'playbook_assignment', 'playbook_project',
        'playbook_job_template', 'playbook_job_template_survey', 'playbook_run_group',
        'playbook_run', 'playbook_run_event', 'playbook_artifact', 'playbook_job_token',
        'playbook_run_group_callback_token', 'playbook_run_group_event_batch',
        'playbook_schedule', 'playbook_schedule_occurrence'
    ] LOOP
        EXECUTE format('SELECT EXISTS (SELECT 1 FROM public.%I)', table_name) INTO has_rows;
        IF has_rows THEN
            RAISE EXCEPTION 'PIP-194 refuses nonempty retired playbook domain: %', table_name;
        END IF;
    END LOOP;
    IF EXISTS (
        SELECT 1 FROM secret_usage_binding
        WHERE consumer_type IN ('playbook_version:runtime_env', 'playbook_project')
    ) THEN
        RAISE EXCEPTION 'PIP-194 refuses retired project/version secret usage bindings';
    END IF;
END $$;

ALTER TABLE playbook_run DROP COLUMN playbook_id, DROP COLUMN playbook_version_id,
    ALTER COLUMN target_kind DROP NOT NULL, ALTER COLUMN target_id DROP NOT NULL,
    ALTER COLUMN source_kind DROP NOT NULL, ALTER COLUMN source_id DROP NOT NULL,
    ADD COLUMN hostname text NOT NULL CHECK (hostname <> ''),
    ADD CONSTRAINT run_group_hostname_attempt_unique UNIQUE (run_group_id, hostname, attempt);
-- A run whose host outcome the native callback never reports ends as
-- `unreported`; the sealed check must accept that terminal value.
ALTER TABLE playbook_run DROP CONSTRAINT chk_playbook_run_status,
    ADD CONSTRAINT chk_playbook_run_status CHECK ((status = ANY (ARRAY[
        'queued'::text, 'leased'::text, 'dispatching'::text, 'running'::text,
        'succeeded'::text, 'failed'::text, 'canceled'::text, 'timed_out'::text,
        'unreported'::text])));
ALTER TABLE playbook_run_group DROP COLUMN playbook_id, DROP COLUMN playbook_version_id,
    ADD COLUMN template_id uuid NOT NULL REFERENCES playbook_job_template(id) ON DELETE RESTRICT,
    ADD COLUMN launch_spec jsonb NOT NULL CHECK (jsonb_typeof(launch_spec) = 'object');
ALTER TABLE playbook_schedule DROP COLUMN playbook_id, DROP COLUMN target_set_id,
    ADD COLUMN template_id uuid NOT NULL REFERENCES playbook_job_template(id) ON DELETE RESTRICT,
    ADD COLUMN targets jsonb NOT NULL CHECK (jsonb_typeof(targets) = 'object'),
    ADD COLUMN delegated_by_user_id uuid NOT NULL REFERENCES identity_user(id);

ALTER TABLE playbook_assignment DROP COLUMN playbook_id, DROP COLUMN default_version_id,
    ADD COLUMN job_template_id uuid NOT NULL REFERENCES playbook_job_template(id) ON DELETE CASCADE;
ALTER TABLE playbook_job_template DROP COLUMN playbook_id, DROP COLUMN playbook_path,
    DROP COLUMN inventory_target_set_id, DROP COLUMN credential_refs,
    DROP COLUMN extra_vars, DROP COLUMN launch_options,
    DROP CONSTRAINT fk_playbook_job_template_project,
    DROP CONSTRAINT fk_playbook_job_template_ee,
    ALTER COLUMN project_id SET NOT NULL, ALTER COLUMN execution_environment_id SET NOT NULL,
    ADD COLUMN workdir text NOT NULL, ADD COLUMN playbook_paths text[] NOT NULL,
    ADD COLUMN revision_selection jsonb NOT NULL, ADD COLUMN inventory jsonb NOT NULL,
    ADD COLUMN options jsonb NOT NULL,
    ADD CONSTRAINT template_project_fk FOREIGN KEY (project_id) REFERENCES playbook_project(id) ON DELETE RESTRICT,
    ADD CONSTRAINT template_environment_fk FOREIGN KEY (execution_environment_id) REFERENCES playbook_execution_environment(id) ON DELETE RESTRICT;
ALTER TABLE playbook DROP COLUMN current_version_id;
DROP TABLE playbook_version;
DROP TABLE playbook;

ALTER TABLE playbook_project DROP COLUMN scm_type, DROP COLUMN scm_url,
    DROP COLUMN scm_branch, DROP COLUMN scm_ref, DROP COLUMN playbook_base_path,
    DROP COLUMN default_execution_environment_id, DROP COLUMN sync_policy,
    DROP COLUMN sync_status, DROP COLUMN last_sync_revision,
    DROP COLUMN last_sync_error, DROP COLUMN last_synced_at,
    ADD COLUMN source jsonb NOT NULL CHECK (jsonb_typeof(source) = 'object'),
    ADD COLUMN current_revision_id uuid;
CREATE TABLE playbook_project_revision (
    revision_id uuid PRIMARY KEY, project_id uuid NOT NULL REFERENCES playbook_project(id) ON DELETE CASCADE,
    tree_sha256 text NOT NULL CHECK (tree_sha256 ~ '^[0-9a-f]{64}$'),
    source_commit_oid text, created_by uuid, created_at timestamptz NOT NULL DEFAULT now(),
    entries jsonb NOT NULL CHECK (jsonb_typeof(entries) = 'array'),
    UNIQUE (project_id, revision_id)
);
ALTER TABLE playbook_project ADD CONSTRAINT project_current_revision_fk
    FOREIGN KEY (id, current_revision_id)
    REFERENCES playbook_project_revision(project_id, revision_id);
CREATE FUNCTION reject_project_revision_mutation() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' AND pg_trigger_depth() > 1 THEN
        RETURN OLD;
    END IF;
    RAISE EXCEPTION 'project revisions are immutable';
END $$;
CREATE TRIGGER project_revision_immutable BEFORE UPDATE OR DELETE
    ON playbook_project_revision FOR EACH ROW EXECUTE FUNCTION reject_project_revision_mutation();

CREATE FUNCTION reject_run_launch_mutation() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.launch_spec IS DISTINCT FROM OLD.launch_spec OR NEW.template_id <> OLD.template_id THEN
        RAISE EXCEPTION 'resolved run launches are immutable';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER run_group_launch_immutable BEFORE UPDATE OF launch_spec, template_id
    ON playbook_run_group FOR EACH ROW EXECUTE FUNCTION reject_run_launch_mutation();

-- Durable supervisor console: a contiguous byte stream independent of derived
-- Ansible events. The group cursor is the next missing sequence; sealing sets
-- the final prefix and forbids later chunks.
ALTER TABLE playbook_run_group
    ADD COLUMN console_next_sequence bigint NOT NULL DEFAULT 0,
    ADD COLUMN console_final_sequence bigint,
    ADD CONSTRAINT chk_playbook_run_group_console_next_sequence
        CHECK (console_next_sequence BETWEEN 0 AND 9007199254740991),
    ADD CONSTRAINT chk_playbook_run_group_console_final_sequence
        CHECK (console_final_sequence IS NULL
               OR console_final_sequence BETWEEN 0 AND 9007199254740991),
    ADD CONSTRAINT chk_playbook_run_group_console_final_prefix
        CHECK (console_final_sequence IS NULL OR console_final_sequence = console_next_sequence);

-- Counts the group's runs whose host outcome the native callback never reported.
ALTER TABLE playbook_run_group ADD COLUMN unreported_runs integer NOT NULL DEFAULT 0;

CREATE TABLE playbook_run_console_chunk (
    run_group_id uuid NOT NULL REFERENCES playbook_run_group(id) ON DELETE CASCADE,
    dispatch_id uuid NOT NULL,
    sequence bigint NOT NULL CHECK (sequence >= 0),
    stream text NOT NULL CHECK (stream IN ('stdout', 'stderr')),
    bytes bytea NOT NULL CHECK (octet_length(bytes) BETWEEN 1 AND 65536),
    created_at timestamp with time zone NOT NULL DEFAULT now(),
    PRIMARY KEY (run_group_id, dispatch_id, sequence)
);

