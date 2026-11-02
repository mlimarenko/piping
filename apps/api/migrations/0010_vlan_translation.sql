-- NetBox 4.7.1: VLANTranslationPolicy is PrimaryModel; VLANTranslationRule is
-- NetBoxModel with Tags, CustomFields and ChangeLogging features, not PrimaryModel.
-- Preserve registry UUIDs and rows. Historical dates are unknown, not NOW().
SELECT dcim_acquire_hardware_graph();

INSERT INTO inventory_content_type (app_label, model)
VALUES ('ipam', 'vlantranslationpolicy'), ('ipam', 'vlantranslationrule')
ON CONFLICT (app_label, model) DO NOTHING;

ALTER TABLE ipam_vlantranslationpolicy
    ADD COLUMN tags text[] NOT NULL DEFAULT ARRAY[]::text[],
    ADD COLUMN created timestamptz,
    ADD COLUMN last_updated timestamptz;
ALTER TABLE ipam_vlantranslationrule
    ADD COLUMN tags text[] NOT NULL DEFAULT ARRAY[]::text[],
    ADD COLUMN created timestamptz,
    ADD COLUMN last_updated timestamptz;

CREATE TRIGGER tag_usage_lock BEFORE INSERT OR UPDATE OR DELETE ON ipam_vlantranslationpolicy
    FOR EACH STATEMENT EXECUTE FUNCTION inventory_tag_usage_lock();
CREATE TRIGGER tag_membership_guard BEFORE INSERT OR UPDATE ON ipam_vlantranslationpolicy
    FOR EACH ROW EXECUTE FUNCTION inventory_tag_membership_guard();
CREATE TRIGGER tag_usage_lock BEFORE INSERT OR UPDATE OR DELETE ON ipam_vlantranslationrule
    FOR EACH STATEMENT EXECUTE FUNCTION inventory_tag_usage_lock();
CREATE TRIGGER tag_membership_guard BEFORE INSERT OR UPDATE ON ipam_vlantranslationrule
    FOR EACH ROW EXECUTE FUNCTION inventory_tag_membership_guard();

-- Validate existing rows too: bad imported VIDs must abort the upgrade, never
-- be silently clamped, deleted or renumbered.
ALTER TABLE ipam_vlantranslationrule
    DROP CONSTRAINT ipam_vlantranslationrule_local_vid_check,
    DROP CONSTRAINT ipam_vlantranslationrule_remote_vid_check,
    ADD CONSTRAINT ipam_vlantranslationrule_local_vid_check CHECK (local_vid BETWEEN 1 AND 4094),
    ADD CONSTRAINT ipam_vlantranslationrule_remote_vid_check CHECK (remote_vid BETWEEN 1 AND 4094),
    DROP CONSTRAINT ipam_vlantranslation_policy_id_09157735_fk_ipam_vlan,
    ADD CONSTRAINT ipam_vlantranslation_policy_id_09157735_fk_ipam_vlan
        FOREIGN KEY (policy_id) REFERENCES ipam_vlantranslationpolicy(id)
        ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED;

-- Interface foreign keys remain NO ACTION (NetBox PROTECT); the canonical
-- inventory delete guard projects those references before deleting a policy.
-- Reuse the hardware graph owner, including its catalog-before-graph ordering,
-- for direct SQL and FK cascades as well as native repository transactions.
CREATE TRIGGER hardware_graph_lock BEFORE INSERT OR UPDATE OR DELETE ON ipam_vlantranslationpolicy
    FOR EACH STATEMENT EXECUTE FUNCTION dcim_lock_cooling_graph();
CREATE TRIGGER hardware_graph_lock BEFORE INSERT OR UPDATE OR DELETE ON ipam_vlantranslationrule
    FOR EACH STATEMENT EXECUTE FUNCTION dcim_lock_cooling_graph();
CREATE TRIGGER hardware_graph_lock BEFORE INSERT OR UPDATE OR DELETE ON dcim_interface
    FOR EACH STATEMENT EXECUTE FUNCTION dcim_lock_cooling_graph();
CREATE TRIGGER hardware_graph_lock BEFORE INSERT OR UPDATE OR DELETE ON virtualization_vminterface
    FOR EACH STATEMENT EXECUTE FUNCTION dcim_lock_cooling_graph();
-- WirelessLink deletion SET NULLs Interface references. Direct writers must
-- enter the graph before locking that parent, not from the later child trigger.
CREATE TRIGGER hardware_graph_lock BEFORE INSERT OR UPDATE OR DELETE ON wireless_wirelesslink
    FOR EACH STATEMENT EXECUTE FUNCTION dcim_lock_cooling_graph();
