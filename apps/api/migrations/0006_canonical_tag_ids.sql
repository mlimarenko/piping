-- SQLx executes this migration in one transaction. Run only with writers stopped.
-- Lock the catalogue and all installed native membership owners before reading.
SELECT pg_advisory_xact_lock(hashtextextended('inventory-tag-catalog', 0));
LOCK TABLE inventory_tag, playbook_target_set IN ACCESS EXCLUSIVE MODE;

DO $$
BEGIN
    -- The old executable vocabulary ignored tag_ids. Adopting an already
    -- persisted unknown key could introduce a predicate without conservation.
    IF EXISTS (SELECT 1 FROM playbook_target_set WHERE selector ? 'tag_ids') THEN
        RAISE EXCEPTION 'Pre-existing tag_ids selector requires explicit maintenance remediation' USING ERRCODE='23514';
    END IF;
END
$$;

CREATE FUNCTION inventory_tag_subjects() RETURNS TABLE(table_name text)
LANGUAGE sql STABLE AS $$
    SELECT DISTINCT r.relname::text
    FROM pg_class r JOIN pg_namespace n ON n.oid=r.relnamespace
    JOIN pg_attribute a ON a.attrelid=r.oid
    WHERE n.nspname='public' AND r.relkind IN ('r','p')
      AND r.relname ~ '^(circuits|dcim|ipam|tenancy|virtualization|vpn|wireless)_'
      AND a.attname='tags' AND a.atttypid='text[]'::regtype AND NOT a.attisdropped
    ORDER BY 1
$$;

DO $$
DECLARE subject record;
BEGIN
    FOR subject IN SELECT * FROM inventory_tag_subjects() LOOP
        EXECUTE format('LOCK TABLE public.%I IN ACCESS EXCLUSIVE MODE', subject.table_name);
    END LOOP;
END
$$;

-- Complete the native factory metadata catalogue seeded by 0001: manufacturer
-- ...1001, roles ...1011+, and the other factory definitions already carry
-- factory-default memberships. Reserve ...1000 for that known factory Tag.
-- This is one explicit seed, never a general unknown-token allocation path.
CREATE TEMP TABLE tag_factory_seed_provenance ON COMMIT DROP AS
SELECT EXISTS (SELECT 1 FROM inventory_tag WHERE id='00000000-0000-7000-8000-000000001000') AS existed;

DO $$
DECLARE factory_id constant uuid := '00000000-0000-7000-8000-000000001000';
BEGIN
    IF EXISTS (
        SELECT 1 FROM inventory_tag
        WHERE (id=factory_id OR slug IN ('factory-default',factory_id::text)
               OR name IN ('Factory default','factory-default',factory_id::text))
          AND NOT (id=factory_id AND name='Factory default' AND slug='factory-default'
                   AND color='9e9e9e' AND description='Marks native factory inventory definitions.'
                   AND weight=0 AND external_id IS NULL AND piping_synced_at IS NULL)
    ) THEN
        RAISE EXCEPTION 'Factory Tag identity or catalog attributes conflict; maintenance remediation required' USING ERRCODE='23514';
    END IF;
    INSERT INTO inventory_tag(id,name,slug,color,description,weight)
    SELECT factory_id,'Factory default','factory-default','9e9e9e','Marks native factory inventory definitions.',0
    WHERE NOT EXISTS (SELECT 1 FROM inventory_tag WHERE id=factory_id);
END
$$;

-- Exact UUID spelling, slug or name; a UUID-shaped unknown is not a new identity.
CREATE FUNCTION pg_temp.resolve_legacy_tag_tokens(tokens text[]) RETURNS text[]
LANGUAGE plpgsql AS $$
DECLARE token text; candidates text[]; result text[] := '{}';
BEGIN
    IF tokens IS NULL OR (cardinality(tokens)>0 AND array_ndims(tokens)<>1) THEN
        RAISE EXCEPTION 'Null legacy tag membership' USING ERRCODE='23514';
    END IF;
    FOREACH token IN ARRAY tokens LOOP
        SELECT array_agg(DISTINCT id::text) INTO candidates FROM inventory_tag
        WHERE (id::text=token OR slug=token OR name=token)
          -- A newly seeded identity may resolve only its known native marker.
          -- Its UUID/display name must not legitimize previously unknown input,
          -- including selectors with an empty current target universe.
          AND (id<>'00000000-0000-7000-8000-000000001000'::uuid
               OR token='factory-default'
               OR (SELECT existed FROM pg_temp.tag_factory_seed_provenance));
        IF cardinality(candidates) IS DISTINCT FROM 1 THEN
            RAISE EXCEPTION 'Legacy tag token is unknown or ambiguous; maintenance remediation required' USING ERRCODE='23514';
        END IF;
        IF candidates[1]=ANY(result) THEN
            RAISE EXCEPTION 'Duplicate legacy tag identity; maintenance remediation required' USING ERRCODE='23514';
        END IF;
        result := array_append(result,candidates[1]);
    END LOOP;
    RETURN result;
END
$$;

-- A lossless, stronger-than-effective-target check: every tag predicate must
-- select the same identities on the entire target universe. All remaining
-- conjunctions, ordering, limits and executable assignment intersections are
-- unchanged. Mixed UUID/slug memberships that widen a predicate are refused.
CREATE TEMP TABLE tag_target_before ON COMMIT DROP AS
SELECT 'device'::text AS kind,id,tags FROM dcim_device
UNION ALL SELECT 'virtual_machine',id,tags FROM virtualization_virtualmachine;

DO $$
DECLARE subject record;
BEGIN
    FOR subject IN SELECT * FROM inventory_tag_subjects() LOOP
        EXECUTE format('UPDATE public.%I SET tags=pg_temp.resolve_legacy_tag_tokens(tags)', subject.table_name);
    END LOOP;
END
$$;

DO $$
DECLARE target_set record; old_tokens text[]; new_tokens text[];
BEGIN
    FOR target_set IN SELECT id,selector FROM playbook_target_set WHERE selector ? 'tag_slugs' LOOP
        IF target_set.selector ? 'tag_ids' OR jsonb_typeof(target_set.selector->'tag_slugs') <> 'array'
           OR EXISTS (SELECT 1 FROM jsonb_array_elements(target_set.selector->'tag_slugs') v WHERE jsonb_typeof(v)<>'string') THEN
            RAISE EXCEPTION 'Invalid legacy tag selector; maintenance remediation required' USING ERRCODE='23514';
        END IF;
        -- Rust selector_string_list: trim Unicode White_Space, ASCII lowercase,
        -- discard empty entries. translate deliberately does not casefold Unicode.
        SELECT COALESCE(array_agg(normalized ORDER BY ordinal) FILTER (WHERE normalized<>''),'{}') INTO old_tokens
        FROM (SELECT ordinal,translate(btrim(value, E' \t\n\r\x0b\x0c' || U&'\0085\00A0\1680\2000\2001\2002\2003\2004\2005\2006\2007\2008\2009\200A\2028\2029\202F\205F\3000'),
            'ABCDEFGHIJKLMNOPQRSTUVWXYZ','abcdefghijklmnopqrstuvwxyz') normalized
            FROM jsonb_array_elements_text(target_set.selector->'tag_slugs') WITH ORDINALITY AS v(value,ordinal)) normalized;
        new_tokens := pg_temp.resolve_legacy_tag_tokens(old_tokens);
        IF EXISTS (
            SELECT 1 FROM tag_target_before before
            JOIN (SELECT 'device'::text kind,id,tags FROM dcim_device
                  UNION ALL SELECT 'virtual_machine',id,tags FROM virtualization_virtualmachine) after USING(kind,id)
            WHERE (before.tags @> old_tokens) IS DISTINCT FROM (after.tags @> new_tokens)
        ) THEN
            RAISE EXCEPTION 'Canonical tags change executable target membership; maintenance remediation required' USING ERRCODE='23514';
        END IF;
        UPDATE playbook_target_set SET selector=(selector-'tag_slugs') || jsonb_build_object('tag_ids',to_jsonb(new_tokens)) WHERE id=target_set.id;
    END LOOP;
END
$$;

CREATE FUNCTION inventory_validate_tag_ids(ids text[]) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    IF ids IS NULL OR (cardinality(ids)>0 AND array_ndims(ids)<>1)
       OR EXISTS (SELECT 1 FROM unnest(ids) id WHERE id IS NULL OR id !~ '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$')
       OR cardinality(ids)<>(SELECT count(DISTINCT id) FROM unnest(ids) id) THEN
        RAISE EXCEPTION 'Tags must be distinct canonical UUID strings, not null' USING ERRCODE='23514';
    END IF;
    IF EXISTS (SELECT 1 FROM unnest(ids) token(value) WHERE NOT EXISTS (SELECT 1 FROM inventory_tag t WHERE t.id=token.value::uuid)) THEN
        RAISE EXCEPTION 'Unknown tag identity' USING ERRCODE='23503';
    END IF;
END
$$;

-- Statement locks precede row locks, following the custom-field catalogue
-- pattern. Mutating stale repeatable-read snapshots is deliberately refused;
-- read-only repeatable-read assessment never executes these triggers.
CREATE FUNCTION inventory_tag_usage_lock() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF current_setting('transaction_isolation')<>'read committed' THEN
        RAISE EXCEPTION 'Tag mutations require read committed isolation' USING ERRCODE='23514';
    END IF;
    PERFORM pg_advisory_xact_lock_shared(hashtextextended('inventory-tag-catalog',0));
    RETURN NULL;
END
$$;

CREATE FUNCTION inventory_tag_catalog_lock() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='TRUNCATE' THEN
        RAISE EXCEPTION 'Tag catalog cannot be truncated; delete definitions individually' USING ERRCODE='23514';
    END IF;
    IF current_setting('transaction_isolation')<>'read committed' THEN
        RAISE EXCEPTION 'Tag mutations require read committed isolation' USING ERRCODE='23514';
    END IF;
    PERFORM pg_advisory_xact_lock(hashtextextended('inventory-tag-catalog',0));
    RETURN NULL;
END
$$;

CREATE FUNCTION inventory_tag_membership_guard() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM inventory_validate_tag_ids(NEW.tags);
    RETURN NEW;
END
$$;

CREATE FUNCTION inventory_tag_selector_guard() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE ids text[];
BEGIN
    IF NEW.selector ? 'tag_slugs' THEN
        RAISE EXCEPTION 'Retired tag_slugs selector; use tag_ids' USING ERRCODE='23514';
    END IF;
    IF NEW.selector ? 'tag_ids' THEN
        IF jsonb_typeof(NEW.selector->'tag_ids')<>'array'
           OR EXISTS (SELECT 1 FROM jsonb_array_elements(NEW.selector->'tag_ids') v WHERE jsonb_typeof(v)<>'string') THEN
            RAISE EXCEPTION 'tag_ids must be an array of canonical UUID strings' USING ERRCODE='23514';
        END IF;
        SELECT COALESCE(array_agg(value),'{}') INTO ids FROM jsonb_array_elements_text(NEW.selector->'tag_ids');
        PERFORM inventory_validate_tag_ids(ids);
    END IF;
    RETURN NEW;
END
$$;

CREATE FUNCTION inventory_tag_definition_guard() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE subject record; referenced boolean;
BEGIN
    IF TG_OP='UPDATE' THEN
        IF NEW.id<>OLD.id THEN
            RAISE EXCEPTION 'Tag identity is immutable' USING ERRCODE='23514';
        END IF;
        RETURN NEW;
    END IF;
    FOR subject IN SELECT * FROM inventory_tag_subjects() LOOP
        EXECUTE format('SELECT EXISTS (SELECT 1 FROM public.%I WHERE tags @> ARRAY[$1])',subject.table_name) INTO referenced USING OLD.id::text;
        IF referenced THEN
            RAISE EXCEPTION 'Tag is referenced by inventory membership' USING ERRCODE='23503';
        END IF;
    END LOOP;
    IF EXISTS (SELECT 1 FROM playbook_target_set WHERE selector->'tag_ids' @> jsonb_build_array(OLD.id::text)) THEN
        RAISE EXCEPTION 'Tag is referenced by a persisted executable selector' USING ERRCODE='23503';
    END IF;
    RETURN OLD;
END
$$;

DO $$
DECLARE subject record;
BEGIN
    FOR subject IN SELECT * FROM inventory_tag_subjects() LOOP
        EXECUTE format('CREATE TRIGGER tag_usage_lock BEFORE INSERT OR UPDATE OR DELETE ON public.%I FOR EACH STATEMENT EXECUTE FUNCTION inventory_tag_usage_lock()',subject.table_name);
        EXECUTE format('CREATE TRIGGER tag_membership_guard BEFORE INSERT OR UPDATE ON public.%I FOR EACH ROW EXECUTE FUNCTION inventory_tag_membership_guard()',subject.table_name);
    END LOOP;
END
$$;
CREATE TRIGGER tag_usage_lock BEFORE INSERT OR UPDATE OR DELETE ON playbook_target_set FOR EACH STATEMENT EXECUTE FUNCTION inventory_tag_usage_lock();
CREATE TRIGGER tag_selector_guard BEFORE INSERT OR UPDATE ON playbook_target_set FOR EACH ROW EXECUTE FUNCTION inventory_tag_selector_guard();
CREATE TRIGGER tag_catalog_lock BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON inventory_tag FOR EACH STATEMENT EXECUTE FUNCTION inventory_tag_catalog_lock();
CREATE TRIGGER tag_definition_guard BEFORE UPDATE OR DELETE ON inventory_tag FOR EACH ROW EXECUTE FUNCTION inventory_tag_definition_guard();

-- Validate any pre-existing tag_ids too, not only converted legacy selectors.
UPDATE playbook_target_set SET selector=selector;
