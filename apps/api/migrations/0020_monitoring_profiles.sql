-- PIP-244 monitoring profiles. One transaction, forward-only, no dual write.
--
-- Every legacy `monitoring_exporter_assignment` row becomes one single-item
-- profile with a profile assignment that keeps the assignment id, scope,
-- weight, enabled flag and explicit target. The old table is dropped in the
-- same transaction, so there is exactly one scrape resolution path afterwards.
-- Built-in generic profiles are seeded only when their exporters exist; they
-- get no assignments, so seeding never changes effective scrape output.

CREATE TABLE monitoring_profile (
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(255) NOT NULL,
    slug character varying(128) NOT NULL,
    description text,
    metric_set_id uuid REFERENCES monitoring_metric_set(id) ON DELETE SET NULL,
    query_source_id uuid REFERENCES monitoring_query_source(id) ON DELETE SET NULL,
    alert_source_id uuid REFERENCES monitoring_alert_source(id) ON DELETE SET NULL,
    enabled boolean DEFAULT true NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT monitoring_profile_pkey PRIMARY KEY (id),
    CONSTRAINT monitoring_profile_name_check CHECK (btrim(name) <> ''),
    CONSTRAINT monitoring_profile_slug_key UNIQUE (slug)
);

CREATE TABLE monitoring_profile_item (
    id uuid DEFAULT uuidv7() NOT NULL,
    profile_id uuid NOT NULL REFERENCES monitoring_profile(id) ON DELETE CASCADE,
    exporter_id uuid NOT NULL REFERENCES monitoring_exporter(id) ON DELETE RESTRICT,
    job character varying(128),
    scheme character varying(16),
    port integer,
    metrics_path character varying(255),
    labels jsonb DEFAULT '{}'::jsonb NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    sort_order integer DEFAULT 0 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT monitoring_profile_item_pkey PRIMARY KEY (id),
    CONSTRAINT monitoring_profile_item_labels_object_check CHECK (jsonb_typeof(labels) = 'object'),
    CONSTRAINT monitoring_profile_item_port_check CHECK (port IS NULL OR (port > 0 AND port <= 65535)),
    CONSTRAINT monitoring_profile_item_scheme_check CHECK (scheme IS NULL OR scheme IN ('http', 'https'))
);

-- Exact duplicate item identity within a profile: same exporter and same job.
CREATE UNIQUE INDEX monitoring_profile_item_identity_key
    ON monitoring_profile_item (profile_id, exporter_id, COALESCE(job, ''));
CREATE INDEX monitoring_profile_item_order_idx
    ON monitoring_profile_item (profile_id, sort_order, id);

CREATE TABLE monitoring_profile_assignment (
    id uuid DEFAULT uuidv7() NOT NULL,
    profile_id uuid NOT NULL REFERENCES monitoring_profile(id) ON DELETE RESTRICT,
    scope_type character varying(64) NOT NULL,
    scope_id uuid,
    weight integer DEFAULT 0 NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    target character varying(255),
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT monitoring_profile_assignment_pkey PRIMARY KEY (id),
    CONSTRAINT monitoring_profile_assignment_scope_check CHECK (
        (scope_type = 'global' AND scope_id IS NULL)
        OR (scope_type <> 'global' AND scope_id IS NOT NULL)
    )
);

CREATE UNIQUE INDEX monitoring_profile_assignment_scope_key
    ON monitoring_profile_assignment (
        profile_id,
        scope_type,
        COALESCE(scope_id, '00000000-0000-0000-0000-000000000000'::uuid)
    );
CREATE INDEX monitoring_profile_assignment_scope_idx
    ON monitoring_profile_assignment (scope_type, scope_id, enabled, weight);

-- The migration source keeps the legacy row and the profile identity it maps
-- to in one place. It is dropped with the migration transaction.
CREATE TEMPORARY TABLE monitoring_profile_migration_source ON COMMIT DROP AS
SELECT
    a.id AS assignment_id,
    a.exporter_id,
    a.scope_type,
    a.scope_id,
    a.weight,
    a.enabled,
    a.target,
    a.job,
    a.scheme,
    a.port,
    a.metrics_path,
    a.labels,
    COALESCE(NULLIF(btrim(a.name), ''), e.name) AS profile_name,
    LEFT(
        COALESCE(
            NULLIF(
                btrim(
                    regexp_replace(
                        regexp_replace(
                            lower(COALESCE(NULLIF(btrim(a.name), ''), e.name)),
                            '[^a-z0-9]+',
                            '-',
                            'g'
                        ),
                        '(^-+)|(-+$)',
                        '',
                        'g'
                    ),
                    '-'
                ),
                ''
            ),
            'profile'
        ),
        95
    ) || '-' || replace(a.id::text, '-', '') AS profile_slug
FROM monitoring_exporter_assignment a
JOIN monitoring_exporter e ON e.id = a.exporter_id;

INSERT INTO monitoring_profile (name, slug, enabled)
SELECT profile_name, profile_slug, true
FROM monitoring_profile_migration_source;

INSERT INTO monitoring_profile_item
    (profile_id, exporter_id, job, scheme, port, metrics_path, labels,
     enabled, sort_order)
SELECT
    p.id,
    s.exporter_id,
    s.job,
    s.scheme,
    s.port,
    s.metrics_path,
    s.labels,
    true,
    0
FROM monitoring_profile_migration_source s
JOIN monitoring_profile p ON p.slug = s.profile_slug;

INSERT INTO monitoring_profile_assignment
    (id, profile_id, scope_type, scope_id, weight, enabled, target)
SELECT
    s.assignment_id,
    p.id,
    s.scope_type,
    s.scope_id,
    s.weight,
    s.enabled,
    s.target
FROM monitoring_profile_migration_source s
JOIN monitoring_profile p ON p.slug = s.profile_slug;

DROP TABLE monitoring_exporter_assignment;

-- Generic built-in profiles. They are created only when at least one
-- referenced exporter exists, and only items whose exporter exists are added.
-- No assignment is seeded: these profiles do not change effective scrapes.
INSERT INTO monitoring_profile (name, slug)
SELECT seed.name, seed.slug
FROM (
    VALUES
        ('Linux baseline', 'linux-baseline', ARRAY['node', 'process', 'smartctl']::text[]),
        ('Docker host', 'docker-host', ARRAY['node', 'docker']::text[]),
        ('Web nginx', 'web-nginx', ARRAY['nginx']::text[]),
        ('Postgres', 'postgres', ARRAY['postgres']::text[]),
        ('Network blackbox', 'network-blackbox', ARRAY['blackbox-interface']::text[]),
        ('K8s node', 'k8s-node', ARRAY['node', 'docker']::text[])
) AS seed(name, slug, exporter_slugs)
WHERE EXISTS (
    SELECT 1
    FROM monitoring_exporter e
    WHERE e.slug = ANY(seed.exporter_slugs)
)
ON CONFLICT (slug) DO NOTHING;

INSERT INTO monitoring_profile_item
    (profile_id, exporter_id, job, scheme, port, metrics_path, labels,
     enabled, sort_order)
SELECT
    p.id,
    e.id,
    NULL,
    NULL,
    NULL,
    NULL,
    '{}'::jsonb,
    true,
    (array_position(seed.exporter_slugs, e.slug) - 1)::integer
FROM (
    VALUES
        ('linux-baseline', ARRAY['node', 'process', 'smartctl']::text[]),
        ('docker-host', ARRAY['node', 'docker']::text[]),
        ('web-nginx', ARRAY['nginx']::text[]),
        ('postgres', ARRAY['postgres']::text[]),
        ('network-blackbox', ARRAY['blackbox-interface']::text[]),
        ('k8s-node', ARRAY['node', 'docker']::text[])
) AS seed(slug, exporter_slugs)
JOIN monitoring_profile p ON p.slug = seed.slug
JOIN monitoring_exporter e ON e.slug = ANY(seed.exporter_slugs)
WHERE NOT EXISTS (
    SELECT 1
    FROM monitoring_profile_item i
    WHERE i.profile_id = p.id AND i.exporter_id = e.id
);
