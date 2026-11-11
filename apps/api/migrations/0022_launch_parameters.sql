-- One canonical parameter contract per job template, plus the run/schedule
-- overrides that resolve on top of it. The declaration is the existing JSON
-- Schema document; there is no second vocabulary and no compatibility read.
ALTER TABLE playbook_job_template
    ADD COLUMN vars_schema jsonb NOT NULL DEFAULT '{"kind":"none"}'::jsonb,
    ADD CONSTRAINT chk_playbook_job_template_vars_schema CHECK (
        (vars_schema->>'kind' = 'none' AND vars_schema = '{"kind":"none"}'::jsonb)
        OR (vars_schema->>'kind' = 'inline'
            AND jsonb_typeof(vars_schema->'schema') = 'object')
        OR (vars_schema->>'kind' = 'project_file'
            AND length(vars_schema->>'path') > 0)
    );

-- Stored launch overrides are safe values only. Secret declarations resolve to
-- UUID references in options.secret_refs; a plaintext secret never reaches this
-- column because the service rejects it before the write.
ALTER TABLE playbook_schedule
    ADD COLUMN overrides jsonb NOT NULL DEFAULT '{"extra_vars":{}}'::jsonb,
    ADD CONSTRAINT chk_playbook_schedule_overrides CHECK (
        jsonb_typeof(overrides) = 'object'
        AND jsonb_typeof(overrides->'extra_vars') = 'object'
    );
