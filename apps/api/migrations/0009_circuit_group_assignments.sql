-- CircuitGroupAssignment is a tagged/custom-field ChangeLoggedModel, not an
-- AdminModel. Preserve existing catalogue identities and unknown local history.
INSERT INTO inventory_content_type(app_label,model) VALUES
    ('circuits','circuitgroupassignment'),
    ('circuits','circuit'), ('circuits','virtualcircuit')
ON CONFLICT(app_label,model) DO NOTHING;
ALTER TABLE circuits_circuitgroupassignment
    ADD COLUMN created timestamptz,
    ADD COLUMN last_updated timestamptz;

CREATE FUNCTION circuits_lock_group_assignments() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    -- Mixed sync writers already hold this catalogue lock exclusively before
    -- target planning. Preserve the same catalogue-before-graph lock order.
    PERFORM pg_advisory_xact_lock_shared(hashtextextended('inventory-tag-catalog',0));
    PERFORM pg_advisory_xact_lock(hashtextextended('circuits-group-assignments',0));
    RETURN NULL;
END $$;
CREATE TRIGGER circuit_group_assignment_lock BEFORE INSERT OR DELETE OR UPDATE OF member_id,member_type_id,group_id
    ON circuits_circuitgroupassignment FOR EACH STATEMENT EXECUTE FUNCTION circuits_lock_group_assignments();
CREATE TRIGGER circuit_group_assignment_lock BEFORE DELETE OR UPDATE OF id ON circuits_circuit
    FOR EACH STATEMENT EXECUTE FUNCTION circuits_lock_group_assignments();
CREATE TRIGGER circuit_group_assignment_lock BEFORE DELETE OR UPDATE OF id ON circuits_virtualcircuit
    FOR EACH STATEMENT EXECUTE FUNCTION circuits_lock_group_assignments();
CREATE TRIGGER circuit_group_assignment_lock BEFORE DELETE ON circuits_circuitgroup
    FOR EACH STATEMENT EXECUTE FUNCTION circuits_lock_group_assignments();

-- Refuse pre-existing invalid GFKs; do not detach or rewrite imported members.
DO $$ BEGIN
    IF EXISTS (
        SELECT 1 FROM circuits_circuitgroupassignment a
        LEFT JOIN inventory_content_type ct ON ct.id=a.member_type_id
        LEFT JOIN circuits_circuit c ON ct.app_label='circuits' AND ct.model='circuit' AND c.id=a.member_id
        LEFT JOIN circuits_virtualcircuit v ON ct.app_label='circuits' AND ct.model='virtualcircuit' AND v.id=a.member_id
        WHERE (c.id IS NULL AND v.id IS NULL)
           OR (a.priority IS NOT NULL AND a.priority NOT IN ('','primary','secondary','tertiary','inactive'))
    ) THEN
        RAISE EXCEPTION 'Existing circuit group assignments have invalid members or priorities' USING ERRCODE='23514';
    END IF;
END $$;
ALTER TABLE circuits_circuitgroupassignment
    ADD CONSTRAINT circuit_group_assignment_member_type_fk FOREIGN KEY(member_type_id)
        REFERENCES inventory_content_type(id) ON DELETE RESTRICT,
    ADD CONSTRAINT circuit_group_assignment_priority_check CHECK(priority IS NULL OR priority IN ('','primary','secondary','tertiary','inactive'));
ALTER TABLE circuits_circuitgroupassignment DROP CONSTRAINT circuits_circuitgrou_group_id_1a7b6580_fk_circuits_;
ALTER TABLE circuits_circuitgroupassignment ADD CONSTRAINT circuits_circuitgrou_group_id_1a7b6580_fk_circuits_
    FOREIGN KEY(group_id) REFERENCES circuits_circuitgroup(id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED;

CREATE FUNCTION circuits_validate_group_member() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE member_model text;
BEGIN
    SELECT model INTO member_model FROM inventory_content_type WHERE id=NEW.member_type_id AND app_label='circuits';
    IF member_model='circuit' THEN
        PERFORM id FROM circuits_circuit WHERE id=NEW.member_id FOR KEY SHARE;
    ELSIF member_model='virtualcircuit' THEN
        PERFORM id FROM circuits_virtualcircuit WHERE id=NEW.member_id FOR KEY SHARE;
    ELSE
        RAISE EXCEPTION 'Circuit group member must be a physical or virtual circuit' USING ERRCODE='23514';
    END IF;
    IF NOT FOUND THEN
        RAISE EXCEPTION 'Circuit group member does not exist' USING ERRCODE='23503';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER circuit_group_member_guard BEFORE INSERT OR UPDATE OF member_id,member_type_id
    ON circuits_circuitgroupassignment FOR EACH ROW EXECUTE FUNCTION circuits_validate_group_member();
CREATE FUNCTION circuits_protect_group_member() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='UPDATE' THEN
        IF NEW.id=OLD.id THEN RETURN NEW; END IF;
    END IF;
    IF EXISTS (SELECT 1 FROM circuits_circuitgroupassignment a JOIN inventory_content_type ct ON ct.id=a.member_type_id
        WHERE a.member_id=OLD.id AND ct.app_label='circuits' AND ct.model=TG_ARGV[0]) THEN
        RAISE EXCEPTION 'Circuit is referenced by circuit group assignments' USING ERRCODE='23503';
    END IF;
    IF TG_OP='DELETE' THEN RETURN OLD; END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER circuit_group_member_guard BEFORE DELETE OR UPDATE OF id ON circuits_circuit
    FOR EACH ROW EXECUTE FUNCTION circuits_protect_group_member('circuit');
CREATE TRIGGER circuit_group_member_guard BEFORE DELETE OR UPDATE OF id ON circuits_virtualcircuit
    FOR EACH ROW EXECUTE FUNCTION circuits_protect_group_member('virtualcircuit');
