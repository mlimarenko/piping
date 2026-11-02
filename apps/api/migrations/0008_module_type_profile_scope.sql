-- ModuleTypeProfile inherits PrimaryModel/NetBoxFeatureSet.CustomFieldsMixin
-- in NetBox 4.7.1 (netbox/dcim/models/modules.py and netbox/models/__init__.py).
-- Preserve an existing content-type UUID and every scope/binding referring to it.
INSERT INTO inventory_content_type (app_label, model)
VALUES ('dcim', 'moduletypeprofile')
ON CONFLICT (app_label, model) DO NOTHING;

-- Existing rows have no observed local history. Never backfill invented dates
-- from migration time or piping_synced_at; provider history belongs to bindings.
ALTER TABLE dcim_moduletypeprofile
    ADD COLUMN tags text[] NOT NULL DEFAULT ARRAY[]::text[],
    ADD COLUMN created timestamptz,
    ADD COLUMN last_updated timestamptz;
CREATE TRIGGER tag_usage_lock BEFORE INSERT OR UPDATE OR DELETE ON dcim_moduletypeprofile
    FOR EACH STATEMENT EXECUTE FUNCTION inventory_tag_usage_lock();
CREATE TRIGGER tag_membership_guard BEFORE INSERT OR UPDATE ON dcim_moduletypeprofile
    FOR EACH ROW EXECUTE FUNCTION inventory_tag_membership_guard();
