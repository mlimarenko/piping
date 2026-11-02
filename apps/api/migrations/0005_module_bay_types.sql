-- Existing graph errors are refused, never repaired by a schema migration.
-- Preserve any existing registry UUID and the custom-field scopes that use it.
INSERT INTO inventory_content_type (app_label,model) VALUES ('dcim','module'),('dcim','moduletype')
    ON CONFLICT (app_label,model) DO NOTHING;
ALTER TABLE dcim_modulebay ADD COLUMN enabled boolean NOT NULL DEFAULT true;
ALTER TABLE dcim_modulebaytemplate ADD COLUMN enabled boolean NOT NULL DEFAULT true;

CREATE TABLE dcim_modulebay_module_bay_types (
    id uuid PRIMARY KEY DEFAULT uuidv7(),
    modulebay_id uuid NOT NULL REFERENCES dcim_modulebay(id) ON DELETE CASCADE,
    modulebaytype_id uuid NOT NULL REFERENCES dcim_modulebaytype(id) ON DELETE RESTRICT,
    UNIQUE (modulebay_id, modulebaytype_id)
);
CREATE INDEX ON dcim_modulebay_module_bay_types (modulebaytype_id);
CREATE TABLE dcim_moduletype_module_bay_types (
    id uuid PRIMARY KEY DEFAULT uuidv7(),
    moduletype_id uuid NOT NULL REFERENCES dcim_moduletype(id) ON DELETE CASCADE,
    modulebaytype_id uuid NOT NULL REFERENCES dcim_modulebaytype(id) ON DELETE RESTRICT,
    UNIQUE (moduletype_id, modulebaytype_id)
);
CREATE INDEX ON dcim_moduletype_module_bay_types (modulebaytype_id);
CREATE TABLE dcim_modulebaytemplate_module_bay_types (
    id uuid PRIMARY KEY DEFAULT uuidv7(),
    modulebaytemplate_id uuid NOT NULL REFERENCES dcim_modulebaytemplate(id) ON DELETE CASCADE,
    modulebaytype_id uuid NOT NULL REFERENCES dcim_modulebaytype(id) ON DELETE RESTRICT,
    UNIQUE (modulebaytemplate_id, modulebaytype_id)
);
CREATE INDEX ON dcim_modulebaytemplate_module_bay_types (modulebaytype_id);

ALTER TABLE dcim_module ADD CONSTRAINT dcim_module_module_bay_fk
    FOREIGN KEY (module_bay_id) REFERENCES dcim_modulebay(id) ON DELETE RESTRICT;
ALTER TABLE dcim_modulebay ADD CONSTRAINT dcim_modulebay_module_fk
    FOREIGN KEY (module_id) REFERENCES dcim_module(id) ON DELETE RESTRICT;
ALTER TABLE dcim_modulebay ADD CONSTRAINT dcim_modulebay_parent_fk
    FOREIGN KEY (parent_id) REFERENCES dcim_modulebay(id) ON DELETE RESTRICT;
ALTER TABLE dcim_modulebay DROP CONSTRAINT dcim_modulebay_unique_device_module_name;
ALTER TABLE dcim_modulebay ADD CONSTRAINT dcim_modulebay_unique_device_module_name
    UNIQUE NULLS NOT DISTINCT (device_id, module_id, name);

CREATE FUNCTION dcim_module_bay_compatible(bay uuid, module_type uuid) RETURNS boolean
LANGUAGE sql STABLE AS $$
    SELECT NOT EXISTS (SELECT 1 FROM dcim_modulebay_module_bay_types WHERE modulebay_id=bay)
        OR NOT EXISTS (SELECT 1 FROM dcim_moduletype_module_bay_types WHERE moduletype_id=module_type)
        OR EXISTS (
            SELECT 1 FROM dcim_modulebay_module_bay_types b
            JOIN dcim_moduletype_module_bay_types t USING (modulebaytype_id)
            WHERE b.modulebay_id=bay AND t.moduletype_id=module_type
        )
$$;

CREATE FUNCTION dcim_validate_module_graph(device uuid) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM dcim_module m JOIN dcim_modulebay b ON b.id=m.module_bay_id
        WHERE (m.device_id=device OR b.device_id=device) AND m.device_id<>b.device_id
    ) OR EXISTS (
        SELECT 1 FROM dcim_modulebay b LEFT JOIN dcim_module m ON m.id=b.module_id
        WHERE b.device_id=device AND (
            (b.module_id IS NOT NULL AND (m.id IS NULL OR m.device_id<>b.device_id))
            OR b.parent_id IS DISTINCT FROM m.module_bay_id
        )
    ) THEN
        RAISE EXCEPTION 'Module bay containment must follow the containing module on the same device' USING ERRCODE='23514';
    END IF;
    IF EXISTS (
        WITH RECURSIVE ancestry AS (
            SELECT id, parent_id, ARRAY[id] AS visited, false AS cycle
            FROM dcim_modulebay WHERE device_id=device
            UNION ALL
            SELECT a.id, b.parent_id, a.visited || b.id, b.id=ANY(a.visited)
            FROM ancestry a JOIN dcim_modulebay b ON b.id=a.parent_id WHERE NOT a.cycle
        ) SELECT 1 FROM ancestry WHERE cycle
    ) THEN
        RAISE EXCEPTION 'Module bay containment cannot contain cycles' USING ERRCODE='23514';
    END IF;
END
$$;

DO $$
DECLARE device uuid;
BEGIN
    FOR device IN SELECT DISTINCT device_id FROM dcim_modulebay UNION SELECT DISTINCT device_id FROM dcim_module LOOP
        PERFORM dcim_validate_module_graph(device);
    END LOOP;
END
$$;

-- Serialize graph writers on their device, including direct native import writers.
CREATE SEQUENCE dcim_modulebay_tree_id_seq;
SELECT setval('dcim_modulebay_tree_id_seq', GREATEST(COALESCE((SELECT MAX(tree_id) FROM dcim_modulebay),0)+1,1), false);

CREATE FUNCTION dcim_module_graph_before_write() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE containing_bay uuid;
DECLARE containing_device uuid;
BEGIN
    IF TG_OP='DELETE' THEN
        PERFORM id FROM dcim_device WHERE id=OLD.device_id FOR UPDATE;
        RETURN OLD;
    END IF;
    IF TG_OP='UPDATE' AND NEW.device_id<>OLD.device_id THEN
        RAISE EXCEPTION 'Moving a module graph between devices is not supported' USING ERRCODE='23514';
    END IF;
    PERFORM id FROM dcim_device WHERE id=NEW.device_id FOR UPDATE;
    IF TG_TABLE_NAME='dcim_modulebay' THEN
        SELECT site_id,rack_id,location_id INTO NEW._site_id,NEW._rack_id,NEW._location_id FROM dcim_device WHERE id=NEW.device_id;
        IF NEW.module_id IS NULL THEN
            NEW.parent_id=NULL;
            IF TG_OP='INSERT' OR OLD.parent_id IS NOT NULL THEN
                NEW.tree_id=nextval('dcim_modulebay_tree_id_seq');
            END IF;
        ELSE
            SELECT module_bay_id,device_id INTO containing_bay,containing_device FROM dcim_module WHERE id=NEW.module_id;
            IF NOT FOUND OR containing_device<>NEW.device_id THEN
                RAISE EXCEPTION 'Containing module must exist on the same device' USING ERRCODE='23514';
            END IF;
            NEW.parent_id=containing_bay;
        END IF;
    ELSE
        PERFORM id FROM dcim_moduletype WHERE id=NEW.module_type_id FOR UPDATE;
        IF TG_OP='INSERT' OR NEW.module_bay_id<>OLD.module_bay_id OR NEW.module_type_id<>OLD.module_type_id THEN
            IF NOT EXISTS (SELECT 1 FROM dcim_modulebay WHERE id=NEW.module_bay_id AND device_id=NEW.device_id AND enabled) THEN
                RAISE EXCEPTION 'Install requires an enabled bay on the same device' USING ERRCODE='23514';
            END IF;
            IF NOT dcim_module_bay_compatible(NEW.module_bay_id,NEW.module_type_id) THEN
                RAISE EXCEPTION 'Module type and bay compatibility categories do not intersect' USING ERRCODE='23514';
            END IF;
        END IF;
    END IF;
    RETURN NEW;
END
$$;

CREATE FUNCTION dcim_module_graph_after_write() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE device uuid;
BEGIN
    device=CASE WHEN TG_OP='DELETE' THEN OLD.device_id ELSE NEW.device_id END;
    PERFORM dcim_validate_module_graph(device);
    -- One graph owner maintains native nested-set projections and counters.
    WITH RECURSIVE tree AS (
        SELECT id, ARRAY[id] AS path, id AS root, tree_id, 0 AS depth
        FROM dcim_modulebay WHERE device_id=device AND parent_id IS NULL
        UNION ALL
        SELECT b.id,t.path || b.id,t.root,t.tree_id,t.depth+1
        FROM tree t JOIN dcim_modulebay b ON b.parent_id=t.id
    ), positioned AS (
        SELECT t.*, (2*row_number() OVER (PARTITION BY root ORDER BY path)-depth-1)::integer AS left_edge,
            (SELECT COUNT(*) FROM tree descendants WHERE t.path <@ descendants.path)::integer AS size
        FROM tree t
    ) UPDATE dcim_modulebay b SET level=p.depth,lft=p.left_edge,rght=p.left_edge+2*p.size-1,tree_id=p.tree_id
      FROM positioned p WHERE b.id=p.id;
    UPDATE dcim_device d SET module_bay_count=(SELECT COUNT(*) FROM dcim_modulebay WHERE device_id=device) WHERE d.id=device;
    UPDATE dcim_moduletype t SET module_count=(SELECT COUNT(*) FROM dcim_module WHERE module_type_id=t.id)
      WHERE t.id IN (SELECT module_type_id FROM dcim_module WHERE device_id=device)
         OR (TG_TABLE_NAME='dcim_module' AND t.id=(to_jsonb(OLD)->>'module_type_id')::uuid);
    RETURN NULL;
END
$$;
CREATE TRIGGER dcim_module_graph_before BEFORE INSERT OR UPDATE OF device_id,module_bay_id,module_type_id OR DELETE ON dcim_module
    FOR EACH ROW EXECUTE FUNCTION dcim_module_graph_before_write();
CREATE TRIGGER dcim_modulebay_graph_before BEFORE INSERT OR UPDATE OF device_id,module_id,parent_id,name,enabled OR DELETE ON dcim_modulebay
    FOR EACH ROW EXECUTE FUNCTION dcim_module_graph_before_write();
CREATE TRIGGER dcim_module_graph_after AFTER INSERT OR UPDATE OF device_id,module_bay_id,module_type_id OR DELETE ON dcim_module
    FOR EACH ROW EXECUTE FUNCTION dcim_module_graph_after_write();
CREATE TRIGGER dcim_modulebay_graph_after AFTER INSERT OR UPDATE OF device_id,module_id,parent_id OR DELETE ON dcim_modulebay
    FOR EACH ROW EXECUTE FUNCTION dcim_module_graph_after_write();

-- Membership changes may make existing occupants incompatible, but serialize
-- with installations so an install cannot pass against a stale category set.
CREATE FUNCTION dcim_module_membership_lock() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE category_owner uuid;
BEGIN
    IF TG_TABLE_NAME='dcim_moduletype_module_bay_types' THEN
        category_owner=CASE WHEN TG_OP='DELETE' THEN OLD.moduletype_id ELSE NEW.moduletype_id END;
        PERFORM id FROM dcim_moduletype WHERE id=category_owner FOR UPDATE;
    ELSE
        category_owner=CASE WHEN TG_OP='DELETE' THEN OLD.modulebay_id ELSE NEW.modulebay_id END;
        PERFORM d.id FROM dcim_device d JOIN dcim_modulebay b ON b.device_id=d.id WHERE b.id=category_owner FOR UPDATE OF d;
    END IF;
    IF TG_OP='DELETE' THEN RETURN OLD; END IF;
    RETURN NEW;
END
$$;
CREATE TRIGGER dcim_modulebay_membership_lock BEFORE INSERT OR UPDATE OR DELETE ON dcim_modulebay_module_bay_types
    FOR EACH ROW EXECUTE FUNCTION dcim_module_membership_lock();
CREATE TRIGGER dcim_moduletype_membership_lock BEFORE INSERT OR UPDATE OR DELETE ON dcim_moduletype_module_bay_types
    FOR EACH ROW EXECUTE FUNCTION dcim_module_membership_lock();
