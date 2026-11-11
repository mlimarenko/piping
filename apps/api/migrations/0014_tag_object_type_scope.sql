-- Scope is normalized against the same native content-type registry as custom fields.
CREATE TABLE inventory_tag_object_type (
    tag_id uuid NOT NULL REFERENCES inventory_tag(id) ON DELETE CASCADE,
    content_type_id uuid NOT NULL REFERENCES inventory_content_type(id) ON DELETE RESTRICT,
    PRIMARY KEY (tag_id, content_type_id)
);

CREATE FUNCTION inventory_tag_scope_lock() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF current_setting('transaction_isolation') <> 'read committed' THEN
        RAISE EXCEPTION 'Tag mutations require read committed isolation' USING ERRCODE='23514';
    END IF;
    -- Truncation clears every restriction, the same final state as DELETE.
    PERFORM pg_advisory_xact_lock(hashtextextended('inventory-tag-catalog',0));
    RETURN NULL;
END
$$;
CREATE TRIGGER tag_scope_catalog_lock BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE
ON inventory_tag_object_type FOR EACH STATEMENT EXECUTE FUNCTION inventory_tag_scope_lock();

CREATE FUNCTION inventory_tag_scope_type_guard() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM inventory_content_type c
        WHERE c.id=NEW.content_type_id
          AND c.app_label ~ '^[a-z][a-z0-9]*$' AND c.model ~ '^[a-z][a-z0-9]*$'
          AND c.app_label || '_' || c.model IN (SELECT table_name FROM inventory_tag_subjects())
    ) THEN
        RAISE EXCEPTION 'Tag object type is not a registered taggable target' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END
$$;
CREATE TRIGGER tag_scope_type_guard BEFORE INSERT OR UPDATE ON inventory_tag_object_type
FOR EACH ROW EXECUTE FUNCTION inventory_tag_scope_type_guard();

-- A registry rename could otherwise change the meaning of an existing scope.
CREATE FUNCTION inventory_tag_scope_registry_guard() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF (NEW.app_label, NEW.model) IS DISTINCT FROM (OLD.app_label, OLD.model)
       AND EXISTS (SELECT 1 FROM inventory_tag_object_type WHERE content_type_id=OLD.id) THEN
        RAISE EXCEPTION 'Scoped content type cannot be renamed' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END
$$;
CREATE TRIGGER tag_scope_registry_guard BEFORE UPDATE ON inventory_content_type
FOR EACH ROW EXECUTE FUNCTION inventory_tag_scope_registry_guard();
CREATE TRIGGER tag_scope_registry_lock BEFORE UPDATE OR DELETE ON inventory_content_type
FOR EACH STATEMENT EXECUTE FUNCTION inventory_tag_catalog_lock();

CREATE OR REPLACE FUNCTION inventory_tag_membership_guard() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM inventory_validate_tag_ids(NEW.tags);
    IF EXISTS (
        SELECT 1 FROM unnest(NEW.tags) token
        WHERE EXISTS (SELECT 1 FROM inventory_tag_object_type s WHERE s.tag_id=token::uuid)
          AND NOT EXISTS (
              SELECT 1 FROM inventory_tag_object_type s JOIN inventory_content_type c ON c.id=s.content_type_id
              WHERE s.tag_id=token::uuid AND c.app_label || '_' || c.model=TG_TABLE_NAME
          )
    ) THEN
        RAISE EXCEPTION 'Tag is incompatible with inventory object type' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END
$$;

-- Deferred to permit replacing a scope as a set, while checking the final set
-- against every existing use. The catalogue statement lock serializes writers.
CREATE FUNCTION inventory_tag_scope_check(tag uuid) RETURNS void LANGUAGE plpgsql AS $$
DECLARE subject record; referenced boolean;
BEGIN
    IF NOT EXISTS (SELECT 1 FROM inventory_tag_object_type WHERE tag_id=tag) THEN RETURN; END IF;
    FOR subject IN SELECT table_name FROM inventory_tag_subjects() LOOP
        IF NOT EXISTS (
            SELECT 1 FROM inventory_tag_object_type s JOIN inventory_content_type c ON c.id=s.content_type_id
            WHERE s.tag_id=tag AND c.app_label || '_' || c.model=subject.table_name
        ) THEN
            EXECUTE format('SELECT EXISTS (SELECT 1 FROM public.%I WHERE tags @> ARRAY[$1])', subject.table_name)
                INTO referenced USING tag::text;
            IF referenced THEN
                RAISE EXCEPTION 'Tag scope conflicts with existing inventory membership' USING ERRCODE='23514';
            END IF;
        END IF;
    END LOOP;
END
$$;

CREATE FUNCTION inventory_tag_scope_constraint() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP <> 'INSERT' THEN PERFORM inventory_tag_scope_check(OLD.tag_id); END IF;
    IF TG_OP <> 'DELETE' THEN PERFORM inventory_tag_scope_check(NEW.tag_id); END IF;
    RETURN NULL;
END
$$;
CREATE CONSTRAINT TRIGGER tag_scope_compatible AFTER INSERT OR UPDATE OR DELETE ON inventory_tag_object_type
DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION inventory_tag_scope_constraint();
