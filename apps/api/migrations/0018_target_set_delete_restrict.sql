-- A target set must not silently remove the assignments that reference it.
-- Deleting the set while assignments still point at it is refused
-- (409 DELETE_BLOCKED) instead of cascading the assignment rows away.
ALTER TABLE playbook_assignment
    DROP CONSTRAINT playbook_assignment_target_set_id_fkey,
    ADD CONSTRAINT playbook_assignment_target_set_id_fkey
        FOREIGN KEY (target_set_id) REFERENCES playbook_target_set(id) ON DELETE RESTRICT;
