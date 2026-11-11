-- PIP-551: provenance for collected inventory facts.
--
-- One row per object field the collector owns. `observed_value` is the value
-- the collector last wrote (JSON), `observed_at` is when the source observed
-- it, and (`source_kind`, `source_id`) is the run source. A field whose current
-- value no longer equals `observed_value` was edited by hand and is not
-- overwritten by the next collection.
--
-- `absent` records an explicit "the source did not report this field" without
-- deleting or zeroing the stored value. Absence is a statement about the
-- observation, not about the object.
CREATE TABLE inventory_collected_field (
    inventory_kind text NOT NULL
        CHECK (inventory_kind IN ('device', 'virtual_machine')),
    inventory_id uuid NOT NULL,
    field_key text NOT NULL,
    source_kind text NOT NULL,
    source_id uuid NOT NULL,
    observed_at timestamp with time zone NOT NULL,
    observed_value jsonb,
    absent boolean NOT NULL DEFAULT false,
    updated_at timestamp with time zone NOT NULL DEFAULT now(),
    PRIMARY KEY (inventory_kind, inventory_id, field_key)
);

CREATE INDEX inventory_collected_field_inventory_idx
    ON inventory_collected_field (inventory_kind, inventory_id);
