DO $$
DECLARE
    type_id uuid := '01900000-0000-7000-8000-000000000096';
    platform_id uuid := '01900000-0000-7000-8000-000000000097';
    unknown_id uuid := '01900000-0000-7000-8000-000000000098';
    legacy_id uuid := '01900000-0000-7000-8000-000000000095';
BEGIN
    IF (SELECT to_jsonb(vm) - 'virtual_machine_type_id'
        FROM virtualization_virtualmachine vm WHERE id = legacy_id)
        IS DISTINCT FROM (SELECT data FROM pwa95_expected_state WHERE kind = 'vm')
    THEN
        RAISE EXCEPTION 'VM values changed while adding its nullable type';
    END IF;
    IF (SELECT to_jsonb(identity) FROM license_instance_identity identity)
        IS DISTINCT FROM (SELECT data FROM pwa95_expected_state WHERE kind = 'license')
    THEN
        RAISE EXCEPTION 'License instance identity changed';
    END IF;

    INSERT INTO dcim_platform
    SELECT (jsonb_populate_record(NULL::dcim_platform,
        to_jsonb(p) || jsonb_build_object('id', platform_id, 'name', 'PWA-95 platform',
                                        'slug', 'pwa95-platform', 'external_id', NULL))).*
    FROM dcim_platform p LIMIT 1;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'Fixture requires the canonical platform seed';
    END IF;

    INSERT INTO virtualization_virtualmachinetype
        (id, custom_field_data, name, slug, default_platform_id,
         default_vcpus, default_memory, description, comments)
    VALUES (type_id, '{}', 'PWA-95 Type', 'pwa95-type', platform_id, 0.25, 0, '', '');

    BEGIN
        INSERT INTO virtualization_virtualmachinetype
            (custom_field_data, name, slug, description, comments)
        VALUES ('{}', 'pwa-95 type', 'pwa95-another', '', '');
        RAISE EXCEPTION 'Case-insensitive name uniqueness was not enforced';
    EXCEPTION WHEN unique_violation THEN NULL; END;

    BEGIN
        INSERT INTO virtualization_virtualmachinetype
            (custom_field_data, name, slug, description, comments)
        VALUES ('{}', 'Different name', 'pwa95-type', '', '');
        RAISE EXCEPTION 'Slug uniqueness was not enforced';
    EXCEPTION WHEN unique_violation THEN NULL; END;

    BEGIN
        UPDATE virtualization_virtualmachinetype SET default_vcpus = 0 WHERE id = type_id;
        RAISE EXCEPTION 'Zero vCPU was accepted';
    EXCEPTION WHEN check_violation THEN NULL; END;
    BEGIN
        UPDATE virtualization_virtualmachinetype SET default_vcpus = 'NaN' WHERE id = type_id;
        RAISE EXCEPTION 'Non-finite vCPU was accepted';
    EXCEPTION WHEN check_violation THEN NULL; END;
    BEGIN
        UPDATE virtualization_virtualmachinetype SET default_vcpus = 10000 WHERE id = type_id;
        RAISE EXCEPTION 'Out-of-range vCPU was accepted';
    EXCEPTION WHEN numeric_value_out_of_range THEN NULL; END;
    BEGIN
        UPDATE virtualization_virtualmachinetype SET default_memory = -1 WHERE id = type_id;
        RAISE EXCEPTION 'Negative memory was accepted';
    EXCEPTION WHEN check_violation THEN NULL; END;
    BEGIN
        UPDATE virtualization_virtualmachinetype SET default_platform_id = unknown_id WHERE id = type_id;
        RAISE EXCEPTION 'Missing platform was accepted';
    EXCEPTION WHEN foreign_key_violation THEN NULL; END;
    BEGIN
        UPDATE virtualization_virtualmachine SET virtual_machine_type_id = unknown_id WHERE id = legacy_id;
        RAISE EXCEPTION 'Missing VM type was accepted';
    EXCEPTION WHEN foreign_key_violation THEN NULL; END;

    UPDATE virtualization_virtualmachine SET virtual_machine_type_id = type_id WHERE id = legacy_id;
    UPDATE virtualization_virtualmachinetype SET default_vcpus = 1.5, default_memory = 8192 WHERE id = type_id;
    IF NOT EXISTS (SELECT FROM virtualization_virtualmachine
                   WHERE id = legacy_id AND vcpus = 0.75 AND memory = 1024)
    THEN
        RAISE EXCEPTION 'Type update overwrote explicit VM values';
    END IF;
    BEGIN
        DELETE FROM virtualization_virtualmachinetype WHERE id = type_id;
        RAISE EXCEPTION 'Referenced type deletion was accepted';
    EXCEPTION WHEN foreign_key_violation OR restrict_violation THEN NULL; END;

    DELETE FROM dcim_platform WHERE id = platform_id;
    IF NOT EXISTS (SELECT FROM virtualization_virtualmachinetype
                   WHERE id = type_id AND default_platform_id IS NULL)
    THEN
        RAISE EXCEPTION 'Platform deletion did not clear the type default';
    END IF;
END;
$$;
UPDATE virtualization_virtualmachine SET virtual_machine_type_id = NULL
WHERE id = '01900000-0000-7000-8000-000000000095';
DELETE FROM virtualization_virtualmachinetype
WHERE id = '01900000-0000-7000-8000-000000000096';
DROP TABLE pwa95_expected_state;
