-- PIP-606: source-owned reconciliation foundation.
--
-- Additive only. Nothing here removes or rewrites inventory data, and no
-- existing row receives an ownership claim: every new column carries a
-- deterministic safe default and the ownership ledger starts empty.
--
-- Durable owners introduced here:
--
--  * `mapping_configurations.absence_policy` — the typed absence policy of one
--    immutable mapping version. The default retains and reports absence, so no
--    existing version becomes destructive through this migration. A destructive
--    policy escalates work on to the remaining gates; it never deletes by itself.
--  * `sync_inventory_ownership` — one row per proved source-created native unit
--    (record, tag membership, junction member, cable attachment), with immutable
--    creation provenance, the last applied digest/version/job/mapping and a
--    lifecycle state. One active owner per physical unit holds across providers;
--    relinquished and deleted rows are retained, so a later pull cannot silently
--    re-adopt a unit whose eligibility was already lost.
--  * `sync_native_unit_lifecycle` — a monotonic per-unit generation maintained by
--    triggers on the native row and junction catalogues below. Value equality
--    cannot see a delete followed by a recreate (the local id is repeated), a
--    moved generation can, so a unit whose generation is ahead of the recorded
--    one is not a removal candidate.
--  * `sync_inventory_absence` — durable two-observation confirmation state per
--    provider, configured source generation and native unit.

-- ---------------------------------------------------------------------------
-- Immutable mapping-version absence policy
-- ---------------------------------------------------------------------------

ALTER TABLE public.mapping_configurations
    ADD COLUMN absence_policy character varying(32) NOT NULL DEFAULT 'retain_and_report';

ALTER TABLE public.mapping_configurations
    ADD CONSTRAINT mapping_configurations_absence_policy_check
        CHECK (absence_policy IN ('retain_and_report', 'delete_source_owned'));

COMMENT ON COLUMN public.mapping_configurations.absence_policy IS
    'PIP-606 typed absence policy of this immutable mapping version; retain_and_report never removes local inventory';

-- ---------------------------------------------------------------------------
-- Native unit lifecycle generation
--
-- The catalogue functions below describe the installed schema. They are the
-- single registration point inside this migration: a later schema change
-- registers its own tables through these owners in its own forward migration.
-- This file is never replayed on an installed database.
-- ---------------------------------------------------------------------------

CREATE TABLE public.sync_native_unit_lifecycle (
    native_table character varying(63) NOT NULL,
    native_unit_key text NOT NULL,
    generation bigint DEFAULT 1 NOT NULL,
    last_event character varying(8) NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT sync_native_unit_lifecycle_pkey PRIMARY KEY (native_table, native_unit_key),
    CONSTRAINT sync_native_unit_lifecycle_generation_check CHECK (generation >= 1),
    CONSTRAINT sync_native_unit_lifecycle_event_check
        CHECK (last_event IN ('inserted', 'updated', 'moved', 'deleted'))
);

-- Native junctions: an inventory table whose identity is a unique pair of uuid
-- foreign keys. A surplus `id uuid` surrogate is allowed and ignored, because the
-- membership is the pair and the surrogate can be recreated; any other column
-- means the row owns payload and is not a pure membership.
CREATE FUNCTION public.sync_native_junction_tables()
RETURNS TABLE(table_name text, first_column text, second_column text)
LANGUAGE sql STABLE AS $$
    WITH membership_pair AS (
        SELECT constraint_row.conrelid, constraint_row.conkey
        FROM pg_constraint constraint_row
        WHERE constraint_row.contype IN ('p', 'u')
          AND array_length(constraint_row.conkey, 1) = 2
          AND NOT EXISTS (
              SELECT 1
              FROM unnest(constraint_row.conkey) AS key_column
              WHERE NOT EXISTS (
                  SELECT 1
                  FROM pg_constraint foreign_key
                  WHERE foreign_key.conrelid = constraint_row.conrelid
                    AND foreign_key.contype = 'f'
                    AND key_column = ANY (foreign_key.conkey)
              )
          )
    ), eligible AS (
        SELECT relation.oid, relation.relname::text AS table_name
        FROM pg_class relation
        JOIN pg_namespace namespace ON namespace.oid = relation.relnamespace
        WHERE namespace.nspname = 'public'
          AND relation.relkind IN ('r', 'p')
          AND relation.relname ~ '^(circuits|dcim|extras|ipam|tenancy|virtualization|vpn|wireless|inventory|custom_field)_'
          AND EXISTS (
              SELECT 1 FROM membership_pair WHERE membership_pair.conrelid = relation.oid
          )
          AND NOT EXISTS (
              SELECT 1
              FROM pg_attribute attribute
              WHERE attribute.attrelid = relation.oid
                AND attribute.attnum > 0
                AND NOT attribute.attisdropped
                AND attribute.atttypid <> 'uuid'::regtype
          )
          AND NOT EXISTS (
              SELECT 1
              FROM pg_attribute attribute
              WHERE attribute.attrelid = relation.oid
                AND attribute.attnum > 0
                AND NOT attribute.attisdropped
                AND attribute.attname <> 'id'
                AND NOT EXISTS (
                    SELECT 1
                    FROM membership_pair
                    JOIN unnest(membership_pair.conkey) AS key_column ON true
                    WHERE membership_pair.conrelid = relation.oid
                      AND key_column = attribute.attnum
                )
          )
    )
    SELECT eligible.table_name,
           (SELECT min(attribute.attname::text) FROM pg_attribute attribute
            WHERE attribute.attrelid = eligible.oid AND attribute.attnum > 0
              AND NOT attribute.attisdropped AND attribute.attname <> 'id'),
           (SELECT max(attribute.attname::text) FROM pg_attribute attribute
            WHERE attribute.attrelid = eligible.oid AND attribute.attnum > 0
              AND NOT attribute.attisdropped AND attribute.attname <> 'id')
    FROM eligible
    ORDER BY 1
$$;

-- Native records: an inventory table with a single-column uuid identity that is
-- not a membership pair. A physical row is registered under exactly one kind.
CREATE FUNCTION public.sync_native_row_tables() RETURNS TABLE(table_name text)
LANGUAGE sql STABLE AS $$
    SELECT DISTINCT relation.relname::text
    FROM pg_class relation
    JOIN pg_namespace namespace ON namespace.oid = relation.relnamespace
    JOIN pg_attribute identity ON identity.attrelid = relation.oid
    WHERE namespace.nspname = 'public'
      AND relation.relkind IN ('r', 'p')
      AND relation.relname ~ '^(circuits|dcim|extras|ipam|tenancy|virtualization|vpn|wireless|inventory|custom_field)_'
      AND identity.attname = 'id'
      AND identity.atttypid = 'uuid'::regtype
      AND NOT identity.attisdropped
      AND relation.relname NOT IN (
          SELECT junction.table_name FROM public.sync_native_junction_tables() AS junction
      )
    ORDER BY 1
$$;

-- One generation step per native unit change. A delete and a recreate move the
-- generation twice, so a proof minted before them cannot be refreshed by a
-- writer that compares values only.
CREATE FUNCTION public.sync_native_unit_bump(
    subject_table text,
    subject_key text,
    event_name text
) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO public.sync_native_unit_lifecycle AS lifecycle
        (native_table, native_unit_key, generation, last_event, updated_at)
    VALUES (subject_table, subject_key, 1, event_name, now())
    ON CONFLICT (native_table, native_unit_key) DO UPDATE
        SET generation = lifecycle.generation + 1,
            last_event = EXCLUDED.last_event,
            updated_at = EXCLUDED.updated_at;
END
$$;

CREATE FUNCTION public.sync_native_unit_track_row() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        PERFORM public.sync_native_unit_bump(TG_TABLE_NAME, OLD.id::text, 'deleted');
    ELSIF TG_OP = 'INSERT' THEN
        PERFORM public.sync_native_unit_bump(TG_TABLE_NAME, NEW.id::text, 'inserted');
    ELSE
        PERFORM public.sync_native_unit_bump(TG_TABLE_NAME, NEW.id::text, 'updated');
    END IF;
    RETURN NULL;
END
$$;

-- A junction row carries no trustworthy surrogate identity: the two uuid columns
-- name the membership. Their catalogue order is stable per table, and an UPDATE
-- that moves the keys retires the old pair as well as recording the new one.
CREATE FUNCTION public.sync_native_unit_track_junction() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    pair_key text;
BEGIN
    IF TG_OP = 'DELETE' THEN
        pair_key := coalesce(to_jsonb(OLD) ->> TG_ARGV[0], '') || '/' || coalesce(to_jsonb(OLD) ->> TG_ARGV[1], '');
        PERFORM public.sync_native_unit_bump(TG_TABLE_NAME, pair_key, 'deleted');
        RETURN NULL;
    END IF;
    IF TG_OP = 'UPDATE' THEN
        pair_key := coalesce(to_jsonb(OLD) ->> TG_ARGV[0], '') || '/' || coalesce(to_jsonb(OLD) ->> TG_ARGV[1], '');
        PERFORM public.sync_native_unit_bump(TG_TABLE_NAME, pair_key, 'moved');
    END IF;
    pair_key := coalesce(to_jsonb(NEW) ->> TG_ARGV[0], '') || '/' || coalesce(to_jsonb(NEW) ->> TG_ARGV[1], '');
    PERFORM public.sync_native_unit_bump(
        TG_TABLE_NAME, pair_key, CASE TG_OP WHEN 'INSERT' THEN 'inserted' ELSE 'moved' END);
    RETURN NULL;
END
$$;

DO $$
DECLARE subject record;
BEGIN
    FOR subject IN SELECT * FROM public.sync_native_row_tables() LOOP
        EXECUTE format(
            'CREATE TRIGGER sync_unit_lifecycle AFTER INSERT OR UPDATE OR DELETE ON public.%I FOR EACH ROW EXECUTE FUNCTION public.sync_native_unit_track_row()',
            subject.table_name);
    END LOOP;
    FOR subject IN SELECT * FROM public.sync_native_junction_tables() LOOP
        EXECUTE format(
            'CREATE TRIGGER sync_unit_lifecycle AFTER INSERT OR UPDATE OR DELETE ON public.%I FOR EACH ROW EXECUTE FUNCTION public.sync_native_unit_track_junction(%L, %L)',
            subject.table_name, subject.first_column, subject.second_column);
    END LOOP;
END
$$;

COMMENT ON TABLE public.sync_native_unit_lifecycle IS
    'PIP-606 monotonic per-native-unit generation maintained by the sync_unit_lifecycle triggers';

-- ---------------------------------------------------------------------------
-- Source ownership ledger
-- ---------------------------------------------------------------------------

CREATE TABLE public.sync_inventory_ownership (
    id uuid DEFAULT uuidv7() NOT NULL,
    unit_kind character varying(24) NOT NULL,
    -- Native identity: the owning table and row, plus the member and its native
    -- qualifier for membership units. Never a source id, name or payload value.
    native_table character varying(63) NOT NULL,
    native_id uuid NOT NULL,
    member_id uuid DEFAULT '00000000-0000-0000-0000-000000000000'::uuid NOT NULL,
    -- Native qualifier of the member: the cable end for a cable attachment, empty
    -- for a tag membership or a junction member, whose member is already unique.
    member_token character varying(16) DEFAULT ''::character varying NOT NULL,
    state character varying(16) DEFAULT 'active' NOT NULL,
    -- Immutable creation provenance: which provider/source record the creator
    -- observed absent from its own transaction, and which job and mapping
    -- version created the unit.
    source_provider_id uuid NOT NULL,
    source_entity character varying(128) NOT NULL,
    external_id character varying(256) NOT NULL,
    created_job_id uuid NOT NULL,
    created_mapping_logical_id uuid NOT NULL,
    created_mapping_version_id integer NOT NULL,
    created_at timestamp with time zone NOT NULL,
    -- Last applied state. Only a writer that verified this proof intact before
    -- its own writes may refresh these columns.
    last_applied_job_id uuid NOT NULL,
    last_applied_mapping_logical_id uuid NOT NULL,
    last_applied_mapping_version_id integer NOT NULL,
    last_applied_digest_version smallint NOT NULL,
    last_applied_digest character varying(64) NOT NULL,
    last_applied_lifecycle_generation bigint NOT NULL,
    last_applied_at timestamp with time zone NOT NULL,
    relinquished_at timestamp with time zone,
    relinquished_reason character varying(32),
    CONSTRAINT sync_inventory_ownership_pkey PRIMARY KEY (id),
    CONSTRAINT sync_inventory_ownership_unit_kind_check
        CHECK (unit_kind IN ('record', 'tag_membership', 'junction_member', 'cable_attachment')),
    CONSTRAINT sync_inventory_ownership_native_table_check
        CHECK (native_table ~ '^[a-z][a-z0-9_]{0,62}$'),
    CONSTRAINT sync_inventory_ownership_member_check
        CHECK (CASE unit_kind
                   WHEN 'record' THEN member_id = '00000000-0000-0000-0000-000000000000'::uuid
                                       AND member_token = ''
                   WHEN 'tag_membership' THEN member_id <> '00000000-0000-0000-0000-000000000000'::uuid
                                              AND member_token = ''
                   WHEN 'junction_member' THEN member_id <> '00000000-0000-0000-0000-000000000000'::uuid
                                               AND member_token = ''
                   WHEN 'cable_attachment' THEN member_id <> '00000000-0000-0000-0000-000000000000'::uuid
                                                AND member_token IN ('A', 'B')
               END),
    CONSTRAINT sync_inventory_ownership_state_check
        CHECK (state IN ('active', 'relinquished', 'deleted')),
    CONSTRAINT sync_inventory_ownership_digest_check
        CHECK (last_applied_digest ~ '^[0-9a-f]{64}$'),
    CONSTRAINT sync_inventory_ownership_digest_version_check
        CHECK (last_applied_digest_version = 1),
    CONSTRAINT sync_inventory_ownership_generation_check
        CHECK (last_applied_lifecycle_generation >= 1),
    CONSTRAINT sync_inventory_ownership_mapping_version_check
        CHECK (created_mapping_version_id > 0 AND last_applied_mapping_version_id > 0),
    CONSTRAINT sync_inventory_ownership_relinquish_reason_check
        CHECK (relinquished_reason IS NULL
               OR relinquished_reason IN ('locally_modified', 'lifecycle_invalidated',
                                          'owner_conflict', 'source_deleted')),
    CONSTRAINT sync_inventory_ownership_state_reason_check
        CHECK ((state = 'active') = (relinquished_reason IS NULL)),
    CONSTRAINT sync_inventory_ownership_state_time_check
        CHECK ((relinquished_reason IS NULL) = (relinquished_at IS NULL))
);

-- One active owner per physical unit, independent of the owning provider.
CREATE UNIQUE INDEX sync_inventory_ownership_active_unit
    ON public.sync_inventory_ownership
        (unit_kind, native_table, native_id, member_id, member_token)
    WHERE state = 'active';
CREATE INDEX sync_inventory_ownership_source_idx
    ON public.sync_inventory_ownership (source_provider_id, source_entity, external_id);
CREATE INDEX sync_inventory_ownership_native_idx
    ON public.sync_inventory_ownership (native_table, native_id);

-- Creation provenance is immutable, a mint is always active, and neither a
-- relinquished nor a deleted row becomes active or refreshed again. Retention is
-- the invariant: deleting provenance would let a later pull re-adopt the unit.
CREATE FUNCTION public.sync_inventory_ownership_guard() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'ownership provenance is retained and cannot be deleted' USING ERRCODE = '23514';
    END IF;
    IF TG_OP = 'INSERT' THEN
        IF NEW.state <> 'active' THEN
            RAISE EXCEPTION 'ownership is only minted as an active claim' USING ERRCODE = '23514';
        END IF;
        RETURN NEW;
    END IF;
    IF NEW.id <> OLD.id
       OR NEW.unit_kind <> OLD.unit_kind
       OR NEW.native_table <> OLD.native_table
       OR NEW.native_id <> OLD.native_id
       OR NEW.member_id <> OLD.member_id
       OR NEW.member_token <> OLD.member_token
       OR NEW.source_provider_id <> OLD.source_provider_id
       OR NEW.source_entity <> OLD.source_entity
       OR NEW.external_id <> OLD.external_id
       OR NEW.created_job_id <> OLD.created_job_id
       OR NEW.created_mapping_logical_id <> OLD.created_mapping_logical_id
       OR NEW.created_mapping_version_id <> OLD.created_mapping_version_id
       OR NEW.created_at <> OLD.created_at THEN
        RAISE EXCEPTION 'ownership creation provenance is immutable' USING ERRCODE = '23514';
    END IF;
    IF OLD.state <> 'active' AND NEW.state = 'active' THEN
        RAISE EXCEPTION 'a relinquished or deleted ownership cannot become active again' USING ERRCODE = '23514';
    END IF;
    IF OLD.state <> 'active'
       AND (NEW.last_applied_digest IS DISTINCT FROM OLD.last_applied_digest
            OR NEW.last_applied_digest_version IS DISTINCT FROM OLD.last_applied_digest_version
            OR NEW.last_applied_lifecycle_generation IS DISTINCT FROM OLD.last_applied_lifecycle_generation
            OR NEW.last_applied_job_id IS DISTINCT FROM OLD.last_applied_job_id
            OR NEW.last_applied_mapping_logical_id IS DISTINCT FROM OLD.last_applied_mapping_logical_id
            OR NEW.last_applied_mapping_version_id IS DISTINCT FROM OLD.last_applied_mapping_version_id
            OR NEW.last_applied_at IS DISTINCT FROM OLD.last_applied_at) THEN
        RAISE EXCEPTION 'a relinquished or deleted ownership cannot be refreshed' USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END
$$;

CREATE TRIGGER ownership_guard
    BEFORE INSERT OR UPDATE OR DELETE ON public.sync_inventory_ownership
    FOR EACH ROW EXECUTE FUNCTION public.sync_inventory_ownership_guard();

COMMENT ON TABLE public.sync_inventory_ownership IS
    'PIP-606 source-created ownership proof per native unit; retained after relinquish, one active owner per unit';

-- ---------------------------------------------------------------------------
-- Two-observation absence confirmation
-- ---------------------------------------------------------------------------

CREATE TABLE public.sync_inventory_absence (
    provider_id uuid NOT NULL,
    unit_kind character varying(24) NOT NULL,
    native_table character varying(63) NOT NULL,
    native_id uuid NOT NULL,
    member_id uuid DEFAULT '00000000-0000-0000-0000-000000000000'::uuid NOT NULL,
    member_token character varying(16) DEFAULT ''::character varying NOT NULL,
    -- Canonical identity of the configured source generation the absence was
    -- observed under: origin, immutable mapping version, mapping digest, policy
    -- and credential revision. A different generation starts a new count.
    source_generation character varying(64) NOT NULL,
    -- The source record the absence is about; kept for reporting and for
    -- clearing the row when the unit is observed again.
    source_entity character varying(128) NOT NULL,
    external_id character varying(256) NOT NULL,
    observation_count smallint DEFAULT 1 NOT NULL,
    first_observed_at timestamp with time zone NOT NULL,
    first_observed_job_id uuid NOT NULL,
    last_observed_at timestamp with time zone NOT NULL,
    last_observed_job_id uuid NOT NULL,
    confirmed_at timestamp with time zone,
    CONSTRAINT sync_inventory_absence_pkey
        PRIMARY KEY (provider_id, unit_kind, native_table, native_id, member_id, member_token),
    CONSTRAINT sync_inventory_absence_unit_kind_check
        CHECK (unit_kind IN ('record', 'tag_membership', 'junction_member', 'cable_attachment')),
    CONSTRAINT sync_inventory_absence_member_check
        CHECK (CASE unit_kind
                   WHEN 'record' THEN member_id = '00000000-0000-0000-0000-000000000000'::uuid
                                       AND member_token = ''
                   WHEN 'tag_membership' THEN member_id <> '00000000-0000-0000-0000-000000000000'::uuid
                                              AND member_token = ''
                   WHEN 'junction_member' THEN member_id <> '00000000-0000-0000-0000-000000000000'::uuid
                                               AND member_token = ''
                   WHEN 'cable_attachment' THEN member_id <> '00000000-0000-0000-0000-000000000000'::uuid
                                                AND member_token IN ('A', 'B')
               END),
    CONSTRAINT sync_inventory_absence_generation_check
        CHECK (source_generation ~ '^[0-9a-f]{64}$'),
    CONSTRAINT sync_inventory_absence_count_check
        CHECK (observation_count BETWEEN 1 AND 2),
    CONSTRAINT sync_inventory_absence_confirmation_check
        CHECK ((observation_count = 2) = (confirmed_at IS NOT NULL)),
    CONSTRAINT sync_inventory_absence_job_check
        CHECK (first_observed_job_id <> last_observed_job_id OR observation_count = 1)
);

CREATE INDEX sync_inventory_absence_source_idx
    ON public.sync_inventory_absence (provider_id, source_entity, external_id);

COMMENT ON TABLE public.sync_inventory_absence IS
    'PIP-606 durable two-observation absence confirmation; a single pending observation never authorises removal';
