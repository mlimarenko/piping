-- Preserve catalogue identities and imported history. Unknown local dates stay NULL.
SELECT dcim_acquire_hardware_graph();
SELECT pg_advisory_xact_lock(hashtextextended('circuits-group-assignments',0));

INSERT INTO inventory_content_type(app_label,model) VALUES
    ('circuits','virtualcircuittermination'), ('vpn','l2vpn'), ('vpn','l2vpntermination'),
    ('dcim','interface'), ('virtualization','vminterface'), ('ipam','vlan')
ON CONFLICT(app_label,model) DO NOTHING;

ALTER TABLE circuits_virtualcircuittermination ADD COLUMN created timestamptz, ADD COLUMN last_updated timestamptz;
ALTER TABLE vpn_l2vpn ADD COLUMN created timestamptz, ADD COLUMN last_updated timestamptz;
ALTER TABLE vpn_l2vpntermination ADD COLUMN created timestamptz, ADD COLUMN last_updated timestamptz;
-- Tags and their canonical membership/usage triggers already exist from 0006.
ALTER TABLE circuits_virtualcircuittermination
    ADD CONSTRAINT virtual_circuit_termination_role_check CHECK(role IN ('peer','hub','spoke')),
    DROP CONSTRAINT circuits_virtualcirc_interface_id_d764b232_fk_dcim_inte,
    ADD CONSTRAINT circuits_virtualcirc_interface_id_d764b232_fk_dcim_inte
        FOREIGN KEY(interface_id) REFERENCES dcim_interface(id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
    DROP CONSTRAINT circuits_virtualcirc_virtual_circuit_id_ca588886_fk_circuits_,
    ADD CONSTRAINT circuits_virtualcirc_virtual_circuit_id_ca588886_fk_circuits_
        FOREIGN KEY(virtual_circuit_id) REFERENCES circuits_virtualcircuit(id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED;
ALTER TABLE vpn_l2vpn
    ADD CONSTRAINT l2vpn_status_check CHECK(status IN ('active','planned','decommissioning')),
    ADD CONSTRAINT l2vpn_type_check CHECK(type IN ('vpls','vpws','epl','evpl','ep-lan','evp-lan','ep-tree','evp-tree','vxlan','vxlan-evpn','mpls-evpn','pbb-evpn','evpn-vpws','spb'));
ALTER TABLE vpn_l2vpntermination
    DROP CONSTRAINT vpn_l2vpntermination_l2vpn_id_f5367bbe_fk_vpn_l2vpn_id,
    ADD CONSTRAINT vpn_l2vpntermination_l2vpn_id_f5367bbe_fk_vpn_l2vpn_id
        FOREIGN KEY(l2vpn_id) REFERENCES vpn_l2vpn(id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
    ADD CONSTRAINT l2vpn_termination_content_type_fk FOREIGN KEY(assigned_object_type_id)
        REFERENCES inventory_content_type(id) DEFERRABLE INITIALLY DEFERRED;
-- The existing global endpoint uniqueness and VCT Interface uniqueness remain.
ALTER TABLE vpn_l2vpn_export_targets
    DROP CONSTRAINT ipam_l2vpn_export_ta_routetarget_id_5ccb758b_fk_ipam_rout,
    ADD CONSTRAINT ipam_l2vpn_export_ta_routetarget_id_5ccb758b_fk_ipam_rout FOREIGN KEY(routetarget_id)
        REFERENCES ipam_routetarget(id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
    DROP CONSTRAINT ipam_l2vpn_export_targets_l2vpn_id_8749bbe8_fk_ipam_l2vpn_id,
    ADD CONSTRAINT ipam_l2vpn_export_targets_l2vpn_id_8749bbe8_fk_ipam_l2vpn_id FOREIGN KEY(l2vpn_id)
        REFERENCES vpn_l2vpn(id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED;
ALTER TABLE vpn_l2vpn_import_targets
    DROP CONSTRAINT ipam_l2vpn_import_ta_routetarget_id_58a188b2_fk_ipam_rout,
    ADD CONSTRAINT ipam_l2vpn_import_ta_routetarget_id_58a188b2_fk_ipam_rout FOREIGN KEY(routetarget_id)
        REFERENCES ipam_routetarget(id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
    DROP CONSTRAINT ipam_l2vpn_import_targets_l2vpn_id_731f5bb4_fk_ipam_l2vpn_id,
    ADD CONSTRAINT ipam_l2vpn_import_targets_l2vpn_id_731f5bb4_fk_ipam_l2vpn_id FOREIGN KEY(l2vpn_id)
        REFERENCES vpn_l2vpn(id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED;

-- This trigger sorts before hardware_graph_lock on VirtualCircuit. Enter the
-- hardware graph inside the existing group owner too, never group -> hardware.
CREATE OR REPLACE FUNCTION circuits_lock_group_assignments() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM dcim_acquire_hardware_graph();
    PERFORM pg_advisory_xact_lock(hashtextextended('circuits-group-assignments',0));
    RETURN NULL;
END $$;
DO $$ DECLARE subject text; BEGIN
    FOREACH subject IN ARRAY ARRAY['circuits_virtualcircuittermination','circuits_virtualcircuit',
        'vpn_l2vpn','vpn_l2vpntermination','ipam_vlan','ipam_routetarget',
        'vpn_l2vpn_import_targets','vpn_l2vpn_export_targets','inventory_content_type',
        'tenancy_contactassignment'] LOOP
        EXECUTE format('CREATE TRIGGER hardware_graph_lock BEFORE INSERT OR UPDATE OR DELETE ON %I FOR EACH STATEMENT EXECUTE FUNCTION dcim_lock_cooling_graph()',subject);
    END LOOP;
END $$;

-- Validate the final graph, not transient batch order. Deferred native FKs must
-- continue to allow children before parents during full/selective replay.
CREATE FUNCTION inventory_validate_termination_graph() RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS (SELECT 1 FROM circuits_virtualcircuittermination t
        LEFT JOIN dcim_interface i ON i.id=t.interface_id
        WHERE i.id IS NULL OR i.type NOT IN ('virtual','lag','bridge')) THEN
        RAISE EXCEPTION 'Virtual circuit termination requires a virtual interface' USING ERRCODE='23514';
    END IF;
    IF EXISTS (SELECT 1 FROM vpn_l2vpntermination t
        LEFT JOIN inventory_content_type ct ON ct.id=t.assigned_object_type_id
        LEFT JOIN dcim_interface i ON ct.app_label='dcim' AND ct.model='interface' AND i.id=t.assigned_object_id
        LEFT JOIN virtualization_vminterface v ON ct.app_label='virtualization' AND ct.model='vminterface' AND v.id=t.assigned_object_id
        LEFT JOIN ipam_vlan l ON ct.app_label='ipam' AND ct.model='vlan' AND l.id=t.assigned_object_id
        WHERE i.id IS NULL AND v.id IS NULL AND l.id IS NULL) THEN
        RAISE EXCEPTION 'L2VPN termination requires an existing supported typed endpoint' USING ERRCODE='23503';
    END IF;
    IF EXISTS (SELECT 1 FROM vpn_l2vpn p JOIN vpn_l2vpntermination t ON t.l2vpn_id=p.id
        WHERE p.type IN ('vpws','epl','ep-lan','ep-tree') GROUP BY p.id HAVING count(*)>2) THEN
        RAISE EXCEPTION 'Point-to-point L2VPN cannot have more than two terminations' USING ERRCODE='23514';
    END IF;
    -- ContactsMixin owns only this typed GenericRelation. Existing contact
    -- target kinds retain their own contracts; do not reinterpret their IDs.
    IF EXISTS (SELECT 1 FROM tenancy_contactassignment a
        JOIN inventory_content_type ct ON ct.id=a.object_type_id
        LEFT JOIN vpn_l2vpn p ON p.id=a.object_id
        WHERE ct.app_label='vpn' AND ct.model='l2vpn' AND p.id IS NULL) THEN
        RAISE EXCEPTION 'L2VPN contact assignment requires an existing L2VPN' USING ERRCODE='23503';
    END IF;
END $$;
-- Existing invalid imported history aborts the transaction; never detach,
-- renumber, truncate or invent dates to make the upgrade pass.
SELECT inventory_validate_termination_graph();
CREATE FUNCTION inventory_check_termination_graph() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM inventory_validate_termination_graph();
    RETURN NULL;
END $$;
DO $$ DECLARE subject text; BEGIN
    FOREACH subject IN ARRAY ARRAY['circuits_virtualcircuittermination','dcim_interface',
        'virtualization_vminterface','ipam_vlan','vpn_l2vpn','vpn_l2vpntermination','inventory_content_type',
        'tenancy_contactassignment'] LOOP
        -- Deletion only removes edges (FK/typed cascades below). Checking a child
        -- DELETE mid-cascade would reject a valid endpoint delete in IMMEDIATE
        -- mode before its other GenericRelation children have been removed.
        EXECUTE format('CREATE CONSTRAINT TRIGGER termination_graph_valid AFTER INSERT OR UPDATE ON %I DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION inventory_check_termination_graph()',subject);
    END LOOP;
END $$;

-- GenericRelations delete children just like the upstream endpoint owners.
-- The native delete guard discovers this same typed edge before retiring secrets.
CREATE FUNCTION inventory_cascade_l2vpn_endpoint() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM vpn_l2vpntermination t USING inventory_content_type ct
        WHERE ct.id=t.assigned_object_type_id AND ct.app_label=TG_ARGV[0]
          AND ct.model=TG_ARGV[1] AND t.assigned_object_id=OLD.id;
    RETURN OLD;
END $$;
CREATE TRIGGER l2vpn_endpoint_cascade AFTER DELETE ON dcim_interface
    FOR EACH ROW EXECUTE FUNCTION inventory_cascade_l2vpn_endpoint('dcim','interface');
CREATE TRIGGER l2vpn_endpoint_cascade AFTER DELETE ON virtualization_vminterface
    FOR EACH ROW EXECUTE FUNCTION inventory_cascade_l2vpn_endpoint('virtualization','vminterface');
CREATE TRIGGER l2vpn_endpoint_cascade AFTER DELETE ON ipam_vlan
    FOR EACH ROW EXECUTE FUNCTION inventory_cascade_l2vpn_endpoint('ipam','vlan');

-- The same graph is held before both parent deletion and contact statement row
-- locks. A concurrent insert/update cannot recreate an orphan after the scan:
-- the final-state validator rejects it if deletion committed first.
CREATE FUNCTION inventory_cascade_l2vpn_contacts() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    DELETE FROM tenancy_contactassignment a USING inventory_content_type ct
        WHERE ct.id=a.object_type_id AND ct.app_label='vpn' AND ct.model='l2vpn'
          AND a.object_id=OLD.id;
    RETURN OLD;
END $$;
CREATE TRIGGER l2vpn_contact_cascade AFTER DELETE ON vpn_l2vpn
    FOR EACH ROW EXECUTE FUNCTION inventory_cascade_l2vpn_contacts();
