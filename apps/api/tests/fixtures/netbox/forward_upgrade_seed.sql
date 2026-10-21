-- Synthetic storage evidence only; no customer rows or usable credentials.
INSERT INTO dcim_manufacturer
    (id, name, slug, description, comments, custom_field_data, tags, external_id, piping_synced_at)
VALUES ('00000000-0000-4000-8000-000000000701', 'Migration manufacturer',
        'migration-manufacturer', 'retained description', 'retained comments',
        '{"retained":true}', ARRAY['migration-tag', 'unresolved-legacy-tag'], 701,
        '2024-01-02T03:04:05Z');
INSERT INTO dcim_interface
    (id, name, label, description, mark_connected, enabled, _name, type, mgmt_only,
     custom_field_data, tags, external_id, piping_synced_at)
VALUES ('00000000-0000-4000-8000-000000000702', 'proof0', '', 'retained interface',
        false, true, 'proof0', 'virtual', false, '{"retained":true}',
        ARRAY['migration-tag', 'unresolved-legacy-tag'], 702, '2024-01-02T03:04:05Z');
INSERT INTO identity_user (id, username, email, auth_source)
VALUES ('00000000-0000-4000-8000-000000000703', 'migration-proof-user',
        'migration-proof@example.invalid', 'local');
INSERT INTO auth_user_credential (user_id, password_hash, password_changed_at)
VALUES ('00000000-0000-4000-8000-000000000703', md5(gen_random_uuid()::text),
        '2024-01-02T03:04:05Z');
INSERT INTO secrets_variable (id, key)
VALUES ('00000000-0000-4000-8000-000000000704', 'migration-proof');
INSERT INTO secrets_store (secret_id, encrypted_value)
VALUES ('00000000-0000-4000-8000-000000000704', md5(gen_random_uuid()::text));
INSERT INTO secrets_binding (secret_id, scope_type, scope_id)
VALUES ('00000000-0000-4000-8000-000000000704', 'user',
        '00000000-0000-4000-8000-000000000703');
INSERT INTO license_instance_identity (instance_fingerprint)
VALUES ('migration-proof-instance-identity') ON CONFLICT (id) DO NOTHING;
INSERT INTO license_state
    (instance_id, status, max_seats, entitlement_jwt, entitlement_version,
     entitlement_expires_at, activation_token_encrypted)
VALUES ('migration-proof-instance', 'inactive', 0, md5(gen_random_uuid()::text),
        'synthetic-storage-proof', '2030-01-01T00:00:00Z', md5(gen_random_uuid()::text));
INSERT INTO settings_values (scope_type, key, value)
VALUES ('instance', 'migration-proof', '{"retained":true}');
INSERT INTO instance_branding (app_name, theme)
VALUES ('Migration proof', '{"retained":true}') ON CONFLICT (id) DO NOTHING;
INSERT INTO ipam_service
    (id, name, protocol, ports, description, comments, custom_field_data,
     parent_object_id, parent_object_type_id)
SELECT '00000000-0000-4000-8000-000000000706', 'Migration service', 'tcp',
       ARRAY[443], 'retained service', '', '{}',
       '00000000-0000-4000-8000-000000000702', id
FROM inventory_content_type WHERE app_label='dcim' AND model='interface';
