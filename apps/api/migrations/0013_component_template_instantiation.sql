-- Native component-template instantiation for the nine DCIM component template
-- models owned by this workstream. The instantiation boundary stays the
-- `dcim_instantiate_hardware_components` trigger function created in 0007:
-- native REST, imports and every other repository consumer receive the same
-- atomic behavior. This migration replaces that function and keeps the
-- ModuleBay and Cooling branches byte-for-byte, including the explicit
-- materialization bypass and the hardware-graph serialization.
--
-- Upstream reference: NetBox v4.7.1 dcim/models/device_component_templates.py,
-- dcim/models/devices.py::Device.save/_instantiate_components and
-- dcim/models/modules.py::Module._save_new.
--
-- Local schema differences honored here:
--   * runtime component uniqueness is (device_id, name), not
--     (device_id, module_id, name), so duplicate detection is device-wide;
--   * InterfaceTemplate has no parent/channels/channel_id locally; only
--     bridge_id is reused and applied in a second pass;
--   * DeviceBayTemplate and InventoryItemTemplate are device-type only, so
--     they are instantiated for a device and never for a module;
--   * MPTT columns are persisted at 0 like every other local inventory-item
--     writer; hierarchy is expressed through parent_id.
--
-- Deliberate, documented deviation: upstream looks typed inventory-item
-- component references up by the raw template name, which cannot resolve the
-- {module}/{vc_position} tokens used when the component itself was
-- instantiated. This migration resolves the referenced template name with the
-- same placeholder rules as component creation so the reference either
-- resolves to the component that actually exists or fails closed.
--
-- The typed component target allowlist is the upstream
-- dcim/constants.py COMPONENT_TEMPLATE_MODELS set for NetBox v4.7.1:
-- consoleporttemplate, consoleserverporttemplate, coolingintaketemplate,
-- coolingoutflowtemplate, frontporttemplate, interfacetemplate,
-- poweroutlettemplate, powerporttemplate and rearporttemplate. DeviceBay,
-- InventoryItem and ModuleBay templates are model identities only and are
-- rejected as typed component targets. Cooling templates resolve through the
-- name convention used by the preserved cooling instantiation branch.

-- Template content types give the nine owned component template models their
-- canonical inventory identity. Typed inventory-item component targets are
-- still restricted to the upstream COMPONENT_TEMPLATE_MODELS allowlist below.
INSERT INTO inventory_content_type (app_label, model) VALUES
    ('dcim', 'consoleporttemplate'),
    ('dcim', 'consoleserverporttemplate'),
    ('dcim', 'powerporttemplate'),
    ('dcim', 'poweroutlettemplate'),
    ('dcim', 'interfacetemplate'),
    ('dcim', 'frontporttemplate'),
    ('dcim', 'rearporttemplate'),
    ('dcim', 'devicebaytemplate'),
    ('dcim', 'inventoryitemtemplate')
ON CONFLICT (app_label, model) DO NOTHING;

-- Single placeholder owner shared by component creation and typed component
-- lookup: {module} only for module instantiation, and {vc_position} /
-- {vc_position:X} against the device's virtual chassis, matching upstream
-- ModularComponentTemplateModel._resolve_all_placeholders.
CREATE FUNCTION dcim_resolve_component_placeholder(
    value text,
    target_device uuid,
    bay_position text,
    instantiate_module boolean
) RETURNS text LANGUAGE plpgsql STABLE AS $$
DECLARE
    vc_position integer;
    chassis_id uuid;
    module_tokens integer;
BEGIN
    IF value IS NULL THEN
        RETURN NULL;
    END IF;
    IF instantiate_module AND position('{module}' IN value) > 0 THEN
        module_tokens := (length(value) - length(replace(value, '{module}', ''))) / length('{module}');
        IF module_tokens > 1 THEN
            RAISE EXCEPTION 'Multiple {module} placeholders need a multi-level module bay position chain, which this instantiation path does not support' USING ERRCODE='23514';
        END IF;
        IF position('{module}' IN bay_position) > 0 THEN
            RAISE EXCEPTION 'Module bay position % still contains an unresolved {module} token; inherited module bay positions are not supported by this instantiation path', bay_position USING ERRCODE='23514';
        END IF;
        value := replace(value, '{module}', bay_position);
    END IF;
    IF position('{vc_position' IN value) = 0 THEN
        RETURN value;
    END IF;
    SELECT d.vc_position, d.virtual_chassis_id INTO vc_position, chassis_id
        FROM dcim_device d WHERE d.id = target_device;
    IF chassis_id IS NOT NULL AND vc_position IS NOT NULL THEN
        RETURN regexp_replace(value, '\{vc_position(:[^}]*)?\}', vc_position::text, 'g');
    END IF;
    value := regexp_replace(value, '\{vc_position\}', '0', 'g');
    RETURN regexp_replace(value, '\{vc_position:([^}]*)\}', '\1', 'g');
END
$$;

CREATE OR REPLACE FUNCTION dcim_instantiate_hardware_components() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    hardware_type uuid;
    target_device uuid;
    target_module uuid;
    intake record;
    outflow record;
    bay record;
    tpl record;
    component_template record;
    matching_rows bigint;
    item record;
    intake_ids jsonb := '{}'::jsonb;
    item_ids jsonb := '{}'::jsonb;
    new_component_id uuid;
    bay_position text;
    resolved_name text;
    resolved_label text;
    resolved_power_port text;
    parent_power_port_id uuid;
    parent_item uuid;
    resolved_endpoint_id uuid;
    resolved_reference_id uuid;
    component_ref_name text;
    component_model text;
    runtime_model text;
    component_runtime_id uuid;
    component_content_type uuid;
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
        new_component_id := uuidv7();
        INSERT INTO dcim_modulebay (id,name,label,position,enabled,description,device_id,module_id,custom_field_data,level,lft,rght,tree_id)
            VALUES (new_component_id,replace(bay.name,'{module}',bay_position),replace(bay.label,'{module}',bay_position),replace(bay.position,'{module}',bay_position),bay.enabled,bay.description,target_device,target_module,'{}',0,1,2,0);
        INSERT INTO dcim_modulebay_module_bay_types (modulebay_id,modulebaytype_id)
            SELECT new_component_id,modulebaytype_id FROM dcim_modulebaytemplate_module_bay_types WHERE modulebaytemplate_id=bay.id;
    END LOOP;
    FOR intake IN SELECT * FROM dcim_coolingintaketemplate
        WHERE (target_module IS NULL AND device_type_id=hardware_type)
           OR (target_module IS NOT NULL AND module_type_id=hardware_type) ORDER BY id
    LOOP
        new_component_id := uuidv7();
        INSERT INTO dcim_coolingintake (id,name,label,type,diameter,diameter_unit,max_flow,max_flow_unit,device_id,module_id,description,custom_field_data,created,last_updated)
            VALUES (new_component_id,replace(intake.name,'{module}',bay_position),replace(intake.label,'{module}',bay_position),intake.type,intake.diameter,intake.diameter_unit,intake.max_flow,intake.max_flow_unit,target_device,target_module,intake.description,'{}',NOW(),NOW());
        intake_ids := intake_ids || jsonb_build_object(intake.id::text,new_component_id);
    END LOOP;
    FOR outflow IN SELECT * FROM dcim_coolingoutflowtemplate
        WHERE (target_module IS NULL AND device_type_id=hardware_type)
           OR (target_module IS NOT NULL AND module_type_id=hardware_type) ORDER BY id
    LOOP
        INSERT INTO dcim_coolingoutflow (name,label,type,diameter,diameter_unit,device_id,module_id,cooling_intake_id,description,custom_field_data,created,last_updated)
            VALUES (replace(outflow.name,'{module}',bay_position),replace(outflow.label,'{module}',bay_position),outflow.type,outflow.diameter,outflow.diameter_unit,target_device,target_module,(intake_ids->>outflow.cooling_intake_id::text)::uuid,outflow.description,'{}',NOW(),NOW());
    END LOOP;

    -- Console ports.
    FOR tpl IN SELECT * FROM dcim_consoleporttemplate
        WHERE (target_module IS NULL AND device_type_id=hardware_type)
           OR (target_module IS NOT NULL AND module_type_id=hardware_type) ORDER BY name
    LOOP
        resolved_name := dcim_resolve_component_placeholder(tpl.name,target_device,bay_position,target_module IS NOT NULL);
        resolved_label := dcim_resolve_component_placeholder(tpl.label,target_device,bay_position,target_module IS NOT NULL);
        IF EXISTS (SELECT 1 FROM dcim_consoleport WHERE device_id=target_device AND name=resolved_name) THEN
            RAISE EXCEPTION 'Duplicate resolved console port name %', resolved_name USING ERRCODE='23505';
        END IF;
        new_component_id := uuidv7();
        INSERT INTO dcim_consoleport (id,custom_field_data,device_id,module_id,name,label,description,type,mark_connected)
            VALUES (new_component_id,'{}',target_device,target_module,resolved_name,resolved_label,tpl.description,tpl.type,false);
    END LOOP;

    -- Console server ports.
    FOR tpl IN SELECT * FROM dcim_consoleserverporttemplate
        WHERE (target_module IS NULL AND device_type_id=hardware_type)
           OR (target_module IS NOT NULL AND module_type_id=hardware_type) ORDER BY name
    LOOP
        resolved_name := dcim_resolve_component_placeholder(tpl.name,target_device,bay_position,target_module IS NOT NULL);
        resolved_label := dcim_resolve_component_placeholder(tpl.label,target_device,bay_position,target_module IS NOT NULL);
        IF EXISTS (SELECT 1 FROM dcim_consoleserverport WHERE device_id=target_device AND name=resolved_name) THEN
            RAISE EXCEPTION 'Duplicate resolved console server port name %', resolved_name USING ERRCODE='23505';
        END IF;
        new_component_id := uuidv7();
        INSERT INTO dcim_consoleserverport (id,custom_field_data,device_id,module_id,name,label,description,type,mark_connected)
            VALUES (new_component_id,'{}',target_device,target_module,resolved_name,resolved_label,tpl.description,tpl.type,false);
    END LOOP;

    -- Power ports. They precede power outlets because outlets reference them.
    FOR tpl IN SELECT * FROM dcim_powerporttemplate
        WHERE (target_module IS NULL AND device_type_id=hardware_type)
           OR (target_module IS NOT NULL AND module_type_id=hardware_type) ORDER BY name
    LOOP
        resolved_name := dcim_resolve_component_placeholder(tpl.name,target_device,bay_position,target_module IS NOT NULL);
        resolved_label := dcim_resolve_component_placeholder(tpl.label,target_device,bay_position,target_module IS NOT NULL);
        IF EXISTS (SELECT 1 FROM dcim_powerport WHERE device_id=target_device AND name=resolved_name) THEN
            RAISE EXCEPTION 'Duplicate resolved power port name %', resolved_name USING ERRCODE='23505';
        END IF;
        new_component_id := uuidv7();
        INSERT INTO dcim_powerport (id,custom_field_data,device_id,module_id,name,label,description,type,maximum_draw,allocated_draw,mark_connected)
            VALUES (new_component_id,'{}',target_device,target_module,resolved_name,resolved_label,tpl.description,tpl.type,tpl.maximum_draw,tpl.allocated_draw,false);
    END LOOP;

    -- Power outlets, resolving their parent power port by resolved name.
    FOR tpl IN SELECT * FROM dcim_poweroutlettemplate
        WHERE (target_module IS NULL AND device_type_id=hardware_type)
           OR (target_module IS NOT NULL AND module_type_id=hardware_type) ORDER BY name
    LOOP
        resolved_name := dcim_resolve_component_placeholder(tpl.name,target_device,bay_position,target_module IS NOT NULL);
        resolved_label := dcim_resolve_component_placeholder(tpl.label,target_device,bay_position,target_module IS NOT NULL);
        parent_power_port_id := NULL;
        IF tpl.power_port_id IS NOT NULL THEN
            SELECT dcim_resolve_component_placeholder(p.name,target_device,bay_position,target_module IS NOT NULL)
                INTO resolved_power_port
                FROM dcim_powerporttemplate p
                WHERE p.id=tpl.power_port_id
                  AND ((target_module IS NULL AND p.device_type_id=hardware_type)
                       OR (target_module IS NOT NULL AND p.module_type_id=hardware_type));
            IF resolved_power_port IS NULL THEN
                RAISE EXCEPTION 'Power outlet template % references a power port template outside its hardware type', tpl.name USING ERRCODE='23503';
            END IF;
            SELECT id INTO parent_power_port_id FROM dcim_powerport
                WHERE device_id=target_device AND name=resolved_power_port;
            IF parent_power_port_id IS NULL THEN
                RAISE EXCEPTION 'Power outlet template % could not resolve instantiated power port %', tpl.name, resolved_power_port USING ERRCODE='23503';
            END IF;
        END IF;
        IF EXISTS (SELECT 1 FROM dcim_poweroutlet WHERE device_id=target_device AND name=resolved_name) THEN
            RAISE EXCEPTION 'Duplicate resolved power outlet name %', resolved_name USING ERRCODE='23505';
        END IF;
        new_component_id := uuidv7();
        INSERT INTO dcim_poweroutlet (id,custom_field_data,device_id,module_id,name,label,description,type,feed_leg,power_port_id,mark_connected,color,status)
            VALUES (new_component_id,'{}',target_device,target_module,resolved_name,resolved_label,tpl.description,tpl.type,tpl.feed_leg,parent_power_port_id,false,tpl.color,'enabled');
    END LOOP;

    -- Interfaces. bridge_id is applied in the second pass below.
    FOR tpl IN SELECT * FROM dcim_interfacetemplate
        WHERE (target_module IS NULL AND device_type_id=hardware_type)
           OR (target_module IS NOT NULL AND module_type_id=hardware_type) ORDER BY name
    LOOP
        resolved_name := dcim_resolve_component_placeholder(tpl.name,target_device,bay_position,target_module IS NOT NULL);
        resolved_label := dcim_resolve_component_placeholder(tpl.label,target_device,bay_position,target_module IS NOT NULL);
        IF EXISTS (SELECT 1 FROM dcim_interface WHERE device_id=target_device AND name=resolved_name) THEN
            RAISE EXCEPTION 'Duplicate resolved interface name %', resolved_name USING ERRCODE='23505';
        END IF;
        new_component_id := uuidv7();
        INSERT INTO dcim_interface (id,custom_field_data,device_id,module_id,name,label,description,mark_connected,enabled,_name,type,mgmt_only,poe_mode,poe_type,rf_role)
            VALUES (new_component_id,'{}',target_device,target_module,resolved_name,resolved_label,tpl.description,false,tpl.enabled,resolved_name,tpl.type,tpl.mgmt_only,tpl.poe_mode,tpl.poe_type,tpl.rf_role);
    END LOOP;

    -- Rear ports precede front ports so template port mappings can resolve.
    FOR tpl IN SELECT * FROM dcim_rearporttemplate
        WHERE (target_module IS NULL AND device_type_id=hardware_type)
           OR (target_module IS NOT NULL AND module_type_id=hardware_type) ORDER BY name
    LOOP
        resolved_name := dcim_resolve_component_placeholder(tpl.name,target_device,bay_position,target_module IS NOT NULL);
        resolved_label := dcim_resolve_component_placeholder(tpl.label,target_device,bay_position,target_module IS NOT NULL);
        IF EXISTS (SELECT 1 FROM dcim_rearport WHERE device_id=target_device AND name=resolved_name) THEN
            RAISE EXCEPTION 'Duplicate resolved rear port name %', resolved_name USING ERRCODE='23505';
        END IF;
        new_component_id := uuidv7();
        INSERT INTO dcim_rearport (id,custom_field_data,device_id,module_id,name,label,description,type,color,positions,mark_connected)
            VALUES (new_component_id,'{}',target_device,target_module,resolved_name,resolved_label,tpl.description,tpl.type,tpl.color,tpl.positions,false);
    END LOOP;

    FOR tpl IN SELECT * FROM dcim_frontporttemplate
        WHERE (target_module IS NULL AND device_type_id=hardware_type)
           OR (target_module IS NOT NULL AND module_type_id=hardware_type) ORDER BY name
    LOOP
        resolved_name := dcim_resolve_component_placeholder(tpl.name,target_device,bay_position,target_module IS NOT NULL);
        resolved_label := dcim_resolve_component_placeholder(tpl.label,target_device,bay_position,target_module IS NOT NULL);
        IF EXISTS (SELECT 1 FROM dcim_frontport WHERE device_id=target_device AND name=resolved_name) THEN
            RAISE EXCEPTION 'Duplicate resolved front port name %', resolved_name USING ERRCODE='23505';
        END IF;
        new_component_id := uuidv7();
        INSERT INTO dcim_frontport (id,custom_field_data,device_id,module_id,name,label,description,type,color,positions,mark_connected)
            VALUES (new_component_id,'{}',target_device,target_module,resolved_name,resolved_label,tpl.description,tpl.type,tpl.color,tpl.positions,false);
    END LOOP;

    -- Replicate front/rear template mappings. PortMapping is device-scoped in
    -- the local schema, exactly like the local runtime table.
    FOR component_template IN
        SELECT m.front_port_position, m.rear_port_position, f.name AS front_name, r.name AS rear_name
        FROM dcim_porttemplatemapping m
        JOIN dcim_frontporttemplate f ON f.id=m.front_port_id
        JOIN dcim_rearporttemplate r ON r.id=m.rear_port_id
        WHERE (target_module IS NULL AND m.device_type_id=hardware_type)
           OR (target_module IS NOT NULL AND m.module_type_id=hardware_type)
        ORDER BY f.name, r.name
    LOOP
        resolved_name := dcim_resolve_component_placeholder(component_template.front_name,target_device,bay_position,target_module IS NOT NULL);
        resolved_label := dcim_resolve_component_placeholder(component_template.rear_name,target_device,bay_position,target_module IS NOT NULL);
        SELECT id INTO resolved_endpoint_id FROM dcim_frontport
            WHERE device_id=target_device AND name=resolved_name;
        IF resolved_endpoint_id IS NULL THEN
            RAISE EXCEPTION 'Port template mapping could not resolve instantiated front port %', resolved_name USING ERRCODE='23503';
        END IF;
        SELECT id INTO resolved_reference_id FROM dcim_rearport
            WHERE device_id=target_device AND name=resolved_label;
        IF resolved_reference_id IS NULL THEN
            RAISE EXCEPTION 'Port template mapping could not resolve instantiated rear port %', resolved_label USING ERRCODE='23503';
        END IF;
        INSERT INTO dcim_portmapping (id,front_port_position,rear_port_position,device_id,front_port_id,rear_port_id)
            VALUES (uuidv7(),component_template.front_port_position,component_template.rear_port_position,target_device,resolved_endpoint_id,resolved_reference_id);
    END LOOP;

    -- Device bays and inventory items are device-type only.
    IF target_module IS NULL THEN
        FOR tpl IN SELECT * FROM dcim_devicebaytemplate
            WHERE device_type_id=hardware_type ORDER BY name
        LOOP
            IF EXISTS (SELECT 1 FROM dcim_devicebay WHERE device_id=target_device AND name=tpl.name) THEN
                RAISE EXCEPTION 'Duplicate resolved device bay name %', tpl.name USING ERRCODE='23505';
            END IF;
            new_component_id := uuidv7();
            INSERT INTO dcim_devicebay (id,custom_field_data,device_id,name,label,description)
                VALUES (new_component_id,'{}',target_device,tpl.name,tpl.label,tpl.description);
        END LOOP;

        -- Refuse cyclic or cross-type inventory-item template hierarchies before
        -- creating anything, so a bad template set never leaves a partial tree.
        IF EXISTS (
            WITH RECURSIVE walk AS (
                SELECT t.id, ARRAY[t.id] AS path, false AS cycle
                FROM dcim_inventoryitemtemplate t
                WHERE t.device_type_id=hardware_type AND t.parent_id IS NULL
                UNION ALL
                SELECT c.id, w.path || c.id, c.id=ANY(w.path)
                FROM dcim_inventoryitemtemplate c
                JOIN walk w ON c.parent_id=w.id
                WHERE c.device_type_id=hardware_type AND NOT w.cycle
            ) SELECT 1 FROM walk WHERE cycle
        ) THEN
            RAISE EXCEPTION 'Inventory item template hierarchy cannot contain cycles' USING ERRCODE='23514';
        END IF;
        IF (SELECT COUNT(*) FROM dcim_inventoryitemtemplate WHERE device_type_id=hardware_type) > (
            WITH RECURSIVE walk AS (
                SELECT t.id
                FROM dcim_inventoryitemtemplate t
                WHERE t.device_type_id=hardware_type AND t.parent_id IS NULL
                UNION ALL
                SELECT c.id
                FROM dcim_inventoryitemtemplate c
                JOIN walk w ON c.parent_id=w.id
                WHERE c.device_type_id=hardware_type
            ) SELECT COUNT(DISTINCT id) FROM walk
        ) THEN
            RAISE EXCEPTION 'Inventory item template hierarchy is cyclic or its parent lies outside the instantiated hardware type' USING ERRCODE='23514';
        END IF;

        FOR item IN
            WITH RECURSIVE tree AS (
                SELECT t.*, 0 AS depth
                FROM dcim_inventoryitemtemplate t
                WHERE t.device_type_id=hardware_type AND t.parent_id IS NULL
                UNION ALL
                SELECT c.*, tr.depth + 1
                FROM dcim_inventoryitemtemplate c
                JOIN tree tr ON c.parent_id=tr.id
                WHERE c.device_type_id=hardware_type
            )
            SELECT * FROM tree ORDER BY depth, name
        LOOP
            parent_item := NULL;
            IF item.parent_id IS NOT NULL THEN
                parent_item := (item_ids->>item.parent_id::text)::uuid;
                IF parent_item IS NULL THEN
                    RAISE EXCEPTION 'Inventory item template % could not resolve its parent', item.name USING ERRCODE='23503';
                END IF;
            END IF;
            component_runtime_id := NULL;
            component_content_type := NULL;
            component_ref_name := NULL;
            IF item.component_type_id IS NOT NULL OR item.component_id IS NOT NULL THEN
                IF item.component_type_id IS NULL OR item.component_id IS NULL THEN
                    RAISE EXCEPTION 'Inventory item template % has an incomplete component reference', item.name USING ERRCODE='23514';
                END IF;
                SELECT model INTO component_model FROM inventory_content_type WHERE id=item.component_type_id AND app_label='dcim';
                IF component_model IS NULL THEN
                    RAISE EXCEPTION 'Inventory item template % references unknown component content type %', item.name, item.component_type_id USING ERRCODE='23503';
                END IF;
                CASE component_model
                        WHEN 'consoleporttemplate' THEN
                            runtime_model := 'consoleport';
                            SELECT dcim_resolve_component_placeholder(t.name,target_device,bay_position,false) INTO component_ref_name
                                FROM dcim_consoleporttemplate t WHERE t.id=item.component_id AND t.device_type_id=hardware_type;
                            IF component_ref_name IS NOT NULL THEN
                                SELECT id INTO component_runtime_id FROM dcim_consoleport
                                    WHERE device_id=target_device AND module_id IS NULL AND name=component_ref_name;
                            END IF;
                        WHEN 'consoleserverporttemplate' THEN
                            runtime_model := 'consoleserverport';
                            SELECT dcim_resolve_component_placeholder(t.name,target_device,bay_position,false) INTO component_ref_name
                                FROM dcim_consoleserverporttemplate t WHERE t.id=item.component_id AND t.device_type_id=hardware_type;
                            IF component_ref_name IS NOT NULL THEN
                                SELECT id INTO component_runtime_id FROM dcim_consoleserverport
                                    WHERE device_id=target_device AND module_id IS NULL AND name=component_ref_name;
                            END IF;
                        WHEN 'powerporttemplate' THEN
                            runtime_model := 'powerport';
                            SELECT dcim_resolve_component_placeholder(t.name,target_device,bay_position,false) INTO component_ref_name
                                FROM dcim_powerporttemplate t WHERE t.id=item.component_id AND t.device_type_id=hardware_type;
                            IF component_ref_name IS NOT NULL THEN
                                SELECT id INTO component_runtime_id FROM dcim_powerport
                                    WHERE device_id=target_device AND module_id IS NULL AND name=component_ref_name;
                            END IF;
                        WHEN 'poweroutlettemplate' THEN
                            runtime_model := 'poweroutlet';
                            SELECT dcim_resolve_component_placeholder(t.name,target_device,bay_position,false) INTO component_ref_name
                                FROM dcim_poweroutlettemplate t WHERE t.id=item.component_id AND t.device_type_id=hardware_type;
                            IF component_ref_name IS NOT NULL THEN
                                SELECT id INTO component_runtime_id FROM dcim_poweroutlet
                                    WHERE device_id=target_device AND module_id IS NULL AND name=component_ref_name;
                            END IF;
                        WHEN 'interfacetemplate' THEN
                            runtime_model := 'interface';
                            SELECT dcim_resolve_component_placeholder(t.name,target_device,bay_position,false) INTO component_ref_name
                                FROM dcim_interfacetemplate t WHERE t.id=item.component_id AND t.device_type_id=hardware_type;
                            IF component_ref_name IS NOT NULL THEN
                                SELECT id INTO component_runtime_id FROM dcim_interface
                                    WHERE device_id=target_device AND module_id IS NULL AND name=component_ref_name;
                            END IF;
                        WHEN 'frontporttemplate' THEN
                            runtime_model := 'frontport';
                            SELECT dcim_resolve_component_placeholder(t.name,target_device,bay_position,false) INTO component_ref_name
                                FROM dcim_frontporttemplate t WHERE t.id=item.component_id AND t.device_type_id=hardware_type;
                            IF component_ref_name IS NOT NULL THEN
                                SELECT id INTO component_runtime_id FROM dcim_frontport
                                    WHERE device_id=target_device AND module_id IS NULL AND name=component_ref_name;
                            END IF;
                        WHEN 'rearporttemplate' THEN
                            runtime_model := 'rearport';
                            SELECT dcim_resolve_component_placeholder(t.name,target_device,bay_position,false) INTO component_ref_name
                                FROM dcim_rearporttemplate t WHERE t.id=item.component_id AND t.device_type_id=hardware_type;
                            IF component_ref_name IS NOT NULL THEN
                                SELECT id INTO component_runtime_id FROM dcim_rearport
                                    WHERE device_id=target_device AND module_id IS NULL AND name=component_ref_name;
                            END IF;
                        WHEN 'coolingintaketemplate' THEN
                            runtime_model := 'coolingintake';
                            SELECT replace(t.name,'{module}',bay_position) INTO component_ref_name
                                FROM dcim_coolingintaketemplate t WHERE t.id=item.component_id AND t.device_type_id=hardware_type;
                            IF component_ref_name IS NOT NULL THEN
                                SELECT COUNT(*) INTO matching_rows FROM dcim_coolingintake
                                    WHERE device_id=target_device AND module_id IS NOT DISTINCT FROM target_module AND name=component_ref_name;
                                IF matching_rows > 1 THEN
                                    RAISE EXCEPTION 'Inventory item template % matches multiple instantiated cooling intakes named %', item.name, component_ref_name USING ERRCODE='23505';
                                END IF;
                                IF matching_rows = 1 THEN
                                    SELECT id INTO component_runtime_id FROM dcim_coolingintake
                                        WHERE device_id=target_device AND module_id IS NOT DISTINCT FROM target_module AND name=component_ref_name;
                                END IF;
                            END IF;
                        WHEN 'coolingoutflowtemplate' THEN
                            runtime_model := 'coolingoutflow';
                            SELECT replace(t.name,'{module}',bay_position) INTO component_ref_name
                                FROM dcim_coolingoutflowtemplate t WHERE t.id=item.component_id AND t.device_type_id=hardware_type;
                            IF component_ref_name IS NOT NULL THEN
                                SELECT COUNT(*) INTO matching_rows FROM dcim_coolingoutflow
                                    WHERE device_id=target_device AND module_id IS NOT DISTINCT FROM target_module AND name=component_ref_name;
                                IF matching_rows > 1 THEN
                                    RAISE EXCEPTION 'Inventory item template % matches multiple instantiated cooling outflows named %', item.name, component_ref_name USING ERRCODE='23505';
                                END IF;
                                IF matching_rows = 1 THEN
                                    SELECT id INTO component_runtime_id FROM dcim_coolingoutflow
                                        WHERE device_id=target_device AND module_id IS NOT DISTINCT FROM target_module AND name=component_ref_name;
                                END IF;
                            END IF;
                        ELSE
                            RAISE EXCEPTION 'Inventory item template % references a component template model that is not an allowed component target: %', item.name, component_model USING ERRCODE='23503';
                    END CASE;
                    IF component_ref_name IS NULL THEN
                        RAISE EXCEPTION 'Inventory item template % references a component template outside its hardware type', item.name USING ERRCODE='23503';
                    END IF;
                IF component_runtime_id IS NULL THEN
                    RAISE EXCEPTION 'Inventory item template % could not resolve instantiated component %', item.name, COALESCE(component_ref_name, component_model) USING ERRCODE='23503';
                END IF;
                component_content_type := (SELECT id FROM inventory_content_type WHERE app_label='dcim' AND model=runtime_model);
                IF component_content_type IS NULL THEN
                    RAISE EXCEPTION 'Missing inventory content type for instantiated component model %', runtime_model USING ERRCODE='23503';
                END IF;
            END IF;
            IF EXISTS (
                SELECT 1 FROM dcim_inventoryitem
                WHERE device_id=target_device AND parent_id IS NOT DISTINCT FROM parent_item AND name=item.name
            ) THEN
                RAISE EXCEPTION 'Duplicate resolved inventory item name %', item.name USING ERRCODE='23505';
            END IF;
            new_component_id := uuidv7();
            INSERT INTO dcim_inventoryitem (id,custom_field_data,device_id,parent_id,role_id,manufacturer_id,name,label,part_id,serial,asset_tag,discovered,description,status,component_type_id,component_id,lft,rght,tree_id,level)
                VALUES (new_component_id,'{}',target_device,parent_item,item.role_id,item.manufacturer_id,item.name,item.label,item.part_id,'',NULL,false,item.description,'active',component_content_type,component_runtime_id,0,0,0,0);
            item_ids := item_ids || jsonb_build_object(item.id::text,new_component_id);
        END LOOP;
    END IF;

    -- Interface bridges are applied after every interface exists. Parents and
    -- channels are not part of the local interface schema and are not invented.
    FOR tpl IN SELECT * FROM dcim_interfacetemplate
        WHERE bridge_id IS NOT NULL
          AND ((target_module IS NULL AND device_type_id=hardware_type)
               OR (target_module IS NOT NULL AND module_type_id=hardware_type)) ORDER BY name
    LOOP
        resolved_name := dcim_resolve_component_placeholder(tpl.name,target_device,bay_position,target_module IS NOT NULL);
        SELECT id INTO resolved_endpoint_id FROM dcim_interface
            WHERE device_id=target_device AND name=resolved_name;
        IF resolved_endpoint_id IS NULL THEN
            RAISE EXCEPTION 'Interface template % could not resolve its instantiated interface', tpl.name USING ERRCODE='23503';
        END IF;
        SELECT dcim_resolve_component_placeholder(b.name,target_device,bay_position,target_module IS NOT NULL)
            INTO resolved_label
            FROM dcim_interfacetemplate b
            WHERE b.id=tpl.bridge_id
              AND ((target_module IS NULL AND b.device_type_id=hardware_type)
                   OR (target_module IS NOT NULL AND b.module_type_id=hardware_type));
        IF resolved_label IS NULL THEN
            RAISE EXCEPTION 'Interface template % references a bridge template outside its hardware type', tpl.name USING ERRCODE='23503';
        END IF;
        SELECT id INTO resolved_reference_id FROM dcim_interface
            WHERE device_id=target_device AND name=resolved_label;
        IF resolved_reference_id IS NULL THEN
            RAISE EXCEPTION 'Interface template % could not resolve instantiated bridge %', tpl.name, resolved_label USING ERRCODE='23503';
        END IF;
        IF resolved_reference_id = resolved_endpoint_id THEN
            RAISE EXCEPTION 'Interface % cannot bridge itself', resolved_name USING ERRCODE='23514';
        END IF;
        UPDATE dcim_interface SET bridge_id=resolved_reference_id WHERE id=resolved_endpoint_id;
    END LOOP;

    RETURN NEW;
END
$$;
