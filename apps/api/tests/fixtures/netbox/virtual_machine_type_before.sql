INSERT INTO virtualization_virtualmachine
    (id, custom_field_data, name, status, vcpus, memory, disk, comments,
     description, interface_count, virtual_disk_count, serial, start_on_boot, tags)
VALUES
    ('01900000-0000-7000-8000-000000000095', '{"migration_sentinel":"kept"}',
     'PWA-95 preserved VM', 'active', 0.75, 1024, 16, 'preserve comment',
     'preserve description', 0, 0, '', 'false', ARRAY['preserve-tag']);

CREATE TABLE pwa95_expected_state AS
SELECT 'vm'::text AS kind, to_jsonb(vm) AS data
FROM virtualization_virtualmachine vm
WHERE id = '01900000-0000-7000-8000-000000000095'
UNION ALL
SELECT 'license', to_jsonb(identity) FROM license_instance_identity identity;
