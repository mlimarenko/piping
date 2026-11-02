-- Cooling hardware fields absent from the immutable baseline.
ALTER TABLE dcim_device ADD COLUMN cooling_method varchar(50)
    CHECK (cooling_method IN ('air','liquid','hybrid','immersion'));
ALTER TABLE dcim_devicetype ADD COLUMN cooling_method varchar(50)
    CHECK (cooling_method IN ('air','liquid','hybrid','immersion')),
    ADD COLUMN end_of_life date;
ALTER TABLE dcim_moduletype ADD COLUMN cooling_method varchar(50)
    CHECK (cooling_method IN ('air','liquid','hybrid','immersion')),
    ADD COLUMN end_of_life date;
ALTER TABLE dcim_rack ADD COLUMN cooling_capability varchar(50)
    CHECK (cooling_capability IN ('air-only','hybrid','liquid-only')),
    ADD COLUMN cooling_capacity numeric(10,2) CHECK (cooling_capacity >= 0);
ALTER TABLE dcim_racktype ADD COLUMN cooling_capability varchar(50)
    CHECK (cooling_capability IN ('air-only','hybrid','liquid-only')),
    ADD COLUMN cooling_capacity numeric(10,2) CHECK (cooling_capacity >= 0);

ALTER TABLE dcim_coolingintaketemplate ADD CONSTRAINT dcim_coolingintaketemplate_one_parent
    CHECK ((device_type_id IS NULL) <> (module_type_id IS NULL));
ALTER TABLE dcim_coolingoutflowtemplate ADD CONSTRAINT dcim_coolingoutflowtemplate_one_parent
    CHECK ((device_type_id IS NULL) <> (module_type_id IS NULL));
ALTER TABLE dcim_modulebaytemplate ADD CONSTRAINT dcim_modulebaytemplate_one_parent
    CHECK ((device_type_id IS NULL) <> (module_type_id IS NULL));

INSERT INTO inventory_content_type (app_label,model) VALUES
    ('dcim','coolingsource'),('dcim','coolingfeed'),('dcim','coolingintake'),
    ('dcim','coolingoutflow'),('dcim','coolingintaketemplate'),('dcim','coolingoutflowtemplate'),('dcim','modulebaytemplate')
    ON CONFLICT (app_label,model) DO NOTHING;

-- Serialize graph mutations before row locks, including writes through other
-- inventory owners. A pair of concurrent valid-looking edges must not commit a
-- cycle or invalidate a feed through a rack/source/location move.
CREATE FUNCTION dcim_acquire_hardware_graph() RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    -- Same-kind statement triggers run alphabetically. Do not let this graph
    -- trigger precede the Tag usage lock while a catalog restore holds exclusive
    -- usage ownership and waits for the graph. The immutable Tag guards still run.
    PERFORM pg_advisory_xact_lock_shared(hashtextextended('inventory-tag-catalog',0));
    PERFORM pg_advisory_xact_lock(780007);
END
$$;
CREATE FUNCTION dcim_lock_cooling_graph() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM dcim_acquire_hardware_graph();
    RETURN NULL;
END
$$;

CREATE FUNCTION dcim_validate_cooling_graph() RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM dcim_coolingsource s JOIN dcim_location l ON l.id=s.location_id
        WHERE s.site_id<>l.site_id
    ) THEN
        RAISE EXCEPTION 'Cooling source location must belong to its site' USING ERRCODE='23514';
    END IF;
    IF EXISTS (
        SELECT 1 FROM dcim_coolingfeed f JOIN dcim_coolingsource s ON s.id=f.cooling_source_id
        JOIN dcim_rack r ON r.id=f.rack_id WHERE r.site_id<>s.site_id
    ) THEN
        RAISE EXCEPTION 'Cooling feed rack must belong to the source site' USING ERRCODE='23514';
    END IF;
    IF EXISTS (
        SELECT 1 FROM (
            SELECT device_id,module_id FROM dcim_coolingintake
            UNION ALL SELECT device_id,module_id FROM dcim_coolingoutflow
        ) p JOIN dcim_module m ON m.id=p.module_id WHERE p.device_id<>m.device_id
    ) THEN
        RAISE EXCEPTION 'Cooling component module must belong to its device' USING ERRCODE='23514';
    END IF;
    IF EXISTS (
        SELECT 1 FROM dcim_coolingoutflow o JOIN dcim_coolingintake i ON i.id=o.cooling_intake_id
        WHERE o.device_id<>i.device_id
    ) THEN
        RAISE EXCEPTION 'Parent cooling intake must belong to the same device' USING ERRCODE='23514';
    END IF;
    IF EXISTS (
        SELECT 1 FROM dcim_coolingoutflowtemplate o JOIN dcim_coolingintaketemplate i ON i.id=o.cooling_intake_id
        WHERE o.device_type_id IS DISTINCT FROM i.device_type_id OR o.module_type_id IS DISTINCT FROM i.module_type_id
    ) THEN
        RAISE EXCEPTION 'Parent cooling intake template must belong to the same hardware type' USING ERRCODE='23514';
    END IF;
    IF EXISTS (
        WITH RECURSIVE edges(source_kind,source_id,target_kind,target_id) AS (
            SELECT true,id,false,cooling_outflow_id FROM dcim_coolingintake WHERE cooling_outflow_id IS NOT NULL
            UNION ALL
            SELECT false,id,true,cooling_intake_id FROM dcim_coolingoutflow WHERE cooling_intake_id IS NOT NULL
        ), walk(start_kind,start_id,kind,id) AS (
            SELECT * FROM edges
            UNION
            SELECT w.start_kind,w.start_id,e.target_kind,e.target_id
            FROM walk w JOIN edges e ON e.source_kind=w.kind AND e.source_id=w.id
        ) SELECT 1 FROM walk WHERE start_kind=kind AND start_id=id
    ) THEN
        RAISE EXCEPTION 'Cooling connections cannot contain cycles' USING ERRCODE='23514';
    END IF;
END
$$;

-- Refuse pre-existing violations; never silently repair inventory data.
SELECT dcim_validate_cooling_graph();

CREATE FUNCTION dcim_check_cooling_graph() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM dcim_validate_cooling_graph();
    RETURN NULL;
END
$$;

DO $$
DECLARE target text;
BEGIN
    FOREACH target IN ARRAY ARRAY[
        'dcim_coolingsource','dcim_coolingfeed','dcim_coolingintake','dcim_coolingoutflow',
        'dcim_coolingintaketemplate','dcim_coolingoutflowtemplate','dcim_location','dcim_rack','dcim_module',
        'dcim_modulebaytemplate','dcim_modulebaytemplate_module_bay_types','dcim_modulebay','dcim_device'
    ] LOOP
        -- Cascading deletes and fresh installations take the graph lock before
        -- any device/type row. Late component-cascade locking reverses this order.
        EXECUTE format('CREATE TRIGGER cooling_graph_lock BEFORE INSERT OR UPDATE OR DELETE ON %I FOR EACH STATEMENT EXECUTE FUNCTION dcim_lock_cooling_graph()', target);
        EXECUTE format('CREATE CONSTRAINT TRIGGER cooling_graph_valid AFTER INSERT OR UPDATE OR DELETE ON %I DEFERRABLE INITIALLY IMMEDIATE FOR EACH ROW EXECUTE FUNCTION dcim_check_cooling_graph()', target);
    END LOOP;
END
$$;

-- Type deletion can cascade template rows. Its graph lock must precede the type
-- row lock, just as module/device deletion precedes cooling-component cascades.
CREATE TRIGGER hardware_graph_delete_lock BEFORE DELETE ON dcim_devicetype
    FOR EACH STATEMENT EXECUTE FUNCTION dcim_lock_cooling_graph();
CREATE TRIGGER hardware_graph_delete_lock BEFORE DELETE ON dcim_moduletype
    FOR EACH STATEMENT EXECUTE FUNCTION dcim_lock_cooling_graph();

-- A hardware creation is the canonical instantiation boundary. Native REST,
-- imports and other repository consumers receive the same atomic behavior.
CREATE FUNCTION dcim_instantiate_hardware_components() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    hardware_type uuid;
    target_device uuid;
    target_module uuid;
    intake record;
    outflow record;
    bay record;
    intake_ids jsonb := '{}'::jsonb;
    component_id uuid;
    bay_position text;
BEGIN
    -- Explicit archive/snapshot graphs own component identities. Native creates
    -- leave this transaction intent unset and instantiate from templates.
    IF current_setting('piping.hardware_materialization',true)='explicit_graph' THEN RETURN NEW; END IF;
    PERFORM dcim_acquire_hardware_graph();
    IF TG_TABLE_NAME='dcim_device' THEN
        hardware_type := NEW.device_type_id;
        target_device := NEW.id;
        target_module := NULL;
        bay_position := '';
    ELSE
        hardware_type := NEW.module_type_id;
        target_device := NEW.device_id;
        target_module := NEW.id;
        SELECT position INTO bay_position FROM dcim_modulebay WHERE id=NEW.module_bay_id;
        bay_position := COALESCE(bay_position,'');
    END IF;
    FOR bay IN SELECT * FROM dcim_modulebaytemplate
        WHERE (target_module IS NULL AND device_type_id=hardware_type)
           OR (target_module IS NOT NULL AND module_type_id=hardware_type) ORDER BY id
    LOOP
        component_id := uuidv7();
        INSERT INTO dcim_modulebay (id,name,label,position,enabled,description,device_id,module_id,custom_field_data,level,lft,rght,tree_id)
            VALUES (component_id,replace(bay.name,'{module}',bay_position),replace(bay.label,'{module}',bay_position),replace(bay.position,'{module}',bay_position),bay.enabled,bay.description,target_device,target_module,'{}',0,1,2,0);
        INSERT INTO dcim_modulebay_module_bay_types (modulebay_id,modulebaytype_id)
            SELECT component_id,modulebaytype_id FROM dcim_modulebaytemplate_module_bay_types WHERE modulebaytemplate_id=bay.id;
    END LOOP;
    FOR intake IN SELECT * FROM dcim_coolingintaketemplate
        WHERE (target_module IS NULL AND device_type_id=hardware_type)
           OR (target_module IS NOT NULL AND module_type_id=hardware_type) ORDER BY id
    LOOP
        component_id := uuidv7();
        INSERT INTO dcim_coolingintake (id,name,label,type,diameter,diameter_unit,max_flow,max_flow_unit,device_id,module_id,description,custom_field_data,created,last_updated)
            VALUES (component_id,replace(intake.name,'{module}',bay_position),replace(intake.label,'{module}',bay_position),intake.type,intake.diameter,intake.diameter_unit,intake.max_flow,intake.max_flow_unit,target_device,target_module,intake.description,'{}',NOW(),NOW());
        intake_ids := intake_ids || jsonb_build_object(intake.id::text,component_id);
    END LOOP;
    FOR outflow IN SELECT * FROM dcim_coolingoutflowtemplate
        WHERE (target_module IS NULL AND device_type_id=hardware_type)
           OR (target_module IS NOT NULL AND module_type_id=hardware_type) ORDER BY id
    LOOP
        INSERT INTO dcim_coolingoutflow (name,label,type,diameter,diameter_unit,device_id,module_id,cooling_intake_id,description,custom_field_data,created,last_updated)
            VALUES (replace(outflow.name,'{module}',bay_position),replace(outflow.label,'{module}',bay_position),outflow.type,outflow.diameter,outflow.diameter_unit,target_device,target_module,(intake_ids->>outflow.cooling_intake_id::text)::uuid,outflow.description,'{}',NOW(),NOW());
    END LOOP;
    RETURN NEW;
END
$$;
CREATE TRIGGER hardware_components_from_device_type AFTER INSERT ON dcim_device
    FOR EACH ROW EXECUTE FUNCTION dcim_instantiate_hardware_components();
CREATE TRIGGER hardware_components_from_module_type AFTER INSERT ON dcim_module
    FOR EACH ROW EXECUTE FUNCTION dcim_instantiate_hardware_components();

-- DELETE must take the shared type lock before the AFTER-trigger recount.
-- Device locks alone do not serialize uninstalls on different devices.
CREATE FUNCTION dcim_lock_module_delete_counter() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM dcim_acquire_hardware_graph();
    PERFORM id FROM dcim_device WHERE id=OLD.device_id FOR UPDATE;
    PERFORM id FROM dcim_moduletype WHERE id=OLD.module_type_id FOR UPDATE;
    RETURN OLD;
END
$$;
CREATE TRIGGER dcim_module_counter_delete_lock BEFORE DELETE ON dcim_module
    FOR EACH ROW EXECUTE FUNCTION dcim_lock_module_delete_counter();
