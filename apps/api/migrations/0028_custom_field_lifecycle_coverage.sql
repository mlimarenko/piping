-- PIP-606: lifecycle coverage for the custom-field definition owner.
--
-- Forward correction of 0027, which is a committed and native-applied
-- checkpoint: it is not replayed or edited here. 0027's namespace rule required
-- a trailing underscore (`^(…|custom_field)_`), so the definition table
-- `custom_field` itself received no lifecycle hook while its membership table
-- `custom_field_object_type` did. The qualified target catalogue
-- (`InventoryEntity::CustomField` → `custom_field`) therefore could never hold a
-- proof.
--
-- Registration stays with the canonical catalogue functions: `sync_native_row_tables()`
-- is redefined to include the exact custom-field definition owner table, and the
-- trigger is installed only where it is missing. An existing `sync_unit_lifecycle`
-- trigger that does not use this migration's tracker functions is never adopted;
-- the migration refuses instead of taking over an unknown definition.

DO $$
DECLARE foreign_trigger text;
BEGIN
    IF to_regprocedure('public.sync_native_unit_track_row()') IS NULL
       OR to_regprocedure('public.sync_native_unit_track_junction()') IS NULL
       OR to_regprocedure('public.sync_native_unit_bump(text,text,text)') IS NULL THEN
        RAISE EXCEPTION 'PIP-606 lifecycle trigger owners are missing; 0027 must be applied first'
            USING ERRCODE = '42P01';
    END IF;
    SELECT format('%s on %s', trigger.tgname, trigger.tgrelid::regclass)
      INTO foreign_trigger
      FROM pg_trigger AS trigger
     WHERE trigger.tgname = 'sync_unit_lifecycle'
       AND NOT trigger.tgisinternal
       AND trigger.tgfoid NOT IN (
           'public.sync_native_unit_track_row()'::regprocedure,
           'public.sync_native_unit_track_junction()'::regprocedure)
     LIMIT 1;
    IF foreign_trigger IS NOT NULL THEN
        RAISE EXCEPTION 'unknown sync_unit_lifecycle definition is installed: %', foreign_trigger
            USING ERRCODE = '42710';
    END IF;
    IF to_regclass('public.custom_field') IS NULL THEN
        RAISE EXCEPTION 'the custom-field definition owner table is missing'
            USING ERRCODE = '42P01';
    END IF;
END
$$;

-- Same catalogue the triggers are registered from, now naming the definition
-- owner table exactly. The underscore-prefixed namespaces keep covering every
-- other native record; only the bare definition table needed the explicit name.
CREATE OR REPLACE FUNCTION public.sync_native_row_tables() RETURNS TABLE(table_name text)
LANGUAGE sql STABLE AS $$
    SELECT DISTINCT relation.relname::text
    FROM pg_class relation
    JOIN pg_namespace namespace ON namespace.oid = relation.relnamespace
    JOIN pg_attribute identity ON identity.attrelid = relation.oid
    WHERE namespace.nspname = 'public'
      AND relation.relkind IN ('r', 'p')
      AND (
          relation.relname ~ '^(circuits|dcim|extras|ipam|tenancy|virtualization|vpn|wireless|inventory|custom_field)_'
          OR relation.relname = 'custom_field'
      )
      AND identity.attname = 'id'
      AND identity.atttypid = 'uuid'::regtype
      AND NOT identity.attisdropped
      AND relation.relname NOT IN (
          SELECT junction.table_name FROM public.sync_native_junction_tables() AS junction
      )
    ORDER BY 1
$$;

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_trigger AS trigger
        WHERE trigger.tgname = 'sync_unit_lifecycle'
          AND trigger.tgrelid = 'public.custom_field'::regclass
          AND trigger.tgfoid = 'public.sync_native_unit_track_row()'::regprocedure
    ) THEN
        IF EXISTS (
            SELECT 1 FROM pg_trigger AS trigger
            WHERE trigger.tgname = 'sync_unit_lifecycle'
              AND trigger.tgrelid = 'public.custom_field'::regclass
        ) THEN
            RAISE EXCEPTION 'custom_field carries an unexpected sync_unit_lifecycle trigger'
                USING ERRCODE = '42710';
        END IF;
        CREATE TRIGGER sync_unit_lifecycle
            AFTER INSERT OR UPDATE OR DELETE ON public.custom_field
            FOR EACH ROW EXECUTE FUNCTION public.sync_native_unit_track_row();
    END IF;
END
$$;

COMMENT ON FUNCTION public.sync_native_row_tables() IS
    'PIP-606 native record catalogue for lifecycle registration; includes the custom-field definition owner table';
