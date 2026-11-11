-- PIP-530: a schedule runs as its owner, never as a service principal.
--
-- Lock first so the archive, the pause decision and the column drop cannot
-- race a concurrent schedule writer.
LOCK TABLE playbook_schedule IN ACCESS EXCLUSIVE MODE;

-- The retired executor identity stays readable on the row: the column is about
-- to disappear, so archive its value in metadata before the drop. Service
-- principal identifiers on historical run and run-group rows stay untouched.
UPDATE playbook_schedule
SET metadata = metadata || jsonb_build_object(
    'migrated_from_service_principal_id', service_principal_id::text
);

-- Ambiguous ownership is paused, never reassigned or deleted: a schedule whose
-- delegated owner is missing or inactive stops firing and keeps the reason on
-- the row so the pause is visible instead of silent.
UPDATE playbook_schedule AS schedule
SET enabled = false,
    metadata = schedule.metadata || jsonb_build_object(
        'owner_migration_paused', true,
        'owner_migration_pause_reason', 'owner_missing_or_inactive'
    )
WHERE NOT EXISTS (
    SELECT 1
    FROM identity_user AS owner
    WHERE owner.id = schedule.delegated_by_user_id
      AND owner.status = 'active'
);

ALTER TABLE playbook_schedule
    DROP CONSTRAINT playbook_schedule_service_principal_id_fkey,
    DROP COLUMN service_principal_id;
ALTER TABLE playbook_schedule RENAME COLUMN delegated_by_user_id TO owner_user_id;
ALTER TABLE playbook_schedule RENAME CONSTRAINT playbook_schedule_delegated_by_user_id_fkey TO playbook_schedule_owner_user_id_fkey;
ALTER TABLE playbook_schedule RENAME CONSTRAINT playbook_schedule_delegated_by_user_id_not_null TO playbook_schedule_owner_user_id_not_null;
