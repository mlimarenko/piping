-- A run is registered before inventory mutation. The receipt is written in the
-- same transaction as inventory and the terminal sync job.
CREATE TABLE sync_inventory_transfer_target (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    target_id uuid NOT NULL UNIQUE DEFAULT uuidv7()
);

INSERT INTO sync_inventory_transfer_target (singleton) VALUES (true);

CREATE TABLE sync_inventory_transfer_run (
    run_id uuid PRIMARY KEY,
    descriptor_digest text NOT NULL CHECK (descriptor_digest ~ '^[0-9a-f]{64}$'),
    descriptor bytea NOT NULL CHECK (octet_length(descriptor) > 0),
    receipt jsonb CHECK (receipt IS NULL OR jsonb_typeof(receipt) = 'object'),
    created_at timestamptz NOT NULL DEFAULT now(),
    committed_at timestamptz,
    CHECK ((receipt IS NULL) = (committed_at IS NULL))
);
