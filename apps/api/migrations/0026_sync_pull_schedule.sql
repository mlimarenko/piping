CREATE TABLE sync_pull_schedule (
    id uuid PRIMARY KEY,
    provider_id uuid NOT NULL,
    mapping_logical_id uuid NOT NULL,
    mapping_version_id integer NOT NULL CHECK (mapping_version_id > 0),
    principal_method text NOT NULL CHECK (principal_method IN ('session', 'oidc', 'api_token')),
    principal_user_id uuid NOT NULL,
    principal_token_id uuid,
    cron_expression text NOT NULL CHECK (char_length(cron_expression) BETWEEN 9 AND 200),
    timezone text NOT NULL CHECK (char_length(timezone) BETWEEN 1 AND 120),
    enabled boolean NOT NULL DEFAULT true,
    cancelled_at timestamptz,
    next_run_at timestamptz,
    last_run_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    CHECK ((principal_method = 'api_token') = (principal_token_id IS NOT NULL)),
    CHECK (cancelled_at IS NULL OR NOT enabled),
    CHECK (NOT enabled OR next_run_at IS NOT NULL),
    CHECK (last_run_at IS NULL OR next_run_at IS NULL OR next_run_at > last_run_at)
);

CREATE INDEX sync_pull_schedule_due ON sync_pull_schedule(next_run_at, id) WHERE enabled;

CREATE TABLE sync_pull_schedule_occurrence (
    schedule_id uuid NOT NULL REFERENCES sync_pull_schedule(id) ON DELETE RESTRICT,
    scheduled_for timestamptz NOT NULL,
    status text NOT NULL CHECK (status IN ('claimed', 'running', 'completed', 'failed', 'skipped')),
    lease_id uuid,
    lease_expires_at timestamptz,
    job_id uuid UNIQUE,
    result_code text CHECK (result_code IS NULL OR char_length(result_code) BETWEEN 1 AND 100),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (schedule_id, scheduled_for),
    CHECK ((lease_id IS NULL) = (lease_expires_at IS NULL)),
    CHECK (status <> 'running' OR job_id IS NOT NULL),
    CHECK (status NOT IN ('claimed', 'skipped') OR job_id IS NULL),
    CHECK (status <> 'completed' OR job_id IS NOT NULL),
    CHECK (status NOT IN ('completed', 'failed', 'skipped') OR result_code IS NOT NULL),
    CHECK (status NOT IN ('completed', 'failed', 'skipped') OR lease_id IS NULL)
);

CREATE INDEX sync_pull_occurrence_expired ON sync_pull_schedule_occurrence(lease_expires_at)
    WHERE status IN ('claimed', 'running');
