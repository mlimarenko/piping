-- PIP-139: PostgreSQL 18 native final schema and factory seeds from versions 0001-0024.
-- Install-local UUIDs and migration-time seed timestamps remain generated.
--
-- PostgreSQL database dump
--


-- Dumped from database version 18.6 (Debian 18.6-1.pgdg13+2)
-- Dumped by pg_dump version 18.6 (Debian 18.6-1.pgdg13+2)

SET statement_timeout = 0;
SET lock_timeout = 0;
SET idle_in_transaction_session_timeout = 0;
SET transaction_timeout = 0;
SET client_encoding = 'UTF8';
SET standard_conforming_strings = on;
SELECT pg_catalog.set_config('search_path', 'public', false);
SET check_function_bodies = false;
SET xmloption = content;
SET client_min_messages = warning;
SET row_security = off;

--
-- Name: natural_sort; Type: COLLATION; Schema: public; Owner: -
--

CREATE COLLATION public.natural_sort (provider = icu, locale = 'und-u-kn');


--
-- Name: pgcrypto; Type: EXTENSION; Schema: -; Owner: -
--

CREATE EXTENSION IF NOT EXISTS pgcrypto WITH SCHEMA public;


--
-- Name: EXTENSION pgcrypto; Type: COMMENT; Schema: -; Owner: -
--

COMMENT ON EXTENSION pgcrypto IS 'cryptographic functions';


--
-- Name: connection_status; Type: TYPE; Schema: public; Owner: -
--

CREATE TYPE public.connection_status AS ENUM (
    'pending',
    'active',
    'disconnected',
    'terminated',
    'failed'
);


--
-- Name: inventory_template_run_status; Type: TYPE; Schema: public; Owner: -
--

CREATE TYPE public.inventory_template_run_status AS ENUM (
    'pending',
    'running',
    'completed',
    'completed_with_errors',
    'failed'
);


--
-- Name: inventory_template_status; Type: TYPE; Schema: public; Owner: -
--

CREATE TYPE public.inventory_template_status AS ENUM (
    'draft',
    'active',
    'archived'
);


--
-- Name: inventory_template_step_result_status; Type: TYPE; Schema: public; Owner: -
--

CREATE TYPE public.inventory_template_step_result_status AS ENUM (
    'success',
    'skipped',
    'failed'
);


--
-- Name: inventory_dataflow_selection_check(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.inventory_dataflow_selection_check() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE flow_id uuid; candidate_ids uuid[]; state record;
BEGIN
    IF TG_TABLE_NAME = 'inventory_dataflow' THEN
        candidate_ids := ARRAY[NEW.id];
    ELSIF TG_OP = 'INSERT' THEN
        candidate_ids := ARRAY[NEW.dataflow_id];
    ELSIF TG_OP = 'DELETE' THEN
        candidate_ids := ARRAY[OLD.dataflow_id];
    ELSE
        candidate_ids := ARRAY[OLD.dataflow_id, NEW.dataflow_id];
    END IF;
    FOR flow_id IN SELECT DISTINCT unnest(candidate_ids) LOOP
        SELECT source_mode, destination_mode INTO state FROM inventory_dataflow WHERE id = flow_id;
        IF FOUND AND (
            (state.source_mode = 'selected') <> EXISTS (SELECT 1 FROM inventory_dataflow_selected_ip WHERE dataflow_id = flow_id AND side = 'source')
            OR (state.destination_mode = 'selected') <> EXISTS (SELECT 1 FROM inventory_dataflow_selected_ip WHERE dataflow_id = flow_id AND side = 'destination')
        ) THEN
            RAISE EXCEPTION 'Selected endpoint mode must have selected IPs, other modes must not' USING ERRCODE = '23514';
        END IF;
    END LOOP;
    RETURN NULL;
END;
$$;


--
-- Name: reject_playbook_version_mutation(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.reject_playbook_version_mutation() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF TG_OP = 'UPDATE' THEN
        IF NEW IS NOT DISTINCT FROM OLD THEN
            RETURN NEW;
        END IF;
        RAISE EXCEPTION 'playbook_version rows are immutable; create a new version instead';
    END IF;

    IF TG_OP = 'DELETE' THEN
        IF current_setting('piping.allow_playbook_delete', true) = 'on' THEN
            RETURN OLD;
        END IF;
        RAISE EXCEPTION 'playbook_version rows are immutable; delete the parent playbook instead';
    END IF;

    RETURN OLD;
END;
$$;


--
-- Name: set_updated_at(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.set_updated_at() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    NEW.updated_at = now();
    RETURN NEW;
END;
$$;


SET default_tablespace = '';

SET default_table_access_method = heap;

--
-- Name: assistant_messages; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.assistant_messages (
    id uuid DEFAULT uuidv7() NOT NULL,
    session_id uuid NOT NULL,
    role text NOT NULL,
    content_json jsonb DEFAULT '{}'::jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT chk_assistant_messages_role CHECK ((role = ANY (ARRAY['system'::text, 'user'::text, 'assistant'::text, 'tool'::text])))
);


--
-- Name: assistant_sessions; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.assistant_sessions (
    id uuid DEFAULT uuidv7() NOT NULL,
    user_id uuid NOT NULL,
    title text NOT NULL,
    status text DEFAULT 'idle'::text NOT NULL,
    lifecycle text DEFAULT 'active'::text NOT NULL,
    pinned boolean DEFAULT false NOT NULL,
    provider text,
    model text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    archived_at timestamp with time zone,
    purge_after timestamp with time zone,
    purged_at timestamp with time zone,
    last_error text,
    message_count integer DEFAULT 0 NOT NULL,
    metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    CONSTRAINT chk_assistant_sessions_archive_fields CHECK ((((lifecycle = 'active'::text) AND (archived_at IS NULL) AND (purge_after IS NULL)) OR ((lifecycle = 'archived'::text) AND (archived_at IS NOT NULL) AND (purge_after IS NOT NULL)))),
    CONSTRAINT chk_assistant_sessions_lifecycle CHECK ((lifecycle = ANY (ARRAY['active'::text, 'archived'::text]))),
    CONSTRAINT chk_assistant_sessions_message_count CHECK ((message_count >= 0)),
    CONSTRAINT chk_assistant_sessions_status CHECK ((status = ANY (ARRAY['idle'::text, 'running'::text, 'error'::text]))),
    CONSTRAINT chk_assistant_sessions_title_length CHECK (((char_length(title) >= 1) AND (char_length(title) <= 200)))
);


--
-- Name: audit_event; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.audit_event (
    id uuid DEFAULT uuidv7() NOT NULL,
    event_type character varying(255) NOT NULL,
    actor_id uuid,
    target_type character varying(100),
    target_id uuid,
    metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    actor_kind character varying(64) DEFAULT 'user'::character varying NOT NULL,
    CONSTRAINT audit_event_actor_kind_check CHECK (((actor_kind)::text = ANY (ARRAY['user'::text, 'system'::text, 'api_token'::text, 'mcp_agent'::text, 'webhook'::text, 'service_principal'::text, 'playbook_job_token'::text])))
);


--
-- Name: auth_oidc_settings; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.auth_oidc_settings (
    id smallint DEFAULT 1 NOT NULL,
    enabled boolean DEFAULT false NOT NULL,
    issuer_url text DEFAULT ''::text NOT NULL,
    client_id text DEFAULT ''::text NOT NULL,
    redirect_url text DEFAULT ''::text NOT NULL,
    scopes text[] DEFAULT ARRAY['openid'::text, 'profile'::text, 'email'::text] NOT NULL,
    group_claim text DEFAULT 'groups'::text NOT NULL,
    provisioning_mode character varying(16) DEFAULT 'manual'::character varying NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT auth_oidc_settings_id_check CHECK ((id = 1)),
    CONSTRAINT auth_oidc_settings_provisioning_mode_check CHECK (((provisioning_mode)::text = ANY (ARRAY[('manual'::character varying)::text, ('jit'::character varying)::text])))
);


--
-- Name: auth_session; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.auth_session (
    id uuid DEFAULT uuidv7() NOT NULL,
    user_id uuid NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    revoked_at timestamp with time zone
);


--
-- Name: auth_user_api_token; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.auth_user_api_token (
    id uuid DEFAULT uuidv7() NOT NULL,
    user_id uuid NOT NULL,
    name character varying(255) NOT NULL,
    token_prefix character varying(64) NOT NULL,
    token_hash text NOT NULL,
    status character varying(16) NOT NULL,
    expires_at timestamp with time zone,
    last_used_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    revoked_at timestamp with time zone,
    revoked_by_user_id uuid,
    CONSTRAINT auth_user_api_token_status_check CHECK (((status)::text = ANY (ARRAY[('active'::character varying)::text, ('revoked'::character varying)::text])))
);


--
-- Name: auth_user_api_token_scope; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.auth_user_api_token_scope (
    id uuid DEFAULT uuidv7() NOT NULL,
    token_id uuid NOT NULL,
    action character varying(64) NOT NULL,
    resource_type character varying(64) NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    resource_pattern text DEFAULT '*'::text NOT NULL,
    CONSTRAINT auth_user_api_token_scope_resource_pattern_check CHECK ((btrim(resource_pattern) <> ''::text))
);


--
-- Name: auth_user_credential; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.auth_user_credential (
    user_id uuid NOT NULL,
    password_hash text NOT NULL,
    password_changed_at timestamp with time zone NOT NULL
);


--
-- Name: automation_template; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.automation_template (
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(255) NOT NULL,
    status character varying(50) NOT NULL,
    description text,
    definition jsonb NOT NULL
);


--
-- Name: circuits_circuit; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.circuits_circuit (
    custom_field_data jsonb NOT NULL,
    id uuid DEFAULT uuidv7() NOT NULL,
    cid character varying(100) NOT NULL,
    status character varying(50) NOT NULL,
    install_date date,
    commit_rate integer,
    description character varying(200) NOT NULL,
    comments text NOT NULL,
    provider_id uuid NOT NULL,
    tenant_id uuid,
    termination_a_id uuid,
    termination_z_id uuid,
    type_id uuid NOT NULL,
    termination_date date,
    provider_account_id uuid,
    _abs_distance numeric(13,4),
    distance numeric(8,2),
    distance_unit character varying(50),
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone,
    CONSTRAINT circuits_circuit_commit_rate_check CHECK ((commit_rate >= 0))
);


--
-- Name: circuits_circuitgroup; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.circuits_circuitgroup (
    id uuid DEFAULT uuidv7() NOT NULL,
    custom_field_data jsonb NOT NULL,
    name character varying(100) NOT NULL,
    slug character varying(100) NOT NULL,
    description character varying(200) NOT NULL,
    tenant_id uuid,
    owner_id uuid,
    comments text NOT NULL,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone
);


--
-- Name: circuits_circuitgroupassignment; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.circuits_circuitgroupassignment (
    id uuid DEFAULT uuidv7() NOT NULL,
    custom_field_data jsonb NOT NULL,
    priority character varying(50),
    member_id uuid NOT NULL,
    group_id uuid NOT NULL,
    member_type_id uuid NOT NULL,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone
);


--
-- Name: circuits_circuittermination; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.circuits_circuittermination (
    id uuid DEFAULT uuidv7() NOT NULL,
    mark_connected boolean NOT NULL,
    term_side character varying(1) NOT NULL,
    port_speed integer,
    upstream_speed integer,
    xconnect_id character varying(50) NOT NULL,
    pp_info character varying(100) NOT NULL,
    description character varying(200) NOT NULL,
    cable_id uuid,
    circuit_id uuid NOT NULL,
    _provider_network_id uuid,
    custom_field_data jsonb NOT NULL,
    cable_end character varying(1),
    termination_id uuid,
    termination_type_id uuid,
    _location_id uuid,
    _region_id uuid,
    _site_id uuid,
    _site_group_id uuid,
    cable_connector smallint,
    cable_positions smallint[],
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone,
    CONSTRAINT circuits_circuittermination_cable_connector_check CHECK ((cable_connector >= 0)),
    CONSTRAINT circuits_circuittermination_port_speed_check CHECK ((port_speed >= 0)),
    CONSTRAINT circuits_circuittermination_upstream_speed_check CHECK ((upstream_speed >= 0))
);


--
-- Name: circuits_circuittype; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.circuits_circuittype (
    custom_field_data jsonb NOT NULL,
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(100) NOT NULL,
    slug character varying(100) NOT NULL,
    description character varying(200) NOT NULL,
    color character varying(6) NOT NULL,
    owner_id uuid,
    comments text NOT NULL,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone
);


--
-- Name: circuits_provider; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.circuits_provider (
    custom_field_data jsonb NOT NULL,
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(100) NOT NULL COLLATE public.natural_sort,
    slug character varying(100) NOT NULL,
    comments text NOT NULL,
    description character varying(200) NOT NULL,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone
);


--
-- Name: circuits_provider_asns; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.circuits_provider_asns (
    id uuid DEFAULT uuidv7() NOT NULL,
    provider_id uuid NOT NULL,
    asn_id uuid NOT NULL
);


--
-- Name: circuits_provideraccount; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.circuits_provideraccount (
    id uuid DEFAULT uuidv7() NOT NULL,
    custom_field_data jsonb NOT NULL,
    description character varying(200) NOT NULL,
    comments text NOT NULL,
    account character varying(100) NOT NULL,
    name character varying(100) NOT NULL,
    provider_id uuid NOT NULL,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone
);


--
-- Name: circuits_providernetwork; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.circuits_providernetwork (
    custom_field_data jsonb NOT NULL,
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(100) NOT NULL COLLATE public.natural_sort,
    description character varying(200) NOT NULL,
    comments text NOT NULL,
    provider_id uuid NOT NULL,
    service_id character varying(100) NOT NULL,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone
);


--
-- Name: circuits_virtualcircuit; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.circuits_virtualcircuit (
    id uuid DEFAULT uuidv7() NOT NULL,
    custom_field_data jsonb NOT NULL,
    description character varying(200) NOT NULL,
    comments text NOT NULL,
    cid character varying(100) NOT NULL,
    status character varying(50) NOT NULL,
    provider_account_id uuid,
    provider_network_id uuid NOT NULL,
    type_id uuid NOT NULL,
    tenant_id uuid,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone
);


--
-- Name: circuits_virtualcircuittermination; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.circuits_virtualcircuittermination (
    id uuid DEFAULT uuidv7() NOT NULL,
    custom_field_data jsonb NOT NULL,
    role character varying(50) NOT NULL,
    description character varying(200) NOT NULL,
    interface_id uuid NOT NULL,
    virtual_circuit_id uuid NOT NULL,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone
);


--
-- Name: circuits_virtualcircuittype; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.circuits_virtualcircuittype (
    id uuid DEFAULT uuidv7() NOT NULL,
    custom_field_data jsonb NOT NULL,
    name character varying(100) NOT NULL,
    slug character varying(100) NOT NULL,
    description character varying(200) NOT NULL,
    color character varying(6) NOT NULL,
    owner_id uuid,
    comments text NOT NULL,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone
);


--
-- Name: config_context; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.config_context (
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(255) NOT NULL,
    weight integer DEFAULT 0 NOT NULL,
    data jsonb NOT NULL,
    is_active boolean DEFAULT true NOT NULL,
    description text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: config_context_binding; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.config_context_binding (
    id uuid DEFAULT uuidv7() NOT NULL,
    context_id uuid NOT NULL,
    scope_type character varying(50) NOT NULL,
    scope_id uuid NOT NULL
);


--
-- Name: connconf_assignment; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.connconf_assignment (
    id uuid DEFAULT uuidv7() NOT NULL,
    record_id uuid NOT NULL,
    scope_type character varying(32) NOT NULL,
    scope_id uuid,
    weight integer DEFAULT 0 NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT connconf_assignment_global_target_check CHECK (((((scope_type)::text = 'global'::text) AND (scope_id IS NULL)) OR (((scope_type)::text <> 'global'::text) AND (scope_id IS NOT NULL)))),
    CONSTRAINT connconf_assignment_scope_type_check CHECK (((scope_type)::text = ANY (ARRAY[('global'::character varying)::text, ('region'::character varying)::text, ('site_group'::character varying)::text, ('site'::character varying)::text, ('location'::character varying)::text, ('rack'::character varying)::text, ('tenant'::character varying)::text, ('cluster'::character varying)::text, ('role'::character varying)::text, ('device_type'::character varying)::text, ('platform'::character varying)::text, ('device'::character varying)::text, ('vm'::character varying)::text, ('connection_target'::character varying)::text])))
);


--
-- Name: connconf_record; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.connconf_record (
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(200) NOT NULL,
    description character varying(500) DEFAULT ''::character varying NOT NULL,
    protocol character varying(20) NOT NULL,
    base_record_id uuid,
    enabled boolean DEFAULT true NOT NULL,
    owner_user_id uuid,
    params jsonb DEFAULT '{}'::jsonb NOT NULL,
    custom_field_data jsonb DEFAULT '{}'::jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT connconf_record_no_self_base CHECK (((base_record_id IS NULL) OR (base_record_id <> id))),
    CONSTRAINT connconf_record_protocol_check CHECK (((protocol)::text = ANY (ARRAY[('ssh'::character varying)::text, ('rdp'::character varying)::text, ('vnc'::character varying)::text, ('app'::character varying)::text, ('cctv'::character varying)::text])))
);


--
-- Name: connection_filter_presets; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.connection_filter_presets (
    id uuid NOT NULL,
    name text NOT NULL,
    filters jsonb NOT NULL,
    created_by uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT connection_filter_presets_name_check CHECK (((length(TRIM(BOTH FROM name)) >= 1) AND (length(TRIM(BOTH FROM name)) <= 80)))
);


--
-- Name: connections_proxy_assignment; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.connections_proxy_assignment (
    id uuid DEFAULT uuidv7() NOT NULL,
    proxy_node_id uuid NOT NULL,
    scope_type character varying(32) NOT NULL,
    scope_id uuid,
    weight integer DEFAULT 0 NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT connections_proxy_assignment_global_scope_check CHECK (((((scope_type)::text = 'global'::text) AND (scope_id IS NULL)) OR (((scope_type)::text <> 'global'::text) AND (scope_id IS NOT NULL)))),
    CONSTRAINT connections_proxy_assignment_scope_type_check CHECK (((scope_type)::text = ANY (ARRAY[('global'::character varying)::text, ('region'::character varying)::text, ('site_group'::character varying)::text, ('site'::character varying)::text, ('location'::character varying)::text, ('rack'::character varying)::text, ('tenant'::character varying)::text, ('cluster'::character varying)::text, ('role'::character varying)::text, ('device_type'::character varying)::text, ('platform'::character varying)::text, ('device'::character varying)::text, ('vm'::character varying)::text])))
);


--
-- Name: connections_proxy_node; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.connections_proxy_node (
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(255) NOT NULL,
    slug character varying(255) NOT NULL,
    base_url character varying(2048) NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    secret_version integer DEFAULT 0 NOT NULL,
    secret_rotated_at timestamp with time zone
);


--
-- Name: connections_session; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.connections_session (
    id uuid DEFAULT uuidv7() NOT NULL,
    target_id uuid NOT NULL,
    user_id uuid NOT NULL,
    protocol character varying(50) NOT NULL,
    status public.connection_status DEFAULT 'pending'::public.connection_status NOT NULL,
    proxy_id uuid,
    proxy_node_id uuid,
    routing_source character varying(64) DEFAULT 'embedded_default'::character varying NOT NULL,
    client_ip character varying(255),
    last_activity timestamp with time zone DEFAULT now() NOT NULL,
    session_metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    started_at timestamp with time zone DEFAULT now() NOT NULL,
    ended_at timestamp with time zone
);


--
-- Name: connections_template; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.connections_template (
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(255) NOT NULL,
    protocol character varying(50) NOT NULL,
    defaults jsonb NOT NULL
);


--
-- Name: connections_user_credential_set; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.connections_user_credential_set (
    id uuid DEFAULT uuidv7() NOT NULL,
    user_id uuid NOT NULL,
    protocol character varying(50) NOT NULL,
    name character varying(255) NOT NULL,
    params jsonb DEFAULT '{}'::jsonb NOT NULL,
    is_default boolean DEFAULT false NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT chk_user_credential_set_params_credentials_object CHECK (((NOT (params ? 'credentials'::text)) OR (jsonb_typeof((params -> 'credentials'::text)) = 'object'::text))),
    CONSTRAINT chk_user_credential_set_params_no_secret_material CHECK (((NOT (params ? 'credentials'::text)) OR (NOT ((params -> 'credentials'::text) ?| ARRAY['password'::text, 'private_key'::text])))),
    CONSTRAINT chk_user_credential_set_params_no_top_level_credentials CHECK ((NOT (params ?| ARRAY['username'::text, 'password'::text, 'private_key'::text, 'screen_resolution'::text, 'color_depth'::text]))),
    CONSTRAINT chk_user_credential_set_params_object CHECK ((jsonb_typeof(params) = 'object'::text))
);


--
-- Name: custom_field; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.custom_field (
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(50) NOT NULL,
    label character varying(50) DEFAULT ''::character varying NOT NULL,
    field_type character varying(20) DEFAULT 'text'::character varying NOT NULL,
    description character varying(200) DEFAULT ''::character varying NOT NULL,
    group_name character varying(50) DEFAULT ''::character varying NOT NULL,
    required boolean DEFAULT false NOT NULL,
    enforce_unique boolean DEFAULT false NOT NULL,
    is_secret boolean DEFAULT false NOT NULL,
    default_value jsonb,
    weight integer DEFAULT 100 NOT NULL,
    validation_minimum numeric(16,4),
    validation_maximum numeric(16,4),
    validation_regex character varying(500) DEFAULT ''::character varying NOT NULL,
    validation_schema jsonb,
    choice_set_id uuid,
    related_object_type_id uuid,
    related_object_filter jsonb,
    ui_visible character varying(10) DEFAULT 'always'::character varying NOT NULL,
    ui_editable character varying(10) DEFAULT 'yes'::character varying NOT NULL,
    is_cloneable boolean DEFAULT false NOT NULL,
    search_weight integer DEFAULT 1000 NOT NULL,
    filter_logic character varying(10) DEFAULT 'loose'::character varying NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT custom_field_filter_logic_valid CHECK (((filter_logic)::text = ANY (ARRAY[('disabled'::character varying)::text, ('loose'::character varying)::text, ('exact'::character varying)::text]))),
    CONSTRAINT custom_field_name_ident CHECK (((name)::text ~ '^[a-z][a-z0-9_]{0,49}$'::text)),
    CONSTRAINT custom_field_type_valid CHECK (((field_type)::text = ANY (ARRAY[('text'::character varying)::text, ('longtext'::character varying)::text, ('integer'::character varying)::text, ('decimal'::character varying)::text, ('boolean'::character varying)::text, ('date'::character varying)::text, ('datetime'::character varying)::text, ('url'::character varying)::text, ('json'::character varying)::text, ('select'::character varying)::text, ('multiselect'::character varying)::text, ('object'::character varying)::text, ('multiobject'::character varying)::text]))),
    CONSTRAINT custom_field_ui_editable_valid CHECK (((ui_editable)::text = ANY (ARRAY[('yes'::character varying)::text, ('no'::character varying)::text, ('hidden'::character varying)::text]))),
    CONSTRAINT custom_field_ui_visible_valid CHECK (((ui_visible)::text = ANY (ARRAY[('always'::character varying)::text, ('if-set'::character varying)::text, ('hidden'::character varying)::text])))
);


--
-- Name: custom_field_choice_set; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.custom_field_choice_set (
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(100) NOT NULL,
    description character varying(200) DEFAULT ''::character varying NOT NULL,
    extra_choices jsonb DEFAULT '[]'::jsonb NOT NULL,
    order_alphabetically boolean DEFAULT false NOT NULL,
    choice_colors jsonb DEFAULT '{}'::jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT custom_field_choice_set_extra_choices_is_array CHECK ((jsonb_typeof(extra_choices) = 'array'::text))
);


--
-- Name: custom_field_object_type; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.custom_field_object_type (
    custom_field_id uuid NOT NULL,
    content_type_id uuid NOT NULL
);


--
-- Name: dcim_cable; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_cable (
    custom_field_data jsonb NOT NULL,
    id uuid DEFAULT uuidv7() NOT NULL,
    type character varying(50),
    status character varying(50) NOT NULL,
    label character varying(100) NOT NULL,
    color character varying(6) NOT NULL,
    length numeric(8,2),
    length_unit character varying(50),
    _abs_length numeric(10,4),
    tenant_id uuid,
    comments text NOT NULL,
    description character varying(200) NOT NULL,
    owner_id uuid,
    profile character varying(50) NOT NULL,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone,
    bundle_id uuid
);


--
-- Name: dcim_cablebundle; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_cablebundle (
    id uuid DEFAULT uuidv7() NOT NULL,
    custom_field_data jsonb NOT NULL,
    name character varying(100) NOT NULL,
    description character varying(200) NOT NULL,
    comments text NOT NULL,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    created timestamp with time zone,
    last_updated timestamp with time zone,
    external_id bigint,
    piping_synced_at timestamp with time zone
);


--
-- Name: dcim_cablepath; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_cablepath (
    id uuid DEFAULT uuidv7() NOT NULL,
    _nodes character varying(40)[] NOT NULL,
    is_active boolean NOT NULL,
    is_split boolean NOT NULL,
    path jsonb NOT NULL,
    is_complete boolean NOT NULL
);


--
-- Name: dcim_cabletermination; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_cabletermination (
    id uuid DEFAULT uuidv7() NOT NULL,
    cable_end character varying(1) NOT NULL,
    termination_id uuid NOT NULL,
    cable_id uuid NOT NULL,
    termination_type_id uuid NOT NULL,
    _device_id uuid,
    _rack_id uuid,
    _location_id uuid,
    _site_id uuid,
    connector smallint,
    positions smallint[],
    CONSTRAINT dcim_cabletermination_connector_check CHECK ((connector >= 0))
);


--
-- Name: dcim_consoleport; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_consoleport (
    custom_field_data jsonb NOT NULL,
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(64) NOT NULL COLLATE public.natural_sort,
    label character varying(64) NOT NULL,
    description character varying(200) NOT NULL,
    mark_connected boolean NOT NULL,
    type character varying(50),
    speed integer,
    _path_id uuid,
    cable_id uuid,
    device_id uuid NOT NULL,
    module_id uuid,
    cable_end character varying(1),
    _location_id uuid,
    _rack_id uuid,
    _site_id uuid,
    owner_id uuid,
    cable_connector smallint,
    cable_positions smallint[],
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone,
    CONSTRAINT dcim_consoleport_cable_connector_check CHECK ((cable_connector >= 0)),
    CONSTRAINT dcim_consoleport_speed_check CHECK ((speed >= 0))
);


--
-- Name: dcim_consoleporttemplate; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_consoleporttemplate (
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(64) NOT NULL COLLATE public.natural_sort,
    label character varying(64) NOT NULL,
    description character varying(200) NOT NULL,
    type character varying(50),
    device_type_id uuid,
    module_type_id uuid
);


--
-- Name: dcim_consoleserverport; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_consoleserverport (
    custom_field_data jsonb NOT NULL,
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(64) NOT NULL COLLATE public.natural_sort,
    label character varying(64) NOT NULL,
    description character varying(200) NOT NULL,
    mark_connected boolean NOT NULL,
    type character varying(50),
    speed integer,
    _path_id uuid,
    cable_id uuid,
    device_id uuid NOT NULL,
    module_id uuid,
    cable_end character varying(1),
    _location_id uuid,
    _rack_id uuid,
    _site_id uuid,
    owner_id uuid,
    cable_connector smallint,
    cable_positions smallint[],
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone,
    CONSTRAINT dcim_consoleserverport_cable_connector_check CHECK ((cable_connector >= 0)),
    CONSTRAINT dcim_consoleserverport_speed_check CHECK ((speed >= 0))
);


--
-- Name: dcim_consoleserverporttemplate; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_consoleserverporttemplate (
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(64) NOT NULL COLLATE public.natural_sort,
    label character varying(64) NOT NULL,
    description character varying(200) NOT NULL,
    type character varying(50),
    device_type_id uuid,
    module_type_id uuid
);


--
-- Name: dcim_coolingfeed; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_coolingfeed (
    id uuid DEFAULT uuidv7() NOT NULL,
    custom_field_data jsonb NOT NULL,
    name character varying(100) NOT NULL COLLATE public.natural_sort,
    status character varying(50) DEFAULT 'active'::character varying NOT NULL,
    cooling_capacity numeric(10,2),
    max_flow numeric(8,2),
    max_flow_unit character varying(50),
    cooling_source_id uuid NOT NULL,
    rack_id uuid,
    tenant_id uuid,
    description character varying(200) NOT NULL,
    comments text NOT NULL,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    created timestamp with time zone,
    last_updated timestamp with time zone,
    external_id bigint,
    piping_synced_at timestamp with time zone,
    CONSTRAINT dcim_coolingfeed_cooling_capacity_check CHECK ((cooling_capacity >= (0)::numeric)),
    CONSTRAINT dcim_coolingfeed_max_flow_check CHECK ((max_flow >= 0.01))
);


--
-- Name: dcim_coolingintake; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_coolingintake (
    id uuid DEFAULT uuidv7() NOT NULL,
    custom_field_data jsonb NOT NULL,
    name character varying(64) NOT NULL COLLATE public.natural_sort,
    label character varying(64) NOT NULL,
    type character varying(50),
    diameter numeric(8,2),
    diameter_unit character varying(50),
    max_flow numeric(8,2),
    max_flow_unit character varying(50),
    device_id uuid NOT NULL,
    module_id uuid,
    cooling_outflow_id uuid,
    description character varying(200) NOT NULL,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    created timestamp with time zone,
    last_updated timestamp with time zone,
    external_id bigint,
    piping_synced_at timestamp with time zone,
    CONSTRAINT dcim_coolingintake_diameter_check CHECK ((diameter >= 0.01)),
    CONSTRAINT dcim_coolingintake_max_flow_check CHECK ((max_flow >= 0.01))
);


--
-- Name: dcim_coolingintaketemplate; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_coolingintaketemplate (
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(64) NOT NULL COLLATE public.natural_sort,
    label character varying(64) NOT NULL,
    type character varying(50),
    diameter numeric(8,2),
    diameter_unit character varying(50),
    max_flow numeric(8,2),
    max_flow_unit character varying(50),
    device_type_id uuid,
    module_type_id uuid,
    description character varying(200) NOT NULL,
    created timestamp with time zone,
    last_updated timestamp with time zone,
    external_id bigint,
    piping_synced_at timestamp with time zone,
    CONSTRAINT dcim_coolingintaketemplate_diameter_check CHECK ((diameter >= 0.01)),
    CONSTRAINT dcim_coolingintaketemplate_max_flow_check CHECK ((max_flow >= 0.01))
);


--
-- Name: dcim_coolingoutflow; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_coolingoutflow (
    id uuid DEFAULT uuidv7() NOT NULL,
    custom_field_data jsonb NOT NULL,
    name character varying(64) NOT NULL COLLATE public.natural_sort,
    label character varying(64) NOT NULL,
    type character varying(50),
    diameter numeric(8,2),
    diameter_unit character varying(50),
    device_id uuid NOT NULL,
    module_id uuid,
    cooling_intake_id uuid,
    description character varying(200) NOT NULL,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    created timestamp with time zone,
    last_updated timestamp with time zone,
    external_id bigint,
    piping_synced_at timestamp with time zone,
    CONSTRAINT dcim_coolingoutflow_diameter_check CHECK ((diameter >= 0.01))
);


--
-- Name: dcim_coolingoutflowtemplate; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_coolingoutflowtemplate (
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(64) NOT NULL COLLATE public.natural_sort,
    label character varying(64) NOT NULL,
    type character varying(50),
    diameter numeric(8,2),
    diameter_unit character varying(50),
    device_type_id uuid,
    module_type_id uuid,
    cooling_intake_id uuid,
    description character varying(200) NOT NULL,
    created timestamp with time zone,
    last_updated timestamp with time zone,
    external_id bigint,
    piping_synced_at timestamp with time zone,
    CONSTRAINT dcim_coolingoutflowtemplate_diameter_check CHECK ((diameter >= 0.01))
);


--
-- Name: dcim_coolingsource; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_coolingsource (
    id uuid DEFAULT uuidv7() NOT NULL,
    custom_field_data jsonb NOT NULL,
    name character varying(100) NOT NULL COLLATE public.natural_sort,
    type character varying(50) NOT NULL,
    status character varying(50) DEFAULT 'active'::character varying NOT NULL,
    fluid_type character varying(50),
    cooling_capacity numeric(10,2),
    site_id uuid NOT NULL,
    location_id uuid,
    description character varying(200) NOT NULL,
    comments text NOT NULL,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    created timestamp with time zone,
    last_updated timestamp with time zone,
    external_id bigint,
    piping_synced_at timestamp with time zone,
    CONSTRAINT dcim_coolingsource_cooling_capacity_check CHECK ((cooling_capacity >= (0)::numeric))
);


--
-- Name: dcim_device; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_device (
    custom_field_data jsonb NOT NULL,
    id uuid DEFAULT uuidv7() NOT NULL,
    local_context_data jsonb,
    name character varying(64) COLLATE public.natural_sort,
    serial character varying(50) NOT NULL,
    asset_tag character varying(50),
    "position" numeric(4,1),
    face character varying(50),
    status character varying(50) NOT NULL,
    vc_position integer,
    vc_priority smallint,
    comments text NOT NULL,
    cluster_id uuid,
    role_id uuid NOT NULL,
    device_type_id uuid NOT NULL,
    location_id uuid,
    platform_id uuid,
    primary_ip4_id uuid,
    primary_ip6_id uuid,
    rack_id uuid,
    site_id uuid NOT NULL,
    tenant_id uuid,
    virtual_chassis_id uuid,
    airflow character varying(50),
    description character varying(200) NOT NULL,
    config_template_id uuid,
    latitude numeric(8,6),
    longitude numeric(9,6),
    oob_ip_id uuid,
    console_port_count bigint NOT NULL,
    console_server_port_count bigint NOT NULL,
    power_port_count bigint NOT NULL,
    power_outlet_count bigint NOT NULL,
    interface_count bigint NOT NULL,
    front_port_count bigint NOT NULL,
    rear_port_count bigint NOT NULL,
    device_bay_count bigint NOT NULL,
    module_bay_count bigint NOT NULL,
    inventory_item_count bigint NOT NULL,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone,
    CONSTRAINT dcim_device_vc_position_check CHECK ((vc_position >= 0)),
    CONSTRAINT dcim_device_vc_priority_check CHECK ((vc_priority >= 0))
);


--
-- Name: dcim_devicebay; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_devicebay (
    custom_field_data jsonb NOT NULL,
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(64) NOT NULL COLLATE public.natural_sort,
    label character varying(64) NOT NULL,
    description character varying(200) NOT NULL,
    device_id uuid NOT NULL,
    installed_device_id uuid,
    _location_id uuid,
    _rack_id uuid,
    _site_id uuid,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone
);


--
-- Name: dcim_devicebaytemplate; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_devicebaytemplate (
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(64) NOT NULL COLLATE public.natural_sort,
    label character varying(64) NOT NULL,
    description character varying(200) NOT NULL,
    device_type_id uuid NOT NULL
);


--
-- Name: dcim_devicerole; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_devicerole (
    custom_field_data jsonb NOT NULL,
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(100) NOT NULL,
    slug character varying(100) NOT NULL,
    color character varying(6) NOT NULL,
    vm_role boolean NOT NULL,
    description character varying(200) NOT NULL,
    config_template_id uuid,
    level integer NOT NULL,
    lft integer NOT NULL,
    rght integer NOT NULL,
    tree_id integer NOT NULL,
    parent_id uuid,
    comments text NOT NULL,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone,
    CONSTRAINT dcim_devicerole_level_check CHECK ((level >= 0)),
    CONSTRAINT dcim_devicerole_lft_check CHECK ((lft >= 0)),
    CONSTRAINT dcim_devicerole_rght_check CHECK ((rght >= 0))
);


--
-- Name: dcim_devicetype; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_devicetype (
    custom_field_data jsonb NOT NULL,
    id uuid DEFAULT uuidv7() NOT NULL,
    model character varying(100) NOT NULL,
    slug character varying(100) NOT NULL,
    part_number character varying(50) NOT NULL,
    u_height numeric(4,1) NOT NULL,
    is_full_depth boolean NOT NULL,
    subdevice_role character varying(50),
    front_image character varying(100) NOT NULL,
    rear_image character varying(100) NOT NULL,
    comments text NOT NULL,
    manufacturer_id uuid NOT NULL,
    airflow character varying(50),
    weight numeric(8,2),
    weight_unit character varying(50),
    _abs_weight bigint,
    description character varying(200) NOT NULL,
    default_platform_id uuid,
    console_port_template_count bigint NOT NULL,
    console_server_port_template_count bigint NOT NULL,
    power_port_template_count bigint NOT NULL,
    power_outlet_template_count bigint NOT NULL,
    interface_template_count bigint NOT NULL,
    front_port_template_count bigint NOT NULL,
    rear_port_template_count bigint NOT NULL,
    device_bay_template_count bigint NOT NULL,
    module_bay_template_count bigint NOT NULL,
    inventory_item_template_count bigint NOT NULL,
    exclude_from_utilization boolean NOT NULL,
    owner_id uuid,
    device_count bigint NOT NULL,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone,
    CONSTRAINT dcim_devicetype__abs_weight_check CHECK ((_abs_weight >= 0))
);


--
-- Name: dcim_frontport; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_frontport (
    custom_field_data jsonb NOT NULL,
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(64) NOT NULL COLLATE public.natural_sort,
    label character varying(64) NOT NULL,
    description character varying(200) NOT NULL,
    mark_connected boolean NOT NULL,
    type character varying(50) NOT NULL,
    cable_id uuid,
    device_id uuid NOT NULL,
    color character varying(6) NOT NULL,
    module_id uuid,
    cable_end character varying(1),
    _location_id uuid,
    _rack_id uuid,
    _site_id uuid,
    owner_id uuid,
    cable_connector smallint,
    cable_positions smallint[],
    positions smallint NOT NULL,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone,
    CONSTRAINT dcim_frontport_cable_connector_check CHECK ((cable_connector >= 0)),
    CONSTRAINT dcim_frontport_positions_check CHECK ((positions >= 0))
);


--
-- Name: dcim_frontporttemplate; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_frontporttemplate (
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(64) NOT NULL COLLATE public.natural_sort,
    label character varying(64) NOT NULL,
    description character varying(200) NOT NULL,
    type character varying(50) NOT NULL,
    device_type_id uuid,
    color character varying(6) NOT NULL,
    module_type_id uuid,
    positions smallint NOT NULL,
    CONSTRAINT dcim_frontporttemplate_positions_check CHECK ((positions >= 0))
);


--
-- Name: dcim_interface; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_interface (
    custom_field_data jsonb NOT NULL,
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(64) NOT NULL COLLATE public.natural_sort,
    label character varying(64) NOT NULL,
    description character varying(200) NOT NULL,
    mark_connected boolean NOT NULL,
    enabled boolean NOT NULL,
    _name character varying(100) NOT NULL,
    type character varying(50) NOT NULL,
    mgmt_only boolean NOT NULL,
    device_id uuid,
    module_id uuid,
    bridge_id uuid,
    lag_id uuid,
    parent_id uuid,
    cable_id uuid,
    untagged_vlan_id uuid,
    qinq_svlan_id uuid,
    vrf_id uuid,
    vlan_translation_policy_id uuid,
    primary_mac_address_id uuid,
    _location_id uuid,
    _rack_id uuid,
    _site_id uuid,
    _path_id uuid,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone,
    mtu integer,
    speed bigint,
    mode character varying(50),
    duplex character varying(50),
    poe_mode character varying(50),
    poe_type character varying(50),
    rf_role character varying(30),
    rf_channel character varying(50),
    rf_channel_frequency numeric(8,3),
    rf_channel_width numeric(7,3),
    tx_power smallint,
    wwn character varying(64),
    channel_id smallint,
    channels smallint,
    cable_connector smallint,
    cable_end character(1),
    cable_positions smallint[],
    wireless_link_id uuid,
    CONSTRAINT dcim_interface_cable_connector_check CHECK (((cable_connector >= 1) AND (cable_connector <= 256))),
    CONSTRAINT dcim_interface_channel_id_check CHECK (((channel_id >= 1) AND (channel_id <= 1024))),
    CONSTRAINT dcim_interface_channels_check CHECK (((channels >= 1) AND (channels <= 1024))),
    CONSTRAINT dcim_interface_mtu_check CHECK (((mtu IS NULL) OR (mtu >= 0))),
    CONSTRAINT dcim_interface_speed_check CHECK (((speed IS NULL) OR (speed >= 0))),
    CONSTRAINT dcim_interface_tx_power_check CHECK (((tx_power >= '-40'::integer) AND (tx_power <= 127)))
);


--
-- Name: dcim_interface_tagged_vlans; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_interface_tagged_vlans (
    id uuid DEFAULT uuidv7() NOT NULL,
    interface_id uuid NOT NULL,
    vlan_id uuid NOT NULL
);


--
-- Name: dcim_interface_vdcs; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_interface_vdcs (
    id uuid DEFAULT uuidv7() NOT NULL,
    interface_id uuid NOT NULL,
    virtualdevicecontext_id uuid NOT NULL
);


--
-- Name: dcim_interface_wirelesslans; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_interface_wirelesslans (
    id uuid DEFAULT uuidv7() NOT NULL,
    interface_id uuid NOT NULL,
    wirelesslan_id uuid NOT NULL
);


--
-- Name: dcim_interfacetemplate; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_interfacetemplate (
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(64) NOT NULL COLLATE public.natural_sort,
    label character varying(64) NOT NULL,
    description character varying(200) NOT NULL,
    _name character varying(100) NOT NULL,
    type character varying(50) NOT NULL,
    mgmt_only boolean NOT NULL,
    device_type_id uuid,
    module_type_id uuid,
    poe_mode character varying(50),
    poe_type character varying(50),
    enabled boolean NOT NULL,
    bridge_id uuid,
    rf_role character varying(30)
);


--
-- Name: dcim_inventoryitem; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_inventoryitem (
    custom_field_data jsonb NOT NULL,
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(64) NOT NULL COLLATE public.natural_sort,
    label character varying(64) NOT NULL,
    description character varying(200) NOT NULL,
    part_id character varying(50) NOT NULL,
    serial character varying(50) NOT NULL,
    asset_tag character varying(50),
    discovered boolean NOT NULL,
    lft integer NOT NULL,
    rght integer NOT NULL,
    tree_id integer NOT NULL,
    level integer NOT NULL,
    device_id uuid NOT NULL,
    manufacturer_id uuid,
    parent_id uuid,
    role_id uuid,
    component_id uuid,
    component_type_id uuid,
    status character varying(50) NOT NULL,
    _location_id uuid,
    _rack_id uuid,
    _site_id uuid,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone,
    CONSTRAINT dcim_inventoryitem_level_check CHECK ((level >= 0)),
    CONSTRAINT dcim_inventoryitem_lft_check CHECK ((lft >= 0)),
    CONSTRAINT dcim_inventoryitem_rght_check CHECK ((rght >= 0))
);


--
-- Name: dcim_inventoryitemrole; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_inventoryitemrole (
    id uuid DEFAULT uuidv7() NOT NULL,
    custom_field_data jsonb NOT NULL,
    name character varying(100) NOT NULL,
    slug character varying(100) NOT NULL,
    color character varying(6) NOT NULL,
    description character varying(200) NOT NULL,
    owner_id uuid,
    comments text NOT NULL,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone
);


--
-- Name: dcim_inventoryitemtemplate; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_inventoryitemtemplate (
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(64) NOT NULL COLLATE public.natural_sort,
    label character varying(64) NOT NULL,
    description character varying(200) NOT NULL,
    component_id uuid,
    part_id character varying(50) NOT NULL,
    lft integer NOT NULL,
    rght integer NOT NULL,
    tree_id integer NOT NULL,
    level integer NOT NULL,
    component_type_id uuid,
    device_type_id uuid NOT NULL,
    manufacturer_id uuid,
    parent_id uuid,
    role_id uuid,
    CONSTRAINT dcim_inventoryitemtemplate_level_check CHECK ((level >= 0)),
    CONSTRAINT dcim_inventoryitemtemplate_lft_check CHECK ((lft >= 0)),
    CONSTRAINT dcim_inventoryitemtemplate_rght_check CHECK ((rght >= 0))
);


--
-- Name: dcim_location; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_location (
    custom_field_data jsonb NOT NULL,
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(100) NOT NULL,
    slug character varying(100) NOT NULL,
    description character varying(200) NOT NULL,
    lft integer NOT NULL,
    rght integer NOT NULL,
    tree_id integer NOT NULL,
    level integer NOT NULL,
    parent_id uuid,
    site_id uuid NOT NULL,
    tenant_id uuid,
    status character varying(50) NOT NULL,
    facility character varying(50) NOT NULL,
    comments text NOT NULL,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone,
    CONSTRAINT dcim_location_level_check CHECK ((level >= 0)),
    CONSTRAINT dcim_location_lft_check CHECK ((lft >= 0)),
    CONSTRAINT dcim_location_rght_check CHECK ((rght >= 0))
);


--
-- Name: dcim_macaddress; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_macaddress (
    id uuid DEFAULT uuidv7() NOT NULL,
    custom_field_data jsonb NOT NULL,
    description character varying(200) NOT NULL,
    comments text NOT NULL,
    mac_address macaddr NOT NULL,
    assigned_object_id uuid,
    assigned_object_type_id uuid,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone
);


--
-- Name: dcim_manufacturer; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_manufacturer (
    custom_field_data jsonb NOT NULL,
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(100) NOT NULL,
    slug character varying(100) NOT NULL,
    description character varying(200) NOT NULL,
    owner_id uuid,
    comments text NOT NULL,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone
);


--
-- Name: dcim_module; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_module (
    id uuid DEFAULT uuidv7() NOT NULL,
    custom_field_data jsonb NOT NULL,
    local_context_data jsonb,
    serial character varying(50) NOT NULL,
    asset_tag character varying(50),
    comments text NOT NULL,
    device_id uuid NOT NULL,
    module_bay_id uuid NOT NULL,
    module_type_id uuid NOT NULL,
    description character varying(200) NOT NULL,
    status character varying(50) NOT NULL,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone
);


--
-- Name: dcim_modulebay; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_modulebay (
    id uuid DEFAULT uuidv7() NOT NULL,
    custom_field_data jsonb NOT NULL,
    name character varying(64) NOT NULL COLLATE public.natural_sort,
    label character varying(64) NOT NULL,
    "position" character varying(30) NOT NULL,
    description character varying(200) NOT NULL,
    device_id uuid NOT NULL,
    level integer NOT NULL,
    lft integer NOT NULL,
    module_id uuid,
    parent_id uuid,
    rght integer NOT NULL,
    tree_id integer NOT NULL,
    _location_id uuid,
    _rack_id uuid,
    _site_id uuid,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone,
    CONSTRAINT dcim_modulebay_level_check CHECK ((level >= 0)),
    CONSTRAINT dcim_modulebay_lft_check CHECK ((lft >= 0)),
    CONSTRAINT dcim_modulebay_rght_check CHECK ((rght >= 0))
);


--
-- Name: dcim_modulebaytemplate; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_modulebaytemplate (
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(64) NOT NULL COLLATE public.natural_sort,
    label character varying(64) NOT NULL,
    "position" character varying(30) NOT NULL,
    description character varying(200) NOT NULL,
    device_type_id uuid,
    module_type_id uuid
);


--
-- Name: dcim_modulebaytype; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_modulebaytype (
    id uuid DEFAULT uuidv7() NOT NULL,
    custom_field_data jsonb NOT NULL,
    name character varying(100) NOT NULL,
    slug character varying(100) NOT NULL,
    manufacturer_id uuid,
    color character varying(6) DEFAULT '9e9e9e'::character varying NOT NULL,
    description character varying(200) NOT NULL,
    comments text NOT NULL,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    created timestamp with time zone,
    last_updated timestamp with time zone,
    external_id bigint,
    piping_synced_at timestamp with time zone
);


--
-- Name: dcim_moduletype; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_moduletype (
    id uuid DEFAULT uuidv7() NOT NULL,
    custom_field_data jsonb NOT NULL,
    model character varying(100) NOT NULL,
    part_number character varying(50) NOT NULL,
    comments text NOT NULL,
    manufacturer_id uuid NOT NULL,
    weight numeric(8,2),
    weight_unit character varying(50),
    _abs_weight bigint,
    description character varying(200) NOT NULL,
    airflow character varying(50),
    attribute_data jsonb,
    profile_id uuid,
    owner_id uuid,
    module_count bigint NOT NULL,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone,
    CONSTRAINT dcim_moduletype__abs_weight_check CHECK ((_abs_weight >= 0))
);


--
-- Name: dcim_moduletypeprofile; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_moduletypeprofile (
    id uuid DEFAULT uuidv7() NOT NULL,
    custom_field_data jsonb NOT NULL,
    description character varying(200) NOT NULL,
    comments text NOT NULL,
    name character varying(100) NOT NULL,
    schema jsonb,
    owner_id uuid
);


--
-- Name: dcim_platform; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_platform (
    custom_field_data jsonb NOT NULL,
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(100) NOT NULL,
    slug character varying(100) NOT NULL,
    description character varying(200) NOT NULL,
    manufacturer_id uuid,
    config_template_id uuid,
    parent_id uuid,
    level integer NOT NULL,
    lft integer NOT NULL,
    rght integer NOT NULL,
    tree_id integer NOT NULL,
    comments text NOT NULL,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone,
    napalm_driver character varying(50),
    CONSTRAINT dcim_platform_level_check CHECK ((level >= 0)),
    CONSTRAINT dcim_platform_lft_check CHECK ((lft >= 0)),
    CONSTRAINT dcim_platform_rght_check CHECK ((rght >= 0))
);


--
-- Name: dcim_portmapping; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_portmapping (
    id uuid DEFAULT uuidv7() NOT NULL,
    front_port_position smallint NOT NULL,
    rear_port_position smallint NOT NULL,
    device_id uuid NOT NULL,
    front_port_id uuid NOT NULL,
    rear_port_id uuid NOT NULL,
    CONSTRAINT dcim_portmapping_front_port_position_check CHECK ((front_port_position >= 0)),
    CONSTRAINT dcim_portmapping_rear_port_position_check CHECK ((rear_port_position >= 0))
);


--
-- Name: dcim_porttemplatemapping; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_porttemplatemapping (
    id uuid DEFAULT uuidv7() NOT NULL,
    front_port_position smallint NOT NULL,
    rear_port_position smallint NOT NULL,
    device_type_id uuid,
    module_type_id uuid,
    front_port_id uuid NOT NULL,
    rear_port_id uuid NOT NULL,
    CONSTRAINT dcim_porttemplatemapping_front_port_position_check CHECK ((front_port_position >= 0)),
    CONSTRAINT dcim_porttemplatemapping_rear_port_position_check CHECK ((rear_port_position >= 0))
);


--
-- Name: dcim_powerfeed; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_powerfeed (
    custom_field_data jsonb NOT NULL,
    id uuid DEFAULT uuidv7() NOT NULL,
    mark_connected boolean NOT NULL,
    name character varying(100) NOT NULL COLLATE public.natural_sort,
    status character varying(50) NOT NULL,
    type character varying(50) NOT NULL,
    supply character varying(50) NOT NULL,
    phase character varying(50) NOT NULL,
    voltage smallint NOT NULL,
    amperage smallint NOT NULL,
    max_utilization smallint NOT NULL,
    available_power integer NOT NULL,
    comments text NOT NULL,
    _path_id uuid,
    cable_id uuid,
    power_panel_id uuid NOT NULL,
    rack_id uuid,
    cable_end character varying(1),
    description character varying(200) NOT NULL,
    tenant_id uuid,
    owner_id uuid,
    cable_connector smallint,
    cable_positions smallint[],
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone,
    CONSTRAINT dcim_powerfeed_amperage_check CHECK ((amperage >= 0)),
    CONSTRAINT dcim_powerfeed_available_power_check CHECK ((available_power >= 0)),
    CONSTRAINT dcim_powerfeed_cable_connector_check CHECK ((cable_connector >= 0)),
    CONSTRAINT dcim_powerfeed_max_utilization_check CHECK ((max_utilization >= 0))
);


--
-- Name: dcim_poweroutlet; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_poweroutlet (
    custom_field_data jsonb NOT NULL,
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(64) NOT NULL COLLATE public.natural_sort,
    label character varying(64) NOT NULL,
    description character varying(200) NOT NULL,
    mark_connected boolean NOT NULL,
    type character varying(50),
    feed_leg character varying(50),
    _path_id uuid,
    cable_id uuid,
    device_id uuid NOT NULL,
    power_port_id uuid,
    module_id uuid,
    cable_end character varying(1),
    color character varying(6) NOT NULL,
    status character varying(50) NOT NULL,
    _location_id uuid,
    _rack_id uuid,
    _site_id uuid,
    owner_id uuid,
    cable_connector smallint,
    cable_positions smallint[],
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone,
    CONSTRAINT dcim_poweroutlet_cable_connector_check CHECK ((cable_connector >= 0))
);


--
-- Name: dcim_poweroutlettemplate; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_poweroutlettemplate (
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(64) NOT NULL COLLATE public.natural_sort,
    label character varying(64) NOT NULL,
    description character varying(200) NOT NULL,
    type character varying(50),
    feed_leg character varying(50),
    device_type_id uuid,
    power_port_id uuid,
    module_type_id uuid,
    color character varying(6) NOT NULL
);


--
-- Name: dcim_powerpanel; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_powerpanel (
    custom_field_data jsonb NOT NULL,
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(100) NOT NULL COLLATE public.natural_sort,
    location_id uuid,
    site_id uuid NOT NULL,
    comments text NOT NULL,
    description character varying(200) NOT NULL,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone
);


--
-- Name: dcim_powerport; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_powerport (
    custom_field_data jsonb NOT NULL,
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(64) NOT NULL COLLATE public.natural_sort,
    label character varying(64) NOT NULL,
    description character varying(200) NOT NULL,
    mark_connected boolean NOT NULL,
    type character varying(50),
    maximum_draw integer,
    allocated_draw integer,
    _path_id uuid,
    cable_id uuid,
    device_id uuid NOT NULL,
    module_id uuid,
    cable_end character varying(1),
    _location_id uuid,
    _rack_id uuid,
    _site_id uuid,
    owner_id uuid,
    cable_connector smallint,
    cable_positions smallint[],
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone,
    CONSTRAINT dcim_powerport_allocated_draw_check CHECK ((allocated_draw >= 0)),
    CONSTRAINT dcim_powerport_cable_connector_check CHECK ((cable_connector >= 0)),
    CONSTRAINT dcim_powerport_maximum_draw_check CHECK ((maximum_draw >= 0))
);


--
-- Name: dcim_powerporttemplate; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_powerporttemplate (
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(64) NOT NULL COLLATE public.natural_sort,
    label character varying(64) NOT NULL,
    description character varying(200) NOT NULL,
    type character varying(50),
    maximum_draw integer,
    allocated_draw integer,
    device_type_id uuid,
    module_type_id uuid,
    CONSTRAINT dcim_powerporttemplate_allocated_draw_check CHECK ((allocated_draw >= 0)),
    CONSTRAINT dcim_powerporttemplate_maximum_draw_check CHECK ((maximum_draw >= 0))
);


--
-- Name: dcim_rack; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_rack (
    custom_field_data jsonb NOT NULL,
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(100) NOT NULL COLLATE public.natural_sort,
    facility_id character varying(50),
    status character varying(50) NOT NULL,
    serial character varying(50) NOT NULL,
    asset_tag character varying(50),
    form_factor character varying(50),
    width smallint NOT NULL,
    u_height smallint NOT NULL,
    desc_units boolean NOT NULL,
    outer_width smallint,
    outer_depth smallint,
    outer_unit character varying(50),
    comments text NOT NULL,
    location_id uuid,
    role_id uuid,
    site_id uuid NOT NULL,
    tenant_id uuid,
    weight numeric(8,2),
    max_weight integer,
    weight_unit character varying(50),
    _abs_weight bigint,
    _abs_max_weight bigint,
    mounting_depth smallint,
    description character varying(200) NOT NULL,
    starting_unit smallint NOT NULL,
    rack_type_id uuid,
    airflow character varying(50),
    outer_height smallint,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone,
    group_id uuid,
    CONSTRAINT dcim_rack__abs_max_weight_check CHECK ((_abs_max_weight >= 0)),
    CONSTRAINT dcim_rack__abs_weight_check CHECK ((_abs_weight >= 0)),
    CONSTRAINT dcim_rack_max_weight_check CHECK ((max_weight >= 0)),
    CONSTRAINT dcim_rack_mounting_depth_check CHECK ((mounting_depth >= 0)),
    CONSTRAINT dcim_rack_outer_depth_check CHECK ((outer_depth >= 0)),
    CONSTRAINT dcim_rack_outer_height_check CHECK ((outer_height >= 0)),
    CONSTRAINT dcim_rack_outer_width_check CHECK ((outer_width >= 0)),
    CONSTRAINT dcim_rack_starting_unit_check CHECK ((starting_unit >= 0)),
    CONSTRAINT dcim_rack_u_height_check CHECK ((u_height >= 0)),
    CONSTRAINT dcim_rack_width_check CHECK ((width >= 0))
);


--
-- Name: dcim_rackgroup; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_rackgroup (
    id uuid DEFAULT uuidv7() NOT NULL,
    custom_field_data jsonb NOT NULL,
    name character varying(100) NOT NULL,
    slug character varying(100) NOT NULL,
    description character varying(200) NOT NULL,
    comments text NOT NULL,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    created timestamp with time zone,
    last_updated timestamp with time zone,
    external_id bigint,
    piping_synced_at timestamp with time zone
);


--
-- Name: dcim_rackreservation; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_rackreservation (
    custom_field_data jsonb NOT NULL,
    id uuid DEFAULT uuidv7() NOT NULL,
    units smallint[] NOT NULL,
    description character varying(200) NOT NULL,
    rack_id uuid NOT NULL,
    tenant_id uuid,
    user_id uuid NOT NULL,
    comments text NOT NULL,
    status character varying(50) NOT NULL,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone
);


--
-- Name: dcim_rackrole; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_rackrole (
    custom_field_data jsonb NOT NULL,
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(100) NOT NULL,
    slug character varying(100) NOT NULL,
    color character varying(6) NOT NULL,
    description character varying(200) NOT NULL,
    owner_id uuid,
    comments text NOT NULL,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone
);


--
-- Name: dcim_racktype; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_racktype (
    id uuid DEFAULT uuidv7() NOT NULL,
    custom_field_data jsonb NOT NULL,
    description character varying(200) NOT NULL,
    comments text NOT NULL,
    weight numeric(8,2),
    weight_unit character varying(50),
    _abs_weight bigint,
    manufacturer_id uuid NOT NULL,
    model character varying(100) NOT NULL,
    slug character varying(100) NOT NULL,
    form_factor character varying(50) NOT NULL,
    width smallint NOT NULL,
    u_height smallint NOT NULL,
    starting_unit smallint NOT NULL,
    desc_units boolean NOT NULL,
    outer_width smallint,
    outer_depth smallint,
    outer_unit character varying(50),
    max_weight integer,
    _abs_max_weight bigint,
    mounting_depth smallint,
    outer_height smallint,
    owner_id uuid,
    rack_count bigint NOT NULL,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone,
    CONSTRAINT dcim_racktype__abs_max_weight_check CHECK ((_abs_max_weight >= 0)),
    CONSTRAINT dcim_racktype__abs_weight_check CHECK ((_abs_weight >= 0)),
    CONSTRAINT dcim_racktype_max_weight_check CHECK ((max_weight >= 0)),
    CONSTRAINT dcim_racktype_mounting_depth_check CHECK ((mounting_depth >= 0)),
    CONSTRAINT dcim_racktype_outer_depth_check CHECK ((outer_depth >= 0)),
    CONSTRAINT dcim_racktype_outer_height_check CHECK ((outer_height >= 0)),
    CONSTRAINT dcim_racktype_outer_width_check CHECK ((outer_width >= 0)),
    CONSTRAINT dcim_racktype_starting_unit_check CHECK ((starting_unit >= 0)),
    CONSTRAINT dcim_racktype_u_height_check CHECK ((u_height >= 0)),
    CONSTRAINT dcim_racktype_width_check CHECK ((width >= 0))
);


--
-- Name: dcim_rearport; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_rearport (
    custom_field_data jsonb NOT NULL,
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(64) NOT NULL COLLATE public.natural_sort,
    label character varying(64) NOT NULL,
    description character varying(200) NOT NULL,
    mark_connected boolean NOT NULL,
    type character varying(50) NOT NULL,
    positions smallint NOT NULL,
    cable_id uuid,
    device_id uuid NOT NULL,
    color character varying(6) NOT NULL,
    module_id uuid,
    cable_end character varying(1),
    _location_id uuid,
    _rack_id uuid,
    _site_id uuid,
    owner_id uuid,
    cable_connector smallint,
    cable_positions smallint[],
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone,
    CONSTRAINT dcim_rearport_cable_connector_check CHECK ((cable_connector >= 0)),
    CONSTRAINT dcim_rearport_positions_check CHECK ((positions >= 0))
);


--
-- Name: dcim_rearporttemplate; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_rearporttemplate (
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(64) NOT NULL COLLATE public.natural_sort,
    label character varying(64) NOT NULL,
    description character varying(200) NOT NULL,
    type character varying(50) NOT NULL,
    positions smallint NOT NULL,
    device_type_id uuid,
    color character varying(6) NOT NULL,
    module_type_id uuid,
    CONSTRAINT dcim_rearporttemplate_positions_check CHECK ((positions >= 0))
);


--
-- Name: dcim_region; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_region (
    custom_field_data jsonb NOT NULL,
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(100) NOT NULL,
    slug character varying(100) NOT NULL,
    description character varying(200) NOT NULL,
    lft integer NOT NULL,
    rght integer NOT NULL,
    tree_id integer NOT NULL,
    level integer NOT NULL,
    parent_id uuid,
    comments text NOT NULL,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone,
    CONSTRAINT dcim_region_level_check CHECK ((level >= 0)),
    CONSTRAINT dcim_region_lft_check CHECK ((lft >= 0)),
    CONSTRAINT dcim_region_rght_check CHECK ((rght >= 0))
);


--
-- Name: dcim_site; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_site (
    custom_field_data jsonb NOT NULL,
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(100) NOT NULL COLLATE public.natural_sort,
    slug character varying(100) NOT NULL,
    status character varying(50) NOT NULL,
    facility character varying(50) NOT NULL,
    time_zone character varying(63),
    description character varying(200) NOT NULL,
    physical_address character varying(200) NOT NULL,
    shipping_address character varying(200) NOT NULL,
    latitude numeric(8,6),
    longitude numeric(9,6),
    comments text NOT NULL,
    group_id uuid,
    region_id uuid,
    tenant_id uuid,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone,
    asns bigint[] DEFAULT '{}'::bigint[] NOT NULL
);


--
-- Name: dcim_site_asns; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_site_asns (
    id uuid DEFAULT uuidv7() NOT NULL,
    site_id uuid NOT NULL,
    asn_id uuid NOT NULL
);


--
-- Name: dcim_sitegroup; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_sitegroup (
    custom_field_data jsonb NOT NULL,
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(100) NOT NULL,
    slug character varying(100) NOT NULL,
    description character varying(200) NOT NULL,
    lft integer NOT NULL,
    rght integer NOT NULL,
    tree_id integer NOT NULL,
    level integer NOT NULL,
    parent_id uuid,
    comments text NOT NULL,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone,
    CONSTRAINT dcim_sitegroup_level_check CHECK ((level >= 0)),
    CONSTRAINT dcim_sitegroup_lft_check CHECK ((lft >= 0)),
    CONSTRAINT dcim_sitegroup_rght_check CHECK ((rght >= 0))
);


--
-- Name: dcim_virtualchassis; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_virtualchassis (
    custom_field_data jsonb NOT NULL,
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(64) NOT NULL COLLATE public.natural_sort,
    domain character varying(30) NOT NULL,
    master_id uuid,
    comments text NOT NULL,
    description character varying(200) NOT NULL,
    member_count bigint NOT NULL,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone
);


--
-- Name: dcim_virtualdevicecontext; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.dcim_virtualdevicecontext (
    id uuid DEFAULT uuidv7() NOT NULL,
    custom_field_data jsonb NOT NULL,
    description character varying(200) NOT NULL,
    name character varying(64) NOT NULL COLLATE public.natural_sort,
    status character varying(50) NOT NULL,
    identifier smallint,
    comments text NOT NULL,
    device_id uuid,
    primary_ip4_id uuid,
    primary_ip6_id uuid,
    tenant_id uuid,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone
);


--
-- Name: entity_mappings; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.entity_mappings (
    id uuid NOT NULL,
    mapping_config_logical_id uuid NOT NULL,
    mapping_config_version_id integer NOT NULL,
    source_entity character varying(255) NOT NULL,
    target_entity character varying(255) NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: field_mappings; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.field_mappings (
    id uuid NOT NULL,
    entity_mapping_id uuid NOT NULL,
    source_field character varying(255) NOT NULL,
    target_field character varying(255) NOT NULL,
    forward_transform jsonb NOT NULL,
    reverse_transform jsonb,
    round_trip_critical boolean DEFAULT false NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: identity_group; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.identity_group (
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(255) NOT NULL,
    external_id character varying(255)
);


--
-- Name: identity_group_membership; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.identity_group_membership (
    id uuid DEFAULT uuidv7() NOT NULL,
    user_id uuid NOT NULL,
    group_id uuid NOT NULL,
    source character varying(32) DEFAULT 'manual'::character varying NOT NULL,
    external_id character varying(255),
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT identity_group_membership_source_check CHECK (((source)::text = ANY (ARRAY[('manual'::character varying)::text, ('oidc'::character varying)::text])))
);


--
-- Name: identity_user; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.identity_user (
    id uuid DEFAULT uuidv7() NOT NULL,
    username character varying(255) NOT NULL,
    email character varying(255) NOT NULL,
    status character varying(50) DEFAULT 'active'::character varying NOT NULL,
    auth_source character varying(100) NOT NULL,
    external_id character varying(255)
);


--
-- Name: instance_branding; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.instance_branding (
    id smallint DEFAULT 1 NOT NULL,
    app_name text DEFAULT 'Piping'::text NOT NULL,
    logo_data_url text,
    favicon_data_url text,
    theme jsonb DEFAULT '{}'::jsonb NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_by uuid,
    CONSTRAINT instance_branding_id_check CHECK ((id = 1))
);


--
-- Name: inventory_content_type; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.inventory_content_type (
    id uuid DEFAULT uuidv7() NOT NULL,
    app_label character varying(100) NOT NULL,
    model character varying(100) NOT NULL
);


--
-- Name: inventory_dataflow; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.inventory_dataflow (
    id uuid DEFAULT uuidv7() NOT NULL,
    name text NOT NULL,
    description text DEFAULT ''::text NOT NULL,
    status text DEFAULT 'draft'::text NOT NULL,
    transport text DEFAULT 'any'::text NOT NULL,
    allow_cross_vrf boolean DEFAULT false NOT NULL,
    destination_ports integer[],
    source_kind text NOT NULL,
    destination_kind text NOT NULL,
    source_mode text DEFAULT 'default'::text NOT NULL,
    destination_mode text DEFAULT 'default'::text NOT NULL,
    source_device_id uuid,
    source_vm_id uuid,
    source_service_id uuid,
    source_ipaddress_id uuid,
    source_prefix_id uuid,
    destination_device_id uuid,
    destination_vm_id uuid,
    destination_service_id uuid,
    destination_ipaddress_id uuid,
    destination_prefix_id uuid,
    source_region_id uuid,
    source_site_id uuid,
    source_sitegroup_id uuid,
    source_devicerole_id uuid,
    source_tenant_id uuid,
    source_tenantgroup_id uuid,
    source_platform_id uuid,
    source_manufacturer_id uuid,
    source_devicetype_id uuid,
    source_rack_id uuid,
    source_location_id uuid,
    source_cluster_id uuid,
    source_clustergroup_id uuid,
    source_clustertype_id uuid,
    destination_region_id uuid,
    destination_site_id uuid,
    destination_sitegroup_id uuid,
    destination_devicerole_id uuid,
    destination_tenant_id uuid,
    destination_tenantgroup_id uuid,
    destination_platform_id uuid,
    destination_manufacturer_id uuid,
    destination_devicetype_id uuid,
    destination_rack_id uuid,
    destination_location_id uuid,
    destination_cluster_id uuid,
    destination_clustergroup_id uuid,
    destination_clustertype_id uuid,
    CONSTRAINT destination_endpoint_kind CHECK ((destination_kind = ANY (ARRAY['all'::text, 'internet'::text, 'dcim.device'::text, 'virtualization.virtualmachine'::text, 'ipam.service'::text, 'ipam.ipaddress'::text, 'ipam.prefix'::text, 'dcim.region'::text, 'dcim.site'::text, 'dcim.sitegroup'::text, 'dcim.devicerole'::text, 'tenancy.tenant'::text, 'tenancy.tenantgroup'::text, 'dcim.platform'::text, 'dcim.manufacturer'::text, 'dcim.devicetype'::text, 'dcim.rack'::text, 'dcim.location'::text, 'virtualization.cluster'::text, 'virtualization.clustergroup'::text, 'virtualization.clustertype'::text]))),
    CONSTRAINT destination_endpoint_reference CHECK (
CASE
    WHEN (destination_kind = ANY (ARRAY['all'::text, 'internet'::text])) THEN ((num_nonnulls(destination_device_id, destination_vm_id, destination_service_id, destination_ipaddress_id, destination_prefix_id, destination_region_id, destination_site_id, destination_sitegroup_id, destination_devicerole_id, destination_tenant_id, destination_tenantgroup_id, destination_platform_id, destination_manufacturer_id, destination_devicetype_id, destination_rack_id, destination_location_id, destination_cluster_id, destination_clustergroup_id, destination_clustertype_id) = 0) AND (destination_mode = 'default'::text))
    ELSE ((num_nonnulls(destination_device_id, destination_vm_id, destination_service_id, destination_ipaddress_id, destination_prefix_id, destination_region_id, destination_site_id, destination_sitegroup_id, destination_devicerole_id, destination_tenant_id, destination_tenantgroup_id, destination_platform_id, destination_manufacturer_id, destination_devicetype_id, destination_rack_id, destination_location_id, destination_cluster_id, destination_clustergroup_id, destination_clustertype_id) = 1) AND
    CASE destination_kind
        WHEN 'dcim.device'::text THEN (destination_device_id IS NOT NULL)
        WHEN 'virtualization.virtualmachine'::text THEN (destination_vm_id IS NOT NULL)
        WHEN 'ipam.service'::text THEN (destination_service_id IS NOT NULL)
        WHEN 'ipam.ipaddress'::text THEN (destination_ipaddress_id IS NOT NULL)
        WHEN 'ipam.prefix'::text THEN (destination_prefix_id IS NOT NULL)
        WHEN 'dcim.region'::text THEN (destination_region_id IS NOT NULL)
        WHEN 'dcim.site'::text THEN (destination_site_id IS NOT NULL)
        WHEN 'dcim.sitegroup'::text THEN (destination_sitegroup_id IS NOT NULL)
        WHEN 'dcim.devicerole'::text THEN (destination_devicerole_id IS NOT NULL)
        WHEN 'tenancy.tenant'::text THEN (destination_tenant_id IS NOT NULL)
        WHEN 'tenancy.tenantgroup'::text THEN (destination_tenantgroup_id IS NOT NULL)
        WHEN 'dcim.platform'::text THEN (destination_platform_id IS NOT NULL)
        WHEN 'dcim.manufacturer'::text THEN (destination_manufacturer_id IS NOT NULL)
        WHEN 'dcim.devicetype'::text THEN (destination_devicetype_id IS NOT NULL)
        WHEN 'dcim.rack'::text THEN (destination_rack_id IS NOT NULL)
        WHEN 'dcim.location'::text THEN (destination_location_id IS NOT NULL)
        WHEN 'virtualization.cluster'::text THEN (destination_cluster_id IS NOT NULL)
        WHEN 'virtualization.clustergroup'::text THEN (destination_clustergroup_id IS NOT NULL)
        WHEN 'virtualization.clustertype'::text THEN (destination_clustertype_id IS NOT NULL)
        ELSE false
    END)
END),
    CONSTRAINT destination_fixed_endpoint_mode CHECK (((destination_kind <> ALL (ARRAY['ipam.ipaddress'::text, 'ipam.prefix'::text])) OR (destination_mode = 'default'::text))),
    CONSTRAINT inventory_dataflow_description_check CHECK ((length(description) <= 4000)),
    CONSTRAINT inventory_dataflow_destination_mode_check CHECK ((destination_mode = ANY (ARRAY['default'::text, 'selected'::text, 'all'::text]))),
    CONSTRAINT inventory_dataflow_destination_ports_check CHECK (cardinality(destination_ports) BETWEEN 1 AND 256 AND array_position(destination_ports, NULL) IS NULL AND 1 <= ALL(destination_ports) AND 65535 >= ALL(destination_ports)),
    CONSTRAINT inventory_dataflow_name_check CHECK (((length(btrim(name)) >= 1) AND (length(btrim(name)) <= 200))),
    CONSTRAINT inventory_dataflow_source_mode_check CHECK ((source_mode = ANY (ARRAY['default'::text, 'selected'::text, 'all'::text]))),
    CONSTRAINT inventory_dataflow_status_check CHECK ((status = ANY (ARRAY['draft'::text, 'active'::text, 'disabled'::text]))),
    CONSTRAINT inventory_dataflow_transport_check CHECK ((transport = ANY (ARRAY['any'::text, 'tcp'::text, 'udp'::text, 'sctp'::text]))),
    CONSTRAINT source_endpoint_kind CHECK ((source_kind = ANY (ARRAY['all'::text, 'internet'::text, 'dcim.device'::text, 'virtualization.virtualmachine'::text, 'ipam.service'::text, 'ipam.ipaddress'::text, 'ipam.prefix'::text, 'dcim.region'::text, 'dcim.site'::text, 'dcim.sitegroup'::text, 'dcim.devicerole'::text, 'tenancy.tenant'::text, 'tenancy.tenantgroup'::text, 'dcim.platform'::text, 'dcim.manufacturer'::text, 'dcim.devicetype'::text, 'dcim.rack'::text, 'dcim.location'::text, 'virtualization.cluster'::text, 'virtualization.clustergroup'::text, 'virtualization.clustertype'::text]))),
    CONSTRAINT source_endpoint_reference CHECK (
CASE
    WHEN (source_kind = ANY (ARRAY['all'::text, 'internet'::text])) THEN ((num_nonnulls(source_device_id, source_vm_id, source_service_id, source_ipaddress_id, source_prefix_id, source_region_id, source_site_id, source_sitegroup_id, source_devicerole_id, source_tenant_id, source_tenantgroup_id, source_platform_id, source_manufacturer_id, source_devicetype_id, source_rack_id, source_location_id, source_cluster_id, source_clustergroup_id, source_clustertype_id) = 0) AND (source_mode = 'default'::text))
    ELSE ((num_nonnulls(source_device_id, source_vm_id, source_service_id, source_ipaddress_id, source_prefix_id, source_region_id, source_site_id, source_sitegroup_id, source_devicerole_id, source_tenant_id, source_tenantgroup_id, source_platform_id, source_manufacturer_id, source_devicetype_id, source_rack_id, source_location_id, source_cluster_id, source_clustergroup_id, source_clustertype_id) = 1) AND
    CASE source_kind
        WHEN 'dcim.device'::text THEN (source_device_id IS NOT NULL)
        WHEN 'virtualization.virtualmachine'::text THEN (source_vm_id IS NOT NULL)
        WHEN 'ipam.service'::text THEN (source_service_id IS NOT NULL)
        WHEN 'ipam.ipaddress'::text THEN (source_ipaddress_id IS NOT NULL)
        WHEN 'ipam.prefix'::text THEN (source_prefix_id IS NOT NULL)
        WHEN 'dcim.region'::text THEN (source_region_id IS NOT NULL)
        WHEN 'dcim.site'::text THEN (source_site_id IS NOT NULL)
        WHEN 'dcim.sitegroup'::text THEN (source_sitegroup_id IS NOT NULL)
        WHEN 'dcim.devicerole'::text THEN (source_devicerole_id IS NOT NULL)
        WHEN 'tenancy.tenant'::text THEN (source_tenant_id IS NOT NULL)
        WHEN 'tenancy.tenantgroup'::text THEN (source_tenantgroup_id IS NOT NULL)
        WHEN 'dcim.platform'::text THEN (source_platform_id IS NOT NULL)
        WHEN 'dcim.manufacturer'::text THEN (source_manufacturer_id IS NOT NULL)
        WHEN 'dcim.devicetype'::text THEN (source_devicetype_id IS NOT NULL)
        WHEN 'dcim.rack'::text THEN (source_rack_id IS NOT NULL)
        WHEN 'dcim.location'::text THEN (source_location_id IS NOT NULL)
        WHEN 'virtualization.cluster'::text THEN (source_cluster_id IS NOT NULL)
        WHEN 'virtualization.clustergroup'::text THEN (source_clustergroup_id IS NOT NULL)
        WHEN 'virtualization.clustertype'::text THEN (source_clustertype_id IS NOT NULL)
        ELSE false
    END)
END),
    CONSTRAINT source_fixed_endpoint_mode CHECK (((source_kind <> ALL (ARRAY['ipam.ipaddress'::text, 'ipam.prefix'::text])) OR (source_mode = 'default'::text)))
);


--
-- Name: inventory_dataflow_selected_ip; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.inventory_dataflow_selected_ip (
    dataflow_id uuid NOT NULL,
    side text NOT NULL,
    ipaddress_id uuid NOT NULL,
    CONSTRAINT inventory_dataflow_selected_ip_side_check CHECK ((side = ANY (ARRAY['source'::text, 'destination'::text])))
);


--
-- Name: inventory_layout_system_profile; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.inventory_layout_system_profile (
    id uuid DEFAULT uuidv7() NOT NULL,
    entity_type character varying(120) NOT NULL,
    layout_json jsonb DEFAULT '{}'::jsonb NOT NULL,
    version integer DEFAULT 1 NOT NULL,
    is_active boolean DEFAULT true NOT NULL,
    updated_by_user_id uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: inventory_layout_user_profile; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.inventory_layout_user_profile (
    id uuid DEFAULT uuidv7() NOT NULL,
    user_id uuid NOT NULL,
    entity_type character varying(120) NOT NULL,
    layout_json jsonb DEFAULT '{}'::jsonb NOT NULL,
    is_enabled boolean DEFAULT true NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: inventory_tag; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.inventory_tag (
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(100) NOT NULL,
    slug character varying(100) NOT NULL,
    color character varying(6) DEFAULT '9e9e9e'::character varying NOT NULL,
    description character varying(200) DEFAULT ''::character varying NOT NULL,
    weight integer DEFAULT 0 NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone
);


--
-- Name: inventory_template; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.inventory_template (
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(160) NOT NULL,
    slug character varying(160),
    description text,
    status public.inventory_template_status DEFAULT 'draft'::public.inventory_template_status NOT NULL,
    owner_user_id uuid,
    is_system boolean DEFAULT false NOT NULL,
    updated_by_user_id uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: inventory_template_run; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.inventory_template_run (
    id uuid DEFAULT uuidv7() NOT NULL,
    template_id uuid NOT NULL,
    requested_by_user_id uuid,
    status public.inventory_template_run_status DEFAULT 'pending'::public.inventory_template_run_status NOT NULL,
    input_variables_json jsonb DEFAULT '{}'::jsonb NOT NULL,
    template_snapshot_json jsonb DEFAULT '{}'::jsonb NOT NULL,
    summary_json jsonb DEFAULT '{}'::jsonb NOT NULL,
    started_at timestamp with time zone,
    finished_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: inventory_template_run_step_result; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.inventory_template_run_step_result (
    id uuid DEFAULT uuidv7() NOT NULL,
    run_id uuid NOT NULL,
    step_key character varying(120) NOT NULL,
    status public.inventory_template_step_result_status NOT NULL,
    created_resource_ids_json jsonb DEFAULT '[]'::jsonb CONSTRAINT inventory_template_run_step__created_resource_ids_json_not_null NOT NULL,
    resolved_payload_json jsonb DEFAULT '{}'::jsonb CONSTRAINT inventory_template_run_step_resu_resolved_payload_json_not_null NOT NULL,
    error_code character varying(120),
    error_message text,
    duration_ms integer,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: inventory_template_step; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.inventory_template_step (
    id uuid DEFAULT uuidv7() NOT NULL,
    template_id uuid NOT NULL,
    step_key character varying(120) NOT NULL,
    title character varying(255) NOT NULL,
    description text,
    resource_type character varying(120) NOT NULL,
    operation character varying(32) DEFAULT 'create'::character varying NOT NULL,
    depends_on_step_keys jsonb DEFAULT '[]'::jsonb NOT NULL,
    payload_template_json jsonb DEFAULT '{}'::jsonb NOT NULL,
    on_conflict_policy character varying(40) DEFAULT 'fail'::character varying NOT NULL,
    sort_order integer DEFAULT 0 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    repeat_json jsonb DEFAULT '{}'::jsonb NOT NULL
);


--
-- Name: inventory_template_variable; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.inventory_template_variable (
    id uuid DEFAULT uuidv7() NOT NULL,
    template_id uuid NOT NULL,
    key character varying(120) NOT NULL,
    label character varying(160) NOT NULL,
    description text,
    value_type character varying(32) NOT NULL,
    required boolean DEFAULT false NOT NULL,
    default_value_json jsonb,
    validation_rules_json jsonb DEFAULT '{}'::jsonb NOT NULL,
    sort_order integer DEFAULT 0 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: ipam_aggregate; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ipam_aggregate (
    custom_field_data jsonb NOT NULL,
    id uuid DEFAULT uuidv7() NOT NULL,
    prefix cidr NOT NULL,
    date_added date,
    description character varying(200) NOT NULL,
    rir_id uuid NOT NULL,
    tenant_id uuid,
    comments text NOT NULL,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone
);


--
-- Name: ipam_asn; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ipam_asn (
    custom_field_data jsonb NOT NULL,
    id uuid DEFAULT uuidv7() NOT NULL,
    asn bigint NOT NULL,
    description character varying(200) NOT NULL,
    rir_id uuid NOT NULL,
    tenant_id uuid,
    comments text NOT NULL,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone
);


--
-- Name: ipam_asnrange; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ipam_asnrange (
    id uuid DEFAULT uuidv7() NOT NULL,
    custom_field_data jsonb NOT NULL,
    description character varying(200) NOT NULL,
    name character varying(100) NOT NULL COLLATE public.natural_sort,
    slug character varying(100) NOT NULL,
    start bigint NOT NULL,
    "end" bigint NOT NULL,
    rir_id uuid NOT NULL,
    tenant_id uuid,
    owner_id uuid,
    comments text NOT NULL,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone
);


--
-- Name: ipam_fhrpgroup; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ipam_fhrpgroup (
    id uuid DEFAULT uuidv7() NOT NULL,
    custom_field_data jsonb NOT NULL,
    group_id smallint NOT NULL,
    name character varying(100) NOT NULL,
    protocol character varying(50) NOT NULL,
    auth_type character varying(50),
    auth_key character varying(255) DEFAULT ''::character varying NOT NULL,
    description character varying(200) NOT NULL,
    comments text NOT NULL,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    created timestamp with time zone,
    last_updated timestamp with time zone,
    external_id bigint,
    piping_synced_at timestamp with time zone,
    CONSTRAINT ipam_fhrpgroup_group_id_check CHECK ((group_id >= 0))
);


--
-- Name: ipam_fhrpgroupassignment; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ipam_fhrpgroupassignment (
    id uuid DEFAULT uuidv7() NOT NULL,
    group_id uuid NOT NULL,
    interface_type_id uuid NOT NULL,
    interface_id uuid NOT NULL,
    priority smallint NOT NULL,
    created timestamp with time zone,
    last_updated timestamp with time zone,
    external_id bigint,
    piping_synced_at timestamp with time zone,
    CONSTRAINT ipam_fhrpgroupassignment_priority_check CHECK (((priority >= 0) AND (priority <= 255)))
);


--
-- Name: ipam_ipaddress; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ipam_ipaddress (
    custom_field_data jsonb NOT NULL,
    id uuid DEFAULT uuidv7() NOT NULL,
    address inet NOT NULL,
    status character varying(50) NOT NULL,
    role character varying(50),
    assigned_object_id uuid,
    dns_name character varying(255) NOT NULL,
    description character varying(200) NOT NULL,
    assigned_object_type_id uuid,
    nat_inside_id uuid,
    tenant_id uuid,
    vrf_id uuid,
    comments text NOT NULL,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone
);


--
-- Name: ipam_iprange; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ipam_iprange (
    custom_field_data jsonb NOT NULL,
    id uuid DEFAULT uuidv7() NOT NULL,
    start_address inet NOT NULL,
    end_address inet NOT NULL,
    size integer NOT NULL,
    status character varying(50) NOT NULL,
    description character varying(200) NOT NULL,
    role_id uuid,
    tenant_id uuid,
    vrf_id uuid,
    comments text NOT NULL,
    mark_utilized boolean NOT NULL,
    mark_populated boolean NOT NULL,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone,
    CONSTRAINT ipam_iprange_size_check CHECK ((size >= 0))
);


--
-- Name: ipam_prefix; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ipam_prefix (
    custom_field_data jsonb NOT NULL,
    id uuid DEFAULT uuidv7() NOT NULL,
    prefix cidr NOT NULL,
    status character varying(50) NOT NULL,
    is_pool boolean NOT NULL,
    description character varying(200) NOT NULL,
    role_id uuid,
    tenant_id uuid,
    vlan_id uuid,
    vrf_id uuid,
    _children bigint NOT NULL,
    _depth smallint NOT NULL,
    mark_utilized boolean NOT NULL,
    comments text NOT NULL,
    scope_id uuid,
    scope_type_id uuid,
    _location_id uuid,
    _region_id uuid,
    _site_id uuid,
    _site_group_id uuid,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone,
    CONSTRAINT ipam_prefix__children_check CHECK ((_children >= 0)),
    CONSTRAINT ipam_prefix__depth_check CHECK ((_depth >= 0))
);


--
-- Name: ipam_rir; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ipam_rir (
    custom_field_data jsonb NOT NULL,
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(100) NOT NULL,
    slug character varying(100) NOT NULL,
    is_private boolean NOT NULL,
    description character varying(200) NOT NULL,
    owner_id uuid,
    comments text NOT NULL,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone
);


--
-- Name: ipam_role; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ipam_role (
    custom_field_data jsonb NOT NULL,
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(100) NOT NULL,
    slug character varying(100) NOT NULL,
    weight smallint NOT NULL,
    description character varying(200) NOT NULL,
    owner_id uuid,
    comments text NOT NULL,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone,
    CONSTRAINT ipam_role_weight_check CHECK ((weight >= 0))
);


--
-- Name: ipam_routetarget; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ipam_routetarget (
    custom_field_data jsonb NOT NULL,
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(21) NOT NULL COLLATE public.natural_sort,
    description character varying(200) NOT NULL,
    tenant_id uuid,
    comments text NOT NULL,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone
);


--
-- Name: ipam_service; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ipam_service (
    custom_field_data jsonb NOT NULL,
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(100) NOT NULL,
    protocol character varying(50),
    ports integer[],
    description character varying(200) NOT NULL,
    comments text NOT NULL,
    parent_object_id uuid NOT NULL,
    parent_object_type_id uuid NOT NULL,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL
);


--
-- Name: ipam_service_ipaddresses; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ipam_service_ipaddresses (
    id uuid DEFAULT uuidv7() NOT NULL,
    service_id uuid NOT NULL,
    ipaddress_id uuid NOT NULL
);


--
-- Name: ipam_service_port_mapping; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ipam_service_port_mapping (
    id uuid DEFAULT uuidv7() NOT NULL,
    service_id uuid NOT NULL,
    protocol character varying(50) NOT NULL,
    port integer NOT NULL,
    ordinal integer NOT NULL,
    CONSTRAINT ipam_service_port_mapping_port_check CHECK (((port >= 0) AND (port <= 65535)))
);


--
-- Name: ipam_servicetemplate; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ipam_servicetemplate (
    id uuid DEFAULT uuidv7() NOT NULL,
    custom_field_data jsonb NOT NULL,
    protocol character varying(50) NOT NULL,
    ports integer[] NOT NULL,
    description character varying(200) NOT NULL,
    name character varying(100) NOT NULL,
    comments text NOT NULL,
    owner_id uuid
);


--
-- Name: ipam_vlan; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ipam_vlan (
    custom_field_data jsonb NOT NULL,
    id uuid DEFAULT uuidv7() NOT NULL,
    vid smallint NOT NULL,
    name character varying(64) NOT NULL,
    status character varying(50) NOT NULL,
    description character varying(200) NOT NULL,
    group_id uuid,
    role_id uuid,
    site_id uuid,
    tenant_id uuid,
    comments text NOT NULL,
    qinq_role character varying(50),
    qinq_svlan_id uuid,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone,
    CONSTRAINT ipam_vlan_vid_check CHECK ((vid >= 0))
);


--
-- Name: ipam_vlangroup; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ipam_vlangroup (
    custom_field_data jsonb NOT NULL,
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(100) NOT NULL COLLATE public.natural_sort,
    slug character varying(100) NOT NULL,
    scope_id uuid,
    description character varying(200) NOT NULL,
    scope_type_id uuid,
    vid_ranges int4range[] NOT NULL,
    _total_vlan_ids bigint NOT NULL,
    tenant_id uuid,
    owner_id uuid,
    comments text NOT NULL,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone
);


--
-- Name: ipam_vlantranslationpolicy; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ipam_vlantranslationpolicy (
    id uuid DEFAULT uuidv7() NOT NULL,
    custom_field_data jsonb NOT NULL,
    comments text NOT NULL,
    name character varying(100) NOT NULL,
    description character varying(200) NOT NULL,
    owner_id uuid
);


--
-- Name: ipam_vlantranslationrule; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ipam_vlantranslationrule (
    id uuid DEFAULT uuidv7() NOT NULL,
    custom_field_data jsonb NOT NULL,
    local_vid smallint NOT NULL,
    remote_vid smallint NOT NULL,
    policy_id uuid NOT NULL,
    description character varying(200) NOT NULL,
    CONSTRAINT ipam_vlantranslationrule_local_vid_check CHECK ((local_vid >= 0)),
    CONSTRAINT ipam_vlantranslationrule_remote_vid_check CHECK ((remote_vid >= 0))
);


--
-- Name: ipam_vrf; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ipam_vrf (
    custom_field_data jsonb NOT NULL,
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(100) NOT NULL COLLATE public.natural_sort,
    rd character varying(21),
    enforce_unique boolean NOT NULL,
    description character varying(200) NOT NULL,
    tenant_id uuid,
    comments text NOT NULL,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone
);


--
-- Name: ipam_vrf_export_targets; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ipam_vrf_export_targets (
    id uuid DEFAULT uuidv7() NOT NULL,
    vrf_id uuid NOT NULL,
    routetarget_id uuid NOT NULL
);


--
-- Name: ipam_vrf_import_targets; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ipam_vrf_import_targets (
    id uuid DEFAULT uuidv7() NOT NULL,
    vrf_id uuid NOT NULL,
    routetarget_id uuid NOT NULL
);


--
-- Name: license_instance_identity; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.license_instance_identity (
    id boolean DEFAULT true NOT NULL,
    instance_fingerprint text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT license_instance_identity_fingerprint_length_check CHECK (((char_length(instance_fingerprint) >= 16) AND (char_length(instance_fingerprint) <= 255))),
    CONSTRAINT license_instance_identity_fingerprint_shape_check CHECK ((instance_fingerprint ~ '^[A-Za-z0-9][A-Za-z0-9._:-]{15,254}$'::text)),
    CONSTRAINT license_instance_identity_singleton_check CHECK (id)
);


--
-- Name: license_seat_assignment; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.license_seat_assignment (
    id uuid DEFAULT uuidv7() NOT NULL,
    user_id uuid NOT NULL,
    assigned_at timestamp with time zone DEFAULT now() NOT NULL,
    revoked_at timestamp with time zone,
    revoked_reason text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT license_seat_assignment_check CHECK ((((revoked_at IS NULL) AND (revoked_reason IS NULL)) OR (revoked_at IS NOT NULL)))
);


--
-- Name: license_seat_ledger; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.license_seat_ledger (
    id uuid DEFAULT uuidv7() NOT NULL,
    instance_id character varying(255) NOT NULL,
    user_id uuid,
    event_type character varying(64) NOT NULL,
    reason_code character varying(64) NOT NULL,
    metadata_json jsonb DEFAULT '{}'::jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT license_seat_ledger_event_type_check CHECK (((event_type)::text = ANY (ARRAY[('seat_assigned'::character varying)::text, ('seat_revoked'::character varying)::text, ('seat_backfilled'::character varying)::text, ('seat_reconciled'::character varying)::text, ('seat_denied'::character varying)::text])))
);


--
-- Name: license_state; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.license_state (
    id uuid DEFAULT uuidv7() NOT NULL,
    instance_id character varying(255) NOT NULL,
    status character varying(32) NOT NULL,
    max_seats integer NOT NULL,
    entitlement_jwt text NOT NULL,
    entitlement_version character varying(255) NOT NULL,
    entitlement_expires_at timestamp with time zone NOT NULL,
    activation_token_encrypted text NOT NULL,
    grace_started_at timestamp with time zone,
    grace_ends_at timestamp with time zone,
    last_sync_at timestamp with time zone,
    last_successful_sync_at timestamp with time zone,
    last_sync_error text,
    last_failure_kind character varying(64),
    consecutive_failures integer DEFAULT 0 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    revision bigint DEFAULT 0 NOT NULL,
    CONSTRAINT license_state_consecutive_failures_check CHECK ((consecutive_failures >= 0)),
    CONSTRAINT license_state_max_seats_check CHECK ((max_seats >= 0)),
    CONSTRAINT license_state_revision_check CHECK ((revision >= 0)),
    CONSTRAINT license_state_status_check CHECK (status IN ('inactive', 'active', 'degraded', 'grace', 'expired', 'suspended'))
);


--
-- Name: mapping_configurations; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.mapping_configurations (
    logical_id uuid NOT NULL,
    version_id integer NOT NULL,
    name character varying(255) NOT NULL,
    description text,
    source_provider_id uuid NOT NULL,
    target_provider_id uuid,
    status character varying(30) NOT NULL,
    direction character varying(30) NOT NULL,
    conflict_mode character varying(30) NOT NULL,
    created_by uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    retired_at timestamp with time zone,
    CONSTRAINT mapping_configurations_conflict_mode_check CHECK (((conflict_mode)::text = ANY (ARRAY[('netbox_wins'::character varying)::text, ('piping_wins'::character varying)::text, ('last_write_wins'::character varying)::text, ('field_level'::character varying)::text]))),
    CONSTRAINT mapping_configurations_direction_check CHECK (((direction)::text = ANY (ARRAY[('pull'::character varying)::text, ('push'::character varying)::text, ('bidirectional'::character varying)::text]))),
    CONSTRAINT mapping_configurations_status_check CHECK (((status)::text = ANY (ARRAY[('draft'::character varying)::text, ('active'::character varying)::text, ('retired'::character varying)::text]))),
    CONSTRAINT mapping_configurations_version_id_check CHECK ((version_id > 0))
);


--
-- Name: monitoring_alert_source; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.monitoring_alert_source (
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(255) NOT NULL,
    source_type character varying(32) NOT NULL,
    query_url text NOT NULL,
    timeout_ms integer DEFAULT 3000 NOT NULL,
    options jsonb DEFAULT '{}'::jsonb NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT chk_monitoring_alert_source_endpoint_url CHECK (((btrim(query_url) !~* '^https?:[\\/]*[^\\/?#]*@'::text) AND (POSITION(('?'::text) IN (query_url)) = 0) AND (POSITION(('#'::text) IN (query_url)) = 0))),
    CONSTRAINT monitoring_alert_source_options_object_check CHECK ((jsonb_typeof(options) = 'object'::text)),
    CONSTRAINT monitoring_alert_source_query_url_not_empty_check CHECK ((length(btrim(query_url)) > 0)),
    CONSTRAINT monitoring_alert_source_timeout_check CHECK ((timeout_ms >= 100)),
    CONSTRAINT monitoring_alert_source_type_check CHECK (((source_type)::text = ANY (ARRAY['alertmanager'::text, 'vmalert'::text, 'zabbix'::text])))
);


--
-- Name: monitoring_alert_source_assignment; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.monitoring_alert_source_assignment (
    id uuid DEFAULT uuidv7() NOT NULL,
    alert_source_id uuid NOT NULL,
    scope_type character varying(64) NOT NULL,
    scope_id uuid,
    weight integer DEFAULT 0 NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT monitoring_alert_source_assignment_scope_check CHECK (((((scope_type)::text = 'global'::text) AND (scope_id IS NULL)) OR (((scope_type)::text <> 'global'::text) AND (scope_id IS NOT NULL))))
);


--
-- Name: monitoring_exporter; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.monitoring_exporter (
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(255) NOT NULL,
    slug character varying(128) NOT NULL,
    exporter_type character varying(64) NOT NULL,
    description text,
    default_job character varying(128) NOT NULL,
    default_scheme character varying(16) DEFAULT 'http'::character varying NOT NULL,
    default_port integer,
    default_metrics_path character varying(255) DEFAULT '/metrics'::character varying NOT NULL,
    default_target_source character varying(32) DEFAULT 'host_address'::character varying NOT NULL,
    labels jsonb DEFAULT '{}'::jsonb NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT monitoring_exporter_default_port_check CHECK (((default_port IS NULL) OR ((default_port > 0) AND (default_port <= 65535)))),
    CONSTRAINT monitoring_exporter_labels_object_check CHECK ((jsonb_typeof(labels) = 'object'::text)),
    CONSTRAINT monitoring_exporter_scheme_check CHECK (((default_scheme)::text = ANY (ARRAY['http'::text, 'https'::text]))),
    CONSTRAINT monitoring_exporter_target_source_check CHECK (((default_target_source)::text = ANY (ARRAY['host_address'::text, 'host_name'::text, 'host_id'::text, 'interface_addresses'::text, 'tunnel_addresses'::text])))
);


--
-- Name: monitoring_exporter_assignment; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.monitoring_exporter_assignment (
    id uuid DEFAULT uuidv7() NOT NULL,
    exporter_id uuid NOT NULL,
    name character varying(255),
    scope_type character varying(64) NOT NULL,
    scope_id uuid,
    weight integer DEFAULT 0 NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    target character varying(255),
    job character varying(128),
    scheme character varying(16),
    port integer,
    metrics_path character varying(255),
    labels jsonb DEFAULT '{}'::jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT monitoring_exporter_assignment_labels_object_check CHECK ((jsonb_typeof(labels) = 'object'::text)),
    CONSTRAINT monitoring_exporter_assignment_port_check CHECK (((port IS NULL) OR ((port > 0) AND (port <= 65535)))),
    CONSTRAINT monitoring_exporter_assignment_scheme_check CHECK (((scheme IS NULL) OR ((scheme)::text = ANY (ARRAY['http'::text, 'https'::text])))),
    CONSTRAINT monitoring_exporter_assignment_scope_check CHECK (((((scope_type)::text = 'global'::text) AND (scope_id IS NULL)) OR (((scope_type)::text <> 'global'::text) AND (scope_id IS NOT NULL))))
);


--
-- Name: monitoring_metric_query; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.monitoring_metric_query (
    id uuid DEFAULT uuidv7() NOT NULL,
    exporter_id uuid,
    name character varying(128) NOT NULL,
    slug character varying(128) NOT NULL,
    label character varying(255),
    query text NOT NULL,
    unit character varying(64),
    description text,
    default_show_in_inspector boolean DEFAULT false NOT NULL,
    default_show_in_list boolean DEFAULT false NOT NULL,
    sort_order integer DEFAULT 0 NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    source_type character varying(32) DEFAULT 'prometheus'::character varying NOT NULL,
    query_type character varying(32) DEFAULT 'promql'::character varying NOT NULL,
    value_type character varying(32) DEFAULT 'auto'::character varying NOT NULL,
    widget_type character varying(32) DEFAULT 'stat'::character varying NOT NULL,
    options jsonb DEFAULT '{}'::jsonb NOT NULL,
    category character varying(64),
    semantic character varying(64),
    CONSTRAINT monitoring_metric_query_options_object_check CHECK ((jsonb_typeof(options) = 'object'::text)),
    CONSTRAINT monitoring_metric_query_query_not_empty_check CHECK ((length(btrim(query)) > 0)),
    CONSTRAINT monitoring_metric_query_query_type_check CHECK (((query_type)::text = ANY (ARRAY['promql'::text, 'zabbix_item_key'::text, 'zabbix_item_name'::text, 'zabbix_itemid'::text]))),
    CONSTRAINT monitoring_metric_query_semantic_check CHECK (((semantic IS NULL) OR ((semantic)::text = ANY (ARRAY['availability'::text])))),
    CONSTRAINT monitoring_metric_query_source_type_check CHECK (((source_type)::text = ANY (ARRAY['prometheus'::text, 'victoriametrics'::text, 'zabbix'::text]))),
    CONSTRAINT monitoring_metric_query_value_type_check CHECK (((value_type)::text = ANY (ARRAY['auto'::text, 'state'::text, 'number'::text, 'percent'::text, 'bytes'::text, 'duration'::text, 'text'::text, 'timestamp'::text, 'log'::text, 'table'::text]))),
    CONSTRAINT monitoring_metric_query_widget_type_check CHECK (((widget_type)::text = ANY (ARRAY['stat'::text, 'badge'::text, 'gauge'::text, 'sparkline'::text, 'table'::text, 'log'::text])))
);


--
-- Name: monitoring_metric_query_host_override; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.monitoring_metric_query_host_override (
    id uuid DEFAULT uuidv7() NOT NULL,
    metric_query_id uuid NOT NULL,
    host_id uuid NOT NULL,
    label_override character varying(255),
    unit_override character varying(64),
    options_override jsonb,
    enabled boolean DEFAULT true NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT monitoring_metric_query_host_override_options_object_check CHECK (((options_override IS NULL) OR (jsonb_typeof(options_override) = 'object'::text)))
);


--
-- Name: monitoring_metric_set; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.monitoring_metric_set (
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(255) NOT NULL,
    slug character varying(128) NOT NULL,
    description text,
    enabled boolean DEFAULT true NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: monitoring_metric_set_assignment; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.monitoring_metric_set_assignment (
    id uuid DEFAULT uuidv7() NOT NULL,
    metric_set_id uuid NOT NULL,
    scope_type character varying(64) NOT NULL,
    scope_id uuid,
    weight integer DEFAULT 0 NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT monitoring_metric_set_assignment_scope_check CHECK (((((scope_type)::text = 'global'::text) AND (scope_id IS NULL)) OR (((scope_type)::text <> 'global'::text) AND (scope_id IS NOT NULL))))
);


--
-- Name: monitoring_metric_set_item; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.monitoring_metric_set_item (
    id uuid DEFAULT uuidv7() NOT NULL,
    metric_set_id uuid NOT NULL,
    metric_query_id uuid NOT NULL,
    show_in_inspector boolean DEFAULT true NOT NULL,
    show_in_list boolean DEFAULT false NOT NULL,
    show_in_hover boolean DEFAULT false NOT NULL,
    list_render character varying(32) DEFAULT 'auto'::character varying NOT NULL,
    sort_order integer DEFAULT 0 NOT NULL,
    label_override character varying(255),
    unit_override character varying(64),
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT monitoring_metric_set_item_list_render_check CHECK (((list_render)::text = ANY (ARRAY['auto'::text, 'dot'::text, 'value'::text, 'boolean'::text, 'gauge'::text])))
);


--
-- Name: monitoring_query_source; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.monitoring_query_source (
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(255) NOT NULL,
    slug character varying(128) NOT NULL,
    query_url text NOT NULL,
    timeout_ms integer DEFAULT 3000 NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    source_type character varying(32) DEFAULT 'prometheus'::character varying NOT NULL,
    options jsonb DEFAULT '{}'::jsonb NOT NULL,
    CONSTRAINT chk_monitoring_query_source_endpoint_url CHECK (((btrim(query_url) !~* '^https?:[\\/]*[^\\/?#]*@'::text) AND (POSITION(('?'::text) IN (query_url)) = 0) AND (POSITION(('#'::text) IN (query_url)) = 0))),
    CONSTRAINT monitoring_query_source_options_object_check CHECK ((jsonb_typeof(options) = 'object'::text)),
    CONSTRAINT monitoring_query_source_query_url_not_empty_check CHECK ((length(btrim(query_url)) > 0)),
    CONSTRAINT monitoring_query_source_timeout_check CHECK ((timeout_ms >= 100)),
    CONSTRAINT monitoring_query_source_type_check CHECK (((source_type)::text = ANY (ARRAY['prometheus'::text, 'victoriametrics'::text, 'zabbix'::text])))
);


--
-- Name: monitoring_query_source_assignment; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.monitoring_query_source_assignment (
    id uuid DEFAULT uuidv7() NOT NULL,
    query_source_id uuid NOT NULL,
    scope_type character varying(64) NOT NULL,
    scope_id uuid,
    weight integer DEFAULT 0 NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    options jsonb DEFAULT '{}'::jsonb NOT NULL,
    CONSTRAINT monitoring_query_source_assignment_options_object_check CHECK ((jsonb_typeof(options) = 'object'::text)),
    CONSTRAINT monitoring_query_source_assignment_scope_check CHECK (((((scope_type)::text = 'global'::text) AND (scope_id IS NULL)) OR (((scope_type)::text <> 'global'::text) AND (scope_id IS NOT NULL))))
);


--
-- Name: monitoring_sd_label_settings; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.monitoring_sd_label_settings (
    id boolean DEFAULT true NOT NULL,
    groups jsonb DEFAULT '{}'::jsonb NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT monitoring_sd_label_settings_id_check CHECK (id)
);


--
-- Name: playbook; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.playbook (
    id uuid DEFAULT uuidv7() NOT NULL,
    slug text NOT NULL,
    name text NOT NULL,
    description text,
    status text DEFAULT 'draft'::text NOT NULL,
    runtime text DEFAULT 'ansible'::text NOT NULL,
    risk text DEFAULT 'low'::text NOT NULL,
    owner_user_id uuid,
    category text DEFAULT 'general'::text NOT NULL,
    current_version_id uuid,
    default_target_set_id uuid,
    created_by_user_id uuid,
    updated_by_user_id uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    source_kind text DEFAULT 'in_app'::text NOT NULL,
    vars_schema jsonb,
    connection_default text DEFAULT 'ssh'::text NOT NULL,
    inventory_rest_enabled boolean DEFAULT false NOT NULL,
    CONSTRAINT chk_playbook_category CHECK ((category ~ '^[a-z0-9][a-z0-9_-]{1,62}[a-z0-9]$'::text)),
    CONSTRAINT chk_playbook_connection_default CHECK ((connection_default = ANY (ARRAY['ssh'::text, 'winrm'::text, 'local'::text]))),
    CONSTRAINT chk_playbook_metadata_object CHECK ((jsonb_typeof(metadata) = 'object'::text)),
    CONSTRAINT chk_playbook_name_length CHECK (((char_length(name) >= 1) AND (char_length(name) <= 200))),
    CONSTRAINT chk_playbook_risk CHECK ((risk = ANY (ARRAY['low'::text, 'medium'::text, 'high'::text, 'critical'::text]))),
    CONSTRAINT chk_playbook_runtime CHECK ((runtime = 'ansible'::text)),
    CONSTRAINT chk_playbook_slug CHECK ((slug ~ '^[a-z0-9][a-z0-9_-]{1,126}[a-z0-9]$'::text)),
    CONSTRAINT chk_playbook_source_kind CHECK ((source_kind = ANY (ARRAY['in_app'::text, 'git'::text, 'system'::text, 'official'::text]))),
    CONSTRAINT chk_playbook_status CHECK ((status = ANY (ARRAY['draft'::text, 'active'::text, 'archived'::text]))),
    CONSTRAINT chk_playbook_vars_schema_object CHECK (((vars_schema IS NULL) OR (jsonb_typeof(vars_schema) = 'object'::text)))
);


--
-- Name: playbook_artifact; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.playbook_artifact (
    id uuid DEFAULT uuidv7() NOT NULL,
    run_id uuid NOT NULL,
    artifact_type text NOT NULL,
    name text NOT NULL,
    content_type text DEFAULT 'application/json'::text NOT NULL,
    content_json jsonb,
    content_text text,
    content_sha256 text NOT NULL,
    content_size_bytes bigint NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT chk_playbook_artifact_content_type CHECK (((char_length(content_type) >= 1) AND (char_length(content_type) <= 200))),
    CONSTRAINT chk_playbook_artifact_hash CHECK ((content_sha256 ~ '^[a-f0-9]{64}$'::text)),
    CONSTRAINT chk_playbook_artifact_json_object CHECK (((content_json IS NULL) OR (jsonb_typeof(content_json) = 'object'::text))),
    CONSTRAINT chk_playbook_artifact_name CHECK (((char_length(name) >= 1) AND (char_length(name) <= 240))),
    CONSTRAINT chk_playbook_artifact_one_content CHECK (((content_json IS NOT NULL) <> (content_text IS NOT NULL))),
    CONSTRAINT chk_playbook_artifact_size CHECK (((content_size_bytes >= 1) AND (content_size_bytes <= 2097152) AND ((content_text IS NULL) OR (octet_length(content_text) <= 2097152)) AND ((content_json IS NULL) OR (octet_length((content_json)::text) <= 2097152)))),
    CONSTRAINT chk_playbook_artifact_type CHECK (((char_length(artifact_type) >= 1) AND (char_length(artifact_type) <= 120)))
);


--
-- Name: playbook_assignment; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.playbook_assignment (
    id uuid DEFAULT uuidv7() NOT NULL,
    playbook_id uuid NOT NULL,
    target_set_id uuid NOT NULL,
    default_version_id uuid,
    name text,
    scope_kind text NOT NULL,
    scope_id uuid,
    priority integer DEFAULT 0 NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    visible boolean DEFAULT true NOT NULL,
    executable boolean DEFAULT true NOT NULL,
    quick_action_enabled boolean DEFAULT false NOT NULL,
    default_input jsonb DEFAULT '{}'::jsonb NOT NULL,
    default_metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    default_timeout_seconds integer,
    default_target_limit integer,
    created_by_user_id uuid,
    updated_by_user_id uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    CONSTRAINT chk_playbook_assignment_default_input_object CHECK ((jsonb_typeof(default_input) = 'object'::text)),
    CONSTRAINT chk_playbook_assignment_default_metadata_object CHECK ((jsonb_typeof(default_metadata) = 'object'::text)),
    CONSTRAINT chk_playbook_assignment_default_target_limit CHECK (((default_target_limit IS NULL) OR ((default_target_limit >= 1) AND (default_target_limit <= 50000)))),
    CONSTRAINT chk_playbook_assignment_default_timeout CHECK (((default_timeout_seconds IS NULL) OR ((default_timeout_seconds >= 1) AND (default_timeout_seconds <= 86400)))),
    CONSTRAINT chk_playbook_assignment_metadata_object CHECK ((jsonb_typeof(metadata) = 'object'::text)),
    CONSTRAINT chk_playbook_assignment_name_length CHECK (((name IS NULL) OR ((char_length(name) >= 1) AND (char_length(name) <= 200)))),
    CONSTRAINT chk_playbook_assignment_scope CHECK ((((scope_kind = 'global'::text) AND (scope_id IS NULL)) OR ((scope_kind = ANY (ARRAY['connection_target'::text, 'role'::text, 'site'::text, 'region'::text, 'tenant'::text, 'cluster'::text, 'platform'::text])) AND (scope_id IS NOT NULL))))
);


--
-- Name: playbook_execution_environment; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.playbook_execution_environment (
    id uuid DEFAULT uuidv7() NOT NULL,
    name text NOT NULL,
    description text,
    image text NOT NULL,
    pull_policy text DEFAULT 'if_not_present'::text NOT NULL,
    runner_group text DEFAULT 'default'::text NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    env jsonb DEFAULT '{}'::jsonb NOT NULL,
    settings jsonb DEFAULT '{}'::jsonb NOT NULL,
    metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    created_by_user_id uuid,
    updated_by_user_id uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT chk_playbook_ee_env_object CHECK ((jsonb_typeof(env) = 'object'::text)),
    CONSTRAINT chk_playbook_ee_metadata_object CHECK ((jsonb_typeof(metadata) = 'object'::text)),
    CONSTRAINT chk_playbook_ee_pull_policy CHECK ((pull_policy = ANY (ARRAY['always'::text, 'if_not_present'::text, 'never'::text]))),
    CONSTRAINT chk_playbook_ee_runner_group CHECK (((runner_group <> ''::text) AND (length(runner_group) <= 120))),
    CONSTRAINT chk_playbook_ee_settings_object CHECK ((jsonb_typeof(settings) = 'object'::text))
);


--
-- Name: playbook_job_template; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.playbook_job_template (
    id uuid DEFAULT uuidv7() NOT NULL,
    name text NOT NULL,
    description text,
    playbook_id uuid,
    project_id uuid,
    playbook_path text,
    inventory_target_set_id uuid,
    execution_environment_id uuid,
    credential_refs jsonb DEFAULT '[]'::jsonb NOT NULL,
    extra_vars jsonb DEFAULT '{}'::jsonb NOT NULL,
    launch_options jsonb DEFAULT '{}'::jsonb NOT NULL,
    ask_variables_on_launch boolean DEFAULT false NOT NULL,
    ask_limit_on_launch boolean DEFAULT false NOT NULL,
    ask_tags_on_launch boolean DEFAULT false NOT NULL,
    ask_skip_tags_on_launch boolean DEFAULT false NOT NULL,
    ask_credential_on_launch boolean DEFAULT false NOT NULL,
    ask_execution_environment_on_launch boolean DEFAULT false CONSTRAINT playbook_job_template_ask_execution_environment_on_lau_not_null NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    created_by_user_id uuid,
    updated_by_user_id uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT chk_playbook_job_template_credential_refs CHECK ((jsonb_typeof(credential_refs) = 'array'::text)),
    CONSTRAINT chk_playbook_job_template_extra_vars CHECK ((jsonb_typeof(extra_vars) = 'object'::text)),
    CONSTRAINT chk_playbook_job_template_launch_options CHECK ((jsonb_typeof(launch_options) = 'object'::text)),
    CONSTRAINT chk_playbook_job_template_metadata_object CHECK ((jsonb_typeof(metadata) = 'object'::text)),
    CONSTRAINT chk_playbook_job_template_playbook_path CHECK (((playbook_path IS NULL) OR ((playbook_path <> ''::text) AND (playbook_path !~ '(^/|(^|/)\.\.(/|$)|\\)'::text)))),
    CONSTRAINT chk_playbook_job_template_source CHECK ((((playbook_id IS NOT NULL) AND (project_id IS NULL) AND (playbook_path IS NULL)) OR ((playbook_id IS NULL) AND (project_id IS NOT NULL) AND (playbook_path IS NOT NULL))))
);


--
-- Name: playbook_job_template_survey; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.playbook_job_template_survey (
    id uuid DEFAULT uuidv7() NOT NULL,
    job_template_id uuid NOT NULL,
    enabled boolean DEFAULT false NOT NULL,
    spec jsonb DEFAULT '{"questions": []}'::jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT chk_playbook_job_template_survey_spec CHECK ((jsonb_typeof(spec) = 'object'::text))
);


--
-- Name: playbook_job_token; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.playbook_job_token (
    id uuid DEFAULT uuidv7() NOT NULL,
    run_id uuid NOT NULL,
    target_kind text NOT NULL,
    target_id uuid NOT NULL,
    source_kind text NOT NULL,
    source_id uuid NOT NULL,
    token_hash text NOT NULL,
    token_prefix text NOT NULL,
    status text DEFAULT 'active'::text NOT NULL,
    scopes jsonb DEFAULT '[]'::jsonb NOT NULL,
    issued_at timestamp with time zone DEFAULT now() NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    last_used_at timestamp with time zone,
    revoked_at timestamp with time zone,
    metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    CONSTRAINT chk_playbook_job_token_expiry CHECK ((expires_at > issued_at)),
    CONSTRAINT chk_playbook_job_token_metadata_object CHECK ((jsonb_typeof(metadata) = 'object'::text)),
    CONSTRAINT chk_playbook_job_token_prefix CHECK ((token_prefix ~ '^pjt_[A-Za-z0-9_-]{8}$'::text)),
    CONSTRAINT chk_playbook_job_token_scopes_array CHECK (((jsonb_typeof(scopes) = 'array'::text) AND ((jsonb_array_length(scopes) >= 1) AND (jsonb_array_length(scopes) <= 16)) AND (scopes <@ '["artifact:create", "event:append", "status:update", "inventory:read", "inventory:create", "inventory:update"]'::jsonb))),
    CONSTRAINT chk_playbook_job_token_source_kind CHECK ((source_kind = ANY (ARRAY['device'::text, 'virtual_machine'::text]))),
    CONSTRAINT chk_playbook_job_token_status CHECK ((status = ANY (ARRAY['active'::text, 'used'::text, 'revoked'::text, 'expired'::text]))),
    CONSTRAINT chk_playbook_job_token_target_kind CHECK ((target_kind = 'connection_target'::text))
);


--
-- Name: playbook_project; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.playbook_project (
    id uuid DEFAULT uuidv7() NOT NULL,
    name text NOT NULL,
    description text,
    scm_type text DEFAULT 'manual'::text NOT NULL,
    scm_url text,
    scm_branch text,
    scm_ref text,
    playbook_base_path text,
    default_execution_environment_id uuid,
    sync_policy text DEFAULT 'manual'::text NOT NULL,
    sync_status text DEFAULT 'never_synced'::text NOT NULL,
    last_sync_revision text,
    last_sync_error text,
    last_synced_at timestamp with time zone,
    metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    created_by_user_id uuid,
    updated_by_user_id uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT chk_playbook_project_base_path CHECK (((playbook_base_path IS NULL) OR ((playbook_base_path <> ''::text) AND (playbook_base_path !~ '(^/|(^|/)\.\.(/|$)|\\)'::text)))),
    CONSTRAINT chk_playbook_project_metadata_object CHECK ((jsonb_typeof(metadata) = 'object'::text)),
    CONSTRAINT chk_playbook_project_scm_manual_url CHECK ((((scm_type <> 'manual'::text) AND (scm_url IS NOT NULL) AND (scm_url <> ''::text)) OR ((scm_type = 'manual'::text) AND (scm_url IS NULL)))),
    CONSTRAINT chk_playbook_project_scm_type CHECK ((scm_type = ANY (ARRAY['manual'::text, 'git'::text]))),
    CONSTRAINT chk_playbook_project_sync_policy CHECK ((sync_policy = ANY (ARRAY['manual'::text, 'on_launch'::text, 'scheduled'::text]))),
    CONSTRAINT chk_playbook_project_sync_status CHECK ((sync_status = ANY (ARRAY['never_synced'::text, 'pending'::text, 'running'::text, 'successful'::text, 'failed'::text])))
);


--
-- Name: playbook_run; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.playbook_run (
    id uuid DEFAULT uuidv7() NOT NULL,
    run_group_id uuid NOT NULL,
    playbook_id uuid NOT NULL,
    playbook_version_id uuid NOT NULL,
    target_kind text NOT NULL,
    target_id uuid NOT NULL,
    source_kind text NOT NULL,
    source_id uuid NOT NULL,
    target_name text,
    target_address text,
    target_address_bound boolean DEFAULT false NOT NULL,
    credential_account_kind text DEFAULT 'auto'::text NOT NULL,
    credential_account_id uuid,
    credential_account_entry_id text,
    credential_merge_policy text DEFAULT 'context_first'::text NOT NULL,
    credential_resolution jsonb DEFAULT '{"status": "pending", "selection": "auto", "merge_policy": "context_first"}'::jsonb NOT NULL,
    status text DEFAULT 'queued'::text NOT NULL,
    cancel_requested_at timestamp with time zone,
    cancel_outcome text,
    lease_owner text,
    lease_expires_at timestamp with time zone,
    queued_at timestamp with time zone DEFAULT now() NOT NULL,
    leased_at timestamp with time zone,
    started_at timestamp with time zone,
    finished_at timestamp with time zone,
    exit_code integer,
    error_message text,
    attempt integer DEFAULT 1 NOT NULL,
    timeout_seconds integer DEFAULT 1800 NOT NULL,
    callback_event_count bigint DEFAULT 0 NOT NULL,
    callback_artifact_count bigint DEFAULT 0 NOT NULL,
    callback_bytes bigint DEFAULT 0 NOT NULL,
    callback_request_count integer DEFAULT 0 NOT NULL,
    callback_request_window_started_at timestamp with time zone DEFAULT now() NOT NULL,
    metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    CONSTRAINT chk_playbook_run_attempt CHECK ((attempt > 0)),
    CONSTRAINT chk_playbook_run_callback_usage CHECK (((callback_event_count >= 0) AND (callback_artifact_count >= 0) AND (callback_bytes >= 0) AND (callback_request_count >= 0))),
    CONSTRAINT chk_playbook_run_cancel_outcome CHECK (((cancel_outcome IS NULL) OR (cancel_outcome = ANY (ARRAY['not_dispatched'::text, 'runner_confirmed'::text, 'deadline_expired'::text])))),
    CONSTRAINT chk_playbook_run_cancel_outcome_request CHECK (((cancel_outcome IS NULL) OR (cancel_requested_at IS NOT NULL))),
    CONSTRAINT chk_playbook_run_credential_account_binding CHECK ((((credential_account_kind = ANY (ARRAY['auto'::text, 'none'::text])) AND (credential_account_id IS NULL)) OR ((credential_account_kind = ANY (ARRAY['connection_config'::text, 'shared_credential_set'::text])) AND (credential_account_id IS NOT NULL)))),
    CONSTRAINT chk_playbook_run_credential_account_entry CHECK (((credential_account_entry_id IS NULL) OR ((char_length(credential_account_entry_id) >= 1) AND (char_length(credential_account_entry_id) <= 240)))),
    CONSTRAINT chk_playbook_run_credential_account_kind CHECK ((credential_account_kind = ANY (ARRAY['auto'::text, 'connection_config'::text, 'shared_credential_set'::text, 'none'::text]))),
    CONSTRAINT chk_playbook_run_credential_merge_policy CHECK ((credential_merge_policy = ANY (ARRAY['context_first'::text, 'personal_first'::text]))),
    CONSTRAINT chk_playbook_run_credential_resolution_object CHECK ((jsonb_typeof(credential_resolution) = 'object'::text)),
    CONSTRAINT chk_playbook_run_metadata_object CHECK ((jsonb_typeof(metadata) = 'object'::text)),
    CONSTRAINT chk_playbook_run_source_kind CHECK ((source_kind = ANY (ARRAY['device'::text, 'virtual_machine'::text]))),
    CONSTRAINT chk_playbook_run_status CHECK ((status = ANY (ARRAY['queued'::text, 'leased'::text, 'dispatching'::text, 'running'::text, 'succeeded'::text, 'failed'::text, 'canceled'::text, 'timed_out'::text]))),
    CONSTRAINT chk_playbook_run_target_kind CHECK ((target_kind = 'connection_target'::text)),
    CONSTRAINT chk_playbook_run_timeout CHECK (((timeout_seconds >= 1) AND (timeout_seconds <= 86400)))
);


--
-- Name: playbook_run_event; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.playbook_run_event (
    id uuid DEFAULT uuidv7() NOT NULL,
    run_group_id uuid NOT NULL,
    run_id uuid,
    host text,
    event_sequence bigint NOT NULL,
    event_kind text NOT NULL,
    event_type text NOT NULL,
    message text,
    payload jsonb DEFAULT '{}'::jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT chk_playbook_run_event_host CHECK (((host IS NULL) OR ((char_length(host) >= 1) AND (char_length(host) <= 120)))),
    CONSTRAINT chk_playbook_run_event_kind CHECK ((event_kind = ANY (ARRAY['status'::text, 'stdout'::text, 'stderr'::text, 'system'::text, 'result'::text]))),
    CONSTRAINT chk_playbook_run_event_payload_object CHECK ((jsonb_typeof(payload) = 'object'::text)),
    CONSTRAINT chk_playbook_run_event_sequence CHECK ((event_sequence > 0)),
    CONSTRAINT chk_playbook_run_event_type CHECK (((char_length(event_type) >= 1) AND (char_length(event_type) <= 120)))
);


--
-- Name: playbook_run_group; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.playbook_run_group (
    id uuid DEFAULT uuidv7() NOT NULL,
    playbook_id uuid NOT NULL,
    playbook_version_id uuid NOT NULL,
    target_set_id uuid,
    schedule_id uuid,
    scheduled_for timestamp with time zone,
    requested_by_user_id uuid,
    service_principal_id uuid,
    trigger_kind text NOT NULL,
    status text DEFAULT 'queued'::text NOT NULL,
    total_runs integer DEFAULT 0 NOT NULL,
    succeeded_runs integer DEFAULT 0 NOT NULL,
    failed_runs integer DEFAULT 0 NOT NULL,
    canceled_runs integer DEFAULT 0 NOT NULL,
    timeout_runs integer DEFAULT 0 NOT NULL,
    event_sequence_counter bigint DEFAULT 0 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    started_at timestamp with time zone,
    finished_at timestamp with time zone,
    lease_owner text,
    lease_expires_at timestamp with time zone,
    leased_at timestamp with time zone,
    dispatch_id uuid,
    dispatch_attempt integer DEFAULT 0 NOT NULL,
    callback_event_count bigint DEFAULT 0 NOT NULL,
    callback_artifact_count bigint DEFAULT 0 NOT NULL,
    callback_bytes bigint DEFAULT 0 NOT NULL,
    callback_request_count integer DEFAULT 0 NOT NULL,
    callback_request_window_started_at timestamp with time zone DEFAULT now() NOT NULL,
    cancel_requested_at timestamp with time zone,
    cancel_deadline_at timestamp with time zone,
    cancel_outcome text,
    metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    CONSTRAINT chk_playbook_run_group_callback_usage CHECK (((callback_event_count >= 0) AND (callback_artifact_count >= 0) AND (callback_bytes >= 0) AND (callback_request_count >= 0))),
    CONSTRAINT chk_playbook_run_group_cancel_deadline CHECK (((cancel_deadline_at IS NULL) OR (cancel_requested_at IS NOT NULL))),
    CONSTRAINT chk_playbook_run_group_cancel_outcome CHECK (((cancel_outcome IS NULL) OR (cancel_outcome = ANY (ARRAY['not_dispatched'::text, 'runner_confirmed'::text, 'deadline_expired'::text])))),
    CONSTRAINT chk_playbook_run_group_cancel_outcome_request CHECK (((cancel_outcome IS NULL) OR (cancel_requested_at IS NOT NULL))),
    CONSTRAINT chk_playbook_run_group_counts CHECK (((total_runs >= 0) AND (succeeded_runs >= 0) AND (failed_runs >= 0) AND (canceled_runs >= 0) AND (timeout_runs >= 0))),
    CONSTRAINT chk_playbook_run_group_dispatch_attempt CHECK ((dispatch_attempt >= 0)),
    CONSTRAINT chk_playbook_run_group_dispatch_identity CHECK (((dispatch_id IS NULL) OR (dispatch_attempt > 0))),
    CONSTRAINT chk_playbook_run_group_event_sequence_counter CHECK ((event_sequence_counter >= 0)),
    CONSTRAINT chk_playbook_run_group_lease_owner CHECK (((lease_owner IS NULL) OR ((char_length(lease_owner) >= 1) AND (char_length(lease_owner) <= 120)))),
    CONSTRAINT chk_playbook_run_group_lease_pair CHECK ((((lease_owner IS NULL) AND (lease_expires_at IS NULL) AND (leased_at IS NULL)) OR ((lease_owner IS NOT NULL) AND (lease_expires_at IS NOT NULL) AND (leased_at IS NOT NULL)))),
    CONSTRAINT chk_playbook_run_group_metadata_object CHECK ((jsonb_typeof(metadata) = 'object'::text)),
    CONSTRAINT chk_playbook_run_group_schedule_binding CHECK (((trigger_kind = 'schedule'::text) = ((schedule_id IS NOT NULL) AND (scheduled_for IS NOT NULL)))),
    CONSTRAINT chk_playbook_run_group_status CHECK ((status = ANY (ARRAY['queued'::text, 'leased'::text, 'dispatching'::text, 'running'::text, 'succeeded'::text, 'failed'::text, 'canceled'::text, 'timed_out'::text]))),
    CONSTRAINT chk_playbook_run_group_trigger CHECK ((trigger_kind = ANY (ARRAY['manual'::text, 'schedule'::text, 'api'::text, 'mcp'::text, 'bulk'::text])))
);


--
-- Name: playbook_run_group_callback_token; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.playbook_run_group_callback_token (
    id uuid DEFAULT uuidv7() NOT NULL,
    run_group_id uuid NOT NULL,
    token_hash text NOT NULL,
    token_prefix text NOT NULL,
    status text DEFAULT 'active'::text NOT NULL,
    issued_at timestamp with time zone DEFAULT now() NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    last_used_at timestamp with time zone,
    revoked_at timestamp with time zone,
    metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    CONSTRAINT chk_playbook_run_group_callback_token_expiry CHECK ((expires_at > issued_at)),
    CONSTRAINT chk_playbook_run_group_callback_token_metadata_object CHECK ((jsonb_typeof(metadata) = 'object'::text)),
    CONSTRAINT chk_playbook_run_group_callback_token_prefix CHECK ((token_prefix ~ '^pjt_[A-Za-z0-9_-]{8}$'::text)),
    CONSTRAINT chk_playbook_run_group_callback_token_status CHECK ((status = ANY (ARRAY['active'::text, 'revoked'::text, 'expired'::text])))
);


--
-- Name: playbook_run_group_event_batch; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.playbook_run_group_event_batch (
    run_group_id uuid NOT NULL,
    batch_id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: playbook_schedule; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.playbook_schedule (
    id uuid DEFAULT uuidv7() NOT NULL,
    playbook_id uuid NOT NULL,
    target_set_id uuid NOT NULL,
    service_principal_id uuid NOT NULL,
    name text NOT NULL,
    cron_expression text NOT NULL,
    timezone text DEFAULT 'UTC'::text NOT NULL,
    misfire_policy text DEFAULT 'fire_once'::text NOT NULL,
    concurrency_policy text DEFAULT 'skip_if_running'::text NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    next_run_at timestamp with time zone,
    last_run_at timestamp with time zone,
    created_by_user_id uuid,
    updated_by_user_id uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    CONSTRAINT chk_playbook_schedule_concurrency CHECK ((concurrency_policy = ANY (ARRAY['skip_if_running'::text, 'queue_pending'::text, 'replace_pending'::text]))),
    CONSTRAINT chk_playbook_schedule_cron CHECK (((char_length(cron_expression) >= 9) AND (char_length(cron_expression) <= 200))),
    CONSTRAINT chk_playbook_schedule_due_order CHECK (((last_run_at IS NULL) OR (next_run_at IS NULL) OR (next_run_at > last_run_at))),
    CONSTRAINT chk_playbook_schedule_enabled_due CHECK (((NOT enabled) OR (next_run_at IS NOT NULL))),
    CONSTRAINT chk_playbook_schedule_metadata_object CHECK ((jsonb_typeof(metadata) = 'object'::text)),
    CONSTRAINT chk_playbook_schedule_misfire CHECK ((misfire_policy = 'fire_once'::text)),
    CONSTRAINT chk_playbook_schedule_name CHECK (((char_length(name) >= 1) AND (char_length(name) <= 200))),
    CONSTRAINT chk_playbook_schedule_timezone CHECK (((char_length(timezone) >= 1) AND (char_length(timezone) <= 120)))
);


--
-- Name: playbook_schedule_occurrence; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.playbook_schedule_occurrence (
    schedule_id uuid NOT NULL,
    scheduled_for timestamp with time zone NOT NULL,
    fired_at timestamp with time zone NOT NULL,
    lease_owner text,
    lease_expires_at timestamp with time zone,
    leased_at timestamp with time zone,
    created_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    CONSTRAINT chk_playbook_schedule_occurrence_fire_order CHECK ((fired_at >= scheduled_for)),
    CONSTRAINT chk_playbook_schedule_occurrence_lease_owner CHECK (((lease_owner IS NULL) OR ((char_length(lease_owner) >= 1) AND (char_length(lease_owner) <= 120)))),
    CONSTRAINT chk_playbook_schedule_occurrence_lease_pair CHECK ((((lease_owner IS NULL) AND (lease_expires_at IS NULL) AND (leased_at IS NULL)) OR ((lease_owner IS NOT NULL) AND (lease_expires_at IS NOT NULL) AND (leased_at IS NOT NULL))))
);


--
-- Name: playbook_service_principal; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.playbook_service_principal (
    id uuid DEFAULT uuidv7() NOT NULL,
    name text NOT NULL,
    slug text NOT NULL,
    description text,
    status text DEFAULT 'active'::text NOT NULL,
    created_by_user_id uuid,
    updated_by_user_id uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    revoked_at timestamp with time zone,
    revoked_by_user_id uuid,
    revocation_reason text,
    metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    CONSTRAINT chk_playbook_service_principal_description CHECK (((description IS NULL) OR (char_length(description) <= 2000))),
    CONSTRAINT chk_playbook_service_principal_metadata_object CHECK ((jsonb_typeof(metadata) = 'object'::text)),
    CONSTRAINT chk_playbook_service_principal_name CHECK (((char_length(name) >= 1) AND (char_length(name) <= 200))),
    CONSTRAINT chk_playbook_service_principal_revocation_reason CHECK (((revocation_reason IS NULL) OR (char_length(revocation_reason) <= 2000))),
    CONSTRAINT chk_playbook_service_principal_revoked_pair CHECK ((((status <> 'revoked'::text) AND (revoked_at IS NULL)) OR ((status = 'revoked'::text) AND (revoked_at IS NOT NULL)))),
    CONSTRAINT chk_playbook_service_principal_slug CHECK ((slug ~ '^[a-z0-9][a-z0-9_-]{1,78}[a-z0-9]$'::text)),
    CONSTRAINT chk_playbook_service_principal_status CHECK ((status = ANY (ARRAY['active'::text, 'disabled'::text, 'revoked'::text])))
);


--
-- Name: playbook_service_principal_credential_binding; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.playbook_service_principal_credential_binding (
    id uuid DEFAULT uuidv7() NOT NULL,
    service_principal_id uuid CONSTRAINT playbook_service_principal_creden_service_principal_id_not_null NOT NULL,
    consumer_type text DEFAULT 'playbook_service_principal:credential'::text CONSTRAINT playbook_service_principal_credential_bi_consumer_type_not_null NOT NULL,
    credential_account_kind text CONSTRAINT playbook_service_principal_cre_credential_account_kind_not_null NOT NULL,
    credential_account_id uuid CONSTRAINT playbook_service_principal_crede_credential_account_id_not_null NOT NULL,
    credential_account_entry_id text,
    field_key text CONSTRAINT playbook_service_principal_credential_bindin_field_key_not_null NOT NULL,
    merge_policy text DEFAULT 'context_first'::text CONSTRAINT playbook_service_principal_credential_bin_merge_policy_not_null NOT NULL,
    created_by_user_id uuid,
    updated_by_user_id uuid,
    created_at timestamp with time zone DEFAULT now() CONSTRAINT playbook_service_principal_credential_bindi_created_at_not_null NOT NULL,
    updated_at timestamp with time zone DEFAULT now() CONSTRAINT playbook_service_principal_credential_bindi_updated_at_not_null NOT NULL,
    metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    CONSTRAINT chk_playbook_service_principal_credential_account_kind CHECK ((credential_account_kind = ANY (ARRAY['connection_config'::text, 'shared_credential_set'::text]))),
    CONSTRAINT chk_playbook_service_principal_credential_consumer_type CHECK ((consumer_type = 'playbook_service_principal:credential'::text)),
    CONSTRAINT chk_playbook_service_principal_credential_entry CHECK (((credential_account_entry_id IS NULL) OR ((char_length(credential_account_entry_id) >= 1) AND (char_length(credential_account_entry_id) <= 240)))),
    CONSTRAINT chk_playbook_service_principal_credential_field CHECK ((field_key ~ '^[a-zA-Z0-9_.:-]{1,240}$'::text)),
    CONSTRAINT chk_playbook_service_principal_credential_merge CHECK ((merge_policy = ANY (ARRAY['context_first'::text, 'personal_first'::text]))),
    CONSTRAINT chk_playbook_service_principal_credential_metadata_object CHECK ((jsonb_typeof(metadata) = 'object'::text))
);


--
-- Name: playbook_target_set; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.playbook_target_set (
    id uuid DEFAULT uuidv7() NOT NULL,
    name text NOT NULL,
    mode text NOT NULL,
    entity_kind text DEFAULT 'connection_target'::text NOT NULL,
    selector jsonb DEFAULT '{}'::jsonb NOT NULL,
    created_by_user_id uuid,
    updated_by_user_id uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT chk_playbook_target_set_entity_kind CHECK ((entity_kind = 'connection_target'::text)),
    CONSTRAINT chk_playbook_target_set_mode CHECK ((mode = ANY (ARRAY['dynamic'::text, 'static'::text]))),
    CONSTRAINT chk_playbook_target_set_name CHECK (((char_length(name) >= 1) AND (char_length(name) <= 200))),
    CONSTRAINT chk_playbook_target_set_selector_object CHECK ((jsonb_typeof(selector) = 'object'::text))
);


--
-- Name: playbook_target_set_member; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.playbook_target_set_member (
    target_set_id uuid NOT NULL,
    target_kind text NOT NULL,
    target_id uuid NOT NULL,
    source_kind text NOT NULL,
    source_id uuid NOT NULL,
    sort_order integer DEFAULT 0 NOT NULL,
    added_by_user_id uuid,
    added_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT chk_playbook_target_set_member_kind CHECK ((target_kind = 'connection_target'::text)),
    CONSTRAINT chk_playbook_target_set_member_sort CHECK ((sort_order >= 0)),
    CONSTRAINT chk_playbook_target_set_member_source CHECK ((source_kind = ANY (ARRAY['device'::text, 'virtual_machine'::text])))
);


--
-- Name: playbook_version; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.playbook_version (
    id uuid DEFAULT uuidv7() NOT NULL,
    playbook_id uuid NOT NULL,
    version integer NOT NULL,
    language text DEFAULT 'yaml'::text NOT NULL,
    runtime text DEFAULT 'ansible'::text NOT NULL,
    entrypoint text,
    content text NOT NULL,
    content_sha256 text NOT NULL,
    input_schema jsonb DEFAULT '{}'::jsonb NOT NULL,
    output_schema jsonb DEFAULT '{}'::jsonb NOT NULL,
    token_policy jsonb DEFAULT '{"scopes": ["artifact:create"], "ttl_seconds": 3600}'::jsonb NOT NULL,
    timeout_seconds integer DEFAULT 1800 NOT NULL,
    changelog text,
    created_by_user_id uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    CONSTRAINT chk_playbook_version_content CHECK (((char_length(content) >= 1) AND (char_length(content) <= 1048576))),
    CONSTRAINT chk_playbook_version_hash CHECK ((content_sha256 ~ '^[a-f0-9]{64}$'::text)),
    CONSTRAINT chk_playbook_version_input_schema_object CHECK ((jsonb_typeof(input_schema) = 'object'::text)),
    CONSTRAINT chk_playbook_version_language CHECK ((language = 'yaml'::text)),
    CONSTRAINT chk_playbook_version_metadata_object CHECK ((jsonb_typeof(metadata) = 'object'::text)),
    CONSTRAINT chk_playbook_version_output_schema_object CHECK ((jsonb_typeof(output_schema) = 'object'::text)),
    CONSTRAINT chk_playbook_version_positive CHECK ((version > 0)),
    CONSTRAINT chk_playbook_version_runtime CHECK ((runtime = 'ansible'::text)),
    CONSTRAINT chk_playbook_version_timeout CHECK (((timeout_seconds >= 1) AND (timeout_seconds <= 86400))),
    CONSTRAINT chk_playbook_version_token_policy_object CHECK ((jsonb_typeof(token_policy) = 'object'::text))
);


--
-- Name: provider_request_journal; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.provider_request_journal (
    id uuid DEFAULT uuidv7() NOT NULL,
    provider_id uuid NOT NULL,
    outbound_event_uuid uuid NOT NULL,
    attempt_no integer NOT NULL,
    state character varying(32) NOT NULL,
    model character varying(255) NOT NULL,
    entity_identity text NOT NULL,
    payload_hash character varying(64) NOT NULL,
    netbox_revision_observed integer,
    stamped_external_request_id character varying(255),
    http_status integer,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    sent_at timestamp with time zone,
    completed_at timestamp with time zone,
    last_error text,
    CONSTRAINT provider_request_journal_attempt_no_check CHECK ((attempt_no >= 1)),
    CONSTRAINT provider_request_journal_http_status_check CHECK (((http_status IS NULL) OR ((http_status >= 100) AND (http_status <= 599)))),
    CONSTRAINT provider_request_journal_payload_hash_check CHECK (((payload_hash)::text ~ '^[0-9a-f]{64}$'::text)),
    CONSTRAINT provider_request_journal_state_check CHECK (((state)::text = ANY (ARRAY['prepared'::text, 'sent_unknown'::text, 'succeeded'::text, 'failed_no_send'::text, 'failed_after_send'::text])))
);


--
-- Name: rbac_policy; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.rbac_policy (
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(255) NOT NULL,
    effect character varying(10) NOT NULL,
    actions jsonb NOT NULL,
    resource_types jsonb NOT NULL,
    constraints jsonb,
    subjects jsonb DEFAULT '[]'::jsonb NOT NULL,
    resources jsonb DEFAULT '[]'::jsonb NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    description text,
    CONSTRAINT rbac_policy_effect_check CHECK (((effect)::text = ANY (ARRAY[('allow'::character varying)::text, ('deny'::character varying)::text])))
);


--
-- Name: rbac_role; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.rbac_role (
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(255) NOT NULL,
    description text
);


--
-- Name: rbac_role_assignment; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.rbac_role_assignment (
    id uuid DEFAULT uuidv7() NOT NULL,
    user_id uuid,
    group_id uuid,
    role_id uuid NOT NULL,
    CONSTRAINT rbac_role_assignment_check CHECK ((((user_id IS NOT NULL) AND (group_id IS NULL)) OR ((user_id IS NULL) AND (group_id IS NOT NULL))))
);


--
-- Name: secret_usage_binding; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.secret_usage_binding (
    id uuid DEFAULT uuidv7() NOT NULL,
    consumer_type character varying(64) NOT NULL,
    consumer_id uuid NOT NULL,
    field_key text NOT NULL,
    owner_scope_type character varying(64) NOT NULL,
    owner_scope_id uuid NOT NULL,
    secret_id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT chk_secret_usage_binding_monitoring_alert_source_shape CHECK ((((consumer_type)::text <> 'monitoring_alert_source'::text) OR ((field_key = 'bearer_token'::text) AND ((owner_scope_type)::text = 'monitoring_alert_source'::text) AND (owner_scope_id = consumer_id)))),
    CONSTRAINT chk_secret_usage_binding_monitoring_query_source_shape CHECK ((((consumer_type)::text <> 'monitoring_query_source'::text) OR ((field_key = 'bearer_token'::text) AND ((owner_scope_type)::text = 'monitoring_query_source'::text) AND (owner_scope_id = consumer_id)))),
    CONSTRAINT chk_secret_usage_binding_playbook_project_shape CHECK ((((consumer_type)::text <> 'playbook_project'::text) OR ((field_key = 'scm_credential'::text) AND ((owner_scope_type)::text = 'playbook_project'::text) AND (owner_scope_id = consumer_id)))),
    CONSTRAINT chk_secret_usage_binding_user_set_field_key CHECK ((((consumer_type)::text <> 'user_connection_credential_set'::text) OR (field_key = ANY (ARRAY['ssh_password'::text, 'ssh_private_key'::text, 'rdp_username'::text, 'rdp_password'::text, 'vnc_password'::text])))),
    CONSTRAINT chk_secret_usage_binding_user_set_owner_scope CHECK ((((consumer_type)::text <> 'user_connection_credential_set'::text) OR ((owner_scope_type)::text = 'user'::text)))
);


--
-- Name: secrets_binding; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.secrets_binding (
    id uuid DEFAULT uuidv7() NOT NULL,
    secret_id uuid NOT NULL,
    scope_type character varying(50) NOT NULL,
    scope_id uuid NOT NULL,
    weight integer DEFAULT 0 NOT NULL,
    managed_by_field_usage boolean DEFAULT false NOT NULL
);


--
-- Name: secrets_store; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.secrets_store (
    secret_id uuid NOT NULL,
    encrypted_value text NOT NULL
);


--
-- Name: secrets_variable; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.secrets_variable (
    id uuid DEFAULT uuidv7() NOT NULL,
    key character varying(255) NOT NULL,
    is_masked boolean DEFAULT true NOT NULL,
    is_protected boolean DEFAULT false NOT NULL,
    kind character varying(32) DEFAULT 'field'::character varying NOT NULL,
    description text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    created_by uuid,
    retire_with_field_usage boolean DEFAULT false NOT NULL,
    CONSTRAINT chk_secrets_variable_field_retirement CHECK (((NOT retire_with_field_usage) OR (((kind)::text = 'field'::text) AND ((key)::text ~~ 'secret-field/%'::text)))),
    CONSTRAINT chk_secrets_variable_kind CHECK (kind IN ('field', 'named'))
);


--
-- Name: settings_values; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.settings_values (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    scope_type text NOT NULL,
    user_id uuid,
    key text NOT NULL,
    value jsonb NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_by_user_id uuid,
    CONSTRAINT chk_settings_values_key_length CHECK (((char_length(key) >= 1) AND (char_length(key) <= 200))),
    CONSTRAINT chk_settings_values_key_shape CHECK ((key ~ '^[a-z][a-z0-9._-]*$'::text)),
    CONSTRAINT chk_settings_values_scope CHECK ((scope_type = ANY (ARRAY['instance'::text, 'user'::text]))),
    CONSTRAINT chk_settings_values_scope_user CHECK ((((scope_type = 'instance'::text) AND (user_id IS NULL)) OR ((scope_type = 'user'::text) AND (user_id IS NOT NULL))))
);


--
-- Name: ssh_host_key_pin; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ssh_host_key_pin (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    host text NOT NULL,
    port integer NOT NULL,
    key_type text NOT NULL,
    known_hosts_line text NOT NULL,
    fingerprint text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    last_seen_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: sync_conflict_record; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.sync_conflict_record (
    id uuid DEFAULT uuidv7() NOT NULL,
    source_id uuid NOT NULL,
    resource_type character varying(100) NOT NULL,
    resource_id character varying(255) NOT NULL,
    conflict_type character varying(50) NOT NULL,
    inbound_payload jsonb,
    outbound_payload jsonb,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: sync_error; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.sync_error (
    id uuid DEFAULT uuidv7() NOT NULL,
    run_id uuid NOT NULL,
    resource_type character varying(100) NOT NULL,
    resource_id character varying(255),
    error_message text NOT NULL
);


--
-- Name: sync_inventory_binding; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.sync_inventory_binding (
    source_provider_id uuid NOT NULL,
    source_entity character varying(128) NOT NULL,
    external_id character varying(256) NOT NULL,
    target_entity character varying(128) NOT NULL,
    local_id uuid NOT NULL,
    source_created_at timestamp with time zone,
    source_updated_at timestamp with time zone,
    last_seen_job_id uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: sync_job; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.sync_job (
    id uuid DEFAULT uuidv7() NOT NULL,
    provider_id uuid NOT NULL,
    mapping_version character varying(50) NOT NULL,
    status character varying(30) NOT NULL,
    started_at timestamp with time zone,
    finished_at timestamp with time zone,
    stats jsonb DEFAULT '{}'::jsonb NOT NULL
);


--
-- Name: sync_job_step; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.sync_job_step (
    id uuid DEFAULT uuidv7() NOT NULL,
    job_id uuid NOT NULL,
    step_key character varying(150) NOT NULL,
    status character varying(30) NOT NULL,
    progress_current integer DEFAULT 0 NOT NULL,
    progress_total integer,
    started_at timestamp with time zone,
    finished_at timestamp with time zone,
    metadata jsonb DEFAULT '{}'::jsonb NOT NULL
);


--
-- Name: sync_outbound_event; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.sync_outbound_event (
    id uuid DEFAULT uuidv7() NOT NULL,
    source_id uuid NOT NULL,
    resource_type character varying(100) NOT NULL,
    operation character varying(30) NOT NULL,
    payload jsonb NOT NULL,
    status character varying(30) DEFAULT 'pending'::character varying NOT NULL,
    attempts integer DEFAULT 0 NOT NULL,
    next_attempt_at timestamp with time zone,
    processed_at timestamp with time zone,
    correlation_id character varying(255),
    last_error text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: sync_provider; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.sync_provider (
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(255) NOT NULL,
    provider_type character varying(50) NOT NULL,
    config jsonb NOT NULL,
    is_enabled boolean DEFAULT true NOT NULL
);


--
-- Name: sync_run; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.sync_run (
    id uuid DEFAULT uuidv7() NOT NULL,
    job_id uuid NOT NULL,
    status character varying(30) NOT NULL,
    started_at timestamp with time zone DEFAULT now() NOT NULL,
    finished_at timestamp with time zone,
    changed_count integer DEFAULT 0 NOT NULL,
    failed_count integer DEFAULT 0 NOT NULL
);


--
-- Name: sync_source_runtime; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.sync_source_runtime (
    source_id uuid NOT NULL,
    bootstrap_status character varying(30) DEFAULT 'pending'::character varying NOT NULL,
    operational_status character varying(30) DEFAULT 'ready'::character varying NOT NULL,
    external_webhook_id character varying(255),
    webhook_last_received_at timestamp with time zone,
    last_error_code character varying(100),
    last_error_message text,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);


--
-- Name: sync_webhook_event; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.sync_webhook_event (
    id uuid DEFAULT uuidv7() NOT NULL,
    source_id uuid NOT NULL,
    dedup_key character varying(255) NOT NULL,
    event_type character varying(100) NOT NULL,
    model character varying(255) NOT NULL,
    request_id character varying(255),
    status character varying(30) DEFAULT 'accepted'::character varying NOT NULL,
    payload_hash character varying(64) NOT NULL,
    received_at timestamp with time zone DEFAULT now() NOT NULL,
    processed_at timestamp with time zone,
    last_error text,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT sync_webhook_event_payload_hash_check CHECK (((payload_hash)::text ~ '^[0-9a-f]{64}$'::text))
);


--
-- Name: tenancy_contact; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.tenancy_contact (
    id uuid DEFAULT uuidv7() NOT NULL,
    custom_field_data jsonb NOT NULL,
    name character varying(100) NOT NULL COLLATE public.natural_sort,
    title character varying(100) NOT NULL,
    phone character varying(50) NOT NULL,
    email character varying(254) NOT NULL,
    address character varying(200) NOT NULL,
    comments text NOT NULL,
    link character varying(200) NOT NULL,
    description character varying(200) NOT NULL,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone
);


--
-- Name: tenancy_contact_groups; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.tenancy_contact_groups (
    id uuid DEFAULT uuidv7() NOT NULL,
    contact_id uuid NOT NULL,
    contactgroup_id uuid NOT NULL
);


--
-- Name: tenancy_contactassignment; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.tenancy_contactassignment (
    id uuid DEFAULT uuidv7() NOT NULL,
    object_id uuid NOT NULL,
    priority character varying(50),
    contact_id uuid NOT NULL,
    object_type_id uuid NOT NULL,
    role_id uuid NOT NULL,
    custom_field_data jsonb NOT NULL,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone
);


--
-- Name: tenancy_contactgroup; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.tenancy_contactgroup (
    id uuid DEFAULT uuidv7() NOT NULL,
    custom_field_data jsonb NOT NULL,
    name character varying(100) NOT NULL,
    slug character varying(100) NOT NULL,
    description character varying(200) NOT NULL,
    lft integer NOT NULL,
    rght integer NOT NULL,
    tree_id integer NOT NULL,
    level integer NOT NULL,
    parent_id uuid,
    comments text NOT NULL,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone,
    CONSTRAINT tenancy_contactgroup_level_check CHECK ((level >= 0)),
    CONSTRAINT tenancy_contactgroup_lft_check CHECK ((lft >= 0)),
    CONSTRAINT tenancy_contactgroup_rght_check CHECK ((rght >= 0))
);


--
-- Name: tenancy_contactrole; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.tenancy_contactrole (
    id uuid DEFAULT uuidv7() NOT NULL,
    custom_field_data jsonb NOT NULL,
    name character varying(100) NOT NULL,
    slug character varying(100) NOT NULL,
    description character varying(200) NOT NULL,
    owner_id uuid,
    comments text NOT NULL,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone
);


--
-- Name: tenancy_tenant; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.tenancy_tenant (
    custom_field_data jsonb NOT NULL,
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(100) NOT NULL COLLATE public.natural_sort,
    slug character varying(100) NOT NULL,
    description character varying(200) NOT NULL,
    comments text NOT NULL,
    group_id uuid,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone
);


--
-- Name: tenancy_tenantgroup; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.tenancy_tenantgroup (
    custom_field_data jsonb NOT NULL,
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(100) NOT NULL COLLATE public.natural_sort,
    slug character varying(100) NOT NULL,
    description character varying(200) NOT NULL,
    lft integer NOT NULL,
    rght integer NOT NULL,
    tree_id integer NOT NULL,
    level integer NOT NULL,
    parent_id uuid,
    comments text NOT NULL,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone,
    CONSTRAINT tenancy_tenantgroup_level_check CHECK ((level >= 0)),
    CONSTRAINT tenancy_tenantgroup_lft_check CHECK ((lft >= 0)),
    CONSTRAINT tenancy_tenantgroup_rght_check CHECK ((rght >= 0))
);


--
-- Name: thumbnail_kvstore; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.thumbnail_kvstore (
    key character varying(200) NOT NULL,
    value text NOT NULL
);


--
-- Name: user_preferences; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.user_preferences (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    user_id uuid NOT NULL,
    key text NOT NULL,
    value jsonb NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT user_preferences_key_check CHECK (((char_length(key) >= 1) AND (char_length(key) <= 200)))
);


--
-- Name: users_owner; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.users_owner (
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(100) NOT NULL,
    description character varying(200) DEFAULT ''::character varying NOT NULL,
    group_id uuid
);


--
-- Name: users_owner_identity_group; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.users_owner_identity_group (
    owner_id uuid NOT NULL,
    group_id uuid NOT NULL
);


--
-- Name: users_owner_identity_user; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.users_owner_identity_user (
    owner_id uuid NOT NULL,
    user_id uuid NOT NULL
);


--
-- Name: users_ownergroup; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.users_ownergroup (
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(100) NOT NULL,
    description character varying(200) DEFAULT ''::character varying NOT NULL
);


--
-- Name: virtualization_cluster; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.virtualization_cluster (
    custom_field_data jsonb NOT NULL,
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(100) NOT NULL COLLATE public.natural_sort,
    comments text NOT NULL,
    group_id uuid,
    tenant_id uuid,
    type_id uuid NOT NULL,
    status character varying(50) NOT NULL,
    description character varying(200) NOT NULL,
    scope_id uuid,
    scope_type_id uuid,
    _location_id uuid,
    _region_id uuid,
    _site_id uuid,
    _site_group_id uuid,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone
);


--
-- Name: virtualization_clustergroup; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.virtualization_clustergroup (
    custom_field_data jsonb NOT NULL,
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(100) NOT NULL,
    slug character varying(100) NOT NULL,
    description character varying(200) NOT NULL,
    owner_id uuid,
    comments text NOT NULL,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone
);


--
-- Name: virtualization_clustertype; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.virtualization_clustertype (
    custom_field_data jsonb NOT NULL,
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(100) NOT NULL,
    slug character varying(100) NOT NULL,
    description character varying(200) NOT NULL,
    owner_id uuid,
    comments text NOT NULL,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone
);


--
-- Name: virtualization_virtualdisk; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.virtualization_virtualdisk (
    id uuid DEFAULT uuidv7() NOT NULL,
    custom_field_data jsonb NOT NULL,
    name character varying(64) NOT NULL COLLATE public.natural_sort,
    description character varying(200) NOT NULL,
    size integer NOT NULL,
    virtual_machine_id uuid NOT NULL,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone,
    CONSTRAINT virtualization_virtualdisk_size_check CHECK ((size >= 0))
);


--
-- Name: virtualization_virtualmachine; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.virtualization_virtualmachine (
    custom_field_data jsonb NOT NULL,
    id uuid DEFAULT uuidv7() NOT NULL,
    local_context_data jsonb,
    name character varying(64) NOT NULL COLLATE public.natural_sort,
    status character varying(50) NOT NULL,
    vcpus numeric(6,2),
    memory integer,
    disk integer,
    comments text NOT NULL,
    cluster_id uuid,
    platform_id uuid,
    primary_ip4_id uuid,
    primary_ip6_id uuid,
    role_id uuid,
    tenant_id uuid,
    site_id uuid,
    device_id uuid,
    description character varying(200) NOT NULL,
    interface_count bigint NOT NULL,
    config_template_id uuid,
    virtual_disk_count bigint NOT NULL,
    serial character varying(50) NOT NULL,
    owner_id uuid,
    start_on_boot character varying(32) NOT NULL,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone,
    virtual_machine_type_id uuid,
    CONSTRAINT virtualization_virtualmachine_disk_check CHECK ((disk >= 0)),
    CONSTRAINT virtualization_virtualmachine_memory_check CHECK ((memory >= 0))
);


--
-- Name: virtualization_virtualmachinetype; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.virtualization_virtualmachinetype (
    id uuid DEFAULT uuidv7() NOT NULL,
    custom_field_data jsonb NOT NULL,
    name character varying(100) NOT NULL,
    slug character varying(100) NOT NULL,
    default_platform_id uuid,
    default_vcpus numeric(6,2),
    default_memory integer,
    description character varying(200) NOT NULL,
    owner_id uuid,
    comments text NOT NULL,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    created timestamp with time zone,
    last_updated timestamp with time zone,
    external_id bigint,
    piping_synced_at timestamp with time zone,
    CONSTRAINT virtualization_virtualmachinetype_default_memory_check CHECK ((default_memory >= 0)),
    CONSTRAINT virtualization_virtualmachinetype_default_vcpus_check CHECK ((default_vcpus >= 0.01)),
    CONSTRAINT virtualization_virtualmachinetype_finite_vcpus_check CHECK ((default_vcpus <> 'NaN'::numeric))
);


--
-- Name: virtualization_vminterface; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.virtualization_vminterface (
    custom_field_data jsonb NOT NULL,
    id uuid DEFAULT uuidv7() NOT NULL,
    enabled boolean NOT NULL,
    mtu integer,
    mode character varying(50),
    name character varying(64) NOT NULL,
    _name character varying(100) NOT NULL,
    description character varying(200) NOT NULL,
    parent_id uuid,
    untagged_vlan_id uuid,
    virtual_machine_id uuid NOT NULL,
    bridge_id uuid,
    vrf_id uuid,
    vlan_translation_policy_id uuid,
    qinq_svlan_id uuid,
    primary_mac_address_id uuid,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone,
    CONSTRAINT virtualization_vminterface_mtu_check CHECK ((mtu >= 0))
);


--
-- Name: virtualization_vminterface_tagged_vlans; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.virtualization_vminterface_tagged_vlans (
    id uuid DEFAULT uuidv7() NOT NULL,
    vminterface_id uuid NOT NULL,
    vlan_id uuid NOT NULL
);


--
-- Name: vpn_ikepolicy; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.vpn_ikepolicy (
    id uuid DEFAULT uuidv7() NOT NULL,
    custom_field_data jsonb NOT NULL,
    description character varying(200) NOT NULL,
    comments text NOT NULL,
    name character varying(100) NOT NULL COLLATE public.natural_sort,
    version smallint NOT NULL,
    mode character varying,
    preshared_key text NOT NULL,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone,
    CONSTRAINT vpn_ikepolicy_version_check CHECK ((version >= 0))
);


--
-- Name: vpn_ikepolicy_proposals; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.vpn_ikepolicy_proposals (
    id uuid DEFAULT uuidv7() NOT NULL,
    ikepolicy_id uuid NOT NULL,
    ikeproposal_id uuid NOT NULL
);


--
-- Name: vpn_ikeproposal; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.vpn_ikeproposal (
    id uuid DEFAULT uuidv7() NOT NULL,
    custom_field_data jsonb NOT NULL,
    description character varying(200) NOT NULL,
    comments text NOT NULL,
    name character varying(100) NOT NULL COLLATE public.natural_sort,
    authentication_method character varying NOT NULL,
    encryption_algorithm character varying NOT NULL,
    authentication_algorithm character varying,
    "group" smallint NOT NULL,
    sa_lifetime integer,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone,
    CONSTRAINT vpn_ikeproposal_group_check CHECK (("group" >= 0)),
    CONSTRAINT vpn_ikeproposal_sa_lifetime_check CHECK ((sa_lifetime >= 0))
);


--
-- Name: vpn_ipsecpolicy; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.vpn_ipsecpolicy (
    id uuid DEFAULT uuidv7() NOT NULL,
    custom_field_data jsonb NOT NULL,
    description character varying(200) NOT NULL,
    comments text NOT NULL,
    name character varying(100) NOT NULL COLLATE public.natural_sort,
    pfs_group smallint,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone,
    CONSTRAINT vpn_ipsecpolicy_pfs_group_check CHECK ((pfs_group >= 0))
);


--
-- Name: vpn_ipsecpolicy_proposals; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.vpn_ipsecpolicy_proposals (
    id uuid DEFAULT uuidv7() NOT NULL,
    ipsecpolicy_id uuid NOT NULL,
    ipsecproposal_id uuid NOT NULL
);


--
-- Name: vpn_ipsecprofile; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.vpn_ipsecprofile (
    id uuid DEFAULT uuidv7() NOT NULL,
    custom_field_data jsonb NOT NULL,
    description character varying(200) NOT NULL,
    comments text NOT NULL,
    name character varying(100) NOT NULL COLLATE public.natural_sort,
    mode character varying NOT NULL,
    ike_policy_id uuid NOT NULL,
    ipsec_policy_id uuid NOT NULL,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone
);


--
-- Name: vpn_ipsecproposal; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.vpn_ipsecproposal (
    id uuid DEFAULT uuidv7() NOT NULL,
    custom_field_data jsonb NOT NULL,
    description character varying(200) NOT NULL,
    comments text NOT NULL,
    name character varying(100) NOT NULL COLLATE public.natural_sort,
    encryption_algorithm character varying,
    authentication_algorithm character varying,
    sa_lifetime_seconds integer,
    sa_lifetime_data integer,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone,
    CONSTRAINT vpn_ipsecproposal_sa_lifetime_data_check CHECK ((sa_lifetime_data >= 0)),
    CONSTRAINT vpn_ipsecproposal_sa_lifetime_seconds_check CHECK ((sa_lifetime_seconds >= 0))
);


--
-- Name: vpn_l2vpn; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.vpn_l2vpn (
    id uuid DEFAULT uuidv7() NOT NULL,
    custom_field_data jsonb NOT NULL,
    name character varying(100) NOT NULL COLLATE public.natural_sort,
    slug character varying(100) NOT NULL,
    type character varying(50) NOT NULL,
    identifier bigint,
    description character varying(200) NOT NULL,
    tenant_id uuid,
    comments text NOT NULL,
    status character varying(50) NOT NULL,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone
);


--
-- Name: vpn_l2vpn_export_targets; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.vpn_l2vpn_export_targets (
    id uuid DEFAULT uuidv7() NOT NULL,
    l2vpn_id uuid NOT NULL,
    routetarget_id uuid NOT NULL
);


--
-- Name: vpn_l2vpn_import_targets; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.vpn_l2vpn_import_targets (
    id uuid DEFAULT uuidv7() NOT NULL,
    l2vpn_id uuid NOT NULL,
    routetarget_id uuid NOT NULL
);


--
-- Name: vpn_l2vpntermination; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.vpn_l2vpntermination (
    id uuid DEFAULT uuidv7() NOT NULL,
    custom_field_data jsonb NOT NULL,
    assigned_object_id uuid NOT NULL,
    assigned_object_type_id uuid NOT NULL,
    l2vpn_id uuid NOT NULL,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone
);


--
-- Name: vpn_tunnel; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.vpn_tunnel (
    id uuid DEFAULT uuidv7() NOT NULL,
    custom_field_data jsonb NOT NULL,
    description character varying(200) NOT NULL,
    comments text NOT NULL,
    name character varying(100) NOT NULL COLLATE public.natural_sort,
    status character varying(50) NOT NULL,
    group_id uuid,
    encapsulation character varying(50) NOT NULL,
    tunnel_id bigint,
    ipsec_profile_id uuid,
    tenant_id uuid,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone
);


--
-- Name: vpn_tunnelgroup; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.vpn_tunnelgroup (
    id uuid DEFAULT uuidv7() NOT NULL,
    custom_field_data jsonb NOT NULL,
    name character varying(100) NOT NULL,
    slug character varying(100) NOT NULL,
    description character varying(200) NOT NULL,
    owner_id uuid,
    comments text NOT NULL,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone
);


--
-- Name: vpn_tunneltermination; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.vpn_tunneltermination (
    id uuid DEFAULT uuidv7() NOT NULL,
    custom_field_data jsonb NOT NULL,
    role character varying(50) NOT NULL,
    termination_id uuid,
    termination_type_id uuid NOT NULL,
    outside_ip_id uuid,
    tunnel_id uuid NOT NULL,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    external_id bigint,
    piping_synced_at timestamp with time zone
);


--
-- Name: webhook_subscriptions; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.webhook_subscriptions (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    name text NOT NULL,
    target_url text NOT NULL,
    secret text,
    event_types text[] NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    last_fired_at timestamp with time zone,
    last_status integer,
    last_error text,
    conditions jsonb
);


--
-- Name: wireless_wirelesslan; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.wireless_wirelesslan (
    id uuid DEFAULT uuidv7() NOT NULL,
    custom_field_data jsonb NOT NULL,
    ssid character varying(32) NOT NULL,
    group_id uuid,
    status character varying(50) DEFAULT 'active'::character varying NOT NULL,
    vlan_id uuid,
    tenant_id uuid,
    scope_type_id uuid,
    scope_id uuid,
    auth_type character varying(50),
    auth_cipher character varying(50),
    auth_psk character varying(64) DEFAULT ''::character varying NOT NULL,
    description character varying(200) NOT NULL,
    comments text NOT NULL,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    created timestamp with time zone,
    last_updated timestamp with time zone,
    external_id bigint,
    piping_synced_at timestamp with time zone
);


--
-- Name: wireless_wirelesslangroup; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.wireless_wirelesslangroup (
    id uuid DEFAULT uuidv7() NOT NULL,
    custom_field_data jsonb NOT NULL,
    name character varying(100) NOT NULL COLLATE public.natural_sort,
    slug character varying(100) NOT NULL,
    description character varying(200) NOT NULL,
    lft integer NOT NULL,
    rght integer NOT NULL,
    tree_id integer NOT NULL,
    level integer NOT NULL,
    parent_id uuid,
    comments text NOT NULL,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    created timestamp with time zone,
    last_updated timestamp with time zone,
    external_id bigint,
    piping_synced_at timestamp with time zone,
    CONSTRAINT wireless_wirelesslangroup_level_check CHECK ((level >= 0)),
    CONSTRAINT wireless_wirelesslangroup_lft_check CHECK ((lft >= 0)),
    CONSTRAINT wireless_wirelesslangroup_rght_check CHECK ((rght >= 0))
);


--
-- Name: wireless_wirelesslink; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.wireless_wirelesslink (
    id uuid DEFAULT uuidv7() NOT NULL,
    custom_field_data jsonb NOT NULL,
    interface_a_id uuid NOT NULL,
    interface_b_id uuid NOT NULL,
    ssid character varying(32) DEFAULT ''::character varying NOT NULL,
    status character varying(50) DEFAULT 'connected'::character varying NOT NULL,
    tenant_id uuid,
    auth_type character varying(50),
    auth_cipher character varying(50),
    auth_psk character varying(64) DEFAULT ''::character varying NOT NULL,
    distance numeric(8,2),
    distance_unit character varying(50),
    description character varying(200) NOT NULL,
    comments text NOT NULL,
    owner_id uuid,
    tags text[] DEFAULT ARRAY[]::text[] NOT NULL,
    created timestamp with time zone,
    last_updated timestamp with time zone,
    external_id bigint,
    piping_synced_at timestamp with time zone
);


--
-- Name: wizard_progress; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.wizard_progress (
    id uuid DEFAULT uuidv7() NOT NULL,
    user_id uuid NOT NULL,
    wizard_type character varying(50) NOT NULL,
    current_step character varying(100),
    step_data jsonb DEFAULT '{}'::jsonb NOT NULL,
    status character varying(20) DEFAULT 'active'::character varying NOT NULL,
    completed_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT wizard_progress_status_check CHECK (((status)::text = ANY (ARRAY[('active'::character varying)::text, ('completed'::character varying)::text, ('dismissed'::character varying)::text])))
);


--
-- Name: workspace_connection_target_layout; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.workspace_connection_target_layout (
    id uuid DEFAULT uuidv7() NOT NULL,
    name character varying(255) NOT NULL,
    blocks jsonb NOT NULL,
    visibility_rules jsonb,
    is_shared boolean DEFAULT false NOT NULL
);


--
-- Data for Name: assistant_messages; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: assistant_sessions; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: audit_event; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: auth_oidc_settings; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: auth_session; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: auth_user_api_token; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: auth_user_api_token_scope; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: auth_user_credential; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: automation_template; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: circuits_circuit; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: circuits_circuitgroup; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: circuits_circuitgroupassignment; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: circuits_circuittermination; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: circuits_circuittype; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: circuits_provider; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: circuits_provider_asns; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: circuits_provideraccount; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: circuits_providernetwork; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: circuits_virtualcircuit; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: circuits_virtualcircuittermination; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: circuits_virtualcircuittype; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: config_context; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: config_context_binding; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: connconf_assignment; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.connconf_assignment (id, record_id, scope_type, scope_id, weight, enabled, created_at) VALUES ('00000000-0000-7000-8000-000000001211', '00000000-0000-7000-8000-000000001201', 'global', NULL, 0, false, CURRENT_TIMESTAMP);
INSERT INTO public.connconf_assignment (id, record_id, scope_type, scope_id, weight, enabled, created_at) VALUES ('00000000-0000-7000-8000-000000001212', '00000000-0000-7000-8000-000000001202', 'global', NULL, 0, false, CURRENT_TIMESTAMP);
INSERT INTO public.connconf_assignment (id, record_id, scope_type, scope_id, weight, enabled, created_at) VALUES ('00000000-0000-7000-8000-000000001213', '00000000-0000-7000-8000-000000001203', 'global', NULL, 0, false, CURRENT_TIMESTAMP);
INSERT INTO public.connconf_assignment (id, record_id, scope_type, scope_id, weight, enabled, created_at) VALUES ('00000000-0000-7000-8000-000000001214', '00000000-0000-7000-8000-000000001204', 'global', NULL, 0, false, CURRENT_TIMESTAMP);


--
-- Data for Name: connconf_record; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.connconf_record (id, name, description, protocol, base_record_id, enabled, owner_user_id, params, custom_field_data, created_at, updated_at) VALUES ('00000000-0000-7000-8000-000000001201', 'SSH via primary IP', 'Disabled factory SSH recipe. Enable it after assigning credentials or user credential sets.', 'ssh', NULL, false, NULL, '{"host": "{{primary_ip4}}", "port": 22}', '{}', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);
INSERT INTO public.connconf_record (id, name, description, protocol, base_record_id, enabled, owner_user_id, params, custom_field_data, created_at, updated_at) VALUES ('00000000-0000-7000-8000-000000001202', 'RDP via primary IP', 'Disabled factory RDP recipe. Enable it after assigning credentials or user credential sets.', 'rdp', NULL, false, NULL, '{"host": "{{primary_ip4}}", "port": 3389}', '{}', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);
INSERT INTO public.connconf_record (id, name, description, protocol, base_record_id, enabled, owner_user_id, params, custom_field_data, created_at, updated_at) VALUES ('00000000-0000-7000-8000-000000001203', 'VNC via primary IP', 'Disabled factory VNC recipe. Enable it after assigning credentials or user credential sets.', 'vnc', NULL, false, NULL, '{"host": "{{primary_ip4}}", "port": 5900}', '{}', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);
INSERT INTO public.connconf_record (id, name, description, protocol, base_record_id, enabled, owner_user_id, params, custom_field_data, created_at, updated_at) VALUES ('00000000-0000-7000-8000-000000001204', 'Open Grafana host dashboard', 'Disabled factory APP recipe. Replace the example URL and enable when a dashboard exists.', 'app', NULL, false, NULL, '{"url": "https://grafana.example.invalid/d/host?var-host={{target.name}}&var-site={{target.site.slug}}"}', '{}', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);


--
-- Data for Name: connection_filter_presets; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: connections_proxy_assignment; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: connections_proxy_node; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: connections_session; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: connections_template; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: connections_user_credential_set; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: custom_field; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: custom_field_choice_set; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: custom_field_object_type; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_cable; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_cablebundle; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_cablepath; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_cabletermination; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_consoleport; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_consoleporttemplate; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_consoleserverport; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_consoleserverporttemplate; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_coolingfeed; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_coolingintake; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_coolingintaketemplate; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_coolingoutflow; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_coolingoutflowtemplate; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_coolingsource; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_device; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_devicebay; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_devicebaytemplate; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_devicerole; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.dcim_devicerole (custom_field_data, id, name, slug, color, vm_role, description, config_template_id, level, lft, rght, tree_id, parent_id, comments, owner_id, tags, external_id, piping_synced_at) VALUES ('{}', '00000000-0000-7000-8000-000000001011', 'Server', 'server', '4caf50', false, 'Factory device role for managed physical or bare-metal servers.', NULL, 0, 0, 0, 0, NULL, '', NULL, '{factory-default}', NULL, NULL);
INSERT INTO public.dcim_devicerole (custom_field_data, id, name, slug, color, vm_role, description, config_template_id, level, lft, rght, tree_id, parent_id, comments, owner_id, tags, external_id, piping_synced_at) VALUES ('{}', '00000000-0000-7000-8000-000000001012', 'Workstation', 'workstation', '2196f3', false, 'Factory device role for operator or office endpoints.', NULL, 0, 0, 0, 0, NULL, '', NULL, '{factory-default}', NULL, NULL);
INSERT INTO public.dcim_devicerole (custom_field_data, id, name, slug, color, vm_role, description, config_template_id, level, lft, rght, tree_id, parent_id, comments, owner_id, tags, external_id, piping_synced_at) VALUES ('{}', '00000000-0000-7000-8000-000000001013', 'Network device', 'network-device', '607d8b', false, 'Factory device role for switches, routers, firewalls, and appliances.', NULL, 0, 0, 0, 0, NULL, '', NULL, '{factory-default}', NULL, NULL);
INSERT INTO public.dcim_devicerole (custom_field_data, id, name, slug, color, vm_role, description, config_template_id, level, lft, rght, tree_id, parent_id, comments, owner_id, tags, external_id, piping_synced_at) VALUES ('{}', '00000000-0000-7000-8000-000000001014', 'Virtual machine', 'virtual-machine', '9c27b0', true, 'Factory role for virtual-machine inventory records.', NULL, 0, 0, 0, 0, NULL, '', NULL, '{factory-default}', NULL, NULL);


--
-- Data for Name: dcim_devicetype; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.dcim_devicetype (custom_field_data, id, model, slug, part_number, u_height, is_full_depth, subdevice_role, front_image, rear_image, comments, manufacturer_id, airflow, weight, weight_unit, _abs_weight, description, default_platform_id, console_port_template_count, console_server_port_template_count, power_port_template_count, power_outlet_template_count, interface_template_count, front_port_template_count, rear_port_template_count, device_bay_template_count, module_bay_template_count, inventory_item_template_count, exclude_from_utilization, owner_id, device_count, tags, external_id, piping_synced_at) VALUES ('{}', '00000000-0000-7000-8000-000000001071', 'Generic server', 'generic-server', '', 1.0, false, NULL, '', '', '', '00000000-0000-7000-8000-000000001001', NULL, NULL, NULL, NULL, 'Factory fallback device type for servers.', NULL, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, false, NULL, 0, '{factory-default}', NULL, NULL);
INSERT INTO public.dcim_devicetype (custom_field_data, id, model, slug, part_number, u_height, is_full_depth, subdevice_role, front_image, rear_image, comments, manufacturer_id, airflow, weight, weight_unit, _abs_weight, description, default_platform_id, console_port_template_count, console_server_port_template_count, power_port_template_count, power_outlet_template_count, interface_template_count, front_port_template_count, rear_port_template_count, device_bay_template_count, module_bay_template_count, inventory_item_template_count, exclude_from_utilization, owner_id, device_count, tags, external_id, piping_synced_at) VALUES ('{}', '00000000-0000-7000-8000-000000001072', 'Generic workstation', 'generic-workstation', '', 0.0, false, NULL, '', '', '', '00000000-0000-7000-8000-000000001001', NULL, NULL, NULL, NULL, 'Factory fallback device type for endpoints.', NULL, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, false, NULL, 0, '{factory-default}', NULL, NULL);
INSERT INTO public.dcim_devicetype (custom_field_data, id, model, slug, part_number, u_height, is_full_depth, subdevice_role, front_image, rear_image, comments, manufacturer_id, airflow, weight, weight_unit, _abs_weight, description, default_platform_id, console_port_template_count, console_server_port_template_count, power_port_template_count, power_outlet_template_count, interface_template_count, front_port_template_count, rear_port_template_count, device_bay_template_count, module_bay_template_count, inventory_item_template_count, exclude_from_utilization, owner_id, device_count, tags, external_id, piping_synced_at) VALUES ('{}', '00000000-0000-7000-8000-000000001073', 'Generic network appliance', 'generic-network-appliance', '', 1.0, false, NULL, '', '', '', '00000000-0000-7000-8000-000000001001', NULL, NULL, NULL, NULL, 'Factory fallback device type for network appliances.', NULL, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, false, NULL, 0, '{factory-default}', NULL, NULL);


--
-- Data for Name: dcim_frontport; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_frontporttemplate; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_interface; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_interface_tagged_vlans; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_interface_vdcs; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_interface_wirelesslans; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_interfacetemplate; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_inventoryitem; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_inventoryitemrole; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_inventoryitemtemplate; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_location; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_macaddress; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_manufacturer; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.dcim_manufacturer (custom_field_data, id, name, slug, description, owner_id, comments, tags, external_id, piping_synced_at) VALUES ('{}', '00000000-0000-7000-8000-000000001001', 'Generic', 'generic', 'Neutral factory manufacturer for first-run inventory templates.', NULL, '', '{factory-default}', NULL, NULL);


--
-- Data for Name: dcim_module; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_modulebay; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_modulebaytemplate; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_modulebaytype; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_moduletype; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_moduletypeprofile; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_platform; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.dcim_platform (custom_field_data, id, name, slug, description, manufacturer_id, config_template_id, parent_id, level, lft, rght, tree_id, comments, owner_id, tags, external_id, piping_synced_at, napalm_driver) VALUES ('{}', '00000000-0000-7000-8000-000000001062', 'Windows', 'windows', 'Generic Windows platform.', '00000000-0000-7000-8000-000000001001', NULL, NULL, 0, 0, 0, 0, '', NULL, '{factory-default}', NULL, NULL, NULL);
INSERT INTO public.dcim_platform (custom_field_data, id, name, slug, description, manufacturer_id, config_template_id, parent_id, level, lft, rght, tree_id, comments, owner_id, tags, external_id, piping_synced_at, napalm_driver) VALUES ('{}', '00000000-0000-7000-8000-000000001063', 'Network OS', 'network-os', 'Generic network operating-system platform.', '00000000-0000-7000-8000-000000001001', NULL, NULL, 0, 0, 0, 0, '', NULL, '{factory-default}', NULL, NULL, NULL);
INSERT INTO public.dcim_platform (custom_field_data, id, name, slug, description, manufacturer_id, config_template_id, parent_id, level, lft, rght, tree_id, comments, owner_id, tags, external_id, piping_synced_at, napalm_driver) VALUES ('{}', '00000000-0000-7000-8000-000000001064', 'Hypervisor', 'hypervisor', 'Generic virtualization host platform.', '00000000-0000-7000-8000-000000001001', NULL, NULL, 0, 0, 0, 0, '', NULL, '{factory-default}', NULL, NULL, NULL);
INSERT INTO public.dcim_platform (custom_field_data, id, name, slug, description, manufacturer_id, config_template_id, parent_id, level, lft, rght, tree_id, comments, owner_id, tags, external_id, piping_synced_at, napalm_driver) VALUES ('{}', 'aa000000-0000-7000-8000-0000000d0001', 'Video DVRIP Sofia', 'video-dvrip-sofia', 'CCTV recorders/cameras speaking DVRIP/Sofia (XMeye family) on TCP 34567.', '00000000-0000-7000-8000-000000001001', NULL, NULL, 0, 0, 0, 0, '', NULL, '{factory-default}', NULL, NULL, 'dvrip_sofia');
INSERT INTO public.dcim_platform (custom_field_data, id, name, slug, description, manufacturer_id, config_template_id, parent_id, level, lft, rght, tree_id, comments, owner_id, tags, external_id, piping_synced_at, napalm_driver) VALUES ('{}', 'aa000000-0000-7000-8000-0000000d0002', 'Video Dahua', 'video-dahua', 'CCTV recorders/cameras speaking the Dahua HTTP CGI plane.', '00000000-0000-7000-8000-000000001001', NULL, NULL, 0, 0, 0, 0, '', NULL, '{factory-default}', NULL, NULL, 'dahua');
INSERT INTO public.dcim_platform (custom_field_data, id, name, slug, description, manufacturer_id, config_template_id, parent_id, level, lft, rght, tree_id, comments, owner_id, tags, external_id, piping_synced_at, napalm_driver) VALUES ('{}', 'aa000000-0000-7000-8000-0000000d0003', 'Video Hikvision', 'video-hikvision', 'CCTV recorders/cameras speaking the Hikvision ISAPI plane.', '00000000-0000-7000-8000-000000001001', NULL, NULL, 0, 0, 0, 0, '', NULL, '{factory-default}', NULL, NULL, 'hikvision');
INSERT INTO public.dcim_platform (custom_field_data, id, name, slug, description, manufacturer_id, config_template_id, parent_id, level, lft, rght, tree_id, comments, owner_id, tags, external_id, piping_synced_at, napalm_driver) VALUES ('{}', 'aa000000-0000-7000-8000-0000000d0004', 'Video ONVIF', 'video-onvif', 'Generic ONVIF cameras — reliable live/snapshot, partial administration.', '00000000-0000-7000-8000-000000001001', NULL, NULL, 0, 0, 0, 0, '', NULL, '{factory-default}', NULL, NULL, 'onvif');
INSERT INTO public.dcim_platform (custom_field_data, id, name, slug, description, manufacturer_id, config_template_id, parent_id, level, lft, rght, tree_id, comments, owner_id, tags, external_id, piping_synced_at, napalm_driver) VALUES ('{}', '00000000-0000-7000-8000-000000001061', 'Linux', 'linux', 'Generic Linux platform.', '00000000-0000-7000-8000-000000001001', NULL, NULL, 0, 0, 0, 0, '', NULL, '{factory-default}', NULL, NULL, 'linux');


--
-- Data for Name: dcim_portmapping; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_porttemplatemapping; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_powerfeed; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_poweroutlet; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_poweroutlettemplate; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_powerpanel; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_powerport; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_powerporttemplate; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_rack; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_rackgroup; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_rackreservation; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_rackrole; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.dcim_rackrole (custom_field_data, id, name, slug, color, description, owner_id, comments, tags, external_id, piping_synced_at) VALUES ('{}', '00000000-0000-7000-8000-000000001021', 'General rack', 'general-rack', '9e9e9e', 'Factory rack role for mixed equipment.', NULL, '', '{factory-default}', NULL, NULL);
INSERT INTO public.dcim_rackrole (custom_field_data, id, name, slug, color, description, owner_id, comments, tags, external_id, piping_synced_at) VALUES ('{}', '00000000-0000-7000-8000-000000001022', 'Network rack', 'network-rack', '607d8b', 'Factory rack role for network equipment.', NULL, '', '{factory-default}', NULL, NULL);
INSERT INTO public.dcim_rackrole (custom_field_data, id, name, slug, color, description, owner_id, comments, tags, external_id, piping_synced_at) VALUES ('{}', '00000000-0000-7000-8000-000000001023', 'Compute rack', 'compute-rack', '4caf50', 'Factory rack role for compute equipment.', NULL, '', '{factory-default}', NULL, NULL);


--
-- Data for Name: dcim_racktype; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.dcim_racktype (id, custom_field_data, description, comments, weight, weight_unit, _abs_weight, manufacturer_id, model, slug, form_factor, width, u_height, starting_unit, desc_units, outer_width, outer_depth, outer_unit, max_weight, _abs_max_weight, mounting_depth, outer_height, owner_id, rack_count, tags, external_id, piping_synced_at) VALUES ('00000000-0000-7000-8000-000000001081', '{}', 'Factory fallback rack type.', '', NULL, NULL, NULL, '00000000-0000-7000-8000-000000001001', 'Generic 42U rack', 'generic-42u-rack', '4-post-frame', 19, 42, 1, false, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, '{factory-default}', NULL, NULL);


--
-- Data for Name: dcim_rearport; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_rearporttemplate; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_region; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_site; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_site_asns; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_sitegroup; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_virtualchassis; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: dcim_virtualdevicecontext; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: entity_mappings; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: field_mappings; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: identity_group; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.identity_group (id, name, external_id) VALUES ('019ee0e0-605b-7c56-98ac-4e78e5786c8d', 'piping', '/piping');
INSERT INTO public.identity_group (id, name, external_id) VALUES ('019ee0e0-605c-7f88-86f6-86bb160e0a70', 'piping-viewer', '/piping-viewer');
INSERT INTO public.identity_group (id, name, external_id) VALUES ('019ee0e0-605d-7b47-baf8-704e6b94501a', 'piping-editor', '/piping-editor');
INSERT INTO public.identity_group (id, name, external_id) VALUES ('019ee0e0-605e-7b2c-aff9-941de10cb0a9', 'piping-admin', '/piping-admin');


--
-- Data for Name: identity_group_membership; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: identity_user; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: instance_branding; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.instance_branding (id, app_name, logo_data_url, favicon_data_url, theme, updated_at, updated_by) VALUES (1, 'Piping', NULL, NULL, '{}', '2026-06-19 17:14:23.627306+00', NULL);


--
-- Data for Name: inventory_content_type; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.inventory_content_type (id, app_label, model) VALUES ('019ee0e0-75d9-73ce-bff8-0a5a9f792734', 'dcim', 'device');
INSERT INTO public.inventory_content_type (id, app_label, model) VALUES ('019ee0e0-75da-7289-b180-9eb729773c6d', 'dcim', 'site');
INSERT INTO public.inventory_content_type (id, app_label, model) VALUES ('019ee0e0-75da-72e3-866d-054815f47bdd', 'dcim', 'region');
INSERT INTO public.inventory_content_type (id, app_label, model) VALUES ('019ee0e0-75da-7303-af1b-17d04f1636b7', 'dcim', 'sitegroup');
INSERT INTO public.inventory_content_type (id, app_label, model) VALUES ('019ee0e0-75da-7324-b547-903e1d5a6ff6', 'dcim', 'location');
INSERT INTO public.inventory_content_type (id, app_label, model) VALUES ('019ee0e0-75da-733e-9071-730e573aa1a3', 'dcim', 'rack');
INSERT INTO public.inventory_content_type (id, app_label, model) VALUES ('019ee0e0-75da-7378-8664-6cef02f0b518', 'dcim', 'consoleport');
INSERT INTO public.inventory_content_type (id, app_label, model) VALUES ('019ee0e0-75da-7394-8665-8fa620acdc3c', 'dcim', 'consoleserverport');
INSERT INTO public.inventory_content_type (id, app_label, model) VALUES ('019ee0e0-75da-73af-ae09-1ed7088425ed', 'dcim', 'powerport');
INSERT INTO public.inventory_content_type (id, app_label, model) VALUES ('019ee0e0-75da-73c8-85ee-4e120cd352cc', 'dcim', 'poweroutlet');
INSERT INTO public.inventory_content_type (id, app_label, model) VALUES ('019ee0e0-75da-73e0-8315-26a301c9d13f', 'dcim', 'frontport');
INSERT INTO public.inventory_content_type (id, app_label, model) VALUES ('019ee0e0-75da-7401-8dfd-8411ca37995f', 'dcim', 'rearport');
INSERT INTO public.inventory_content_type (id, app_label, model) VALUES ('019ee0e0-75da-7418-bebb-5861ab4a93b8', 'dcim', 'devicebay');
INSERT INTO public.inventory_content_type (id, app_label, model) VALUES ('019ee0e0-75da-7431-9087-5ca4286085cb', 'dcim', 'modulebay');
INSERT INTO public.inventory_content_type (id, app_label, model) VALUES ('019ee0e0-75da-7448-ac20-a6b5ce4805b3', 'dcim', 'inventoryitem');
INSERT INTO public.inventory_content_type (id, app_label, model) VALUES ('019ee0e0-75da-7468-9e1d-1a626d8734e6', 'virtualization', 'virtualmachine');
INSERT INTO public.inventory_content_type (id, app_label, model) VALUES ('019ee0e0-75da-749b-8133-67dc61378bb3', 'virtualization', 'cluster');
INSERT INTO public.inventory_content_type (id, app_label, model) VALUES ('019ee0e0-75da-74b3-ad00-ca5c881d4a5d', 'virtualization', 'clustergroup');
INSERT INTO public.inventory_content_type (id, app_label, model) VALUES ('019ee0e0-75da-74ce-815e-6317e26de32c', 'tenancy', 'tenant');
INSERT INTO public.inventory_content_type (id, app_label, model) VALUES ('019ee0e0-75da-735c-8aef-34d45e14753d', 'dcim', 'interface');
INSERT INTO public.inventory_content_type (id, app_label, model) VALUES ('019ee0e0-75da-7481-a9b8-503167d7f426', 'virtualization', 'vminterface');
INSERT INTO public.inventory_content_type (id, app_label, model) VALUES (uuidv7(), 'ipam', 'ipaddress');
INSERT INTO public.inventory_content_type (id, app_label, model) VALUES (uuidv7(), 'ipam', 'prefix');
INSERT INTO public.inventory_content_type (id, app_label, model) VALUES (uuidv7(), 'ipam', 'service');
INSERT INTO public.inventory_content_type (id, app_label, model) VALUES (uuidv7(), 'virtualization', 'virtualdisk');
INSERT INTO public.inventory_content_type (id, app_label, model) VALUES (uuidv7(), 'users', 'owner');
INSERT INTO public.inventory_content_type (id, app_label, model) VALUES (uuidv7(), 'users', 'ownergroup');
INSERT INTO public.inventory_content_type (id, app_label, model) VALUES (uuidv7(), 'dcim', 'rackgroup');
INSERT INTO public.inventory_content_type (id, app_label, model) VALUES (uuidv7(), 'dcim', 'cablebundle');
INSERT INTO public.inventory_content_type (id, app_label, model) VALUES (uuidv7(), 'dcim', 'modulebaytype');
INSERT INTO public.inventory_content_type (id, app_label, model) VALUES (uuidv7(), 'virtualization', 'virtualmachinetype');
INSERT INTO public.inventory_content_type (id, app_label, model) VALUES (uuidv7(), 'dcim', 'coolingsource');
INSERT INTO public.inventory_content_type (id, app_label, model) VALUES (uuidv7(), 'dcim', 'coolingfeed');
INSERT INTO public.inventory_content_type (id, app_label, model) VALUES (uuidv7(), 'dcim', 'coolingintake');
INSERT INTO public.inventory_content_type (id, app_label, model) VALUES (uuidv7(), 'dcim', 'coolingoutflow');
INSERT INTO public.inventory_content_type (id, app_label, model) VALUES (uuidv7(), 'dcim', 'coolingintaketemplate');
INSERT INTO public.inventory_content_type (id, app_label, model) VALUES (uuidv7(), 'dcim', 'coolingoutflowtemplate');
INSERT INTO public.inventory_content_type (id, app_label, model) VALUES (uuidv7(), 'wireless', 'wirelesslangroup');
INSERT INTO public.inventory_content_type (id, app_label, model) VALUES (uuidv7(), 'wireless', 'wirelesslan');
INSERT INTO public.inventory_content_type (id, app_label, model) VALUES (uuidv7(), 'wireless', 'wirelesslink');
INSERT INTO public.inventory_content_type (id, app_label, model) VALUES (uuidv7(), 'ipam', 'fhrpgroup');
INSERT INTO public.inventory_content_type (id, app_label, model) VALUES (uuidv7(), 'ipam', 'fhrpgroupassignment');


--
-- Data for Name: inventory_dataflow; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: inventory_dataflow_selected_ip; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: inventory_layout_system_profile; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: inventory_layout_user_profile; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: inventory_tag; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: inventory_template; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.inventory_template (id, name, slug, description, status, owner_user_id, is_system, updated_by_user_id, created_at, updated_at) VALUES ('00000000-0000-7000-8000-000000002001', 'Create managed server', 'create-managed-server', 'Create a site, device, management interface, and primary IP address in one run.', 'active', NULL, true, NULL, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);
INSERT INTO public.inventory_template (id, name, slug, description, status, owner_user_id, is_system, updated_by_user_id, created_at, updated_at) VALUES ('00000000-0000-7000-8000-000000002002', 'Create network device', 'create-network-device', 'Create a site, network device, management interface, and primary IP address in one run.', 'active', NULL, true, NULL, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);
INSERT INTO public.inventory_template (id, name, slug, description, status, owner_user_id, is_system, updated_by_user_id, created_at, updated_at) VALUES ('00000000-0000-7000-8000-000000002003', 'Create virtual machine', 'create-virtual-machine', 'Create a site, virtual machine, interface, and primary IP address in one run.', 'active', NULL, true, NULL, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);
INSERT INTO public.inventory_template (id, name, slug, description, status, owner_user_id, is_system, updated_by_user_id, created_at, updated_at) VALUES ('00000000-0000-7000-8000-000000002004', 'Create site with contact', 'create-site-with-contact', 'Create a site, contact, and technical contact assignment in one run.', 'active', NULL, true, NULL, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);
INSERT INTO public.inventory_template (id, name, slug, description, status, owner_user_id, is_system, updated_by_user_id, created_at, updated_at) VALUES ('00000000-0000-7000-8000-000000002005', 'Create rack in site', 'create-rack', 'Create a site and a rack with factory rack defaults in one run.', 'active', NULL, true, NULL, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);


--
-- Data for Name: inventory_template_run; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: inventory_template_run_step_result; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: inventory_template_step; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.inventory_template_step (id, template_id, step_key, title, description, resource_type, operation, depends_on_step_keys, payload_template_json, on_conflict_policy, sort_order, created_at, updated_at, repeat_json) VALUES (uuidv7(), '00000000-0000-7000-8000-000000002001', 'create_site', 'Create site', NULL, 'site', 'create', '[]', '{"name": "{{site_name}}", "status": "active"}', 'fail', 10, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, '{}');
INSERT INTO public.inventory_template_step (id, template_id, step_key, title, description, resource_type, operation, depends_on_step_keys, payload_template_json, on_conflict_policy, sort_order, created_at, updated_at, repeat_json) VALUES (uuidv7(), '00000000-0000-7000-8000-000000002001', 'create_device', 'Create server', NULL, 'device', 'create', '["create_site"]', '{"name": "{{device_name}}", "role": "{{device_role}}", "status": "active", "site_id": "{{steps.create_site.id}}", "device_type": "{{device_type}}"}', 'fail', 20, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, '{}');
INSERT INTO public.inventory_template_step (id, template_id, step_key, title, description, resource_type, operation, depends_on_step_keys, payload_template_json, on_conflict_policy, sort_order, created_at, updated_at, repeat_json) VALUES (uuidv7(), '00000000-0000-7000-8000-000000002001', 'create_interface', 'Create management interface', NULL, 'device_interface', 'create', '["create_device"]', '{"name": "{{interface_name}}", "type": "1000base-t", "device_id": "{{steps.create_device.id}}", "mgmt_only": true}', 'fail', 30, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, '{}');
INSERT INTO public.inventory_template_step (id, template_id, step_key, title, description, resource_type, operation, depends_on_step_keys, payload_template_json, on_conflict_policy, sort_order, created_at, updated_at, repeat_json) VALUES (uuidv7(), '00000000-0000-7000-8000-000000002001', 'assign_primary_ip', 'Assign primary IP', NULL, 'ip_address', 'create', '["create_interface"]', '{"status": "active", "address": "{{primary_ip}}", "set_primary": true, "interface_id": "{{steps.create_interface.id}}"}', 'fail', 40, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, '{}');
INSERT INTO public.inventory_template_step (id, template_id, step_key, title, description, resource_type, operation, depends_on_step_keys, payload_template_json, on_conflict_policy, sort_order, created_at, updated_at, repeat_json) VALUES (uuidv7(), '00000000-0000-7000-8000-000000002002', 'create_site', 'Create site', NULL, 'site', 'create', '[]', '{"name": "{{site_name}}", "status": "active"}', 'fail', 10, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, '{}');
INSERT INTO public.inventory_template_step (id, template_id, step_key, title, description, resource_type, operation, depends_on_step_keys, payload_template_json, on_conflict_policy, sort_order, created_at, updated_at, repeat_json) VALUES (uuidv7(), '00000000-0000-7000-8000-000000002002', 'create_device', 'Create network device', NULL, 'device', 'create', '["create_site"]', '{"name": "{{device_name}}", "role": "{{device_role}}", "status": "active", "site_id": "{{steps.create_site.id}}", "device_type": "{{device_type}}"}', 'fail', 20, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, '{}');
INSERT INTO public.inventory_template_step (id, template_id, step_key, title, description, resource_type, operation, depends_on_step_keys, payload_template_json, on_conflict_policy, sort_order, created_at, updated_at, repeat_json) VALUES (uuidv7(), '00000000-0000-7000-8000-000000002002', 'create_interface', 'Create management interface', NULL, 'device_interface', 'create', '["create_device"]', '{"name": "{{interface_name}}", "type": "1000base-t", "device_id": "{{steps.create_device.id}}", "mgmt_only": true}', 'fail', 30, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, '{}');
INSERT INTO public.inventory_template_step (id, template_id, step_key, title, description, resource_type, operation, depends_on_step_keys, payload_template_json, on_conflict_policy, sort_order, created_at, updated_at, repeat_json) VALUES (uuidv7(), '00000000-0000-7000-8000-000000002002', 'assign_primary_ip', 'Assign primary IP', NULL, 'ip_address', 'create', '["create_interface"]', '{"status": "active", "address": "{{primary_ip}}", "set_primary": true, "interface_id": "{{steps.create_interface.id}}"}', 'fail', 40, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, '{}');
INSERT INTO public.inventory_template_step (id, template_id, step_key, title, description, resource_type, operation, depends_on_step_keys, payload_template_json, on_conflict_policy, sort_order, created_at, updated_at, repeat_json) VALUES (uuidv7(), '00000000-0000-7000-8000-000000002003', 'create_site', 'Create site', NULL, 'site', 'create', '[]', '{"name": "{{site_name}}", "status": "active"}', 'fail', 10, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, '{}');
INSERT INTO public.inventory_template_step (id, template_id, step_key, title, description, resource_type, operation, depends_on_step_keys, payload_template_json, on_conflict_policy, sort_order, created_at, updated_at, repeat_json) VALUES (uuidv7(), '00000000-0000-7000-8000-000000002003', 'create_vm', 'Create virtual machine', NULL, 'virtual_machine', 'create', '["create_site"]', '{"name": "{{vm_name}}", "status": "active", "role_id": "00000000-0000-7000-8000-000000001014", "site_id": "{{steps.create_site.id}}"}', 'fail', 20, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, '{}');
INSERT INTO public.inventory_template_step (id, template_id, step_key, title, description, resource_type, operation, depends_on_step_keys, payload_template_json, on_conflict_policy, sort_order, created_at, updated_at, repeat_json) VALUES (uuidv7(), '00000000-0000-7000-8000-000000002003', 'create_interface', 'Create VM interface', NULL, 'vm_interface', 'create', '["create_vm"]', '{"name": "{{interface_name}}", "virtual_machine_id": "{{steps.create_vm.id}}"}', 'fail', 30, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, '{}');
INSERT INTO public.inventory_template_step (id, template_id, step_key, title, description, resource_type, operation, depends_on_step_keys, payload_template_json, on_conflict_policy, sort_order, created_at, updated_at, repeat_json) VALUES (uuidv7(), '00000000-0000-7000-8000-000000002003', 'assign_primary_ip', 'Assign primary IP', NULL, 'ip_address', 'create', '["create_interface"]', '{"status": "active", "address": "{{primary_ip}}", "set_primary": true, "vm_interface_id": "{{steps.create_interface.id}}"}', 'fail', 40, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, '{}');
INSERT INTO public.inventory_template_step (id, template_id, step_key, title, description, resource_type, operation, depends_on_step_keys, payload_template_json, on_conflict_policy, sort_order, created_at, updated_at, repeat_json) VALUES (uuidv7(), '00000000-0000-7000-8000-000000002004', 'create_site', 'Create site', NULL, 'site', 'create', '[]', '{"name": "{{site_name}}", "status": "active"}', 'fail', 10, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, '{}');
INSERT INTO public.inventory_template_step (id, template_id, step_key, title, description, resource_type, operation, depends_on_step_keys, payload_template_json, on_conflict_policy, sort_order, created_at, updated_at, repeat_json) VALUES (uuidv7(), '00000000-0000-7000-8000-000000002004', 'create_contact', 'Create contact', NULL, 'contact', 'create', '[]', '{"name": "{{contact_name}}", "email": "{{contact_email}}"}', 'fail', 20, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, '{}');
INSERT INTO public.inventory_template_step (id, template_id, step_key, title, description, resource_type, operation, depends_on_step_keys, payload_template_json, on_conflict_policy, sort_order, created_at, updated_at, repeat_json) VALUES (uuidv7(), '00000000-0000-7000-8000-000000002004', 'assign_contact', 'Assign contact to site', NULL, 'contact_assignment', 'create', '["create_site", "create_contact"]', '{"role_id": "00000000-0000-7000-8000-000000001031", "priority": "primary", "object_id": "{{steps.create_site.id}}", "contact_id": "{{steps.create_contact.id}}", "object_type": "dcim.site"}', 'fail', 30, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, '{}');
INSERT INTO public.inventory_template_step (id, template_id, step_key, title, description, resource_type, operation, depends_on_step_keys, payload_template_json, on_conflict_policy, sort_order, created_at, updated_at, repeat_json) VALUES (uuidv7(), '00000000-0000-7000-8000-000000002005', 'create_site', 'Create site', NULL, 'site', 'create', '[]', '{"name": "{{site_name}}", "status": "active"}', 'fail', 10, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, '{}');
INSERT INTO public.inventory_template_step (id, template_id, step_key, title, description, resource_type, operation, depends_on_step_keys, payload_template_json, on_conflict_policy, sort_order, created_at, updated_at, repeat_json) VALUES (uuidv7(), '00000000-0000-7000-8000-000000002005', 'create_rack', 'Create rack', NULL, 'rack', 'create', '["create_site"]', '{"name": "{{rack_name}}", "width": 19, "status": "active", "role_id": "00000000-0000-7000-8000-000000001021", "site_id": "{{steps.create_site.id}}", "u_height": "{{u_height}}", "desc_units": false}', 'fail', 20, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, '{}');


--
-- Data for Name: inventory_template_variable; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.inventory_template_variable (id, template_id, key, label, description, value_type, required, default_value_json, validation_rules_json, sort_order, created_at, updated_at) VALUES (uuidv7(), '00000000-0000-7000-8000-000000002001', 'site_name', 'Site name', 'Site to create for the server.', 'string', true, '"Default site"', '{}', 10, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);
INSERT INTO public.inventory_template_variable (id, template_id, key, label, description, value_type, required, default_value_json, validation_rules_json, sort_order, created_at, updated_at) VALUES (uuidv7(), '00000000-0000-7000-8000-000000002001', 'device_name', 'Device name', 'Server inventory name.', 'string', true, '"server-01"', '{}', 20, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);
INSERT INTO public.inventory_template_variable (id, template_id, key, label, description, value_type, required, default_value_json, validation_rules_json, sort_order, created_at, updated_at) VALUES (uuidv7(), '00000000-0000-7000-8000-000000002001', 'device_role', 'Device role', 'Existing device role slug or name.', 'string', true, '"server"', '{}', 30, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);
INSERT INTO public.inventory_template_variable (id, template_id, key, label, description, value_type, required, default_value_json, validation_rules_json, sort_order, created_at, updated_at) VALUES (uuidv7(), '00000000-0000-7000-8000-000000002001', 'device_type', 'Device type', 'Existing device type slug or model.', 'string', true, '"generic-server"', '{}', 40, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);
INSERT INTO public.inventory_template_variable (id, template_id, key, label, description, value_type, required, default_value_json, validation_rules_json, sort_order, created_at, updated_at) VALUES (uuidv7(), '00000000-0000-7000-8000-000000002001', 'interface_name', 'Interface name', 'Management interface name.', 'string', true, '"eth0"', '{}', 50, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);
INSERT INTO public.inventory_template_variable (id, template_id, key, label, description, value_type, required, default_value_json, validation_rules_json, sort_order, created_at, updated_at) VALUES (uuidv7(), '00000000-0000-7000-8000-000000002001', 'primary_ip', 'Primary IP', 'Primary address in CIDR notation.', 'string', true, '"192.0.2.10/32"', '{}', 60, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);
INSERT INTO public.inventory_template_variable (id, template_id, key, label, description, value_type, required, default_value_json, validation_rules_json, sort_order, created_at, updated_at) VALUES (uuidv7(), '00000000-0000-7000-8000-000000002002', 'site_name', 'Site name', 'Site to create for the network device.', 'string', true, '"Default site"', '{}', 10, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);
INSERT INTO public.inventory_template_variable (id, template_id, key, label, description, value_type, required, default_value_json, validation_rules_json, sort_order, created_at, updated_at) VALUES (uuidv7(), '00000000-0000-7000-8000-000000002002', 'device_name', 'Device name', 'Network device inventory name.', 'string', true, '"switch-01"', '{}', 20, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);
INSERT INTO public.inventory_template_variable (id, template_id, key, label, description, value_type, required, default_value_json, validation_rules_json, sort_order, created_at, updated_at) VALUES (uuidv7(), '00000000-0000-7000-8000-000000002002', 'device_role', 'Device role', 'Existing device role slug or name.', 'string', true, '"network-device"', '{}', 30, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);
INSERT INTO public.inventory_template_variable (id, template_id, key, label, description, value_type, required, default_value_json, validation_rules_json, sort_order, created_at, updated_at) VALUES (uuidv7(), '00000000-0000-7000-8000-000000002002', 'device_type', 'Device type', 'Existing device type slug or model.', 'string', true, '"generic-network-appliance"', '{}', 40, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);
INSERT INTO public.inventory_template_variable (id, template_id, key, label, description, value_type, required, default_value_json, validation_rules_json, sort_order, created_at, updated_at) VALUES (uuidv7(), '00000000-0000-7000-8000-000000002002', 'interface_name', 'Interface name', 'Management interface name.', 'string', true, '"mgmt0"', '{}', 50, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);
INSERT INTO public.inventory_template_variable (id, template_id, key, label, description, value_type, required, default_value_json, validation_rules_json, sort_order, created_at, updated_at) VALUES (uuidv7(), '00000000-0000-7000-8000-000000002002', 'primary_ip', 'Primary IP', 'Primary address in CIDR notation.', 'string', true, '"192.0.2.1/32"', '{}', 60, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);
INSERT INTO public.inventory_template_variable (id, template_id, key, label, description, value_type, required, default_value_json, validation_rules_json, sort_order, created_at, updated_at) VALUES (uuidv7(), '00000000-0000-7000-8000-000000002003', 'site_name', 'Site name', 'Site to create for the virtual machine.', 'string', true, '"Default site"', '{}', 10, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);
INSERT INTO public.inventory_template_variable (id, template_id, key, label, description, value_type, required, default_value_json, validation_rules_json, sort_order, created_at, updated_at) VALUES (uuidv7(), '00000000-0000-7000-8000-000000002003', 'vm_name', 'VM name', 'Virtual machine inventory name.', 'string', true, '"vm-01"', '{}', 20, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);
INSERT INTO public.inventory_template_variable (id, template_id, key, label, description, value_type, required, default_value_json, validation_rules_json, sort_order, created_at, updated_at) VALUES (uuidv7(), '00000000-0000-7000-8000-000000002003', 'interface_name', 'Interface name', 'Virtual NIC name.', 'string', true, '"eth0"', '{}', 30, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);
INSERT INTO public.inventory_template_variable (id, template_id, key, label, description, value_type, required, default_value_json, validation_rules_json, sort_order, created_at, updated_at) VALUES (uuidv7(), '00000000-0000-7000-8000-000000002003', 'primary_ip', 'Primary IP', 'Primary address in CIDR notation.', 'string', true, '"192.0.2.20/32"', '{}', 40, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);
INSERT INTO public.inventory_template_variable (id, template_id, key, label, description, value_type, required, default_value_json, validation_rules_json, sort_order, created_at, updated_at) VALUES (uuidv7(), '00000000-0000-7000-8000-000000002004', 'site_name', 'Site name', 'Site to create.', 'string', true, '"Default site"', '{}', 10, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);
INSERT INTO public.inventory_template_variable (id, template_id, key, label, description, value_type, required, default_value_json, validation_rules_json, sort_order, created_at, updated_at) VALUES (uuidv7(), '00000000-0000-7000-8000-000000002004', 'contact_name', 'Contact name', 'Contact display name.', 'string', true, '"Primary contact"', '{}', 20, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);
INSERT INTO public.inventory_template_variable (id, template_id, key, label, description, value_type, required, default_value_json, validation_rules_json, sort_order, created_at, updated_at) VALUES (uuidv7(), '00000000-0000-7000-8000-000000002004', 'contact_email', 'Contact email', 'Contact email address.', 'string', true, '"ops@example.invalid"', '{}', 30, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);
INSERT INTO public.inventory_template_variable (id, template_id, key, label, description, value_type, required, default_value_json, validation_rules_json, sort_order, created_at, updated_at) VALUES (uuidv7(), '00000000-0000-7000-8000-000000002005', 'site_name', 'Site name', 'Site to create for the rack.', 'string', true, '"Default site"', '{}', 10, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);
INSERT INTO public.inventory_template_variable (id, template_id, key, label, description, value_type, required, default_value_json, validation_rules_json, sort_order, created_at, updated_at) VALUES (uuidv7(), '00000000-0000-7000-8000-000000002005', 'rack_name', 'Rack name', 'Rack inventory name.', 'string', true, '"rack-01"', '{}', 20, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);
INSERT INTO public.inventory_template_variable (id, template_id, key, label, description, value_type, required, default_value_json, validation_rules_json, sort_order, created_at, updated_at) VALUES (uuidv7(), '00000000-0000-7000-8000-000000002005', 'u_height', 'Rack units', 'Rack height in rack units.', 'integer', true, '42', '{}', 30, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);


--
-- Data for Name: ipam_aggregate; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: ipam_asn; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: ipam_asnrange; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: ipam_fhrpgroup; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: ipam_fhrpgroupassignment; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: ipam_ipaddress; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: ipam_iprange; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: ipam_prefix; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: ipam_rir; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: ipam_role; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.ipam_role (custom_field_data, id, name, slug, weight, description, owner_id, comments, tags, external_id, piping_synced_at) VALUES ('{}', '00000000-0000-7000-8000-000000001041', 'Management', 'management', 100, 'Management addressing.', NULL, '', '{factory-default}', NULL, NULL);
INSERT INTO public.ipam_role (custom_field_data, id, name, slug, weight, description, owner_id, comments, tags, external_id, piping_synced_at) VALUES ('{}', '00000000-0000-7000-8000-000000001042', 'Out-of-band', 'out-of-band', 110, 'Out-of-band management addressing.', NULL, '', '{factory-default}', NULL, NULL);
INSERT INTO public.ipam_role (custom_field_data, id, name, slug, weight, description, owner_id, comments, tags, external_id, piping_synced_at) VALUES ('{}', '00000000-0000-7000-8000-000000001043', 'Loopback', 'loopback', 120, 'Loopback addressing.', NULL, '', '{factory-default}', NULL, NULL);
INSERT INTO public.ipam_role (custom_field_data, id, name, slug, weight, description, owner_id, comments, tags, external_id, piping_synced_at) VALUES ('{}', '00000000-0000-7000-8000-000000001044', 'Transit', 'transit', 130, 'Point-to-point or transit addressing.', NULL, '', '{factory-default}', NULL, NULL);
INSERT INTO public.ipam_role (custom_field_data, id, name, slug, weight, description, owner_id, comments, tags, external_id, piping_synced_at) VALUES ('{}', '00000000-0000-7000-8000-000000001045', 'Service', 'service', 140, 'Service-facing addressing.', NULL, '', '{factory-default}', NULL, NULL);


--
-- Data for Name: ipam_routetarget; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: ipam_service; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: ipam_service_ipaddresses; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: ipam_service_port_mapping; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: ipam_servicetemplate; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: ipam_vlan; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: ipam_vlangroup; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: ipam_vlantranslationpolicy; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: ipam_vlantranslationrule; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: ipam_vrf; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: ipam_vrf_export_targets; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: ipam_vrf_import_targets; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: license_instance_identity; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.license_instance_identity (id, instance_fingerprint, created_at, updated_at) VALUES (true, 'piping-' || uuidv7()::text, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);


--
-- Data for Name: license_seat_assignment; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: license_seat_ledger; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: license_state; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: mapping_configurations; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: monitoring_alert_source; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: monitoring_alert_source_assignment; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: monitoring_exporter; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.monitoring_exporter (id, name, slug, exporter_type, description, default_job, default_scheme, default_port, default_metrics_path, default_target_source, labels, enabled, created_at, updated_at) VALUES ('019ee0e0-69f8-7d17-93d1-d5299497329a', 'Node exporter', 'node', 'node', NULL, 'node', 'http', 9100, '/metrics', 'host_address', '{}', true, '2026-06-19 17:14:24.120682+00', '2026-06-19 17:14:24.120682+00');
INSERT INTO public.monitoring_exporter (id, name, slug, exporter_type, description, default_job, default_scheme, default_port, default_metrics_path, default_target_source, labels, enabled, created_at, updated_at) VALUES ('019ee0e0-69f8-7e6b-8384-21c2c209aa81', 'cAdvisor / Docker', 'docker', 'docker', NULL, 'docker', 'http', 8081, '/metrics', 'host_address', '{}', true, '2026-06-19 17:14:24.120682+00', '2026-06-19 17:14:24.120682+00');
INSERT INTO public.monitoring_exporter (id, name, slug, exporter_type, description, default_job, default_scheme, default_port, default_metrics_path, default_target_source, labels, enabled, created_at, updated_at) VALUES ('019ee0e0-69f8-7e95-bdf6-1ad81f201faa', 'NGINX exporter', 'nginx', 'nginx', NULL, 'nginx', 'http', 9113, '/metrics', 'host_address', '{}', true, '2026-06-19 17:14:24.120682+00', '2026-06-19 17:14:24.120682+00');
INSERT INTO public.monitoring_exporter (id, name, slug, exporter_type, description, default_job, default_scheme, default_port, default_metrics_path, default_target_source, labels, enabled, created_at, updated_at) VALUES ('019ee0e0-69f8-7eae-9923-019965141280', 'Postgres exporter', 'postgres', 'postgres', NULL, 'postgres', 'http', 9187, '/metrics', 'host_address', '{}', true, '2026-06-19 17:14:24.120682+00', '2026-06-19 17:14:24.120682+00');
INSERT INTO public.monitoring_exporter (id, name, slug, exporter_type, description, default_job, default_scheme, default_port, default_metrics_path, default_target_source, labels, enabled, created_at, updated_at) VALUES ('019ee0e0-69f8-7ec7-9b6d-03902c6b2d63', 'Redis exporter', 'redis', 'redis', NULL, 'redis', 'http', 9121, '/metrics', 'host_address', '{}', true, '2026-06-19 17:14:24.120682+00', '2026-06-19 17:14:24.120682+00');
INSERT INTO public.monitoring_exporter (id, name, slug, exporter_type, description, default_job, default_scheme, default_port, default_metrics_path, default_target_source, labels, enabled, created_at, updated_at) VALUES ('019ee0e0-69f8-7ede-afdb-4955362b6190', 'Proxmox exporter', 'proxmox', 'proxmox', NULL, 'proxmox', 'http', 9221, '/pve', 'host_address', '{}', true, '2026-06-19 17:14:24.120682+00', '2026-06-19 17:14:24.120682+00');
INSERT INTO public.monitoring_exporter (id, name, slug, exporter_type, description, default_job, default_scheme, default_port, default_metrics_path, default_target_source, labels, enabled, created_at, updated_at) VALUES ('019ee0e0-69f8-7ef8-b4d0-25e2197c6d65', 'smartctl exporter', 'smartctl', 'smartctl', NULL, 'smartctl', 'http', 9633, '/metrics', 'host_address', '{}', true, '2026-06-19 17:14:24.120682+00', '2026-06-19 17:14:24.120682+00');
INSERT INTO public.monitoring_exporter (id, name, slug, exporter_type, description, default_job, default_scheme, default_port, default_metrics_path, default_target_source, labels, enabled, created_at, updated_at) VALUES ('019ee0e0-69f8-7f11-ac7b-73040758bba5', 'Process exporter', 'process', 'process', NULL, 'process', 'http', 9256, '/metrics', 'host_address', '{}', true, '2026-06-19 17:14:24.120682+00', '2026-06-19 17:14:24.120682+00');
INSERT INTO public.monitoring_exporter (id, name, slug, exporter_type, description, default_job, default_scheme, default_port, default_metrics_path, default_target_source, labels, enabled, created_at, updated_at) VALUES ('019ee0e0-69f8-7f2a-a7a2-d737c5cf0346', 'GitLab exporter', 'gitlab', 'gitlab', NULL, 'gitlab', 'http', 9168, '/metrics', 'host_address', '{}', true, '2026-06-19 17:14:24.120682+00', '2026-06-19 17:14:24.120682+00');
INSERT INTO public.monitoring_exporter (id, name, slug, exporter_type, description, default_job, default_scheme, default_port, default_metrics_path, default_target_source, labels, enabled, created_at, updated_at) VALUES ('019ee0e0-6bd4-7a9c-8832-2265afd237ef', 'Blackbox exporter', 'blackbox', 'blackbox', 'External probe (HTTP/TCP/ICMP) for network devices not running a local agent.', 'blackbox', 'http', 9115, '/probe', 'host_address', '{}', true, '2026-06-19 17:14:24.596065+00', '2026-06-19 17:14:24.596065+00');
INSERT INTO public.monitoring_exporter (id, name, slug, exporter_type, description, default_job, default_scheme, default_port, default_metrics_path, default_target_source, labels, enabled, created_at, updated_at) VALUES ('019ee0e0-6e80-7d63-b30e-4b69ad4f8dab', 'NGINX vhost exporter', 'nginx-vhost', 'nginx', NULL, 'nginx-vhost', 'http', 9113, '/metrics', 'host_address', '{}', true, '2026-06-19 17:14:25.27979+00', '2026-06-19 17:14:25.27979+00');
INSERT INTO public.monitoring_exporter (id, name, slug, exporter_type, description, default_job, default_scheme, default_port, default_metrics_path, default_target_source, labels, enabled, created_at, updated_at) VALUES ('019ee0e0-7a38-7647-bdc0-ade4dfd056f8', 'Blackbox exporter (interface addresses)', 'blackbox-interface', 'blackbox', 'External blackbox probes for each assigned interface address.', 'blackbox', 'http', 9115, '/probe', 'interface_addresses', '{}', true, '2026-06-19 17:14:28.280026+00', '2026-06-19 17:14:28.280026+00');


--
-- Data for Name: monitoring_exporter_assignment; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.monitoring_exporter_assignment (id, exporter_id, name, scope_type, scope_id, weight, enabled, target, job, scheme, port, metrics_path, labels, created_at, updated_at) VALUES ('019ee0e0-69fb-795f-9bc3-4f26240efac1', '019ee0e0-69f8-7d17-93d1-d5299497329a', 'Default node exporter', 'global', NULL, 0, true, NULL, NULL, NULL, NULL, NULL, '{}', '2026-06-19 17:14:24.123457+00', '2026-06-19 17:14:24.123457+00');


--
-- Data for Name: monitoring_metric_query; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.monitoring_metric_query (id, exporter_id, name, slug, label, query, unit, description, default_show_in_inspector, default_show_in_list, sort_order, enabled, created_at, updated_at, source_type, query_type, value_type, widget_type, options, category, semantic) VALUES ('019ee0e0-69fa-73bd-966a-737cbb254279', '019ee0e0-69f8-7d17-93d1-d5299497329a', 'load1', 'load1', 'Load 1m', 'node_load1{piping_target_id="$host_id",job="node"}', 'load', 'Load 1m', true, false, 20, true, '2026-06-19 17:14:24.121611+00', '2026-06-19 17:14:25.353769+00', 'prometheus', 'promql', 'number', 'stat', '{}', 'System', NULL);
INSERT INTO public.monitoring_metric_query (id, exporter_id, name, slug, label, query, unit, description, default_show_in_inspector, default_show_in_list, sort_order, enabled, created_at, updated_at, source_type, query_type, value_type, widget_type, options, category, semantic) VALUES ('019ee0e0-69fa-7394-8801-a669209ca971', '019ee0e0-69f8-7d17-93d1-d5299497329a', 'cpu_busy', 'cpu_busy', 'CPU busy', '100 - (avg by (piping_target_id) (rate(node_cpu_seconds_total{piping_target_id="$host_id",job="node",mode="idle"}[5m])) * 100)', '%', 'CPU busy', true, true, 30, true, '2026-06-19 17:14:24.121611+00', '2026-06-19 17:14:25.452606+00', 'prometheus', 'promql', 'percent', 'gauge', '{"display": {"status": {"mode": "numeric", "warning": 70, "critical": 90, "direction": "higher_is_worse"}}}', 'System', NULL);
INSERT INTO public.monitoring_metric_query (id, exporter_id, name, slug, label, query, unit, description, default_show_in_inspector, default_show_in_list, sort_order, enabled, created_at, updated_at, source_type, query_type, value_type, widget_type, options, category, semantic) VALUES ('019ee0e0-6faa-7237-ac66-2491b4d53be5', '019ee0e0-69f8-7d17-93d1-d5299497329a', 'node_uptime', 'node_uptime', 'Uptime', 'node_time_seconds{piping_host_name="$host",job="node"} - node_boot_time_seconds{piping_host_name="$host",job="node"}', 's', 'Time since the host booted', true, false, 15, true, '2026-06-19 17:14:25.576757+00', '2026-06-19 17:14:25.576757+00', 'prometheus', 'promql', 'duration', 'stat', '{}', 'System', NULL);
INSERT INTO public.monitoring_metric_query (id, exporter_id, name, slug, label, query, unit, description, default_show_in_inspector, default_show_in_list, sort_order, enabled, created_at, updated_at, source_type, query_type, value_type, widget_type, options, category, semantic) VALUES ('019ee0e0-6faa-741f-a143-5bfeaac16fda', '019ee0e0-69f8-7d17-93d1-d5299497329a', 'node_root_readonly', 'node_root_readonly', 'Root FS writable', 'node_filesystem_readonly{piping_host_name="$host",job="node",mountpoint="/",fstype!~"tmpfs|overlay|squashfs"}', 'state', 'Whether the root filesystem is writable', true, false, 55, true, '2026-06-19 17:14:25.576757+00', '2026-06-19 17:14:25.576757+00', 'prometheus', 'promql', 'state', 'badge', '{"display": {"status": {"mode": "state", "normal": ["down", "0", "false"], "critical": ["up", "1", "true"]}}}', 'System', NULL);
INSERT INTO public.monitoring_metric_query (id, exporter_id, name, slug, label, query, unit, description, default_show_in_inspector, default_show_in_list, sort_order, enabled, created_at, updated_at, source_type, query_type, value_type, widget_type, options, category, semantic) VALUES ('019ee0e0-6a82-755e-9cac-700055715cc4', NULL, 'zabbix_cpu_load1', 'zabbix-cpu-load1', 'CPU load 1m', 'system.cpu.load[percpu,avg1]', 'load', 'CPU load 1m', true, false, 30, true, '2026-06-19 17:14:24.258172+00', '2026-06-19 17:14:25.353769+00', 'zabbix', 'zabbix_item_key', 'number', 'stat', '{}', 'System', NULL);
INSERT INTO public.monitoring_metric_query (id, exporter_id, name, slug, label, query, unit, description, default_show_in_inspector, default_show_in_list, sort_order, enabled, created_at, updated_at, source_type, query_type, value_type, widget_type, options, category, semantic) VALUES ('019ee0e0-6faa-7453-941c-d76f7124bacf', '019ee0e0-69f8-7d17-93d1-d5299497329a', 'node_network_rx', 'node_network_rx', 'Network RX', 'sum by (piping_host_name) (rate(node_network_receive_bytes_total{piping_host_name="$host",job="node",device!~"lo|veth.*|docker.*|br-.*"}[5m]))', 'B/s', 'Receive throughput excluding loopback and container bridge devices', true, false, 60, true, '2026-06-19 17:14:25.576757+00', '2026-06-19 17:14:25.576757+00', 'prometheus', 'promql', 'number', 'stat', '{}', 'System', NULL);
INSERT INTO public.monitoring_metric_query (id, exporter_id, name, slug, label, query, unit, description, default_show_in_inspector, default_show_in_list, sort_order, enabled, created_at, updated_at, source_type, query_type, value_type, widget_type, options, category, semantic) VALUES ('019ee0e0-6faa-7483-b416-84338566195a', '019ee0e0-69f8-7d17-93d1-d5299497329a', 'node_network_tx', 'node_network_tx', 'Network TX', 'sum by (piping_host_name) (rate(node_network_transmit_bytes_total{piping_host_name="$host",job="node",device!~"lo|veth.*|docker.*|br-.*"}[5m]))', 'B/s', 'Transmit throughput excluding loopback and container bridge devices', true, false, 70, true, '2026-06-19 17:14:25.576757+00', '2026-06-19 17:14:25.576757+00', 'prometheus', 'promql', 'number', 'stat', '{}', 'System', NULL);
INSERT INTO public.monitoring_metric_query (id, exporter_id, name, slug, label, query, unit, description, default_show_in_inspector, default_show_in_list, sort_order, enabled, created_at, updated_at, source_type, query_type, value_type, widget_type, options, category, semantic) VALUES ('019ee0e0-6a7f-7996-9e3f-bde12af6ff37', '019ee0e0-69f8-7d17-93d1-d5299497329a', 'disk_used_root', 'disk_used_root', 'Disk used /', '(1 - (node_filesystem_avail_bytes{piping_target_id="$host_id",job="node",mountpoint="/",fstype!~"tmpfs|overlay|squashfs"} / node_filesystem_size_bytes{piping_target_id="$host_id",job="node",mountpoint="/",fstype!~"tmpfs|overlay|squashfs"})) * 100', '%', 'Disk used /', true, true, 50, true, '2026-06-19 17:14:24.255323+00', '2026-06-19 17:14:25.452606+00', 'prometheus', 'promql', 'percent', 'gauge', '{"display": {"status": {"mode": "numeric", "warning": 80, "critical": 90, "direction": "higher_is_worse"}}}', 'System', NULL);
INSERT INTO public.monitoring_metric_query (id, exporter_id, name, slug, label, query, unit, description, default_show_in_inspector, default_show_in_list, sort_order, enabled, created_at, updated_at, source_type, query_type, value_type, widget_type, options, category, semantic) VALUES ('019ee0e0-69fa-7288-acbe-934f2cccee73', '019ee0e0-69f8-7d17-93d1-d5299497329a', 'memory_used', 'memory_used', 'Memory used', '(1 - (node_memory_MemAvailable_bytes{piping_target_id="$host_id",job="node"} / node_memory_MemTotal_bytes{piping_target_id="$host_id",job="node"})) * 100', '%', 'Memory used', true, true, 40, true, '2026-06-19 17:14:24.121611+00', '2026-06-19 17:14:25.452606+00', 'prometheus', 'promql', 'percent', 'gauge', '{"display": {"status": {"mode": "numeric", "warning": 80, "critical": 92, "direction": "higher_is_worse"}}}', 'System', NULL);
INSERT INTO public.monitoring_metric_query (id, exporter_id, name, slug, label, query, unit, description, default_show_in_inspector, default_show_in_list, sort_order, enabled, created_at, updated_at, source_type, query_type, value_type, widget_type, options, category, semantic) VALUES ('019ee0e0-69fa-74ff-8fb4-fac1641c6d1c', '019ee0e0-69f8-7f11-ac7b-73040758bba5', 'named_processes', 'named_processes', 'Processes', 'sum(namedprocess_namegroup_num_procs{piping_host_name="$host",job="process"})', 'count', 'Processes', true, false, 10, true, '2026-06-19 17:14:24.121611+00', '2026-06-19 17:14:25.353769+00', 'prometheus', 'promql', 'number', 'stat', '{}', 'System', NULL);
INSERT INTO public.monitoring_metric_query (id, exporter_id, name, slug, label, query, unit, description, default_show_in_inspector, default_show_in_list, sort_order, enabled, created_at, updated_at, source_type, query_type, value_type, widget_type, options, category, semantic) VALUES ('019ee0e0-69fa-7401-b622-220ae754177c', '019ee0e0-69f8-7e6b-8384-21c2c209aa81', 'container_cpu', 'container_cpu', 'Container CPU', 'sum(rate(container_cpu_usage_seconds_total{piping_host_name="$host",job="docker",container!=""}[5m])) * 100', '%', 'Container CPU', true, true, 20, true, '2026-06-19 17:14:24.121611+00', '2026-06-19 17:14:25.452606+00', 'prometheus', 'promql', 'percent', 'gauge', '{"display": {"status": {"mode": "numeric", "warning": 70, "critical": 90, "direction": "higher_is_worse"}}}', 'Containers', NULL);
INSERT INTO public.monitoring_metric_query (id, exporter_id, name, slug, label, query, unit, description, default_show_in_inspector, default_show_in_list, sort_order, enabled, created_at, updated_at, source_type, query_type, value_type, widget_type, options, category, semantic) VALUES ('019ee0e0-69fa-7419-af76-bfc3d44db100', '019ee0e0-69f8-7e6b-8384-21c2c209aa81', 'containers', 'containers', 'Containers', 'count(container_last_seen{piping_host_name="$host",job="docker"})', 'count', 'Containers', true, true, 10, true, '2026-06-19 17:14:24.121611+00', '2026-06-19 17:14:25.353769+00', 'prometheus', 'promql', 'number', 'stat', '{}', 'Containers', NULL);
INSERT INTO public.monitoring_metric_query (id, exporter_id, name, slug, label, query, unit, description, default_show_in_inspector, default_show_in_list, sort_order, enabled, created_at, updated_at, source_type, query_type, value_type, widget_type, options, category, semantic) VALUES ('019ee0e0-6c42-7383-9e09-e3918290b10f', '019ee0e0-6bd4-7a9c-8832-2265afd237ef', 'probe_duration', 'probe_duration', 'Probe time', 'min by (piping_target_id) (last_over_time(ping_rtt_worst_seconds{piping_target_id="$host_id"}[5m]))', 's', 'Probe time', true, false, 20, true, '2026-06-19 17:14:24.704043+00', '2026-06-19 17:14:25.353769+00', 'prometheus', 'promql', 'auto', 'stat', '{}', 'Network', NULL);
INSERT INTO public.monitoring_metric_query (id, exporter_id, name, slug, label, query, unit, description, default_show_in_inspector, default_show_in_list, sort_order, enabled, created_at, updated_at, source_type, query_type, value_type, widget_type, options, category, semantic) VALUES ('019ee0e0-6c42-70e7-954c-a6841cb0cb46', '019ee0e0-6bd4-7a9c-8832-2265afd237ef', 'icmp_rtt', 'icmp_rtt', 'ICMP RTT', 'min by (piping_target_id) (last_over_time(ping_rtt_mean_seconds{piping_target_id="$host_id"}[5m]))', 's', 'ICMP RTT', true, true, 30, true, '2026-06-19 17:14:24.704043+00', '2026-06-19 17:14:25.353769+00', 'prometheus', 'promql', 'auto', 'stat', '{}', 'Network', NULL);
INSERT INTO public.monitoring_metric_query (id, exporter_id, name, slug, label, query, unit, description, default_show_in_inspector, default_show_in_list, sort_order, enabled, created_at, updated_at, source_type, query_type, value_type, widget_type, options, category, semantic) VALUES ('019ee0e0-69fa-7449-9c30-111c68209ea1', '019ee0e0-69f8-7e95-bdf6-1ad81f201faa', 'nginx_connections', 'nginx_connections', 'Active connections', 'nginx_connections_active{piping_host_name="$host",job="nginx"}', 'count', 'Active connections', true, false, 10, true, '2026-06-19 17:14:24.121611+00', '2026-06-19 17:14:25.353769+00', 'prometheus', 'promql', 'number', 'stat', '{}', 'Web', NULL);
INSERT INTO public.monitoring_metric_query (id, exporter_id, name, slug, label, query, unit, description, default_show_in_inspector, default_show_in_list, sort_order, enabled, created_at, updated_at, source_type, query_type, value_type, widget_type, options, category, semantic) VALUES ('019ee0e0-69fa-7432-a25a-3a0aa4a47122', '019ee0e0-69f8-7e95-bdf6-1ad81f201faa', 'nginx_requests', 'nginx_requests', 'Requests/s', 'rate(nginx_http_requests_total{piping_host_name="$host",job="nginx"}[5m])', 'rps', 'Requests/s', true, false, 20, true, '2026-06-19 17:14:24.121611+00', '2026-06-19 17:14:25.353769+00', 'prometheus', 'promql', 'number', 'stat', '{}', 'Web', NULL);
INSERT INTO public.monitoring_metric_query (id, exporter_id, name, slug, label, query, unit, description, default_show_in_inspector, default_show_in_list, sort_order, enabled, created_at, updated_at, source_type, query_type, value_type, widget_type, options, category, semantic) VALUES ('019ee0e0-69fa-747d-8c55-85027c310746', '019ee0e0-69f8-7eae-9923-019965141280', 'pg_up', 'pg_up', 'Postgres up', 'pg_up{piping_host_name="$host",job="postgres"}', 'state', 'Postgres up', true, false, 10, true, '2026-06-19 17:14:24.121611+00', '2026-06-19 17:14:25.452606+00', 'prometheus', 'promql', 'state', 'badge', '{"display": {"status": {"mode": "state", "normal": ["up", "1", "true", "ok", "active"], "warning": ["unknown", "pending"], "critical": ["down", "0", "false", "failed", "error"]}}}', 'Database', NULL);
INSERT INTO public.monitoring_metric_query (id, exporter_id, name, slug, label, query, unit, description, default_show_in_inspector, default_show_in_list, sort_order, enabled, created_at, updated_at, source_type, query_type, value_type, widget_type, options, category, semantic) VALUES ('019ee0e0-69fa-7463-86ce-87ee09e99c97', '019ee0e0-69f8-7eae-9923-019965141280', 'pg_connections', 'pg_connections', 'DB connections', 'sum(pg_stat_activity_count{piping_host_name="$host",job="postgres"})', 'count', 'DB connections', true, true, 20, true, '2026-06-19 17:14:24.121611+00', '2026-06-19 17:14:25.353769+00', 'prometheus', 'promql', 'number', 'stat', '{}', 'Database', NULL);
INSERT INTO public.monitoring_metric_query (id, exporter_id, name, slug, label, query, unit, description, default_show_in_inspector, default_show_in_list, sort_order, enabled, created_at, updated_at, source_type, query_type, value_type, widget_type, options, category, semantic) VALUES ('019ee0e0-69fa-7497-8ed1-7739c52877e5', '019ee0e0-69f8-7ec7-9b6d-03902c6b2d63', 'redis_memory', 'redis_memory', 'Redis memory', 'redis_memory_used_bytes{piping_host_name="$host",job="redis"}', 'bytes', 'Redis memory', true, false, 20, true, '2026-06-19 17:14:24.121611+00', '2026-06-19 17:14:25.353769+00', 'prometheus', 'promql', 'bytes', 'stat', '{}', 'Database', NULL);
INSERT INTO public.monitoring_metric_query (id, exporter_id, name, slug, label, query, unit, description, default_show_in_inspector, default_show_in_list, sort_order, enabled, created_at, updated_at, source_type, query_type, value_type, widget_type, options, category, semantic) VALUES ('019ee0e0-69fa-74b1-ba45-527317a4f221', '019ee0e0-69f8-7ec7-9b6d-03902c6b2d63', 'redis_up', 'redis_up', 'Redis up', 'redis_up{piping_host_name="$host",job="redis"}', 'state', 'Redis up', true, false, 10, true, '2026-06-19 17:14:24.121611+00', '2026-06-19 17:14:25.452606+00', 'prometheus', 'promql', 'state', 'badge', '{"display": {"status": {"mode": "state", "normal": ["up", "1", "true", "ok", "active"], "warning": ["unknown", "pending"], "critical": ["down", "0", "false", "failed", "error"]}}}', 'Database', NULL);
INSERT INTO public.monitoring_metric_query (id, exporter_id, name, slug, label, query, unit, description, default_show_in_inspector, default_show_in_list, sort_order, enabled, created_at, updated_at, source_type, query_type, value_type, widget_type, options, category, semantic) VALUES ('019ee0e0-69fa-74d2-93dd-2f41800ac2c3', '019ee0e0-69f8-7ede-afdb-4955362b6190', 'pve_up', 'pve_up', 'PVE scrape', 'up{piping_host_name="$host",job="proxmox"}', 'state', 'PVE scrape', true, false, 10, true, '2026-06-19 17:14:24.121611+00', '2026-06-19 17:14:25.452606+00', 'prometheus', 'promql', 'state', 'badge', '{"display": {"status": {"mode": "state", "normal": ["up", "1", "true", "ok", "active"], "warning": ["unknown", "pending"], "critical": ["down", "0", "false", "failed", "error"]}}}', 'Virtualization', NULL);
INSERT INTO public.monitoring_metric_query (id, exporter_id, name, slug, label, query, unit, description, default_show_in_inspector, default_show_in_list, sort_order, enabled, created_at, updated_at, source_type, query_type, value_type, widget_type, options, category, semantic) VALUES ('019ee0e0-69fa-74e9-a9a3-7e123422f560', '019ee0e0-69f8-7ef8-b4d0-25e2197c6d65', 'disk_temperature', 'disk_temperature', 'Disk temperature', 'max(smartctl_device_temperature{piping_host_name="$host",job="smartctl"})', 'C', 'Disk temperature', true, false, 10, true, '2026-06-19 17:14:24.121611+00', '2026-06-19 17:14:25.452606+00', 'prometheus', 'promql', 'number', 'stat', '{"display": {"status": {"mode": "numeric", "warning": 55, "critical": 65, "direction": "higher_is_worse"}}}', 'Storage', NULL);
INSERT INTO public.monitoring_metric_query (id, exporter_id, name, slug, label, query, unit, description, default_show_in_inspector, default_show_in_list, sort_order, enabled, created_at, updated_at, source_type, query_type, value_type, widget_type, options, category, semantic) VALUES ('019ee0e0-69fa-7520-ae0c-42883add0c34', '019ee0e0-69f8-7f2a-a7a2-d737c5cf0346', 'gitlab_up', 'gitlab_up', 'GitLab exporter', 'up{piping_host_name="$host",job="gitlab"}', 'state', 'GitLab exporter', true, false, 10, true, '2026-06-19 17:14:24.121611+00', '2026-06-19 17:14:25.452606+00', 'prometheus', 'promql', 'state', 'badge', '{"display": {"status": {"mode": "state", "normal": ["up", "1", "true", "ok", "active"], "warning": ["unknown", "pending"], "critical": ["down", "0", "false", "failed", "error"]}}}', 'Services', NULL);
INSERT INTO public.monitoring_metric_query (id, exporter_id, name, slug, label, query, unit, description, default_show_in_inspector, default_show_in_list, sort_order, enabled, created_at, updated_at, source_type, query_type, value_type, widget_type, options, category, semantic) VALUES ('019ee0e0-6a82-7521-8072-73effd45b7e3', NULL, 'zabbix_system_uptime', 'zabbix-system-uptime', 'System uptime', 'system.uptime', 's', 'System uptime', true, false, 20, true, '2026-06-19 17:14:24.258172+00', '2026-06-19 17:14:25.353769+00', 'zabbix', 'zabbix_item_key', 'duration', 'stat', '{}', 'System', NULL);
INSERT INTO public.monitoring_metric_query (id, exporter_id, name, slug, label, query, unit, description, default_show_in_inspector, default_show_in_list, sort_order, enabled, created_at, updated_at, source_type, query_type, value_type, widget_type, options, category, semantic) VALUES ('019ee0e0-6a82-75e9-ae8a-1958b62abd3e', NULL, 'zabbix_ssh_last_login', 'zabbix-ssh-last-login', 'Latest SSH login', 'log[/var/log/auth.log,"Accepted (publickey|password)",,,skip]', NULL, 'Latest SSH login', true, false, 60, true, '2026-06-19 17:14:24.258172+00', '2026-06-19 17:14:25.353769+00', 'zabbix', 'zabbix_item_key', 'log', 'log', '{}', 'System', NULL);
INSERT INTO public.monitoring_metric_query (id, exporter_id, name, slug, label, query, unit, description, default_show_in_inspector, default_show_in_list, sort_order, enabled, created_at, updated_at, source_type, query_type, value_type, widget_type, options, category, semantic) VALUES ('019ee0e0-6a82-73f7-b1fa-95eaff31b9df', NULL, 'zabbix_agent_ping', 'zabbix-agent-ping', 'Agent ping', 'agent.ping', 'state', 'Agent ping', true, false, 10, true, '2026-06-19 17:14:24.258172+00', '2026-06-19 17:14:25.452606+00', 'zabbix', 'zabbix_item_key', 'state', 'badge', '{"display": {"status": {"mode": "state", "normal": ["up", "1", "true", "ok", "active"], "warning": ["unknown", "pending"], "critical": ["down", "0", "false", "failed", "error"]}}}', 'System', NULL);
INSERT INTO public.monitoring_metric_query (id, exporter_id, name, slug, label, query, unit, description, default_show_in_inspector, default_show_in_list, sort_order, enabled, created_at, updated_at, source_type, query_type, value_type, widget_type, options, category, semantic) VALUES ('019ee0e0-6a82-7590-a322-9c33d04d761d', NULL, 'zabbix_memory_used', 'zabbix-memory-used', 'Memory used', 'vm.memory.size[pused]', '%', 'Memory used', true, false, 40, true, '2026-06-19 17:14:24.258172+00', '2026-06-19 17:14:25.452606+00', 'zabbix', 'zabbix_item_key', 'percent', 'gauge', '{"display": {"status": {"mode": "numeric", "warning": 80, "critical": 92, "direction": "higher_is_worse"}}}', 'System', NULL);
INSERT INTO public.monitoring_metric_query (id, exporter_id, name, slug, label, query, unit, description, default_show_in_inspector, default_show_in_list, sort_order, enabled, created_at, updated_at, source_type, query_type, value_type, widget_type, options, category, semantic) VALUES ('019ee0e0-6a82-75bc-95b7-2879c31da16b', NULL, 'zabbix_disk_used_root', 'zabbix-disk-used-root', 'Disk used /', 'vfs.fs.size[/,pused]', '%', 'Disk used /', true, false, 50, true, '2026-06-19 17:14:24.258172+00', '2026-06-19 17:14:25.452606+00', 'zabbix', 'zabbix_item_key', 'percent', 'gauge', '{"display": {"status": {"mode": "numeric", "warning": 80, "critical": 90, "direction": "higher_is_worse"}}}', 'System', NULL);
INSERT INTO public.monitoring_metric_query (id, exporter_id, name, slug, label, query, unit, description, default_show_in_inspector, default_show_in_list, sort_order, enabled, created_at, updated_at, source_type, query_type, value_type, widget_type, options, category, semantic) VALUES ('019ee0e0-69fa-73d8-a6f3-5431e6e7e8d6', '019ee0e0-69f8-7d17-93d1-d5299497329a', 'up', 'up', 'Up', 'up{job="node"}', 'state', 'Up', true, true, 10, true, '2026-06-19 17:14:24.121611+00', CURRENT_TIMESTAMP, 'prometheus', 'promql', 'state', 'badge', '{"display": {"status": {"mode": "state", "normal": ["up", "1", "true", "ok", "active"], "warning": ["unknown", "pending"], "critical": ["down", "0", "false", "failed", "error"]}}}', 'System', 'availability');
INSERT INTO public.monitoring_metric_query (id, exporter_id, name, slug, label, query, unit, description, default_show_in_inspector, default_show_in_list, sort_order, enabled, created_at, updated_at, source_type, query_type, value_type, widget_type, options, category, semantic) VALUES ('019ee0e0-6c42-73c0-92f5-8f2d1c95deb0', '019ee0e0-6bd4-7a9c-8832-2265afd237ef', 'probe_success', 'probe_success', 'Reachable', 'probe_success{piping_host_name="$host",job="blackbox"}', 'state', 'Reachable', true, true, 10, true, '2026-06-19 17:14:24.704043+00', CURRENT_TIMESTAMP, 'prometheus', 'promql', 'auto', 'stat', '{"display": {"status": {"mode": "state", "normal": ["up", "1", "true", "ok", "active"], "warning": ["unknown", "pending"], "critical": ["down", "0", "false", "failed", "error"]}}}', 'Network', NULL);


--
-- Data for Name: monitoring_metric_query_host_override; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: monitoring_metric_set; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.monitoring_metric_set (id, name, slug, description, enabled, created_at, updated_at) VALUES ('019ee0e0-69fc-7746-8901-5f0382399367', 'Default inspector metrics', 'default-inspector', 'Default connection target inspector metrics for node exporter', true, '2026-06-19 17:14:24.124395+00', '2026-06-19 17:14:24.124395+00');
INSERT INTO public.monitoring_metric_set (id, name, slug, description, enabled, created_at, updated_at) VALUES ('019ee0e0-6a83-7565-90d1-c66cae0010b1', 'Zabbix default inspector metrics', 'zabbix-default-inspector', 'Default connection target inspector metrics from Zabbix', true, '2026-06-19 17:14:24.259309+00', '2026-06-19 17:14:24.259309+00');
INSERT INTO public.monitoring_metric_set (id, name, slug, description, enabled, created_at, updated_at) VALUES ('019ee0e1-0001-7000-8000-000000000001', 'Node exporter quick view', 'preset-node-exporter', 'Up + CPU + memory + root disk for Linux node exporter.', true, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);
INSERT INTO public.monitoring_metric_set (id, name, slug, description, enabled, created_at, updated_at) VALUES ('019ee0e1-0001-7000-8000-000000000002', 'cAdvisor quick view', 'preset-cadvisor', 'Container count and CPU pressure for Docker hosts.', true, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);
INSERT INTO public.monitoring_metric_set (id, name, slug, description, enabled, created_at, updated_at) VALUES ('019ee0e1-0001-7000-8000-000000000003', 'Blackbox ICMP probe', 'preset-blackbox-icmp', 'Reachability + ICMP RTT + total probe duration via the blackbox exporter.', true, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);
INSERT INTO public.monitoring_metric_set (id, name, slug, description, enabled, created_at, updated_at) VALUES ('019ee0e1-0001-7000-8000-000000000004', 'Postgres exporter quick view', 'preset-postgres-exporter', 'Active database connection count for the postgres exporter.', true, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);


--
-- Data for Name: monitoring_metric_set_assignment; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.monitoring_metric_set_assignment (id, metric_set_id, scope_type, scope_id, weight, enabled, created_at, updated_at) VALUES ('00000000-0000-7000-8000-000000001311', '019ee0e0-69fc-7746-8901-5f0382399367', 'global', NULL, 0, false, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);
INSERT INTO public.monitoring_metric_set_assignment (id, metric_set_id, scope_type, scope_id, weight, enabled, created_at, updated_at) VALUES ('00000000-0000-7000-8000-000000001312', '019ee0e1-0001-7000-8000-000000000001', 'global', NULL, 10, false, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);
INSERT INTO public.monitoring_metric_set_assignment (id, metric_set_id, scope_type, scope_id, weight, enabled, created_at, updated_at) VALUES ('00000000-0000-7000-8000-000000001313', '019ee0e0-6a83-7565-90d1-c66cae0010b1', 'global', NULL, 20, false, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);


--
-- Data for Name: monitoring_metric_set_item; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.monitoring_metric_set_item (id, metric_set_id, metric_query_id, show_in_inspector, show_in_list, show_in_hover, list_render, sort_order, label_override, unit_override, created_at, updated_at) VALUES ('019ee0e0-69fd-7b48-8dff-cf17407b413c', '019ee0e0-69fc-7746-8901-5f0382399367', '019ee0e0-69fa-73d8-a6f3-5431e6e7e8d6', true, false, false, 'auto', 10, NULL, NULL, '2026-06-19 17:14:24.125448+00', '2026-06-19 17:14:24.125448+00');
INSERT INTO public.monitoring_metric_set_item (id, metric_set_id, metric_query_id, show_in_inspector, show_in_list, show_in_hover, list_render, sort_order, label_override, unit_override, created_at, updated_at) VALUES ('019ee0e0-69fd-7c6e-8571-76b237dda637', '019ee0e0-69fc-7746-8901-5f0382399367', '019ee0e0-69fa-73bd-966a-737cbb254279', true, false, false, 'auto', 20, NULL, NULL, '2026-06-19 17:14:24.125448+00', '2026-06-19 17:14:24.125448+00');
INSERT INTO public.monitoring_metric_set_item (id, metric_set_id, metric_query_id, show_in_inspector, show_in_list, show_in_hover, list_render, sort_order, label_override, unit_override, created_at, updated_at) VALUES ('019ee0e0-69fd-7c93-9a60-7cf584ccb582', '019ee0e0-69fc-7746-8901-5f0382399367', '019ee0e0-69fa-7394-8801-a669209ca971', true, false, false, 'auto', 30, NULL, NULL, '2026-06-19 17:14:24.125448+00', '2026-06-19 17:14:24.125448+00');
INSERT INTO public.monitoring_metric_set_item (id, metric_set_id, metric_query_id, show_in_inspector, show_in_list, show_in_hover, list_render, sort_order, label_override, unit_override, created_at, updated_at) VALUES ('019ee0e0-69fd-7caf-a07d-f37081de8ed0', '019ee0e0-69fc-7746-8901-5f0382399367', '019ee0e0-69fa-7288-acbe-934f2cccee73', true, false, false, 'auto', 40, NULL, NULL, '2026-06-19 17:14:24.125448+00', '2026-06-19 17:14:24.125448+00');
INSERT INTO public.monitoring_metric_set_item (id, metric_set_id, metric_query_id, show_in_inspector, show_in_list, show_in_hover, list_render, sort_order, label_override, unit_override, created_at, updated_at) VALUES ('019ee0e0-6a81-7623-a73c-8159de046c22', '019ee0e0-69fc-7746-8901-5f0382399367', '019ee0e0-6a7f-7996-9e3f-bde12af6ff37', true, false, false, 'auto', 50, NULL, NULL, '2026-06-19 17:14:24.256719+00', '2026-06-19 17:14:24.256719+00');
INSERT INTO public.monitoring_metric_set_item (id, metric_set_id, metric_query_id, show_in_inspector, show_in_list, show_in_hover, list_render, sort_order, label_override, unit_override, created_at, updated_at) VALUES ('019ee0e0-6a84-711b-8b2d-0d930b052504', '019ee0e0-6a83-7565-90d1-c66cae0010b1', '019ee0e0-6a82-73f7-b1fa-95eaff31b9df', true, false, false, 'auto', 10, NULL, NULL, '2026-06-19 17:14:24.259963+00', '2026-06-19 17:14:24.259963+00');
INSERT INTO public.monitoring_metric_set_item (id, metric_set_id, metric_query_id, show_in_inspector, show_in_list, show_in_hover, list_render, sort_order, label_override, unit_override, created_at, updated_at) VALUES ('019ee0e0-6a84-7156-a6f8-f2d0d2c4f88c', '019ee0e0-6a83-7565-90d1-c66cae0010b1', '019ee0e0-6a82-7521-8072-73effd45b7e3', true, false, false, 'auto', 20, NULL, NULL, '2026-06-19 17:14:24.259963+00', '2026-06-19 17:14:24.259963+00');
INSERT INTO public.monitoring_metric_set_item (id, metric_set_id, metric_query_id, show_in_inspector, show_in_list, show_in_hover, list_render, sort_order, label_override, unit_override, created_at, updated_at) VALUES ('019ee0e0-6a84-716e-b4d4-6ffe137285a1', '019ee0e0-6a83-7565-90d1-c66cae0010b1', '019ee0e0-6a82-755e-9cac-700055715cc4', true, false, false, 'auto', 30, NULL, NULL, '2026-06-19 17:14:24.259963+00', '2026-06-19 17:14:24.259963+00');
INSERT INTO public.monitoring_metric_set_item (id, metric_set_id, metric_query_id, show_in_inspector, show_in_list, show_in_hover, list_render, sort_order, label_override, unit_override, created_at, updated_at) VALUES ('019ee0e0-6a84-7185-931e-afb3c1030727', '019ee0e0-6a83-7565-90d1-c66cae0010b1', '019ee0e0-6a82-7590-a322-9c33d04d761d', true, false, false, 'auto', 40, NULL, NULL, '2026-06-19 17:14:24.259963+00', '2026-06-19 17:14:24.259963+00');
INSERT INTO public.monitoring_metric_set_item (id, metric_set_id, metric_query_id, show_in_inspector, show_in_list, show_in_hover, list_render, sort_order, label_override, unit_override, created_at, updated_at) VALUES ('019ee0e0-6a84-7199-991b-7479b6cd95b6', '019ee0e0-6a83-7565-90d1-c66cae0010b1', '019ee0e0-6a82-75bc-95b7-2879c31da16b', true, false, false, 'auto', 50, NULL, NULL, '2026-06-19 17:14:24.259963+00', '2026-06-19 17:14:24.259963+00');
INSERT INTO public.monitoring_metric_set_item (id, metric_set_id, metric_query_id, show_in_inspector, show_in_list, show_in_hover, list_render, sort_order, label_override, unit_override, created_at, updated_at) VALUES ('019ee0e0-6a84-71ac-99fc-4922319c02c7', '019ee0e0-6a83-7565-90d1-c66cae0010b1', '019ee0e0-6a82-75e9-ae8a-1958b62abd3e', true, false, false, 'auto', 60, NULL, NULL, '2026-06-19 17:14:24.259963+00', '2026-06-19 17:14:24.259963+00');
INSERT INTO public.monitoring_metric_set_item (id, metric_set_id, metric_query_id, show_in_inspector, show_in_list, show_in_hover, list_render, sort_order, label_override, unit_override, created_at, updated_at) VALUES ('019ee0e0-6fac-7802-9373-c5d6b925e8be', '019ee0e0-69fc-7746-8901-5f0382399367', '019ee0e0-6faa-7237-ac66-2491b4d53be5', true, false, false, 'auto', 15, NULL, NULL, '2026-06-19 17:14:25.579573+00', '2026-06-19 17:14:25.579573+00');
INSERT INTO public.monitoring_metric_set_item (id, metric_set_id, metric_query_id, show_in_inspector, show_in_list, show_in_hover, list_render, sort_order, label_override, unit_override, created_at, updated_at) VALUES ('019ee0e0-6fac-78a2-b3e7-7b498fc607cd', '019ee0e0-69fc-7746-8901-5f0382399367', '019ee0e0-6faa-741f-a143-5bfeaac16fda', true, false, false, 'auto', 55, NULL, NULL, '2026-06-19 17:14:25.579573+00', '2026-06-19 17:14:25.579573+00');
INSERT INTO public.monitoring_metric_set_item (id, metric_set_id, metric_query_id, show_in_inspector, show_in_list, show_in_hover, list_render, sort_order, label_override, unit_override, created_at, updated_at) VALUES ('019ee0e0-6fac-78c9-86fe-0c10cff70cae', '019ee0e0-69fc-7746-8901-5f0382399367', '019ee0e0-6faa-7453-941c-d76f7124bacf', true, false, false, 'auto', 60, NULL, NULL, '2026-06-19 17:14:25.579573+00', '2026-06-19 17:14:25.579573+00');
INSERT INTO public.monitoring_metric_set_item (id, metric_set_id, metric_query_id, show_in_inspector, show_in_list, show_in_hover, list_render, sort_order, label_override, unit_override, created_at, updated_at) VALUES ('019ee0e0-6fac-78f0-8aeb-1b433ba7dc85', '019ee0e0-69fc-7746-8901-5f0382399367', '019ee0e0-6faa-7483-b416-84338566195a', true, false, false, 'auto', 70, NULL, NULL, '2026-06-19 17:14:25.579573+00', '2026-06-19 17:14:25.579573+00');
INSERT INTO public.monitoring_metric_set_item (id, metric_set_id, metric_query_id, show_in_inspector, show_in_list, show_in_hover, list_render, sort_order, label_override, unit_override, created_at, updated_at) VALUES (uuidv7(), '019ee0e1-0001-7000-8000-000000000001', '019ee0e0-6a7f-7996-9e3f-bde12af6ff37', true, true, false, 'auto', 50, NULL, NULL, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);
INSERT INTO public.monitoring_metric_set_item (id, metric_set_id, metric_query_id, show_in_inspector, show_in_list, show_in_hover, list_render, sort_order, label_override, unit_override, created_at, updated_at) VALUES (uuidv7(), '019ee0e1-0001-7000-8000-000000000001', '019ee0e0-69fa-7288-acbe-934f2cccee73', true, true, false, 'auto', 40, NULL, NULL, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);
INSERT INTO public.monitoring_metric_set_item (id, metric_set_id, metric_query_id, show_in_inspector, show_in_list, show_in_hover, list_render, sort_order, label_override, unit_override, created_at, updated_at) VALUES (uuidv7(), '019ee0e1-0001-7000-8000-000000000001', '019ee0e0-69fa-7394-8801-a669209ca971', true, true, false, 'auto', 30, NULL, NULL, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);
INSERT INTO public.monitoring_metric_set_item (id, metric_set_id, metric_query_id, show_in_inspector, show_in_list, show_in_hover, list_render, sort_order, label_override, unit_override, created_at, updated_at) VALUES (uuidv7(), '019ee0e1-0001-7000-8000-000000000001', '019ee0e0-69fa-73d8-a6f3-5431e6e7e8d6', true, true, false, 'auto', 10, NULL, NULL, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);
INSERT INTO public.monitoring_metric_set_item (id, metric_set_id, metric_query_id, show_in_inspector, show_in_list, show_in_hover, list_render, sort_order, label_override, unit_override, created_at, updated_at) VALUES (uuidv7(), '019ee0e1-0001-7000-8000-000000000002', '019ee0e0-69fa-7401-b622-220ae754177c', true, true, false, 'auto', 20, NULL, NULL, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);
INSERT INTO public.monitoring_metric_set_item (id, metric_set_id, metric_query_id, show_in_inspector, show_in_list, show_in_hover, list_render, sort_order, label_override, unit_override, created_at, updated_at) VALUES (uuidv7(), '019ee0e1-0001-7000-8000-000000000002', '019ee0e0-69fa-7419-af76-bfc3d44db100', true, true, false, 'auto', 10, NULL, NULL, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);
INSERT INTO public.monitoring_metric_set_item (id, metric_set_id, metric_query_id, show_in_inspector, show_in_list, show_in_hover, list_render, sort_order, label_override, unit_override, created_at, updated_at) VALUES (uuidv7(), '019ee0e1-0001-7000-8000-000000000004', '019ee0e0-69fa-7463-86ce-87ee09e99c97', true, true, false, 'auto', 20, NULL, NULL, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);
INSERT INTO public.monitoring_metric_set_item (id, metric_set_id, metric_query_id, show_in_inspector, show_in_list, show_in_hover, list_render, sort_order, label_override, unit_override, created_at, updated_at) VALUES (uuidv7(), '019ee0e1-0001-7000-8000-000000000003', '019ee0e0-6c42-7383-9e09-e3918290b10f', true, false, false, 'auto', 20, NULL, NULL, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);
INSERT INTO public.monitoring_metric_set_item (id, metric_set_id, metric_query_id, show_in_inspector, show_in_list, show_in_hover, list_render, sort_order, label_override, unit_override, created_at, updated_at) VALUES (uuidv7(), '019ee0e1-0001-7000-8000-000000000003', '019ee0e0-6c42-70e7-954c-a6841cb0cb46', true, true, false, 'auto', 10, NULL, NULL, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);
INSERT INTO public.monitoring_metric_set_item (id, metric_set_id, metric_query_id, show_in_inspector, show_in_list, show_in_hover, list_render, sort_order, label_override, unit_override, created_at, updated_at) VALUES (uuidv7(), '019ee0e1-0001-7000-8000-000000000003', '019ee0e0-6c42-73c0-92f5-8f2d1c95deb0', true, true, false, 'auto', 5, NULL, NULL, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);


--
-- Data for Name: monitoring_query_source; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.monitoring_query_source (id, name, slug, query_url, timeout_ms, enabled, created_at, updated_at, source_type, options) VALUES ('00000000-0000-7000-8000-000000001301', 'Factory Prometheus', 'factory-prometheus', 'http://prometheus.example.invalid/api/v1/query', 3000, false, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, 'prometheus', '{"factory": true, "placeholder": true}');


--
-- Data for Name: monitoring_query_source_assignment; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.monitoring_query_source_assignment (id, query_source_id, scope_type, scope_id, weight, enabled, created_at, updated_at, options) VALUES ('00000000-0000-7000-8000-000000001302', '00000000-0000-7000-8000-000000001301', 'global', NULL, 0, false, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, '{"factory": true}');


--
-- Data for Name: monitoring_sd_label_settings; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.monitoring_sd_label_settings (id, groups, updated_at) VALUES (true, '{"role": true, "status": true, "tenant": true, "network": false, "contacts": false, "hardware": false, "location": true, "description": false, "site_statuses": ["active"], "device_statuses": ["active"]}', '2026-06-19 17:14:28.388589+00');


--
-- Data for Name: playbook; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: playbook_artifact; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: playbook_assignment; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: playbook_execution_environment; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.playbook_execution_environment (id, name, description, image, pull_policy, runner_group, enabled, env, settings, metadata, created_by_user_id, updated_by_user_id, created_at, updated_at) VALUES ('00000000-0000-7000-8000-000000000301', 'Default Ansible', 'Built-in native ansible-playbook execution environment.', 'piping-ansible-runner:local', 'if_not_present', 'default', true, '{}', '{}', '{"system": true}', NULL, NULL, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);


--
-- Data for Name: playbook_job_template; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: playbook_job_template_survey; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: playbook_job_token; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: playbook_project; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: playbook_run; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: playbook_run_event; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: playbook_run_group; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: playbook_run_group_callback_token; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: playbook_run_group_event_batch; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: playbook_schedule; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: playbook_schedule_occurrence; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: playbook_service_principal; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: playbook_service_principal_credential_binding; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: playbook_target_set; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: playbook_target_set_member; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: playbook_version; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: provider_request_journal; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: rbac_policy; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.rbac_policy (id, name, effect, actions, resource_types, constraints, subjects, resources, enabled, description) VALUES ('019ee0e0-6059-7789-b1bd-d181a4ae33be', 'editor:workspace-write', 'allow', '["CREATE", "READ", "UPDATE", "CONNECT", "OBSERVE", "EXECUTE", "WRITE"]', '["CONNECTION_TARGET", "CONNECTION_SESSION", "INVENTORY", "CONFIG_CONTEXT", "DEVICE", "VIRTUAL_MACHINE", "WEBHOOK"]', NULL, '["role:019ee0e0-6059-7727-b92e-47fee1abe29f"]', '["*"]', true, NULL);
INSERT INTO public.rbac_policy (id, name, effect, actions, resource_types, constraints, subjects, resources, enabled, description) VALUES ('019ee0e0-605a-7882-bf3b-3cc0904df23c', 'viewer:portal-access', 'allow', '["READ", "CONNECT", "OBSERVE"]', '["CONNECTION_TARGET", "CONNECTION_SESSION", "INVENTORY", "CONFIG_CONTEXT", "DEVICE", "VIRTUAL_MACHINE"]', NULL, '["role:019ee0e0-605a-784b-8e9b-04608346a461"]', '["*"]', true, NULL);
INSERT INTO public.rbac_policy (id, name, effect, actions, resource_types, constraints, subjects, resources, enabled, description) VALUES ('019ee0e0-6058-77ce-bcd7-415fc97c2c34', 'admin:full-access', 'allow', '["CREATE", "READ", "UPDATE", "DELETE", "CONNECT", "OBSERVE", "EXECUTE", "MANAGE_ROLES", "MANAGE_POLICIES", "VIEW_AUDIT", "WRITE", "SECRET"]', '["ASSISTANT", "ASSISTANT_CONVERSATION", "AUDIT_EVENT", "BACKUP", "BRANDING", "CONFIG_CONTEXT", "CONNECTION_FILTER_PRESET", "CONNECTION_SESSION", "CONNECTION_TARGET", "DEVICE", "GROUP", "INVENTORY", "KEYCLOAK", "LICENSE", "MONITORING", "PLAYBOOK", "PLAYBOOK_ARTIFACT", "PLAYBOOK_ASSIGNMENT", "PLAYBOOK_EXECUTION_ENVIRONMENT", "PLAYBOOK_JOB_TEMPLATE", "PLAYBOOK_JOB_TOKEN", "PLAYBOOK_PROJECT", "PLAYBOOK_RUN", "PLAYBOOK_RUN_GROUP", "PLAYBOOK_SCHEDULE", "PLAYBOOK_SERVICE_PRINCIPAL", "PLAYBOOK_TARGET_SET", "PLAYBOOK_VERSION", "POLICY", "PROXY_NODE", "ROLE", "SECRET_VARIABLE", "SETTINGS", "USER", "VIRTUAL_MACHINE", "WEBHOOK"]', NULL, '["role:019ee0e0-6058-70cd-b2a5-7685da7c5c09"]', '["*"]', true, NULL);


--
-- Data for Name: rbac_role; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.rbac_role (id, name, description) VALUES ('019ee0e0-6058-70cd-b2a5-7685da7c5c09', 'admin', 'Full administrative access');
INSERT INTO public.rbac_role (id, name, description) VALUES ('019ee0e0-6059-7727-b92e-47fee1abe29f', 'editor', 'Workspace editing and connection access without RBAC administration');
INSERT INTO public.rbac_role (id, name, description) VALUES ('019ee0e0-605a-784b-8e9b-04608346a461', 'viewer', 'Read-only portal access with connection launch permissions');


--
-- Data for Name: rbac_role_assignment; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.rbac_role_assignment (id, user_id, group_id, role_id) VALUES ('019ee0e0-605b-7dcc-b6a3-ff4ebbf730c8', NULL, '019ee0e0-605b-7c56-98ac-4e78e5786c8d', '019ee0e0-605a-784b-8e9b-04608346a461');
INSERT INTO public.rbac_role_assignment (id, user_id, group_id, role_id) VALUES ('019ee0e0-605c-7fd4-a88b-04b36b080edd', NULL, '019ee0e0-605c-7f88-86f6-86bb160e0a70', '019ee0e0-605a-784b-8e9b-04608346a461');
INSERT INTO public.rbac_role_assignment (id, user_id, group_id, role_id) VALUES ('019ee0e0-605d-7b83-bd52-6ceb08af7db0', NULL, '019ee0e0-605d-7b47-baf8-704e6b94501a', '019ee0e0-6059-7727-b92e-47fee1abe29f');
INSERT INTO public.rbac_role_assignment (id, user_id, group_id, role_id) VALUES ('019ee0e0-605e-7b65-b590-fa04cbadf8ef', NULL, '019ee0e0-605e-7b2c-aff9-941de10cb0a9', '019ee0e0-6058-70cd-b2a5-7685da7c5c09');


--
-- Data for Name: secret_usage_binding; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: secrets_binding; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: secrets_store; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: secrets_variable; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: settings_values; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: ssh_host_key_pin; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: sync_conflict_record; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: sync_error; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: sync_inventory_binding; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: sync_job; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: sync_job_step; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: sync_outbound_event; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: sync_provider; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: sync_run; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: sync_source_runtime; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: sync_webhook_event; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: tenancy_contact; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: tenancy_contact_groups; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: tenancy_contactassignment; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: tenancy_contactgroup; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: tenancy_contactrole; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.tenancy_contactrole (id, custom_field_data, name, slug, description, owner_id, comments, tags, external_id, piping_synced_at) VALUES ('00000000-0000-7000-8000-000000001031', '{}', 'Technical contact', 'technical-contact', 'Default role for technical ownership.', NULL, '', '{factory-default}', NULL, NULL);
INSERT INTO public.tenancy_contactrole (id, custom_field_data, name, slug, description, owner_id, comments, tags, external_id, piping_synced_at) VALUES ('00000000-0000-7000-8000-000000001032', '{}', 'Administrative contact', 'administrative-contact', 'Default role for administrative ownership.', NULL, '', '{factory-default}', NULL, NULL);
INSERT INTO public.tenancy_contactrole (id, custom_field_data, name, slug, description, owner_id, comments, tags, external_id, piping_synced_at) VALUES ('00000000-0000-7000-8000-000000001033', '{}', 'Escalation contact', 'escalation-contact', 'Default role for escalation ownership.', NULL, '', '{factory-default}', NULL, NULL);


--
-- Data for Name: tenancy_tenant; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: tenancy_tenantgroup; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: thumbnail_kvstore; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: user_preferences; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: users_owner; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: users_owner_identity_group; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: users_owner_identity_user; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: users_ownergroup; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: virtualization_cluster; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: virtualization_clustergroup; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: virtualization_clustertype; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.virtualization_clustertype (custom_field_data, id, name, slug, description, owner_id, comments, tags, external_id, piping_synced_at) VALUES ('{}', '00000000-0000-7000-8000-000000001051', 'Virtualization', 'virtualization', 'Generic virtualization cluster type.', NULL, '', '{factory-default}', NULL, NULL);
INSERT INTO public.virtualization_clustertype (custom_field_data, id, name, slug, description, owner_id, comments, tags, external_id, piping_synced_at) VALUES ('{}', '00000000-0000-7000-8000-000000001052', 'Container platform', 'container-platform', 'Generic container platform cluster type.', NULL, '', '{factory-default}', NULL, NULL);
INSERT INTO public.virtualization_clustertype (custom_field_data, id, name, slug, description, owner_id, comments, tags, external_id, piping_synced_at) VALUES ('{}', '00000000-0000-7000-8000-000000001053', 'Application cluster', 'application-cluster', 'Generic application cluster type.', NULL, '', '{factory-default}', NULL, NULL);


--
-- Data for Name: virtualization_virtualdisk; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: virtualization_virtualmachine; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: virtualization_virtualmachinetype; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: virtualization_vminterface; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: virtualization_vminterface_tagged_vlans; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: vpn_ikepolicy; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: vpn_ikepolicy_proposals; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: vpn_ikeproposal; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: vpn_ipsecpolicy; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: vpn_ipsecpolicy_proposals; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: vpn_ipsecprofile; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: vpn_ipsecproposal; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: vpn_l2vpn; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: vpn_l2vpn_export_targets; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: vpn_l2vpn_import_targets; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: vpn_l2vpntermination; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: vpn_tunnel; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: vpn_tunnelgroup; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: vpn_tunneltermination; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: webhook_subscriptions; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: wireless_wirelesslan; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: wireless_wirelesslangroup; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: wireless_wirelesslink; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: wizard_progress; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Data for Name: workspace_connection_target_layout; Type: TABLE DATA; Schema: public; Owner: -
--



--
-- Name: assistant_messages assistant_messages_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.assistant_messages
    ADD CONSTRAINT assistant_messages_pkey PRIMARY KEY (id);


--
-- Name: assistant_sessions assistant_sessions_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.assistant_sessions
    ADD CONSTRAINT assistant_sessions_pkey PRIMARY KEY (id);


--
-- Name: audit_event audit_event_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.audit_event
    ADD CONSTRAINT audit_event_pkey PRIMARY KEY (id);


--
-- Name: auth_oidc_settings auth_oidc_settings_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.auth_oidc_settings
    ADD CONSTRAINT auth_oidc_settings_pkey PRIMARY KEY (id);


--
-- Name: auth_session auth_session_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.auth_session
    ADD CONSTRAINT auth_session_pkey PRIMARY KEY (id);


--
-- Name: auth_user_api_token auth_user_api_token_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.auth_user_api_token
    ADD CONSTRAINT auth_user_api_token_pkey PRIMARY KEY (id);


--
-- Name: auth_user_api_token_scope auth_user_api_token_scope_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.auth_user_api_token_scope
    ADD CONSTRAINT auth_user_api_token_scope_pkey PRIMARY KEY (id);


--
-- Name: auth_user_api_token_scope auth_user_api_token_scope_token_action_resource_pattern_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.auth_user_api_token_scope
    ADD CONSTRAINT auth_user_api_token_scope_token_action_resource_pattern_key UNIQUE (token_id, action, resource_type, resource_pattern);


--
-- Name: auth_user_credential auth_user_credential_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.auth_user_credential
    ADD CONSTRAINT auth_user_credential_pkey PRIMARY KEY (user_id);


--
-- Name: automation_template automation_template_name_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.automation_template
    ADD CONSTRAINT automation_template_name_key UNIQUE (name);


--
-- Name: automation_template automation_template_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.automation_template
    ADD CONSTRAINT automation_template_pkey PRIMARY KEY (id);


--
-- Name: circuits_circuit circuits_circuit_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_circuit
    ADD CONSTRAINT circuits_circuit_pkey PRIMARY KEY (id);


--
-- Name: circuits_circuit circuits_circuit_unique_provider_cid; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_circuit
    ADD CONSTRAINT circuits_circuit_unique_provider_cid UNIQUE (provider_id, cid);


--
-- Name: circuits_circuit circuits_circuit_unique_provideraccount_cid; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_circuit
    ADD CONSTRAINT circuits_circuit_unique_provideraccount_cid UNIQUE (provider_account_id, cid);


--
-- Name: circuits_circuitgroup circuits_circuitgroup_name_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_circuitgroup
    ADD CONSTRAINT circuits_circuitgroup_name_key UNIQUE (name);


--
-- Name: circuits_circuitgroup circuits_circuitgroup_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_circuitgroup
    ADD CONSTRAINT circuits_circuitgroup_pkey PRIMARY KEY (id);


--
-- Name: circuits_circuitgroup circuits_circuitgroup_slug_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_circuitgroup
    ADD CONSTRAINT circuits_circuitgroup_slug_key UNIQUE (slug);


--
-- Name: circuits_circuitgroupassignment circuits_circuitgroupassignment_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_circuitgroupassignment
    ADD CONSTRAINT circuits_circuitgroupassignment_pkey PRIMARY KEY (id);


--
-- Name: circuits_circuitgroupassignment circuits_circuitgroupassignment_unique_member_group; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_circuitgroupassignment
    ADD CONSTRAINT circuits_circuitgroupassignment_unique_member_group UNIQUE (member_type_id, member_id, group_id);


--
-- Name: circuits_circuittermination circuits_circuittermination_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_circuittermination
    ADD CONSTRAINT circuits_circuittermination_pkey PRIMARY KEY (id);


--
-- Name: circuits_circuittermination circuits_circuittermination_unique_circuit_term_side; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_circuittermination
    ADD CONSTRAINT circuits_circuittermination_unique_circuit_term_side UNIQUE (circuit_id, term_side);


--
-- Name: circuits_circuittype circuits_circuittype_name_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_circuittype
    ADD CONSTRAINT circuits_circuittype_name_key UNIQUE (name);


--
-- Name: circuits_circuittype circuits_circuittype_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_circuittype
    ADD CONSTRAINT circuits_circuittype_pkey PRIMARY KEY (id);


--
-- Name: circuits_circuittype circuits_circuittype_slug_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_circuittype
    ADD CONSTRAINT circuits_circuittype_slug_key UNIQUE (slug);


--
-- Name: circuits_provider_asns circuits_provider_asns_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_provider_asns
    ADD CONSTRAINT circuits_provider_asns_pkey PRIMARY KEY (id);


--
-- Name: circuits_provider_asns circuits_provider_asns_provider_id_asn_id_6e573798_uniq; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_provider_asns
    ADD CONSTRAINT circuits_provider_asns_provider_id_asn_id_6e573798_uniq UNIQUE (provider_id, asn_id);


--
-- Name: circuits_provider circuits_provider_name_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_provider
    ADD CONSTRAINT circuits_provider_name_key UNIQUE (name);


--
-- Name: circuits_provider circuits_provider_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_provider
    ADD CONSTRAINT circuits_provider_pkey PRIMARY KEY (id);


--
-- Name: circuits_provider circuits_provider_slug_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_provider
    ADD CONSTRAINT circuits_provider_slug_key UNIQUE (slug);


--
-- Name: circuits_provideraccount circuits_provideraccount_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_provideraccount
    ADD CONSTRAINT circuits_provideraccount_pkey PRIMARY KEY (id);


--
-- Name: circuits_provideraccount circuits_provideraccount_unique_provider_account; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_provideraccount
    ADD CONSTRAINT circuits_provideraccount_unique_provider_account UNIQUE (provider_id, account);


--
-- Name: circuits_providernetwork circuits_providernetwork_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_providernetwork
    ADD CONSTRAINT circuits_providernetwork_pkey PRIMARY KEY (id);


--
-- Name: circuits_providernetwork circuits_providernetwork_unique_provider_name; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_providernetwork
    ADD CONSTRAINT circuits_providernetwork_unique_provider_name UNIQUE (provider_id, name);


--
-- Name: circuits_virtualcircuit circuits_virtualcircuit_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_virtualcircuit
    ADD CONSTRAINT circuits_virtualcircuit_pkey PRIMARY KEY (id);


--
-- Name: circuits_virtualcircuit circuits_virtualcircuit_unique_provider_network_cid; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_virtualcircuit
    ADD CONSTRAINT circuits_virtualcircuit_unique_provider_network_cid UNIQUE (provider_network_id, cid);


--
-- Name: circuits_virtualcircuit circuits_virtualcircuit_unique_provideraccount_cid; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_virtualcircuit
    ADD CONSTRAINT circuits_virtualcircuit_unique_provideraccount_cid UNIQUE (provider_account_id, cid);


--
-- Name: circuits_virtualcircuittermination circuits_virtualcircuittermination_interface_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_virtualcircuittermination
    ADD CONSTRAINT circuits_virtualcircuittermination_interface_id_key UNIQUE (interface_id);


--
-- Name: circuits_virtualcircuittermination circuits_virtualcircuittermination_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_virtualcircuittermination
    ADD CONSTRAINT circuits_virtualcircuittermination_pkey PRIMARY KEY (id);


--
-- Name: circuits_virtualcircuittype circuits_virtualcircuittype_name_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_virtualcircuittype
    ADD CONSTRAINT circuits_virtualcircuittype_name_key UNIQUE (name);


--
-- Name: circuits_virtualcircuittype circuits_virtualcircuittype_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_virtualcircuittype
    ADD CONSTRAINT circuits_virtualcircuittype_pkey PRIMARY KEY (id);


--
-- Name: circuits_virtualcircuittype circuits_virtualcircuittype_slug_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_virtualcircuittype
    ADD CONSTRAINT circuits_virtualcircuittype_slug_key UNIQUE (slug);


--
-- Name: config_context_binding config_context_binding_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.config_context_binding
    ADD CONSTRAINT config_context_binding_pkey PRIMARY KEY (id);


--
-- Name: config_context config_context_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.config_context
    ADD CONSTRAINT config_context_pkey PRIMARY KEY (id);


--
-- Name: connconf_assignment connconf_assignment_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.connconf_assignment
    ADD CONSTRAINT connconf_assignment_pkey PRIMARY KEY (id);


--
-- Name: connconf_record connconf_record_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.connconf_record
    ADD CONSTRAINT connconf_record_pkey PRIMARY KEY (id);


--
-- Name: connection_filter_presets connection_filter_presets_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.connection_filter_presets
    ADD CONSTRAINT connection_filter_presets_pkey PRIMARY KEY (id);


--
-- Name: connections_proxy_assignment connections_proxy_assignment_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.connections_proxy_assignment
    ADD CONSTRAINT connections_proxy_assignment_pkey PRIMARY KEY (id);


--
-- Name: connections_proxy_node connections_proxy_node_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.connections_proxy_node
    ADD CONSTRAINT connections_proxy_node_pkey PRIMARY KEY (id);


--
-- Name: connections_proxy_node connections_proxy_node_slug_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.connections_proxy_node
    ADD CONSTRAINT connections_proxy_node_slug_key UNIQUE (slug);


--
-- Name: connections_session connections_session_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.connections_session
    ADD CONSTRAINT connections_session_pkey PRIMARY KEY (id);


--
-- Name: connections_template connections_template_name_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.connections_template
    ADD CONSTRAINT connections_template_name_key UNIQUE (name);


--
-- Name: connections_template connections_template_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.connections_template
    ADD CONSTRAINT connections_template_pkey PRIMARY KEY (id);


--
-- Name: connections_user_credential_set connections_user_credential_set_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.connections_user_credential_set
    ADD CONSTRAINT connections_user_credential_set_pkey PRIMARY KEY (id);


--
-- Name: custom_field_choice_set custom_field_choice_set_name_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.custom_field_choice_set
    ADD CONSTRAINT custom_field_choice_set_name_key UNIQUE (name);


--
-- Name: custom_field_choice_set custom_field_choice_set_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.custom_field_choice_set
    ADD CONSTRAINT custom_field_choice_set_pkey PRIMARY KEY (id);


--
-- Name: custom_field custom_field_name_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.custom_field
    ADD CONSTRAINT custom_field_name_key UNIQUE (name);


--
-- Name: custom_field_object_type custom_field_object_type_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.custom_field_object_type
    ADD CONSTRAINT custom_field_object_type_pkey PRIMARY KEY (custom_field_id, content_type_id);


--
-- Name: custom_field custom_field_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.custom_field
    ADD CONSTRAINT custom_field_pkey PRIMARY KEY (id);


--
-- Name: dcim_cable dcim_cable_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_cable
    ADD CONSTRAINT dcim_cable_pkey PRIMARY KEY (id);


--
-- Name: dcim_cablebundle dcim_cablebundle_name_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_cablebundle
    ADD CONSTRAINT dcim_cablebundle_name_key UNIQUE (name);


--
-- Name: dcim_cablebundle dcim_cablebundle_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_cablebundle
    ADD CONSTRAINT dcim_cablebundle_pkey PRIMARY KEY (id);


--
-- Name: dcim_cablepath dcim_cablepath_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_cablepath
    ADD CONSTRAINT dcim_cablepath_pkey PRIMARY KEY (id);


--
-- Name: dcim_cabletermination dcim_cabletermination_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_cabletermination
    ADD CONSTRAINT dcim_cabletermination_pkey PRIMARY KEY (id);


--
-- Name: dcim_cabletermination dcim_cabletermination_unique_connector; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_cabletermination
    ADD CONSTRAINT dcim_cabletermination_unique_connector UNIQUE (cable_id, cable_end, connector);


--
-- Name: dcim_cabletermination dcim_cabletermination_unique_termination; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_cabletermination
    ADD CONSTRAINT dcim_cabletermination_unique_termination UNIQUE (termination_type_id, termination_id);


--
-- Name: dcim_consoleport dcim_consoleport_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_consoleport
    ADD CONSTRAINT dcim_consoleport_pkey PRIMARY KEY (id);


--
-- Name: dcim_consoleport dcim_consoleport_unique_device_name; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_consoleport
    ADD CONSTRAINT dcim_consoleport_unique_device_name UNIQUE (device_id, name);


--
-- Name: dcim_consoleporttemplate dcim_consoleporttemplate_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_consoleporttemplate
    ADD CONSTRAINT dcim_consoleporttemplate_pkey PRIMARY KEY (id);


--
-- Name: dcim_consoleporttemplate dcim_consoleporttemplate_unique_device_type_name; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_consoleporttemplate
    ADD CONSTRAINT dcim_consoleporttemplate_unique_device_type_name UNIQUE (device_type_id, name);


--
-- Name: dcim_consoleporttemplate dcim_consoleporttemplate_unique_module_type_name; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_consoleporttemplate
    ADD CONSTRAINT dcim_consoleporttemplate_unique_module_type_name UNIQUE (module_type_id, name);


--
-- Name: dcim_consoleserverport dcim_consoleserverport_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_consoleserverport
    ADD CONSTRAINT dcim_consoleserverport_pkey PRIMARY KEY (id);


--
-- Name: dcim_consoleserverport dcim_consoleserverport_unique_device_name; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_consoleserverport
    ADD CONSTRAINT dcim_consoleserverport_unique_device_name UNIQUE (device_id, name);


--
-- Name: dcim_consoleserverporttemplate dcim_consoleserverporttemplate_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_consoleserverporttemplate
    ADD CONSTRAINT dcim_consoleserverporttemplate_pkey PRIMARY KEY (id);


--
-- Name: dcim_consoleserverporttemplate dcim_consoleserverporttemplate_unique_device_type_name; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_consoleserverporttemplate
    ADD CONSTRAINT dcim_consoleserverporttemplate_unique_device_type_name UNIQUE (device_type_id, name);


--
-- Name: dcim_consoleserverporttemplate dcim_consoleserverporttemplate_unique_module_type_name; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_consoleserverporttemplate
    ADD CONSTRAINT dcim_consoleserverporttemplate_unique_module_type_name UNIQUE (module_type_id, name);


--
-- Name: dcim_coolingfeed dcim_coolingfeed_cooling_source_name_uniq; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_coolingfeed
    ADD CONSTRAINT dcim_coolingfeed_cooling_source_name_uniq UNIQUE (cooling_source_id, name);


--
-- Name: dcim_coolingfeed dcim_coolingfeed_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_coolingfeed
    ADD CONSTRAINT dcim_coolingfeed_pkey PRIMARY KEY (id);


--
-- Name: dcim_coolingintake dcim_coolingintake_device_name_uniq; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_coolingintake
    ADD CONSTRAINT dcim_coolingintake_device_name_uniq UNIQUE (device_id, name);


--
-- Name: dcim_coolingintake dcim_coolingintake_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_coolingintake
    ADD CONSTRAINT dcim_coolingintake_pkey PRIMARY KEY (id);


--
-- Name: dcim_coolingintaketemplate dcim_coolingintaketemplate_device_type_name_uniq; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_coolingintaketemplate
    ADD CONSTRAINT dcim_coolingintaketemplate_device_type_name_uniq UNIQUE (device_type_id, name);


--
-- Name: dcim_coolingintaketemplate dcim_coolingintaketemplate_module_type_name_uniq; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_coolingintaketemplate
    ADD CONSTRAINT dcim_coolingintaketemplate_module_type_name_uniq UNIQUE (module_type_id, name);


--
-- Name: dcim_coolingintaketemplate dcim_coolingintaketemplate_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_coolingintaketemplate
    ADD CONSTRAINT dcim_coolingintaketemplate_pkey PRIMARY KEY (id);


--
-- Name: dcim_coolingoutflow dcim_coolingoutflow_device_name_uniq; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_coolingoutflow
    ADD CONSTRAINT dcim_coolingoutflow_device_name_uniq UNIQUE (device_id, name);


--
-- Name: dcim_coolingoutflow dcim_coolingoutflow_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_coolingoutflow
    ADD CONSTRAINT dcim_coolingoutflow_pkey PRIMARY KEY (id);


--
-- Name: dcim_coolingoutflowtemplate dcim_coolingoutflowtemplate_device_type_name_uniq; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_coolingoutflowtemplate
    ADD CONSTRAINT dcim_coolingoutflowtemplate_device_type_name_uniq UNIQUE (device_type_id, name);


--
-- Name: dcim_coolingoutflowtemplate dcim_coolingoutflowtemplate_module_type_name_uniq; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_coolingoutflowtemplate
    ADD CONSTRAINT dcim_coolingoutflowtemplate_module_type_name_uniq UNIQUE (module_type_id, name);


--
-- Name: dcim_coolingoutflowtemplate dcim_coolingoutflowtemplate_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_coolingoutflowtemplate
    ADD CONSTRAINT dcim_coolingoutflowtemplate_pkey PRIMARY KEY (id);


--
-- Name: dcim_coolingsource dcim_coolingsource_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_coolingsource
    ADD CONSTRAINT dcim_coolingsource_pkey PRIMARY KEY (id);


--
-- Name: dcim_coolingsource dcim_coolingsource_site_name_uniq; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_coolingsource
    ADD CONSTRAINT dcim_coolingsource_site_name_uniq UNIQUE (site_id, name);


--
-- Name: dcim_device dcim_device_asset_tag_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_device
    ADD CONSTRAINT dcim_device_asset_tag_key UNIQUE (asset_tag);


--
-- Name: dcim_device dcim_device_oob_ip_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_device
    ADD CONSTRAINT dcim_device_oob_ip_id_key UNIQUE (oob_ip_id);


--
-- Name: dcim_device dcim_device_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_device
    ADD CONSTRAINT dcim_device_pkey PRIMARY KEY (id);


--
-- Name: dcim_device dcim_device_primary_ip4_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_device
    ADD CONSTRAINT dcim_device_primary_ip4_id_key UNIQUE (primary_ip4_id);


--
-- Name: dcim_device dcim_device_primary_ip6_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_device
    ADD CONSTRAINT dcim_device_primary_ip6_id_key UNIQUE (primary_ip6_id);


--
-- Name: dcim_device dcim_device_unique_rack_position_face; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_device
    ADD CONSTRAINT dcim_device_unique_rack_position_face UNIQUE (rack_id, "position", face);


--
-- Name: dcim_device dcim_device_unique_virtual_chassis_vc_position; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_device
    ADD CONSTRAINT dcim_device_unique_virtual_chassis_vc_position UNIQUE (virtual_chassis_id, vc_position);


--
-- Name: dcim_devicebay dcim_devicebay_installed_device_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_devicebay
    ADD CONSTRAINT dcim_devicebay_installed_device_id_key UNIQUE (installed_device_id);


--
-- Name: dcim_devicebay dcim_devicebay_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_devicebay
    ADD CONSTRAINT dcim_devicebay_pkey PRIMARY KEY (id);


--
-- Name: dcim_devicebay dcim_devicebay_unique_device_name; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_devicebay
    ADD CONSTRAINT dcim_devicebay_unique_device_name UNIQUE (device_id, name);


--
-- Name: dcim_devicebaytemplate dcim_devicebaytemplate_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_devicebaytemplate
    ADD CONSTRAINT dcim_devicebaytemplate_pkey PRIMARY KEY (id);


--
-- Name: dcim_devicebaytemplate dcim_devicebaytemplate_unique_device_type_name; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_devicebaytemplate
    ADD CONSTRAINT dcim_devicebaytemplate_unique_device_type_name UNIQUE (device_type_id, name);


--
-- Name: dcim_devicerole dcim_devicerole_parent_name; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_devicerole
    ADD CONSTRAINT dcim_devicerole_parent_name UNIQUE (parent_id, name);


--
-- Name: dcim_devicerole dcim_devicerole_parent_slug; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_devicerole
    ADD CONSTRAINT dcim_devicerole_parent_slug UNIQUE (parent_id, slug);


--
-- Name: dcim_devicerole dcim_devicerole_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_devicerole
    ADD CONSTRAINT dcim_devicerole_pkey PRIMARY KEY (id);


--
-- Name: dcim_devicetype dcim_devicetype_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_devicetype
    ADD CONSTRAINT dcim_devicetype_pkey PRIMARY KEY (id);


--
-- Name: dcim_devicetype dcim_devicetype_unique_manufacturer_model; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_devicetype
    ADD CONSTRAINT dcim_devicetype_unique_manufacturer_model UNIQUE (manufacturer_id, model);


--
-- Name: dcim_devicetype dcim_devicetype_unique_manufacturer_slug; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_devicetype
    ADD CONSTRAINT dcim_devicetype_unique_manufacturer_slug UNIQUE (manufacturer_id, slug);


--
-- Name: dcim_frontport dcim_frontport_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_frontport
    ADD CONSTRAINT dcim_frontport_pkey PRIMARY KEY (id);


--
-- Name: dcim_frontport dcim_frontport_unique_device_name; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_frontport
    ADD CONSTRAINT dcim_frontport_unique_device_name UNIQUE (device_id, name);


--
-- Name: dcim_frontporttemplate dcim_frontporttemplate_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_frontporttemplate
    ADD CONSTRAINT dcim_frontporttemplate_pkey PRIMARY KEY (id);


--
-- Name: dcim_frontporttemplate dcim_frontporttemplate_unique_device_type_name; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_frontporttemplate
    ADD CONSTRAINT dcim_frontporttemplate_unique_device_type_name UNIQUE (device_type_id, name);


--
-- Name: dcim_frontporttemplate dcim_frontporttemplate_unique_module_type_name; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_frontporttemplate
    ADD CONSTRAINT dcim_frontporttemplate_unique_module_type_name UNIQUE (module_type_id, name);


--
-- Name: dcim_interface dcim_interface_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_interface
    ADD CONSTRAINT dcim_interface_pkey PRIMARY KEY (id);


--
-- Name: dcim_interface dcim_interface_primary_mac_address_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_interface
    ADD CONSTRAINT dcim_interface_primary_mac_address_id_key UNIQUE (primary_mac_address_id);


--
-- Name: dcim_interface_tagged_vlans dcim_interface_tagged_vlans_interface_id_vlan_id_0d55c576_uniq; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_interface_tagged_vlans
    ADD CONSTRAINT dcim_interface_tagged_vlans_interface_id_vlan_id_0d55c576_uniq UNIQUE (interface_id, vlan_id);


--
-- Name: dcim_interface_tagged_vlans dcim_interface_tagged_vlans_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_interface_tagged_vlans
    ADD CONSTRAINT dcim_interface_tagged_vlans_pkey PRIMARY KEY (id);


--
-- Name: dcim_interface dcim_interface_unique_device_name; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_interface
    ADD CONSTRAINT dcim_interface_unique_device_name UNIQUE (device_id, name);


--
-- Name: dcim_interface_vdcs dcim_interface_vdcs_interface_id_virtualdevi_cca9c2a6_uniq; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_interface_vdcs
    ADD CONSTRAINT dcim_interface_vdcs_interface_id_virtualdevi_cca9c2a6_uniq UNIQUE (interface_id, virtualdevicecontext_id);


--
-- Name: dcim_interface_vdcs dcim_interface_vdcs_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_interface_vdcs
    ADD CONSTRAINT dcim_interface_vdcs_pkey PRIMARY KEY (id);


--
-- Name: dcim_interface_wirelesslans dcim_interface_wirelesslans_interface_id_wirelesslan_id_uniq; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_interface_wirelesslans
    ADD CONSTRAINT dcim_interface_wirelesslans_interface_id_wirelesslan_id_uniq UNIQUE (interface_id, wirelesslan_id);


--
-- Name: dcim_interface_wirelesslans dcim_interface_wirelesslans_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_interface_wirelesslans
    ADD CONSTRAINT dcim_interface_wirelesslans_pkey PRIMARY KEY (id);


--
-- Name: dcim_interfacetemplate dcim_interfacetemplate_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_interfacetemplate
    ADD CONSTRAINT dcim_interfacetemplate_pkey PRIMARY KEY (id);


--
-- Name: dcim_interfacetemplate dcim_interfacetemplate_unique_device_type_name; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_interfacetemplate
    ADD CONSTRAINT dcim_interfacetemplate_unique_device_type_name UNIQUE (device_type_id, name);


--
-- Name: dcim_interfacetemplate dcim_interfacetemplate_unique_module_type_name; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_interfacetemplate
    ADD CONSTRAINT dcim_interfacetemplate_unique_module_type_name UNIQUE (module_type_id, name);


--
-- Name: dcim_inventoryitem dcim_inventoryitem_asset_tag_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_inventoryitem
    ADD CONSTRAINT dcim_inventoryitem_asset_tag_key UNIQUE (asset_tag);


--
-- Name: dcim_inventoryitem dcim_inventoryitem_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_inventoryitem
    ADD CONSTRAINT dcim_inventoryitem_pkey PRIMARY KEY (id);


--
-- Name: dcim_inventoryitem dcim_inventoryitem_unique_device_parent_name; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_inventoryitem
    ADD CONSTRAINT dcim_inventoryitem_unique_device_parent_name UNIQUE (device_id, parent_id, name);


--
-- Name: dcim_inventoryitemrole dcim_inventoryitemrole_name_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_inventoryitemrole
    ADD CONSTRAINT dcim_inventoryitemrole_name_key UNIQUE (name);


--
-- Name: dcim_inventoryitemrole dcim_inventoryitemrole_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_inventoryitemrole
    ADD CONSTRAINT dcim_inventoryitemrole_pkey PRIMARY KEY (id);


--
-- Name: dcim_inventoryitemrole dcim_inventoryitemrole_slug_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_inventoryitemrole
    ADD CONSTRAINT dcim_inventoryitemrole_slug_key UNIQUE (slug);


--
-- Name: dcim_inventoryitemtemplate dcim_inventoryitemtemplate_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_inventoryitemtemplate
    ADD CONSTRAINT dcim_inventoryitemtemplate_pkey PRIMARY KEY (id);


--
-- Name: dcim_inventoryitemtemplate dcim_inventoryitemtemplate_unique_device_type_parent_name; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_inventoryitemtemplate
    ADD CONSTRAINT dcim_inventoryitemtemplate_unique_device_type_parent_name UNIQUE (device_type_id, parent_id, name);


--
-- Name: dcim_location dcim_location_parent_name; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_location
    ADD CONSTRAINT dcim_location_parent_name UNIQUE (site_id, parent_id, name);


--
-- Name: dcim_location dcim_location_parent_slug; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_location
    ADD CONSTRAINT dcim_location_parent_slug UNIQUE (site_id, parent_id, slug);


--
-- Name: dcim_location dcim_location_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_location
    ADD CONSTRAINT dcim_location_pkey PRIMARY KEY (id);


--
-- Name: dcim_macaddress dcim_macaddress_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_macaddress
    ADD CONSTRAINT dcim_macaddress_pkey PRIMARY KEY (id);


--
-- Name: dcim_manufacturer dcim_manufacturer_name_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_manufacturer
    ADD CONSTRAINT dcim_manufacturer_name_key UNIQUE (name);


--
-- Name: dcim_manufacturer dcim_manufacturer_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_manufacturer
    ADD CONSTRAINT dcim_manufacturer_pkey PRIMARY KEY (id);


--
-- Name: dcim_manufacturer dcim_manufacturer_slug_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_manufacturer
    ADD CONSTRAINT dcim_manufacturer_slug_key UNIQUE (slug);


--
-- Name: dcim_module dcim_module_asset_tag_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_module
    ADD CONSTRAINT dcim_module_asset_tag_key UNIQUE (asset_tag);


--
-- Name: dcim_module dcim_module_module_bay_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_module
    ADD CONSTRAINT dcim_module_module_bay_id_key UNIQUE (module_bay_id);


--
-- Name: dcim_module dcim_module_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_module
    ADD CONSTRAINT dcim_module_pkey PRIMARY KEY (id);


--
-- Name: dcim_modulebay dcim_modulebay_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_modulebay
    ADD CONSTRAINT dcim_modulebay_pkey PRIMARY KEY (id);


--
-- Name: dcim_modulebay dcim_modulebay_unique_device_module_name; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_modulebay
    ADD CONSTRAINT dcim_modulebay_unique_device_module_name UNIQUE (device_id, module_id, name);


--
-- Name: dcim_modulebaytemplate dcim_modulebaytemplate_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_modulebaytemplate
    ADD CONSTRAINT dcim_modulebaytemplate_pkey PRIMARY KEY (id);


--
-- Name: dcim_modulebaytemplate dcim_modulebaytemplate_unique_device_type_name; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_modulebaytemplate
    ADD CONSTRAINT dcim_modulebaytemplate_unique_device_type_name UNIQUE (device_type_id, name);


--
-- Name: dcim_modulebaytemplate dcim_modulebaytemplate_unique_module_type_name; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_modulebaytemplate
    ADD CONSTRAINT dcim_modulebaytemplate_unique_module_type_name UNIQUE (module_type_id, name);


--
-- Name: dcim_modulebaytype dcim_modulebaytype_manufacturer_name_uniq; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_modulebaytype
    ADD CONSTRAINT dcim_modulebaytype_manufacturer_name_uniq UNIQUE NULLS NOT DISTINCT (manufacturer_id, name);


--
-- Name: dcim_modulebaytype dcim_modulebaytype_manufacturer_slug_uniq; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_modulebaytype
    ADD CONSTRAINT dcim_modulebaytype_manufacturer_slug_uniq UNIQUE NULLS NOT DISTINCT (manufacturer_id, slug);


--
-- Name: dcim_modulebaytype dcim_modulebaytype_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_modulebaytype
    ADD CONSTRAINT dcim_modulebaytype_pkey PRIMARY KEY (id);


--
-- Name: dcim_moduletype dcim_moduletype_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_moduletype
    ADD CONSTRAINT dcim_moduletype_pkey PRIMARY KEY (id);


--
-- Name: dcim_moduletype dcim_moduletype_unique_manufacturer_model; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_moduletype
    ADD CONSTRAINT dcim_moduletype_unique_manufacturer_model UNIQUE (manufacturer_id, model);


--
-- Name: dcim_moduletypeprofile dcim_moduletypeprofile_name_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_moduletypeprofile
    ADD CONSTRAINT dcim_moduletypeprofile_name_key UNIQUE (name);


--
-- Name: dcim_moduletypeprofile dcim_moduletypeprofile_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_moduletypeprofile
    ADD CONSTRAINT dcim_moduletypeprofile_pkey PRIMARY KEY (id);


--
-- Name: dcim_platform dcim_platform_manufacturer_name; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_platform
    ADD CONSTRAINT dcim_platform_manufacturer_name UNIQUE (manufacturer_id, name);


--
-- Name: dcim_platform dcim_platform_manufacturer_slug; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_platform
    ADD CONSTRAINT dcim_platform_manufacturer_slug UNIQUE (manufacturer_id, slug);


--
-- Name: dcim_platform dcim_platform_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_platform
    ADD CONSTRAINT dcim_platform_pkey PRIMARY KEY (id);


--
-- Name: dcim_portmapping dcim_portmapping_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_portmapping
    ADD CONSTRAINT dcim_portmapping_pkey PRIMARY KEY (id);


--
-- Name: dcim_portmapping dcim_portmapping_unique_front_port_position; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_portmapping
    ADD CONSTRAINT dcim_portmapping_unique_front_port_position UNIQUE (front_port_id, front_port_position);


--
-- Name: dcim_portmapping dcim_portmapping_unique_rear_port_position; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_portmapping
    ADD CONSTRAINT dcim_portmapping_unique_rear_port_position UNIQUE (rear_port_id, rear_port_position);


--
-- Name: dcim_porttemplatemapping dcim_porttemplatemapping_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_porttemplatemapping
    ADD CONSTRAINT dcim_porttemplatemapping_pkey PRIMARY KEY (id);


--
-- Name: dcim_porttemplatemapping dcim_porttemplatemapping_unique_front_port_position; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_porttemplatemapping
    ADD CONSTRAINT dcim_porttemplatemapping_unique_front_port_position UNIQUE (front_port_id, front_port_position);


--
-- Name: dcim_porttemplatemapping dcim_porttemplatemapping_unique_rear_port_position; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_porttemplatemapping
    ADD CONSTRAINT dcim_porttemplatemapping_unique_rear_port_position UNIQUE (rear_port_id, rear_port_position);


--
-- Name: dcim_powerfeed dcim_powerfeed_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_powerfeed
    ADD CONSTRAINT dcim_powerfeed_pkey PRIMARY KEY (id);


--
-- Name: dcim_powerfeed dcim_powerfeed_unique_power_panel_name; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_powerfeed
    ADD CONSTRAINT dcim_powerfeed_unique_power_panel_name UNIQUE (power_panel_id, name);


--
-- Name: dcim_poweroutlet dcim_poweroutlet_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_poweroutlet
    ADD CONSTRAINT dcim_poweroutlet_pkey PRIMARY KEY (id);


--
-- Name: dcim_poweroutlet dcim_poweroutlet_unique_device_name; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_poweroutlet
    ADD CONSTRAINT dcim_poweroutlet_unique_device_name UNIQUE (device_id, name);


--
-- Name: dcim_poweroutlettemplate dcim_poweroutlettemplate_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_poweroutlettemplate
    ADD CONSTRAINT dcim_poweroutlettemplate_pkey PRIMARY KEY (id);


--
-- Name: dcim_poweroutlettemplate dcim_poweroutlettemplate_unique_device_type_name; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_poweroutlettemplate
    ADD CONSTRAINT dcim_poweroutlettemplate_unique_device_type_name UNIQUE (device_type_id, name);


--
-- Name: dcim_poweroutlettemplate dcim_poweroutlettemplate_unique_module_type_name; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_poweroutlettemplate
    ADD CONSTRAINT dcim_poweroutlettemplate_unique_module_type_name UNIQUE (module_type_id, name);


--
-- Name: dcim_powerpanel dcim_powerpanel_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_powerpanel
    ADD CONSTRAINT dcim_powerpanel_pkey PRIMARY KEY (id);


--
-- Name: dcim_powerpanel dcim_powerpanel_unique_site_name; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_powerpanel
    ADD CONSTRAINT dcim_powerpanel_unique_site_name UNIQUE (site_id, name);


--
-- Name: dcim_powerport dcim_powerport_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_powerport
    ADD CONSTRAINT dcim_powerport_pkey PRIMARY KEY (id);


--
-- Name: dcim_powerport dcim_powerport_unique_device_name; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_powerport
    ADD CONSTRAINT dcim_powerport_unique_device_name UNIQUE (device_id, name);


--
-- Name: dcim_powerporttemplate dcim_powerporttemplate_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_powerporttemplate
    ADD CONSTRAINT dcim_powerporttemplate_pkey PRIMARY KEY (id);


--
-- Name: dcim_powerporttemplate dcim_powerporttemplate_unique_device_type_name; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_powerporttemplate
    ADD CONSTRAINT dcim_powerporttemplate_unique_device_type_name UNIQUE (device_type_id, name);


--
-- Name: dcim_powerporttemplate dcim_powerporttemplate_unique_module_type_name; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_powerporttemplate
    ADD CONSTRAINT dcim_powerporttemplate_unique_module_type_name UNIQUE (module_type_id, name);


--
-- Name: dcim_rack dcim_rack_asset_tag_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_rack
    ADD CONSTRAINT dcim_rack_asset_tag_key UNIQUE (asset_tag);


--
-- Name: dcim_rack dcim_rack_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_rack
    ADD CONSTRAINT dcim_rack_pkey PRIMARY KEY (id);


--
-- Name: dcim_rack dcim_rack_unique_location_facility_id; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_rack
    ADD CONSTRAINT dcim_rack_unique_location_facility_id UNIQUE (location_id, facility_id);


--
-- Name: dcim_rack dcim_rack_unique_location_name; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_rack
    ADD CONSTRAINT dcim_rack_unique_location_name UNIQUE (location_id, name);


--
-- Name: dcim_rackgroup dcim_rackgroup_name_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_rackgroup
    ADD CONSTRAINT dcim_rackgroup_name_key UNIQUE (name);


--
-- Name: dcim_rackgroup dcim_rackgroup_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_rackgroup
    ADD CONSTRAINT dcim_rackgroup_pkey PRIMARY KEY (id);


--
-- Name: dcim_rackgroup dcim_rackgroup_slug_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_rackgroup
    ADD CONSTRAINT dcim_rackgroup_slug_key UNIQUE (slug);


--
-- Name: dcim_rackreservation dcim_rackreservation_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_rackreservation
    ADD CONSTRAINT dcim_rackreservation_pkey PRIMARY KEY (id);


--
-- Name: dcim_rackrole dcim_rackrole_name_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_rackrole
    ADD CONSTRAINT dcim_rackrole_name_key UNIQUE (name);


--
-- Name: dcim_rackrole dcim_rackrole_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_rackrole
    ADD CONSTRAINT dcim_rackrole_pkey PRIMARY KEY (id);


--
-- Name: dcim_rackrole dcim_rackrole_slug_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_rackrole
    ADD CONSTRAINT dcim_rackrole_slug_key UNIQUE (slug);


--
-- Name: dcim_racktype dcim_racktype_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_racktype
    ADD CONSTRAINT dcim_racktype_pkey PRIMARY KEY (id);


--
-- Name: dcim_racktype dcim_racktype_slug_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_racktype
    ADD CONSTRAINT dcim_racktype_slug_key UNIQUE (slug);


--
-- Name: dcim_racktype dcim_racktype_unique_manufacturer_model; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_racktype
    ADD CONSTRAINT dcim_racktype_unique_manufacturer_model UNIQUE (manufacturer_id, model);


--
-- Name: dcim_racktype dcim_racktype_unique_manufacturer_slug; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_racktype
    ADD CONSTRAINT dcim_racktype_unique_manufacturer_slug UNIQUE (manufacturer_id, slug);


--
-- Name: dcim_rearport dcim_rearport_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_rearport
    ADD CONSTRAINT dcim_rearport_pkey PRIMARY KEY (id);


--
-- Name: dcim_rearport dcim_rearport_unique_device_name; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_rearport
    ADD CONSTRAINT dcim_rearport_unique_device_name UNIQUE (device_id, name);


--
-- Name: dcim_rearporttemplate dcim_rearporttemplate_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_rearporttemplate
    ADD CONSTRAINT dcim_rearporttemplate_pkey PRIMARY KEY (id);


--
-- Name: dcim_rearporttemplate dcim_rearporttemplate_unique_device_type_name; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_rearporttemplate
    ADD CONSTRAINT dcim_rearporttemplate_unique_device_type_name UNIQUE (device_type_id, name);


--
-- Name: dcim_rearporttemplate dcim_rearporttemplate_unique_module_type_name; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_rearporttemplate
    ADD CONSTRAINT dcim_rearporttemplate_unique_module_type_name UNIQUE (module_type_id, name);


--
-- Name: dcim_region dcim_region_parent_name; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_region
    ADD CONSTRAINT dcim_region_parent_name UNIQUE (parent_id, name);


--
-- Name: dcim_region dcim_region_parent_slug; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_region
    ADD CONSTRAINT dcim_region_parent_slug UNIQUE (parent_id, slug);


--
-- Name: dcim_region dcim_region_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_region
    ADD CONSTRAINT dcim_region_pkey PRIMARY KEY (id);


--
-- Name: dcim_site_asns dcim_site_asns_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_site_asns
    ADD CONSTRAINT dcim_site_asns_pkey PRIMARY KEY (id);


--
-- Name: dcim_site_asns dcim_site_asns_site_id_asn_id_1a5a6f23_uniq; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_site_asns
    ADD CONSTRAINT dcim_site_asns_site_id_asn_id_1a5a6f23_uniq UNIQUE (site_id, asn_id);


--
-- Name: dcim_site dcim_site_name_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_site
    ADD CONSTRAINT dcim_site_name_key UNIQUE (name);


--
-- Name: dcim_site dcim_site_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_site
    ADD CONSTRAINT dcim_site_pkey PRIMARY KEY (id);


--
-- Name: dcim_site dcim_site_slug_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_site
    ADD CONSTRAINT dcim_site_slug_key UNIQUE (slug);


--
-- Name: dcim_sitegroup dcim_sitegroup_parent_name; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_sitegroup
    ADD CONSTRAINT dcim_sitegroup_parent_name UNIQUE (parent_id, name);


--
-- Name: dcim_sitegroup dcim_sitegroup_parent_slug; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_sitegroup
    ADD CONSTRAINT dcim_sitegroup_parent_slug UNIQUE (parent_id, slug);


--
-- Name: dcim_sitegroup dcim_sitegroup_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_sitegroup
    ADD CONSTRAINT dcim_sitegroup_pkey PRIMARY KEY (id);


--
-- Name: dcim_virtualchassis dcim_virtualchassis_master_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_virtualchassis
    ADD CONSTRAINT dcim_virtualchassis_master_id_key UNIQUE (master_id);


--
-- Name: dcim_virtualchassis dcim_virtualchassis_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_virtualchassis
    ADD CONSTRAINT dcim_virtualchassis_pkey PRIMARY KEY (id);


--
-- Name: dcim_virtualdevicecontext dcim_virtualdevicecontext_device_identifier; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_virtualdevicecontext
    ADD CONSTRAINT dcim_virtualdevicecontext_device_identifier UNIQUE (device_id, identifier);


--
-- Name: dcim_virtualdevicecontext dcim_virtualdevicecontext_device_name; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_virtualdevicecontext
    ADD CONSTRAINT dcim_virtualdevicecontext_device_name UNIQUE (device_id, name);


--
-- Name: dcim_virtualdevicecontext dcim_virtualdevicecontext_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_virtualdevicecontext
    ADD CONSTRAINT dcim_virtualdevicecontext_pkey PRIMARY KEY (id);


--
-- Name: dcim_virtualdevicecontext dcim_virtualdevicecontext_primary_ip4_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_virtualdevicecontext
    ADD CONSTRAINT dcim_virtualdevicecontext_primary_ip4_id_key UNIQUE (primary_ip4_id);


--
-- Name: dcim_virtualdevicecontext dcim_virtualdevicecontext_primary_ip6_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_virtualdevicecontext
    ADD CONSTRAINT dcim_virtualdevicecontext_primary_ip6_id_key UNIQUE (primary_ip6_id);


--
-- Name: entity_mappings entity_mappings_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.entity_mappings
    ADD CONSTRAINT entity_mappings_pkey PRIMARY KEY (id);


--
-- Name: field_mappings field_mappings_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.field_mappings
    ADD CONSTRAINT field_mappings_pkey PRIMARY KEY (id);


--
-- Name: identity_group_membership identity_group_membership_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.identity_group_membership
    ADD CONSTRAINT identity_group_membership_pkey PRIMARY KEY (id);


--
-- Name: identity_group_membership identity_group_membership_user_group_source_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.identity_group_membership
    ADD CONSTRAINT identity_group_membership_user_group_source_key UNIQUE (user_id, group_id, source);


--
-- Name: identity_group identity_group_name_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.identity_group
    ADD CONSTRAINT identity_group_name_key UNIQUE (name);


--
-- Name: identity_group identity_group_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.identity_group
    ADD CONSTRAINT identity_group_pkey PRIMARY KEY (id);


--
-- Name: identity_user identity_user_email_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.identity_user
    ADD CONSTRAINT identity_user_email_key UNIQUE (email);


--
-- Name: identity_user identity_user_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.identity_user
    ADD CONSTRAINT identity_user_pkey PRIMARY KEY (id);


--
-- Name: identity_user identity_user_username_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.identity_user
    ADD CONSTRAINT identity_user_username_key UNIQUE (username);


--
-- Name: instance_branding instance_branding_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.instance_branding
    ADD CONSTRAINT instance_branding_pkey PRIMARY KEY (id);


--
-- Name: inventory_content_type inventory_content_type_app_model_uniq; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_content_type
    ADD CONSTRAINT inventory_content_type_app_model_uniq UNIQUE (app_label, model);


--
-- Name: inventory_content_type inventory_content_type_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_content_type
    ADD CONSTRAINT inventory_content_type_pkey PRIMARY KEY (id);


--
-- Name: inventory_dataflow inventory_dataflow_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_dataflow
    ADD CONSTRAINT inventory_dataflow_pkey PRIMARY KEY (id);


--
-- Name: inventory_dataflow_selected_ip inventory_dataflow_selected_ip_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_dataflow_selected_ip
    ADD CONSTRAINT inventory_dataflow_selected_ip_pkey PRIMARY KEY (dataflow_id, side, ipaddress_id);


--
-- Name: inventory_layout_system_profile inventory_layout_system_profile_entity_type_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_layout_system_profile
    ADD CONSTRAINT inventory_layout_system_profile_entity_type_key UNIQUE (entity_type);


--
-- Name: inventory_layout_system_profile inventory_layout_system_profile_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_layout_system_profile
    ADD CONSTRAINT inventory_layout_system_profile_pkey PRIMARY KEY (id);


--
-- Name: inventory_layout_user_profile inventory_layout_user_profile_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_layout_user_profile
    ADD CONSTRAINT inventory_layout_user_profile_pkey PRIMARY KEY (id);


--
-- Name: inventory_layout_user_profile inventory_layout_user_profile_user_entity_uniq; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_layout_user_profile
    ADD CONSTRAINT inventory_layout_user_profile_user_entity_uniq UNIQUE (user_id, entity_type);


--
-- Name: inventory_tag inventory_tag_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_tag
    ADD CONSTRAINT inventory_tag_pkey PRIMARY KEY (id);


--
-- Name: inventory_tag inventory_tag_slug_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_tag
    ADD CONSTRAINT inventory_tag_slug_key UNIQUE (slug);


--
-- Name: inventory_template inventory_template_name_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_template
    ADD CONSTRAINT inventory_template_name_key UNIQUE (name);


--
-- Name: inventory_template inventory_template_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_template
    ADD CONSTRAINT inventory_template_pkey PRIMARY KEY (id);


--
-- Name: inventory_template_run inventory_template_run_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_template_run
    ADD CONSTRAINT inventory_template_run_pkey PRIMARY KEY (id);


--
-- Name: inventory_template_run_step_result inventory_template_run_step_result_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_template_run_step_result
    ADD CONSTRAINT inventory_template_run_step_result_pkey PRIMARY KEY (id);


--
-- Name: inventory_template_run_step_result inventory_template_run_step_result_run_step_key_uniq; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_template_run_step_result
    ADD CONSTRAINT inventory_template_run_step_result_run_step_key_uniq UNIQUE (run_id, step_key);


--
-- Name: inventory_template inventory_template_slug_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_template
    ADD CONSTRAINT inventory_template_slug_key UNIQUE (slug);


--
-- Name: inventory_template_step inventory_template_step_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_template_step
    ADD CONSTRAINT inventory_template_step_pkey PRIMARY KEY (id);


--
-- Name: inventory_template_step inventory_template_step_template_step_key_uniq; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_template_step
    ADD CONSTRAINT inventory_template_step_template_step_key_uniq UNIQUE (template_id, step_key);


--
-- Name: inventory_template_variable inventory_template_variable_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_template_variable
    ADD CONSTRAINT inventory_template_variable_pkey PRIMARY KEY (id);


--
-- Name: inventory_template_variable inventory_template_variable_template_key_uniq; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_template_variable
    ADD CONSTRAINT inventory_template_variable_template_key_uniq UNIQUE (template_id, key);


--
-- Name: ipam_aggregate ipam_aggregate_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_aggregate
    ADD CONSTRAINT ipam_aggregate_pkey PRIMARY KEY (id);


--
-- Name: ipam_asn ipam_asn_asn_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_asn
    ADD CONSTRAINT ipam_asn_asn_key UNIQUE (asn);


--
-- Name: ipam_asn ipam_asn_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_asn
    ADD CONSTRAINT ipam_asn_pkey PRIMARY KEY (id);


--
-- Name: ipam_asnrange ipam_asnrange_name_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_asnrange
    ADD CONSTRAINT ipam_asnrange_name_key UNIQUE (name);


--
-- Name: ipam_asnrange ipam_asnrange_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_asnrange
    ADD CONSTRAINT ipam_asnrange_pkey PRIMARY KEY (id);


--
-- Name: ipam_asnrange ipam_asnrange_slug_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_asnrange
    ADD CONSTRAINT ipam_asnrange_slug_key UNIQUE (slug);


--
-- Name: ipam_fhrpgroup ipam_fhrpgroup_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_fhrpgroup
    ADD CONSTRAINT ipam_fhrpgroup_pkey PRIMARY KEY (id);


--
-- Name: ipam_fhrpgroupassignment ipam_fhrpgroupassignment_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_fhrpgroupassignment
    ADD CONSTRAINT ipam_fhrpgroupassignment_pkey PRIMARY KEY (id);


--
-- Name: ipam_fhrpgroupassignment ipam_fhrpgroupassignment_unique_interface_group; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_fhrpgroupassignment
    ADD CONSTRAINT ipam_fhrpgroupassignment_unique_interface_group UNIQUE (interface_type_id, interface_id, group_id);


--
-- Name: ipam_ipaddress ipam_ipaddress_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_ipaddress
    ADD CONSTRAINT ipam_ipaddress_pkey PRIMARY KEY (id);


--
-- Name: ipam_iprange ipam_iprange_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_iprange
    ADD CONSTRAINT ipam_iprange_pkey PRIMARY KEY (id);


--
-- Name: vpn_l2vpn_export_targets ipam_l2vpn_export_targets_l2vpn_id_routetarget_id_eea90661_uniq; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_l2vpn_export_targets
    ADD CONSTRAINT ipam_l2vpn_export_targets_l2vpn_id_routetarget_id_eea90661_uniq UNIQUE (l2vpn_id, routetarget_id);


--
-- Name: vpn_l2vpn_export_targets ipam_l2vpn_export_targets_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_l2vpn_export_targets
    ADD CONSTRAINT ipam_l2vpn_export_targets_pkey PRIMARY KEY (id);


--
-- Name: vpn_l2vpn_import_targets ipam_l2vpn_import_targets_l2vpn_id_routetarget_id_96af344c_uniq; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_l2vpn_import_targets
    ADD CONSTRAINT ipam_l2vpn_import_targets_l2vpn_id_routetarget_id_96af344c_uniq UNIQUE (l2vpn_id, routetarget_id);


--
-- Name: vpn_l2vpn_import_targets ipam_l2vpn_import_targets_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_l2vpn_import_targets
    ADD CONSTRAINT ipam_l2vpn_import_targets_pkey PRIMARY KEY (id);


--
-- Name: ipam_prefix ipam_prefix_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_prefix
    ADD CONSTRAINT ipam_prefix_pkey PRIMARY KEY (id);


--
-- Name: ipam_rir ipam_rir_name_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_rir
    ADD CONSTRAINT ipam_rir_name_key UNIQUE (name);


--
-- Name: ipam_rir ipam_rir_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_rir
    ADD CONSTRAINT ipam_rir_pkey PRIMARY KEY (id);


--
-- Name: ipam_rir ipam_rir_slug_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_rir
    ADD CONSTRAINT ipam_rir_slug_key UNIQUE (slug);


--
-- Name: ipam_role ipam_role_name_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_role
    ADD CONSTRAINT ipam_role_name_key UNIQUE (name);


--
-- Name: ipam_role ipam_role_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_role
    ADD CONSTRAINT ipam_role_pkey PRIMARY KEY (id);


--
-- Name: ipam_role ipam_role_slug_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_role
    ADD CONSTRAINT ipam_role_slug_key UNIQUE (slug);


--
-- Name: ipam_routetarget ipam_routetarget_name_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_routetarget
    ADD CONSTRAINT ipam_routetarget_name_key UNIQUE (name);


--
-- Name: ipam_routetarget ipam_routetarget_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_routetarget
    ADD CONSTRAINT ipam_routetarget_pkey PRIMARY KEY (id);


--
-- Name: ipam_service_ipaddresses ipam_service_ipaddresses_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_service_ipaddresses
    ADD CONSTRAINT ipam_service_ipaddresses_pkey PRIMARY KEY (id);


--
-- Name: ipam_service_ipaddresses ipam_service_ipaddresses_service_id_ipaddress_id_d019a805_uniq; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_service_ipaddresses
    ADD CONSTRAINT ipam_service_ipaddresses_service_id_ipaddress_id_d019a805_uniq UNIQUE (service_id, ipaddress_id);


--
-- Name: ipam_service ipam_service_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_service
    ADD CONSTRAINT ipam_service_pkey PRIMARY KEY (id);


--
-- Name: ipam_service_port_mapping ipam_service_port_mapping_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_service_port_mapping
    ADD CONSTRAINT ipam_service_port_mapping_pkey PRIMARY KEY (id);


--
-- Name: ipam_servicetemplate ipam_servicetemplate_name_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_servicetemplate
    ADD CONSTRAINT ipam_servicetemplate_name_key UNIQUE (name);


--
-- Name: ipam_servicetemplate ipam_servicetemplate_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_servicetemplate
    ADD CONSTRAINT ipam_servicetemplate_pkey PRIMARY KEY (id);


--
-- Name: ipam_vlan ipam_vlan_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_vlan
    ADD CONSTRAINT ipam_vlan_pkey PRIMARY KEY (id);


--
-- Name: ipam_vlan ipam_vlan_unique_group_name; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_vlan
    ADD CONSTRAINT ipam_vlan_unique_group_name UNIQUE (group_id, name);


--
-- Name: ipam_vlan ipam_vlan_unique_group_vid; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_vlan
    ADD CONSTRAINT ipam_vlan_unique_group_vid UNIQUE (group_id, vid);


--
-- Name: ipam_vlan ipam_vlan_unique_qinq_svlan_name; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_vlan
    ADD CONSTRAINT ipam_vlan_unique_qinq_svlan_name UNIQUE (qinq_svlan_id, name);


--
-- Name: ipam_vlan ipam_vlan_unique_qinq_svlan_vid; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_vlan
    ADD CONSTRAINT ipam_vlan_unique_qinq_svlan_vid UNIQUE (qinq_svlan_id, vid);


--
-- Name: ipam_vlangroup ipam_vlangroup_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_vlangroup
    ADD CONSTRAINT ipam_vlangroup_pkey PRIMARY KEY (id);


--
-- Name: ipam_vlangroup ipam_vlangroup_unique_scope_name; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_vlangroup
    ADD CONSTRAINT ipam_vlangroup_unique_scope_name UNIQUE (scope_type_id, scope_id, name);


--
-- Name: ipam_vlangroup ipam_vlangroup_unique_scope_slug; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_vlangroup
    ADD CONSTRAINT ipam_vlangroup_unique_scope_slug UNIQUE (scope_type_id, scope_id, slug);


--
-- Name: ipam_vlantranslationpolicy ipam_vlantranslationpolicy_name_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_vlantranslationpolicy
    ADD CONSTRAINT ipam_vlantranslationpolicy_name_key UNIQUE (name);


--
-- Name: ipam_vlantranslationpolicy ipam_vlantranslationpolicy_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_vlantranslationpolicy
    ADD CONSTRAINT ipam_vlantranslationpolicy_pkey PRIMARY KEY (id);


--
-- Name: ipam_vlantranslationrule ipam_vlantranslationrule_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_vlantranslationrule
    ADD CONSTRAINT ipam_vlantranslationrule_pkey PRIMARY KEY (id);


--
-- Name: ipam_vlantranslationrule ipam_vlantranslationrule_unique_policy_local_vid; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_vlantranslationrule
    ADD CONSTRAINT ipam_vlantranslationrule_unique_policy_local_vid UNIQUE (policy_id, local_vid);


--
-- Name: ipam_vlantranslationrule ipam_vlantranslationrule_unique_policy_remote_vid; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_vlantranslationrule
    ADD CONSTRAINT ipam_vlantranslationrule_unique_policy_remote_vid UNIQUE (policy_id, remote_vid);


--
-- Name: ipam_vrf_export_targets ipam_vrf_export_targets_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_vrf_export_targets
    ADD CONSTRAINT ipam_vrf_export_targets_pkey PRIMARY KEY (id);


--
-- Name: ipam_vrf_export_targets ipam_vrf_export_targets_vrf_id_routetarget_id_63ba8c62_uniq; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_vrf_export_targets
    ADD CONSTRAINT ipam_vrf_export_targets_vrf_id_routetarget_id_63ba8c62_uniq UNIQUE (vrf_id, routetarget_id);


--
-- Name: ipam_vrf_import_targets ipam_vrf_import_targets_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_vrf_import_targets
    ADD CONSTRAINT ipam_vrf_import_targets_pkey PRIMARY KEY (id);


--
-- Name: ipam_vrf_import_targets ipam_vrf_import_targets_vrf_id_routetarget_id_399b155f_uniq; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_vrf_import_targets
    ADD CONSTRAINT ipam_vrf_import_targets_vrf_id_routetarget_id_399b155f_uniq UNIQUE (vrf_id, routetarget_id);


--
-- Name: ipam_vrf ipam_vrf_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_vrf
    ADD CONSTRAINT ipam_vrf_pkey PRIMARY KEY (id);


--
-- Name: ipam_vrf ipam_vrf_rd_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_vrf
    ADD CONSTRAINT ipam_vrf_rd_key UNIQUE (rd);


--
-- Name: license_instance_identity license_instance_identity_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.license_instance_identity
    ADD CONSTRAINT license_instance_identity_pkey PRIMARY KEY (id);


--
-- Name: license_seat_assignment license_seat_assignment_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.license_seat_assignment
    ADD CONSTRAINT license_seat_assignment_pkey PRIMARY KEY (id);


--
-- Name: license_seat_ledger license_seat_ledger_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.license_seat_ledger
    ADD CONSTRAINT license_seat_ledger_pkey PRIMARY KEY (id);


--
-- Name: license_state license_state_instance_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.license_state
    ADD CONSTRAINT license_state_instance_id_key UNIQUE (instance_id);


--
-- Name: license_state license_state_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.license_state
    ADD CONSTRAINT license_state_pkey PRIMARY KEY (id);


--
-- Name: mapping_configurations mapping_configurations_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.mapping_configurations
    ADD CONSTRAINT mapping_configurations_pkey PRIMARY KEY (logical_id, version_id);


--
-- Name: monitoring_alert_source_assignment monitoring_alert_source_assignment_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.monitoring_alert_source_assignment
    ADD CONSTRAINT monitoring_alert_source_assignment_pkey PRIMARY KEY (id);


--
-- Name: monitoring_alert_source monitoring_alert_source_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.monitoring_alert_source
    ADD CONSTRAINT monitoring_alert_source_pkey PRIMARY KEY (id);


--
-- Name: monitoring_exporter_assignment monitoring_exporter_assignment_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.monitoring_exporter_assignment
    ADD CONSTRAINT monitoring_exporter_assignment_pkey PRIMARY KEY (id);


--
-- Name: monitoring_exporter monitoring_exporter_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.monitoring_exporter
    ADD CONSTRAINT monitoring_exporter_pkey PRIMARY KEY (id);


--
-- Name: monitoring_exporter monitoring_exporter_slug_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.monitoring_exporter
    ADD CONSTRAINT monitoring_exporter_slug_key UNIQUE (slug);


--
-- Name: monitoring_metric_query monitoring_metric_query_exporter_slug_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.monitoring_metric_query
    ADD CONSTRAINT monitoring_metric_query_exporter_slug_key UNIQUE (exporter_id, slug);


--
-- Name: monitoring_metric_query_host_override monitoring_metric_query_host_override_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.monitoring_metric_query_host_override
    ADD CONSTRAINT monitoring_metric_query_host_override_pkey PRIMARY KEY (id);


--
-- Name: monitoring_metric_query_host_override monitoring_metric_query_host_override_query_host_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.monitoring_metric_query_host_override
    ADD CONSTRAINT monitoring_metric_query_host_override_query_host_key UNIQUE (metric_query_id, host_id);


--
-- Name: monitoring_metric_query monitoring_metric_query_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.monitoring_metric_query
    ADD CONSTRAINT monitoring_metric_query_pkey PRIMARY KEY (id);


--
-- Name: monitoring_metric_set_assignment monitoring_metric_set_assignment_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.monitoring_metric_set_assignment
    ADD CONSTRAINT monitoring_metric_set_assignment_pkey PRIMARY KEY (id);


--
-- Name: monitoring_metric_set_item monitoring_metric_set_item_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.monitoring_metric_set_item
    ADD CONSTRAINT monitoring_metric_set_item_pkey PRIMARY KEY (id);


--
-- Name: monitoring_metric_set_item monitoring_metric_set_item_set_query_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.monitoring_metric_set_item
    ADD CONSTRAINT monitoring_metric_set_item_set_query_key UNIQUE (metric_set_id, metric_query_id);


--
-- Name: monitoring_metric_set monitoring_metric_set_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.monitoring_metric_set
    ADD CONSTRAINT monitoring_metric_set_pkey PRIMARY KEY (id);


--
-- Name: monitoring_metric_set monitoring_metric_set_slug_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.monitoring_metric_set
    ADD CONSTRAINT monitoring_metric_set_slug_key UNIQUE (slug);


--
-- Name: monitoring_query_source_assignment monitoring_query_source_assignment_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.monitoring_query_source_assignment
    ADD CONSTRAINT monitoring_query_source_assignment_pkey PRIMARY KEY (id);


--
-- Name: monitoring_query_source monitoring_query_source_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.monitoring_query_source
    ADD CONSTRAINT monitoring_query_source_pkey PRIMARY KEY (id);


--
-- Name: monitoring_query_source monitoring_query_source_slug_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.monitoring_query_source
    ADD CONSTRAINT monitoring_query_source_slug_key UNIQUE (slug);


--
-- Name: monitoring_sd_label_settings monitoring_sd_label_settings_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.monitoring_sd_label_settings
    ADD CONSTRAINT monitoring_sd_label_settings_pkey PRIMARY KEY (id);


--
-- Name: playbook_artifact playbook_artifact_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_artifact
    ADD CONSTRAINT playbook_artifact_pkey PRIMARY KEY (id);


--
-- Name: playbook_assignment playbook_assignment_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_assignment
    ADD CONSTRAINT playbook_assignment_pkey PRIMARY KEY (id);


--
-- Name: playbook_execution_environment playbook_execution_environment_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_execution_environment
    ADD CONSTRAINT playbook_execution_environment_pkey PRIMARY KEY (id);


--
-- Name: playbook_job_template playbook_job_template_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_job_template
    ADD CONSTRAINT playbook_job_template_pkey PRIMARY KEY (id);


--
-- Name: playbook_job_template_survey playbook_job_template_survey_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_job_template_survey
    ADD CONSTRAINT playbook_job_template_survey_pkey PRIMARY KEY (id);


--
-- Name: playbook_job_token playbook_job_token_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_job_token
    ADD CONSTRAINT playbook_job_token_pkey PRIMARY KEY (id);


--
-- Name: playbook_job_token playbook_job_token_token_hash_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_job_token
    ADD CONSTRAINT playbook_job_token_token_hash_key UNIQUE (token_hash);


--
-- Name: playbook playbook_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook
    ADD CONSTRAINT playbook_pkey PRIMARY KEY (id);


--
-- Name: playbook_project playbook_project_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_project
    ADD CONSTRAINT playbook_project_pkey PRIMARY KEY (id);


--
-- Name: playbook_run_event playbook_run_event_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_run_event
    ADD CONSTRAINT playbook_run_event_pkey PRIMARY KEY (id);


--
-- Name: playbook_run_group_callback_token playbook_run_group_callback_token_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_run_group_callback_token
    ADD CONSTRAINT playbook_run_group_callback_token_pkey PRIMARY KEY (id);


--
-- Name: playbook_run_group_callback_token playbook_run_group_callback_token_run_group_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_run_group_callback_token
    ADD CONSTRAINT playbook_run_group_callback_token_run_group_id_key UNIQUE (run_group_id);


--
-- Name: playbook_run_group_callback_token playbook_run_group_callback_token_token_hash_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_run_group_callback_token
    ADD CONSTRAINT playbook_run_group_callback_token_token_hash_key UNIQUE (token_hash);


--
-- Name: playbook_run_group_event_batch playbook_run_group_event_batch_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_run_group_event_batch
    ADD CONSTRAINT playbook_run_group_event_batch_pkey PRIMARY KEY (run_group_id, batch_id);


--
-- Name: playbook_run_group playbook_run_group_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_run_group
    ADD CONSTRAINT playbook_run_group_pkey PRIMARY KEY (id);


--
-- Name: playbook_run_group playbook_run_group_schedule_occurrence_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_run_group
    ADD CONSTRAINT playbook_run_group_schedule_occurrence_key UNIQUE (schedule_id, scheduled_for);


--
-- Name: playbook_run playbook_run_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_run
    ADD CONSTRAINT playbook_run_pkey PRIMARY KEY (id);


--
-- Name: playbook_schedule_occurrence playbook_schedule_occurrence_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_schedule_occurrence
    ADD CONSTRAINT playbook_schedule_occurrence_pkey PRIMARY KEY (schedule_id, scheduled_for);


--
-- Name: playbook_schedule playbook_schedule_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_schedule
    ADD CONSTRAINT playbook_schedule_pkey PRIMARY KEY (id);


--
-- Name: playbook_service_principal_credential_binding playbook_service_principal_credential_binding_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_service_principal_credential_binding
    ADD CONSTRAINT playbook_service_principal_credential_binding_pkey PRIMARY KEY (id);


--
-- Name: playbook_service_principal playbook_service_principal_name_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_service_principal
    ADD CONSTRAINT playbook_service_principal_name_key UNIQUE (name);


--
-- Name: playbook_service_principal playbook_service_principal_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_service_principal
    ADD CONSTRAINT playbook_service_principal_pkey PRIMARY KEY (id);


--
-- Name: playbook_service_principal playbook_service_principal_slug_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_service_principal
    ADD CONSTRAINT playbook_service_principal_slug_key UNIQUE (slug);


--
-- Name: playbook playbook_slug_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook
    ADD CONSTRAINT playbook_slug_key UNIQUE (slug);


--
-- Name: playbook_target_set_member playbook_target_set_member_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_target_set_member
    ADD CONSTRAINT playbook_target_set_member_pkey PRIMARY KEY (target_set_id, target_kind, target_id, source_kind, source_id);


--
-- Name: playbook_target_set playbook_target_set_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_target_set
    ADD CONSTRAINT playbook_target_set_pkey PRIMARY KEY (id);


--
-- Name: playbook_version playbook_version_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_version
    ADD CONSTRAINT playbook_version_pkey PRIMARY KEY (id);


--
-- Name: provider_request_journal provider_request_journal_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.provider_request_journal
    ADD CONSTRAINT provider_request_journal_pkey PRIMARY KEY (id);


--
-- Name: provider_request_journal provider_request_journal_provider_attempt_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.provider_request_journal
    ADD CONSTRAINT provider_request_journal_provider_attempt_key UNIQUE (provider_id, outbound_event_uuid, attempt_no);


--
-- Name: rbac_policy rbac_policy_name_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.rbac_policy
    ADD CONSTRAINT rbac_policy_name_key UNIQUE (name);


--
-- Name: rbac_policy rbac_policy_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.rbac_policy
    ADD CONSTRAINT rbac_policy_pkey PRIMARY KEY (id);


--
-- Name: rbac_role_assignment rbac_role_assignment_group_id_role_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.rbac_role_assignment
    ADD CONSTRAINT rbac_role_assignment_group_id_role_id_key UNIQUE (group_id, role_id);


--
-- Name: rbac_role_assignment rbac_role_assignment_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.rbac_role_assignment
    ADD CONSTRAINT rbac_role_assignment_pkey PRIMARY KEY (id);


--
-- Name: rbac_role_assignment rbac_role_assignment_user_id_role_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.rbac_role_assignment
    ADD CONSTRAINT rbac_role_assignment_user_id_role_id_key UNIQUE (user_id, role_id);


--
-- Name: rbac_role rbac_role_name_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.rbac_role
    ADD CONSTRAINT rbac_role_name_key UNIQUE (name);


--
-- Name: rbac_role rbac_role_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.rbac_role
    ADD CONSTRAINT rbac_role_pkey PRIMARY KEY (id);


--
-- Name: secret_usage_binding secret_usage_binding_consumer_type_consumer_id_field_key_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.secret_usage_binding
    ADD CONSTRAINT secret_usage_binding_consumer_type_consumer_id_field_key_key UNIQUE (consumer_type, consumer_id, field_key);


--
-- Name: secret_usage_binding secret_usage_binding_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.secret_usage_binding
    ADD CONSTRAINT secret_usage_binding_pkey PRIMARY KEY (id);


--
-- Name: secrets_binding secrets_binding_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.secrets_binding
    ADD CONSTRAINT secrets_binding_pkey PRIMARY KEY (id);


--
-- Name: secrets_store secrets_store_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.secrets_store
    ADD CONSTRAINT secrets_store_pkey PRIMARY KEY (secret_id);


--
-- Name: secrets_variable secrets_variable_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.secrets_variable
    ADD CONSTRAINT secrets_variable_pkey PRIMARY KEY (id);


--
-- Name: settings_values settings_values_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.settings_values
    ADD CONSTRAINT settings_values_pkey PRIMARY KEY (id);


--
-- Name: ssh_host_key_pin ssh_host_key_pin_host_port_key_type_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ssh_host_key_pin
    ADD CONSTRAINT ssh_host_key_pin_host_port_key_type_key UNIQUE (host, port, key_type);


--
-- Name: ssh_host_key_pin ssh_host_key_pin_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ssh_host_key_pin
    ADD CONSTRAINT ssh_host_key_pin_pkey PRIMARY KEY (id);


--
-- Name: sync_conflict_record sync_conflict_record_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.sync_conflict_record
    ADD CONSTRAINT sync_conflict_record_pkey PRIMARY KEY (id);


--
-- Name: sync_error sync_error_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.sync_error
    ADD CONSTRAINT sync_error_pkey PRIMARY KEY (id);


--
-- Name: sync_inventory_binding sync_inventory_binding_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.sync_inventory_binding
    ADD CONSTRAINT sync_inventory_binding_pkey PRIMARY KEY (source_provider_id, source_entity, external_id, target_entity);


--
-- Name: sync_inventory_binding sync_inventory_binding_target_local_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.sync_inventory_binding
    ADD CONSTRAINT sync_inventory_binding_target_local_key UNIQUE (target_entity, local_id);


--
-- Name: sync_job sync_job_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.sync_job
    ADD CONSTRAINT sync_job_pkey PRIMARY KEY (id);


--
-- Name: sync_job_step sync_job_step_job_id_step_key_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.sync_job_step
    ADD CONSTRAINT sync_job_step_job_id_step_key_key UNIQUE (job_id, step_key);


--
-- Name: sync_job_step sync_job_step_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.sync_job_step
    ADD CONSTRAINT sync_job_step_pkey PRIMARY KEY (id);


--
-- Name: sync_outbound_event sync_outbound_event_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.sync_outbound_event
    ADD CONSTRAINT sync_outbound_event_pkey PRIMARY KEY (id);


--
-- Name: sync_provider sync_provider_name_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.sync_provider
    ADD CONSTRAINT sync_provider_name_key UNIQUE (name);


--
-- Name: sync_provider sync_provider_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.sync_provider
    ADD CONSTRAINT sync_provider_pkey PRIMARY KEY (id);


--
-- Name: sync_run sync_run_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.sync_run
    ADD CONSTRAINT sync_run_pkey PRIMARY KEY (id);


--
-- Name: sync_source_runtime sync_source_runtime_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.sync_source_runtime
    ADD CONSTRAINT sync_source_runtime_pkey PRIMARY KEY (source_id);


--
-- Name: sync_webhook_event sync_webhook_event_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.sync_webhook_event
    ADD CONSTRAINT sync_webhook_event_pkey PRIMARY KEY (id);


--
-- Name: sync_webhook_event sync_webhook_event_source_id_dedup_key_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.sync_webhook_event
    ADD CONSTRAINT sync_webhook_event_source_id_dedup_key_key UNIQUE (source_id, dedup_key);


--
-- Name: tenancy_contact_groups tenancy_contact_groups_contact_id_contactgroup_id_f4434f2c_uniq; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tenancy_contact_groups
    ADD CONSTRAINT tenancy_contact_groups_contact_id_contactgroup_id_f4434f2c_uniq UNIQUE (contactgroup_id, contact_id);


--
-- Name: tenancy_contact_groups tenancy_contact_groups_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tenancy_contact_groups
    ADD CONSTRAINT tenancy_contact_groups_pkey PRIMARY KEY (id);


--
-- Name: tenancy_contact tenancy_contact_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tenancy_contact
    ADD CONSTRAINT tenancy_contact_pkey PRIMARY KEY (id);


--
-- Name: tenancy_contactassignment tenancy_contactassignment_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tenancy_contactassignment
    ADD CONSTRAINT tenancy_contactassignment_pkey PRIMARY KEY (id);


--
-- Name: tenancy_contactassignment tenancy_contactassignment_unique_object_contact_role; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tenancy_contactassignment
    ADD CONSTRAINT tenancy_contactassignment_unique_object_contact_role UNIQUE (object_type_id, object_id, contact_id, role_id);


--
-- Name: tenancy_contactgroup tenancy_contactgroup_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tenancy_contactgroup
    ADD CONSTRAINT tenancy_contactgroup_pkey PRIMARY KEY (id);


--
-- Name: tenancy_contactgroup tenancy_contactgroup_unique_parent_name; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tenancy_contactgroup
    ADD CONSTRAINT tenancy_contactgroup_unique_parent_name UNIQUE (parent_id, name);


--
-- Name: tenancy_contactrole tenancy_contactrole_name_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tenancy_contactrole
    ADD CONSTRAINT tenancy_contactrole_name_key UNIQUE (name);


--
-- Name: tenancy_contactrole tenancy_contactrole_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tenancy_contactrole
    ADD CONSTRAINT tenancy_contactrole_pkey PRIMARY KEY (id);


--
-- Name: tenancy_contactrole tenancy_contactrole_slug_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tenancy_contactrole
    ADD CONSTRAINT tenancy_contactrole_slug_key UNIQUE (slug);


--
-- Name: tenancy_tenant tenancy_tenant_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tenancy_tenant
    ADD CONSTRAINT tenancy_tenant_pkey PRIMARY KEY (id);


--
-- Name: tenancy_tenant tenancy_tenant_unique_group_name; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tenancy_tenant
    ADD CONSTRAINT tenancy_tenant_unique_group_name UNIQUE (group_id, name);


--
-- Name: tenancy_tenant tenancy_tenant_unique_group_slug; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tenancy_tenant
    ADD CONSTRAINT tenancy_tenant_unique_group_slug UNIQUE (group_id, slug);


--
-- Name: tenancy_tenantgroup tenancy_tenantgroup_name_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tenancy_tenantgroup
    ADD CONSTRAINT tenancy_tenantgroup_name_key UNIQUE (name);


--
-- Name: tenancy_tenantgroup tenancy_tenantgroup_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tenancy_tenantgroup
    ADD CONSTRAINT tenancy_tenantgroup_pkey PRIMARY KEY (id);


--
-- Name: tenancy_tenantgroup tenancy_tenantgroup_slug_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tenancy_tenantgroup
    ADD CONSTRAINT tenancy_tenantgroup_slug_key UNIQUE (slug);


--
-- Name: thumbnail_kvstore thumbnail_kvstore_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.thumbnail_kvstore
    ADD CONSTRAINT thumbnail_kvstore_pkey PRIMARY KEY (key);


--
-- Name: playbook_job_template_survey uq_playbook_job_template_survey_template; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_job_template_survey
    ADD CONSTRAINT uq_playbook_job_template_survey_template UNIQUE (job_template_id);


--
-- Name: playbook_run_event uq_playbook_run_event_sequence; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_run_event
    ADD CONSTRAINT uq_playbook_run_event_sequence UNIQUE (run_group_id, event_sequence);


--
-- Name: playbook_run uq_playbook_run_group_target_attempt; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_run
    ADD CONSTRAINT uq_playbook_run_group_target_attempt UNIQUE (run_group_id, target_kind, target_id, source_kind, source_id, attempt);


--
-- Name: playbook_version uq_playbook_version_number; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_version
    ADD CONSTRAINT uq_playbook_version_number UNIQUE (playbook_id, version);


--
-- Name: user_preferences uq_user_preferences_user_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_preferences
    ADD CONSTRAINT uq_user_preferences_user_key UNIQUE (user_id, key);


--
-- Name: user_preferences user_preferences_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_preferences
    ADD CONSTRAINT user_preferences_pkey PRIMARY KEY (id);


--
-- Name: users_owner_identity_group users_owner_identity_group_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.users_owner_identity_group
    ADD CONSTRAINT users_owner_identity_group_pkey PRIMARY KEY (owner_id, group_id);


--
-- Name: users_owner_identity_user users_owner_identity_user_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.users_owner_identity_user
    ADD CONSTRAINT users_owner_identity_user_pkey PRIMARY KEY (owner_id, user_id);


--
-- Name: users_owner users_owner_name_uniq; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.users_owner
    ADD CONSTRAINT users_owner_name_uniq UNIQUE (name);


--
-- Name: users_owner users_owner_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.users_owner
    ADD CONSTRAINT users_owner_pkey PRIMARY KEY (id);


--
-- Name: users_ownergroup users_ownergroup_name_uniq; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.users_ownergroup
    ADD CONSTRAINT users_ownergroup_name_uniq UNIQUE (name);


--
-- Name: users_ownergroup users_ownergroup_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.users_ownergroup
    ADD CONSTRAINT users_ownergroup_pkey PRIMARY KEY (id);


--
-- Name: virtualization_cluster virtualization_cluster_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_cluster
    ADD CONSTRAINT virtualization_cluster_pkey PRIMARY KEY (id);


--
-- Name: virtualization_cluster virtualization_cluster_unique__site_name; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_cluster
    ADD CONSTRAINT virtualization_cluster_unique__site_name UNIQUE (_site_id, name);


--
-- Name: virtualization_cluster virtualization_cluster_unique_group_name; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_cluster
    ADD CONSTRAINT virtualization_cluster_unique_group_name UNIQUE (group_id, name);


--
-- Name: virtualization_clustergroup virtualization_clustergroup_name_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_clustergroup
    ADD CONSTRAINT virtualization_clustergroup_name_key UNIQUE (name);


--
-- Name: virtualization_clustergroup virtualization_clustergroup_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_clustergroup
    ADD CONSTRAINT virtualization_clustergroup_pkey PRIMARY KEY (id);


--
-- Name: virtualization_clustergroup virtualization_clustergroup_slug_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_clustergroup
    ADD CONSTRAINT virtualization_clustergroup_slug_key UNIQUE (slug);


--
-- Name: virtualization_clustertype virtualization_clustertype_name_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_clustertype
    ADD CONSTRAINT virtualization_clustertype_name_key UNIQUE (name);


--
-- Name: virtualization_clustertype virtualization_clustertype_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_clustertype
    ADD CONSTRAINT virtualization_clustertype_pkey PRIMARY KEY (id);


--
-- Name: virtualization_clustertype virtualization_clustertype_slug_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_clustertype
    ADD CONSTRAINT virtualization_clustertype_slug_key UNIQUE (slug);


--
-- Name: virtualization_virtualdisk virtualization_virtualdisk_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_virtualdisk
    ADD CONSTRAINT virtualization_virtualdisk_pkey PRIMARY KEY (id);


--
-- Name: virtualization_virtualdisk virtualization_virtualdisk_unique_virtual_machine_name; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_virtualdisk
    ADD CONSTRAINT virtualization_virtualdisk_unique_virtual_machine_name UNIQUE (virtual_machine_id, name);


--
-- Name: virtualization_virtualmachine virtualization_virtualmachine_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_virtualmachine
    ADD CONSTRAINT virtualization_virtualmachine_pkey PRIMARY KEY (id);


--
-- Name: virtualization_virtualmachine virtualization_virtualmachine_primary_ip4_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_virtualmachine
    ADD CONSTRAINT virtualization_virtualmachine_primary_ip4_id_key UNIQUE (primary_ip4_id);


--
-- Name: virtualization_virtualmachine virtualization_virtualmachine_primary_ip6_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_virtualmachine
    ADD CONSTRAINT virtualization_virtualmachine_primary_ip6_id_key UNIQUE (primary_ip6_id);


--
-- Name: virtualization_virtualmachinetype virtualization_virtualmachinetype_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_virtualmachinetype
    ADD CONSTRAINT virtualization_virtualmachinetype_pkey PRIMARY KEY (id);


--
-- Name: virtualization_virtualmachinetype virtualization_virtualmachinetype_slug_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_virtualmachinetype
    ADD CONSTRAINT virtualization_virtualmachinetype_slug_key UNIQUE (slug);


--
-- Name: virtualization_vminterface_tagged_vlans virtualization_vminterfa_vminterface_id_vlan_id_27e907db_uniq; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_vminterface_tagged_vlans
    ADD CONSTRAINT virtualization_vminterfa_vminterface_id_vlan_id_27e907db_uniq UNIQUE (vminterface_id, vlan_id);


--
-- Name: virtualization_vminterface virtualization_vminterface_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_vminterface
    ADD CONSTRAINT virtualization_vminterface_pkey PRIMARY KEY (id);


--
-- Name: virtualization_vminterface virtualization_vminterface_primary_mac_address_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_vminterface
    ADD CONSTRAINT virtualization_vminterface_primary_mac_address_id_key UNIQUE (primary_mac_address_id);


--
-- Name: virtualization_vminterface_tagged_vlans virtualization_vminterface_tagged_vlans_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_vminterface_tagged_vlans
    ADD CONSTRAINT virtualization_vminterface_tagged_vlans_pkey PRIMARY KEY (id);


--
-- Name: virtualization_vminterface virtualization_vminterface_unique_virtual_machine_name; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_vminterface
    ADD CONSTRAINT virtualization_vminterface_unique_virtual_machine_name UNIQUE (virtual_machine_id, name);


--
-- Name: vpn_ikepolicy vpn_ikepolicy_name_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_ikepolicy
    ADD CONSTRAINT vpn_ikepolicy_name_key UNIQUE (name);


--
-- Name: vpn_ikepolicy vpn_ikepolicy_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_ikepolicy
    ADD CONSTRAINT vpn_ikepolicy_pkey PRIMARY KEY (id);


--
-- Name: vpn_ikepolicy_proposals vpn_ikepolicy_proposals_ikepolicy_id_ikeproposal_0c3baa9c_uniq; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_ikepolicy_proposals
    ADD CONSTRAINT vpn_ikepolicy_proposals_ikepolicy_id_ikeproposal_0c3baa9c_uniq UNIQUE (ikepolicy_id, ikeproposal_id);


--
-- Name: vpn_ikepolicy_proposals vpn_ikepolicy_proposals_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_ikepolicy_proposals
    ADD CONSTRAINT vpn_ikepolicy_proposals_pkey PRIMARY KEY (id);


--
-- Name: vpn_ikeproposal vpn_ikeproposal_name_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_ikeproposal
    ADD CONSTRAINT vpn_ikeproposal_name_key UNIQUE (name);


--
-- Name: vpn_ikeproposal vpn_ikeproposal_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_ikeproposal
    ADD CONSTRAINT vpn_ikeproposal_pkey PRIMARY KEY (id);


--
-- Name: vpn_ipsecpolicy vpn_ipsecpolicy_name_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_ipsecpolicy
    ADD CONSTRAINT vpn_ipsecpolicy_name_key UNIQUE (name);


--
-- Name: vpn_ipsecpolicy vpn_ipsecpolicy_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_ipsecpolicy
    ADD CONSTRAINT vpn_ipsecpolicy_pkey PRIMARY KEY (id);


--
-- Name: vpn_ipsecpolicy_proposals vpn_ipsecpolicy_proposal_ipsecpolicy_id_ipsecprop_72096768_uniq; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_ipsecpolicy_proposals
    ADD CONSTRAINT vpn_ipsecpolicy_proposal_ipsecpolicy_id_ipsecprop_72096768_uniq UNIQUE (ipsecpolicy_id, ipsecproposal_id);


--
-- Name: vpn_ipsecpolicy_proposals vpn_ipsecpolicy_proposals_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_ipsecpolicy_proposals
    ADD CONSTRAINT vpn_ipsecpolicy_proposals_pkey PRIMARY KEY (id);


--
-- Name: vpn_ipsecprofile vpn_ipsecprofile_name_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_ipsecprofile
    ADD CONSTRAINT vpn_ipsecprofile_name_key UNIQUE (name);


--
-- Name: vpn_ipsecprofile vpn_ipsecprofile_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_ipsecprofile
    ADD CONSTRAINT vpn_ipsecprofile_pkey PRIMARY KEY (id);


--
-- Name: vpn_ipsecproposal vpn_ipsecproposal_name_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_ipsecproposal
    ADD CONSTRAINT vpn_ipsecproposal_name_key UNIQUE (name);


--
-- Name: vpn_ipsecproposal vpn_ipsecproposal_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_ipsecproposal
    ADD CONSTRAINT vpn_ipsecproposal_pkey PRIMARY KEY (id);


--
-- Name: vpn_l2vpn vpn_l2vpn_name_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_l2vpn
    ADD CONSTRAINT vpn_l2vpn_name_key UNIQUE (name);


--
-- Name: vpn_l2vpn vpn_l2vpn_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_l2vpn
    ADD CONSTRAINT vpn_l2vpn_pkey PRIMARY KEY (id);


--
-- Name: vpn_l2vpn vpn_l2vpn_slug_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_l2vpn
    ADD CONSTRAINT vpn_l2vpn_slug_key UNIQUE (slug);


--
-- Name: vpn_l2vpntermination vpn_l2vpntermination_assigned_object; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_l2vpntermination
    ADD CONSTRAINT vpn_l2vpntermination_assigned_object UNIQUE (assigned_object_type_id, assigned_object_id);


--
-- Name: vpn_l2vpntermination vpn_l2vpntermination_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_l2vpntermination
    ADD CONSTRAINT vpn_l2vpntermination_pkey PRIMARY KEY (id);


--
-- Name: vpn_tunnel vpn_tunnel_group_name; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_tunnel
    ADD CONSTRAINT vpn_tunnel_group_name UNIQUE (group_id, name);


--
-- Name: vpn_tunnel vpn_tunnel_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_tunnel
    ADD CONSTRAINT vpn_tunnel_pkey PRIMARY KEY (id);


--
-- Name: vpn_tunnelgroup vpn_tunnelgroup_name_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_tunnelgroup
    ADD CONSTRAINT vpn_tunnelgroup_name_key UNIQUE (name);


--
-- Name: vpn_tunnelgroup vpn_tunnelgroup_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_tunnelgroup
    ADD CONSTRAINT vpn_tunnelgroup_pkey PRIMARY KEY (id);


--
-- Name: vpn_tunnelgroup vpn_tunnelgroup_slug_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_tunnelgroup
    ADD CONSTRAINT vpn_tunnelgroup_slug_key UNIQUE (slug);


--
-- Name: vpn_tunneltermination vpn_tunneltermination_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_tunneltermination
    ADD CONSTRAINT vpn_tunneltermination_pkey PRIMARY KEY (id);


--
-- Name: vpn_tunneltermination vpn_tunneltermination_termination; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_tunneltermination
    ADD CONSTRAINT vpn_tunneltermination_termination UNIQUE (termination_type_id, termination_id);


--
-- Name: webhook_subscriptions webhook_subscriptions_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.webhook_subscriptions
    ADD CONSTRAINT webhook_subscriptions_pkey PRIMARY KEY (id);


--
-- Name: wireless_wirelesslan wireless_wirelesslan_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.wireless_wirelesslan
    ADD CONSTRAINT wireless_wirelesslan_pkey PRIMARY KEY (id);


--
-- Name: wireless_wirelesslangroup wireless_wirelesslangroup_name_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.wireless_wirelesslangroup
    ADD CONSTRAINT wireless_wirelesslangroup_name_key UNIQUE (name);


--
-- Name: wireless_wirelesslangroup wireless_wirelesslangroup_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.wireless_wirelesslangroup
    ADD CONSTRAINT wireless_wirelesslangroup_pkey PRIMARY KEY (id);


--
-- Name: wireless_wirelesslangroup wireless_wirelesslangroup_slug_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.wireless_wirelesslangroup
    ADD CONSTRAINT wireless_wirelesslangroup_slug_key UNIQUE (slug);


--
-- Name: wireless_wirelesslink wireless_wirelesslink_interface_pair_uniq; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.wireless_wirelesslink
    ADD CONSTRAINT wireless_wirelesslink_interface_pair_uniq UNIQUE (interface_a_id, interface_b_id);


--
-- Name: wireless_wirelesslink wireless_wirelesslink_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.wireless_wirelesslink
    ADD CONSTRAINT wireless_wirelesslink_pkey PRIMARY KEY (id);


--
-- Name: wizard_progress wizard_progress_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.wizard_progress
    ADD CONSTRAINT wizard_progress_pkey PRIMARY KEY (id);


--
-- Name: wizard_progress wizard_progress_user_type_unique; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.wizard_progress
    ADD CONSTRAINT wizard_progress_user_type_unique UNIQUE (user_id, wizard_type);


--
-- Name: workspace_connection_target_layout workspace_connection_target_layout_name_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.workspace_connection_target_layout
    ADD CONSTRAINT workspace_connection_target_layout_name_key UNIQUE (name);


--
-- Name: workspace_connection_target_layout workspace_connection_target_layout_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.workspace_connection_target_layout
    ADD CONSTRAINT workspace_connection_target_layout_pkey PRIMARY KEY (id);


--
-- Name: circuits_ci_termina_505dda_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX circuits_ci_termina_505dda_idx ON public.circuits_circuittermination USING btree (termination_type_id, termination_id);


--
-- Name: circuits_circuit_owner_id_c330c2d0; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX circuits_circuit_owner_id_c330c2d0 ON public.circuits_circuit USING btree (owner_id);


--
-- Name: circuits_circuit_provider_account_id_a7c8f61b; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX circuits_circuit_provider_account_id_a7c8f61b ON public.circuits_circuit USING btree (provider_account_id);


--
-- Name: circuits_circuit_provider_id_d9195418; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX circuits_circuit_provider_id_d9195418 ON public.circuits_circuit USING btree (provider_id);


--
-- Name: circuits_circuit_tenant_id_812508a5; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX circuits_circuit_tenant_id_812508a5 ON public.circuits_circuit USING btree (tenant_id);


--
-- Name: circuits_circuit_termination_a_id_f891adac; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX circuits_circuit_termination_a_id_f891adac ON public.circuits_circuit USING btree (termination_a_id);


--
-- Name: circuits_circuit_termination_z_id_377b8551; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX circuits_circuit_termination_z_id_377b8551 ON public.circuits_circuit USING btree (termination_z_id);


--
-- Name: circuits_circuit_type_id_1b9f485a; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX circuits_circuit_type_id_1b9f485a ON public.circuits_circuit USING btree (type_id);


--
-- Name: circuits_circuitgroup_name_ec8ac1e5_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX circuits_circuitgroup_name_ec8ac1e5_like ON public.circuits_circuitgroup USING btree (name varchar_pattern_ops);


--
-- Name: circuits_circuitgroup_owner_id_1edf4d64; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX circuits_circuitgroup_owner_id_1edf4d64 ON public.circuits_circuitgroup USING btree (owner_id);


--
-- Name: circuits_circuitgroup_slug_61ca866b_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX circuits_circuitgroup_slug_61ca866b_like ON public.circuits_circuitgroup USING btree (slug varchar_pattern_ops);


--
-- Name: circuits_circuitgroup_tenant_id_5bafdc3f; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX circuits_circuitgroup_tenant_id_5bafdc3f ON public.circuits_circuitgroup USING btree (tenant_id);


--
-- Name: circuits_circuitgroupassignment_group_id_1a7b6580; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX circuits_circuitgroupassignment_group_id_1a7b6580 ON public.circuits_circuitgroupassignment USING btree (group_id);


--
-- Name: circuits_circuitgroupassignment_member_type_id_779d1a13; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX circuits_circuitgroupassignment_member_type_id_779d1a13 ON public.circuits_circuitgroupassignment USING btree (member_type_id);


--
-- Name: circuits_circuittermination__location_id_4a578dca; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX circuits_circuittermination__location_id_4a578dca ON public.circuits_circuittermination USING btree (_location_id);


--
-- Name: circuits_circuittermination__region_id_1fa03379; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX circuits_circuittermination__region_id_1fa03379 ON public.circuits_circuittermination USING btree (_region_id);


--
-- Name: circuits_circuittermination__site_group_id_ec0c1998; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX circuits_circuittermination__site_group_id_ec0c1998 ON public.circuits_circuittermination USING btree (_site_group_id);


--
-- Name: circuits_circuittermination__site_id_84942491; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX circuits_circuittermination__site_id_84942491 ON public.circuits_circuittermination USING btree (_site_id);


--
-- Name: circuits_circuittermination_cable_id_35e9f703; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX circuits_circuittermination_cable_id_35e9f703 ON public.circuits_circuittermination USING btree (cable_id);


--
-- Name: circuits_circuittermination_circuit_id_257e87e7; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX circuits_circuittermination_circuit_id_257e87e7 ON public.circuits_circuittermination USING btree (circuit_id);


--
-- Name: circuits_circuittermination_provider_network_id_b0c660f1; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX circuits_circuittermination_provider_network_id_b0c660f1 ON public.circuits_circuittermination USING btree (_provider_network_id);


--
-- Name: circuits_circuittermination_termination_type_id_c9262c91; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX circuits_circuittermination_termination_type_id_c9262c91 ON public.circuits_circuittermination USING btree (termination_type_id);


--
-- Name: circuits_circuittype_name_8256ea9a_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX circuits_circuittype_name_8256ea9a_like ON public.circuits_circuittype USING btree (name varchar_pattern_ops);


--
-- Name: circuits_circuittype_owner_id_9c3200c2; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX circuits_circuittype_owner_id_9c3200c2 ON public.circuits_circuittype USING btree (owner_id);


--
-- Name: circuits_circuittype_slug_9b4b3cf9_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX circuits_circuittype_slug_9b4b3cf9_like ON public.circuits_circuittype USING btree (slug varchar_pattern_ops);


--
-- Name: circuits_provider_asns_asn_id_0a6c53b3; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX circuits_provider_asns_asn_id_0a6c53b3 ON public.circuits_provider_asns USING btree (asn_id);


--
-- Name: circuits_provider_asns_provider_id_becc3f7e; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX circuits_provider_asns_provider_id_becc3f7e ON public.circuits_provider_asns USING btree (provider_id);


--
-- Name: circuits_provider_name_8f2514f5_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX circuits_provider_name_8f2514f5_like ON public.circuits_provider USING btree (name varchar_pattern_ops);


--
-- Name: circuits_provider_owner_id_9f9749b4; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX circuits_provider_owner_id_9f9749b4 ON public.circuits_provider USING btree (owner_id);


--
-- Name: circuits_provider_slug_c3c0aa10_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX circuits_provider_slug_c3c0aa10_like ON public.circuits_provider USING btree (slug varchar_pattern_ops);


--
-- Name: circuits_provideraccount_owner_id_6dcd4819; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX circuits_provideraccount_owner_id_6dcd4819 ON public.circuits_provideraccount USING btree (owner_id);


--
-- Name: circuits_provideraccount_provider_id_4bcd7e50; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX circuits_provideraccount_provider_id_4bcd7e50 ON public.circuits_provideraccount USING btree (provider_id);


--
-- Name: circuits_provideraccount_unique_provider_name; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX circuits_provideraccount_unique_provider_name ON public.circuits_provideraccount USING btree (provider_id, name) WHERE (NOT ((name)::text = ''::text));


--
-- Name: circuits_providernetwork_owner_id_caa8afde; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX circuits_providernetwork_owner_id_caa8afde ON public.circuits_providernetwork USING btree (owner_id);


--
-- Name: circuits_providernetwork_provider_id_7992236c; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX circuits_providernetwork_provider_id_7992236c ON public.circuits_providernetwork USING btree (provider_id);


--
-- Name: circuits_virtualcircuit_owner_id_89564a1e; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX circuits_virtualcircuit_owner_id_89564a1e ON public.circuits_virtualcircuit USING btree (owner_id);


--
-- Name: circuits_virtualcircuit_provider_account_id_d942f455; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX circuits_virtualcircuit_provider_account_id_d942f455 ON public.circuits_virtualcircuit USING btree (provider_account_id);


--
-- Name: circuits_virtualcircuit_provider_network_id_a902f409; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX circuits_virtualcircuit_provider_network_id_a902f409 ON public.circuits_virtualcircuit USING btree (provider_network_id);


--
-- Name: circuits_virtualcircuit_tenant_id_4458eca7; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX circuits_virtualcircuit_tenant_id_4458eca7 ON public.circuits_virtualcircuit USING btree (tenant_id);


--
-- Name: circuits_virtualcircuit_type_id_8527d682; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX circuits_virtualcircuit_type_id_8527d682 ON public.circuits_virtualcircuit USING btree (type_id);


--
-- Name: circuits_virtualcircuittermination_virtual_circuit_id_ca588886; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX circuits_virtualcircuittermination_virtual_circuit_id_ca588886 ON public.circuits_virtualcircuittermination USING btree (virtual_circuit_id);


--
-- Name: circuits_virtualcircuittype_name_5184db16_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX circuits_virtualcircuittype_name_5184db16_like ON public.circuits_virtualcircuittype USING btree (name varchar_pattern_ops);


--
-- Name: circuits_virtualcircuittype_owner_id_12a50b80; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX circuits_virtualcircuittype_owner_id_12a50b80 ON public.circuits_virtualcircuittype USING btree (owner_id);


--
-- Name: circuits_virtualcircuittype_slug_75d5c661_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX circuits_virtualcircuittype_slug_75d5c661_like ON public.circuits_virtualcircuittype USING btree (slug varchar_pattern_ops);


--
-- Name: custom_field_object_type_content_type_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX custom_field_object_type_content_type_idx ON public.custom_field_object_type USING btree (content_type_id);


--
-- Name: custom_field_weight_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX custom_field_weight_idx ON public.custom_field USING btree (weight, name);


--
-- Name: dcim_cable_bundle_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_cable_bundle_id ON public.dcim_cable USING btree (bundle_id);


--
-- Name: dcim_cable_owner_id_9cea1430; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_cable_owner_id_9cea1430 ON public.dcim_cable USING btree (owner_id);


--
-- Name: dcim_cable_tenant_id_3a7fdbb8; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_cable_tenant_id_3a7fdbb8 ON public.dcim_cable USING btree (tenant_id);


--
-- Name: dcim_cablebundle_owner_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_cablebundle_owner_id ON public.dcim_cablebundle USING btree (owner_id);


--
-- Name: dcim_cabletermination__device_id_f5884934; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_cabletermination__device_id_f5884934 ON public.dcim_cabletermination USING btree (_device_id);


--
-- Name: dcim_cabletermination__location_id_ff4be503; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_cabletermination__location_id_ff4be503 ON public.dcim_cabletermination USING btree (_location_id);


--
-- Name: dcim_cabletermination__rack_id_83a548e1; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_cabletermination__rack_id_83a548e1 ON public.dcim_cabletermination USING btree (_rack_id);


--
-- Name: dcim_cabletermination__site_id_616962fa; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_cabletermination__site_id_616962fa ON public.dcim_cabletermination USING btree (_site_id);


--
-- Name: dcim_cabletermination_cable_id_b50010d1; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_cabletermination_cable_id_b50010d1 ON public.dcim_cabletermination USING btree (cable_id);


--
-- Name: dcim_cabletermination_termination_type_id_20da439e; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_cabletermination_termination_type_id_20da439e ON public.dcim_cabletermination USING btree (termination_type_id);


--
-- Name: dcim_consoleport__location_id_d0e7437b; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_consoleport__location_id_d0e7437b ON public.dcim_consoleport USING btree (_location_id);


--
-- Name: dcim_consoleport__path_id_e40a4436; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_consoleport__path_id_e40a4436 ON public.dcim_consoleport USING btree (_path_id);


--
-- Name: dcim_consoleport__rack_id_71f16827; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_consoleport__rack_id_71f16827 ON public.dcim_consoleport USING btree (_rack_id);


--
-- Name: dcim_consoleport__site_id_9652df9c; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_consoleport__site_id_9652df9c ON public.dcim_consoleport USING btree (_site_id);


--
-- Name: dcim_consoleport_cable_id_a9ae5465; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_consoleport_cable_id_a9ae5465 ON public.dcim_consoleport USING btree (cable_id);


--
-- Name: dcim_consoleport_device_id_f2d90d3c; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_consoleport_device_id_f2d90d3c ON public.dcim_consoleport USING btree (device_id);


--
-- Name: dcim_consoleport_module_id_d17b2519; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_consoleport_module_id_d17b2519 ON public.dcim_consoleport USING btree (module_id);


--
-- Name: dcim_consoleport_owner_id_0648eae1; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_consoleport_owner_id_0648eae1 ON public.dcim_consoleport USING btree (owner_id);


--
-- Name: dcim_consoleporttemplate_device_type_id_075d4015; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_consoleporttemplate_device_type_id_075d4015 ON public.dcim_consoleporttemplate USING btree (device_type_id);


--
-- Name: dcim_consoleporttemplate_module_type_id_c0f35d97; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_consoleporttemplate_module_type_id_c0f35d97 ON public.dcim_consoleporttemplate USING btree (module_type_id);


--
-- Name: dcim_consoleserverport__location_id_7525ec0e; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_consoleserverport__location_id_7525ec0e ON public.dcim_consoleserverport USING btree (_location_id);


--
-- Name: dcim_consoleserverport__path_id_dc5abe09; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_consoleserverport__path_id_dc5abe09 ON public.dcim_consoleserverport USING btree (_path_id);


--
-- Name: dcim_consoleserverport__rack_id_3eea9f1b; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_consoleserverport__rack_id_3eea9f1b ON public.dcim_consoleserverport USING btree (_rack_id);


--
-- Name: dcim_consoleserverport__site_id_c500ac62; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_consoleserverport__site_id_c500ac62 ON public.dcim_consoleserverport USING btree (_site_id);


--
-- Name: dcim_consoleserverport_cable_id_f2940dfd; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_consoleserverport_cable_id_f2940dfd ON public.dcim_consoleserverport USING btree (cable_id);


--
-- Name: dcim_consoleserverport_device_id_d9866581; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_consoleserverport_device_id_d9866581 ON public.dcim_consoleserverport USING btree (device_id);


--
-- Name: dcim_consoleserverport_module_id_d060cfc8; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_consoleserverport_module_id_d060cfc8 ON public.dcim_consoleserverport USING btree (module_id);


--
-- Name: dcim_consoleserverport_owner_id_12b1a827; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_consoleserverport_owner_id_12b1a827 ON public.dcim_consoleserverport USING btree (owner_id);


--
-- Name: dcim_consoleserverporttemplate_device_type_id_579bdc86; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_consoleserverporttemplate_device_type_id_579bdc86 ON public.dcim_consoleserverporttemplate USING btree (device_type_id);


--
-- Name: dcim_consoleserverporttemplate_module_type_id_4abf751a; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_consoleserverporttemplate_module_type_id_4abf751a ON public.dcim_consoleserverporttemplate USING btree (module_type_id);


--
-- Name: dcim_coolingfeed_cooling_source_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_coolingfeed_cooling_source_id ON public.dcim_coolingfeed USING btree (cooling_source_id);


--
-- Name: dcim_coolingfeed_owner_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_coolingfeed_owner_id ON public.dcim_coolingfeed USING btree (owner_id);


--
-- Name: dcim_coolingfeed_rack_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_coolingfeed_rack_id ON public.dcim_coolingfeed USING btree (rack_id);


--
-- Name: dcim_coolingfeed_tenant_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_coolingfeed_tenant_id ON public.dcim_coolingfeed USING btree (tenant_id);


--
-- Name: dcim_coolingintake_cooling_outflow_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_coolingintake_cooling_outflow_id ON public.dcim_coolingintake USING btree (cooling_outflow_id);


--
-- Name: dcim_coolingintake_device_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_coolingintake_device_id ON public.dcim_coolingintake USING btree (device_id);


--
-- Name: dcim_coolingintake_module_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_coolingintake_module_id ON public.dcim_coolingintake USING btree (module_id);


--
-- Name: dcim_coolingintake_owner_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_coolingintake_owner_id ON public.dcim_coolingintake USING btree (owner_id);


--
-- Name: dcim_coolingintaketemplate_device_type_module_type_name; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_coolingintaketemplate_device_type_module_type_name ON public.dcim_coolingintaketemplate USING btree (device_type_id, module_type_id, name);


--
-- Name: dcim_coolingintaketemplate_module_type_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_coolingintaketemplate_module_type_id ON public.dcim_coolingintaketemplate USING btree (module_type_id);


--
-- Name: dcim_coolingoutflow_cooling_intake_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_coolingoutflow_cooling_intake_id ON public.dcim_coolingoutflow USING btree (cooling_intake_id);


--
-- Name: dcim_coolingoutflow_device_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_coolingoutflow_device_id ON public.dcim_coolingoutflow USING btree (device_id);


--
-- Name: dcim_coolingoutflow_module_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_coolingoutflow_module_id ON public.dcim_coolingoutflow USING btree (module_id);


--
-- Name: dcim_coolingoutflow_owner_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_coolingoutflow_owner_id ON public.dcim_coolingoutflow USING btree (owner_id);


--
-- Name: dcim_coolingoutflowtemplate_cooling_intake_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_coolingoutflowtemplate_cooling_intake_id ON public.dcim_coolingoutflowtemplate USING btree (cooling_intake_id);


--
-- Name: dcim_coolingoutflowtemplate_device_type_module_type_name; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_coolingoutflowtemplate_device_type_module_type_name ON public.dcim_coolingoutflowtemplate USING btree (device_type_id, module_type_id, name);


--
-- Name: dcim_coolingoutflowtemplate_module_type_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_coolingoutflowtemplate_module_type_id ON public.dcim_coolingoutflowtemplate USING btree (module_type_id);


--
-- Name: dcim_coolingsource_location_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_coolingsource_location_id ON public.dcim_coolingsource USING btree (location_id);


--
-- Name: dcim_coolingsource_owner_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_coolingsource_owner_id ON public.dcim_coolingsource USING btree (owner_id);


--
-- Name: dcim_coolingsource_site_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_coolingsource_site_id ON public.dcim_coolingsource USING btree (site_id);


--
-- Name: dcim_device_asset_tag_8dac1079_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_device_asset_tag_8dac1079_like ON public.dcim_device USING btree (asset_tag varchar_pattern_ops);


--
-- Name: dcim_device_cluster_id_cf852f78; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_device_cluster_id_cf852f78 ON public.dcim_device USING btree (cluster_id);


--
-- Name: dcim_device_config_template_id_316328c4; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_device_config_template_id_316328c4 ON public.dcim_device USING btree (config_template_id);


--
-- Name: dcim_device_device_role_id_682e8188; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_device_device_role_id_682e8188 ON public.dcim_device USING btree (role_id);


--
-- Name: dcim_device_device_type_id_d61b4086; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_device_device_type_id_d61b4086 ON public.dcim_device USING btree (device_type_id);


--
-- Name: dcim_device_location_id_11a7bedb; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_device_location_id_11a7bedb ON public.dcim_device USING btree (location_id);


--
-- Name: dcim_device_owner_id_87378f76; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_device_owner_id_87378f76 ON public.dcim_device USING btree (owner_id);


--
-- Name: dcim_device_platform_id_468138f1; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_device_platform_id_468138f1 ON public.dcim_device USING btree (platform_id);


--
-- Name: dcim_device_rack_id_23bde71f; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_device_rack_id_23bde71f ON public.dcim_device USING btree (rack_id);


--
-- Name: dcim_device_site_id_ff897cf6; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_device_site_id_ff897cf6 ON public.dcim_device USING btree (site_id);


--
-- Name: dcim_device_tenant_id_dcea7969; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_device_tenant_id_dcea7969 ON public.dcim_device USING btree (tenant_id);


--
-- Name: dcim_device_unique_name_site; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX dcim_device_unique_name_site ON public.dcim_device USING btree (lower((name)::text), site_id) WHERE (tenant_id IS NULL);


--
-- Name: dcim_device_unique_name_site_tenant; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX dcim_device_unique_name_site_tenant ON public.dcim_device USING btree (lower((name)::text), site_id, tenant_id);


--
-- Name: dcim_device_virtual_chassis_id_aed51693; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_device_virtual_chassis_id_aed51693 ON public.dcim_device USING btree (virtual_chassis_id);


--
-- Name: dcim_devicebay__location_id_6c869a87; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_devicebay__location_id_6c869a87 ON public.dcim_devicebay USING btree (_location_id);


--
-- Name: dcim_devicebay__rack_id_75672431; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_devicebay__rack_id_75672431 ON public.dcim_devicebay USING btree (_rack_id);


--
-- Name: dcim_devicebay__site_id_8ddc30f4; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_devicebay__site_id_8ddc30f4 ON public.dcim_devicebay USING btree (_site_id);


--
-- Name: dcim_devicebay_device_id_0c8a1218; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_devicebay_device_id_0c8a1218 ON public.dcim_devicebay USING btree (device_id);


--
-- Name: dcim_devicebay_owner_id_d7bcb3e6; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_devicebay_owner_id_d7bcb3e6 ON public.dcim_devicebay USING btree (owner_id);


--
-- Name: dcim_devicebaytemplate_device_type_id_f4b24a29; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_devicebaytemplate_device_type_id_f4b24a29 ON public.dcim_devicebaytemplate USING btree (device_type_id);


--
-- Name: dcim_devicerole_config_template_id_5874002c; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_devicerole_config_template_id_5874002c ON public.dcim_devicerole USING btree (config_template_id);


--
-- Name: dcim_devicerole_name; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX dcim_devicerole_name ON public.dcim_devicerole USING btree (name) WHERE (parent_id IS NULL);


--
-- Name: dcim_devicerole_owner_id_fdcc527d; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_devicerole_owner_id_fdcc527d ON public.dcim_devicerole USING btree (owner_id);


--
-- Name: dcim_devicerole_parent_id_7f9ccf87; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_devicerole_parent_id_7f9ccf87 ON public.dcim_devicerole USING btree (parent_id);


--
-- Name: dcim_devicerole_slug; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX dcim_devicerole_slug ON public.dcim_devicerole USING btree (slug) WHERE (parent_id IS NULL);


--
-- Name: dcim_devicerole_slug_7952643b; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_devicerole_slug_7952643b ON public.dcim_devicerole USING btree (slug);


--
-- Name: dcim_devicerole_slug_7952643b_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_devicerole_slug_7952643b_like ON public.dcim_devicerole USING btree (slug varchar_pattern_ops);


--
-- Name: dcim_devicerole_tree_id_196b114c; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_devicerole_tree_id_196b114c ON public.dcim_devicerole USING btree (tree_id);


--
-- Name: dcim_devicetype_default_platform_id_1f6ff6ac; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_devicetype_default_platform_id_1f6ff6ac ON public.dcim_devicetype USING btree (default_platform_id);


--
-- Name: dcim_devicetype_manufacturer_id_a3e8029e; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_devicetype_manufacturer_id_a3e8029e ON public.dcim_devicetype USING btree (manufacturer_id);


--
-- Name: dcim_devicetype_owner_id_243048ed; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_devicetype_owner_id_243048ed ON public.dcim_devicetype USING btree (owner_id);


--
-- Name: dcim_devicetype_slug_448745bd; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_devicetype_slug_448745bd ON public.dcim_devicetype USING btree (slug);


--
-- Name: dcim_devicetype_slug_448745bd_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_devicetype_slug_448745bd_like ON public.dcim_devicetype USING btree (slug varchar_pattern_ops);


--
-- Name: dcim_frontport__location_id_dc0402e7; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_frontport__location_id_dc0402e7 ON public.dcim_frontport USING btree (_location_id);


--
-- Name: dcim_frontport__rack_id_1ba6d11c; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_frontport__rack_id_1ba6d11c ON public.dcim_frontport USING btree (_rack_id);


--
-- Name: dcim_frontport__site_id_51efdefe; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_frontport__site_id_51efdefe ON public.dcim_frontport USING btree (_site_id);


--
-- Name: dcim_frontport_cable_id_04ff8aab; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_frontport_cable_id_04ff8aab ON public.dcim_frontport USING btree (cable_id);


--
-- Name: dcim_frontport_device_id_950557b5; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_frontport_device_id_950557b5 ON public.dcim_frontport USING btree (device_id);


--
-- Name: dcim_frontport_module_id_952c3f9a; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_frontport_module_id_952c3f9a ON public.dcim_frontport USING btree (module_id);


--
-- Name: dcim_frontport_owner_id_dfb61151; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_frontport_owner_id_dfb61151 ON public.dcim_frontport USING btree (owner_id);


--
-- Name: dcim_frontporttemplate_device_type_id_f088b952; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_frontporttemplate_device_type_id_f088b952 ON public.dcim_frontporttemplate USING btree (device_type_id);


--
-- Name: dcim_frontporttemplate_module_type_id_66851ff9; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_frontporttemplate_module_type_id_66851ff9 ON public.dcim_frontporttemplate USING btree (module_type_id);


--
-- Name: dcim_interface__location_id_72d5e107; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_interface__location_id_72d5e107 ON public.dcim_interface USING btree (_location_id);


--
-- Name: dcim_interface__path_id_f8f4f7f0; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_interface__path_id_f8f4f7f0 ON public.dcim_interface USING btree (_path_id);


--
-- Name: dcim_interface__rack_id_0d3f84a6; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_interface__rack_id_0d3f84a6 ON public.dcim_interface USING btree (_rack_id);


--
-- Name: dcim_interface__site_id_0fffd4ef; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_interface__site_id_0fffd4ef ON public.dcim_interface USING btree (_site_id);


--
-- Name: dcim_interface_bridge_id_f2a8df85; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_interface_bridge_id_f2a8df85 ON public.dcim_interface USING btree (bridge_id);


--
-- Name: dcim_interface_cable_id_1b264edb; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_interface_cable_id_1b264edb ON public.dcim_interface USING btree (cable_id);


--
-- Name: dcim_interface_device_id_359c6115; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_interface_device_id_359c6115 ON public.dcim_interface USING btree (device_id);


--
-- Name: dcim_interface_lag_id_ea1a1d12; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_interface_lag_id_ea1a1d12 ON public.dcim_interface USING btree (lag_id);


--
-- Name: dcim_interface_module_id_05ca2da5; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_interface_module_id_05ca2da5 ON public.dcim_interface USING btree (module_id);


--
-- Name: dcim_interface_owner_id_3ae797e2; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_interface_owner_id_3ae797e2 ON public.dcim_interface USING btree (owner_id);


--
-- Name: dcim_interface_parent_id_3e2b159b; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_interface_parent_id_3e2b159b ON public.dcim_interface USING btree (parent_id);


--
-- Name: dcim_interface_qinq_svlan_id_21189a58; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_interface_qinq_svlan_id_21189a58 ON public.dcim_interface USING btree (qinq_svlan_id);


--
-- Name: dcim_interface_tagged_vlans_interface_id_5870c9e9; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_interface_tagged_vlans_interface_id_5870c9e9 ON public.dcim_interface_tagged_vlans USING btree (interface_id);


--
-- Name: dcim_interface_tagged_vlans_vlan_id_e027005c; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_interface_tagged_vlans_vlan_id_e027005c ON public.dcim_interface_tagged_vlans USING btree (vlan_id);


--
-- Name: dcim_interface_untagged_vlan_id_838dc7be; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_interface_untagged_vlan_id_838dc7be ON public.dcim_interface USING btree (untagged_vlan_id);


--
-- Name: dcim_interface_vdcs_interface_id_6d58dbaf; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_interface_vdcs_interface_id_6d58dbaf ON public.dcim_interface_vdcs USING btree (interface_id);


--
-- Name: dcim_interface_vdcs_virtualdevicecontext_id_af0bfd4b; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_interface_vdcs_virtualdevicecontext_id_af0bfd4b ON public.dcim_interface_vdcs USING btree (virtualdevicecontext_id);


--
-- Name: dcim_interface_vlan_translation_policy_id_91060031; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_interface_vlan_translation_policy_id_91060031 ON public.dcim_interface USING btree (vlan_translation_policy_id);


--
-- Name: dcim_interface_vrf_id_a92e59b2; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_interface_vrf_id_a92e59b2 ON public.dcim_interface USING btree (vrf_id);


--
-- Name: dcim_interface_wireless_link_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_interface_wireless_link_id ON public.dcim_interface USING btree (wireless_link_id);


--
-- Name: dcim_interface_wirelesslans_interface_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_interface_wirelesslans_interface_id ON public.dcim_interface_wirelesslans USING btree (interface_id);


--
-- Name: dcim_interface_wirelesslans_wirelesslan_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_interface_wirelesslans_wirelesslan_id ON public.dcim_interface_wirelesslans USING btree (wirelesslan_id);


--
-- Name: dcim_interfacetemplate_bridge_id_4e44355b; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_interfacetemplate_bridge_id_4e44355b ON public.dcim_interfacetemplate USING btree (bridge_id);


--
-- Name: dcim_interfacetemplate_device_type_id_4bfcbfab; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_interfacetemplate_device_type_id_4bfcbfab ON public.dcim_interfacetemplate USING btree (device_type_id);


--
-- Name: dcim_interfacetemplate_module_type_id_f941f180; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_interfacetemplate_module_type_id_f941f180 ON public.dcim_interfacetemplate USING btree (module_type_id);


--
-- Name: dcim_invent_compone_0560bb_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_invent_compone_0560bb_idx ON public.dcim_inventoryitem USING btree (component_type_id, component_id);


--
-- Name: dcim_invent_compone_77b5f8_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_invent_compone_77b5f8_idx ON public.dcim_inventoryitemtemplate USING btree (component_type_id, component_id);


--
-- Name: dcim_inventoryitem__location_id_05a23b33; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_inventoryitem__location_id_05a23b33 ON public.dcim_inventoryitem USING btree (_location_id);


--
-- Name: dcim_inventoryitem__rack_id_25b80d23; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_inventoryitem__rack_id_25b80d23 ON public.dcim_inventoryitem USING btree (_rack_id);


--
-- Name: dcim_inventoryitem__site_id_cd94573b; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_inventoryitem__site_id_cd94573b ON public.dcim_inventoryitem USING btree (_site_id);


--
-- Name: dcim_inventoryitem_asset_tag_d3289273_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_inventoryitem_asset_tag_d3289273_like ON public.dcim_inventoryitem USING btree (asset_tag varchar_pattern_ops);


--
-- Name: dcim_inventoryitem_component_type_id_f0e4d83a; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_inventoryitem_component_type_id_f0e4d83a ON public.dcim_inventoryitem USING btree (component_type_id);


--
-- Name: dcim_inventoryitem_device_id_033d83f8; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_inventoryitem_device_id_033d83f8 ON public.dcim_inventoryitem USING btree (device_id);


--
-- Name: dcim_inventoryitem_manufacturer_id_dcd1b78a; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_inventoryitem_manufacturer_id_dcd1b78a ON public.dcim_inventoryitem USING btree (manufacturer_id);


--
-- Name: dcim_inventoryitem_owner_id_ad4332eb; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_inventoryitem_owner_id_ad4332eb ON public.dcim_inventoryitem USING btree (owner_id);


--
-- Name: dcim_inventoryitem_parent_id_7ebcd457; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_inventoryitem_parent_id_7ebcd457 ON public.dcim_inventoryitem USING btree (parent_id);


--
-- Name: dcim_inventoryitem_role_id_2bcfcb04; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_inventoryitem_role_id_2bcfcb04 ON public.dcim_inventoryitem USING btree (role_id);


--
-- Name: dcim_inventoryitem_tree_id_4676ade2; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_inventoryitem_tree_id_4676ade2 ON public.dcim_inventoryitem USING btree (tree_id);


--
-- Name: dcim_inventoryitemrole_name_4c8cfe6d_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_inventoryitemrole_name_4c8cfe6d_like ON public.dcim_inventoryitemrole USING btree (name varchar_pattern_ops);


--
-- Name: dcim_inventoryitemrole_owner_id_067442e9; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_inventoryitemrole_owner_id_067442e9 ON public.dcim_inventoryitemrole USING btree (owner_id);


--
-- Name: dcim_inventoryitemrole_slug_3556c227_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_inventoryitemrole_slug_3556c227_like ON public.dcim_inventoryitemrole USING btree (slug varchar_pattern_ops);


--
-- Name: dcim_inventoryitemtemplate_component_type_id_161623a2; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_inventoryitemtemplate_component_type_id_161623a2 ON public.dcim_inventoryitemtemplate USING btree (component_type_id);


--
-- Name: dcim_inventoryitemtemplate_device_type_id_6a1be904; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_inventoryitemtemplate_device_type_id_6a1be904 ON public.dcim_inventoryitemtemplate USING btree (device_type_id);


--
-- Name: dcim_inventoryitemtemplate_manufacturer_id_b388c5d9; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_inventoryitemtemplate_manufacturer_id_b388c5d9 ON public.dcim_inventoryitemtemplate USING btree (manufacturer_id);


--
-- Name: dcim_inventoryitemtemplate_parent_id_0dac73bb; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_inventoryitemtemplate_parent_id_0dac73bb ON public.dcim_inventoryitemtemplate USING btree (parent_id);


--
-- Name: dcim_inventoryitemtemplate_role_id_292676e6; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_inventoryitemtemplate_role_id_292676e6 ON public.dcim_inventoryitemtemplate USING btree (role_id);


--
-- Name: dcim_inventoryitemtemplate_tree_id_75ebcb8e; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_inventoryitemtemplate_tree_id_75ebcb8e ON public.dcim_inventoryitemtemplate USING btree (tree_id);


--
-- Name: dcim_location_name; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX dcim_location_name ON public.dcim_location USING btree (site_id, name) WHERE (parent_id IS NULL);


--
-- Name: dcim_location_owner_id_919a8713; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_location_owner_id_919a8713 ON public.dcim_location USING btree (owner_id);


--
-- Name: dcim_location_parent_id_d77f3318; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_location_parent_id_d77f3318 ON public.dcim_location USING btree (parent_id);


--
-- Name: dcim_location_site_id_b55e975f; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_location_site_id_b55e975f ON public.dcim_location USING btree (site_id);


--
-- Name: dcim_location_slug; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX dcim_location_slug ON public.dcim_location USING btree (site_id, slug) WHERE (parent_id IS NULL);


--
-- Name: dcim_location_slug_352c5472; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_location_slug_352c5472 ON public.dcim_location USING btree (slug);


--
-- Name: dcim_location_slug_352c5472_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_location_slug_352c5472_like ON public.dcim_location USING btree (slug varchar_pattern_ops);


--
-- Name: dcim_location_tenant_id_2c4df974; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_location_tenant_id_2c4df974 ON public.dcim_location USING btree (tenant_id);


--
-- Name: dcim_location_tree_id_5089ef14; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_location_tree_id_5089ef14 ON public.dcim_location USING btree (tree_id);


--
-- Name: dcim_macadd_assigne_54115d_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_macadd_assigne_54115d_idx ON public.dcim_macaddress USING btree (assigned_object_type_id, assigned_object_id);


--
-- Name: dcim_macaddress_assigned_object_type_id_28814a20; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_macaddress_assigned_object_type_id_28814a20 ON public.dcim_macaddress USING btree (assigned_object_type_id);


--
-- Name: dcim_macaddress_mac_address; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_macaddress_mac_address ON public.dcim_macaddress USING btree (mac_address);


--
-- Name: dcim_macaddress_owner_id_29ba2f60; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_macaddress_owner_id_29ba2f60 ON public.dcim_macaddress USING btree (owner_id);


--
-- Name: dcim_manufacturer_name_841fcd92_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_manufacturer_name_841fcd92_like ON public.dcim_manufacturer USING btree (name varchar_pattern_ops);


--
-- Name: dcim_manufacturer_owner_id_8d78661f; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_manufacturer_owner_id_8d78661f ON public.dcim_manufacturer USING btree (owner_id);


--
-- Name: dcim_manufacturer_slug_00430749_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_manufacturer_slug_00430749_like ON public.dcim_manufacturer USING btree (slug varchar_pattern_ops);


--
-- Name: dcim_module_asset_tag_2fd91eed_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_module_asset_tag_2fd91eed_like ON public.dcim_module USING btree (asset_tag varchar_pattern_ops);


--
-- Name: dcim_module_device_id_53cfd5be; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_module_device_id_53cfd5be ON public.dcim_module USING btree (device_id);


--
-- Name: dcim_module_module_type_id_a50b39fc; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_module_module_type_id_a50b39fc ON public.dcim_module USING btree (module_type_id);


--
-- Name: dcim_module_owner_id_ee6f1ef4; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_module_owner_id_ee6f1ef4 ON public.dcim_module USING btree (owner_id);


--
-- Name: dcim_modulebay__location_id_17290069; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_modulebay__location_id_17290069 ON public.dcim_modulebay USING btree (_location_id);


--
-- Name: dcim_modulebay__rack_id_673d8fb5; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_modulebay__rack_id_673d8fb5 ON public.dcim_modulebay USING btree (_rack_id);


--
-- Name: dcim_modulebay__site_id_cedd61bc; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_modulebay__site_id_cedd61bc ON public.dcim_modulebay USING btree (_site_id);


--
-- Name: dcim_modulebay_device_id_3526abc2; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_modulebay_device_id_3526abc2 ON public.dcim_modulebay USING btree (device_id);


--
-- Name: dcim_modulebay_module_id_a21ddd9a; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_modulebay_module_id_a21ddd9a ON public.dcim_modulebay USING btree (module_id);


--
-- Name: dcim_modulebay_owner_id_311f58c6; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_modulebay_owner_id_311f58c6 ON public.dcim_modulebay USING btree (owner_id);


--
-- Name: dcim_modulebay_parent_id_e483f9b7; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_modulebay_parent_id_e483f9b7 ON public.dcim_modulebay USING btree (parent_id);


--
-- Name: dcim_modulebay_tree_id_223db581; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_modulebay_tree_id_223db581 ON public.dcim_modulebay USING btree (tree_id);


--
-- Name: dcim_modulebaytemplate_device_type_id_9eaf9bd3; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_modulebaytemplate_device_type_id_9eaf9bd3 ON public.dcim_modulebaytemplate USING btree (device_type_id);


--
-- Name: dcim_modulebaytemplate_module_type_id_2fdfb491; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_modulebaytemplate_module_type_id_2fdfb491 ON public.dcim_modulebaytemplate USING btree (module_type_id);


--
-- Name: dcim_modulebaytype_manufacturer_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_modulebaytype_manufacturer_id ON public.dcim_modulebaytype USING btree (manufacturer_id);


--
-- Name: dcim_modulebaytype_owner_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_modulebaytype_owner_id ON public.dcim_modulebaytype USING btree (owner_id);


--
-- Name: dcim_moduletype_manufacturer_id_7347392e; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_moduletype_manufacturer_id_7347392e ON public.dcim_moduletype USING btree (manufacturer_id);


--
-- Name: dcim_moduletype_owner_id_2ea2fd3b; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_moduletype_owner_id_2ea2fd3b ON public.dcim_moduletype USING btree (owner_id);


--
-- Name: dcim_moduletype_profile_id_62b5d02d; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_moduletype_profile_id_62b5d02d ON public.dcim_moduletype USING btree (profile_id);


--
-- Name: dcim_moduletypeprofile_name_1709c36e_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_moduletypeprofile_name_1709c36e_like ON public.dcim_moduletypeprofile USING btree (name varchar_pattern_ops);


--
-- Name: dcim_moduletypeprofile_owner_id_c4488ff6; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_moduletypeprofile_owner_id_c4488ff6 ON public.dcim_moduletypeprofile USING btree (owner_id);


--
-- Name: dcim_platform_config_template_id_013a4d3c; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_platform_config_template_id_013a4d3c ON public.dcim_platform USING btree (config_template_id);


--
-- Name: dcim_platform_manufacturer_id_83f72d3d; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_platform_manufacturer_id_83f72d3d ON public.dcim_platform USING btree (manufacturer_id);


--
-- Name: dcim_platform_name; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX dcim_platform_name ON public.dcim_platform USING btree (name) WHERE (manufacturer_id IS NULL);


--
-- Name: dcim_platform_owner_id_0990eb87; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_platform_owner_id_0990eb87 ON public.dcim_platform USING btree (owner_id);


--
-- Name: dcim_platform_parent_id_795c7101; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_platform_parent_id_795c7101 ON public.dcim_platform USING btree (parent_id);


--
-- Name: dcim_platform_slug; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX dcim_platform_slug ON public.dcim_platform USING btree (slug) WHERE (manufacturer_id IS NULL);


--
-- Name: dcim_platform_slug_b0908ae4; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_platform_slug_b0908ae4 ON public.dcim_platform USING btree (slug);


--
-- Name: dcim_platform_slug_b0908ae4_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_platform_slug_b0908ae4_like ON public.dcim_platform USING btree (slug varchar_pattern_ops);


--
-- Name: dcim_platform_tree_id_ca32aeb8; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_platform_tree_id_ca32aeb8 ON public.dcim_platform USING btree (tree_id);


--
-- Name: dcim_portmapping_device_id_eb86e378; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_portmapping_device_id_eb86e378 ON public.dcim_portmapping USING btree (device_id);


--
-- Name: dcim_portmapping_front_port_id_d8413d45; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_portmapping_front_port_id_d8413d45 ON public.dcim_portmapping USING btree (front_port_id);


--
-- Name: dcim_portmapping_rear_port_id_dc3fb4b8; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_portmapping_rear_port_id_dc3fb4b8 ON public.dcim_portmapping USING btree (rear_port_id);


--
-- Name: dcim_porttemplatemapping_device_type_id_dede0eeb; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_porttemplatemapping_device_type_id_dede0eeb ON public.dcim_porttemplatemapping USING btree (device_type_id);


--
-- Name: dcim_porttemplatemapping_front_port_id_090c3c11; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_porttemplatemapping_front_port_id_090c3c11 ON public.dcim_porttemplatemapping USING btree (front_port_id);


--
-- Name: dcim_porttemplatemapping_module_type_id_3a84e529; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_porttemplatemapping_module_type_id_3a84e529 ON public.dcim_porttemplatemapping USING btree (module_type_id);


--
-- Name: dcim_porttemplatemapping_rear_port_id_93a9b08f; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_porttemplatemapping_rear_port_id_93a9b08f ON public.dcim_porttemplatemapping USING btree (rear_port_id);


--
-- Name: dcim_powerfeed__path_id_a1ea1f28; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_powerfeed__path_id_a1ea1f28 ON public.dcim_powerfeed USING btree (_path_id);


--
-- Name: dcim_powerfeed_cable_id_ec44c4f8; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_powerfeed_cable_id_ec44c4f8 ON public.dcim_powerfeed USING btree (cable_id);


--
-- Name: dcim_powerfeed_owner_id_97320081; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_powerfeed_owner_id_97320081 ON public.dcim_powerfeed USING btree (owner_id);


--
-- Name: dcim_powerfeed_power_panel_id_32bde3be; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_powerfeed_power_panel_id_32bde3be ON public.dcim_powerfeed USING btree (power_panel_id);


--
-- Name: dcim_powerfeed_rack_id_7abba090; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_powerfeed_rack_id_7abba090 ON public.dcim_powerfeed USING btree (rack_id);


--
-- Name: dcim_powerfeed_tenant_id_947bee85; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_powerfeed_tenant_id_947bee85 ON public.dcim_powerfeed USING btree (tenant_id);


--
-- Name: dcim_poweroutlet__location_id_49563316; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_poweroutlet__location_id_49563316 ON public.dcim_poweroutlet USING btree (_location_id);


--
-- Name: dcim_poweroutlet__path_id_cbb47bb9; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_poweroutlet__path_id_cbb47bb9 ON public.dcim_poweroutlet USING btree (_path_id);


--
-- Name: dcim_poweroutlet__rack_id_aca89672; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_poweroutlet__rack_id_aca89672 ON public.dcim_poweroutlet USING btree (_rack_id);


--
-- Name: dcim_poweroutlet__site_id_a956755e; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_poweroutlet__site_id_a956755e ON public.dcim_poweroutlet USING btree (_site_id);


--
-- Name: dcim_poweroutlet_cable_id_8dbea1ec; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_poweroutlet_cable_id_8dbea1ec ON public.dcim_poweroutlet USING btree (cable_id);


--
-- Name: dcim_poweroutlet_device_id_286351d7; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_poweroutlet_device_id_286351d7 ON public.dcim_poweroutlet USING btree (device_id);


--
-- Name: dcim_poweroutlet_module_id_032f5af2; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_poweroutlet_module_id_032f5af2 ON public.dcim_poweroutlet USING btree (module_id);


--
-- Name: dcim_poweroutlet_owner_id_0806d01d; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_poweroutlet_owner_id_0806d01d ON public.dcim_poweroutlet USING btree (owner_id);


--
-- Name: dcim_poweroutlet_power_port_id_9bdf4163; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_poweroutlet_power_port_id_9bdf4163 ON public.dcim_poweroutlet USING btree (power_port_id);


--
-- Name: dcim_poweroutlettemplate_device_type_id_26b2316c; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_poweroutlettemplate_device_type_id_26b2316c ON public.dcim_poweroutlettemplate USING btree (device_type_id);


--
-- Name: dcim_poweroutlettemplate_module_type_id_6142b416; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_poweroutlettemplate_module_type_id_6142b416 ON public.dcim_poweroutlettemplate USING btree (module_type_id);


--
-- Name: dcim_poweroutlettemplate_power_port_id_c0fb0c42; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_poweroutlettemplate_power_port_id_c0fb0c42 ON public.dcim_poweroutlettemplate USING btree (power_port_id);


--
-- Name: dcim_powerpanel_location_id_474b60f8; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_powerpanel_location_id_474b60f8 ON public.dcim_powerpanel USING btree (location_id);


--
-- Name: dcim_powerpanel_owner_id_11e7d421; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_powerpanel_owner_id_11e7d421 ON public.dcim_powerpanel USING btree (owner_id);


--
-- Name: dcim_powerpanel_site_id_c430bc89; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_powerpanel_site_id_c430bc89 ON public.dcim_powerpanel USING btree (site_id);


--
-- Name: dcim_powerport__location_id_ccf995e7; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_powerport__location_id_ccf995e7 ON public.dcim_powerport USING btree (_location_id);


--
-- Name: dcim_powerport__path_id_9fe4af8f; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_powerport__path_id_9fe4af8f ON public.dcim_powerport USING btree (_path_id);


--
-- Name: dcim_powerport__rack_id_4c1c316d; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_powerport__rack_id_4c1c316d ON public.dcim_powerport USING btree (_rack_id);


--
-- Name: dcim_powerport__site_id_87bfa142; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_powerport__site_id_87bfa142 ON public.dcim_powerport USING btree (_site_id);


--
-- Name: dcim_powerport_cable_id_c9682ba2; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_powerport_cable_id_c9682ba2 ON public.dcim_powerport USING btree (cable_id);


--
-- Name: dcim_powerport_device_id_ef7185ae; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_powerport_device_id_ef7185ae ON public.dcim_powerport USING btree (device_id);


--
-- Name: dcim_powerport_module_id_d0c27534; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_powerport_module_id_d0c27534 ON public.dcim_powerport USING btree (module_id);


--
-- Name: dcim_powerport_owner_id_b83ff931; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_powerport_owner_id_b83ff931 ON public.dcim_powerport USING btree (owner_id);


--
-- Name: dcim_powerporttemplate_device_type_id_1ddfbfcc; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_powerporttemplate_device_type_id_1ddfbfcc ON public.dcim_powerporttemplate USING btree (device_type_id);


--
-- Name: dcim_powerporttemplate_module_type_id_93e26849; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_powerporttemplate_module_type_id_93e26849 ON public.dcim_powerporttemplate USING btree (module_type_id);


--
-- Name: dcim_rack_asset_tag_f88408e5_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_rack_asset_tag_f88408e5_like ON public.dcim_rack USING btree (asset_tag varchar_pattern_ops);


--
-- Name: dcim_rack_group_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_rack_group_id ON public.dcim_rack USING btree (group_id);


--
-- Name: dcim_rack_location_id_5f63ec31; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_rack_location_id_5f63ec31 ON public.dcim_rack USING btree (location_id);


--
-- Name: dcim_rack_owner_id_7a5532fc; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_rack_owner_id_7a5532fc ON public.dcim_rack USING btree (owner_id);


--
-- Name: dcim_rack_rack_type_id_39433a22; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_rack_rack_type_id_39433a22 ON public.dcim_rack USING btree (rack_type_id);


--
-- Name: dcim_rack_role_id_62d6919e; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_rack_role_id_62d6919e ON public.dcim_rack USING btree (role_id);


--
-- Name: dcim_rack_site_id_403c7b3a; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_rack_site_id_403c7b3a ON public.dcim_rack USING btree (site_id);


--
-- Name: dcim_rack_tenant_id_7cdf3725; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_rack_tenant_id_7cdf3725 ON public.dcim_rack USING btree (tenant_id);


--
-- Name: dcim_rackgroup_owner_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_rackgroup_owner_id ON public.dcim_rackgroup USING btree (owner_id);


--
-- Name: dcim_rackreservation_owner_id_12c19e94; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_rackreservation_owner_id_12c19e94 ON public.dcim_rackreservation USING btree (owner_id);


--
-- Name: dcim_rackreservation_rack_id_1ebbaa9b; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_rackreservation_rack_id_1ebbaa9b ON public.dcim_rackreservation USING btree (rack_id);


--
-- Name: dcim_rackreservation_tenant_id_eb5e045f; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_rackreservation_tenant_id_eb5e045f ON public.dcim_rackreservation USING btree (tenant_id);


--
-- Name: dcim_rackreservation_user_id_0785a527; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_rackreservation_user_id_0785a527 ON public.dcim_rackreservation USING btree (user_id);


--
-- Name: dcim_rackrole_name_9077cfcc_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_rackrole_name_9077cfcc_like ON public.dcim_rackrole USING btree (name varchar_pattern_ops);


--
-- Name: dcim_rackrole_owner_id_fb6f0b77; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_rackrole_owner_id_fb6f0b77 ON public.dcim_rackrole USING btree (owner_id);


--
-- Name: dcim_rackrole_slug_40bbcd3a_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_rackrole_slug_40bbcd3a_like ON public.dcim_rackrole USING btree (slug varchar_pattern_ops);


--
-- Name: dcim_racktype_manufacturer_id_d46a05c6; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_racktype_manufacturer_id_d46a05c6 ON public.dcim_racktype USING btree (manufacturer_id);


--
-- Name: dcim_racktype_owner_id_ce0a0d75; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_racktype_owner_id_ce0a0d75 ON public.dcim_racktype USING btree (owner_id);


--
-- Name: dcim_racktype_slug_6bbb384a_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_racktype_slug_6bbb384a_like ON public.dcim_racktype USING btree (slug varchar_pattern_ops);


--
-- Name: dcim_rearport__location_id_72554006; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_rearport__location_id_72554006 ON public.dcim_rearport USING btree (_location_id);


--
-- Name: dcim_rearport__rack_id_9af9a402; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_rearport__rack_id_9af9a402 ON public.dcim_rearport USING btree (_rack_id);


--
-- Name: dcim_rearport__site_id_35e05ccf; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_rearport__site_id_35e05ccf ON public.dcim_rearport USING btree (_site_id);


--
-- Name: dcim_rearport_cable_id_42c0e4e7; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_rearport_cable_id_42c0e4e7 ON public.dcim_rearport USING btree (cable_id);


--
-- Name: dcim_rearport_device_id_0bdfe9c0; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_rearport_device_id_0bdfe9c0 ON public.dcim_rearport USING btree (device_id);


--
-- Name: dcim_rearport_module_id_9a7b7e91; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_rearport_module_id_9a7b7e91 ON public.dcim_rearport USING btree (module_id);


--
-- Name: dcim_rearport_owner_id_51174512; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_rearport_owner_id_51174512 ON public.dcim_rearport USING btree (owner_id);


--
-- Name: dcim_rearporttemplate_device_type_id_6a02fd01; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_rearporttemplate_device_type_id_6a02fd01 ON public.dcim_rearporttemplate USING btree (device_type_id);


--
-- Name: dcim_rearporttemplate_module_type_id_4d970e5b; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_rearporttemplate_module_type_id_4d970e5b ON public.dcim_rearporttemplate USING btree (module_type_id);


--
-- Name: dcim_region_name; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX dcim_region_name ON public.dcim_region USING btree (name) WHERE (parent_id IS NULL);


--
-- Name: dcim_region_owner_id_7e9d3adf; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_region_owner_id_7e9d3adf ON public.dcim_region USING btree (owner_id);


--
-- Name: dcim_region_parent_id_2486f5d4; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_region_parent_id_2486f5d4 ON public.dcim_region USING btree (parent_id);


--
-- Name: dcim_region_slug; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX dcim_region_slug ON public.dcim_region USING btree (slug) WHERE (parent_id IS NULL);


--
-- Name: dcim_region_slug_ff078a66; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_region_slug_ff078a66 ON public.dcim_region USING btree (slug);


--
-- Name: dcim_region_slug_ff078a66_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_region_slug_ff078a66_like ON public.dcim_region USING btree (slug varchar_pattern_ops);


--
-- Name: dcim_region_tree_id_a09ea9a7; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_region_tree_id_a09ea9a7 ON public.dcim_region USING btree (tree_id);


--
-- Name: dcim_site_asns_asn_id_3cfd0f00; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_site_asns_asn_id_3cfd0f00 ON public.dcim_site_asns USING btree (asn_id);


--
-- Name: dcim_site_asns_site_id_112ccacf; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_site_asns_site_id_112ccacf ON public.dcim_site_asns USING btree (site_id);


--
-- Name: dcim_site_group_id_3910c975; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_site_group_id_3910c975 ON public.dcim_site USING btree (group_id);


--
-- Name: dcim_site_name_8fe66c76_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_site_name_8fe66c76_like ON public.dcim_site USING btree (name varchar_pattern_ops);


--
-- Name: dcim_site_owner_id_ef94687e; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_site_owner_id_ef94687e ON public.dcim_site USING btree (owner_id);


--
-- Name: dcim_site_region_id_45210932; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_site_region_id_45210932 ON public.dcim_site USING btree (region_id);


--
-- Name: dcim_site_slug_4412c762_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_site_slug_4412c762_like ON public.dcim_site USING btree (slug varchar_pattern_ops);


--
-- Name: dcim_site_tenant_id_15e7df63; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_site_tenant_id_15e7df63 ON public.dcim_site USING btree (tenant_id);


--
-- Name: dcim_sitegroup_name; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX dcim_sitegroup_name ON public.dcim_sitegroup USING btree (name) WHERE (parent_id IS NULL);


--
-- Name: dcim_sitegroup_owner_id_50283a64; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_sitegroup_owner_id_50283a64 ON public.dcim_sitegroup USING btree (owner_id);


--
-- Name: dcim_sitegroup_parent_id_533a5e44; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_sitegroup_parent_id_533a5e44 ON public.dcim_sitegroup USING btree (parent_id);


--
-- Name: dcim_sitegroup_slug; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX dcim_sitegroup_slug ON public.dcim_sitegroup USING btree (slug) WHERE (parent_id IS NULL);


--
-- Name: dcim_sitegroup_slug_a11d2b04; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_sitegroup_slug_a11d2b04 ON public.dcim_sitegroup USING btree (slug);


--
-- Name: dcim_sitegroup_slug_a11d2b04_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_sitegroup_slug_a11d2b04_like ON public.dcim_sitegroup USING btree (slug varchar_pattern_ops);


--
-- Name: dcim_sitegroup_tree_id_e76dc999; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_sitegroup_tree_id_e76dc999 ON public.dcim_sitegroup USING btree (tree_id);


--
-- Name: dcim_virtualchassis_owner_id_76116efe; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_virtualchassis_owner_id_76116efe ON public.dcim_virtualchassis USING btree (owner_id);


--
-- Name: dcim_virtualdevicecontext_device_id_4f39274b; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_virtualdevicecontext_device_id_4f39274b ON public.dcim_virtualdevicecontext USING btree (device_id);


--
-- Name: dcim_virtualdevicecontext_owner_id_33c19ef7; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_virtualdevicecontext_owner_id_33c19ef7 ON public.dcim_virtualdevicecontext USING btree (owner_id);


--
-- Name: dcim_virtualdevicecontext_tenant_id_b6a21753; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX dcim_virtualdevicecontext_tenant_id_b6a21753 ON public.dcim_virtualdevicecontext USING btree (tenant_id);


--
-- Name: idx_assistant_messages_session_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_assistant_messages_session_created ON public.assistant_messages USING btree (session_id, created_at);


--
-- Name: idx_assistant_sessions_purge_after; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_assistant_sessions_purge_after ON public.assistant_sessions USING btree (purge_after) WHERE ((lifecycle = 'archived'::text) AND (purged_at IS NULL));


--
-- Name: idx_assistant_sessions_user_lifecycle_updated; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_assistant_sessions_user_lifecycle_updated ON public.assistant_sessions USING btree (user_id, lifecycle, updated_at DESC);


--
-- Name: idx_audit_event_actor_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_audit_event_actor_id ON public.audit_event USING btree (actor_id);


--
-- Name: idx_audit_event_actor_kind; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_audit_event_actor_kind ON public.audit_event USING btree (actor_kind, created_at DESC);


--
-- Name: idx_audit_event_created_at; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_audit_event_created_at ON public.audit_event USING btree (created_at);


--
-- Name: idx_audit_event_event_type; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_audit_event_event_type ON public.audit_event USING btree (event_type);


--
-- Name: idx_audit_event_target; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_audit_event_target ON public.audit_event USING btree (target_type, target_id);


--
-- Name: idx_auth_oidc_settings_enabled; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_auth_oidc_settings_enabled ON public.auth_oidc_settings USING btree (enabled);


--
-- Name: idx_auth_session_expires_at; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_auth_session_expires_at ON public.auth_session USING btree (expires_at);


--
-- Name: idx_auth_session_user_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_auth_session_user_id ON public.auth_session USING btree (user_id);


--
-- Name: idx_auth_user_api_token_expires_at; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_auth_user_api_token_expires_at ON public.auth_user_api_token USING btree (expires_at);


--
-- Name: idx_auth_user_api_token_scope_token_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_auth_user_api_token_scope_token_id ON public.auth_user_api_token_scope USING btree (token_id);


--
-- Name: idx_auth_user_api_token_status; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_auth_user_api_token_status ON public.auth_user_api_token USING btree (status);


--
-- Name: idx_auth_user_api_token_status_expires; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_auth_user_api_token_status_expires ON public.auth_user_api_token USING btree (status, expires_at);


--
-- Name: idx_auth_user_api_token_user_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_auth_user_api_token_user_id ON public.auth_user_api_token USING btree (user_id);


--
-- Name: idx_config_context_binding_scope; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_config_context_binding_scope ON public.config_context_binding USING btree (scope_type, scope_id);


--
-- Name: idx_config_context_binding_unique; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX idx_config_context_binding_unique ON public.config_context_binding USING btree (context_id, scope_type, scope_id);


--
-- Name: idx_config_context_name; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_config_context_name ON public.config_context USING btree (name);


--
-- Name: idx_connconf_assignment_record; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_connconf_assignment_record ON public.connconf_assignment USING btree (record_id);


--
-- Name: idx_connconf_assignment_scope; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_connconf_assignment_scope ON public.connconf_assignment USING btree (scope_type, scope_id) WHERE enabled;


--
-- Name: idx_connconf_record_base; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_connconf_record_base ON public.connconf_record USING btree (base_record_id);


--
-- Name: idx_connconf_record_owner; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_connconf_record_owner ON public.connconf_record USING btree (owner_user_id);


--
-- Name: idx_connconf_record_protocol_enabled; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_connconf_record_protocol_enabled ON public.connconf_record USING btree (protocol, enabled);


--
-- Name: idx_connection_filter_presets_updated_at; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_connection_filter_presets_updated_at ON public.connection_filter_presets USING btree (updated_at DESC);


--
-- Name: idx_connections_proxy_assignment_proxy_node; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_connections_proxy_assignment_proxy_node ON public.connections_proxy_assignment USING btree (proxy_node_id);


--
-- Name: idx_connections_proxy_assignment_scope; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_connections_proxy_assignment_scope ON public.connections_proxy_assignment USING btree (scope_type, scope_id) WHERE enabled;


--
-- Name: idx_connections_proxy_node_enabled; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_connections_proxy_node_enabled ON public.connections_proxy_node USING btree (enabled);


--
-- Name: idx_connections_session_last_activity; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_connections_session_last_activity ON public.connections_session USING btree (last_activity);


--
-- Name: idx_connections_session_protocol; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_connections_session_protocol ON public.connections_session USING btree (protocol);


--
-- Name: idx_connections_session_proxy_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_connections_session_proxy_id ON public.connections_session USING btree (proxy_id);


--
-- Name: idx_connections_session_proxy_node_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_connections_session_proxy_node_id ON public.connections_session USING btree (proxy_node_id);


--
-- Name: idx_connections_session_routing_source; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_connections_session_routing_source ON public.connections_session USING btree (routing_source);


--
-- Name: idx_connections_session_started_at; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_connections_session_started_at ON public.connections_session USING btree (started_at);


--
-- Name: idx_connections_session_status; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_connections_session_status ON public.connections_session USING btree (status);


--
-- Name: idx_connections_session_target_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_connections_session_target_id ON public.connections_session USING btree (target_id);


--
-- Name: idx_connections_session_user_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_connections_session_user_id ON public.connections_session USING btree (user_id);


--
-- Name: idx_connections_template_name; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_connections_template_name ON public.connections_template USING btree (name);


--
-- Name: idx_connections_template_protocol; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_connections_template_protocol ON public.connections_template USING btree (protocol);


--
-- Name: idx_connections_user_credential_set_default; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX idx_connections_user_credential_set_default ON public.connections_user_credential_set USING btree (user_id, protocol) WHERE (is_default = true);


--
-- Name: idx_connections_user_credential_set_user_protocol; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_connections_user_credential_set_user_protocol ON public.connections_user_credential_set USING btree (user_id, protocol, updated_at DESC);


--
-- Name: idx_entity_mappings_configuration; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_entity_mappings_configuration ON public.entity_mappings USING btree (mapping_config_logical_id, mapping_config_version_id, created_at, id);


--
-- Name: idx_field_mappings_entity; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_field_mappings_entity ON public.field_mappings USING btree (entity_mapping_id, created_at, id);


--
-- Name: idx_identity_group_external_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_identity_group_external_id ON public.identity_group USING btree (external_id);


--
-- Name: idx_identity_group_membership_group_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_identity_group_membership_group_id ON public.identity_group_membership USING btree (group_id);


--
-- Name: idx_identity_group_membership_source_external_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_identity_group_membership_source_external_id ON public.identity_group_membership USING btree (source, external_id);


--
-- Name: idx_identity_group_membership_user_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_identity_group_membership_user_id ON public.identity_group_membership USING btree (user_id);


--
-- Name: idx_identity_group_name; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_identity_group_name ON public.identity_group USING btree (name);


--
-- Name: idx_identity_user_email; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_identity_user_email ON public.identity_user USING btree (email);


--
-- Name: idx_identity_user_external_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_identity_user_external_id ON public.identity_user USING btree (external_id);


--
-- Name: idx_identity_user_username; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_identity_user_username ON public.identity_user USING btree (username);


--
-- Name: idx_inventory_layout_system_profile_entity_type; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_inventory_layout_system_profile_entity_type ON public.inventory_layout_system_profile USING btree (entity_type);


--
-- Name: idx_inventory_layout_user_profile_entity_type; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_inventory_layout_user_profile_entity_type ON public.inventory_layout_user_profile USING btree (entity_type);


--
-- Name: idx_inventory_layout_user_profile_user_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_inventory_layout_user_profile_user_id ON public.inventory_layout_user_profile USING btree (user_id);


--
-- Name: idx_inventory_tag_external_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_inventory_tag_external_id ON public.inventory_tag USING btree (external_id);


--
-- Name: idx_inventory_template_owner_user_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_inventory_template_owner_user_id ON public.inventory_template USING btree (owner_user_id);


--
-- Name: idx_inventory_template_run_requested_by; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_inventory_template_run_requested_by ON public.inventory_template_run USING btree (requested_by_user_id);


--
-- Name: idx_inventory_template_run_status; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_inventory_template_run_status ON public.inventory_template_run USING btree (status);


--
-- Name: idx_inventory_template_run_step_result_run_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_inventory_template_run_step_result_run_id ON public.inventory_template_run_step_result USING btree (run_id);


--
-- Name: idx_inventory_template_run_template_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_inventory_template_run_template_id ON public.inventory_template_run USING btree (template_id);


--
-- Name: idx_inventory_template_status; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_inventory_template_status ON public.inventory_template USING btree (status);


--
-- Name: idx_inventory_template_step_template_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_inventory_template_step_template_id ON public.inventory_template_step USING btree (template_id);


--
-- Name: idx_inventory_template_updated_at; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_inventory_template_updated_at ON public.inventory_template USING btree (updated_at DESC);


--
-- Name: idx_inventory_template_variable_template_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_inventory_template_variable_template_id ON public.inventory_template_variable USING btree (template_id);


--
-- Name: idx_ipam_service_port_mapping_service; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_ipam_service_port_mapping_service ON public.ipam_service_port_mapping USING btree (service_id);


--
-- Name: idx_license_seat_assignment_active; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_license_seat_assignment_active ON public.license_seat_assignment USING btree (revoked_at, assigned_at);


--
-- Name: idx_license_seat_ledger_created_at; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_license_seat_ledger_created_at ON public.license_seat_ledger USING btree (created_at DESC);


--
-- Name: idx_license_seat_ledger_instance_event; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_license_seat_ledger_instance_event ON public.license_seat_ledger USING btree (instance_id, event_type, created_at DESC);


--
-- Name: idx_license_seat_ledger_user_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_license_seat_ledger_user_id ON public.license_seat_ledger USING btree (user_id, created_at DESC);


--
-- Name: idx_license_state_entitlement_expires_at; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_license_state_entitlement_expires_at ON public.license_state USING btree (entitlement_expires_at);


--
-- Name: idx_license_state_last_sync_at; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_license_state_last_sync_at ON public.license_state USING btree (last_sync_at);


--
-- Name: idx_license_state_status; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_license_state_status ON public.license_state USING btree (status);


--
-- Name: idx_license_state_status_grace_ends_at; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_license_state_status_grace_ends_at ON public.license_state USING btree (status, grace_ends_at);


--
-- Name: idx_mapping_configurations_provider_pair_active; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_mapping_configurations_provider_pair_active ON public.mapping_configurations USING btree (source_provider_id, target_provider_id, version_id DESC) WHERE ((status)::text = 'active'::text);


--
-- Name: idx_mapping_configurations_source_provider; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_mapping_configurations_source_provider ON public.mapping_configurations USING btree (source_provider_id, logical_id, version_id DESC);


--
-- Name: idx_mapping_configurations_target_provider; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_mapping_configurations_target_provider ON public.mapping_configurations USING btree (target_provider_id, logical_id, version_id DESC);


--
-- Name: idx_monitoring_alert_source_assignment_scope; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_monitoring_alert_source_assignment_scope ON public.monitoring_alert_source_assignment USING btree (scope_type, scope_id, enabled, weight);


--
-- Name: idx_monitoring_alert_source_type; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_monitoring_alert_source_type ON public.monitoring_alert_source USING btree (source_type, enabled);


--
-- Name: idx_monitoring_exporter_assignment_exporter; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_monitoring_exporter_assignment_exporter ON public.monitoring_exporter_assignment USING btree (exporter_id);


--
-- Name: idx_monitoring_exporter_assignment_scope; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_monitoring_exporter_assignment_scope ON public.monitoring_exporter_assignment USING btree (scope_type, scope_id, enabled, weight);


--
-- Name: idx_monitoring_metric_query_category; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_monitoring_metric_query_category ON public.monitoring_metric_query USING btree (category, sort_order);


--
-- Name: idx_monitoring_metric_query_exporter; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_monitoring_metric_query_exporter ON public.monitoring_metric_query USING btree (exporter_id, enabled, sort_order);


--
-- Name: idx_monitoring_metric_query_host_override_host; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_monitoring_metric_query_host_override_host ON public.monitoring_metric_query_host_override USING btree (host_id, enabled);


--
-- Name: idx_monitoring_metric_query_provider_slug; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX idx_monitoring_metric_query_provider_slug ON public.monitoring_metric_query USING btree (source_type, slug) WHERE (exporter_id IS NULL);


--
-- Name: idx_monitoring_metric_query_source; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_monitoring_metric_query_source ON public.monitoring_metric_query USING btree (source_type, enabled, sort_order);


--
-- Name: idx_monitoring_metric_set_assignment_scope; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_monitoring_metric_set_assignment_scope ON public.monitoring_metric_set_assignment USING btree (scope_type, scope_id, enabled, weight);


--
-- Name: idx_monitoring_metric_set_item_set; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_monitoring_metric_set_item_set ON public.monitoring_metric_set_item USING btree (metric_set_id, sort_order);


--
-- Name: idx_monitoring_query_source_assignment_scope; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_monitoring_query_source_assignment_scope ON public.monitoring_query_source_assignment USING btree (scope_type, scope_id, enabled, weight);


--
-- Name: idx_monitoring_query_source_type; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_monitoring_query_source_type ON public.monitoring_query_source USING btree (source_type, enabled);


--
-- Name: idx_playbook_artifact_run_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_playbook_artifact_run_created ON public.playbook_artifact USING btree (run_id, created_at);


--
-- Name: idx_playbook_assignment_quick_action; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_playbook_assignment_quick_action ON public.playbook_assignment USING btree (scope_kind, scope_id, priority DESC) WHERE (enabled AND visible AND executable AND quick_action_enabled);


--
-- Name: idx_playbook_assignment_scope; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_playbook_assignment_scope ON public.playbook_assignment USING btree (scope_kind, scope_id, priority DESC) WHERE enabled;


--
-- Name: idx_playbook_assignment_visible; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_playbook_assignment_visible ON public.playbook_assignment USING btree (scope_kind, scope_id, priority DESC) WHERE (enabled AND visible);


--
-- Name: idx_playbook_execution_environment_name_lower; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX idx_playbook_execution_environment_name_lower ON public.playbook_execution_environment USING btree (lower(name));


--
-- Name: idx_playbook_job_template_name_lower; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX idx_playbook_job_template_name_lower ON public.playbook_job_template USING btree (lower(name));


--
-- Name: idx_playbook_job_template_playbook_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_playbook_job_template_playbook_id ON public.playbook_job_template USING btree (playbook_id);


--
-- Name: idx_playbook_job_template_project_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_playbook_job_template_project_id ON public.playbook_job_template USING btree (project_id);


--
-- Name: idx_playbook_job_template_target_set_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_playbook_job_template_target_set_id ON public.playbook_job_template USING btree (inventory_target_set_id);


--
-- Name: idx_playbook_job_token_active_expiry; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_playbook_job_token_active_expiry ON public.playbook_job_token USING btree (expires_at) WHERE (status = 'active'::text);


--
-- Name: idx_playbook_job_token_run; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_playbook_job_token_run ON public.playbook_job_token USING btree (run_id);


--
-- Name: idx_playbook_job_token_run_target; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_playbook_job_token_run_target ON public.playbook_job_token USING btree (run_id, target_kind, target_id, source_kind, source_id);


--
-- Name: idx_playbook_project_name_lower; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX idx_playbook_project_name_lower ON public.playbook_project USING btree (lower(name));


--
-- Name: idx_playbook_project_sync_status; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_playbook_project_sync_status ON public.playbook_project USING btree (sync_status);


--
-- Name: idx_playbook_run_credential_account; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_playbook_run_credential_account ON public.playbook_run USING btree (credential_account_kind, credential_account_id) WHERE (credential_account_id IS NOT NULL);


--
-- Name: idx_playbook_run_event_group_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_playbook_run_event_group_created ON public.playbook_run_event USING btree (run_group_id, created_at);


--
-- Name: idx_playbook_run_event_group_host_sequence; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_playbook_run_event_group_host_sequence ON public.playbook_run_event USING btree (run_group_id, host, event_sequence);


--
-- Name: idx_playbook_run_event_run_sequence; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_playbook_run_event_run_sequence ON public.playbook_run_event USING btree (run_id, event_sequence);


--
-- Name: idx_playbook_run_group_callback_token_active_expiry; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_playbook_run_group_callback_token_active_expiry ON public.playbook_run_group_callback_token USING btree (expires_at) WHERE (status = 'active'::text);


--
-- Name: idx_playbook_run_group_callback_token_group; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_playbook_run_group_callback_token_group ON public.playbook_run_group_callback_token USING btree (run_group_id);


--
-- Name: idx_playbook_run_group_pending_cancel; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_playbook_run_group_pending_cancel ON public.playbook_run_group USING btree (cancel_requested_at, id) WHERE ((cancel_requested_at IS NOT NULL) AND (cancel_outcome IS NULL) AND (dispatch_id IS NOT NULL));


--
-- Name: idx_playbook_run_group_schedule; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_playbook_run_group_schedule ON public.playbook_run_group USING btree (schedule_id, created_at DESC) WHERE (schedule_id IS NOT NULL);


--
-- Name: idx_playbook_run_group_status_lease; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_playbook_run_group_status_lease ON public.playbook_run_group USING btree (status, lease_expires_at);


--
-- Name: idx_playbook_run_group_status_updated; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_playbook_run_group_status_updated ON public.playbook_run_group USING btree (status, updated_at DESC);


--
-- Name: idx_playbook_run_status_lease; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_playbook_run_status_lease ON public.playbook_run USING btree (status, lease_expires_at);


--
-- Name: idx_playbook_run_target; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_playbook_run_target ON public.playbook_run USING btree (target_kind, target_id, source_kind, source_id, queued_at DESC);


--
-- Name: idx_playbook_schedule_due; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_playbook_schedule_due ON public.playbook_schedule USING btree (next_run_at) WHERE enabled;


--
-- Name: idx_playbook_schedule_occurrence_lease; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_playbook_schedule_occurrence_lease ON public.playbook_schedule_occurrence USING btree (lease_expires_at, scheduled_for);


--
-- Name: idx_playbook_schedule_playbook; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_playbook_schedule_playbook ON public.playbook_schedule USING btree (playbook_id, enabled, next_run_at);


--
-- Name: idx_playbook_schedule_service_principal; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_playbook_schedule_service_principal ON public.playbook_schedule USING btree (service_principal_id, enabled);


--
-- Name: idx_playbook_service_principal_credential_principal; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_playbook_service_principal_credential_principal ON public.playbook_service_principal_credential_binding USING btree (service_principal_id, credential_account_kind);


--
-- Name: idx_playbook_service_principal_status_updated; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_playbook_service_principal_status_updated ON public.playbook_service_principal USING btree (status, updated_at DESC);


--
-- Name: idx_playbook_status_updated; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_playbook_status_updated ON public.playbook USING btree (status, updated_at DESC);


--
-- Name: idx_playbook_target_set_member_target; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_playbook_target_set_member_target ON public.playbook_target_set_member USING btree (target_kind, target_id, source_kind, source_id);


--
-- Name: idx_playbook_target_set_mode; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_playbook_target_set_mode ON public.playbook_target_set USING btree (mode, updated_at DESC);


--
-- Name: idx_playbook_version_playbook_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_playbook_version_playbook_created ON public.playbook_version USING btree (playbook_id, version DESC);


--
-- Name: idx_provider_request_journal_echo_dedup; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_provider_request_journal_echo_dedup ON public.provider_request_journal USING btree (provider_id, model, entity_identity, payload_hash, created_at DESC);


--
-- Name: idx_provider_request_journal_hash_window; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_provider_request_journal_hash_window ON public.provider_request_journal USING btree (provider_id, model, payload_hash, created_at DESC);


--
-- Name: idx_provider_request_journal_outbound_event; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_provider_request_journal_outbound_event ON public.provider_request_journal USING btree (provider_id, outbound_event_uuid);


--
-- Name: idx_provider_request_journal_prune; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_provider_request_journal_prune ON public.provider_request_journal USING btree (state, created_at);


--
-- Name: idx_rbac_policy_name; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_rbac_policy_name ON public.rbac_policy USING btree (name);


--
-- Name: idx_rbac_policy_resources_gin; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_rbac_policy_resources_gin ON public.rbac_policy USING gin (resources);


--
-- Name: idx_rbac_policy_subjects_gin; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_rbac_policy_subjects_gin ON public.rbac_policy USING gin (subjects);


--
-- Name: idx_rbac_role_assignment_group_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_rbac_role_assignment_group_id ON public.rbac_role_assignment USING btree (group_id);


--
-- Name: idx_rbac_role_assignment_role_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_rbac_role_assignment_role_id ON public.rbac_role_assignment USING btree (role_id);


--
-- Name: idx_rbac_role_assignment_user_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_rbac_role_assignment_user_id ON public.rbac_role_assignment USING btree (user_id);


--
-- Name: idx_rbac_role_name; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_rbac_role_name ON public.rbac_role USING btree (name);


--
-- Name: idx_secret_usage_binding_consumer; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_secret_usage_binding_consumer ON public.secret_usage_binding USING btree (consumer_type, consumer_id);


--
-- Name: idx_secret_usage_binding_owner; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_secret_usage_binding_owner ON public.secret_usage_binding USING btree (owner_scope_type, owner_scope_id);


--
-- Name: idx_secret_usage_binding_secret; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_secret_usage_binding_secret ON public.secret_usage_binding USING btree (secret_id);


--
-- Name: idx_secrets_binding_scope; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_secrets_binding_scope ON public.secrets_binding USING btree (scope_type, scope_id);


--
-- Name: idx_secrets_binding_secret_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_secrets_binding_secret_id ON public.secrets_binding USING btree (secret_id);


--
-- Name: idx_secrets_variable_key_unique; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX idx_secrets_variable_key_unique ON public.secrets_variable USING btree (key);


--
-- Name: idx_settings_values_updated_at; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_settings_values_updated_at ON public.settings_values USING btree (updated_at DESC);


--
-- Name: idx_settings_values_user_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_settings_values_user_id ON public.settings_values USING btree (user_id) WHERE (scope_type = 'user'::text);


--
-- Name: idx_sync_conflict_record_created_at; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_sync_conflict_record_created_at ON public.sync_conflict_record USING btree (created_at DESC);


--
-- Name: idx_sync_conflict_record_source_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_sync_conflict_record_source_id ON public.sync_conflict_record USING btree (source_id);


--
-- Name: idx_sync_error_run; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_sync_error_run ON public.sync_error USING btree (run_id);


--
-- Name: idx_sync_inventory_binding_last_seen_job; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_sync_inventory_binding_last_seen_job ON public.sync_inventory_binding USING btree (last_seen_job_id);


--
-- Name: idx_sync_job_provider; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_sync_job_provider ON public.sync_job USING btree (provider_id);


--
-- Name: idx_sync_job_step_job; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_sync_job_step_job ON public.sync_job_step USING btree (job_id);


--
-- Name: idx_sync_outbound_event_source_status_schedule; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_sync_outbound_event_source_status_schedule ON public.sync_outbound_event USING btree (source_id, status, next_attempt_at);


--
-- Name: idx_sync_outbound_event_status_schedule; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_sync_outbound_event_status_schedule ON public.sync_outbound_event USING btree (status, next_attempt_at, created_at);


--
-- Name: idx_sync_provider_type; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_sync_provider_type ON public.sync_provider USING btree (provider_type);


--
-- Name: idx_sync_run_job; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_sync_run_job ON public.sync_run USING btree (job_id);


--
-- Name: idx_sync_source_runtime_bootstrap_status; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_sync_source_runtime_bootstrap_status ON public.sync_source_runtime USING btree (bootstrap_status);


--
-- Name: idx_sync_source_runtime_operational_status; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_sync_source_runtime_operational_status ON public.sync_source_runtime USING btree (operational_status);


--
-- Name: idx_sync_webhook_event_received_at; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_sync_webhook_event_received_at ON public.sync_webhook_event USING btree (received_at DESC);


--
-- Name: idx_sync_webhook_event_source_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_sync_webhook_event_source_id ON public.sync_webhook_event USING btree (source_id);


--
-- Name: idx_sync_webhook_event_status; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_sync_webhook_event_status ON public.sync_webhook_event USING btree (status);


--
-- Name: idx_user_preferences_user_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_user_preferences_user_id ON public.user_preferences USING btree (user_id);


--
-- Name: idx_wizard_progress_user_type; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX idx_wizard_progress_user_type ON public.wizard_progress USING btree (user_id, wizard_type);


--
-- Name: inventory_dataflow_destination_cluster_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX inventory_dataflow_destination_cluster_id ON public.inventory_dataflow USING btree (destination_cluster_id);


--
-- Name: inventory_dataflow_destination_clustergroup_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX inventory_dataflow_destination_clustergroup_id ON public.inventory_dataflow USING btree (destination_clustergroup_id);


--
-- Name: inventory_dataflow_destination_clustertype_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX inventory_dataflow_destination_clustertype_id ON public.inventory_dataflow USING btree (destination_clustertype_id);


--
-- Name: inventory_dataflow_destination_device; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX inventory_dataflow_destination_device ON public.inventory_dataflow USING btree (destination_device_id);


--
-- Name: inventory_dataflow_destination_devicerole_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX inventory_dataflow_destination_devicerole_id ON public.inventory_dataflow USING btree (destination_devicerole_id);


--
-- Name: inventory_dataflow_destination_devicetype_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX inventory_dataflow_destination_devicetype_id ON public.inventory_dataflow USING btree (destination_devicetype_id);


--
-- Name: inventory_dataflow_destination_ipaddress; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX inventory_dataflow_destination_ipaddress ON public.inventory_dataflow USING btree (destination_ipaddress_id);


--
-- Name: inventory_dataflow_destination_location_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX inventory_dataflow_destination_location_id ON public.inventory_dataflow USING btree (destination_location_id);


--
-- Name: inventory_dataflow_destination_manufacturer_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX inventory_dataflow_destination_manufacturer_id ON public.inventory_dataflow USING btree (destination_manufacturer_id);


--
-- Name: inventory_dataflow_destination_platform_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX inventory_dataflow_destination_platform_id ON public.inventory_dataflow USING btree (destination_platform_id);


--
-- Name: inventory_dataflow_destination_prefix; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX inventory_dataflow_destination_prefix ON public.inventory_dataflow USING btree (destination_prefix_id);


--
-- Name: inventory_dataflow_destination_rack_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX inventory_dataflow_destination_rack_id ON public.inventory_dataflow USING btree (destination_rack_id);


--
-- Name: inventory_dataflow_destination_region_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX inventory_dataflow_destination_region_id ON public.inventory_dataflow USING btree (destination_region_id);


--
-- Name: inventory_dataflow_destination_service; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX inventory_dataflow_destination_service ON public.inventory_dataflow USING btree (destination_service_id);


--
-- Name: inventory_dataflow_destination_site_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX inventory_dataflow_destination_site_id ON public.inventory_dataflow USING btree (destination_site_id);


--
-- Name: inventory_dataflow_destination_sitegroup_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX inventory_dataflow_destination_sitegroup_id ON public.inventory_dataflow USING btree (destination_sitegroup_id);


--
-- Name: inventory_dataflow_destination_tenant_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX inventory_dataflow_destination_tenant_id ON public.inventory_dataflow USING btree (destination_tenant_id);


--
-- Name: inventory_dataflow_destination_tenantgroup_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX inventory_dataflow_destination_tenantgroup_id ON public.inventory_dataflow USING btree (destination_tenantgroup_id);


--
-- Name: inventory_dataflow_destination_vm; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX inventory_dataflow_destination_vm ON public.inventory_dataflow USING btree (destination_vm_id);


--
-- Name: inventory_dataflow_selected_ip_address; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX inventory_dataflow_selected_ip_address ON public.inventory_dataflow_selected_ip USING btree (ipaddress_id);


--
-- Name: inventory_dataflow_source_cluster_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX inventory_dataflow_source_cluster_id ON public.inventory_dataflow USING btree (source_cluster_id);


--
-- Name: inventory_dataflow_source_clustergroup_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX inventory_dataflow_source_clustergroup_id ON public.inventory_dataflow USING btree (source_clustergroup_id);


--
-- Name: inventory_dataflow_source_clustertype_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX inventory_dataflow_source_clustertype_id ON public.inventory_dataflow USING btree (source_clustertype_id);


--
-- Name: inventory_dataflow_source_device; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX inventory_dataflow_source_device ON public.inventory_dataflow USING btree (source_device_id);


--
-- Name: inventory_dataflow_source_devicerole_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX inventory_dataflow_source_devicerole_id ON public.inventory_dataflow USING btree (source_devicerole_id);


--
-- Name: inventory_dataflow_source_devicetype_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX inventory_dataflow_source_devicetype_id ON public.inventory_dataflow USING btree (source_devicetype_id);


--
-- Name: inventory_dataflow_source_ipaddress; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX inventory_dataflow_source_ipaddress ON public.inventory_dataflow USING btree (source_ipaddress_id);


--
-- Name: inventory_dataflow_source_location_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX inventory_dataflow_source_location_id ON public.inventory_dataflow USING btree (source_location_id);


--
-- Name: inventory_dataflow_source_manufacturer_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX inventory_dataflow_source_manufacturer_id ON public.inventory_dataflow USING btree (source_manufacturer_id);


--
-- Name: inventory_dataflow_source_platform_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX inventory_dataflow_source_platform_id ON public.inventory_dataflow USING btree (source_platform_id);


--
-- Name: inventory_dataflow_source_prefix; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX inventory_dataflow_source_prefix ON public.inventory_dataflow USING btree (source_prefix_id);


--
-- Name: inventory_dataflow_source_rack_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX inventory_dataflow_source_rack_id ON public.inventory_dataflow USING btree (source_rack_id);


--
-- Name: inventory_dataflow_source_region_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX inventory_dataflow_source_region_id ON public.inventory_dataflow USING btree (source_region_id);


--
-- Name: inventory_dataflow_source_service; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX inventory_dataflow_source_service ON public.inventory_dataflow USING btree (source_service_id);


--
-- Name: inventory_dataflow_source_site_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX inventory_dataflow_source_site_id ON public.inventory_dataflow USING btree (source_site_id);


--
-- Name: inventory_dataflow_source_sitegroup_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX inventory_dataflow_source_sitegroup_id ON public.inventory_dataflow USING btree (source_sitegroup_id);


--
-- Name: inventory_dataflow_source_tenant_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX inventory_dataflow_source_tenant_id ON public.inventory_dataflow USING btree (source_tenant_id);


--
-- Name: inventory_dataflow_source_tenantgroup_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX inventory_dataflow_source_tenantgroup_id ON public.inventory_dataflow USING btree (source_tenantgroup_id);


--
-- Name: inventory_dataflow_source_vm; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX inventory_dataflow_source_vm ON public.inventory_dataflow USING btree (source_vm_id);


--
-- Name: inventory_dataflow_status_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX inventory_dataflow_status_id ON public.inventory_dataflow USING btree (status, id);


--
-- Name: ipam_aggregate_owner_id_fdaa939f; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_aggregate_owner_id_fdaa939f ON public.ipam_aggregate USING btree (owner_id);


--
-- Name: ipam_aggregate_rir_id_ef7a27bd; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_aggregate_rir_id_ef7a27bd ON public.ipam_aggregate USING btree (rir_id);


--
-- Name: ipam_aggregate_tenant_id_637dd1a1; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_aggregate_tenant_id_637dd1a1 ON public.ipam_aggregate USING btree (tenant_id);


--
-- Name: ipam_asn_owner_id_2ab253b3; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_asn_owner_id_2ab253b3 ON public.ipam_asn USING btree (owner_id);


--
-- Name: ipam_asn_rir_id_f5ad3cff; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_asn_rir_id_f5ad3cff ON public.ipam_asn USING btree (rir_id);


--
-- Name: ipam_asn_tenant_id_07e8188e; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_asn_tenant_id_07e8188e ON public.ipam_asn USING btree (tenant_id);


--
-- Name: ipam_asnrange_name_c7585e73_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_asnrange_name_c7585e73_like ON public.ipam_asnrange USING btree (name varchar_pattern_ops);


--
-- Name: ipam_asnrange_owner_id_a943e984; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_asnrange_owner_id_a943e984 ON public.ipam_asnrange USING btree (owner_id);


--
-- Name: ipam_asnrange_rir_id_c9c31183; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_asnrange_rir_id_c9c31183 ON public.ipam_asnrange USING btree (rir_id);


--
-- Name: ipam_asnrange_slug_c8a7d8a1_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_asnrange_slug_c8a7d8a1_like ON public.ipam_asnrange USING btree (slug varchar_pattern_ops);


--
-- Name: ipam_asnrange_tenant_id_ed8f80b7; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_asnrange_tenant_id_ed8f80b7 ON public.ipam_asnrange USING btree (tenant_id);


--
-- Name: ipam_fhrpgroup_owner_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_fhrpgroup_owner_id ON public.ipam_fhrpgroup USING btree (owner_id);


--
-- Name: ipam_fhrpgroup_protocol_group_id_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_fhrpgroup_protocol_group_id_id ON public.ipam_fhrpgroup USING btree (protocol, group_id, id);


--
-- Name: ipam_fhrpgroupassignment_group_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_fhrpgroupassignment_group_id ON public.ipam_fhrpgroupassignment USING btree (group_id);


--
-- Name: ipam_fhrpgroupassignment_interface_type_interface_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_fhrpgroupassignment_interface_type_interface_id ON public.ipam_fhrpgroupassignment USING btree (interface_type_id, interface_id);


--
-- Name: ipam_fhrpgroupassignment_priority_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_fhrpgroupassignment_priority_id ON public.ipam_fhrpgroupassignment USING btree (priority DESC, id);


--
-- Name: ipam_ipaddr_assigne_890ab8_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_ipaddr_assigne_890ab8_idx ON public.ipam_ipaddress USING btree (assigned_object_type_id, assigned_object_id);


--
-- Name: ipam_ipaddress_address; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_ipaddress_address ON public.ipam_ipaddress USING btree (address);


--
-- Name: ipam_ipaddress_assigned_object_type_id_02354370; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_ipaddress_assigned_object_type_id_02354370 ON public.ipam_ipaddress USING btree (assigned_object_type_id);


--
-- Name: ipam_ipaddress_host; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_ipaddress_host ON public.ipam_ipaddress USING btree (((host(address))::inet));


--
-- Name: ipam_ipaddress_host_address_gist; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_ipaddress_host_address_gist ON public.ipam_ipaddress USING gist (((host(address))::inet) inet_ops);


--
-- Name: ipam_ipaddress_nat_inside_id_a45fb7c5; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_ipaddress_nat_inside_id_a45fb7c5 ON public.ipam_ipaddress USING btree (nat_inside_id);


--
-- Name: ipam_ipaddress_owner_id_47736b63; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_ipaddress_owner_id_47736b63 ON public.ipam_ipaddress USING btree (owner_id);


--
-- Name: ipam_ipaddress_tenant_id_ac55acfd; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_ipaddress_tenant_id_ac55acfd ON public.ipam_ipaddress USING btree (tenant_id);


--
-- Name: ipam_ipaddress_vrf_id_51fcc59b; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_ipaddress_vrf_id_51fcc59b ON public.ipam_ipaddress USING btree (vrf_id);


--
-- Name: ipam_iprange_owner_id_b327afb7; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_iprange_owner_id_b327afb7 ON public.ipam_iprange USING btree (owner_id);


--
-- Name: ipam_iprange_role_id_2782e864; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_iprange_role_id_2782e864 ON public.ipam_iprange USING btree (role_id);


--
-- Name: ipam_iprange_tenant_id_856027ea; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_iprange_tenant_id_856027ea ON public.ipam_iprange USING btree (tenant_id);


--
-- Name: ipam_iprange_vrf_id_613e9dd2; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_iprange_vrf_id_613e9dd2 ON public.ipam_iprange USING btree (vrf_id);


--
-- Name: ipam_l2vpn_export_targets_l2vpn_id_8749bbe8; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_l2vpn_export_targets_l2vpn_id_8749bbe8 ON public.vpn_l2vpn_export_targets USING btree (l2vpn_id);


--
-- Name: ipam_l2vpn_export_targets_routetarget_id_5ccb758b; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_l2vpn_export_targets_routetarget_id_5ccb758b ON public.vpn_l2vpn_export_targets USING btree (routetarget_id);


--
-- Name: ipam_l2vpn_import_targets_l2vpn_id_731f5bb4; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_l2vpn_import_targets_l2vpn_id_731f5bb4 ON public.vpn_l2vpn_import_targets USING btree (l2vpn_id);


--
-- Name: ipam_l2vpn_import_targets_routetarget_id_58a188b2; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_l2vpn_import_targets_routetarget_id_58a188b2 ON public.vpn_l2vpn_import_targets USING btree (routetarget_id);


--
-- Name: ipam_prefix__location_id_f5925c42; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_prefix__location_id_f5925c42 ON public.ipam_prefix USING btree (_location_id);


--
-- Name: ipam_prefix__region_id_54aedc72; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_prefix__region_id_54aedc72 ON public.ipam_prefix USING btree (_region_id);


--
-- Name: ipam_prefix__site_group_id_8277120b; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_prefix__site_group_id_8277120b ON public.ipam_prefix USING btree (_site_group_id);


--
-- Name: ipam_prefix__site_id_b479fb05; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_prefix__site_id_b479fb05 ON public.ipam_prefix USING btree (_site_id);


--
-- Name: ipam_prefix_gist_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_prefix_gist_idx ON public.ipam_prefix USING gist (prefix inet_ops);


--
-- Name: ipam_prefix_owner_id_12d43bc8; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_prefix_owner_id_12d43bc8 ON public.ipam_prefix USING btree (owner_id);


--
-- Name: ipam_prefix_role_id_0a98d415; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_prefix_role_id_0a98d415 ON public.ipam_prefix USING btree (role_id);


--
-- Name: ipam_prefix_scope_t_fe84a6_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_prefix_scope_t_fe84a6_idx ON public.ipam_prefix USING btree (scope_type_id, scope_id);


--
-- Name: ipam_prefix_scope_type_id_413319e2; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_prefix_scope_type_id_413319e2 ON public.ipam_prefix USING btree (scope_type_id);


--
-- Name: ipam_prefix_tenant_id_7ba1fcc4; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_prefix_tenant_id_7ba1fcc4 ON public.ipam_prefix USING btree (tenant_id);


--
-- Name: ipam_prefix_vlan_id_1db91bff; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_prefix_vlan_id_1db91bff ON public.ipam_prefix USING btree (vlan_id);


--
-- Name: ipam_prefix_vrf_id_34f78ed0; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_prefix_vrf_id_34f78ed0 ON public.ipam_prefix USING btree (vrf_id);


--
-- Name: ipam_rir_name_64a71982_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_rir_name_64a71982_like ON public.ipam_rir USING btree (name varchar_pattern_ops);


--
-- Name: ipam_rir_owner_id_172cc053; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_rir_owner_id_172cc053 ON public.ipam_rir USING btree (owner_id);


--
-- Name: ipam_rir_slug_ff1a369a_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_rir_slug_ff1a369a_like ON public.ipam_rir USING btree (slug varchar_pattern_ops);


--
-- Name: ipam_role_name_13784849_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_role_name_13784849_like ON public.ipam_role USING btree (name varchar_pattern_ops);


--
-- Name: ipam_role_owner_id_b42367ab; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_role_owner_id_b42367ab ON public.ipam_role USING btree (owner_id);


--
-- Name: ipam_role_slug_309ca14c_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_role_slug_309ca14c_like ON public.ipam_role USING btree (slug varchar_pattern_ops);


--
-- Name: ipam_routetarget_name_212be79f_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_routetarget_name_212be79f_like ON public.ipam_routetarget USING btree (name varchar_pattern_ops);


--
-- Name: ipam_routetarget_owner_id_e968953a; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_routetarget_owner_id_e968953a ON public.ipam_routetarget USING btree (owner_id);


--
-- Name: ipam_routetarget_tenant_id_5a0b35e8; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_routetarget_tenant_id_5a0b35e8 ON public.ipam_routetarget USING btree (tenant_id);


--
-- Name: ipam_servic_parent__563d2b_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_servic_parent__563d2b_idx ON public.ipam_service USING btree (parent_object_type_id, parent_object_id);


--
-- Name: ipam_service_ipaddresses_ipaddress_id_b4138c6d; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_service_ipaddresses_ipaddress_id_b4138c6d ON public.ipam_service_ipaddresses USING btree (ipaddress_id);


--
-- Name: ipam_service_ipaddresses_service_id_ae26b9ab; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_service_ipaddresses_service_id_ae26b9ab ON public.ipam_service_ipaddresses USING btree (service_id);


--
-- Name: ipam_service_owner_id_ab1c827a; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_service_owner_id_ab1c827a ON public.ipam_service USING btree (owner_id);


--
-- Name: ipam_service_parent_object_type_id_8e76bfb3; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_service_parent_object_type_id_8e76bfb3 ON public.ipam_service USING btree (parent_object_type_id);


--
-- Name: ipam_servicetemplate_name_1a2f3410_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_servicetemplate_name_1a2f3410_like ON public.ipam_servicetemplate USING btree (name varchar_pattern_ops);


--
-- Name: ipam_servicetemplate_owner_id_bd152acb; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_servicetemplate_owner_id_bd152acb ON public.ipam_servicetemplate USING btree (owner_id);


--
-- Name: ipam_vlan_group_id_88cbfa62; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_vlan_group_id_88cbfa62 ON public.ipam_vlan USING btree (group_id);


--
-- Name: ipam_vlan_owner_id_ead15d2b; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_vlan_owner_id_ead15d2b ON public.ipam_vlan USING btree (owner_id);


--
-- Name: ipam_vlan_qinq_svlan_id_acbd7a5d; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_vlan_qinq_svlan_id_acbd7a5d ON public.ipam_vlan USING btree (qinq_svlan_id);


--
-- Name: ipam_vlan_role_id_f5015962; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_vlan_role_id_f5015962 ON public.ipam_vlan USING btree (role_id);


--
-- Name: ipam_vlan_site_id_a59334e3; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_vlan_site_id_a59334e3 ON public.ipam_vlan USING btree (site_id);


--
-- Name: ipam_vlan_tenant_id_71a8290d; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_vlan_tenant_id_71a8290d ON public.ipam_vlan USING btree (tenant_id);


--
-- Name: ipam_vlangr_scope_t_9da557_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_vlangr_scope_t_9da557_idx ON public.ipam_vlangroup USING btree (scope_type_id, scope_id);


--
-- Name: ipam_vlangroup_owner_id_3a50b541; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_vlangroup_owner_id_3a50b541 ON public.ipam_vlangroup USING btree (owner_id);


--
-- Name: ipam_vlangroup_scope_type_id_6606a755; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_vlangroup_scope_type_id_6606a755 ON public.ipam_vlangroup USING btree (scope_type_id);


--
-- Name: ipam_vlangroup_slug_40abcf6b; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_vlangroup_slug_40abcf6b ON public.ipam_vlangroup USING btree (slug);


--
-- Name: ipam_vlangroup_slug_40abcf6b_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_vlangroup_slug_40abcf6b_like ON public.ipam_vlangroup USING btree (slug varchar_pattern_ops);


--
-- Name: ipam_vlangroup_tenant_id_29197fca; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_vlangroup_tenant_id_29197fca ON public.ipam_vlangroup USING btree (tenant_id);


--
-- Name: ipam_vlantranslationpolicy_name_17e0a007_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_vlantranslationpolicy_name_17e0a007_like ON public.ipam_vlantranslationpolicy USING btree (name varchar_pattern_ops);


--
-- Name: ipam_vlantranslationpolicy_owner_id_f4e1cb82; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_vlantranslationpolicy_owner_id_f4e1cb82 ON public.ipam_vlantranslationpolicy USING btree (owner_id);


--
-- Name: ipam_vlantranslationrule_policy_id_09157735; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_vlantranslationrule_policy_id_09157735 ON public.ipam_vlantranslationrule USING btree (policy_id);


--
-- Name: ipam_vrf_export_targets_routetarget_id_8d9319f7; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_vrf_export_targets_routetarget_id_8d9319f7 ON public.ipam_vrf_export_targets USING btree (routetarget_id);


--
-- Name: ipam_vrf_export_targets_vrf_id_6f4875c4; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_vrf_export_targets_vrf_id_6f4875c4 ON public.ipam_vrf_export_targets USING btree (vrf_id);


--
-- Name: ipam_vrf_import_targets_routetarget_id_0e05b144; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_vrf_import_targets_routetarget_id_0e05b144 ON public.ipam_vrf_import_targets USING btree (routetarget_id);


--
-- Name: ipam_vrf_import_targets_vrf_id_ed491b19; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_vrf_import_targets_vrf_id_ed491b19 ON public.ipam_vrf_import_targets USING btree (vrf_id);


--
-- Name: ipam_vrf_owner_id_9b591781; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_vrf_owner_id_9b591781 ON public.ipam_vrf USING btree (owner_id);


--
-- Name: ipam_vrf_rd_0ac1bde1_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_vrf_rd_0ac1bde1_like ON public.ipam_vrf USING btree (rd varchar_pattern_ops);


--
-- Name: ipam_vrf_tenant_id_498b0051; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ipam_vrf_tenant_id_498b0051 ON public.ipam_vrf USING btree (tenant_id);


--
-- Name: tenancy_con_object__6f20f7_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX tenancy_con_object__6f20f7_idx ON public.tenancy_contactassignment USING btree (object_type_id, object_id);


--
-- Name: tenancy_contact_groups_contact_id_84c9d84f; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX tenancy_contact_groups_contact_id_84c9d84f ON public.tenancy_contact_groups USING btree (contact_id);


--
-- Name: tenancy_contact_groups_contactgroup_id_5c8d6c5a; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX tenancy_contact_groups_contactgroup_id_5c8d6c5a ON public.tenancy_contact_groups USING btree (contactgroup_id);


--
-- Name: tenancy_contact_owner_id_9d93abff; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX tenancy_contact_owner_id_9d93abff ON public.tenancy_contact USING btree (owner_id);


--
-- Name: tenancy_contactassignment_contact_id_5302baf0; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX tenancy_contactassignment_contact_id_5302baf0 ON public.tenancy_contactassignment USING btree (contact_id);


--
-- Name: tenancy_contactassignment_content_type_id_0c3f0c67; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX tenancy_contactassignment_content_type_id_0c3f0c67 ON public.tenancy_contactassignment USING btree (object_type_id);


--
-- Name: tenancy_contactassignment_role_id_fc08bfb5; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX tenancy_contactassignment_role_id_fc08bfb5 ON public.tenancy_contactassignment USING btree (role_id);


--
-- Name: tenancy_contactgroup_owner_id_4bb044c2; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX tenancy_contactgroup_owner_id_4bb044c2 ON public.tenancy_contactgroup USING btree (owner_id);


--
-- Name: tenancy_contactgroup_parent_id_c087d69f; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX tenancy_contactgroup_parent_id_c087d69f ON public.tenancy_contactgroup USING btree (parent_id);


--
-- Name: tenancy_contactgroup_slug_5b0f3e75; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX tenancy_contactgroup_slug_5b0f3e75 ON public.tenancy_contactgroup USING btree (slug);


--
-- Name: tenancy_contactgroup_slug_5b0f3e75_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX tenancy_contactgroup_slug_5b0f3e75_like ON public.tenancy_contactgroup USING btree (slug varchar_pattern_ops);


--
-- Name: tenancy_contactgroup_tree_id_57456c98; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX tenancy_contactgroup_tree_id_57456c98 ON public.tenancy_contactgroup USING btree (tree_id);


--
-- Name: tenancy_contactrole_name_44b01a1f_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX tenancy_contactrole_name_44b01a1f_like ON public.tenancy_contactrole USING btree (name varchar_pattern_ops);


--
-- Name: tenancy_contactrole_owner_id_3677102e; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX tenancy_contactrole_owner_id_3677102e ON public.tenancy_contactrole USING btree (owner_id);


--
-- Name: tenancy_contactrole_slug_c5837d7d_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX tenancy_contactrole_slug_c5837d7d_like ON public.tenancy_contactrole USING btree (slug varchar_pattern_ops);


--
-- Name: tenancy_tenant_group_id_7daef6f4; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX tenancy_tenant_group_id_7daef6f4 ON public.tenancy_tenant USING btree (group_id);


--
-- Name: tenancy_tenant_owner_id_02823f0b; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX tenancy_tenant_owner_id_02823f0b ON public.tenancy_tenant USING btree (owner_id);


--
-- Name: tenancy_tenant_slug_0716575e; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX tenancy_tenant_slug_0716575e ON public.tenancy_tenant USING btree (slug);


--
-- Name: tenancy_tenant_slug_0716575e_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX tenancy_tenant_slug_0716575e_like ON public.tenancy_tenant USING btree (slug varchar_pattern_ops);


--
-- Name: tenancy_tenant_unique_name; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX tenancy_tenant_unique_name ON public.tenancy_tenant USING btree (name) WHERE (group_id IS NULL);


--
-- Name: tenancy_tenant_unique_slug; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX tenancy_tenant_unique_slug ON public.tenancy_tenant USING btree (slug) WHERE (group_id IS NULL);


--
-- Name: tenancy_tenantgroup_name_53363199_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX tenancy_tenantgroup_name_53363199_like ON public.tenancy_tenantgroup USING btree (name varchar_pattern_ops);


--
-- Name: tenancy_tenantgroup_owner_id_a4f64bbd; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX tenancy_tenantgroup_owner_id_a4f64bbd ON public.tenancy_tenantgroup USING btree (owner_id);


--
-- Name: tenancy_tenantgroup_parent_id_2542fc18; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX tenancy_tenantgroup_parent_id_2542fc18 ON public.tenancy_tenantgroup USING btree (parent_id);


--
-- Name: tenancy_tenantgroup_slug_e2af1cb6_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX tenancy_tenantgroup_slug_e2af1cb6_like ON public.tenancy_tenantgroup USING btree (slug varchar_pattern_ops);


--
-- Name: tenancy_tenantgroup_tree_id_769a98bf; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX tenancy_tenantgroup_tree_id_769a98bf ON public.tenancy_tenantgroup USING btree (tree_id);


--
-- Name: thumbnail_kvstore_key_3f850178_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX thumbnail_kvstore_key_3f850178_like ON public.thumbnail_kvstore USING btree (key varchar_pattern_ops);


--
-- Name: uq_connconf_assignment; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX uq_connconf_assignment ON public.connconf_assignment USING btree (record_id, scope_type, COALESCE(scope_id, '00000000-0000-0000-0000-000000000000'::uuid));


--
-- Name: uq_connconf_record_name; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX uq_connconf_record_name ON public.connconf_record USING btree (protocol, lower((name)::text), COALESCE(owner_user_id, '00000000-0000-0000-0000-000000000000'::uuid));


--
-- Name: uq_connections_proxy_assignment; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX uq_connections_proxy_assignment ON public.connections_proxy_assignment USING btree (proxy_node_id, scope_type, COALESCE(scope_id, '00000000-0000-0000-0000-000000000000'::uuid));


--
-- Name: uq_license_seat_assignment_user_active; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX uq_license_seat_assignment_user_active ON public.license_seat_assignment USING btree (user_id) WHERE (revoked_at IS NULL);


--
-- Name: uq_playbook_service_principal_credential_field; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX uq_playbook_service_principal_credential_field ON public.playbook_service_principal_credential_binding USING btree (service_principal_id, consumer_type, credential_account_kind, credential_account_id, COALESCE(credential_account_entry_id, ''::text), field_key);


--
-- Name: users_owner_group_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX users_owner_group_id ON public.users_owner USING btree (group_id);


--
-- Name: users_owner_identity_group_group_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX users_owner_identity_group_group_id ON public.users_owner_identity_group USING btree (group_id);


--
-- Name: users_owner_identity_user_user_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX users_owner_identity_user_user_id ON public.users_owner_identity_user USING btree (user_id);


--
-- Name: ux_connection_filter_presets_name_lower; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX ux_connection_filter_presets_name_lower ON public.connection_filter_presets USING btree (lower(name));


--
-- Name: ux_settings_values_instance_key; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX ux_settings_values_instance_key ON public.settings_values USING btree (key) WHERE (scope_type = 'instance'::text);


--
-- Name: ux_settings_values_user_key; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX ux_settings_values_user_key ON public.settings_values USING btree (user_id, key) WHERE (scope_type = 'user'::text);


--
-- Name: virtualizat_scope_t_fb3b6e_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX virtualizat_scope_t_fb3b6e_idx ON public.virtualization_cluster USING btree (scope_type_id, scope_id);


--
-- Name: virtualization_cluster__location_id_f553e386; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX virtualization_cluster__location_id_f553e386 ON public.virtualization_cluster USING btree (_location_id);


--
-- Name: virtualization_cluster__region_id_9244325e; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX virtualization_cluster__region_id_9244325e ON public.virtualization_cluster USING btree (_region_id);


--
-- Name: virtualization_cluster__site_group_id_7d9bff8f; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX virtualization_cluster__site_group_id_7d9bff8f ON public.virtualization_cluster USING btree (_site_group_id);


--
-- Name: virtualization_cluster__site_id_883df848; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX virtualization_cluster__site_id_883df848 ON public.virtualization_cluster USING btree (_site_id);


--
-- Name: virtualization_cluster_group_id_de379828; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX virtualization_cluster_group_id_de379828 ON public.virtualization_cluster USING btree (group_id);


--
-- Name: virtualization_cluster_owner_id_2ea44dea; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX virtualization_cluster_owner_id_2ea44dea ON public.virtualization_cluster USING btree (owner_id);


--
-- Name: virtualization_cluster_scope_type_id_c49d797a; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX virtualization_cluster_scope_type_id_c49d797a ON public.virtualization_cluster USING btree (scope_type_id);


--
-- Name: virtualization_cluster_tenant_id_bc2868d0; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX virtualization_cluster_tenant_id_bc2868d0 ON public.virtualization_cluster USING btree (tenant_id);


--
-- Name: virtualization_cluster_type_id_4efafb0a; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX virtualization_cluster_type_id_4efafb0a ON public.virtualization_cluster USING btree (type_id);


--
-- Name: virtualization_clustergroup_name_4fcd26b4_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX virtualization_clustergroup_name_4fcd26b4_like ON public.virtualization_clustergroup USING btree (name varchar_pattern_ops);


--
-- Name: virtualization_clustergroup_owner_id_a865db22; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX virtualization_clustergroup_owner_id_a865db22 ON public.virtualization_clustergroup USING btree (owner_id);


--
-- Name: virtualization_clustergroup_slug_57ca1d23_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX virtualization_clustergroup_slug_57ca1d23_like ON public.virtualization_clustergroup USING btree (slug varchar_pattern_ops);


--
-- Name: virtualization_clustertype_name_ea854d3d_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX virtualization_clustertype_name_ea854d3d_like ON public.virtualization_clustertype USING btree (name varchar_pattern_ops);


--
-- Name: virtualization_clustertype_owner_id_7284f9e4; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX virtualization_clustertype_owner_id_7284f9e4 ON public.virtualization_clustertype USING btree (owner_id);


--
-- Name: virtualization_clustertype_slug_8ee4d0e0_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX virtualization_clustertype_slug_8ee4d0e0_like ON public.virtualization_clustertype USING btree (slug varchar_pattern_ops);


--
-- Name: virtualization_virtualdisk_owner_id_2e14487d; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX virtualization_virtualdisk_owner_id_2e14487d ON public.virtualization_virtualdisk USING btree (owner_id);


--
-- Name: virtualization_virtualdisk_virtual_machine_id_7bc8b6c2; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX virtualization_virtualdisk_virtual_machine_id_7bc8b6c2 ON public.virtualization_virtualdisk USING btree (virtual_machine_id);


--
-- Name: virtualization_virtualmachine_cluster_id_6c9f9047; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX virtualization_virtualmachine_cluster_id_6c9f9047 ON public.virtualization_virtualmachine USING btree (cluster_id);


--
-- Name: virtualization_virtualmachine_config_template_id_d7fc7874; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX virtualization_virtualmachine_config_template_id_d7fc7874 ON public.virtualization_virtualmachine USING btree (config_template_id);


--
-- Name: virtualization_virtualmachine_device_id_5a49ed18; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX virtualization_virtualmachine_device_id_5a49ed18 ON public.virtualization_virtualmachine USING btree (device_id);


--
-- Name: virtualization_virtualmachine_owner_id_f6593561; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX virtualization_virtualmachine_owner_id_f6593561 ON public.virtualization_virtualmachine USING btree (owner_id);


--
-- Name: virtualization_virtualmachine_platform_id_a6c5ccb2; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX virtualization_virtualmachine_platform_id_a6c5ccb2 ON public.virtualization_virtualmachine USING btree (platform_id);


--
-- Name: virtualization_virtualmachine_role_id_0cc898f9; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX virtualization_virtualmachine_role_id_0cc898f9 ON public.virtualization_virtualmachine USING btree (role_id);


--
-- Name: virtualization_virtualmachine_site_id_54475a27; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX virtualization_virtualmachine_site_id_54475a27 ON public.virtualization_virtualmachine USING btree (site_id);


--
-- Name: virtualization_virtualmachine_tenant_id_d00d1d77; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX virtualization_virtualmachine_tenant_id_d00d1d77 ON public.virtualization_virtualmachine USING btree (tenant_id);


--
-- Name: virtualization_virtualmachine_unique_name_cluster; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX virtualization_virtualmachine_unique_name_cluster ON public.virtualization_virtualmachine USING btree (lower((name)::text), cluster_id) WHERE (tenant_id IS NULL);


--
-- Name: virtualization_virtualmachine_unique_name_cluster_tenant; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX virtualization_virtualmachine_unique_name_cluster_tenant ON public.virtualization_virtualmachine USING btree (lower((name)::text), cluster_id, tenant_id);


--
-- Name: virtualization_virtualmachine_virtual_machine_type_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX virtualization_virtualmachine_virtual_machine_type_id ON public.virtualization_virtualmachine USING btree (virtual_machine_type_id);


--
-- Name: virtualization_virtualmachinetype_default_platform_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX virtualization_virtualmachinetype_default_platform_id ON public.virtualization_virtualmachinetype USING btree (default_platform_id);


--
-- Name: virtualization_virtualmachinetype_name; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX virtualization_virtualmachinetype_name ON public.virtualization_virtualmachinetype USING btree (name);


--
-- Name: virtualization_virtualmachinetype_owner_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX virtualization_virtualmachinetype_owner_id ON public.virtualization_virtualmachinetype USING btree (owner_id);


--
-- Name: virtualization_virtualmachinetype_unique_name; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX virtualization_virtualmachinetype_unique_name ON public.virtualization_virtualmachinetype USING btree (lower((name)::text));


--
-- Name: virtualization_vminterface_bridge_id_7462b91e; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX virtualization_vminterface_bridge_id_7462b91e ON public.virtualization_vminterface USING btree (bridge_id);


--
-- Name: virtualization_vminterface_owner_id_486754ee; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX virtualization_vminterface_owner_id_486754ee ON public.virtualization_vminterface USING btree (owner_id);


--
-- Name: virtualization_vminterface_parent_id_f86958e1; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX virtualization_vminterface_parent_id_f86958e1 ON public.virtualization_vminterface USING btree (parent_id);


--
-- Name: virtualization_vminterface_qinq_svlan_id_afacb024; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX virtualization_vminterface_qinq_svlan_id_afacb024 ON public.virtualization_vminterface USING btree (qinq_svlan_id);


--
-- Name: virtualization_vminterface_tagged_vlans_vlan_id_4e77411e; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX virtualization_vminterface_tagged_vlans_vlan_id_4e77411e ON public.virtualization_vminterface_tagged_vlans USING btree (vlan_id);


--
-- Name: virtualization_vminterface_tagged_vlans_vminterface_id_904b12de; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX virtualization_vminterface_tagged_vlans_vminterface_id_904b12de ON public.virtualization_vminterface_tagged_vlans USING btree (vminterface_id);


--
-- Name: virtualization_vminterface_untagged_vlan_id_aea4fc69; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX virtualization_vminterface_untagged_vlan_id_aea4fc69 ON public.virtualization_vminterface USING btree (untagged_vlan_id);


--
-- Name: virtualization_vminterface_virtual_machine_id_e9f89829; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX virtualization_vminterface_virtual_machine_id_e9f89829 ON public.virtualization_vminterface USING btree (virtual_machine_id);


--
-- Name: virtualization_vminterface_vlan_translation_policy_id_938e6637; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX virtualization_vminterface_vlan_translation_policy_id_938e6637 ON public.virtualization_vminterface USING btree (vlan_translation_policy_id);


--
-- Name: virtualization_vminterface_vrf_id_4b570a8c; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX virtualization_vminterface_vrf_id_4b570a8c ON public.virtualization_vminterface USING btree (vrf_id);


--
-- Name: vpn_ikepolicy_name_5124aa3b_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX vpn_ikepolicy_name_5124aa3b_like ON public.vpn_ikepolicy USING btree (name varchar_pattern_ops);


--
-- Name: vpn_ikepolicy_owner_id_dbcaf1bd; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX vpn_ikepolicy_owner_id_dbcaf1bd ON public.vpn_ikepolicy USING btree (owner_id);


--
-- Name: vpn_ikepolicy_proposals_ikepolicy_id_1e1deaab; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX vpn_ikepolicy_proposals_ikepolicy_id_1e1deaab ON public.vpn_ikepolicy_proposals USING btree (ikepolicy_id);


--
-- Name: vpn_ikepolicy_proposals_ikeproposal_id_a9ead252; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX vpn_ikepolicy_proposals_ikeproposal_id_a9ead252 ON public.vpn_ikepolicy_proposals USING btree (ikeproposal_id);


--
-- Name: vpn_ikeproposal_name_254623b7_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX vpn_ikeproposal_name_254623b7_like ON public.vpn_ikeproposal USING btree (name varchar_pattern_ops);


--
-- Name: vpn_ikeproposal_owner_id_018a3e69; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX vpn_ikeproposal_owner_id_018a3e69 ON public.vpn_ikeproposal USING btree (owner_id);


--
-- Name: vpn_ipsecpolicy_name_cf28a1aa_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX vpn_ipsecpolicy_name_cf28a1aa_like ON public.vpn_ipsecpolicy USING btree (name varchar_pattern_ops);


--
-- Name: vpn_ipsecpolicy_owner_id_e976d198; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX vpn_ipsecpolicy_owner_id_e976d198 ON public.vpn_ipsecpolicy USING btree (owner_id);


--
-- Name: vpn_ipsecpolicy_proposals_ipsecpolicy_id_0e7771a1; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX vpn_ipsecpolicy_proposals_ipsecpolicy_id_0e7771a1 ON public.vpn_ipsecpolicy_proposals USING btree (ipsecpolicy_id);


--
-- Name: vpn_ipsecpolicy_proposals_ipsecproposal_id_685fe509; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX vpn_ipsecpolicy_proposals_ipsecproposal_id_685fe509 ON public.vpn_ipsecpolicy_proposals USING btree (ipsecproposal_id);


--
-- Name: vpn_ipsecprofile_ike_policy_id_4eff5fb9; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX vpn_ipsecprofile_ike_policy_id_4eff5fb9 ON public.vpn_ipsecprofile USING btree (ike_policy_id);


--
-- Name: vpn_ipsecprofile_ipsec_policy_id_e06f2323; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX vpn_ipsecprofile_ipsec_policy_id_e06f2323 ON public.vpn_ipsecprofile USING btree (ipsec_policy_id);


--
-- Name: vpn_ipsecprofile_name_3ac63c72_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX vpn_ipsecprofile_name_3ac63c72_like ON public.vpn_ipsecprofile USING btree (name varchar_pattern_ops);


--
-- Name: vpn_ipsecprofile_owner_id_d7ebc4a0; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX vpn_ipsecprofile_owner_id_d7ebc4a0 ON public.vpn_ipsecprofile USING btree (owner_id);


--
-- Name: vpn_ipsecproposal_name_2fb98e2b_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX vpn_ipsecproposal_name_2fb98e2b_like ON public.vpn_ipsecproposal USING btree (name varchar_pattern_ops);


--
-- Name: vpn_ipsecproposal_owner_id_fdb3b755; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX vpn_ipsecproposal_owner_id_fdb3b755 ON public.vpn_ipsecproposal USING btree (owner_id);


--
-- Name: vpn_l2vpn_name_8824eda5_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX vpn_l2vpn_name_8824eda5_like ON public.vpn_l2vpn USING btree (name varchar_pattern_ops);


--
-- Name: vpn_l2vpn_owner_id_894bd11d; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX vpn_l2vpn_owner_id_894bd11d ON public.vpn_l2vpn USING btree (owner_id);


--
-- Name: vpn_l2vpn_slug_76b5a174_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX vpn_l2vpn_slug_76b5a174_like ON public.vpn_l2vpn USING btree (slug varchar_pattern_ops);


--
-- Name: vpn_l2vpn_tenant_id_57ec8f92; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX vpn_l2vpn_tenant_id_57ec8f92 ON public.vpn_l2vpn USING btree (tenant_id);


--
-- Name: vpn_l2vpntermination_assigned_object_type_id_f063b865; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX vpn_l2vpntermination_assigned_object_type_id_f063b865 ON public.vpn_l2vpntermination USING btree (assigned_object_type_id);


--
-- Name: vpn_l2vpntermination_l2vpn_id_f5367bbe; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX vpn_l2vpntermination_l2vpn_id_f5367bbe ON public.vpn_l2vpntermination USING btree (l2vpn_id);


--
-- Name: vpn_tunnel_group_id_56b1e7f7; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX vpn_tunnel_group_id_56b1e7f7 ON public.vpn_tunnel USING btree (group_id);


--
-- Name: vpn_tunnel_ipsec_profile_id_fd0361c5; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX vpn_tunnel_ipsec_profile_id_fd0361c5 ON public.vpn_tunnel USING btree (ipsec_profile_id);


--
-- Name: vpn_tunnel_name; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX vpn_tunnel_name ON public.vpn_tunnel USING btree (name) WHERE (group_id IS NULL);


--
-- Name: vpn_tunnel_name_f060beab_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX vpn_tunnel_name_f060beab_like ON public.vpn_tunnel USING btree (name varchar_pattern_ops);


--
-- Name: vpn_tunnel_owner_id_d04ee1fb; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX vpn_tunnel_owner_id_d04ee1fb ON public.vpn_tunnel USING btree (owner_id);


--
-- Name: vpn_tunnel_tenant_id_f3df2ab3; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX vpn_tunnel_tenant_id_f3df2ab3 ON public.vpn_tunnel USING btree (tenant_id);


--
-- Name: vpn_tunnelgroup_name_9f6ebf92_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX vpn_tunnelgroup_name_9f6ebf92_like ON public.vpn_tunnelgroup USING btree (name varchar_pattern_ops);


--
-- Name: vpn_tunnelgroup_owner_id_64609bfe; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX vpn_tunnelgroup_owner_id_64609bfe ON public.vpn_tunnelgroup USING btree (owner_id);


--
-- Name: vpn_tunnelgroup_slug_9e614d62_like; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX vpn_tunnelgroup_slug_9e614d62_like ON public.vpn_tunnelgroup USING btree (slug varchar_pattern_ops);


--
-- Name: vpn_tunneltermination_outside_ip_id_2c6f3a7c; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX vpn_tunneltermination_outside_ip_id_2c6f3a7c ON public.vpn_tunneltermination USING btree (outside_ip_id);


--
-- Name: vpn_tunneltermination_termination_type_id_e546f7a1; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX vpn_tunneltermination_termination_type_id_e546f7a1 ON public.vpn_tunneltermination USING btree (termination_type_id);


--
-- Name: vpn_tunneltermination_tunnel_id_962efa25; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX vpn_tunneltermination_tunnel_id_962efa25 ON public.vpn_tunneltermination USING btree (tunnel_id);


--
-- Name: webhook_subscriptions_enabled; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX webhook_subscriptions_enabled ON public.webhook_subscriptions USING btree (enabled) WHERE (enabled = true);


--
-- Name: webhook_subscriptions_name_uniq; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX webhook_subscriptions_name_uniq ON public.webhook_subscriptions USING btree (lower(name));


--
-- Name: wireless_wirelesslan_group_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX wireless_wirelesslan_group_id ON public.wireless_wirelesslan USING btree (group_id);


--
-- Name: wireless_wirelesslan_owner_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX wireless_wirelesslan_owner_id ON public.wireless_wirelesslan USING btree (owner_id);


--
-- Name: wireless_wirelesslan_scope_type_scope_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX wireless_wirelesslan_scope_type_scope_id ON public.wireless_wirelesslan USING btree (scope_type_id, scope_id);


--
-- Name: wireless_wirelesslan_ssid_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX wireless_wirelesslan_ssid_id ON public.wireless_wirelesslan USING btree (ssid, id);


--
-- Name: wireless_wirelesslan_tenant_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX wireless_wirelesslan_tenant_id ON public.wireless_wirelesslan USING btree (tenant_id);


--
-- Name: wireless_wirelesslan_vlan_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX wireless_wirelesslan_vlan_id ON public.wireless_wirelesslan USING btree (vlan_id);


--
-- Name: wireless_wirelesslangroup_owner_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX wireless_wirelesslangroup_owner_id ON public.wireless_wirelesslangroup USING btree (owner_id);


--
-- Name: wireless_wirelesslangroup_parent_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX wireless_wirelesslangroup_parent_id ON public.wireless_wirelesslangroup USING btree (parent_id);


--
-- Name: wireless_wirelesslink_interface_a_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX wireless_wirelesslink_interface_a_id ON public.wireless_wirelesslink USING btree (interface_a_id);


--
-- Name: wireless_wirelesslink_interface_b_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX wireless_wirelesslink_interface_b_id ON public.wireless_wirelesslink USING btree (interface_b_id);


--
-- Name: wireless_wirelesslink_owner_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX wireless_wirelesslink_owner_id ON public.wireless_wirelesslink USING btree (owner_id);


--
-- Name: wireless_wirelesslink_tenant_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX wireless_wirelesslink_tenant_id ON public.wireless_wirelesslink USING btree (tenant_id);


--
-- Name: inventory_dataflow_selected_ip inventory_dataflow_selected_ip_consistency; Type: TRIGGER; Schema: public; Owner: -
--

CREATE CONSTRAINT TRIGGER inventory_dataflow_selected_ip_consistency AFTER INSERT OR DELETE OR UPDATE ON public.inventory_dataflow_selected_ip DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION public.inventory_dataflow_selection_check();


--
-- Name: inventory_dataflow inventory_dataflow_selection_consistency; Type: TRIGGER; Schema: public; Owner: -
--

CREATE CONSTRAINT TRIGGER inventory_dataflow_selection_consistency AFTER INSERT OR UPDATE ON public.inventory_dataflow DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION public.inventory_dataflow_selection_check();


--
-- Name: playbook_execution_environment trg_playbook_execution_environment_updated_at; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER trg_playbook_execution_environment_updated_at BEFORE UPDATE ON public.playbook_execution_environment FOR EACH ROW EXECUTE FUNCTION public.set_updated_at();


--
-- Name: playbook_job_template_survey trg_playbook_job_template_survey_updated_at; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER trg_playbook_job_template_survey_updated_at BEFORE UPDATE ON public.playbook_job_template_survey FOR EACH ROW EXECUTE FUNCTION public.set_updated_at();


--
-- Name: playbook_job_template trg_playbook_job_template_updated_at; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER trg_playbook_job_template_updated_at BEFORE UPDATE ON public.playbook_job_template FOR EACH ROW EXECUTE FUNCTION public.set_updated_at();


--
-- Name: playbook_project trg_playbook_project_updated_at; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER trg_playbook_project_updated_at BEFORE UPDATE ON public.playbook_project FOR EACH ROW EXECUTE FUNCTION public.set_updated_at();


--
-- Name: playbook_version trg_playbook_version_immutable; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER trg_playbook_version_immutable BEFORE DELETE OR UPDATE ON public.playbook_version FOR EACH ROW EXECUTE FUNCTION public.reject_playbook_version_mutation();


--
-- Name: assistant_messages assistant_messages_session_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.assistant_messages
    ADD CONSTRAINT assistant_messages_session_id_fkey FOREIGN KEY (session_id) REFERENCES public.assistant_sessions(id) ON DELETE CASCADE;


--
-- Name: assistant_sessions assistant_sessions_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.assistant_sessions
    ADD CONSTRAINT assistant_sessions_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.identity_user(id) ON DELETE CASCADE;


--
-- Name: audit_event audit_event_actor_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.audit_event
    ADD CONSTRAINT audit_event_actor_id_fkey FOREIGN KEY (actor_id) REFERENCES public.identity_user(id) ON DELETE SET NULL;


--
-- Name: auth_session auth_session_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.auth_session
    ADD CONSTRAINT auth_session_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.identity_user(id) ON DELETE CASCADE;


--
-- Name: auth_user_api_token auth_user_api_token_revoked_by_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.auth_user_api_token
    ADD CONSTRAINT auth_user_api_token_revoked_by_user_id_fkey FOREIGN KEY (revoked_by_user_id) REFERENCES public.identity_user(id) ON DELETE SET NULL;


--
-- Name: auth_user_api_token_scope auth_user_api_token_scope_token_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.auth_user_api_token_scope
    ADD CONSTRAINT auth_user_api_token_scope_token_id_fkey FOREIGN KEY (token_id) REFERENCES public.auth_user_api_token(id) ON DELETE CASCADE;


--
-- Name: auth_user_api_token auth_user_api_token_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.auth_user_api_token
    ADD CONSTRAINT auth_user_api_token_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.identity_user(id) ON DELETE CASCADE;


--
-- Name: auth_user_credential auth_user_credential_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.auth_user_credential
    ADD CONSTRAINT auth_user_credential_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.identity_user(id) ON DELETE CASCADE;


--
-- Name: circuits_circuit circuits_circuit_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_circuit
    ADD CONSTRAINT circuits_circuit_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: circuits_circuit circuits_circuit_provider_account_id_a7c8f61b_fk_circuits_; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_circuit
    ADD CONSTRAINT circuits_circuit_provider_account_id_a7c8f61b_fk_circuits_ FOREIGN KEY (provider_account_id) REFERENCES public.circuits_provideraccount(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: circuits_circuit circuits_circuit_provider_id_d9195418_fk_circuits_provider_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_circuit
    ADD CONSTRAINT circuits_circuit_provider_id_d9195418_fk_circuits_provider_id FOREIGN KEY (provider_id) REFERENCES public.circuits_provider(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: circuits_circuit circuits_circuit_tenant_id_812508a5_fk_tenancy_tenant_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_circuit
    ADD CONSTRAINT circuits_circuit_tenant_id_812508a5_fk_tenancy_tenant_id FOREIGN KEY (tenant_id) REFERENCES public.tenancy_tenant(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: circuits_circuit circuits_circuit_type_id_1b9f485a_fk_circuits_circuittype_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_circuit
    ADD CONSTRAINT circuits_circuit_type_id_1b9f485a_fk_circuits_circuittype_id FOREIGN KEY (type_id) REFERENCES public.circuits_circuittype(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: circuits_circuitgroupassignment circuits_circuitgrou_group_id_1a7b6580_fk_circuits_; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_circuitgroupassignment
    ADD CONSTRAINT circuits_circuitgrou_group_id_1a7b6580_fk_circuits_ FOREIGN KEY (group_id) REFERENCES public.circuits_circuitgroup(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: circuits_circuitgroup circuits_circuitgroup_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_circuitgroup
    ADD CONSTRAINT circuits_circuitgroup_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: circuits_circuitgroup circuits_circuitgroup_tenant_id_5bafdc3f_fk_tenancy_tenant_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_circuitgroup
    ADD CONSTRAINT circuits_circuitgroup_tenant_id_5bafdc3f_fk_tenancy_tenant_id FOREIGN KEY (tenant_id) REFERENCES public.tenancy_tenant(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: circuits_circuittermination circuits_circuitterm__location_id_4a578dca_fk_dcim_loca; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_circuittermination
    ADD CONSTRAINT circuits_circuitterm__location_id_4a578dca_fk_dcim_loca FOREIGN KEY (_location_id) REFERENCES public.dcim_location(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: circuits_circuittermination circuits_circuitterm__provider_network_id_85ca661e_fk_circuits_; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_circuittermination
    ADD CONSTRAINT circuits_circuitterm__provider_network_id_85ca661e_fk_circuits_ FOREIGN KEY (_provider_network_id) REFERENCES public.circuits_providernetwork(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: circuits_circuittermination circuits_circuitterm__region_id_1fa03379_fk_dcim_regi; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_circuittermination
    ADD CONSTRAINT circuits_circuitterm__region_id_1fa03379_fk_dcim_regi FOREIGN KEY (_region_id) REFERENCES public.dcim_region(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: circuits_circuittermination circuits_circuitterm__site_group_id_ec0c1998_fk_dcim_site; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_circuittermination
    ADD CONSTRAINT circuits_circuitterm__site_group_id_ec0c1998_fk_dcim_site FOREIGN KEY (_site_group_id) REFERENCES public.dcim_sitegroup(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: circuits_circuittermination circuits_circuittermination__site_id_84942491_fk_dcim_site_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_circuittermination
    ADD CONSTRAINT circuits_circuittermination__site_id_84942491_fk_dcim_site_id FOREIGN KEY (_site_id) REFERENCES public.dcim_site(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: circuits_circuittermination circuits_circuittermination_cable_id_35e9f703_fk_dcim_cable_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_circuittermination
    ADD CONSTRAINT circuits_circuittermination_cable_id_35e9f703_fk_dcim_cable_id FOREIGN KEY (cable_id) REFERENCES public.dcim_cable(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: circuits_circuittype circuits_circuittype_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_circuittype
    ADD CONSTRAINT circuits_circuittype_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: circuits_provider_asns circuits_provider_as_provider_id_becc3f7e_fk_circuits_; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_provider_asns
    ADD CONSTRAINT circuits_provider_as_provider_id_becc3f7e_fk_circuits_ FOREIGN KEY (provider_id) REFERENCES public.circuits_provider(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: circuits_provider_asns circuits_provider_asns_asn_id_0a6c53b3_fk_ipam_asn_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_provider_asns
    ADD CONSTRAINT circuits_provider_asns_asn_id_0a6c53b3_fk_ipam_asn_id FOREIGN KEY (asn_id) REFERENCES public.ipam_asn(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: circuits_provider circuits_provider_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_provider
    ADD CONSTRAINT circuits_provider_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: circuits_provideraccount circuits_provideracc_provider_id_4bcd7e50_fk_circuits_; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_provideraccount
    ADD CONSTRAINT circuits_provideracc_provider_id_4bcd7e50_fk_circuits_ FOREIGN KEY (provider_id) REFERENCES public.circuits_provider(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: circuits_provideraccount circuits_provideraccount_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_provideraccount
    ADD CONSTRAINT circuits_provideraccount_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: circuits_providernetwork circuits_providernet_provider_id_7992236c_fk_circuits_; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_providernetwork
    ADD CONSTRAINT circuits_providernet_provider_id_7992236c_fk_circuits_ FOREIGN KEY (provider_id) REFERENCES public.circuits_provider(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: circuits_providernetwork circuits_providernetwork_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_providernetwork
    ADD CONSTRAINT circuits_providernetwork_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: circuits_virtualcircuittermination circuits_virtualcirc_interface_id_d764b232_fk_dcim_inte; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_virtualcircuittermination
    ADD CONSTRAINT circuits_virtualcirc_interface_id_d764b232_fk_dcim_inte FOREIGN KEY (interface_id) REFERENCES public.dcim_interface(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: circuits_virtualcircuit circuits_virtualcirc_provider_account_id_d942f455_fk_circuits_; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_virtualcircuit
    ADD CONSTRAINT circuits_virtualcirc_provider_account_id_d942f455_fk_circuits_ FOREIGN KEY (provider_account_id) REFERENCES public.circuits_provideraccount(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: circuits_virtualcircuit circuits_virtualcirc_provider_network_id_a902f409_fk_circuits_; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_virtualcircuit
    ADD CONSTRAINT circuits_virtualcirc_provider_network_id_a902f409_fk_circuits_ FOREIGN KEY (provider_network_id) REFERENCES public.circuits_providernetwork(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: circuits_virtualcircuit circuits_virtualcirc_type_id_8527d682_fk_circuits_; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_virtualcircuit
    ADD CONSTRAINT circuits_virtualcirc_type_id_8527d682_fk_circuits_ FOREIGN KEY (type_id) REFERENCES public.circuits_virtualcircuittype(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: circuits_virtualcircuittermination circuits_virtualcirc_virtual_circuit_id_ca588886_fk_circuits_; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_virtualcircuittermination
    ADD CONSTRAINT circuits_virtualcirc_virtual_circuit_id_ca588886_fk_circuits_ FOREIGN KEY (virtual_circuit_id) REFERENCES public.circuits_virtualcircuit(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: circuits_virtualcircuit circuits_virtualcircuit_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_virtualcircuit
    ADD CONSTRAINT circuits_virtualcircuit_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: circuits_virtualcircuit circuits_virtualcircuit_tenant_id_4458eca7_fk_tenancy_tenant_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_virtualcircuit
    ADD CONSTRAINT circuits_virtualcircuit_tenant_id_4458eca7_fk_tenancy_tenant_id FOREIGN KEY (tenant_id) REFERENCES public.tenancy_tenant(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: circuits_virtualcircuittype circuits_virtualcircuittype_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.circuits_virtualcircuittype
    ADD CONSTRAINT circuits_virtualcircuittype_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: config_context_binding config_context_binding_context_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.config_context_binding
    ADD CONSTRAINT config_context_binding_context_id_fkey FOREIGN KEY (context_id) REFERENCES public.config_context(id) ON DELETE CASCADE;


--
-- Name: connconf_assignment connconf_assignment_record_fk; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.connconf_assignment
    ADD CONSTRAINT connconf_assignment_record_fk FOREIGN KEY (record_id) REFERENCES public.connconf_record(id) ON DELETE CASCADE;


--
-- Name: connconf_record connconf_record_base_fk; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.connconf_record
    ADD CONSTRAINT connconf_record_base_fk FOREIGN KEY (base_record_id) REFERENCES public.connconf_record(id) ON DELETE RESTRICT;


--
-- Name: connconf_record connconf_record_owner_fk; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.connconf_record
    ADD CONSTRAINT connconf_record_owner_fk FOREIGN KEY (owner_user_id) REFERENCES public.identity_user(id) ON DELETE CASCADE;


--
-- Name: connection_filter_presets connection_filter_presets_created_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.connection_filter_presets
    ADD CONSTRAINT connection_filter_presets_created_by_fkey FOREIGN KEY (created_by) REFERENCES public.identity_user(id) ON DELETE CASCADE;


--
-- Name: connections_proxy_assignment connections_proxy_assignment_proxy_node_fk; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.connections_proxy_assignment
    ADD CONSTRAINT connections_proxy_assignment_proxy_node_fk FOREIGN KEY (proxy_node_id) REFERENCES public.connections_proxy_node(id) ON DELETE RESTRICT;


--
-- Name: connections_session connections_session_proxy_node_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.connections_session
    ADD CONSTRAINT connections_session_proxy_node_id_fkey FOREIGN KEY (proxy_node_id) REFERENCES public.connections_proxy_node(id) ON DELETE SET NULL;


--
-- Name: connections_session connections_session_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.connections_session
    ADD CONSTRAINT connections_session_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.identity_user(id) ON DELETE CASCADE;


--
-- Name: connections_user_credential_set connections_user_credential_set_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.connections_user_credential_set
    ADD CONSTRAINT connections_user_credential_set_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.identity_user(id) ON DELETE CASCADE;


--
-- Name: custom_field custom_field_choice_set_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.custom_field
    ADD CONSTRAINT custom_field_choice_set_fkey FOREIGN KEY (choice_set_id) REFERENCES public.custom_field_choice_set(id) ON DELETE RESTRICT;


--
-- Name: custom_field_object_type custom_field_object_type_content_type_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.custom_field_object_type
    ADD CONSTRAINT custom_field_object_type_content_type_fkey FOREIGN KEY (content_type_id) REFERENCES public.inventory_content_type(id) ON DELETE CASCADE;


--
-- Name: custom_field_object_type custom_field_object_type_field_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.custom_field_object_type
    ADD CONSTRAINT custom_field_object_type_field_fkey FOREIGN KEY (custom_field_id) REFERENCES public.custom_field(id) ON DELETE CASCADE;


--
-- Name: custom_field custom_field_related_object_type_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.custom_field
    ADD CONSTRAINT custom_field_related_object_type_fkey FOREIGN KEY (related_object_type_id) REFERENCES public.inventory_content_type(id) ON DELETE RESTRICT;


--
-- Name: dcim_cable dcim_cable_bundle_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_cable
    ADD CONSTRAINT dcim_cable_bundle_id_fkey FOREIGN KEY (bundle_id) REFERENCES public.dcim_cablebundle(id) ON DELETE SET NULL;


--
-- Name: dcim_cable dcim_cable_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_cable
    ADD CONSTRAINT dcim_cable_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: dcim_cable dcim_cable_tenant_id_3a7fdbb8_fk_tenancy_tenant_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_cable
    ADD CONSTRAINT dcim_cable_tenant_id_3a7fdbb8_fk_tenancy_tenant_id FOREIGN KEY (tenant_id) REFERENCES public.tenancy_tenant(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_cablebundle dcim_cablebundle_owner_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_cablebundle
    ADD CONSTRAINT dcim_cablebundle_owner_id_fkey FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: dcim_cabletermination dcim_cabletermination__device_id_f5884934_fk_dcim_device_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_cabletermination
    ADD CONSTRAINT dcim_cabletermination__device_id_f5884934_fk_dcim_device_id FOREIGN KEY (_device_id) REFERENCES public.dcim_device(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_cabletermination dcim_cabletermination__location_id_ff4be503_fk_dcim_location_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_cabletermination
    ADD CONSTRAINT dcim_cabletermination__location_id_ff4be503_fk_dcim_location_id FOREIGN KEY (_location_id) REFERENCES public.dcim_location(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_cabletermination dcim_cabletermination__rack_id_83a548e1_fk_dcim_rack_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_cabletermination
    ADD CONSTRAINT dcim_cabletermination__rack_id_83a548e1_fk_dcim_rack_id FOREIGN KEY (_rack_id) REFERENCES public.dcim_rack(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_cabletermination dcim_cabletermination__site_id_616962fa_fk_dcim_site_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_cabletermination
    ADD CONSTRAINT dcim_cabletermination__site_id_616962fa_fk_dcim_site_id FOREIGN KEY (_site_id) REFERENCES public.dcim_site(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_cabletermination dcim_cabletermination_cable_id_b50010d1_fk_dcim_cable_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_cabletermination
    ADD CONSTRAINT dcim_cabletermination_cable_id_b50010d1_fk_dcim_cable_id FOREIGN KEY (cable_id) REFERENCES public.dcim_cable(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_consoleport dcim_consoleport__location_id_d0e7437b_fk_dcim_location_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_consoleport
    ADD CONSTRAINT dcim_consoleport__location_id_d0e7437b_fk_dcim_location_id FOREIGN KEY (_location_id) REFERENCES public.dcim_location(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_consoleport dcim_consoleport__path_id_e40a4436_fk_dcim_cablepath_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_consoleport
    ADD CONSTRAINT dcim_consoleport__path_id_e40a4436_fk_dcim_cablepath_id FOREIGN KEY (_path_id) REFERENCES public.dcim_cablepath(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_consoleport dcim_consoleport__rack_id_71f16827_fk_dcim_rack_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_consoleport
    ADD CONSTRAINT dcim_consoleport__rack_id_71f16827_fk_dcim_rack_id FOREIGN KEY (_rack_id) REFERENCES public.dcim_rack(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_consoleport dcim_consoleport__site_id_9652df9c_fk_dcim_site_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_consoleport
    ADD CONSTRAINT dcim_consoleport__site_id_9652df9c_fk_dcim_site_id FOREIGN KEY (_site_id) REFERENCES public.dcim_site(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_consoleport dcim_consoleport_cable_id_a9ae5465_fk_dcim_cable_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_consoleport
    ADD CONSTRAINT dcim_consoleport_cable_id_a9ae5465_fk_dcim_cable_id FOREIGN KEY (cable_id) REFERENCES public.dcim_cable(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_consoleport dcim_consoleport_device_id_f2d90d3c_fk_dcim_device_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_consoleport
    ADD CONSTRAINT dcim_consoleport_device_id_f2d90d3c_fk_dcim_device_id FOREIGN KEY (device_id) REFERENCES public.dcim_device(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_consoleport dcim_consoleport_module_id_d17b2519_fk_dcim_module_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_consoleport
    ADD CONSTRAINT dcim_consoleport_module_id_d17b2519_fk_dcim_module_id FOREIGN KEY (module_id) REFERENCES public.dcim_module(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_consoleport dcim_consoleport_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_consoleport
    ADD CONSTRAINT dcim_consoleport_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: dcim_consoleporttemplate dcim_consoleporttemp_device_type_id_075d4015_fk_dcim_devi; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_consoleporttemplate
    ADD CONSTRAINT dcim_consoleporttemp_device_type_id_075d4015_fk_dcim_devi FOREIGN KEY (device_type_id) REFERENCES public.dcim_devicetype(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_consoleporttemplate dcim_consoleporttemp_module_type_id_c0f35d97_fk_dcim_modu; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_consoleporttemplate
    ADD CONSTRAINT dcim_consoleporttemp_module_type_id_c0f35d97_fk_dcim_modu FOREIGN KEY (module_type_id) REFERENCES public.dcim_moduletype(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_consoleserverport dcim_consoleserverpo__location_id_7525ec0e_fk_dcim_loca; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_consoleserverport
    ADD CONSTRAINT dcim_consoleserverpo__location_id_7525ec0e_fk_dcim_loca FOREIGN KEY (_location_id) REFERENCES public.dcim_location(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_consoleserverporttemplate dcim_consoleserverpo_device_type_id_579bdc86_fk_dcim_devi; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_consoleserverporttemplate
    ADD CONSTRAINT dcim_consoleserverpo_device_type_id_579bdc86_fk_dcim_devi FOREIGN KEY (device_type_id) REFERENCES public.dcim_devicetype(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_consoleserverporttemplate dcim_consoleserverpo_module_type_id_4abf751a_fk_dcim_modu; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_consoleserverporttemplate
    ADD CONSTRAINT dcim_consoleserverpo_module_type_id_4abf751a_fk_dcim_modu FOREIGN KEY (module_type_id) REFERENCES public.dcim_moduletype(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_consoleserverport dcim_consoleserverport__path_id_dc5abe09_fk_dcim_cablepath_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_consoleserverport
    ADD CONSTRAINT dcim_consoleserverport__path_id_dc5abe09_fk_dcim_cablepath_id FOREIGN KEY (_path_id) REFERENCES public.dcim_cablepath(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_consoleserverport dcim_consoleserverport__rack_id_3eea9f1b_fk_dcim_rack_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_consoleserverport
    ADD CONSTRAINT dcim_consoleserverport__rack_id_3eea9f1b_fk_dcim_rack_id FOREIGN KEY (_rack_id) REFERENCES public.dcim_rack(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_consoleserverport dcim_consoleserverport__site_id_c500ac62_fk_dcim_site_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_consoleserverport
    ADD CONSTRAINT dcim_consoleserverport__site_id_c500ac62_fk_dcim_site_id FOREIGN KEY (_site_id) REFERENCES public.dcim_site(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_consoleserverport dcim_consoleserverport_cable_id_f2940dfd_fk_dcim_cable_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_consoleserverport
    ADD CONSTRAINT dcim_consoleserverport_cable_id_f2940dfd_fk_dcim_cable_id FOREIGN KEY (cable_id) REFERENCES public.dcim_cable(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_consoleserverport dcim_consoleserverport_device_id_d9866581_fk_dcim_device_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_consoleserverport
    ADD CONSTRAINT dcim_consoleserverport_device_id_d9866581_fk_dcim_device_id FOREIGN KEY (device_id) REFERENCES public.dcim_device(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_consoleserverport dcim_consoleserverport_module_id_d060cfc8_fk_dcim_module_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_consoleserverport
    ADD CONSTRAINT dcim_consoleserverport_module_id_d060cfc8_fk_dcim_module_id FOREIGN KEY (module_id) REFERENCES public.dcim_module(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_consoleserverport dcim_consoleserverport_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_consoleserverport
    ADD CONSTRAINT dcim_consoleserverport_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: dcim_coolingfeed dcim_coolingfeed_cooling_source_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_coolingfeed
    ADD CONSTRAINT dcim_coolingfeed_cooling_source_id_fkey FOREIGN KEY (cooling_source_id) REFERENCES public.dcim_coolingsource(id) ON DELETE RESTRICT;


--
-- Name: dcim_coolingfeed dcim_coolingfeed_owner_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_coolingfeed
    ADD CONSTRAINT dcim_coolingfeed_owner_id_fkey FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: dcim_coolingfeed dcim_coolingfeed_rack_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_coolingfeed
    ADD CONSTRAINT dcim_coolingfeed_rack_id_fkey FOREIGN KEY (rack_id) REFERENCES public.dcim_rack(id) ON DELETE RESTRICT;


--
-- Name: dcim_coolingfeed dcim_coolingfeed_tenant_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_coolingfeed
    ADD CONSTRAINT dcim_coolingfeed_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenancy_tenant(id) ON DELETE RESTRICT;


--
-- Name: dcim_coolingintake dcim_coolingintake_cooling_outflow_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_coolingintake
    ADD CONSTRAINT dcim_coolingintake_cooling_outflow_id_fkey FOREIGN KEY (cooling_outflow_id) REFERENCES public.dcim_coolingoutflow(id) ON DELETE SET NULL;


--
-- Name: dcim_coolingintake dcim_coolingintake_device_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_coolingintake
    ADD CONSTRAINT dcim_coolingintake_device_id_fkey FOREIGN KEY (device_id) REFERENCES public.dcim_device(id) ON DELETE CASCADE;


--
-- Name: dcim_coolingintake dcim_coolingintake_module_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_coolingintake
    ADD CONSTRAINT dcim_coolingintake_module_id_fkey FOREIGN KEY (module_id) REFERENCES public.dcim_module(id) ON DELETE CASCADE;


--
-- Name: dcim_coolingintake dcim_coolingintake_owner_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_coolingintake
    ADD CONSTRAINT dcim_coolingintake_owner_id_fkey FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: dcim_coolingintaketemplate dcim_coolingintaketemplate_device_type_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_coolingintaketemplate
    ADD CONSTRAINT dcim_coolingintaketemplate_device_type_id_fkey FOREIGN KEY (device_type_id) REFERENCES public.dcim_devicetype(id) ON DELETE CASCADE;


--
-- Name: dcim_coolingintaketemplate dcim_coolingintaketemplate_module_type_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_coolingintaketemplate
    ADD CONSTRAINT dcim_coolingintaketemplate_module_type_id_fkey FOREIGN KEY (module_type_id) REFERENCES public.dcim_moduletype(id) ON DELETE CASCADE;


--
-- Name: dcim_coolingoutflow dcim_coolingoutflow_cooling_intake_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_coolingoutflow
    ADD CONSTRAINT dcim_coolingoutflow_cooling_intake_id_fkey FOREIGN KEY (cooling_intake_id) REFERENCES public.dcim_coolingintake(id) ON DELETE SET NULL;


--
-- Name: dcim_coolingoutflow dcim_coolingoutflow_device_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_coolingoutflow
    ADD CONSTRAINT dcim_coolingoutflow_device_id_fkey FOREIGN KEY (device_id) REFERENCES public.dcim_device(id) ON DELETE CASCADE;


--
-- Name: dcim_coolingoutflow dcim_coolingoutflow_module_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_coolingoutflow
    ADD CONSTRAINT dcim_coolingoutflow_module_id_fkey FOREIGN KEY (module_id) REFERENCES public.dcim_module(id) ON DELETE CASCADE;


--
-- Name: dcim_coolingoutflow dcim_coolingoutflow_owner_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_coolingoutflow
    ADD CONSTRAINT dcim_coolingoutflow_owner_id_fkey FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: dcim_coolingoutflowtemplate dcim_coolingoutflowtemplate_cooling_intake_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_coolingoutflowtemplate
    ADD CONSTRAINT dcim_coolingoutflowtemplate_cooling_intake_id_fkey FOREIGN KEY (cooling_intake_id) REFERENCES public.dcim_coolingintaketemplate(id) ON DELETE SET NULL;


--
-- Name: dcim_coolingoutflowtemplate dcim_coolingoutflowtemplate_device_type_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_coolingoutflowtemplate
    ADD CONSTRAINT dcim_coolingoutflowtemplate_device_type_id_fkey FOREIGN KEY (device_type_id) REFERENCES public.dcim_devicetype(id) ON DELETE CASCADE;


--
-- Name: dcim_coolingoutflowtemplate dcim_coolingoutflowtemplate_module_type_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_coolingoutflowtemplate
    ADD CONSTRAINT dcim_coolingoutflowtemplate_module_type_id_fkey FOREIGN KEY (module_type_id) REFERENCES public.dcim_moduletype(id) ON DELETE CASCADE;


--
-- Name: dcim_coolingsource dcim_coolingsource_location_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_coolingsource
    ADD CONSTRAINT dcim_coolingsource_location_id_fkey FOREIGN KEY (location_id) REFERENCES public.dcim_location(id) ON DELETE RESTRICT;


--
-- Name: dcim_coolingsource dcim_coolingsource_owner_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_coolingsource
    ADD CONSTRAINT dcim_coolingsource_owner_id_fkey FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: dcim_coolingsource dcim_coolingsource_site_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_coolingsource
    ADD CONSTRAINT dcim_coolingsource_site_id_fkey FOREIGN KEY (site_id) REFERENCES public.dcim_site(id) ON DELETE RESTRICT;


--
-- Name: dcim_device dcim_device_cluster_id_cf852f78_fk_virtualization_cluster_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_device
    ADD CONSTRAINT dcim_device_cluster_id_cf852f78_fk_virtualization_cluster_id FOREIGN KEY (cluster_id) REFERENCES public.virtualization_cluster(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_device dcim_device_device_type_id_d61b4086_fk_dcim_devicetype_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_device
    ADD CONSTRAINT dcim_device_device_type_id_d61b4086_fk_dcim_devicetype_id FOREIGN KEY (device_type_id) REFERENCES public.dcim_devicetype(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_device dcim_device_location_id_11a7bedb_fk_dcim_location_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_device
    ADD CONSTRAINT dcim_device_location_id_11a7bedb_fk_dcim_location_id FOREIGN KEY (location_id) REFERENCES public.dcim_location(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_device dcim_device_oob_ip_id_5e7219c1_fk_ipam_ipaddress_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_device
    ADD CONSTRAINT dcim_device_oob_ip_id_5e7219c1_fk_ipam_ipaddress_id FOREIGN KEY (oob_ip_id) REFERENCES public.ipam_ipaddress(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_device dcim_device_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_device
    ADD CONSTRAINT dcim_device_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: dcim_device dcim_device_platform_id_468138f1_fk_dcim_platform_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_device
    ADD CONSTRAINT dcim_device_platform_id_468138f1_fk_dcim_platform_id FOREIGN KEY (platform_id) REFERENCES public.dcim_platform(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_device dcim_device_primary_ip4_id_2ccd943a_fk_ipam_ipaddress_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_device
    ADD CONSTRAINT dcim_device_primary_ip4_id_2ccd943a_fk_ipam_ipaddress_id FOREIGN KEY (primary_ip4_id) REFERENCES public.ipam_ipaddress(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_device dcim_device_primary_ip6_id_d180fe91_fk_ipam_ipaddress_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_device
    ADD CONSTRAINT dcim_device_primary_ip6_id_d180fe91_fk_ipam_ipaddress_id FOREIGN KEY (primary_ip6_id) REFERENCES public.ipam_ipaddress(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_device dcim_device_rack_id_23bde71f_fk_dcim_rack_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_device
    ADD CONSTRAINT dcim_device_rack_id_23bde71f_fk_dcim_rack_id FOREIGN KEY (rack_id) REFERENCES public.dcim_rack(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_device dcim_device_role_id_61edcc33_fk_dcim_devicerole_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_device
    ADD CONSTRAINT dcim_device_role_id_61edcc33_fk_dcim_devicerole_id FOREIGN KEY (role_id) REFERENCES public.dcim_devicerole(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_device dcim_device_site_id_ff897cf6_fk_dcim_site_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_device
    ADD CONSTRAINT dcim_device_site_id_ff897cf6_fk_dcim_site_id FOREIGN KEY (site_id) REFERENCES public.dcim_site(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_device dcim_device_tenant_id_dcea7969_fk_tenancy_tenant_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_device
    ADD CONSTRAINT dcim_device_tenant_id_dcea7969_fk_tenancy_tenant_id FOREIGN KEY (tenant_id) REFERENCES public.tenancy_tenant(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_devicebay dcim_devicebay__location_id_6c869a87_fk_dcim_location_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_devicebay
    ADD CONSTRAINT dcim_devicebay__location_id_6c869a87_fk_dcim_location_id FOREIGN KEY (_location_id) REFERENCES public.dcim_location(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_devicebay dcim_devicebay__rack_id_75672431_fk_dcim_rack_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_devicebay
    ADD CONSTRAINT dcim_devicebay__rack_id_75672431_fk_dcim_rack_id FOREIGN KEY (_rack_id) REFERENCES public.dcim_rack(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_devicebay dcim_devicebay__site_id_8ddc30f4_fk_dcim_site_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_devicebay
    ADD CONSTRAINT dcim_devicebay__site_id_8ddc30f4_fk_dcim_site_id FOREIGN KEY (_site_id) REFERENCES public.dcim_site(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_devicebay dcim_devicebay_device_id_0c8a1218_fk_dcim_device_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_devicebay
    ADD CONSTRAINT dcim_devicebay_device_id_0c8a1218_fk_dcim_device_id FOREIGN KEY (device_id) REFERENCES public.dcim_device(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_devicebay dcim_devicebay_installed_device_id_04618112_fk_dcim_device_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_devicebay
    ADD CONSTRAINT dcim_devicebay_installed_device_id_04618112_fk_dcim_device_id FOREIGN KEY (installed_device_id) REFERENCES public.dcim_device(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_devicebay dcim_devicebay_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_devicebay
    ADD CONSTRAINT dcim_devicebay_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: dcim_devicebaytemplate dcim_devicebaytempla_device_type_id_f4b24a29_fk_dcim_devi; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_devicebaytemplate
    ADD CONSTRAINT dcim_devicebaytempla_device_type_id_f4b24a29_fk_dcim_devi FOREIGN KEY (device_type_id) REFERENCES public.dcim_devicetype(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_devicerole dcim_devicerole_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_devicerole
    ADD CONSTRAINT dcim_devicerole_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: dcim_devicerole dcim_devicerole_parent_id_7f9ccf87_fk_dcim_devicerole_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_devicerole
    ADD CONSTRAINT dcim_devicerole_parent_id_7f9ccf87_fk_dcim_devicerole_id FOREIGN KEY (parent_id) REFERENCES public.dcim_devicerole(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_devicetype dcim_devicetype_default_platform_id_1f6ff6ac_fk_dcim_plat; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_devicetype
    ADD CONSTRAINT dcim_devicetype_default_platform_id_1f6ff6ac_fk_dcim_plat FOREIGN KEY (default_platform_id) REFERENCES public.dcim_platform(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_devicetype dcim_devicetype_manufacturer_id_a3e8029e_fk_dcim_manu; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_devicetype
    ADD CONSTRAINT dcim_devicetype_manufacturer_id_a3e8029e_fk_dcim_manu FOREIGN KEY (manufacturer_id) REFERENCES public.dcim_manufacturer(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_devicetype dcim_devicetype_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_devicetype
    ADD CONSTRAINT dcim_devicetype_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: dcim_frontport dcim_frontport__location_id_dc0402e7_fk_dcim_location_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_frontport
    ADD CONSTRAINT dcim_frontport__location_id_dc0402e7_fk_dcim_location_id FOREIGN KEY (_location_id) REFERENCES public.dcim_location(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_frontport dcim_frontport__rack_id_1ba6d11c_fk_dcim_rack_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_frontport
    ADD CONSTRAINT dcim_frontport__rack_id_1ba6d11c_fk_dcim_rack_id FOREIGN KEY (_rack_id) REFERENCES public.dcim_rack(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_frontport dcim_frontport__site_id_51efdefe_fk_dcim_site_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_frontport
    ADD CONSTRAINT dcim_frontport__site_id_51efdefe_fk_dcim_site_id FOREIGN KEY (_site_id) REFERENCES public.dcim_site(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_frontport dcim_frontport_cable_id_04ff8aab_fk_dcim_cable_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_frontport
    ADD CONSTRAINT dcim_frontport_cable_id_04ff8aab_fk_dcim_cable_id FOREIGN KEY (cable_id) REFERENCES public.dcim_cable(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_frontport dcim_frontport_device_id_950557b5_fk_dcim_device_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_frontport
    ADD CONSTRAINT dcim_frontport_device_id_950557b5_fk_dcim_device_id FOREIGN KEY (device_id) REFERENCES public.dcim_device(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_frontport dcim_frontport_module_id_952c3f9a_fk_dcim_module_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_frontport
    ADD CONSTRAINT dcim_frontport_module_id_952c3f9a_fk_dcim_module_id FOREIGN KEY (module_id) REFERENCES public.dcim_module(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_frontport dcim_frontport_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_frontport
    ADD CONSTRAINT dcim_frontport_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: dcim_frontporttemplate dcim_frontporttempla_device_type_id_f088b952_fk_dcim_devi; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_frontporttemplate
    ADD CONSTRAINT dcim_frontporttempla_device_type_id_f088b952_fk_dcim_devi FOREIGN KEY (device_type_id) REFERENCES public.dcim_devicetype(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_frontporttemplate dcim_frontporttempla_module_type_id_66851ff9_fk_dcim_modu; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_frontporttemplate
    ADD CONSTRAINT dcim_frontporttempla_module_type_id_66851ff9_fk_dcim_modu FOREIGN KEY (module_type_id) REFERENCES public.dcim_moduletype(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_interface dcim_interface__location_id_72d5e107_fk_dcim_location_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_interface
    ADD CONSTRAINT dcim_interface__location_id_72d5e107_fk_dcim_location_id FOREIGN KEY (_location_id) REFERENCES public.dcim_location(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_interface dcim_interface__path_id_f8f4f7f0_fk_dcim_cablepath_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_interface
    ADD CONSTRAINT dcim_interface__path_id_f8f4f7f0_fk_dcim_cablepath_id FOREIGN KEY (_path_id) REFERENCES public.dcim_cablepath(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_interface dcim_interface__rack_id_0d3f84a6_fk_dcim_rack_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_interface
    ADD CONSTRAINT dcim_interface__rack_id_0d3f84a6_fk_dcim_rack_id FOREIGN KEY (_rack_id) REFERENCES public.dcim_rack(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_interface dcim_interface__site_id_0fffd4ef_fk_dcim_site_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_interface
    ADD CONSTRAINT dcim_interface__site_id_0fffd4ef_fk_dcim_site_id FOREIGN KEY (_site_id) REFERENCES public.dcim_site(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_interface dcim_interface_bridge_id_f2a8df85_fk_dcim_interface_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_interface
    ADD CONSTRAINT dcim_interface_bridge_id_f2a8df85_fk_dcim_interface_id FOREIGN KEY (bridge_id) REFERENCES public.dcim_interface(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_interface dcim_interface_cable_id_1b264edb_fk_dcim_cable_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_interface
    ADD CONSTRAINT dcim_interface_cable_id_1b264edb_fk_dcim_cable_id FOREIGN KEY (cable_id) REFERENCES public.dcim_cable(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_interface dcim_interface_device_id_359c6115_fk_dcim_device_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_interface
    ADD CONSTRAINT dcim_interface_device_id_359c6115_fk_dcim_device_id FOREIGN KEY (device_id) REFERENCES public.dcim_device(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_interface dcim_interface_lag_id_ea1a1d12_fk_dcim_interface_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_interface
    ADD CONSTRAINT dcim_interface_lag_id_ea1a1d12_fk_dcim_interface_id FOREIGN KEY (lag_id) REFERENCES public.dcim_interface(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_interface dcim_interface_module_id_05ca2da5_fk_dcim_module_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_interface
    ADD CONSTRAINT dcim_interface_module_id_05ca2da5_fk_dcim_module_id FOREIGN KEY (module_id) REFERENCES public.dcim_module(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_interface dcim_interface_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_interface
    ADD CONSTRAINT dcim_interface_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: dcim_interface dcim_interface_parent_id_3e2b159b_fk_dcim_interface_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_interface
    ADD CONSTRAINT dcim_interface_parent_id_3e2b159b_fk_dcim_interface_id FOREIGN KEY (parent_id) REFERENCES public.dcim_interface(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_interface dcim_interface_primary_mac_address__a0bb90ca_fk_dcim_maca; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_interface
    ADD CONSTRAINT dcim_interface_primary_mac_address__a0bb90ca_fk_dcim_maca FOREIGN KEY (primary_mac_address_id) REFERENCES public.dcim_macaddress(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_interface dcim_interface_qinq_svlan_id_21189a58_fk_ipam_vlan_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_interface
    ADD CONSTRAINT dcim_interface_qinq_svlan_id_21189a58_fk_ipam_vlan_id FOREIGN KEY (qinq_svlan_id) REFERENCES public.ipam_vlan(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_interface_tagged_vlans dcim_interface_tagge_interface_id_5870c9e9_fk_dcim_inte; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_interface_tagged_vlans
    ADD CONSTRAINT dcim_interface_tagge_interface_id_5870c9e9_fk_dcim_inte FOREIGN KEY (interface_id) REFERENCES public.dcim_interface(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_interface_tagged_vlans dcim_interface_tagged_vlans_vlan_id_e027005c_fk_ipam_vlan_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_interface_tagged_vlans
    ADD CONSTRAINT dcim_interface_tagged_vlans_vlan_id_e027005c_fk_ipam_vlan_id FOREIGN KEY (vlan_id) REFERENCES public.ipam_vlan(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_interface dcim_interface_untagged_vlan_id_838dc7be_fk_ipam_vlan_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_interface
    ADD CONSTRAINT dcim_interface_untagged_vlan_id_838dc7be_fk_ipam_vlan_id FOREIGN KEY (untagged_vlan_id) REFERENCES public.ipam_vlan(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_interface_vdcs dcim_interface_vdcs_interface_id_6d58dbaf_fk_dcim_interface_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_interface_vdcs
    ADD CONSTRAINT dcim_interface_vdcs_interface_id_6d58dbaf_fk_dcim_interface_id FOREIGN KEY (interface_id) REFERENCES public.dcim_interface(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_interface_vdcs dcim_interface_vdcs_virtualdevicecontext_af0bfd4b_fk_dcim_virt; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_interface_vdcs
    ADD CONSTRAINT dcim_interface_vdcs_virtualdevicecontext_af0bfd4b_fk_dcim_virt FOREIGN KEY (virtualdevicecontext_id) REFERENCES public.dcim_virtualdevicecontext(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_interface dcim_interface_vlan_translation_pol_91060031_fk_ipam_vlan; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_interface
    ADD CONSTRAINT dcim_interface_vlan_translation_pol_91060031_fk_ipam_vlan FOREIGN KEY (vlan_translation_policy_id) REFERENCES public.ipam_vlantranslationpolicy(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_interface dcim_interface_vrf_id_a92e59b2_fk_ipam_vrf_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_interface
    ADD CONSTRAINT dcim_interface_vrf_id_a92e59b2_fk_ipam_vrf_id FOREIGN KEY (vrf_id) REFERENCES public.ipam_vrf(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_interface dcim_interface_wireless_link_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_interface
    ADD CONSTRAINT dcim_interface_wireless_link_id_fkey FOREIGN KEY (wireless_link_id) REFERENCES public.wireless_wirelesslink(id) ON DELETE SET NULL;


--
-- Name: dcim_interface_wirelesslans dcim_interface_wirelesslans_interface_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_interface_wirelesslans
    ADD CONSTRAINT dcim_interface_wirelesslans_interface_id_fkey FOREIGN KEY (interface_id) REFERENCES public.dcim_interface(id) ON DELETE CASCADE;


--
-- Name: dcim_interface_wirelesslans dcim_interface_wirelesslans_wirelesslan_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_interface_wirelesslans
    ADD CONSTRAINT dcim_interface_wirelesslans_wirelesslan_id_fkey FOREIGN KEY (wirelesslan_id) REFERENCES public.wireless_wirelesslan(id) ON DELETE CASCADE;


--
-- Name: dcim_interfacetemplate dcim_interfacetempla_bridge_id_4e44355b_fk_dcim_inte; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_interfacetemplate
    ADD CONSTRAINT dcim_interfacetempla_bridge_id_4e44355b_fk_dcim_inte FOREIGN KEY (bridge_id) REFERENCES public.dcim_interfacetemplate(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_interfacetemplate dcim_interfacetempla_device_type_id_4bfcbfab_fk_dcim_devi; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_interfacetemplate
    ADD CONSTRAINT dcim_interfacetempla_device_type_id_4bfcbfab_fk_dcim_devi FOREIGN KEY (device_type_id) REFERENCES public.dcim_devicetype(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_interfacetemplate dcim_interfacetempla_module_type_id_f941f180_fk_dcim_modu; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_interfacetemplate
    ADD CONSTRAINT dcim_interfacetempla_module_type_id_f941f180_fk_dcim_modu FOREIGN KEY (module_type_id) REFERENCES public.dcim_moduletype(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_inventoryitem dcim_inventoryitem__location_id_05a23b33_fk_dcim_location_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_inventoryitem
    ADD CONSTRAINT dcim_inventoryitem__location_id_05a23b33_fk_dcim_location_id FOREIGN KEY (_location_id) REFERENCES public.dcim_location(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_inventoryitem dcim_inventoryitem__rack_id_25b80d23_fk_dcim_rack_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_inventoryitem
    ADD CONSTRAINT dcim_inventoryitem__rack_id_25b80d23_fk_dcim_rack_id FOREIGN KEY (_rack_id) REFERENCES public.dcim_rack(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_inventoryitem dcim_inventoryitem__site_id_cd94573b_fk_dcim_site_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_inventoryitem
    ADD CONSTRAINT dcim_inventoryitem__site_id_cd94573b_fk_dcim_site_id FOREIGN KEY (_site_id) REFERENCES public.dcim_site(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_inventoryitem dcim_inventoryitem_device_id_033d83f8_fk_dcim_device_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_inventoryitem
    ADD CONSTRAINT dcim_inventoryitem_device_id_033d83f8_fk_dcim_device_id FOREIGN KEY (device_id) REFERENCES public.dcim_device(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_inventoryitem dcim_inventoryitem_manufacturer_id_dcd1b78a_fk_dcim_manu; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_inventoryitem
    ADD CONSTRAINT dcim_inventoryitem_manufacturer_id_dcd1b78a_fk_dcim_manu FOREIGN KEY (manufacturer_id) REFERENCES public.dcim_manufacturer(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_inventoryitem dcim_inventoryitem_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_inventoryitem
    ADD CONSTRAINT dcim_inventoryitem_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: dcim_inventoryitem dcim_inventoryitem_parent_id_7ebcd457_fk_dcim_inventoryitem_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_inventoryitem
    ADD CONSTRAINT dcim_inventoryitem_parent_id_7ebcd457_fk_dcim_inventoryitem_id FOREIGN KEY (parent_id) REFERENCES public.dcim_inventoryitem(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_inventoryitem dcim_inventoryitem_role_id_2bcfcb04_fk_dcim_inve; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_inventoryitem
    ADD CONSTRAINT dcim_inventoryitem_role_id_2bcfcb04_fk_dcim_inve FOREIGN KEY (role_id) REFERENCES public.dcim_inventoryitemrole(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_inventoryitemrole dcim_inventoryitemrole_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_inventoryitemrole
    ADD CONSTRAINT dcim_inventoryitemrole_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: dcim_inventoryitemtemplate dcim_inventoryitemte_device_type_id_6a1be904_fk_dcim_devi; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_inventoryitemtemplate
    ADD CONSTRAINT dcim_inventoryitemte_device_type_id_6a1be904_fk_dcim_devi FOREIGN KEY (device_type_id) REFERENCES public.dcim_devicetype(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_inventoryitemtemplate dcim_inventoryitemte_manufacturer_id_b388c5d9_fk_dcim_manu; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_inventoryitemtemplate
    ADD CONSTRAINT dcim_inventoryitemte_manufacturer_id_b388c5d9_fk_dcim_manu FOREIGN KEY (manufacturer_id) REFERENCES public.dcim_manufacturer(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_inventoryitemtemplate dcim_inventoryitemte_parent_id_0dac73bb_fk_dcim_inve; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_inventoryitemtemplate
    ADD CONSTRAINT dcim_inventoryitemte_parent_id_0dac73bb_fk_dcim_inve FOREIGN KEY (parent_id) REFERENCES public.dcim_inventoryitemtemplate(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_inventoryitemtemplate dcim_inventoryitemte_role_id_292676e6_fk_dcim_inve; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_inventoryitemtemplate
    ADD CONSTRAINT dcim_inventoryitemte_role_id_292676e6_fk_dcim_inve FOREIGN KEY (role_id) REFERENCES public.dcim_inventoryitemrole(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_location dcim_location_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_location
    ADD CONSTRAINT dcim_location_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: dcim_location dcim_location_parent_id_d77f3318_fk_dcim_location_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_location
    ADD CONSTRAINT dcim_location_parent_id_d77f3318_fk_dcim_location_id FOREIGN KEY (parent_id) REFERENCES public.dcim_location(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_location dcim_location_site_id_b55e975f_fk_dcim_site_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_location
    ADD CONSTRAINT dcim_location_site_id_b55e975f_fk_dcim_site_id FOREIGN KEY (site_id) REFERENCES public.dcim_site(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_location dcim_location_tenant_id_2c4df974_fk_tenancy_tenant_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_location
    ADD CONSTRAINT dcim_location_tenant_id_2c4df974_fk_tenancy_tenant_id FOREIGN KEY (tenant_id) REFERENCES public.tenancy_tenant(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_macaddress dcim_macaddress_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_macaddress
    ADD CONSTRAINT dcim_macaddress_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: dcim_manufacturer dcim_manufacturer_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_manufacturer
    ADD CONSTRAINT dcim_manufacturer_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: dcim_module dcim_module_device_id_53cfd5be_fk_dcim_device_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_module
    ADD CONSTRAINT dcim_module_device_id_53cfd5be_fk_dcim_device_id FOREIGN KEY (device_id) REFERENCES public.dcim_device(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_module dcim_module_module_type_id_a50b39fc_fk_dcim_moduletype_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_module
    ADD CONSTRAINT dcim_module_module_type_id_a50b39fc_fk_dcim_moduletype_id FOREIGN KEY (module_type_id) REFERENCES public.dcim_moduletype(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_module dcim_module_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_module
    ADD CONSTRAINT dcim_module_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: dcim_modulebay dcim_modulebay__location_id_17290069_fk_dcim_location_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_modulebay
    ADD CONSTRAINT dcim_modulebay__location_id_17290069_fk_dcim_location_id FOREIGN KEY (_location_id) REFERENCES public.dcim_location(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_modulebay dcim_modulebay__rack_id_673d8fb5_fk_dcim_rack_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_modulebay
    ADD CONSTRAINT dcim_modulebay__rack_id_673d8fb5_fk_dcim_rack_id FOREIGN KEY (_rack_id) REFERENCES public.dcim_rack(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_modulebay dcim_modulebay__site_id_cedd61bc_fk_dcim_site_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_modulebay
    ADD CONSTRAINT dcim_modulebay__site_id_cedd61bc_fk_dcim_site_id FOREIGN KEY (_site_id) REFERENCES public.dcim_site(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_modulebay dcim_modulebay_device_id_3526abc2_fk_dcim_device_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_modulebay
    ADD CONSTRAINT dcim_modulebay_device_id_3526abc2_fk_dcim_device_id FOREIGN KEY (device_id) REFERENCES public.dcim_device(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_modulebay dcim_modulebay_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_modulebay
    ADD CONSTRAINT dcim_modulebay_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: dcim_modulebaytemplate dcim_modulebaytempla_device_type_id_9eaf9bd3_fk_dcim_devi; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_modulebaytemplate
    ADD CONSTRAINT dcim_modulebaytempla_device_type_id_9eaf9bd3_fk_dcim_devi FOREIGN KEY (device_type_id) REFERENCES public.dcim_devicetype(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_modulebaytemplate dcim_modulebaytempla_module_type_id_2fdfb491_fk_dcim_modu; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_modulebaytemplate
    ADD CONSTRAINT dcim_modulebaytempla_module_type_id_2fdfb491_fk_dcim_modu FOREIGN KEY (module_type_id) REFERENCES public.dcim_moduletype(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_modulebaytype dcim_modulebaytype_manufacturer_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_modulebaytype
    ADD CONSTRAINT dcim_modulebaytype_manufacturer_id_fkey FOREIGN KEY (manufacturer_id) REFERENCES public.dcim_manufacturer(id) ON DELETE RESTRICT;


--
-- Name: dcim_modulebaytype dcim_modulebaytype_owner_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_modulebaytype
    ADD CONSTRAINT dcim_modulebaytype_owner_id_fkey FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: dcim_moduletype dcim_moduletype_manufacturer_id_7347392e_fk_dcim_manu; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_moduletype
    ADD CONSTRAINT dcim_moduletype_manufacturer_id_7347392e_fk_dcim_manu FOREIGN KEY (manufacturer_id) REFERENCES public.dcim_manufacturer(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_moduletype dcim_moduletype_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_moduletype
    ADD CONSTRAINT dcim_moduletype_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: dcim_moduletype dcim_moduletype_profile_id_62b5d02d_fk_dcim_modu; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_moduletype
    ADD CONSTRAINT dcim_moduletype_profile_id_62b5d02d_fk_dcim_modu FOREIGN KEY (profile_id) REFERENCES public.dcim_moduletypeprofile(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_moduletypeprofile dcim_moduletypeprofile_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_moduletypeprofile
    ADD CONSTRAINT dcim_moduletypeprofile_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: dcim_platform dcim_platform_manufacturer_id_83f72d3d_fk_dcim_manufacturer_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_platform
    ADD CONSTRAINT dcim_platform_manufacturer_id_83f72d3d_fk_dcim_manufacturer_id FOREIGN KEY (manufacturer_id) REFERENCES public.dcim_manufacturer(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_platform dcim_platform_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_platform
    ADD CONSTRAINT dcim_platform_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: dcim_platform dcim_platform_parent_id_795c7101_fk_dcim_platform_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_platform
    ADD CONSTRAINT dcim_platform_parent_id_795c7101_fk_dcim_platform_id FOREIGN KEY (parent_id) REFERENCES public.dcim_platform(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_portmapping dcim_portmapping_device_id_eb86e378_fk_dcim_device_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_portmapping
    ADD CONSTRAINT dcim_portmapping_device_id_eb86e378_fk_dcim_device_id FOREIGN KEY (device_id) REFERENCES public.dcim_device(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_portmapping dcim_portmapping_front_port_id_d8413d45_fk_dcim_frontport_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_portmapping
    ADD CONSTRAINT dcim_portmapping_front_port_id_d8413d45_fk_dcim_frontport_id FOREIGN KEY (front_port_id) REFERENCES public.dcim_frontport(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_portmapping dcim_portmapping_rear_port_id_dc3fb4b8_fk_dcim_rearport_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_portmapping
    ADD CONSTRAINT dcim_portmapping_rear_port_id_dc3fb4b8_fk_dcim_rearport_id FOREIGN KEY (rear_port_id) REFERENCES public.dcim_rearport(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_porttemplatemapping dcim_porttemplatemap_device_type_id_dede0eeb_fk_dcim_devi; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_porttemplatemapping
    ADD CONSTRAINT dcim_porttemplatemap_device_type_id_dede0eeb_fk_dcim_devi FOREIGN KEY (device_type_id) REFERENCES public.dcim_devicetype(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_porttemplatemapping dcim_porttemplatemap_front_port_id_090c3c11_fk_dcim_fron; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_porttemplatemapping
    ADD CONSTRAINT dcim_porttemplatemap_front_port_id_090c3c11_fk_dcim_fron FOREIGN KEY (front_port_id) REFERENCES public.dcim_frontporttemplate(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_porttemplatemapping dcim_porttemplatemap_module_type_id_3a84e529_fk_dcim_modu; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_porttemplatemapping
    ADD CONSTRAINT dcim_porttemplatemap_module_type_id_3a84e529_fk_dcim_modu FOREIGN KEY (module_type_id) REFERENCES public.dcim_moduletype(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_porttemplatemapping dcim_porttemplatemap_rear_port_id_93a9b08f_fk_dcim_rear; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_porttemplatemapping
    ADD CONSTRAINT dcim_porttemplatemap_rear_port_id_93a9b08f_fk_dcim_rear FOREIGN KEY (rear_port_id) REFERENCES public.dcim_rearporttemplate(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_powerfeed dcim_powerfeed__path_id_a1ea1f28_fk_dcim_cablepath_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_powerfeed
    ADD CONSTRAINT dcim_powerfeed__path_id_a1ea1f28_fk_dcim_cablepath_id FOREIGN KEY (_path_id) REFERENCES public.dcim_cablepath(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_powerfeed dcim_powerfeed_cable_id_ec44c4f8_fk_dcim_cable_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_powerfeed
    ADD CONSTRAINT dcim_powerfeed_cable_id_ec44c4f8_fk_dcim_cable_id FOREIGN KEY (cable_id) REFERENCES public.dcim_cable(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_powerfeed dcim_powerfeed_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_powerfeed
    ADD CONSTRAINT dcim_powerfeed_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: dcim_powerfeed dcim_powerfeed_power_panel_id_32bde3be_fk_dcim_powerpanel_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_powerfeed
    ADD CONSTRAINT dcim_powerfeed_power_panel_id_32bde3be_fk_dcim_powerpanel_id FOREIGN KEY (power_panel_id) REFERENCES public.dcim_powerpanel(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_powerfeed dcim_powerfeed_rack_id_7abba090_fk_dcim_rack_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_powerfeed
    ADD CONSTRAINT dcim_powerfeed_rack_id_7abba090_fk_dcim_rack_id FOREIGN KEY (rack_id) REFERENCES public.dcim_rack(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_powerfeed dcim_powerfeed_tenant_id_947bee85_fk_tenancy_tenant_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_powerfeed
    ADD CONSTRAINT dcim_powerfeed_tenant_id_947bee85_fk_tenancy_tenant_id FOREIGN KEY (tenant_id) REFERENCES public.tenancy_tenant(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_poweroutlet dcim_poweroutlet__location_id_49563316_fk_dcim_location_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_poweroutlet
    ADD CONSTRAINT dcim_poweroutlet__location_id_49563316_fk_dcim_location_id FOREIGN KEY (_location_id) REFERENCES public.dcim_location(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_poweroutlet dcim_poweroutlet__path_id_cbb47bb9_fk_dcim_cablepath_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_poweroutlet
    ADD CONSTRAINT dcim_poweroutlet__path_id_cbb47bb9_fk_dcim_cablepath_id FOREIGN KEY (_path_id) REFERENCES public.dcim_cablepath(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_poweroutlet dcim_poweroutlet__rack_id_aca89672_fk_dcim_rack_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_poweroutlet
    ADD CONSTRAINT dcim_poweroutlet__rack_id_aca89672_fk_dcim_rack_id FOREIGN KEY (_rack_id) REFERENCES public.dcim_rack(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_poweroutlet dcim_poweroutlet__site_id_a956755e_fk_dcim_site_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_poweroutlet
    ADD CONSTRAINT dcim_poweroutlet__site_id_a956755e_fk_dcim_site_id FOREIGN KEY (_site_id) REFERENCES public.dcim_site(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_poweroutlet dcim_poweroutlet_cable_id_8dbea1ec_fk_dcim_cable_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_poweroutlet
    ADD CONSTRAINT dcim_poweroutlet_cable_id_8dbea1ec_fk_dcim_cable_id FOREIGN KEY (cable_id) REFERENCES public.dcim_cable(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_poweroutlet dcim_poweroutlet_device_id_286351d7_fk_dcim_device_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_poweroutlet
    ADD CONSTRAINT dcim_poweroutlet_device_id_286351d7_fk_dcim_device_id FOREIGN KEY (device_id) REFERENCES public.dcim_device(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_poweroutlet dcim_poweroutlet_module_id_032f5af2_fk_dcim_module_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_poweroutlet
    ADD CONSTRAINT dcim_poweroutlet_module_id_032f5af2_fk_dcim_module_id FOREIGN KEY (module_id) REFERENCES public.dcim_module(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_poweroutlet dcim_poweroutlet_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_poweroutlet
    ADD CONSTRAINT dcim_poweroutlet_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: dcim_poweroutlet dcim_poweroutlet_power_port_id_9bdf4163_fk_dcim_powerport_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_poweroutlet
    ADD CONSTRAINT dcim_poweroutlet_power_port_id_9bdf4163_fk_dcim_powerport_id FOREIGN KEY (power_port_id) REFERENCES public.dcim_powerport(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_poweroutlettemplate dcim_poweroutlettemp_device_type_id_26b2316c_fk_dcim_devi; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_poweroutlettemplate
    ADD CONSTRAINT dcim_poweroutlettemp_device_type_id_26b2316c_fk_dcim_devi FOREIGN KEY (device_type_id) REFERENCES public.dcim_devicetype(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_poweroutlettemplate dcim_poweroutlettemp_module_type_id_6142b416_fk_dcim_modu; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_poweroutlettemplate
    ADD CONSTRAINT dcim_poweroutlettemp_module_type_id_6142b416_fk_dcim_modu FOREIGN KEY (module_type_id) REFERENCES public.dcim_moduletype(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_poweroutlettemplate dcim_poweroutlettemp_power_port_id_c0fb0c42_fk_dcim_powe; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_poweroutlettemplate
    ADD CONSTRAINT dcim_poweroutlettemp_power_port_id_c0fb0c42_fk_dcim_powe FOREIGN KEY (power_port_id) REFERENCES public.dcim_powerporttemplate(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_powerpanel dcim_powerpanel_location_id_474b60f8_fk_dcim_location_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_powerpanel
    ADD CONSTRAINT dcim_powerpanel_location_id_474b60f8_fk_dcim_location_id FOREIGN KEY (location_id) REFERENCES public.dcim_location(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_powerpanel dcim_powerpanel_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_powerpanel
    ADD CONSTRAINT dcim_powerpanel_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: dcim_powerpanel dcim_powerpanel_site_id_c430bc89_fk_dcim_site_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_powerpanel
    ADD CONSTRAINT dcim_powerpanel_site_id_c430bc89_fk_dcim_site_id FOREIGN KEY (site_id) REFERENCES public.dcim_site(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_powerport dcim_powerport__location_id_ccf995e7_fk_dcim_location_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_powerport
    ADD CONSTRAINT dcim_powerport__location_id_ccf995e7_fk_dcim_location_id FOREIGN KEY (_location_id) REFERENCES public.dcim_location(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_powerport dcim_powerport__path_id_9fe4af8f_fk_dcim_cablepath_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_powerport
    ADD CONSTRAINT dcim_powerport__path_id_9fe4af8f_fk_dcim_cablepath_id FOREIGN KEY (_path_id) REFERENCES public.dcim_cablepath(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_powerport dcim_powerport__rack_id_4c1c316d_fk_dcim_rack_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_powerport
    ADD CONSTRAINT dcim_powerport__rack_id_4c1c316d_fk_dcim_rack_id FOREIGN KEY (_rack_id) REFERENCES public.dcim_rack(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_powerport dcim_powerport__site_id_87bfa142_fk_dcim_site_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_powerport
    ADD CONSTRAINT dcim_powerport__site_id_87bfa142_fk_dcim_site_id FOREIGN KEY (_site_id) REFERENCES public.dcim_site(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_powerport dcim_powerport_cable_id_c9682ba2_fk_dcim_cable_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_powerport
    ADD CONSTRAINT dcim_powerport_cable_id_c9682ba2_fk_dcim_cable_id FOREIGN KEY (cable_id) REFERENCES public.dcim_cable(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_powerport dcim_powerport_device_id_ef7185ae_fk_dcim_device_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_powerport
    ADD CONSTRAINT dcim_powerport_device_id_ef7185ae_fk_dcim_device_id FOREIGN KEY (device_id) REFERENCES public.dcim_device(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_powerport dcim_powerport_module_id_d0c27534_fk_dcim_module_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_powerport
    ADD CONSTRAINT dcim_powerport_module_id_d0c27534_fk_dcim_module_id FOREIGN KEY (module_id) REFERENCES public.dcim_module(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_powerport dcim_powerport_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_powerport
    ADD CONSTRAINT dcim_powerport_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: dcim_powerporttemplate dcim_powerporttempla_device_type_id_1ddfbfcc_fk_dcim_devi; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_powerporttemplate
    ADD CONSTRAINT dcim_powerporttempla_device_type_id_1ddfbfcc_fk_dcim_devi FOREIGN KEY (device_type_id) REFERENCES public.dcim_devicetype(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_powerporttemplate dcim_powerporttempla_module_type_id_93e26849_fk_dcim_modu; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_powerporttemplate
    ADD CONSTRAINT dcim_powerporttempla_module_type_id_93e26849_fk_dcim_modu FOREIGN KEY (module_type_id) REFERENCES public.dcim_moduletype(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_rack dcim_rack_group_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_rack
    ADD CONSTRAINT dcim_rack_group_id_fkey FOREIGN KEY (group_id) REFERENCES public.dcim_rackgroup(id) ON DELETE RESTRICT;


--
-- Name: dcim_rack dcim_rack_location_id_5f63ec31_fk_dcim_location_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_rack
    ADD CONSTRAINT dcim_rack_location_id_5f63ec31_fk_dcim_location_id FOREIGN KEY (location_id) REFERENCES public.dcim_location(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_rack dcim_rack_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_rack
    ADD CONSTRAINT dcim_rack_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: dcim_rack dcim_rack_rack_type_id_39433a22_fk_dcim_racktype_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_rack
    ADD CONSTRAINT dcim_rack_rack_type_id_39433a22_fk_dcim_racktype_id FOREIGN KEY (rack_type_id) REFERENCES public.dcim_racktype(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_rack dcim_rack_role_id_62d6919e_fk_dcim_rackrole_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_rack
    ADD CONSTRAINT dcim_rack_role_id_62d6919e_fk_dcim_rackrole_id FOREIGN KEY (role_id) REFERENCES public.dcim_rackrole(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_rack dcim_rack_site_id_403c7b3a_fk_dcim_site_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_rack
    ADD CONSTRAINT dcim_rack_site_id_403c7b3a_fk_dcim_site_id FOREIGN KEY (site_id) REFERENCES public.dcim_site(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_rack dcim_rack_tenant_id_7cdf3725_fk_tenancy_tenant_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_rack
    ADD CONSTRAINT dcim_rack_tenant_id_7cdf3725_fk_tenancy_tenant_id FOREIGN KEY (tenant_id) REFERENCES public.tenancy_tenant(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_rackgroup dcim_rackgroup_owner_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_rackgroup
    ADD CONSTRAINT dcim_rackgroup_owner_id_fkey FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: dcim_rackreservation dcim_rackreservation_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_rackreservation
    ADD CONSTRAINT dcim_rackreservation_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: dcim_rackreservation dcim_rackreservation_rack_id_1ebbaa9b_fk_dcim_rack_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_rackreservation
    ADD CONSTRAINT dcim_rackreservation_rack_id_1ebbaa9b_fk_dcim_rack_id FOREIGN KEY (rack_id) REFERENCES public.dcim_rack(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_rackreservation dcim_rackreservation_tenant_id_eb5e045f_fk_tenancy_tenant_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_rackreservation
    ADD CONSTRAINT dcim_rackreservation_tenant_id_eb5e045f_fk_tenancy_tenant_id FOREIGN KEY (tenant_id) REFERENCES public.tenancy_tenant(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_rackrole dcim_rackrole_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_rackrole
    ADD CONSTRAINT dcim_rackrole_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: dcim_racktype dcim_racktype_manufacturer_id_d46a05c6_fk_dcim_manufacturer_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_racktype
    ADD CONSTRAINT dcim_racktype_manufacturer_id_d46a05c6_fk_dcim_manufacturer_id FOREIGN KEY (manufacturer_id) REFERENCES public.dcim_manufacturer(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_racktype dcim_racktype_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_racktype
    ADD CONSTRAINT dcim_racktype_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: dcim_rearport dcim_rearport__location_id_72554006_fk_dcim_location_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_rearport
    ADD CONSTRAINT dcim_rearport__location_id_72554006_fk_dcim_location_id FOREIGN KEY (_location_id) REFERENCES public.dcim_location(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_rearport dcim_rearport__rack_id_9af9a402_fk_dcim_rack_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_rearport
    ADD CONSTRAINT dcim_rearport__rack_id_9af9a402_fk_dcim_rack_id FOREIGN KEY (_rack_id) REFERENCES public.dcim_rack(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_rearport dcim_rearport__site_id_35e05ccf_fk_dcim_site_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_rearport
    ADD CONSTRAINT dcim_rearport__site_id_35e05ccf_fk_dcim_site_id FOREIGN KEY (_site_id) REFERENCES public.dcim_site(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_rearport dcim_rearport_cable_id_42c0e4e7_fk_dcim_cable_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_rearport
    ADD CONSTRAINT dcim_rearport_cable_id_42c0e4e7_fk_dcim_cable_id FOREIGN KEY (cable_id) REFERENCES public.dcim_cable(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_rearport dcim_rearport_device_id_0bdfe9c0_fk_dcim_device_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_rearport
    ADD CONSTRAINT dcim_rearport_device_id_0bdfe9c0_fk_dcim_device_id FOREIGN KEY (device_id) REFERENCES public.dcim_device(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_rearport dcim_rearport_module_id_9a7b7e91_fk_dcim_module_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_rearport
    ADD CONSTRAINT dcim_rearport_module_id_9a7b7e91_fk_dcim_module_id FOREIGN KEY (module_id) REFERENCES public.dcim_module(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_rearport dcim_rearport_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_rearport
    ADD CONSTRAINT dcim_rearport_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: dcim_rearporttemplate dcim_rearporttemplat_device_type_id_6a02fd01_fk_dcim_devi; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_rearporttemplate
    ADD CONSTRAINT dcim_rearporttemplat_device_type_id_6a02fd01_fk_dcim_devi FOREIGN KEY (device_type_id) REFERENCES public.dcim_devicetype(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_rearporttemplate dcim_rearporttemplat_module_type_id_4d970e5b_fk_dcim_modu; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_rearporttemplate
    ADD CONSTRAINT dcim_rearporttemplat_module_type_id_4d970e5b_fk_dcim_modu FOREIGN KEY (module_type_id) REFERENCES public.dcim_moduletype(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_region dcim_region_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_region
    ADD CONSTRAINT dcim_region_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: dcim_region dcim_region_parent_id_2486f5d4_fk_dcim_region_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_region
    ADD CONSTRAINT dcim_region_parent_id_2486f5d4_fk_dcim_region_id FOREIGN KEY (parent_id) REFERENCES public.dcim_region(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_site_asns dcim_site_asns_asn_id_3cfd0f00_fk_ipam_asn_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_site_asns
    ADD CONSTRAINT dcim_site_asns_asn_id_3cfd0f00_fk_ipam_asn_id FOREIGN KEY (asn_id) REFERENCES public.ipam_asn(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_site_asns dcim_site_asns_site_id_112ccacf_fk_dcim_site_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_site_asns
    ADD CONSTRAINT dcim_site_asns_site_id_112ccacf_fk_dcim_site_id FOREIGN KEY (site_id) REFERENCES public.dcim_site(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_site dcim_site_group_id_3910c975_fk_dcim_sitegroup_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_site
    ADD CONSTRAINT dcim_site_group_id_3910c975_fk_dcim_sitegroup_id FOREIGN KEY (group_id) REFERENCES public.dcim_sitegroup(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_site dcim_site_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_site
    ADD CONSTRAINT dcim_site_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: dcim_site dcim_site_region_id_45210932_fk_dcim_region_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_site
    ADD CONSTRAINT dcim_site_region_id_45210932_fk_dcim_region_id FOREIGN KEY (region_id) REFERENCES public.dcim_region(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_site dcim_site_tenant_id_15e7df63_fk_tenancy_tenant_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_site
    ADD CONSTRAINT dcim_site_tenant_id_15e7df63_fk_tenancy_tenant_id FOREIGN KEY (tenant_id) REFERENCES public.tenancy_tenant(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_sitegroup dcim_sitegroup_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_sitegroup
    ADD CONSTRAINT dcim_sitegroup_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: dcim_sitegroup dcim_sitegroup_parent_id_533a5e44_fk_dcim_sitegroup_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_sitegroup
    ADD CONSTRAINT dcim_sitegroup_parent_id_533a5e44_fk_dcim_sitegroup_id FOREIGN KEY (parent_id) REFERENCES public.dcim_sitegroup(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_virtualchassis dcim_virtualchassis_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_virtualchassis
    ADD CONSTRAINT dcim_virtualchassis_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: dcim_virtualdevicecontext dcim_virtualdeviceco_primary_ip4_id_6bd0605b_fk_ipam_ipad; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_virtualdevicecontext
    ADD CONSTRAINT dcim_virtualdeviceco_primary_ip4_id_6bd0605b_fk_ipam_ipad FOREIGN KEY (primary_ip4_id) REFERENCES public.ipam_ipaddress(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_virtualdevicecontext dcim_virtualdeviceco_primary_ip6_id_ed3b81bd_fk_ipam_ipad; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_virtualdevicecontext
    ADD CONSTRAINT dcim_virtualdeviceco_primary_ip6_id_ed3b81bd_fk_ipam_ipad FOREIGN KEY (primary_ip6_id) REFERENCES public.ipam_ipaddress(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_virtualdevicecontext dcim_virtualdeviceco_tenant_id_b6a21753_fk_tenancy_t; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_virtualdevicecontext
    ADD CONSTRAINT dcim_virtualdeviceco_tenant_id_b6a21753_fk_tenancy_t FOREIGN KEY (tenant_id) REFERENCES public.tenancy_tenant(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_virtualdevicecontext dcim_virtualdevicecontext_device_id_4f39274b_fk_dcim_device_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_virtualdevicecontext
    ADD CONSTRAINT dcim_virtualdevicecontext_device_id_4f39274b_fk_dcim_device_id FOREIGN KEY (device_id) REFERENCES public.dcim_device(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: dcim_virtualdevicecontext dcim_virtualdevicecontext_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.dcim_virtualdevicecontext
    ADD CONSTRAINT dcim_virtualdevicecontext_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: entity_mappings entity_mappings_mapping_config_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.entity_mappings
    ADD CONSTRAINT entity_mappings_mapping_config_fkey FOREIGN KEY (mapping_config_logical_id, mapping_config_version_id) REFERENCES public.mapping_configurations(logical_id, version_id) ON DELETE CASCADE;


--
-- Name: field_mappings field_mappings_entity_mapping_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.field_mappings
    ADD CONSTRAINT field_mappings_entity_mapping_id_fkey FOREIGN KEY (entity_mapping_id) REFERENCES public.entity_mappings(id) ON DELETE CASCADE;


--
-- Name: playbook fk_playbook_current_version; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook
    ADD CONSTRAINT fk_playbook_current_version FOREIGN KEY (current_version_id) REFERENCES public.playbook_version(id) ON DELETE SET NULL;


--
-- Name: playbook fk_playbook_default_target_set; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook
    ADD CONSTRAINT fk_playbook_default_target_set FOREIGN KEY (default_target_set_id) REFERENCES public.playbook_target_set(id) ON DELETE SET NULL;


--
-- Name: playbook_execution_environment fk_playbook_ee_created_by; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_execution_environment
    ADD CONSTRAINT fk_playbook_ee_created_by FOREIGN KEY (created_by_user_id) REFERENCES public.identity_user(id) ON DELETE SET NULL;


--
-- Name: playbook_execution_environment fk_playbook_ee_updated_by; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_execution_environment
    ADD CONSTRAINT fk_playbook_ee_updated_by FOREIGN KEY (updated_by_user_id) REFERENCES public.identity_user(id) ON DELETE SET NULL;


--
-- Name: playbook_job_template fk_playbook_job_template_created_by; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_job_template
    ADD CONSTRAINT fk_playbook_job_template_created_by FOREIGN KEY (created_by_user_id) REFERENCES public.identity_user(id) ON DELETE SET NULL;


--
-- Name: playbook_job_template fk_playbook_job_template_ee; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_job_template
    ADD CONSTRAINT fk_playbook_job_template_ee FOREIGN KEY (execution_environment_id) REFERENCES public.playbook_execution_environment(id) ON DELETE SET NULL;


--
-- Name: playbook_job_template fk_playbook_job_template_playbook; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_job_template
    ADD CONSTRAINT fk_playbook_job_template_playbook FOREIGN KEY (playbook_id) REFERENCES public.playbook(id) ON DELETE CASCADE;


--
-- Name: playbook_job_template fk_playbook_job_template_project; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_job_template
    ADD CONSTRAINT fk_playbook_job_template_project FOREIGN KEY (project_id) REFERENCES public.playbook_project(id) ON DELETE CASCADE;


--
-- Name: playbook_job_template_survey fk_playbook_job_template_survey_template; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_job_template_survey
    ADD CONSTRAINT fk_playbook_job_template_survey_template FOREIGN KEY (job_template_id) REFERENCES public.playbook_job_template(id) ON DELETE CASCADE;


--
-- Name: playbook_job_template fk_playbook_job_template_target_set; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_job_template
    ADD CONSTRAINT fk_playbook_job_template_target_set FOREIGN KEY (inventory_target_set_id) REFERENCES public.playbook_target_set(id) ON DELETE SET NULL;


--
-- Name: playbook_job_template fk_playbook_job_template_updated_by; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_job_template
    ADD CONSTRAINT fk_playbook_job_template_updated_by FOREIGN KEY (updated_by_user_id) REFERENCES public.identity_user(id) ON DELETE SET NULL;


--
-- Name: playbook_project fk_playbook_project_created_by; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_project
    ADD CONSTRAINT fk_playbook_project_created_by FOREIGN KEY (created_by_user_id) REFERENCES public.identity_user(id) ON DELETE SET NULL;


--
-- Name: playbook_project fk_playbook_project_default_ee; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_project
    ADD CONSTRAINT fk_playbook_project_default_ee FOREIGN KEY (default_execution_environment_id) REFERENCES public.playbook_execution_environment(id) ON DELETE SET NULL;


--
-- Name: playbook_project fk_playbook_project_updated_by; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_project
    ADD CONSTRAINT fk_playbook_project_updated_by FOREIGN KEY (updated_by_user_id) REFERENCES public.identity_user(id) ON DELETE SET NULL;


--
-- Name: identity_group_membership identity_group_membership_group_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.identity_group_membership
    ADD CONSTRAINT identity_group_membership_group_id_fkey FOREIGN KEY (group_id) REFERENCES public.identity_group(id) ON DELETE CASCADE;


--
-- Name: identity_group_membership identity_group_membership_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.identity_group_membership
    ADD CONSTRAINT identity_group_membership_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.identity_user(id) ON DELETE CASCADE;


--
-- Name: inventory_dataflow inventory_dataflow_destination_cluster_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_dataflow
    ADD CONSTRAINT inventory_dataflow_destination_cluster_id_fkey FOREIGN KEY (destination_cluster_id) REFERENCES public.virtualization_cluster(id) ON DELETE RESTRICT;


--
-- Name: inventory_dataflow inventory_dataflow_destination_clustergroup_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_dataflow
    ADD CONSTRAINT inventory_dataflow_destination_clustergroup_id_fkey FOREIGN KEY (destination_clustergroup_id) REFERENCES public.virtualization_clustergroup(id) ON DELETE RESTRICT;


--
-- Name: inventory_dataflow inventory_dataflow_destination_clustertype_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_dataflow
    ADD CONSTRAINT inventory_dataflow_destination_clustertype_id_fkey FOREIGN KEY (destination_clustertype_id) REFERENCES public.virtualization_clustertype(id) ON DELETE RESTRICT;


--
-- Name: inventory_dataflow inventory_dataflow_destination_device_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_dataflow
    ADD CONSTRAINT inventory_dataflow_destination_device_id_fkey FOREIGN KEY (destination_device_id) REFERENCES public.dcim_device(id) ON DELETE RESTRICT;


--
-- Name: inventory_dataflow inventory_dataflow_destination_devicerole_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_dataflow
    ADD CONSTRAINT inventory_dataflow_destination_devicerole_id_fkey FOREIGN KEY (destination_devicerole_id) REFERENCES public.dcim_devicerole(id) ON DELETE RESTRICT;


--
-- Name: inventory_dataflow inventory_dataflow_destination_devicetype_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_dataflow
    ADD CONSTRAINT inventory_dataflow_destination_devicetype_id_fkey FOREIGN KEY (destination_devicetype_id) REFERENCES public.dcim_devicetype(id) ON DELETE RESTRICT;


--
-- Name: inventory_dataflow inventory_dataflow_destination_ipaddress_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_dataflow
    ADD CONSTRAINT inventory_dataflow_destination_ipaddress_id_fkey FOREIGN KEY (destination_ipaddress_id) REFERENCES public.ipam_ipaddress(id) ON DELETE RESTRICT;


--
-- Name: inventory_dataflow inventory_dataflow_destination_location_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_dataflow
    ADD CONSTRAINT inventory_dataflow_destination_location_id_fkey FOREIGN KEY (destination_location_id) REFERENCES public.dcim_location(id) ON DELETE RESTRICT;


--
-- Name: inventory_dataflow inventory_dataflow_destination_manufacturer_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_dataflow
    ADD CONSTRAINT inventory_dataflow_destination_manufacturer_id_fkey FOREIGN KEY (destination_manufacturer_id) REFERENCES public.dcim_manufacturer(id) ON DELETE RESTRICT;


--
-- Name: inventory_dataflow inventory_dataflow_destination_platform_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_dataflow
    ADD CONSTRAINT inventory_dataflow_destination_platform_id_fkey FOREIGN KEY (destination_platform_id) REFERENCES public.dcim_platform(id) ON DELETE RESTRICT;


--
-- Name: inventory_dataflow inventory_dataflow_destination_prefix_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_dataflow
    ADD CONSTRAINT inventory_dataflow_destination_prefix_id_fkey FOREIGN KEY (destination_prefix_id) REFERENCES public.ipam_prefix(id) ON DELETE RESTRICT;


--
-- Name: inventory_dataflow inventory_dataflow_destination_rack_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_dataflow
    ADD CONSTRAINT inventory_dataflow_destination_rack_id_fkey FOREIGN KEY (destination_rack_id) REFERENCES public.dcim_rack(id) ON DELETE RESTRICT;


--
-- Name: inventory_dataflow inventory_dataflow_destination_region_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_dataflow
    ADD CONSTRAINT inventory_dataflow_destination_region_id_fkey FOREIGN KEY (destination_region_id) REFERENCES public.dcim_region(id) ON DELETE RESTRICT;


--
-- Name: inventory_dataflow inventory_dataflow_destination_service_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_dataflow
    ADD CONSTRAINT inventory_dataflow_destination_service_id_fkey FOREIGN KEY (destination_service_id) REFERENCES public.ipam_service(id) ON DELETE RESTRICT;


--
-- Name: inventory_dataflow inventory_dataflow_destination_site_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_dataflow
    ADD CONSTRAINT inventory_dataflow_destination_site_id_fkey FOREIGN KEY (destination_site_id) REFERENCES public.dcim_site(id) ON DELETE RESTRICT;


--
-- Name: inventory_dataflow inventory_dataflow_destination_sitegroup_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_dataflow
    ADD CONSTRAINT inventory_dataflow_destination_sitegroup_id_fkey FOREIGN KEY (destination_sitegroup_id) REFERENCES public.dcim_sitegroup(id) ON DELETE RESTRICT;


--
-- Name: inventory_dataflow inventory_dataflow_destination_tenant_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_dataflow
    ADD CONSTRAINT inventory_dataflow_destination_tenant_id_fkey FOREIGN KEY (destination_tenant_id) REFERENCES public.tenancy_tenant(id) ON DELETE RESTRICT;


--
-- Name: inventory_dataflow inventory_dataflow_destination_tenantgroup_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_dataflow
    ADD CONSTRAINT inventory_dataflow_destination_tenantgroup_id_fkey FOREIGN KEY (destination_tenantgroup_id) REFERENCES public.tenancy_tenantgroup(id) ON DELETE RESTRICT;


--
-- Name: inventory_dataflow inventory_dataflow_destination_vm_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_dataflow
    ADD CONSTRAINT inventory_dataflow_destination_vm_id_fkey FOREIGN KEY (destination_vm_id) REFERENCES public.virtualization_virtualmachine(id) ON DELETE RESTRICT;


--
-- Name: inventory_dataflow_selected_ip inventory_dataflow_selected_ip_dataflow_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_dataflow_selected_ip
    ADD CONSTRAINT inventory_dataflow_selected_ip_dataflow_id_fkey FOREIGN KEY (dataflow_id) REFERENCES public.inventory_dataflow(id) ON DELETE CASCADE;


--
-- Name: inventory_dataflow_selected_ip inventory_dataflow_selected_ip_ipaddress_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_dataflow_selected_ip
    ADD CONSTRAINT inventory_dataflow_selected_ip_ipaddress_id_fkey FOREIGN KEY (ipaddress_id) REFERENCES public.ipam_ipaddress(id) ON DELETE RESTRICT;


--
-- Name: inventory_dataflow inventory_dataflow_source_cluster_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_dataflow
    ADD CONSTRAINT inventory_dataflow_source_cluster_id_fkey FOREIGN KEY (source_cluster_id) REFERENCES public.virtualization_cluster(id) ON DELETE RESTRICT;


--
-- Name: inventory_dataflow inventory_dataflow_source_clustergroup_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_dataflow
    ADD CONSTRAINT inventory_dataflow_source_clustergroup_id_fkey FOREIGN KEY (source_clustergroup_id) REFERENCES public.virtualization_clustergroup(id) ON DELETE RESTRICT;


--
-- Name: inventory_dataflow inventory_dataflow_source_clustertype_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_dataflow
    ADD CONSTRAINT inventory_dataflow_source_clustertype_id_fkey FOREIGN KEY (source_clustertype_id) REFERENCES public.virtualization_clustertype(id) ON DELETE RESTRICT;


--
-- Name: inventory_dataflow inventory_dataflow_source_device_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_dataflow
    ADD CONSTRAINT inventory_dataflow_source_device_id_fkey FOREIGN KEY (source_device_id) REFERENCES public.dcim_device(id) ON DELETE RESTRICT;


--
-- Name: inventory_dataflow inventory_dataflow_source_devicerole_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_dataflow
    ADD CONSTRAINT inventory_dataflow_source_devicerole_id_fkey FOREIGN KEY (source_devicerole_id) REFERENCES public.dcim_devicerole(id) ON DELETE RESTRICT;


--
-- Name: inventory_dataflow inventory_dataflow_source_devicetype_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_dataflow
    ADD CONSTRAINT inventory_dataflow_source_devicetype_id_fkey FOREIGN KEY (source_devicetype_id) REFERENCES public.dcim_devicetype(id) ON DELETE RESTRICT;


--
-- Name: inventory_dataflow inventory_dataflow_source_ipaddress_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_dataflow
    ADD CONSTRAINT inventory_dataflow_source_ipaddress_id_fkey FOREIGN KEY (source_ipaddress_id) REFERENCES public.ipam_ipaddress(id) ON DELETE RESTRICT;


--
-- Name: inventory_dataflow inventory_dataflow_source_location_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_dataflow
    ADD CONSTRAINT inventory_dataflow_source_location_id_fkey FOREIGN KEY (source_location_id) REFERENCES public.dcim_location(id) ON DELETE RESTRICT;


--
-- Name: inventory_dataflow inventory_dataflow_source_manufacturer_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_dataflow
    ADD CONSTRAINT inventory_dataflow_source_manufacturer_id_fkey FOREIGN KEY (source_manufacturer_id) REFERENCES public.dcim_manufacturer(id) ON DELETE RESTRICT;


--
-- Name: inventory_dataflow inventory_dataflow_source_platform_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_dataflow
    ADD CONSTRAINT inventory_dataflow_source_platform_id_fkey FOREIGN KEY (source_platform_id) REFERENCES public.dcim_platform(id) ON DELETE RESTRICT;


--
-- Name: inventory_dataflow inventory_dataflow_source_prefix_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_dataflow
    ADD CONSTRAINT inventory_dataflow_source_prefix_id_fkey FOREIGN KEY (source_prefix_id) REFERENCES public.ipam_prefix(id) ON DELETE RESTRICT;


--
-- Name: inventory_dataflow inventory_dataflow_source_rack_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_dataflow
    ADD CONSTRAINT inventory_dataflow_source_rack_id_fkey FOREIGN KEY (source_rack_id) REFERENCES public.dcim_rack(id) ON DELETE RESTRICT;


--
-- Name: inventory_dataflow inventory_dataflow_source_region_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_dataflow
    ADD CONSTRAINT inventory_dataflow_source_region_id_fkey FOREIGN KEY (source_region_id) REFERENCES public.dcim_region(id) ON DELETE RESTRICT;


--
-- Name: inventory_dataflow inventory_dataflow_source_service_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_dataflow
    ADD CONSTRAINT inventory_dataflow_source_service_id_fkey FOREIGN KEY (source_service_id) REFERENCES public.ipam_service(id) ON DELETE RESTRICT;


--
-- Name: inventory_dataflow inventory_dataflow_source_site_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_dataflow
    ADD CONSTRAINT inventory_dataflow_source_site_id_fkey FOREIGN KEY (source_site_id) REFERENCES public.dcim_site(id) ON DELETE RESTRICT;


--
-- Name: inventory_dataflow inventory_dataflow_source_sitegroup_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_dataflow
    ADD CONSTRAINT inventory_dataflow_source_sitegroup_id_fkey FOREIGN KEY (source_sitegroup_id) REFERENCES public.dcim_sitegroup(id) ON DELETE RESTRICT;


--
-- Name: inventory_dataflow inventory_dataflow_source_tenant_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_dataflow
    ADD CONSTRAINT inventory_dataflow_source_tenant_id_fkey FOREIGN KEY (source_tenant_id) REFERENCES public.tenancy_tenant(id) ON DELETE RESTRICT;


--
-- Name: inventory_dataflow inventory_dataflow_source_tenantgroup_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_dataflow
    ADD CONSTRAINT inventory_dataflow_source_tenantgroup_id_fkey FOREIGN KEY (source_tenantgroup_id) REFERENCES public.tenancy_tenantgroup(id) ON DELETE RESTRICT;


--
-- Name: inventory_dataflow inventory_dataflow_source_vm_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_dataflow
    ADD CONSTRAINT inventory_dataflow_source_vm_id_fkey FOREIGN KEY (source_vm_id) REFERENCES public.virtualization_virtualmachine(id) ON DELETE RESTRICT;


--
-- Name: inventory_layout_system_profile inventory_layout_system_profile_updated_by_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_layout_system_profile
    ADD CONSTRAINT inventory_layout_system_profile_updated_by_user_id_fkey FOREIGN KEY (updated_by_user_id) REFERENCES public.identity_user(id) ON DELETE SET NULL;


--
-- Name: inventory_layout_user_profile inventory_layout_user_profile_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_layout_user_profile
    ADD CONSTRAINT inventory_layout_user_profile_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.identity_user(id) ON DELETE CASCADE;


--
-- Name: inventory_template inventory_template_owner_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_template
    ADD CONSTRAINT inventory_template_owner_user_id_fkey FOREIGN KEY (owner_user_id) REFERENCES public.identity_user(id) ON DELETE SET NULL;


--
-- Name: inventory_template_run inventory_template_run_requested_by_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_template_run
    ADD CONSTRAINT inventory_template_run_requested_by_user_id_fkey FOREIGN KEY (requested_by_user_id) REFERENCES public.identity_user(id) ON DELETE SET NULL;


--
-- Name: inventory_template_run_step_result inventory_template_run_step_result_run_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_template_run_step_result
    ADD CONSTRAINT inventory_template_run_step_result_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.inventory_template_run(id) ON DELETE CASCADE;


--
-- Name: inventory_template_run inventory_template_run_template_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_template_run
    ADD CONSTRAINT inventory_template_run_template_id_fkey FOREIGN KEY (template_id) REFERENCES public.inventory_template(id) ON DELETE CASCADE;


--
-- Name: inventory_template_step inventory_template_step_template_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_template_step
    ADD CONSTRAINT inventory_template_step_template_id_fkey FOREIGN KEY (template_id) REFERENCES public.inventory_template(id) ON DELETE CASCADE;


--
-- Name: inventory_template inventory_template_updated_by_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_template
    ADD CONSTRAINT inventory_template_updated_by_user_id_fkey FOREIGN KEY (updated_by_user_id) REFERENCES public.identity_user(id) ON DELETE SET NULL;


--
-- Name: inventory_template_variable inventory_template_variable_template_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.inventory_template_variable
    ADD CONSTRAINT inventory_template_variable_template_id_fkey FOREIGN KEY (template_id) REFERENCES public.inventory_template(id) ON DELETE CASCADE;


--
-- Name: ipam_aggregate ipam_aggregate_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_aggregate
    ADD CONSTRAINT ipam_aggregate_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: ipam_aggregate ipam_aggregate_rir_id_ef7a27bd_fk_ipam_rir_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_aggregate
    ADD CONSTRAINT ipam_aggregate_rir_id_ef7a27bd_fk_ipam_rir_id FOREIGN KEY (rir_id) REFERENCES public.ipam_rir(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: ipam_aggregate ipam_aggregate_tenant_id_637dd1a1_fk_tenancy_tenant_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_aggregate
    ADD CONSTRAINT ipam_aggregate_tenant_id_637dd1a1_fk_tenancy_tenant_id FOREIGN KEY (tenant_id) REFERENCES public.tenancy_tenant(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: ipam_asn ipam_asn_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_asn
    ADD CONSTRAINT ipam_asn_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: ipam_asn ipam_asn_rir_id_f5ad3cff_fk_ipam_rir_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_asn
    ADD CONSTRAINT ipam_asn_rir_id_f5ad3cff_fk_ipam_rir_id FOREIGN KEY (rir_id) REFERENCES public.ipam_rir(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: ipam_asn ipam_asn_tenant_id_07e8188e_fk_tenancy_tenant_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_asn
    ADD CONSTRAINT ipam_asn_tenant_id_07e8188e_fk_tenancy_tenant_id FOREIGN KEY (tenant_id) REFERENCES public.tenancy_tenant(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: ipam_asnrange ipam_asnrange_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_asnrange
    ADD CONSTRAINT ipam_asnrange_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: ipam_asnrange ipam_asnrange_rir_id_c9c31183_fk_ipam_rir_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_asnrange
    ADD CONSTRAINT ipam_asnrange_rir_id_c9c31183_fk_ipam_rir_id FOREIGN KEY (rir_id) REFERENCES public.ipam_rir(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: ipam_asnrange ipam_asnrange_tenant_id_ed8f80b7_fk_tenancy_tenant_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_asnrange
    ADD CONSTRAINT ipam_asnrange_tenant_id_ed8f80b7_fk_tenancy_tenant_id FOREIGN KEY (tenant_id) REFERENCES public.tenancy_tenant(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: ipam_fhrpgroup ipam_fhrpgroup_owner_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_fhrpgroup
    ADD CONSTRAINT ipam_fhrpgroup_owner_id_fkey FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: ipam_fhrpgroupassignment ipam_fhrpgroupassignment_group_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_fhrpgroupassignment
    ADD CONSTRAINT ipam_fhrpgroupassignment_group_id_fkey FOREIGN KEY (group_id) REFERENCES public.ipam_fhrpgroup(id) ON DELETE CASCADE;


--
-- Name: ipam_fhrpgroupassignment ipam_fhrpgroupassignment_interface_type_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_fhrpgroupassignment
    ADD CONSTRAINT ipam_fhrpgroupassignment_interface_type_id_fkey FOREIGN KEY (interface_type_id) REFERENCES public.inventory_content_type(id) ON DELETE CASCADE;


--
-- Name: ipam_ipaddress ipam_ipaddress_nat_inside_id_a45fb7c5_fk_ipam_ipaddress_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_ipaddress
    ADD CONSTRAINT ipam_ipaddress_nat_inside_id_a45fb7c5_fk_ipam_ipaddress_id FOREIGN KEY (nat_inside_id) REFERENCES public.ipam_ipaddress(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: ipam_ipaddress ipam_ipaddress_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_ipaddress
    ADD CONSTRAINT ipam_ipaddress_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: ipam_ipaddress ipam_ipaddress_tenant_id_ac55acfd_fk_tenancy_tenant_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_ipaddress
    ADD CONSTRAINT ipam_ipaddress_tenant_id_ac55acfd_fk_tenancy_tenant_id FOREIGN KEY (tenant_id) REFERENCES public.tenancy_tenant(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: ipam_ipaddress ipam_ipaddress_vrf_id_51fcc59b_fk_ipam_vrf_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_ipaddress
    ADD CONSTRAINT ipam_ipaddress_vrf_id_51fcc59b_fk_ipam_vrf_id FOREIGN KEY (vrf_id) REFERENCES public.ipam_vrf(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: ipam_iprange ipam_iprange_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_iprange
    ADD CONSTRAINT ipam_iprange_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: ipam_iprange ipam_iprange_role_id_2782e864_fk_ipam_role_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_iprange
    ADD CONSTRAINT ipam_iprange_role_id_2782e864_fk_ipam_role_id FOREIGN KEY (role_id) REFERENCES public.ipam_role(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: ipam_iprange ipam_iprange_tenant_id_856027ea_fk_tenancy_tenant_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_iprange
    ADD CONSTRAINT ipam_iprange_tenant_id_856027ea_fk_tenancy_tenant_id FOREIGN KEY (tenant_id) REFERENCES public.tenancy_tenant(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: ipam_iprange ipam_iprange_vrf_id_613e9dd2_fk_ipam_vrf_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_iprange
    ADD CONSTRAINT ipam_iprange_vrf_id_613e9dd2_fk_ipam_vrf_id FOREIGN KEY (vrf_id) REFERENCES public.ipam_vrf(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: vpn_l2vpn_export_targets ipam_l2vpn_export_ta_routetarget_id_5ccb758b_fk_ipam_rout; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_l2vpn_export_targets
    ADD CONSTRAINT ipam_l2vpn_export_ta_routetarget_id_5ccb758b_fk_ipam_rout FOREIGN KEY (routetarget_id) REFERENCES public.ipam_routetarget(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: vpn_l2vpn_export_targets ipam_l2vpn_export_targets_l2vpn_id_8749bbe8_fk_ipam_l2vpn_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_l2vpn_export_targets
    ADD CONSTRAINT ipam_l2vpn_export_targets_l2vpn_id_8749bbe8_fk_ipam_l2vpn_id FOREIGN KEY (l2vpn_id) REFERENCES public.vpn_l2vpn(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: vpn_l2vpn_import_targets ipam_l2vpn_import_ta_routetarget_id_58a188b2_fk_ipam_rout; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_l2vpn_import_targets
    ADD CONSTRAINT ipam_l2vpn_import_ta_routetarget_id_58a188b2_fk_ipam_rout FOREIGN KEY (routetarget_id) REFERENCES public.ipam_routetarget(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: vpn_l2vpn_import_targets ipam_l2vpn_import_targets_l2vpn_id_731f5bb4_fk_ipam_l2vpn_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_l2vpn_import_targets
    ADD CONSTRAINT ipam_l2vpn_import_targets_l2vpn_id_731f5bb4_fk_ipam_l2vpn_id FOREIGN KEY (l2vpn_id) REFERENCES public.vpn_l2vpn(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: ipam_prefix ipam_prefix__location_id_f5925c42_fk_dcim_location_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_prefix
    ADD CONSTRAINT ipam_prefix__location_id_f5925c42_fk_dcim_location_id FOREIGN KEY (_location_id) REFERENCES public.dcim_location(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: ipam_prefix ipam_prefix__region_id_54aedc72_fk_dcim_region_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_prefix
    ADD CONSTRAINT ipam_prefix__region_id_54aedc72_fk_dcim_region_id FOREIGN KEY (_region_id) REFERENCES public.dcim_region(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: ipam_prefix ipam_prefix__site_group_id_8277120b_fk_dcim_sitegroup_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_prefix
    ADD CONSTRAINT ipam_prefix__site_group_id_8277120b_fk_dcim_sitegroup_id FOREIGN KEY (_site_group_id) REFERENCES public.dcim_sitegroup(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: ipam_prefix ipam_prefix__site_id_b479fb05_fk_dcim_site_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_prefix
    ADD CONSTRAINT ipam_prefix__site_id_b479fb05_fk_dcim_site_id FOREIGN KEY (_site_id) REFERENCES public.dcim_site(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: ipam_prefix ipam_prefix_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_prefix
    ADD CONSTRAINT ipam_prefix_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: ipam_prefix ipam_prefix_role_id_0a98d415_fk_ipam_role_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_prefix
    ADD CONSTRAINT ipam_prefix_role_id_0a98d415_fk_ipam_role_id FOREIGN KEY (role_id) REFERENCES public.ipam_role(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: ipam_prefix ipam_prefix_tenant_id_7ba1fcc4_fk_tenancy_tenant_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_prefix
    ADD CONSTRAINT ipam_prefix_tenant_id_7ba1fcc4_fk_tenancy_tenant_id FOREIGN KEY (tenant_id) REFERENCES public.tenancy_tenant(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: ipam_prefix ipam_prefix_vlan_id_1db91bff_fk_ipam_vlan_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_prefix
    ADD CONSTRAINT ipam_prefix_vlan_id_1db91bff_fk_ipam_vlan_id FOREIGN KEY (vlan_id) REFERENCES public.ipam_vlan(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: ipam_prefix ipam_prefix_vrf_id_34f78ed0_fk_ipam_vrf_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_prefix
    ADD CONSTRAINT ipam_prefix_vrf_id_34f78ed0_fk_ipam_vrf_id FOREIGN KEY (vrf_id) REFERENCES public.ipam_vrf(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: ipam_rir ipam_rir_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_rir
    ADD CONSTRAINT ipam_rir_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: ipam_role ipam_role_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_role
    ADD CONSTRAINT ipam_role_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: ipam_routetarget ipam_routetarget_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_routetarget
    ADD CONSTRAINT ipam_routetarget_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: ipam_routetarget ipam_routetarget_tenant_id_5a0b35e8_fk_tenancy_tenant_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_routetarget
    ADD CONSTRAINT ipam_routetarget_tenant_id_5a0b35e8_fk_tenancy_tenant_id FOREIGN KEY (tenant_id) REFERENCES public.tenancy_tenant(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: ipam_service_ipaddresses ipam_service_ipaddre_ipaddress_id_b4138c6d_fk_ipam_ipad; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_service_ipaddresses
    ADD CONSTRAINT ipam_service_ipaddre_ipaddress_id_b4138c6d_fk_ipam_ipad FOREIGN KEY (ipaddress_id) REFERENCES public.ipam_ipaddress(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: ipam_service_ipaddresses ipam_service_ipaddresses_service_id_ae26b9ab_fk_ipam_service_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_service_ipaddresses
    ADD CONSTRAINT ipam_service_ipaddresses_service_id_ae26b9ab_fk_ipam_service_id FOREIGN KEY (service_id) REFERENCES public.ipam_service(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: ipam_service ipam_service_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_service
    ADD CONSTRAINT ipam_service_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: ipam_service_port_mapping ipam_service_port_mapping_service_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_service_port_mapping
    ADD CONSTRAINT ipam_service_port_mapping_service_fkey FOREIGN KEY (service_id) REFERENCES public.ipam_service(id) ON DELETE CASCADE;


--
-- Name: ipam_servicetemplate ipam_servicetemplate_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_servicetemplate
    ADD CONSTRAINT ipam_servicetemplate_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: ipam_vlan ipam_vlan_group_id_88cbfa62_fk_ipam_vlangroup_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_vlan
    ADD CONSTRAINT ipam_vlan_group_id_88cbfa62_fk_ipam_vlangroup_id FOREIGN KEY (group_id) REFERENCES public.ipam_vlangroup(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: ipam_vlan ipam_vlan_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_vlan
    ADD CONSTRAINT ipam_vlan_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: ipam_vlan ipam_vlan_qinq_svlan_id_acbd7a5d_fk_ipam_vlan_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_vlan
    ADD CONSTRAINT ipam_vlan_qinq_svlan_id_acbd7a5d_fk_ipam_vlan_id FOREIGN KEY (qinq_svlan_id) REFERENCES public.ipam_vlan(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: ipam_vlan ipam_vlan_role_id_f5015962_fk_ipam_role_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_vlan
    ADD CONSTRAINT ipam_vlan_role_id_f5015962_fk_ipam_role_id FOREIGN KEY (role_id) REFERENCES public.ipam_role(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: ipam_vlan ipam_vlan_site_id_a59334e3_fk_dcim_site_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_vlan
    ADD CONSTRAINT ipam_vlan_site_id_a59334e3_fk_dcim_site_id FOREIGN KEY (site_id) REFERENCES public.dcim_site(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: ipam_vlan ipam_vlan_tenant_id_71a8290d_fk_tenancy_tenant_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_vlan
    ADD CONSTRAINT ipam_vlan_tenant_id_71a8290d_fk_tenancy_tenant_id FOREIGN KEY (tenant_id) REFERENCES public.tenancy_tenant(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: ipam_vlangroup ipam_vlangroup_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_vlangroup
    ADD CONSTRAINT ipam_vlangroup_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: ipam_vlangroup ipam_vlangroup_tenant_id_29197fca_fk_tenancy_tenant_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_vlangroup
    ADD CONSTRAINT ipam_vlangroup_tenant_id_29197fca_fk_tenancy_tenant_id FOREIGN KEY (tenant_id) REFERENCES public.tenancy_tenant(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: ipam_vlantranslationrule ipam_vlantranslation_policy_id_09157735_fk_ipam_vlan; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_vlantranslationrule
    ADD CONSTRAINT ipam_vlantranslation_policy_id_09157735_fk_ipam_vlan FOREIGN KEY (policy_id) REFERENCES public.ipam_vlantranslationpolicy(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: ipam_vlantranslationpolicy ipam_vlantranslationpolicy_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_vlantranslationpolicy
    ADD CONSTRAINT ipam_vlantranslationpolicy_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: ipam_vrf_export_targets ipam_vrf_export_targ_routetarget_id_8d9319f7_fk_ipam_rout; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_vrf_export_targets
    ADD CONSTRAINT ipam_vrf_export_targ_routetarget_id_8d9319f7_fk_ipam_rout FOREIGN KEY (routetarget_id) REFERENCES public.ipam_routetarget(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: ipam_vrf_export_targets ipam_vrf_export_targets_vrf_id_6f4875c4_fk_ipam_vrf_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_vrf_export_targets
    ADD CONSTRAINT ipam_vrf_export_targets_vrf_id_6f4875c4_fk_ipam_vrf_id FOREIGN KEY (vrf_id) REFERENCES public.ipam_vrf(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: ipam_vrf_import_targets ipam_vrf_import_targ_routetarget_id_0e05b144_fk_ipam_rout; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_vrf_import_targets
    ADD CONSTRAINT ipam_vrf_import_targ_routetarget_id_0e05b144_fk_ipam_rout FOREIGN KEY (routetarget_id) REFERENCES public.ipam_routetarget(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: ipam_vrf_import_targets ipam_vrf_import_targets_vrf_id_ed491b19_fk_ipam_vrf_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_vrf_import_targets
    ADD CONSTRAINT ipam_vrf_import_targets_vrf_id_ed491b19_fk_ipam_vrf_id FOREIGN KEY (vrf_id) REFERENCES public.ipam_vrf(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: ipam_vrf ipam_vrf_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_vrf
    ADD CONSTRAINT ipam_vrf_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: ipam_vrf ipam_vrf_tenant_id_498b0051_fk_tenancy_tenant_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ipam_vrf
    ADD CONSTRAINT ipam_vrf_tenant_id_498b0051_fk_tenancy_tenant_id FOREIGN KEY (tenant_id) REFERENCES public.tenancy_tenant(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: license_seat_assignment license_seat_assignment_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.license_seat_assignment
    ADD CONSTRAINT license_seat_assignment_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.identity_user(id) ON DELETE CASCADE;


--
-- Name: license_seat_ledger license_seat_ledger_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.license_seat_ledger
    ADD CONSTRAINT license_seat_ledger_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.identity_user(id) ON DELETE SET NULL;


--
-- Name: mapping_configurations mapping_configurations_created_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.mapping_configurations
    ADD CONSTRAINT mapping_configurations_created_by_fkey FOREIGN KEY (created_by) REFERENCES public.identity_user(id) ON DELETE SET NULL;


--
-- Name: mapping_configurations mapping_configurations_source_provider_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.mapping_configurations
    ADD CONSTRAINT mapping_configurations_source_provider_id_fkey FOREIGN KEY (source_provider_id) REFERENCES public.sync_provider(id) ON DELETE CASCADE;


--
-- Name: mapping_configurations mapping_configurations_target_provider_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.mapping_configurations
    ADD CONSTRAINT mapping_configurations_target_provider_id_fkey FOREIGN KEY (target_provider_id) REFERENCES public.sync_provider(id) ON DELETE CASCADE;


--
-- Name: monitoring_alert_source_assignment monitoring_alert_source_assignment_source_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.monitoring_alert_source_assignment
    ADD CONSTRAINT monitoring_alert_source_assignment_source_id_fkey FOREIGN KEY (alert_source_id) REFERENCES public.monitoring_alert_source(id) ON DELETE CASCADE;


--
-- Name: monitoring_exporter_assignment monitoring_exporter_assignment_exporter_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.monitoring_exporter_assignment
    ADD CONSTRAINT monitoring_exporter_assignment_exporter_id_fkey FOREIGN KEY (exporter_id) REFERENCES public.monitoring_exporter(id) ON DELETE CASCADE;


--
-- Name: monitoring_metric_query monitoring_metric_query_exporter_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.monitoring_metric_query
    ADD CONSTRAINT monitoring_metric_query_exporter_id_fkey FOREIGN KEY (exporter_id) REFERENCES public.monitoring_exporter(id) ON DELETE CASCADE;


--
-- Name: monitoring_metric_query_host_override monitoring_metric_query_host_override_metric_query_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.monitoring_metric_query_host_override
    ADD CONSTRAINT monitoring_metric_query_host_override_metric_query_id_fkey FOREIGN KEY (metric_query_id) REFERENCES public.monitoring_metric_query(id) ON DELETE CASCADE;


--
-- Name: monitoring_metric_set_assignment monitoring_metric_set_assignment_metric_set_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.monitoring_metric_set_assignment
    ADD CONSTRAINT monitoring_metric_set_assignment_metric_set_id_fkey FOREIGN KEY (metric_set_id) REFERENCES public.monitoring_metric_set(id) ON DELETE CASCADE;


--
-- Name: monitoring_metric_set_item monitoring_metric_set_item_metric_query_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.monitoring_metric_set_item
    ADD CONSTRAINT monitoring_metric_set_item_metric_query_id_fkey FOREIGN KEY (metric_query_id) REFERENCES public.monitoring_metric_query(id) ON DELETE CASCADE;


--
-- Name: monitoring_metric_set_item monitoring_metric_set_item_metric_set_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.monitoring_metric_set_item
    ADD CONSTRAINT monitoring_metric_set_item_metric_set_id_fkey FOREIGN KEY (metric_set_id) REFERENCES public.monitoring_metric_set(id) ON DELETE CASCADE;


--
-- Name: monitoring_query_source_assignment monitoring_query_source_assignment_source_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.monitoring_query_source_assignment
    ADD CONSTRAINT monitoring_query_source_assignment_source_id_fkey FOREIGN KEY (query_source_id) REFERENCES public.monitoring_query_source(id) ON DELETE CASCADE;


--
-- Name: playbook_artifact playbook_artifact_run_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_artifact
    ADD CONSTRAINT playbook_artifact_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.playbook_run(id) ON DELETE CASCADE;


--
-- Name: playbook_assignment playbook_assignment_created_by_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_assignment
    ADD CONSTRAINT playbook_assignment_created_by_user_id_fkey FOREIGN KEY (created_by_user_id) REFERENCES public.identity_user(id) ON DELETE SET NULL;


--
-- Name: playbook_assignment playbook_assignment_default_version_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_assignment
    ADD CONSTRAINT playbook_assignment_default_version_id_fkey FOREIGN KEY (default_version_id) REFERENCES public.playbook_version(id) ON DELETE SET NULL;


--
-- Name: playbook_assignment playbook_assignment_playbook_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_assignment
    ADD CONSTRAINT playbook_assignment_playbook_id_fkey FOREIGN KEY (playbook_id) REFERENCES public.playbook(id) ON DELETE CASCADE;


--
-- Name: playbook_assignment playbook_assignment_target_set_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_assignment
    ADD CONSTRAINT playbook_assignment_target_set_id_fkey FOREIGN KEY (target_set_id) REFERENCES public.playbook_target_set(id) ON DELETE CASCADE;


--
-- Name: playbook_assignment playbook_assignment_updated_by_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_assignment
    ADD CONSTRAINT playbook_assignment_updated_by_user_id_fkey FOREIGN KEY (updated_by_user_id) REFERENCES public.identity_user(id) ON DELETE SET NULL;


--
-- Name: playbook playbook_created_by_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook
    ADD CONSTRAINT playbook_created_by_user_id_fkey FOREIGN KEY (created_by_user_id) REFERENCES public.identity_user(id) ON DELETE SET NULL;


--
-- Name: playbook_job_token playbook_job_token_run_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_job_token
    ADD CONSTRAINT playbook_job_token_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.playbook_run(id) ON DELETE CASCADE;


--
-- Name: playbook playbook_owner_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook
    ADD CONSTRAINT playbook_owner_user_id_fkey FOREIGN KEY (owner_user_id) REFERENCES public.identity_user(id) ON DELETE SET NULL;


--
-- Name: playbook_run_event playbook_run_event_run_group_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_run_event
    ADD CONSTRAINT playbook_run_event_run_group_id_fkey FOREIGN KEY (run_group_id) REFERENCES public.playbook_run_group(id) ON DELETE CASCADE;


--
-- Name: playbook_run_event playbook_run_event_run_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_run_event
    ADD CONSTRAINT playbook_run_event_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.playbook_run(id) ON DELETE CASCADE;


--
-- Name: playbook_run_group_callback_token playbook_run_group_callback_token_run_group_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_run_group_callback_token
    ADD CONSTRAINT playbook_run_group_callback_token_run_group_id_fkey FOREIGN KEY (run_group_id) REFERENCES public.playbook_run_group(id) ON DELETE CASCADE;


--
-- Name: playbook_run_group_event_batch playbook_run_group_event_batch_run_group_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_run_group_event_batch
    ADD CONSTRAINT playbook_run_group_event_batch_run_group_id_fkey FOREIGN KEY (run_group_id) REFERENCES public.playbook_run_group(id) ON DELETE CASCADE;


--
-- Name: playbook_run_group playbook_run_group_playbook_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_run_group
    ADD CONSTRAINT playbook_run_group_playbook_id_fkey FOREIGN KEY (playbook_id) REFERENCES public.playbook(id) ON DELETE RESTRICT;


--
-- Name: playbook_run_group playbook_run_group_playbook_version_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_run_group
    ADD CONSTRAINT playbook_run_group_playbook_version_id_fkey FOREIGN KEY (playbook_version_id) REFERENCES public.playbook_version(id) ON DELETE RESTRICT;


--
-- Name: playbook_run_group playbook_run_group_requested_by_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_run_group
    ADD CONSTRAINT playbook_run_group_requested_by_user_id_fkey FOREIGN KEY (requested_by_user_id) REFERENCES public.identity_user(id) ON DELETE SET NULL;


--
-- Name: playbook_run_group playbook_run_group_schedule_occurrence_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_run_group
    ADD CONSTRAINT playbook_run_group_schedule_occurrence_fkey FOREIGN KEY (schedule_id, scheduled_for) REFERENCES public.playbook_schedule_occurrence(schedule_id, scheduled_for) ON DELETE RESTRICT;


--
-- Name: playbook_run_group playbook_run_group_service_principal_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_run_group
    ADD CONSTRAINT playbook_run_group_service_principal_id_fkey FOREIGN KEY (service_principal_id) REFERENCES public.playbook_service_principal(id) ON DELETE SET NULL;


--
-- Name: playbook_run_group playbook_run_group_target_set_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_run_group
    ADD CONSTRAINT playbook_run_group_target_set_id_fkey FOREIGN KEY (target_set_id) REFERENCES public.playbook_target_set(id) ON DELETE SET NULL;


--
-- Name: playbook_run playbook_run_playbook_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_run
    ADD CONSTRAINT playbook_run_playbook_id_fkey FOREIGN KEY (playbook_id) REFERENCES public.playbook(id) ON DELETE RESTRICT;


--
-- Name: playbook_run playbook_run_playbook_version_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_run
    ADD CONSTRAINT playbook_run_playbook_version_id_fkey FOREIGN KEY (playbook_version_id) REFERENCES public.playbook_version(id) ON DELETE RESTRICT;


--
-- Name: playbook_run playbook_run_run_group_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_run
    ADD CONSTRAINT playbook_run_run_group_id_fkey FOREIGN KEY (run_group_id) REFERENCES public.playbook_run_group(id) ON DELETE CASCADE;


--
-- Name: playbook_schedule playbook_schedule_created_by_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_schedule
    ADD CONSTRAINT playbook_schedule_created_by_user_id_fkey FOREIGN KEY (created_by_user_id) REFERENCES public.identity_user(id) ON DELETE SET NULL;


--
-- Name: playbook_schedule_occurrence playbook_schedule_occurrence_schedule_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_schedule_occurrence
    ADD CONSTRAINT playbook_schedule_occurrence_schedule_id_fkey FOREIGN KEY (schedule_id) REFERENCES public.playbook_schedule(id) ON DELETE CASCADE;


--
-- Name: playbook_schedule playbook_schedule_playbook_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_schedule
    ADD CONSTRAINT playbook_schedule_playbook_id_fkey FOREIGN KEY (playbook_id) REFERENCES public.playbook(id) ON DELETE CASCADE;


--
-- Name: playbook_schedule playbook_schedule_service_principal_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_schedule
    ADD CONSTRAINT playbook_schedule_service_principal_id_fkey FOREIGN KEY (service_principal_id) REFERENCES public.playbook_service_principal(id) ON DELETE RESTRICT;


--
-- Name: playbook_schedule playbook_schedule_target_set_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_schedule
    ADD CONSTRAINT playbook_schedule_target_set_id_fkey FOREIGN KEY (target_set_id) REFERENCES public.playbook_target_set(id) ON DELETE RESTRICT;


--
-- Name: playbook_schedule playbook_schedule_updated_by_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_schedule
    ADD CONSTRAINT playbook_schedule_updated_by_user_id_fkey FOREIGN KEY (updated_by_user_id) REFERENCES public.identity_user(id) ON DELETE SET NULL;


--
-- Name: playbook_service_principal playbook_service_principal_created_by_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_service_principal
    ADD CONSTRAINT playbook_service_principal_created_by_user_id_fkey FOREIGN KEY (created_by_user_id) REFERENCES public.identity_user(id) ON DELETE SET NULL;


--
-- Name: playbook_service_principal_credential_binding playbook_service_principal_credential_b_created_by_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_service_principal_credential_binding
    ADD CONSTRAINT playbook_service_principal_credential_b_created_by_user_id_fkey FOREIGN KEY (created_by_user_id) REFERENCES public.identity_user(id) ON DELETE SET NULL;


--
-- Name: playbook_service_principal_credential_binding playbook_service_principal_credential_b_updated_by_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_service_principal_credential_binding
    ADD CONSTRAINT playbook_service_principal_credential_b_updated_by_user_id_fkey FOREIGN KEY (updated_by_user_id) REFERENCES public.identity_user(id) ON DELETE SET NULL;


--
-- Name: playbook_service_principal_credential_binding playbook_service_principal_credential_service_principal_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_service_principal_credential_binding
    ADD CONSTRAINT playbook_service_principal_credential_service_principal_id_fkey FOREIGN KEY (service_principal_id) REFERENCES public.playbook_service_principal(id) ON DELETE CASCADE;


--
-- Name: playbook_service_principal playbook_service_principal_revoked_by_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_service_principal
    ADD CONSTRAINT playbook_service_principal_revoked_by_user_id_fkey FOREIGN KEY (revoked_by_user_id) REFERENCES public.identity_user(id) ON DELETE SET NULL;


--
-- Name: playbook_service_principal playbook_service_principal_updated_by_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_service_principal
    ADD CONSTRAINT playbook_service_principal_updated_by_user_id_fkey FOREIGN KEY (updated_by_user_id) REFERENCES public.identity_user(id) ON DELETE SET NULL;


--
-- Name: playbook_target_set playbook_target_set_created_by_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_target_set
    ADD CONSTRAINT playbook_target_set_created_by_user_id_fkey FOREIGN KEY (created_by_user_id) REFERENCES public.identity_user(id) ON DELETE SET NULL;


--
-- Name: playbook_target_set_member playbook_target_set_member_added_by_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_target_set_member
    ADD CONSTRAINT playbook_target_set_member_added_by_user_id_fkey FOREIGN KEY (added_by_user_id) REFERENCES public.identity_user(id) ON DELETE SET NULL;


--
-- Name: playbook_target_set_member playbook_target_set_member_target_set_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_target_set_member
    ADD CONSTRAINT playbook_target_set_member_target_set_id_fkey FOREIGN KEY (target_set_id) REFERENCES public.playbook_target_set(id) ON DELETE CASCADE;


--
-- Name: playbook_target_set playbook_target_set_updated_by_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_target_set
    ADD CONSTRAINT playbook_target_set_updated_by_user_id_fkey FOREIGN KEY (updated_by_user_id) REFERENCES public.identity_user(id) ON DELETE SET NULL;


--
-- Name: playbook playbook_updated_by_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook
    ADD CONSTRAINT playbook_updated_by_user_id_fkey FOREIGN KEY (updated_by_user_id) REFERENCES public.identity_user(id) ON DELETE SET NULL;


--
-- Name: playbook_version playbook_version_created_by_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_version
    ADD CONSTRAINT playbook_version_created_by_user_id_fkey FOREIGN KEY (created_by_user_id) REFERENCES public.identity_user(id) ON DELETE SET NULL;


--
-- Name: playbook_version playbook_version_playbook_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.playbook_version
    ADD CONSTRAINT playbook_version_playbook_id_fkey FOREIGN KEY (playbook_id) REFERENCES public.playbook(id) ON DELETE CASCADE;


--
-- Name: provider_request_journal provider_request_journal_provider_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.provider_request_journal
    ADD CONSTRAINT provider_request_journal_provider_id_fkey FOREIGN KEY (provider_id) REFERENCES public.sync_provider(id) ON DELETE CASCADE;


--
-- Name: rbac_role_assignment rbac_role_assignment_group_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.rbac_role_assignment
    ADD CONSTRAINT rbac_role_assignment_group_id_fkey FOREIGN KEY (group_id) REFERENCES public.identity_group(id) ON DELETE CASCADE;


--
-- Name: rbac_role_assignment rbac_role_assignment_role_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.rbac_role_assignment
    ADD CONSTRAINT rbac_role_assignment_role_id_fkey FOREIGN KEY (role_id) REFERENCES public.rbac_role(id) ON DELETE CASCADE;


--
-- Name: rbac_role_assignment rbac_role_assignment_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.rbac_role_assignment
    ADD CONSTRAINT rbac_role_assignment_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.identity_user(id) ON DELETE CASCADE;


--
-- Name: secret_usage_binding secret_usage_binding_secret_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.secret_usage_binding
    ADD CONSTRAINT secret_usage_binding_secret_id_fkey FOREIGN KEY (secret_id) REFERENCES public.secrets_variable(id) ON DELETE CASCADE;


--
-- Name: secrets_binding secrets_binding_secret_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.secrets_binding
    ADD CONSTRAINT secrets_binding_secret_id_fkey FOREIGN KEY (secret_id) REFERENCES public.secrets_variable(id) ON DELETE CASCADE;


--
-- Name: secrets_store secrets_store_secret_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.secrets_store
    ADD CONSTRAINT secrets_store_secret_id_fkey FOREIGN KEY (secret_id) REFERENCES public.secrets_variable(id) ON DELETE CASCADE;


--
-- Name: settings_values settings_values_updated_by_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.settings_values
    ADD CONSTRAINT settings_values_updated_by_user_id_fkey FOREIGN KEY (updated_by_user_id) REFERENCES public.identity_user(id) ON DELETE SET NULL;


--
-- Name: settings_values settings_values_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.settings_values
    ADD CONSTRAINT settings_values_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.identity_user(id) ON DELETE CASCADE;


--
-- Name: sync_conflict_record sync_conflict_record_source_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.sync_conflict_record
    ADD CONSTRAINT sync_conflict_record_source_id_fkey FOREIGN KEY (source_id) REFERENCES public.sync_provider(id) ON DELETE CASCADE;


--
-- Name: sync_error sync_error_run_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.sync_error
    ADD CONSTRAINT sync_error_run_id_fkey FOREIGN KEY (run_id) REFERENCES public.sync_run(id) ON DELETE CASCADE;


--
-- Name: sync_inventory_binding sync_inventory_binding_last_seen_job_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.sync_inventory_binding
    ADD CONSTRAINT sync_inventory_binding_last_seen_job_fkey FOREIGN KEY (last_seen_job_id) REFERENCES public.sync_job(id) ON DELETE SET NULL;


--
-- Name: sync_inventory_binding sync_inventory_binding_source_provider_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.sync_inventory_binding
    ADD CONSTRAINT sync_inventory_binding_source_provider_fkey FOREIGN KEY (source_provider_id) REFERENCES public.sync_provider(id) ON DELETE CASCADE;


--
-- Name: sync_job sync_job_provider_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.sync_job
    ADD CONSTRAINT sync_job_provider_id_fkey FOREIGN KEY (provider_id) REFERENCES public.sync_provider(id) ON DELETE CASCADE;


--
-- Name: sync_job_step sync_job_step_job_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.sync_job_step
    ADD CONSTRAINT sync_job_step_job_id_fkey FOREIGN KEY (job_id) REFERENCES public.sync_job(id) ON DELETE CASCADE;


--
-- Name: sync_outbound_event sync_outbound_event_source_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.sync_outbound_event
    ADD CONSTRAINT sync_outbound_event_source_id_fkey FOREIGN KEY (source_id) REFERENCES public.sync_provider(id) ON DELETE CASCADE;


--
-- Name: sync_run sync_run_job_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.sync_run
    ADD CONSTRAINT sync_run_job_id_fkey FOREIGN KEY (job_id) REFERENCES public.sync_job(id) ON DELETE CASCADE;


--
-- Name: sync_source_runtime sync_source_runtime_source_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.sync_source_runtime
    ADD CONSTRAINT sync_source_runtime_source_id_fkey FOREIGN KEY (source_id) REFERENCES public.sync_provider(id) ON DELETE CASCADE;


--
-- Name: sync_webhook_event sync_webhook_event_source_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.sync_webhook_event
    ADD CONSTRAINT sync_webhook_event_source_id_fkey FOREIGN KEY (source_id) REFERENCES public.sync_provider(id) ON DELETE CASCADE;


--
-- Name: tenancy_contact_groups tenancy_contact_grou_contact_id_84c9d84f_fk_tenancy_c; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tenancy_contact_groups
    ADD CONSTRAINT tenancy_contact_grou_contact_id_84c9d84f_fk_tenancy_c FOREIGN KEY (contact_id) REFERENCES public.tenancy_contact(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: tenancy_contact_groups tenancy_contact_grou_contactgroup_id_5c8d6c5a_fk_tenancy_c; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tenancy_contact_groups
    ADD CONSTRAINT tenancy_contact_grou_contactgroup_id_5c8d6c5a_fk_tenancy_c FOREIGN KEY (contactgroup_id) REFERENCES public.tenancy_contactgroup(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: tenancy_contact tenancy_contact_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tenancy_contact
    ADD CONSTRAINT tenancy_contact_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: tenancy_contactassignment tenancy_contactassig_contact_id_5302baf0_fk_tenancy_c; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tenancy_contactassignment
    ADD CONSTRAINT tenancy_contactassig_contact_id_5302baf0_fk_tenancy_c FOREIGN KEY (contact_id) REFERENCES public.tenancy_contact(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: tenancy_contactassignment tenancy_contactassig_role_id_fc08bfb5_fk_tenancy_c; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tenancy_contactassignment
    ADD CONSTRAINT tenancy_contactassig_role_id_fc08bfb5_fk_tenancy_c FOREIGN KEY (role_id) REFERENCES public.tenancy_contactrole(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: tenancy_contactgroup tenancy_contactgroup_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tenancy_contactgroup
    ADD CONSTRAINT tenancy_contactgroup_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: tenancy_contactgroup tenancy_contactgroup_parent_id_c087d69f_fk_tenancy_c; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tenancy_contactgroup
    ADD CONSTRAINT tenancy_contactgroup_parent_id_c087d69f_fk_tenancy_c FOREIGN KEY (parent_id) REFERENCES public.tenancy_contactgroup(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: tenancy_contactrole tenancy_contactrole_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tenancy_contactrole
    ADD CONSTRAINT tenancy_contactrole_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: tenancy_tenant tenancy_tenant_group_id_7daef6f4_fk_tenancy_tenantgroup_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tenancy_tenant
    ADD CONSTRAINT tenancy_tenant_group_id_7daef6f4_fk_tenancy_tenantgroup_id FOREIGN KEY (group_id) REFERENCES public.tenancy_tenantgroup(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: tenancy_tenant tenancy_tenant_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tenancy_tenant
    ADD CONSTRAINT tenancy_tenant_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: tenancy_tenantgroup tenancy_tenantgroup_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tenancy_tenantgroup
    ADD CONSTRAINT tenancy_tenantgroup_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: tenancy_tenantgroup tenancy_tenantgroup_parent_id_2542fc18_fk_tenancy_t; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tenancy_tenantgroup
    ADD CONSTRAINT tenancy_tenantgroup_parent_id_2542fc18_fk_tenancy_t FOREIGN KEY (parent_id) REFERENCES public.tenancy_tenantgroup(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: user_preferences user_preferences_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_preferences
    ADD CONSTRAINT user_preferences_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.identity_user(id) ON DELETE CASCADE;


--
-- Name: users_owner users_owner_group_id_fk_users_ownergroup_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.users_owner
    ADD CONSTRAINT users_owner_group_id_fk_users_ownergroup_id FOREIGN KEY (group_id) REFERENCES public.users_ownergroup(id) ON DELETE RESTRICT;


--
-- Name: users_owner_identity_group users_owner_identity_group_group_id_fk_identity_group_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.users_owner_identity_group
    ADD CONSTRAINT users_owner_identity_group_group_id_fk_identity_group_id FOREIGN KEY (group_id) REFERENCES public.identity_group(id) ON DELETE CASCADE;


--
-- Name: users_owner_identity_group users_owner_identity_group_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.users_owner_identity_group
    ADD CONSTRAINT users_owner_identity_group_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE CASCADE;


--
-- Name: users_owner_identity_user users_owner_identity_user_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.users_owner_identity_user
    ADD CONSTRAINT users_owner_identity_user_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE CASCADE;


--
-- Name: users_owner_identity_user users_owner_identity_user_user_id_fk_identity_user_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.users_owner_identity_user
    ADD CONSTRAINT users_owner_identity_user_user_id_fk_identity_user_id FOREIGN KEY (user_id) REFERENCES public.identity_user(id) ON DELETE CASCADE;


--
-- Name: virtualization_cluster virtualization_clust__location_id_f553e386_fk_dcim_loca; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_cluster
    ADD CONSTRAINT virtualization_clust__location_id_f553e386_fk_dcim_loca FOREIGN KEY (_location_id) REFERENCES public.dcim_location(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: virtualization_cluster virtualization_clust__site_group_id_7d9bff8f_fk_dcim_site; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_cluster
    ADD CONSTRAINT virtualization_clust__site_group_id_7d9bff8f_fk_dcim_site FOREIGN KEY (_site_group_id) REFERENCES public.dcim_sitegroup(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: virtualization_cluster virtualization_clust_group_id_de379828_fk_virtualiz; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_cluster
    ADD CONSTRAINT virtualization_clust_group_id_de379828_fk_virtualiz FOREIGN KEY (group_id) REFERENCES public.virtualization_clustergroup(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: virtualization_cluster virtualization_clust_type_id_4efafb0a_fk_virtualiz; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_cluster
    ADD CONSTRAINT virtualization_clust_type_id_4efafb0a_fk_virtualiz FOREIGN KEY (type_id) REFERENCES public.virtualization_clustertype(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: virtualization_cluster virtualization_cluster__region_id_9244325e_fk_dcim_region_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_cluster
    ADD CONSTRAINT virtualization_cluster__region_id_9244325e_fk_dcim_region_id FOREIGN KEY (_region_id) REFERENCES public.dcim_region(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: virtualization_cluster virtualization_cluster__site_id_883df848_fk_dcim_site_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_cluster
    ADD CONSTRAINT virtualization_cluster__site_id_883df848_fk_dcim_site_id FOREIGN KEY (_site_id) REFERENCES public.dcim_site(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: virtualization_cluster virtualization_cluster_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_cluster
    ADD CONSTRAINT virtualization_cluster_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: virtualization_cluster virtualization_cluster_tenant_id_bc2868d0_fk_tenancy_tenant_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_cluster
    ADD CONSTRAINT virtualization_cluster_tenant_id_bc2868d0_fk_tenancy_tenant_id FOREIGN KEY (tenant_id) REFERENCES public.tenancy_tenant(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: virtualization_clustergroup virtualization_clustergroup_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_clustergroup
    ADD CONSTRAINT virtualization_clustergroup_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: virtualization_clustertype virtualization_clustertype_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_clustertype
    ADD CONSTRAINT virtualization_clustertype_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: virtualization_virtualmachine virtualization_virtu_cluster_id_6c9f9047_fk_virtualiz; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_virtualmachine
    ADD CONSTRAINT virtualization_virtu_cluster_id_6c9f9047_fk_virtualiz FOREIGN KEY (cluster_id) REFERENCES public.virtualization_cluster(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: virtualization_virtualmachine virtualization_virtu_device_id_5a49ed18_fk_dcim_devi; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_virtualmachine
    ADD CONSTRAINT virtualization_virtu_device_id_5a49ed18_fk_dcim_devi FOREIGN KEY (device_id) REFERENCES public.dcim_device(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: virtualization_virtualmachine virtualization_virtu_platform_id_a6c5ccb2_fk_dcim_plat; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_virtualmachine
    ADD CONSTRAINT virtualization_virtu_platform_id_a6c5ccb2_fk_dcim_plat FOREIGN KEY (platform_id) REFERENCES public.dcim_platform(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: virtualization_virtualmachine virtualization_virtu_primary_ip4_id_942e42ae_fk_ipam_ipad; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_virtualmachine
    ADD CONSTRAINT virtualization_virtu_primary_ip4_id_942e42ae_fk_ipam_ipad FOREIGN KEY (primary_ip4_id) REFERENCES public.ipam_ipaddress(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: virtualization_virtualmachine virtualization_virtu_primary_ip6_id_b7904e73_fk_ipam_ipad; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_virtualmachine
    ADD CONSTRAINT virtualization_virtu_primary_ip6_id_b7904e73_fk_ipam_ipad FOREIGN KEY (primary_ip6_id) REFERENCES public.ipam_ipaddress(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: virtualization_virtualmachine virtualization_virtu_role_id_0cc898f9_fk_dcim_devi; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_virtualmachine
    ADD CONSTRAINT virtualization_virtu_role_id_0cc898f9_fk_dcim_devi FOREIGN KEY (role_id) REFERENCES public.dcim_devicerole(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: virtualization_virtualmachine virtualization_virtu_tenant_id_d00d1d77_fk_tenancy_t; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_virtualmachine
    ADD CONSTRAINT virtualization_virtu_tenant_id_d00d1d77_fk_tenancy_t FOREIGN KEY (tenant_id) REFERENCES public.tenancy_tenant(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: virtualization_virtualdisk virtualization_virtu_virtual_machine_id_7bc8b6c2_fk_virtualiz; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_virtualdisk
    ADD CONSTRAINT virtualization_virtu_virtual_machine_id_7bc8b6c2_fk_virtualiz FOREIGN KEY (virtual_machine_id) REFERENCES public.virtualization_virtualmachine(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: virtualization_virtualdisk virtualization_virtualdisk_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_virtualdisk
    ADD CONSTRAINT virtualization_virtualdisk_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: virtualization_virtualmachine virtualization_virtualmachine_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_virtualmachine
    ADD CONSTRAINT virtualization_virtualmachine_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: virtualization_virtualmachine virtualization_virtualmachine_site_id_54475a27_fk_dcim_site_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_virtualmachine
    ADD CONSTRAINT virtualization_virtualmachine_site_id_54475a27_fk_dcim_site_id FOREIGN KEY (site_id) REFERENCES public.dcim_site(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: virtualization_virtualmachine virtualization_virtualmachine_virtual_machine_type_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_virtualmachine
    ADD CONSTRAINT virtualization_virtualmachine_virtual_machine_type_id_fkey FOREIGN KEY (virtual_machine_type_id) REFERENCES public.virtualization_virtualmachinetype(id) ON DELETE RESTRICT;


--
-- Name: virtualization_virtualmachinetype virtualization_virtualmachinetype_default_platform_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_virtualmachinetype
    ADD CONSTRAINT virtualization_virtualmachinetype_default_platform_id_fkey FOREIGN KEY (default_platform_id) REFERENCES public.dcim_platform(id) ON DELETE SET NULL;


--
-- Name: virtualization_virtualmachinetype virtualization_virtualmachinetype_owner_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_virtualmachinetype
    ADD CONSTRAINT virtualization_virtualmachinetype_owner_id_fkey FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: virtualization_vminterface virtualization_vmint_bridge_id_7462b91e_fk_virtualiz; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_vminterface
    ADD CONSTRAINT virtualization_vmint_bridge_id_7462b91e_fk_virtualiz FOREIGN KEY (bridge_id) REFERENCES public.virtualization_vminterface(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: virtualization_vminterface virtualization_vmint_parent_id_f86958e1_fk_virtualiz; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_vminterface
    ADD CONSTRAINT virtualization_vmint_parent_id_f86958e1_fk_virtualiz FOREIGN KEY (parent_id) REFERENCES public.virtualization_vminterface(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: virtualization_vminterface virtualization_vmint_primary_mac_address__7a6c9b66_fk_dcim_maca; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_vminterface
    ADD CONSTRAINT virtualization_vmint_primary_mac_address__7a6c9b66_fk_dcim_maca FOREIGN KEY (primary_mac_address_id) REFERENCES public.dcim_macaddress(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: virtualization_vminterface virtualization_vmint_qinq_svlan_id_afacb024_fk_ipam_vlan; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_vminterface
    ADD CONSTRAINT virtualization_vmint_qinq_svlan_id_afacb024_fk_ipam_vlan FOREIGN KEY (qinq_svlan_id) REFERENCES public.ipam_vlan(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: virtualization_vminterface virtualization_vmint_untagged_vlan_id_aea4fc69_fk_ipam_vlan; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_vminterface
    ADD CONSTRAINT virtualization_vmint_untagged_vlan_id_aea4fc69_fk_ipam_vlan FOREIGN KEY (untagged_vlan_id) REFERENCES public.ipam_vlan(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: virtualization_vminterface virtualization_vmint_virtual_machine_id_e9f89829_fk_virtualiz; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_vminterface
    ADD CONSTRAINT virtualization_vmint_virtual_machine_id_e9f89829_fk_virtualiz FOREIGN KEY (virtual_machine_id) REFERENCES public.virtualization_virtualmachine(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: virtualization_vminterface_tagged_vlans virtualization_vmint_vlan_id_4e77411e_fk_ipam_vlan; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_vminterface_tagged_vlans
    ADD CONSTRAINT virtualization_vmint_vlan_id_4e77411e_fk_ipam_vlan FOREIGN KEY (vlan_id) REFERENCES public.ipam_vlan(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: virtualization_vminterface virtualization_vmint_vlan_translation_pol_938e6637_fk_ipam_vlan; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_vminterface
    ADD CONSTRAINT virtualization_vmint_vlan_translation_pol_938e6637_fk_ipam_vlan FOREIGN KEY (vlan_translation_policy_id) REFERENCES public.ipam_vlantranslationpolicy(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: virtualization_vminterface_tagged_vlans virtualization_vmint_vminterface_id_904b12de_fk_virtualiz; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_vminterface_tagged_vlans
    ADD CONSTRAINT virtualization_vmint_vminterface_id_904b12de_fk_virtualiz FOREIGN KEY (vminterface_id) REFERENCES public.virtualization_vminterface(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: virtualization_vminterface virtualization_vminterface_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_vminterface
    ADD CONSTRAINT virtualization_vminterface_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: virtualization_vminterface virtualization_vminterface_vrf_id_4b570a8c_fk_ipam_vrf_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.virtualization_vminterface
    ADD CONSTRAINT virtualization_vminterface_vrf_id_4b570a8c_fk_ipam_vrf_id FOREIGN KEY (vrf_id) REFERENCES public.ipam_vrf(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: vpn_ikepolicy vpn_ikepolicy_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_ikepolicy
    ADD CONSTRAINT vpn_ikepolicy_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: vpn_ikepolicy_proposals vpn_ikepolicy_propos_ikepolicy_id_1e1deaab_fk_vpn_ikepo; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_ikepolicy_proposals
    ADD CONSTRAINT vpn_ikepolicy_propos_ikepolicy_id_1e1deaab_fk_vpn_ikepo FOREIGN KEY (ikepolicy_id) REFERENCES public.vpn_ikepolicy(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: vpn_ikepolicy_proposals vpn_ikepolicy_propos_ikeproposal_id_a9ead252_fk_vpn_ikepr; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_ikepolicy_proposals
    ADD CONSTRAINT vpn_ikepolicy_propos_ikeproposal_id_a9ead252_fk_vpn_ikepr FOREIGN KEY (ikeproposal_id) REFERENCES public.vpn_ikeproposal(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: vpn_ikeproposal vpn_ikeproposal_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_ikeproposal
    ADD CONSTRAINT vpn_ikeproposal_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: vpn_ipsecpolicy vpn_ipsecpolicy_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_ipsecpolicy
    ADD CONSTRAINT vpn_ipsecpolicy_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: vpn_ipsecpolicy_proposals vpn_ipsecpolicy_prop_ipsecpolicy_id_0e7771a1_fk_vpn_ipsec; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_ipsecpolicy_proposals
    ADD CONSTRAINT vpn_ipsecpolicy_prop_ipsecpolicy_id_0e7771a1_fk_vpn_ipsec FOREIGN KEY (ipsecpolicy_id) REFERENCES public.vpn_ipsecpolicy(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: vpn_ipsecpolicy_proposals vpn_ipsecpolicy_prop_ipsecproposal_id_685fe509_fk_vpn_ipsec; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_ipsecpolicy_proposals
    ADD CONSTRAINT vpn_ipsecpolicy_prop_ipsecproposal_id_685fe509_fk_vpn_ipsec FOREIGN KEY (ipsecproposal_id) REFERENCES public.vpn_ipsecproposal(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: vpn_ipsecprofile vpn_ipsecprofile_ike_policy_id_4eff5fb9_fk_vpn_ikepolicy_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_ipsecprofile
    ADD CONSTRAINT vpn_ipsecprofile_ike_policy_id_4eff5fb9_fk_vpn_ikepolicy_id FOREIGN KEY (ike_policy_id) REFERENCES public.vpn_ikepolicy(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: vpn_ipsecprofile vpn_ipsecprofile_ipsec_policy_id_e06f2323_fk_vpn_ipsecpolicy_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_ipsecprofile
    ADD CONSTRAINT vpn_ipsecprofile_ipsec_policy_id_e06f2323_fk_vpn_ipsecpolicy_id FOREIGN KEY (ipsec_policy_id) REFERENCES public.vpn_ipsecpolicy(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: vpn_ipsecprofile vpn_ipsecprofile_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_ipsecprofile
    ADD CONSTRAINT vpn_ipsecprofile_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: vpn_ipsecproposal vpn_ipsecproposal_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_ipsecproposal
    ADD CONSTRAINT vpn_ipsecproposal_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: vpn_l2vpn vpn_l2vpn_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_l2vpn
    ADD CONSTRAINT vpn_l2vpn_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: vpn_l2vpn vpn_l2vpn_tenant_id_57ec8f92_fk_tenancy_tenant_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_l2vpn
    ADD CONSTRAINT vpn_l2vpn_tenant_id_57ec8f92_fk_tenancy_tenant_id FOREIGN KEY (tenant_id) REFERENCES public.tenancy_tenant(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: vpn_l2vpntermination vpn_l2vpntermination_l2vpn_id_f5367bbe_fk_vpn_l2vpn_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_l2vpntermination
    ADD CONSTRAINT vpn_l2vpntermination_l2vpn_id_f5367bbe_fk_vpn_l2vpn_id FOREIGN KEY (l2vpn_id) REFERENCES public.vpn_l2vpn(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: vpn_tunnel vpn_tunnel_group_id_56b1e7f7_fk_vpn_tunnelgroup_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_tunnel
    ADD CONSTRAINT vpn_tunnel_group_id_56b1e7f7_fk_vpn_tunnelgroup_id FOREIGN KEY (group_id) REFERENCES public.vpn_tunnelgroup(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: vpn_tunnel vpn_tunnel_ipsec_profile_id_fd0361c5_fk_vpn_ipsecprofile_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_tunnel
    ADD CONSTRAINT vpn_tunnel_ipsec_profile_id_fd0361c5_fk_vpn_ipsecprofile_id FOREIGN KEY (ipsec_profile_id) REFERENCES public.vpn_ipsecprofile(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: vpn_tunnel vpn_tunnel_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_tunnel
    ADD CONSTRAINT vpn_tunnel_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: vpn_tunnel vpn_tunnel_tenant_id_f3df2ab3_fk_tenancy_tenant_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_tunnel
    ADD CONSTRAINT vpn_tunnel_tenant_id_f3df2ab3_fk_tenancy_tenant_id FOREIGN KEY (tenant_id) REFERENCES public.tenancy_tenant(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: vpn_tunnelgroup vpn_tunnelgroup_owner_id_fk_users_owner_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_tunnelgroup
    ADD CONSTRAINT vpn_tunnelgroup_owner_id_fk_users_owner_id FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: vpn_tunneltermination vpn_tunnelterminatio_outside_ip_id_2c6f3a7c_fk_ipam_ipad; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_tunneltermination
    ADD CONSTRAINT vpn_tunnelterminatio_outside_ip_id_2c6f3a7c_fk_ipam_ipad FOREIGN KEY (outside_ip_id) REFERENCES public.ipam_ipaddress(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: vpn_tunneltermination vpn_tunneltermination_tunnel_id_962efa25_fk_vpn_tunnel_id; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.vpn_tunneltermination
    ADD CONSTRAINT vpn_tunneltermination_tunnel_id_962efa25_fk_vpn_tunnel_id FOREIGN KEY (tunnel_id) REFERENCES public.vpn_tunnel(id) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: wireless_wirelesslan wireless_wirelesslan_group_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.wireless_wirelesslan
    ADD CONSTRAINT wireless_wirelesslan_group_id_fkey FOREIGN KEY (group_id) REFERENCES public.wireless_wirelesslangroup(id) ON DELETE SET NULL;


--
-- Name: wireless_wirelesslan wireless_wirelesslan_owner_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.wireless_wirelesslan
    ADD CONSTRAINT wireless_wirelesslan_owner_id_fkey FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: wireless_wirelesslan wireless_wirelesslan_scope_type_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.wireless_wirelesslan
    ADD CONSTRAINT wireless_wirelesslan_scope_type_id_fkey FOREIGN KEY (scope_type_id) REFERENCES public.inventory_content_type(id) ON DELETE RESTRICT;


--
-- Name: wireless_wirelesslan wireless_wirelesslan_tenant_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.wireless_wirelesslan
    ADD CONSTRAINT wireless_wirelesslan_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenancy_tenant(id) ON DELETE RESTRICT;


--
-- Name: wireless_wirelesslan wireless_wirelesslan_vlan_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.wireless_wirelesslan
    ADD CONSTRAINT wireless_wirelesslan_vlan_id_fkey FOREIGN KEY (vlan_id) REFERENCES public.ipam_vlan(id) ON DELETE RESTRICT;


--
-- Name: wireless_wirelesslangroup wireless_wirelesslangroup_owner_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.wireless_wirelesslangroup
    ADD CONSTRAINT wireless_wirelesslangroup_owner_id_fkey FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: wireless_wirelesslangroup wireless_wirelesslangroup_parent_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.wireless_wirelesslangroup
    ADD CONSTRAINT wireless_wirelesslangroup_parent_id_fkey FOREIGN KEY (parent_id) REFERENCES public.wireless_wirelesslangroup(id) ON DELETE CASCADE;


--
-- Name: wireless_wirelesslink wireless_wirelesslink_interface_a_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.wireless_wirelesslink
    ADD CONSTRAINT wireless_wirelesslink_interface_a_id_fkey FOREIGN KEY (interface_a_id) REFERENCES public.dcim_interface(id) ON DELETE RESTRICT;


--
-- Name: wireless_wirelesslink wireless_wirelesslink_interface_b_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.wireless_wirelesslink
    ADD CONSTRAINT wireless_wirelesslink_interface_b_id_fkey FOREIGN KEY (interface_b_id) REFERENCES public.dcim_interface(id) ON DELETE RESTRICT;


--
-- Name: wireless_wirelesslink wireless_wirelesslink_owner_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.wireless_wirelesslink
    ADD CONSTRAINT wireless_wirelesslink_owner_id_fkey FOREIGN KEY (owner_id) REFERENCES public.users_owner(id) ON DELETE RESTRICT;


--
-- Name: wireless_wirelesslink wireless_wirelesslink_tenant_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.wireless_wirelesslink
    ADD CONSTRAINT wireless_wirelesslink_tenant_id_fkey FOREIGN KEY (tenant_id) REFERENCES public.tenancy_tenant(id) ON DELETE RESTRICT;


--
-- Name: wizard_progress wizard_progress_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.wizard_progress
    ADD CONSTRAINT wizard_progress_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.identity_user(id) ON DELETE CASCADE;


--
-- PostgreSQL database dump complete
--
