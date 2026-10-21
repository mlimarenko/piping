-- Independent NetBox v4.7.1 schema-family contract for the reconciled tail.
-- This is schema prerequisite evidence, not native API/UI/import parity.
DO $$
DECLARE
    target text;
    owner_tables text[] := ARRAY[
        'dcim_rackgroup', 'dcim_cablebundle', 'dcim_modulebaytype',
        'virtualization_virtualmachinetype', 'dcim_coolingsource', 'dcim_coolingfeed',
        'dcim_coolingintake', 'dcim_coolingoutflow', 'wireless_wirelesslangroup',
        'wireless_wirelesslan', 'wireless_wirelesslink', 'ipam_fhrpgroup'
    ];
BEGIN
    IF (SELECT count(DISTINCT c.conrelid) FROM pg_constraint c
        JOIN pg_attribute a ON a.attrelid=c.conrelid AND a.attnum=ANY(c.conkey)
        WHERE c.contype='f' AND a.attname='owner_id'
          AND c.confrelid='public.users_owner'::regclass AND c.confdeltype='r') <> 87 THEN
        RAISE EXCEPTION 'Expected 75 published plus 12 new protected Owner columns';
    END IF;
    FOREACH target IN ARRAY owner_tables LOOP
        IF to_regclass('public.' || target) IS NULL THEN
            RAISE EXCEPTION 'Missing forward model %', target;
        END IF;
        IF NOT EXISTS (
            SELECT 1 FROM pg_constraint c
            JOIN pg_attribute a ON a.attrelid=c.conrelid AND a.attnum=ANY(c.conkey)
            WHERE c.conrelid=to_regclass('public.' || target)
              AND c.contype='f' AND a.attname='owner_id'
              AND c.confrelid='public.users_owner'::regclass AND c.confdeltype='r'
        ) THEN RAISE EXCEPTION 'Missing PROTECT Owner FK on %', target; END IF;
        IF NOT EXISTS (
            SELECT 1 FROM pg_index i
            JOIN pg_attribute a ON a.attrelid=i.indrelid AND a.attnum=ANY(i.indkey)
            WHERE i.indrelid=to_regclass('public.' || target) AND a.attname='owner_id'
        ) THEN RAISE EXCEPTION 'Missing Owner lookup index on %', target; END IF;
    END LOOP;
    FOREACH target IN ARRAY ARRAY['dcim_coolingintaketemplate', 'dcim_coolingoutflowtemplate',
                                  'ipam_fhrpgroupassignment'] LOOP
        IF to_regclass('public.' || target) IS NULL THEN
            RAISE EXCEPTION 'Missing ancillary model %', target;
        END IF;
        IF EXISTS (SELECT 1 FROM information_schema.columns
                   WHERE table_schema='public' AND table_name=target
                     AND column_name IN ('owner_id', 'tags', 'custom_field_data', 'comments')) THEN
            RAISE EXCEPTION 'Invented PrimaryModel metadata on %', target;
        END IF;
    END LOOP;
    IF to_regclass('public.dcim_interface_wirelesslans') IS NULL THEN
        RAISE EXCEPTION 'Missing wireless interface M2M';
    END IF;
    IF EXISTS (SELECT 1 FROM information_schema.columns
               WHERE table_schema='public' AND table_name=ANY(owner_tables || ARRAY['ipam_fhrpgroupassignment'])
                 AND column_name IN ('created', 'last_updated')
                 AND (is_nullable <> 'YES' OR column_default IS NOT NULL)) THEN
        RAISE EXCEPTION 'Unknown source dates must stay nullable without fabricated defaults';
    END IF;
    IF to_regclass('public.extras_tag') IS NOT NULL OR to_regclass('public.extras_taggeditem') IS NOT NULL THEN
        RAISE EXCEPTION 'Reconciliation introduced dual tag storage';
    END IF;
    IF NOT EXISTS (SELECT 1 FROM information_schema.columns
                   WHERE table_schema='public' AND table_name='dcim_interface'
                     AND column_name='speed' AND data_type='bigint') THEN
        RAISE EXCEPTION 'Published interface speed representation changed';
    END IF;
    IF NOT EXISTS (SELECT 1 FROM information_schema.columns
                   WHERE table_schema='public' AND table_name='ipam_service'
                     AND column_name='ports' AND is_nullable='YES')
       OR to_regclass('public.ipam_service_port_mapping') IS NULL THEN
        RAISE EXCEPTION 'Published service storage path changed';
    END IF;
END $$;
