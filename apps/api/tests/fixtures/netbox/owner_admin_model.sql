-- NetBox v4.7.1 abccf4e03650fecd366e676b392729320258c49e:
-- users/models/owners.py inherits netbox/models/__init__.py AdminModel,
-- not NetBoxModel or ChangeLoggedModel. Test both siblings as one class.
DO $$
DECLARE
    target text;
    actual text[];
    expected text[];
BEGIN
    FOREACH target IN ARRAY ARRAY['users_owner', 'users_ownergroup'] LOOP
        IF EXISTS (
            SELECT 1 FROM information_schema.columns
            WHERE table_schema = 'public' AND table_name = target
              AND column_name IN ('tags', 'custom_field_data', 'created', 'last_updated')
        ) THEN
            RAISE EXCEPTION 'AdminModel forbidden metadata columns on %', target;
        END IF;
        SELECT array_agg(column_name::text ORDER BY column_name) INTO actual
        FROM information_schema.columns
        WHERE table_schema = 'public' AND table_name = target;
        expected := CASE target
            WHEN 'users_owner' THEN ARRAY['description', 'group_id', 'id', 'name']
            ELSE ARRAY['description', 'id', 'name'] END;
        IF actual IS DISTINCT FROM expected THEN
            RAISE EXCEPTION 'AdminModel concrete columns mismatch on %', target;
        END IF;
        IF NOT EXISTS (
            SELECT 1 FROM information_schema.columns
            WHERE table_schema = 'public' AND table_name = target
              AND column_name = 'description' AND character_maximum_length = 200
              AND is_nullable = 'NO'
        ) OR NOT EXISTS (
            SELECT 1 FROM information_schema.columns
            WHERE table_schema = 'public' AND table_name = target
              AND column_name = 'name' AND character_maximum_length = 100
              AND is_nullable = 'NO'
        ) THEN
            RAISE EXCEPTION 'AdminModel field types mismatch on %', target;
        END IF;
    END LOOP;
    IF (SELECT count(*) FROM inventory_content_type
        WHERE app_label = 'users' AND model IN ('owner', 'ownergroup')) <> 2 THEN
        RAISE EXCEPTION 'Owner content types missing';
    END IF;
END $$;
