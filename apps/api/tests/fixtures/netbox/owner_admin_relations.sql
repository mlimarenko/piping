-- Deterministic data; the enclosing transaction always rolls back.
BEGIN;
INSERT INTO users_ownergroup (id, name) VALUES
    ('00000000-0000-4000-8000-000000000079', 'Owner schema proof group'),
    ('00000000-0000-4000-8000-000000000086', 'Isolated OwnerGroup PROTECT proof');
INSERT INTO users_owner (id, name, group_id) VALUES
    ('00000000-0000-4000-8000-000000000080', 'Owner schema proof',
     '00000000-0000-4000-8000-000000000079');
INSERT INTO users_owner (id, name) VALUES
    ('00000000-0000-4000-8000-000000000081', 'Unassigned owner schema proof');
-- This Owner has no inventory or assignment references. A CASCADE on its
-- group must fail this proof instead of accidentally reaching inventory RESTRICT.
INSERT INTO users_owner (id, name, group_id) VALUES
    ('00000000-0000-4000-8000-000000000087', 'Isolated OwnerGroup member',
     '00000000-0000-4000-8000-000000000086');
INSERT INTO identity_user (id, username, email, auth_source) VALUES
    ('00000000-0000-4000-8000-000000000082', 'owner-schema-proof',
     'owner-schema-proof@example.invalid', 'local');
INSERT INTO identity_group (id, name) VALUES
    ('00000000-0000-4000-8000-000000000083', 'Owner identity proof');
INSERT INTO users_owner_identity_user (owner_id, user_id) VALUES
    ('00000000-0000-4000-8000-000000000080', '00000000-0000-4000-8000-000000000082');
INSERT INTO users_owner_identity_group (owner_id, group_id) VALUES
    ('00000000-0000-4000-8000-000000000080', '00000000-0000-4000-8000-000000000083');
INSERT INTO dcim_manufacturer (id, name, slug, description, comments, custom_field_data, owner_id)
VALUES ('00000000-0000-4000-8000-000000000084', 'Owner inventory proof',
    'owner-inventory-proof', '', '', '{}', '00000000-0000-4000-8000-000000000080');
DO $$
DECLARE
    target record;
    protected_columns integer := 0;
    rejected_constraint text;
BEGIN
    -- Mechanically close every baseline inventory owner_id, excluding the two
    -- M2M junctions (CASCADE). owner_user_id is identity, not inventory ownership.
    FOR target IN
        SELECT table_name FROM information_schema.columns
        WHERE table_schema = 'public' AND column_name = 'owner_id'
          AND table_name NOT IN ('users_owner_identity_user', 'users_owner_identity_group')
    LOOP
        IF NOT EXISTS (
            SELECT 1 FROM pg_constraint c
            JOIN pg_attribute a ON a.attrelid = c.conrelid AND a.attnum = ANY(c.conkey)
            WHERE c.conrelid = format('public.%I', target.table_name)::regclass
              AND c.contype = 'f' AND a.attname = 'owner_id'
              AND c.confrelid = 'users_owner'::regclass
              AND c.confdeltype = 'r' AND c.convalidated
        ) THEN
            RAISE EXCEPTION 'Inventory owner PROTECT missing on %', target.table_name;
        END IF;
        protected_columns := protected_columns + 1;
    END LOOP;
    IF protected_columns <> 75 THEN
        RAISE EXCEPTION 'Expected 75 inventory owner columns, got %', protected_columns;
    END IF;
    BEGIN
        DELETE FROM users_ownergroup WHERE id = '00000000-0000-4000-8000-000000000086';
        RAISE EXCEPTION 'OwnerGroup deletion was not protected';
    EXCEPTION WHEN foreign_key_violation OR restrict_violation THEN
        GET STACKED DIAGNOSTICS rejected_constraint = CONSTRAINT_NAME;
        IF rejected_constraint <> 'users_owner_group_id_fk_users_ownergroup_id' THEN
            RAISE EXCEPTION 'OwnerGroup deletion reached a different protection constraint';
        END IF;
    END;
    BEGIN
        DELETE FROM users_owner WHERE id = '00000000-0000-4000-8000-000000000080';
        RAISE EXCEPTION 'Inventory Owner deletion was not protected';
    EXCEPTION WHEN foreign_key_violation OR restrict_violation THEN NULL;
    END;
    BEGIN
        INSERT INTO users_owner_identity_user VALUES
            ('00000000-0000-4000-8000-000000000080', '00000000-0000-4000-8000-000000000099');
        RAISE EXCEPTION 'Unknown user assignment accepted';
    EXCEPTION WHEN foreign_key_violation THEN NULL;
    END;
    BEGIN
        INSERT INTO users_owner_identity_group VALUES
            ('00000000-0000-4000-8000-000000000080', '00000000-0000-4000-8000-000000000099');
        RAISE EXCEPTION 'Unknown group assignment accepted';
    EXCEPTION WHEN foreign_key_violation THEN NULL;
    END;
    BEGIN
        INSERT INTO users_owner_identity_user SELECT * FROM users_owner_identity_user
        WHERE owner_id = '00000000-0000-4000-8000-000000000080';
        RAISE EXCEPTION 'Duplicate user assignment accepted';
    EXCEPTION WHEN unique_violation THEN NULL;
    END;
    BEGIN
        INSERT INTO users_owner_identity_group SELECT * FROM users_owner_identity_group
        WHERE owner_id = '00000000-0000-4000-8000-000000000080';
        RAISE EXCEPTION 'Duplicate group assignment accepted';
    EXCEPTION WHEN unique_violation THEN NULL;
    END;
    BEGIN
        INSERT INTO users_owner (name, group_id) VALUES
            ('Unknown OwnerGroup proof', '00000000-0000-4000-8000-000000000099');
        RAISE EXCEPTION 'Unknown OwnerGroup assignment accepted';
    EXCEPTION WHEN foreign_key_violation THEN NULL;
    END;
    BEGIN
        INSERT INTO users_owner (name) VALUES ('Owner schema proof');
        RAISE EXCEPTION 'Duplicate Owner name accepted';
    EXCEPTION WHEN unique_violation THEN NULL;
    END;
    BEGIN
        INSERT INTO users_ownergroup (name) VALUES ('Owner schema proof group');
        RAISE EXCEPTION 'Duplicate OwnerGroup name accepted';
    EXCEPTION WHEN unique_violation THEN NULL;
    END;
    IF (SELECT description FROM users_owner WHERE id = '00000000-0000-4000-8000-000000000080') <> ''
       OR (SELECT group_id FROM users_owner WHERE id = '00000000-0000-4000-8000-000000000081') IS NOT NULL
       OR (SELECT description FROM users_ownergroup WHERE id = '00000000-0000-4000-8000-000000000079') <> '' THEN
        RAISE EXCEPTION 'Unassigned AdminModel defaults mismatch';
    END IF;
END $$;
DELETE FROM identity_user WHERE id = '00000000-0000-4000-8000-000000000082';
DELETE FROM identity_group WHERE id = '00000000-0000-4000-8000-000000000083';
DO $$ BEGIN
    IF EXISTS (SELECT 1 FROM users_owner_identity_user WHERE owner_id = '00000000-0000-4000-8000-000000000080')
       OR EXISTS (SELECT 1 FROM users_owner_identity_group WHERE owner_id = '00000000-0000-4000-8000-000000000080')
       OR NOT EXISTS (SELECT 1 FROM users_owner WHERE id = '00000000-0000-4000-8000-000000000080') THEN
        RAISE EXCEPTION 'Identity deletion must cascade only assignments';
    END IF;
END $$;
-- Reassign both edges, then verify deleting an unreferenced owner cascades them.
INSERT INTO identity_user (id, username, email, auth_source) VALUES
    ('00000000-0000-4000-8000-000000000082', 'owner-schema-proof',
     'owner-schema-proof@example.invalid', 'local');
INSERT INTO identity_group (id, name) VALUES
    ('00000000-0000-4000-8000-000000000083', 'Owner identity proof');
INSERT INTO users_owner_identity_user VALUES
    ('00000000-0000-4000-8000-000000000080', '00000000-0000-4000-8000-000000000082');
INSERT INTO users_owner_identity_group VALUES
    ('00000000-0000-4000-8000-000000000080', '00000000-0000-4000-8000-000000000083');
DELETE FROM dcim_manufacturer WHERE id = '00000000-0000-4000-8000-000000000084';
DELETE FROM users_owner WHERE id = '00000000-0000-4000-8000-000000000080';
DO $$ BEGIN
    IF EXISTS (SELECT 1 FROM users_owner_identity_user WHERE owner_id = '00000000-0000-4000-8000-000000000080')
       OR EXISTS (SELECT 1 FROM users_owner_identity_group WHERE owner_id = '00000000-0000-4000-8000-000000000080')
       OR NOT EXISTS (SELECT 1 FROM identity_user WHERE id = '00000000-0000-4000-8000-000000000082')
       OR NOT EXISTS (SELECT 1 FROM identity_group WHERE id = '00000000-0000-4000-8000-000000000083') THEN
        RAISE EXCEPTION 'Owner deletion must cascade only assignments';
    END IF;
END $$;
ROLLBACK;
