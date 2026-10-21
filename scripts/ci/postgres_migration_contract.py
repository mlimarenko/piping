"""Lossless, server-native catalog used by migration and deployment guards."""

import json
import hashlib
from dataclasses import dataclass, field


# RI trigger names contain database-local OIDs. Internal identities instead bind
# constraint relation/name, trigger relation, function and event semantics; the
# firing mode remains part of the definition for both user and internal triggers.
CATALOG_SQL = """
WITH namespaces AS (
 SELECT oid,nspname FROM pg_namespace
 WHERE nspname NOT LIKE 'pg\\_%' ESCAPE '\\' AND nspname <> 'information_schema'
), objects AS (
 SELECT 'namespace' AS kind,nspname::text AS name,to_jsonb(nspname) AS definition
 FROM namespaces
 UNION ALL
 SELECT 'relation',n.nspname || '.' || c.relname,
 jsonb_build_object('kind',c.relkind,'persistence',c.relpersistence,'options',c.reloptions,
                   'row_security',c.relrowsecurity,'force_row_security',c.relforcerowsecurity)
 FROM pg_class c JOIN namespaces n ON n.oid=c.relnamespace
 WHERE c.relkind IN ('r','p','v','m','S','f')
 UNION ALL
 SELECT 'column',n.nspname || '.' || c.relname || '.' || a.attname,
 jsonb_build_object('position',a.attnum,'type',format_type(a.atttypid,a.atttypmod),
 'not_null',a.attnotnull,'identity',a.attidentity,'generated',a.attgenerated,
 'collation',co.collname,'default',pg_get_expr(d.adbin,d.adrelid))
 FROM pg_attribute a JOIN pg_class c ON c.oid=a.attrelid
 JOIN namespaces n ON n.oid=c.relnamespace
 LEFT JOIN pg_attrdef d ON d.adrelid=c.oid AND d.adnum=a.attnum
 LEFT JOIN pg_collation co ON co.oid=a.attcollation
 WHERE a.attnum>0 AND NOT a.attisdropped AND c.relkind IN ('r','p','v','m','f')
 UNION ALL
 SELECT 'constraint',n.nspname || '.' || coalesce(c.relname,t.typname) || '.' || k.conname,
 to_jsonb(pg_get_constraintdef(k.oid))
 FROM pg_constraint k JOIN namespaces n ON n.oid=k.connamespace
 LEFT JOIN pg_class c ON c.oid=k.conrelid LEFT JOIN pg_type t ON t.oid=k.contypid
 UNION ALL
 SELECT 'index',n.nspname || '.' || c.relname,to_jsonb(pg_get_indexdef(c.oid))
 FROM pg_class c JOIN namespaces n ON n.oid=c.relnamespace WHERE c.relkind IN ('i','I')
 UNION ALL
 SELECT 'function',n.nspname || '.' || p.proname || '(' || pg_get_function_identity_arguments(p.oid) || ')',
 to_jsonb(pg_get_functiondef(p.oid))
 FROM pg_proc p JOIN namespaces n ON n.oid=p.pronamespace WHERE p.prokind IN ('f','p')
 UNION ALL
 SELECT 'enum',n.nspname || '.' || t.typname || '.' || e.enumsortorder,to_jsonb(e.enumlabel)
 FROM pg_enum e JOIN pg_type t ON t.oid=e.enumtypid JOIN namespaces n ON n.oid=t.typnamespace
 UNION ALL
 SELECT 'view',n.nspname || '.' || c.relname,to_jsonb(pg_get_viewdef(c.oid))
 FROM pg_class c JOIN namespaces n ON n.oid=c.relnamespace WHERE c.relkind IN ('v','m')
 UNION ALL
 SELECT 'trigger',
 CASE WHEN t.tgisinternal THEN jsonb_build_array(
   n.nspname,c.relname,'internal',kn.nspname,kc.relname,k.conname,
   pn.nspname,p.proname,pg_get_function_identity_arguments(p.oid),t.tgtype
 )::text ELSE n.nspname || '.' || c.relname || '.' || t.tgname END,
 jsonb_build_object(
   'enabled',t.tgenabled,'internal',t.tgisinternal,'events',t.tgtype,
   'function',pn.nspname || '.' || p.proname || '(' || pg_get_function_identity_arguments(p.oid) || ')',
   'deferrable',t.tgdeferrable,'initially_deferred',t.tginitdeferred,
   'constraint',CASE WHEN k.oid IS NULL THEN NULL ELSE jsonb_build_object(
      'schema',kn.nspname,'relation',kc.relname,'name',k.conname,'definition',pg_get_constraintdef(k.oid)) END,
   'referenced_relation',CASE WHEN rc.oid IS NULL THEN NULL ELSE jsonb_build_array(rn.nspname,rc.relname) END,
   'columns',(SELECT coalesce(jsonb_agg(a.attname ORDER BY a.attnum),'[]'::jsonb)
              FROM pg_attribute a WHERE a.attrelid=t.tgrelid AND a.attnum=ANY(t.tgattr)),
   'arguments',encode(t.tgargs,'hex'),'condition_present',t.tgqual IS NOT NULL,
   'parent_relation',CASE WHEN pc.oid IS NULL THEN NULL ELSE jsonb_build_array(pcn.nspname,pc.relname) END,
   'create',CASE WHEN t.tgisinternal THEN NULL ELSE pg_get_triggerdef(t.oid) END
 )
 FROM pg_trigger t JOIN pg_class c ON c.oid=t.tgrelid
 JOIN namespaces n ON n.oid=c.relnamespace
 JOIN pg_proc p ON p.oid=t.tgfoid JOIN pg_namespace pn ON pn.oid=p.pronamespace
 LEFT JOIN pg_constraint k ON k.oid=t.tgconstraint
 LEFT JOIN pg_namespace kn ON kn.oid=k.connamespace LEFT JOIN pg_class kc ON kc.oid=k.conrelid
 LEFT JOIN pg_class rc ON rc.oid=t.tgconstrrelid LEFT JOIN pg_namespace rn ON rn.oid=rc.relnamespace
 LEFT JOIN pg_trigger pt ON pt.oid=t.tgparentid LEFT JOIN pg_class pc ON pc.oid=pt.tgrelid
 LEFT JOIN pg_namespace pcn ON pcn.oid=pc.relnamespace
 UNION ALL
 SELECT 'type',n.nspname || '.' || t.typname,
 jsonb_build_object('kind',t.typtype,'base',format_type(t.typbasetype,t.typtypmod),
                   'not_null',t.typnotnull,'default',t.typdefault)
 FROM pg_type t JOIN namespaces n ON n.oid=t.typnamespace WHERE t.typtype IN ('d','e')
 UNION ALL
 SELECT 'policy',n.nspname || '.' || c.relname || '.' || p.polname,
 jsonb_build_object('command',p.polcmd,'permissive',p.polpermissive,
 'using',pg_get_expr(p.polqual,p.polrelid),'check',pg_get_expr(p.polwithcheck,p.polrelid))
 FROM pg_policy p JOIN pg_class c ON c.oid=p.polrelid JOIN namespaces n ON n.oid=c.relnamespace
 UNION ALL
  SELECT 'extension',e.extname,to_jsonb(e.extversion) FROM pg_extension e
  UNION ALL
  SELECT 'collation',n.nspname || '.' || c.collname,
  jsonb_build_object('provider',c.collprovider,'deterministic',c.collisdeterministic,
                    'encoding',c.collencoding,'collate',c.collcollate,'ctype',c.collctype,
                    'locale',c.colllocale,'rules',c.collicurules,'version',c.collversion)
  FROM pg_collation c JOIN namespaces n ON n.oid=c.collnamespace
  UNION ALL
 SELECT 'sequence',n.nspname || '.' || c.relname,
 jsonb_build_object('type',format_type(s.seqtypid,NULL),'start',s.seqstart,'increment',s.seqincrement,
 'maximum',s.seqmax,'minimum',s.seqmin,'cache',s.seqcache,'cycle',s.seqcycle)
 FROM pg_sequence s JOIN pg_class c ON c.oid=s.seqrelid JOIN namespaces n ON n.oid=c.relnamespace
 UNION ALL
 SELECT 'event_trigger',evtname,
 jsonb_build_object('event',evtevent,'enabled',evtenabled,'tags',evttags,'function',evtfoid::regprocedure::text)
 FROM pg_event_trigger
)
SELECT coalesce(jsonb_agg(to_jsonb(objects) ORDER BY kind COLLATE "C",name COLLATE "C"),'[]'::jsonb)
FROM objects;
"""


def catalog_objects(raw):
    objects = json.loads(raw)
    if not isinstance(objects, list) or any(
        not isinstance(o, dict) or set(o) != {"kind", "name", "definition"}
        or not isinstance(o["kind"], str) or not isinstance(o["name"], str)
        for o in objects
    ):
        raise ValueError("Invalid native catalog")
    if len({(o["kind"], o["name"]) for o in objects}) != len(objects):
        raise ValueError("Schema catalog identities must remain lossless and unique")
    return objects


def definition_hash(definition):
    """Hash the complete JSON definition, not its display rendering."""
    return hashlib.sha256(json.dumps(
        definition, sort_keys=True, separators=(",", ":"), ensure_ascii=False,
        allow_nan=False,
    ).encode()).hexdigest()


def _catalog_index(objects):
    return {(o["kind"], o["name"]): o["definition"]
            for o in catalog_objects(json.dumps(objects, allow_nan=False))}


def preservation_extensions(canonical, installed, *, trusted_inventory=()):
    """Require the entire extra inventory to equal an externally approved one.

    Approval authentication, target binding and expiry belong to the caller.
    Observing an object never grants approval; names and prefixes grant none.
    """
    expected, actual = _catalog_index(canonical), _catalog_index(installed)
    inventory = {}
    for item in trusted_inventory:
        if (not isinstance(item, dict)
                or set(item) != {"kind", "name", "definition_sha256"}
                or not all(isinstance(value, str) for value in item.values())
                or len(item["definition_sha256"]) != 64
                or any(c not in "0123456789abcdef" for c in item["definition_sha256"])):
            raise ValueError("Invalid trusted extension inventory")
        identity = item["kind"], item["name"]
        if identity in expected or identity in inventory:
            raise ValueError("Extension inventory must be unique and noncanonical")
        inventory[identity] = item["definition_sha256"]
    extras = {key: definition_hash(value) for key, value in actual.items()
              if key not in expected}
    if extras != inventory:
        raise ValueError("Installed extras do not equal the trusted inventory")
    return inventory


def migration_catalog(before, *, additions=(), replacements=()):
    """Apply an explicitly authorized delta, binding both sides of each replacement."""
    expected = _catalog_index(before)
    replaced = set()
    for replacement in replacements:
        if (not isinstance(replacement, dict)
                or set(replacement) != {"kind", "name", "before", "after"}
                or replacement["kind"] != "constraint"):
            raise ValueError("Invalid authorized constraint replacement")
        key = replacement["kind"], replacement["name"]
        if key in replaced or key not in expected or expected[key] != replacement["before"]:
            raise ValueError("Constraint replacement does not bind the sealed target")
        replaced.add(key)
        expected[key] = replacement["after"]
    for key, value in _catalog_index(additions).items():
        if key in expected:
            raise ValueError("An addition cannot replace a sealed target object")
        expected[key] = value
    return [{"kind": key[0], "name": key[1], "definition": value} for key, value in sorted(expected.items())]


def exact_target_preservation(before, after, *, additions=(), replacements=()):
    """A sealed target remains exact outside the source-authorized migration delta."""
    expected = _catalog_index(migration_catalog(before, additions=additions, replacements=replacements))
    if expected != _catalog_index(after):
        raise ValueError("Sealed target catalog was not preserved exactly")


# Native node trees retain source token offsets. They are not execution semantics.
# No other node field is removed or rewritten (including OIDs and attnums).
CHECK_BINDINGS_SQL = r"""
SELECT coalesce(jsonb_agg(jsonb_build_object(
 'name', n.nspname || '.' || c.relname || '.' || k.conname,
 'definition', pg_get_constraintdef(k.oid),
 'ast', regexp_replace(k.conbin::text, ':(location|list_start|list_end) -?[0-9]+', ':\1 -1', 'g'),
 'validated', k.convalidated, 'enforced', k.conenforced,
 'no_inherit', k.connoinherit,
 'columns', (SELECT jsonb_agg(jsonb_build_object(
    'name', a.attname, 'position', a.attnum,
    'type', format_type(a.atttypid,a.atttypmod),
    'collation', CASE WHEN a.attcollation=0 THEN NULL ELSE
       cn.nspname || '.' || co.collname END,
    'deterministic', co.collisdeterministic) ORDER BY a.attnum)
   FROM pg_attribute a LEFT JOIN pg_collation co ON co.oid=a.attcollation
   LEFT JOIN pg_namespace cn ON cn.oid=co.collnamespace
   WHERE a.attrelid=k.conrelid AND a.attnum=ANY(k.conkey)),
 'dependencies', (SELECT coalesce(jsonb_agg(
    pg_describe_object(d.refclassid,d.refobjid,d.refobjsubid)
    ORDER BY pg_describe_object(d.refclassid,d.refobjid,d.refobjsubid)), '[]'::jsonb)
   FROM pg_depend d WHERE d.classid='pg_constraint'::regclass AND d.objid=k.oid
     AND NOT (d.refclassid='pg_class'::regclass AND d.refobjid=k.conrelid))
) ORDER BY k.conname), '[]'::jsonb)
FROM pg_constraint k JOIN pg_class c ON c.oid=k.conrelid
JOIN pg_namespace n ON n.oid=c.relnamespace
WHERE k.contype='c' AND k.conrelid=%s::regclass;
"""


@dataclass(frozen=True)
class CheckForm:
    """Exact source-registered syntax, never a general expression normalizer."""

    name: str
    columns: tuple
    canonical: str
    historical: str
    canonical_sql: str
    historical_sql: str
    members: tuple = ()
    atoms: tuple = ()


HISTORICAL_SOURCE_SHA = "77f58d4c7ac58a6792efe1044c9684bb9316da32"
HISTORICAL_POSTGRES_IMAGE = "sha256:54451ecb8ab38c24c3ec123f2fd501303a3a1856a5c66e98cecf2460d5e1e9d7"
# SHA-256 of the canonical JSON {migration path: SQLx SHA-384}, versions 1..22.
HISTORICAL_PREFIX_HASH = "45d7d65473b8fe39947105fe6350824fb9ef224b6968596bf86e2aca39316485"
REGISTERED_CHECK_SOURCES = {
    HISTORICAL_SOURCE_SHA: (HISTORICAL_PREFIX_HASH, HISTORICAL_POSTGRES_IMAGE),
    "96c9833e6a4cfdee0eff9c206fcd2cbee9e8dd53": (HISTORICAL_PREFIX_HASH, HISTORICAL_POSTGRES_IMAGE),
    "70254dc18cd36e8717de88284ba7456a12f50742": (HISTORICAL_PREFIX_HASH, HISTORICAL_POSTGRES_IMAGE),
    "ccb216481bcbc4d54315205269753e5d9d276d29": (HISTORICAL_PREFIX_HASH, HISTORICAL_POSTGRES_IMAGE),
    "5e4b78bfe647ea55829f24992b944cd2a8392172": (HISTORICAL_PREFIX_HASH, HISTORICAL_POSTGRES_IMAGE),
}


def validate_check_source(source_sha, prefix_hash, postgres_image):
    """Registration binds the actual candidate, not an alias to the historical oracle."""
    if not isinstance(source_sha, str) or REGISTERED_CHECK_SOURCES.get(source_sha) != (prefix_hash, postgres_image):
        raise ValueError("Unregistered source or PostgreSQL binding")


def historical_check_forms():
    """The twelve source predicates; registration is not permission to accept drift."""
    specs = (
        ("connconf_assignment", "connconf_assignment_scope_type_check", "scope_type", 32,
         "global region site_group site location rack tenant cluster role device_type platform device vm connection_target"),
        ("connections_proxy_assignment", "connections_proxy_assignment_scope_type_check", "scope_type", 32,
         "global region site_group site location rack tenant cluster role device_type platform device vm"),
        ("custom_field", "custom_field_filter_logic_valid", "filter_logic", 10, "disabled loose exact"),
        ("custom_field", "custom_field_type_valid", "field_type", 20,
         "text longtext integer decimal boolean date datetime url json select multiselect object multiobject"),
        ("custom_field", "custom_field_ui_editable_valid", "ui_editable", 10, "yes no hidden"),
        ("custom_field", "custom_field_ui_visible_valid", "ui_visible", 10, "always if-set hidden"),
        ("mapping_configurations", "mapping_configurations_conflict_mode_check", "conflict_mode", 30,
         "netbox_wins piping_wins last_write_wins field_level"),
        ("mapping_configurations", "mapping_configurations_direction_check", "direction", 30, "pull push bidirectional"),
        ("mapping_configurations", "mapping_configurations_status_check", "status", 30, "draft active retired"),
        ("secrets_variable", "chk_secrets_variable_kind", "kind", 32, "field named"),
        ("wizard_progress", "wizard_progress_status_check", "status", 20, "active completed dismissed"),
    )
    forms = []
    for table, name, column, width, values in specs:
        literals = [f"'{value}'::character varying" for value in values.split()]
        element = f"CHECK ((({column})::text = ANY (ARRAY[" + ", ".join(
            f"({literal})::text" for literal in literals) + "])))"
        array = f"CHECK ((({column})::text = ANY ((ARRAY[" + ", ".join(literals) + "])::text[])))"
        canonical, historical = (array, element) if table == "secrets_variable" else (element, array)
        # PostgreSQL 18 rewrites an explicit varchar[] -> text[] cast while
        # parsing DDL. Its native IN compiler still produces that typed array
        # node. Require the resulting deparser text to match the registered
        # form exactly, rather than rewriting or interpreting the node tree.
        native_array = f"CHECK ({column} IN (" + ", ".join(
            f"'{value}'" for value in values.split()) + "))"
        canonical_sql, historical_sql = (native_array, element) if table == "secrets_variable" else (element, native_array)
        forms.append(CheckForm(f"public.{table}.{name}",
                               ((column, f"character varying({width})"),), canonical, historical,
                               canonical_sql, historical_sql, tuple(values.split())))
    atoms = ("(content_size_bytes >= 1)", "(content_size_bytes <= 2097152)",
             "((content_text IS NULL) OR (octet_length(content_text) <= 2097152))",
             "((content_json IS NULL) OR (octet_length((content_json)::text) <= 2097152))")
    flat = "CHECK ((" + " AND ".join(atoms) + "))"
    nested = "CHECK (((" + " AND ".join(atoms[:2]) + ") AND " + " AND ".join(atoms[2:]) + "))"
    forms.append(CheckForm("public.playbook_artifact.chk_playbook_artifact_size",
                           (("content_size_bytes", "bigint"), ("content_text", "text"), ("content_json", "jsonb")),
                           flat, nested, flat, nested, atoms=atoms))
    return {form.name: form for form in forms}


def _identifier(value):
    if not isinstance(value, str) or not value or not value.isascii() or not all(
        c.isalnum() or c == "_" for c in value
    ) or value[0].isdigit():
        raise ValueError("Unsafe native proof identifier")
    return '"' + value + '"'


_NATIVE_PROOF_ISSUER = object()


@dataclass(frozen=True)
class NativeCheckProof:
    name: str
    canonical: str
    historical: str
    binding_hash: str
    planned_expression: str
    source_sha: str
    prefix_hash: str
    postgres_image: str
    canonical_ast_hash: str
    historical_ast_hash: str
    historical_compiler_definition: str
    association_truth_rows: int = 0
    capture_hash: str = ""
    _issuer: object = field(default=None, init=False, repr=False, compare=False)

    def receipt(self):
        """Diagnostic JSON is not an importable authorization receipt."""
        return {key: value for key, value in self.__dict__.items() if not key.startswith("_")}


def _associated_source_ast(references, form):
    """Compose only the registered first-two-atoms AND association.

    Every inserted node is emitted by PostgreSQL for a source-owned expression.
    Exact string equality checks the entire resulting typed tree. No node is
    parsed, no arbitrary tree is normalized, and no field other than the existing
    source-offset projection is ignored.
    """
    native = {r["name"].rsplit(".", 1)[1]: r for r in references}
    atoms = [native[f"proof_atom_{index}"]["ast"] for index in range(4)]
    canonical = native["canonical"]["ast"]
    pair = native["proof_pair"]["ast"]
    consecutive = " ".join(atoms[:2])
    if (not form.atoms or canonical.count(" ".join(atoms)) != 1
            or pair.count(consecutive) != 1 or canonical.count(consecutive) != 1):
        raise ValueError("Native source atom nodes are not the registered AND structure")
    return canonical.replace(consecutive, pair, 1)


def prove_native_check(sql, form, installed_binding, *, source_sha, postgres_image,
                       prefix_sha384, original_conkey=None, original_dependencies=None,
                       original_identity=None):
    """Compile exact registered forms on the target server and bind its real AST.

    ``sql`` executes one native statement and returns its scalar text. The caller
    owns a disposable transaction/session and rollback. It must supply bindings
    read with CHECK_BINDINGS_SQL on that same server, not restored deparser text.
    The planner is the equivalence oracle; sample evaluations are not the proof.
    """
    prefix_hash = definition_hash(prefix_sha384)
    validate_check_source(source_sha, prefix_hash, postgres_image)
    if form != historical_check_forms().get(form.name):
        raise ValueError("Unregistered source or PostgreSQL binding")
    if sql("SELECT current_setting('server_version')") != "18.3":
        raise ValueError("Historical proof requires PostgreSQL 18.3")
    binding = installed_binding
    if (not isinstance(binding, dict)
            or set(binding) != {"name", "definition", "ast", "validated", "enforced", "no_inherit", "columns", "dependencies"}
            or binding["name"] != form.name
            or binding["definition"] not in (form.canonical, form.historical)
            or binding["validated"] is not True or binding["enforced"] is not True
            or binding["no_inherit"] is not False):
        raise ValueError("Installed CHECK is not the registered enforced, validated form")
    columns = binding["columns"]
    if (not isinstance(columns, list) or len(columns) != len(form.columns)
            or any(not isinstance(c, dict) or set(c) != {"name", "position", "type", "collation", "deterministic"}
                   or not isinstance(c["name"], str) or not isinstance(c["type"], str) for c in columns)
            or {c["name"]: c["type"] for c in columns} != dict(form.columns)
            or any(set(c) != {"name", "position", "type", "collation", "deterministic"}
                   or type(c["position"]) is not int or not 1 <= c["position"] <= 1600
                   or c["collation"] not in (None, "pg_catalog.default")
                   or (c["collation"] is not None and c["deterministic"] is not True)
                   for c in columns)
            or len({c["position"] for c in columns}) != len(columns)):
        raise ValueError("CHECK column type, position or collation is unsupported")
    slots = {c["position"]: c for c in columns}
    declarations = []
    for position in range(1, max(slots) + 1):
        column = slots.get(position)
        declarations.append(f'{_identifier(column["name"])} {column["type"]}' if column else
                            f'"_proof_gap_{position}" integer')
    sql("SET search_path = pg_catalog, pg_temp")
    atom_constraints = ""
    if form.atoms:
        atom_constraints = "".join(f", CONSTRAINT proof_atom_{index} CHECK ({atom})"
                                   for index, atom in enumerate(form.atoms))
        atom_constraints += ", CONSTRAINT proof_pair CHECK (" + " AND ".join(form.atoms[:2]) + ")"
    sql("CREATE TEMP TABLE pwa_check_proof (" + ",".join(declarations)
        + ", CONSTRAINT canonical " + form.canonical_sql
        + ", CONSTRAINT historical " + form.historical_sql + atom_constraints + ")")
    try:
        references = json.loads(sql(CHECK_BINDINGS_SQL % "'pg_temp.pwa_check_proof'"))
        canonical_reference = next(r for r in references if r["name"].endswith(".canonical"))
        historical_reference = next(r for r in references if r["name"].endswith(".historical"))
        association_rows = 0
        if form.atoms:
            historical_reference = dict(historical_reference)
            historical_reference["ast"] = _associated_source_ast(references, form)
            historical_reference["definition"] = form.historical
            # Boolean atoms each have exactly three possible SQL values. This
            # exhausts the domain; it is not sampling the unbounded column data.
            truth = json.loads(sql("WITH truth(v) AS (VALUES(true),(false),(NULL::boolean)) "
                                   "SELECT jsonb_build_array(count(*),bool_and("
                                   "((a.v AND b.v) AND c.v AND d.v) IS NOT DISTINCT FROM "
                                   "(a.v AND b.v AND c.v AND d.v))) FROM truth a "
                                   "CROSS JOIN truth b CROSS JOIN truth c CROSS JOIN truth d"))
            if truth != [81, True]:
                raise ValueError("Native SQL three-valued AND association failed")
            association_rows = 81
        reference = canonical_reference if binding["definition"] == form.canonical else historical_reference
        if reference["definition"] != binding["definition"] or any(binding[key] != reference[key] for key in (
            "ast", "validated", "enforced", "no_inherit", "columns", "dependencies"
        )):
            raise ValueError("Installed typed AST or dependencies differ from the native compiler")
        if original_conkey is not None:
            native_key = json.loads(sql("SELECT to_jsonb(conkey) FROM pg_constraint "
                                        "WHERE conrelid='pg_temp.pwa_check_proof'::regclass AND conname='canonical'"))
            if original_conkey != native_key or any(type(position) is not int for position in original_conkey):
                raise ValueError("Original CHECK column key differs from native source binding")
        if original_dependencies is not None:
            native_dependencies = json.loads(sql("SELECT jsonb_agg(jsonb_build_object("
                "'classid',d.classid::text,'refclassid',d.refclassid::text,'deptype',d.deptype,"
                "'objsubid',d.objsubid,'refobjsubid',d.refobjsubid)) FROM pg_depend d "
                "JOIN pg_constraint k ON d.classid='pg_constraint'::regclass AND d.objid=k.oid "
                "WHERE k.conrelid='pg_temp.pwa_check_proof'::regclass AND k.conname='canonical'"))
            expected_dependencies = []
            columns = {column["position"]: column["name"] for column in binding["columns"]}
            relation = form.name.rsplit(".", 1)[0]
            # pg_describe_object renders public names according to the native
            # capture search_path. Resolve the source-owned column by name on
            # the immutable reference, not by the target's physical attnum.
            sql("SET search_path=pg_catalog,public")
            descriptions = {}
            for position, name in columns.items():
                descriptions[position] = sql("SELECT pg_describe_object('pg_class'::regclass,attrelid,attnum) "
                    "FROM pg_attribute WHERE attrelid=" + _sql_literal(relation)
                    + "::regclass AND attname=" + _sql_literal(name) + " AND NOT attisdropped")
            sql("SET search_path=pg_catalog,pg_temp")
            for dependency in native_dependencies:
                expected_dependencies.append(dict(dependency, name=form.name,
                    objid=original_identity["constraint_oid"], refobjid=original_identity["relation_oid"],
                    description=descriptions[dependency["refobjsubid"]]))
            if not _same_native_inventory(original_dependencies, expected_dependencies):
                raise ValueError("Original CHECK native dependency identity differs from the compiler")
        outputs = []
        for definition in (form.canonical_sql, form.historical_sql):
            plan = json.loads(sql("EXPLAIN (VERBOSE, FORMAT JSON, COSTS OFF) SELECT "
                                  + definition[len("CHECK "):]
                                  + " FROM pg_temp.pwa_check_proof"))
            outputs.append(plan[0]["Plan"]["Output"])
        if outputs[0] != outputs[1] or len(outputs[0]) != 1:
            raise ValueError("Native planner did not prove the registered representation pair")
        proof = NativeCheckProof(form.name, form.canonical, form.historical,
                                definition_hash(binding), outputs[0][0], source_sha,
                                prefix_hash, postgres_image,
                                definition_hash(canonical_reference["ast"]),
                                definition_hash(historical_reference["ast"]),
                                historical_reference["definition"], association_rows)
        object.__setattr__(proof, "_issuer", _NATIVE_PROOF_ISSUER)
        return proof
    finally:
        sql("DROP TABLE pg_temp.pwa_check_proof")


HISTORICAL_BUILTINS_SQL = """
SELECT jsonb_build_object(
 'objects',jsonb_build_object(
  'types',(SELECT jsonb_agg(jsonb_build_object('oid',t.oid::text,'name',t.typname,
      'namespace',n.nspname,'kind',t.typtype,'category',t.typcategory,'collation_oid',t.typcollation::text))
    FROM pg_type t JOIN pg_namespace n ON n.oid=t.typnamespace
    WHERE t.oid IN ('varchar'::regtype,'text'::regtype,'int4'::regtype,'text[]'::regtype,'int8'::regtype,'jsonb'::regtype)),
  'functions',(SELECT jsonb_agg(jsonb_build_object('oid',p.oid::text,'namespace',n.nspname,
      'identity',p.oid::regprocedure::text,'strict',p.proisstrict,'volatility',p.provolatile,
      'return_type',p.prorettype::regtype::text))
    FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace
    WHERE p.oid IN ('texteq(text,text)'::regprocedure,'int4le(integer,integer)'::regprocedure,
      'int84le(bigint,integer)'::regprocedure,'int84ge(bigint,integer)'::regprocedure,'octet_length(text)'::regprocedure)),
  'operators',(SELECT jsonb_agg(jsonb_build_object('oid',o.oid::text,'namespace',n.nspname,'name',o.oprname,
      'function',o.oprcode::regprocedure::text,'left_type',o.oprleft::regtype::text,
      'right_type',o.oprright::regtype::text,'result_type',o.oprresult::regtype::text))
    FROM pg_operator o JOIN pg_namespace n ON n.oid=o.oprnamespace
    WHERE o.oprcode IN ('texteq(text,text)'::regprocedure,'int4le(integer,integer)'::regprocedure,
      'int84le(bigint,integer)'::regprocedure,'int84ge(bigint,integer)'::regprocedure)),
  'collations',(SELECT jsonb_agg(jsonb_build_object('oid',c.oid::text,'namespace',n.nspname,
      'name',c.collname,'deterministic',c.collisdeterministic))
    FROM pg_collation c JOIN pg_namespace n ON n.oid=c.collnamespace
    WHERE n.nspname='pg_catalog' AND c.collname='default')),
 'casts',(SELECT jsonb_agg(jsonb_build_object('oid',c.oid::text,'source',c.castsource::regtype::text,
      'target',c.casttarget::regtype::text,'method',c.castmethod,'context',c.castcontext,
      'function',CASE WHEN c.castfunc=0 THEN NULL ELSE c.castfunc::regprocedure::text END))
    FROM pg_cast c WHERE (c.castsource,c.casttarget) IN (
      ('int8'::regtype,'int4'::regtype),('int4'::regtype,'int8'::regtype),
      ('text'::regtype,'varchar'::regtype),('varchar'::regtype,'text'::regtype),
      ('varchar'::regtype,'varchar'::regtype),('jsonb'::regtype,'int4'::regtype),('jsonb'::regtype,'int8'::regtype)))
);
"""


def _sql_literal(value):
    if not isinstance(value, str) or "\0" in value:
        raise ValueError("Invalid native proof text")
    return "'" + value.replace("'", "''") + "'"


def _same_native_inventory(left, right):
    return (isinstance(left, list) and isinstance(right, list)
            and sorted(definition_hash(item) for item in left)
            == sorted(definition_hash(item) for item in right))


def prove_original_native_checks(sql, capture_raw, *, trusted_capture_sha256,
                                source_sha, postgres_image, prefix_sha384):
    """Recompute all twelve proofs from an externally authenticated capture.

    The trusted digest must come from the source authority, not from the client
    or from observing the same bytes. The caller authenticates capture provenance,
    binds target/root/project/helper/backend/ledger and freshness, and owns the
    disposable PG session. No client proof JSON is accepted as authorization.
    """
    validate_check_source(source_sha, definition_hash(prefix_sha384), postgres_image)
    if (not isinstance(capture_raw, bytes) or len(capture_raw) > 262144
            or not isinstance(trusted_capture_sha256, str)
            or hashlib.sha256(capture_raw).hexdigest() != trusted_capture_sha256):
        raise ValueError("Original CHECK capture does not match its source-trusted digest")
    capture = json.loads(capture_raw)
    if (not isinstance(capture, dict) or capture.get("postgres_version") != "18.3"
            or capture.get("images", {}).get("postgres") != postgres_image
            or capture.get("ledger") != {"count": 22, "max": 22, "successful": 22}):
        raise ValueError("Original CHECK capture has an unsupported PG or ledger binding")
    sql("SET search_path=pg_catalog,pg_temp")
    native = json.loads(sql(HISTORICAL_BUILTINS_SQL))
    if (not isinstance(capture.get("objects"), dict)
            or set(capture["objects"]) != set(native["objects"])
            or any(not _same_native_inventory(capture["objects"][kind], rows)
                   for kind, rows in native["objects"].items())
            or not _same_native_inventory(capture.get("casts"), native["casts"])):
        raise ValueError("Original builtin operator, function, type, cast or collation binding changed")
    forms = historical_check_forms()
    originals, bindings = {}, {}
    for field_name, destination in (("original", originals), ("bindings", bindings)):
        entries = capture.get(field_name)
        if not isinstance(entries, list) or len(entries) != len(forms):
            raise ValueError("Original CHECK capture must contain exactly twelve bindings")
        for entry in entries:
            if (not isinstance(entry, dict) or entry.get("name") not in forms
                    or entry["name"] in destination):
                raise ValueError("Original CHECK identities must be registered and unique")
            destination[entry["name"]] = entry
    dependencies = capture.get("dependencies")
    if not isinstance(dependencies, list):
        raise ValueError("Missing original native dependency inventory")
    proofs = []
    for name, form in forms.items():
        original, binding = originals[name], bindings[name]
        if (set(original) != {"name", "conkey", "enforced", "validated", "relation_oid", "namespace_oid", "constraint_oid", "original_conbin"}
                or not isinstance(original["conkey"], list) or not original["conkey"]
                or any(type(position) is not int or not 1 <= position <= 1600 for position in original["conkey"])
                or len(set(original["conkey"])) != len(original["conkey"])
                or any(not isinstance(original[key], str) or not original[key].isascii()
                       or not original[key].isdigit() or int(original[key]) <= 0
                       for key in ("relation_oid", "namespace_oid", "constraint_oid"))
                or original.get("enforced") is not True or original.get("validated") is not True
                or not isinstance(original.get("original_conbin"), str)
                or len(original["original_conbin"]) > 32768):
            raise ValueError("Original CHECK is not an enforced, validated native node tree")
        normalized = sql("SELECT regexp_replace(" + _sql_literal(original["original_conbin"])
                         + r", ':(location|list_start|list_end) -?[0-9]+', ':\1 -1', 'g')")
        if normalized != binding.get("ast"):
            raise ValueError("Original stored tree does not match its native source-offset projection")
        own_dependencies = [row for row in dependencies if isinstance(row, dict) and row.get("name") == name]
        if not own_dependencies:
            raise ValueError("Original CHECK native relation dependencies are missing")
        for dependency in own_dependencies:
            if (set(dependency) != {"name", "objid", "classid", "deptype", "objsubid", "refobjid", "refclassid", "description", "refobjsubid"}
                    or dependency["objid"] != original.get("constraint_oid")
                    or dependency["refobjid"] != original.get("relation_oid")
                    or type(dependency["refobjsubid"]) is not int):
                raise ValueError("Original CHECK native dependency identity is unsupported")
        proof = prove_native_check(sql, form, binding, source_sha=source_sha,
                                   postgres_image=postgres_image, prefix_sha384=prefix_sha384,
                                   original_conkey=original.get("conkey"),
                                   original_dependencies=own_dependencies, original_identity=original)
        object.__setattr__(proof, "capture_hash", trusted_capture_sha256)
        proofs.append(proof)
    if any(not isinstance(row, dict) or row.get("name") not in forms for row in dependencies):
        raise ValueError("Unlisted original CHECK dependencies")
    return tuple(proofs)


def canonical_readiness(canonical, installed, *, trusted_inventory=(),
                        position_pairs=None, check_proofs=(), installed_check_bindings=(),
                        source_sha=None, prefix_sha384=None, postgres_image=None):
    """Exact canonical fields except explicitly bound positions and native proofs.

    Position pairs are a source-trusted, target-bound mapping of column name to
    (canonical attnum, installed attnum), not a global layout ignore. This does
    not seal or authorize the target; exact_target_preservation does that.
    """
    expected, actual = _catalog_index(canonical), _catalog_index(installed)
    preservation_extensions(canonical, installed, trusted_inventory=trusted_inventory)
    positions = dict(position_pairs or {})
    if any(not isinstance(name, str) or not isinstance(pair, (list, tuple)) or len(pair) != 2
           or any(type(position) is not int or not 1 <= position <= 1600 for position in pair)
           for name, pair in positions.items()):
        raise ValueError("Invalid target-bound column positions")
    proofs = {}
    if check_proofs:
        validate_check_source(source_sha, definition_hash(prefix_sha384), postgres_image)
    for proof in check_proofs:
        if (not isinstance(proof, NativeCheckProof) or proof.name in proofs
                or proof._issuer is not _NATIVE_PROOF_ISSUER
                or proof.source_sha != source_sha
                or proof.prefix_hash != definition_hash(prefix_sha384)
                or proof.postgres_image != postgres_image):
            raise ValueError("Invalid or duplicate native CHECK proof")
        proofs[proof.name] = proof
    bindings = {}
    for binding in installed_check_bindings:
        if not isinstance(binding, dict) or not isinstance(binding.get("name"), str) or binding["name"] in bindings:
            raise ValueError("Invalid or duplicate installed CHECK binding")
        bindings[binding["name"]] = binding
    used_positions, used_proofs = set(), set()
    for key, definition in expected.items():
        if key not in actual:
            raise ValueError("Canonical catalog object is missing")
        observed = actual[key]
        if key[0] == "column" and key[1] in positions:
            if (not isinstance(definition, dict) or not isinstance(observed, dict)
                    or tuple(positions[key[1]]) != (definition.get("position"), observed.get("position"))):
                raise ValueError("Column position approval does not bind this target")
            used_positions.add(key[1])
            definition, observed = dict(definition), dict(observed)
            definition.pop("position")
            observed.pop("position")
        if definition == observed:
            continue
        proof = proofs.get(key[1]) if key[0] == "constraint" else None
        binding = bindings.get(key[1])
        if (proof is None or binding is None
                or (definition, observed) != (proof.canonical, proof.historical)
                or binding.get("definition") != observed
                or definition_hash(binding) != proof.binding_hash):
            raise ValueError("Canonical catalog definition changed without a native proof")
        used_proofs.add(proof.name)
    if used_positions != set(positions) or used_proofs != set(proofs):
        raise ValueError("Unused position approval or CHECK proof")
