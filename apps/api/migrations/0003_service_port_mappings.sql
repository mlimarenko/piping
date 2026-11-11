-- SQLx executes this migration in one transaction. Lock all sources before
-- validation so a concurrent writer cannot escape the preservation checks.
LOCK TABLE public.ipam_service, public.ipam_servicetemplate,
    public.ipam_service_port_mapping IN ACCESS EXCLUSIVE MODE;

ALTER TABLE public.ipam_service ADD COLUMN port_mappings text[];
ALTER TABLE public.ipam_servicetemplate ADD COLUMN port_mappings text[];

DO $migration$
DECLARE
    source record;
    native_mappings text[];
    relation_mappings text[];
BEGIN
    IF EXISTS (
        SELECT 1 FROM public.ipam_service_port_mapping m
        LEFT JOIN public.ipam_service s ON s.id = m.service_id
        WHERE s.id IS NULL OR m.protocol NOT IN ('tcp', 'udp', 'sctp')
            OR m.protocol IS NULL OR m.port IS NULL OR m.port NOT BETWEEN 1 AND 65535
            OR m.ordinal IS NULL
    ) OR EXISTS (
        SELECT 1 FROM public.ipam_service_port_mapping
        GROUP BY service_id, protocol, port HAVING count(*) > 1
    ) OR EXISTS (
        SELECT 1 FROM public.ipam_service_port_mapping
        GROUP BY service_id, ordinal HAVING count(*) > 1
    ) THEN
        RAISE EXCEPTION 'Invalid or ambiguous legacy service port mapping relation';
    END IF;

    FOR source IN
        SELECT 'ipam_service'::text AS owning_table, id, protocol, ports
        FROM public.ipam_service
        UNION ALL
        SELECT 'ipam_servicetemplate', id, protocol, ports
        FROM public.ipam_servicetemplate
    LOOP
        native_mappings := NULL;
        relation_mappings := NULL;
        IF source.protocol IS NULL AND source.ports IS NULL THEN
            -- Both absent is historical absence, not an unknown protocol.
            native_mappings := NULL;
        ELSE
            IF source.protocol IS NULL OR source.ports IS NULL
                OR source.protocol NOT IN ('tcp', 'udp', 'sctp')
                OR cardinality(source.ports) = 0 OR array_ndims(source.ports) <> 1
                OR EXISTS (SELECT 1 FROM unnest(source.ports) p
                           WHERE p IS NULL OR p NOT BETWEEN 1 AND 65535)
                OR cardinality(source.ports) <>
                    (SELECT count(DISTINCT p) FROM unnest(source.ports) p)
            THEN
                RAISE EXCEPTION 'Invalid legacy native service ports in % row %',
                    source.owning_table, source.id;
            END IF;
            SELECT array_agg(source.protocol || '/' || p::text ORDER BY ordinal)
            INTO native_mappings
            FROM unnest(source.ports) WITH ORDINALITY AS ports(p, ordinal);
        END IF;

        IF source.owning_table = 'ipam_service' THEN
            SELECT array_agg(protocol || '/' || port::text ORDER BY ordinal)
            INTO relation_mappings
            FROM public.ipam_service_port_mapping WHERE service_id = source.id;
        END IF;

        IF native_mappings IS NOT NULL AND relation_mappings IS NOT NULL
            AND NOT (native_mappings @> relation_mappings
                     AND native_mappings <@ relation_mappings)
        THEN
            RAISE EXCEPTION 'Conflicting native and relation service ports for row %', source.id;
        END IF;

        EXECUTE format('UPDATE public.%I SET port_mappings = $1 WHERE id = $2',
                       source.owning_table)
        USING coalesce(relation_mappings, native_mappings, ARRAY[]::text[]), source.id;
    END LOOP;
END
$migration$;

-- Empty is retained only for historical absence. Other invalid values are
-- forbidden even on rows that have not yet received a mapping mutation.
CREATE FUNCTION public.ipam_port_mappings_valid(mappings text[]) RETURNS boolean
LANGUAGE sql IMMUTABLE PARALLEL SAFE
RETURN mappings IS NOT NULL
    AND (cardinality(mappings) = 0 OR array_ndims(mappings) = 1)
    AND NOT EXISTS (
        SELECT 1 FROM unnest(mappings) m
        WHERE m IS NULL OR CASE
            WHEN m ~ '^(tcp|udp|sctp)/[0-9]+$'
            THEN split_part(m, '/', 2)::numeric NOT BETWEEN 1 AND 65535
            ELSE true END
    )
    AND cardinality(mappings) = (
        SELECT count(DISTINCT (split_part(m, '/', 1), split_part(m, '/', 2)::numeric))
        FROM unnest(mappings) m
        WHERE m ~ '^(tcp|udp|sctp)/[0-9]+$'
    );

ALTER TABLE public.ipam_service
    ALTER COLUMN port_mappings SET NOT NULL,
    ADD CONSTRAINT ipam_service_port_mappings_valid
        CHECK (public.ipam_port_mappings_valid(port_mappings)),
    DROP COLUMN protocol,
    DROP COLUMN ports;
ALTER TABLE public.ipam_servicetemplate
    ALTER COLUMN port_mappings SET NOT NULL,
    ADD CONSTRAINT ipam_servicetemplate_port_mappings_valid
        CHECK (public.ipam_port_mappings_valid(port_mappings)),
    DROP COLUMN protocol,
    DROP COLUMN ports;

CREATE FUNCTION public.ipam_require_port_mappings() RETURNS trigger
LANGUAGE plpgsql AS $function$
BEGIN
    IF NEW.port_mappings IS NULL OR cardinality(NEW.port_mappings) = 0 THEN
        RAISE EXCEPTION 'port_mappings requires at least one endpoint'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END
$function$;
CREATE TRIGGER ipam_service_require_port_mappings
BEFORE INSERT OR UPDATE OF port_mappings ON public.ipam_service
FOR EACH ROW EXECUTE FUNCTION public.ipam_require_port_mappings();
CREATE TRIGGER ipam_servicetemplate_require_port_mappings
BEFORE INSERT OR UPDATE OF port_mappings ON public.ipam_servicetemplate
FOR EACH ROW EXECUTE FUNCTION public.ipam_require_port_mappings();

-- No CASCADE: unexpected dependent objects must block removal atomically.
DROP TABLE public.ipam_service_port_mapping;
