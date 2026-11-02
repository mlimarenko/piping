#!/usr/bin/env python3
"""Offline, core-only NetBox snapshot producer (NetBox 4.7.1)."""

import argparse
import hashlib
import importlib.abc
import json
import os
import re
import sys
import uuid
from datetime import datetime, timezone
from pathlib import Path


CORE_APPS = [
    "django.contrib.auth", "django.contrib.contenttypes", "django.contrib.sessions",
    "django.contrib.messages", "django.contrib.staticfiles", "django.contrib.humanize",
    "django.contrib.postgres", "django.forms", "corsheaders", "django_filters",
    "django_htmx", "django_tables2", "django_prometheus", "strawberry_django", "mptt",
    "rest_framework", "social_django", "sorl.thumbnail", "taggit", "timezone_field",
    "core", "account", "circuits", "dcim", "ipam", "extras", "tenancy", "users",
    "utilities", "virtualization", "vpn", "wireless", "django_rq", "drf_spectacular",
    "drf_spectacular_sidecar",
]

# The shipped contract is a security policy, not an operator-provided allowlist.
CONTRACT_SHA256 = "c986ea985b7b7c70833da311a6659e8d9109c60e67ad8a8ea724e8a41e3a4443"


class SafeArgumentParser(argparse.ArgumentParser):
    def error(self, message):
        # argparse's message can contain an unrecognized secret-bearing argument.
        self.exit(2, "invalid command-line arguments\n")


class CoreImportGuard(importlib.abc.MetaPathFinder):
    """Refuse source settings and script serializers before their code executes."""

    def find_spec(self, fullname, path=None, target=None):
        forbidden = fullname in {"netbox.settings", "netbox.configuration", "configuration"}
        forbidden |= fullname.startswith("extras.api.serializers") and "script" in fullname
        if forbidden:
            raise ImportError("source configuration and script serializers are not permitted")
        return None


def bootstrap(database):
    if os.environ.get("DJANGO_SETTINGS_MODULE") or "netbox.settings" in sys.modules:
        raise ValueError("external Django settings are not permitted")
    # The official image stores its application modules at this fixed location.
    sys.path.insert(0, "/opt/netbox/netbox")
    sys.meta_path.insert(0, CoreImportGuard())
    from django.conf import settings

    if settings.configured:
        raise ValueError("Django settings already configured")
    settings.configure(
        INSTALLED_APPS=CORE_APPS, DATABASES={"default": database},
        SECRET_KEY="offline-export-not-used-for-authentication", USE_TZ=True,
        TIME_ZONE="UTC", AUTH_USER_MODEL="users.User", DEFAULT_AUTO_FIELD="django.db.models.AutoField",
        PLUGINS=[], PLUGINS_CONFIG={}, BASE_PATH="", STATIC_URL="/static/",
        REDIS={"tasks": {}, "caching": {}}, RQ_QUEUES={}, FIELD_CHOICES={},
        WEBHOOK_DEFAULT_TIMEOUT=60, SEARCH_BACKEND="netbox.search.backends.CachedValueSearchBackend",
    )
    import django
    django.setup()
    if "netbox.settings" in sys.modules or "netbox.configuration" in sys.modules:
        raise ValueError("source configuration was imported")


def census():
    """Return the model and relation shape without reading database values."""
    from django.apps import apps
    from utilities.release import load_release_data

    version = load_release_data().version
    if version != "4.7.1":
        raise ValueError("unsupported NetBox version")
    models = []
    for model in sorted(apps.get_models(include_auto_created=True), key=lambda m: m._meta.label_lower):
        fields = []
        for field in sorted(model._meta.get_fields(include_hidden=True), key=lambda f: f.name):
            target = getattr(field, "related_model", None)
            fields.append({
                "name": field.name,
                "type": field.__class__.__name__,
                "concrete": field.concrete,
                "many_to_many": field.many_to_many,
                "many_to_one": field.many_to_one,
                "one_to_many": field.one_to_many,
                "one_to_one": field.one_to_one,
                "target": target._meta.label_lower if target is not None else None,
            })
        models.append({"model": model._meta.label_lower, "table": model._meta.db_table,
                       "auto_created": bool(model._meta.auto_created), "fields": fields})
    return {"netbox_version": version, "models": models}


def main():
    parser = SafeArgumentParser()
    parser.add_argument("--offline-census", action="store_true")
    parser.add_argument("--source-provider-id")
    parser.add_argument("--exporter-source-sha")
    parser.add_argument("--contract", type=Path, default=Path(__file__).parents[1] / "contracts/netbox-4.7.1-source.json")
    args = parser.parse_args()
    if args.offline_census:
        bootstrap({"ENGINE": "django.db.backends.postgresql", "NAME": "offline"})
        print(json.dumps(census(), sort_keys=True, separators=(",", ":")))
        return
    if not args.source_provider_id or not args.exporter_source_sha:
        parser.error("export requires source provider UUID and exporter source SHA")
    provider = uuid.UUID(args.source_provider_id)
    if provider.int == 0 or not re.fullmatch(r"[0-9a-f]{40}", args.exporter_source_sha):
        parser.error("invalid provider UUID or exporter source SHA")
    required = ("NETBOX_DB_NAME", "NETBOX_DB_USER", "NETBOX_DB_PASSWORD", "NETBOX_DB_HOST", "NETBOX_DB_PORT")
    if any(not os.environ.get(name) for name in required):
        parser.error("explicit NETBOX_DB_NAME, NETBOX_DB_USER, NETBOX_DB_PASSWORD, NETBOX_DB_HOST and NETBOX_DB_PORT are required")
    if not os.environ["NETBOX_DB_PORT"].isdigit() or not 1 <= int(os.environ["NETBOX_DB_PORT"]) <= 65535:
        parser.error("invalid database port")
    with args.contract.open("rb") as source:
        raw_contract = source.read(4_000_001)
    if len(raw_contract) > 4_000_000:
        parser.error("contract exceeds size limit")
    contract = load_contract(raw_contract)
    if contract.get("netbox_version") != "4.7.1" or contract.get("format_version") != 1:
        parser.error("unsupported contract")
    database = {"ENGINE": "django.db.backends.postgresql", "NAME": os.environ["NETBOX_DB_NAME"],
                "USER": os.environ["NETBOX_DB_USER"], "PASSWORD": os.environ["NETBOX_DB_PASSWORD"],
                "HOST": os.environ["NETBOX_DB_HOST"], "PORT": os.environ["NETBOX_DB_PORT"],
                "CONN_MAX_AGE": 0}
    bootstrap(database)
    transport = export_snapshot(contract, provider, args.exporter_source_sha, hashlib.sha256(raw_contract).hexdigest())
    from django.core.serializers.json import DjangoJSONEncoder
    payload = json.dumps(transport, cls=DjangoJSONEncoder, separators=(",", ":"), ensure_ascii=False).encode()
    if len(payload) > 256_000_000:
        raise ValueError("snapshot exceeds size limit")
    sys.stdout.buffer.write(payload + b"\n")


def load_contract(raw_contract):
    if hashlib.sha256(raw_contract).hexdigest() != CONTRACT_SHA256:
        raise ValueError("contract digest mismatch")
    return json.loads(raw_contract)


def validate_census(schema, observed):
    expected = [{k: m[k] for k in ("model", "table", "auto_created", "fields")} for m in schema["models"]]
    for model in expected:
        model["fields"] = [{k: v for k, v in field.items() if k not in ("disposition", "reason", "output_name", "value_shape")} for field in model["fields"]]
    if observed != {"netbox_version": "4.7.1", "models": expected}:
        raise ValueError("NetBox model census differs from pinned contract")
    return len(expected)


def custom_field_plan(definitions, policy):
    """Choose typed keys using definitions, before any custom values are queried."""
    result = {}
    excluded = {"credential_name": set(), "untyped": set()}
    denied = re.compile(policy["denied_name_regex"], re.IGNORECASE)
    for definition in definitions:
        name, kind = definition["name"], definition["type"]
        if not re.fullmatch(policy["valid_name_regex"], name) or "__" in name:
            raise ValueError("invalid custom field name")
        if denied.search(name):
            excluded["credential_name"].add(name)
            continue
        if kind in policy["excluded_types"]:
            excluded["untyped"].add(name)
            continue
        if kind not in policy["types"]:
            raise ValueError("unknown custom field type")
        if definition["status"] != "active":
            raise ValueError("custom field data transition in progress")
        label = definition["object_types__app_label"]
        model = definition["object_types__model"]
        if label is not None and model is not None:
            result.setdefault(f"{label}.{model}", {})[name] = kind
    return result, {key: len(names) for key, names in excluded.items()}


def custom_field_value(value, kind, policy):
    if value is None:
        return None
    shape = policy["types"][kind]
    scalar = lambda v: isinstance(v, str)
    integer = lambda v: type(v) is int and v >= 0
    valid = (
        (shape == "string" and scalar(value))
        or (shape == "integer" and type(value) is int)
        or (shape == "number" and type(value) in (int, float))
        or (shape == "boolean" and type(value) is bool)
        or (shape == "object_id" and integer(value))
        or (shape == "string_list" and isinstance(value, list) and all(scalar(v) for v in value))
        or (shape == "object_id_list" and isinstance(value, list) and all(integer(v) for v in value))
    )
    if not valid:
        raise ValueError("custom field value does not match its declared type")
    return value


def custom_field_default(value, kind, policy):
    value = custom_field_value(value, kind, policy)
    if kind in ("object", "multiobject") and value is not None and value != []:
        raise ValueError("populated object custom field defaults are unsupported")
    return value


def approved_definition_names(plan):
    return sorted({name for names in plan.values() for name in names})


def output_field(spec, value):
    if spec.get("value_shape") == "cable_termination_positions":
        if value is not None and (not isinstance(value, list) or any(
                type(position) is not int or not 1 <= position <= 1024 for position in value)):
            raise ValueError("invalid cable termination positions")
    if spec.get("value_shape") == "service_port_mappings":
        if not isinstance(value, list) or not value:
            raise ValueError("invalid service port mappings")
        seen = set()
        for item in value:
            if not isinstance(item, str):
                raise ValueError("invalid service port mapping")
            match = re.fullmatch(r"(tcp|udp|sctp)/\s*([0-9]+)\s*", item, re.IGNORECASE | re.ASCII)
            if not match or not 1 <= int(match.group(2)) <= 65535:
                raise ValueError("invalid service port mapping")
            key = (match.group(1).lower(), int(match.group(2)))
            if key in seen:
                raise ValueError("duplicate service port mapping")
            seen.add(key)
    return spec.get("output_name", spec["name"]), value


def validate_export_array(model, field, spec):
    if spec.get("value_shape") != "cable_termination_positions":
        return
    if (model._meta.label_lower != "dcim.cabletermination" or field.name != "positions"
            or field.__class__.__name__ != "ArrayField"
            or field.base_field.__class__.__name__ != "PositiveSmallIntegerField"):
        raise ValueError("unsupported exported array field")


def validate_generic_pairs(row, model_spec, exported_types):
    for pair in model_spec.get("generic_pairs", []):
        name, type_name = pair["field"], pair["type_field"]
        if name not in row or type_name not in row:
            raise ValueError("generic reference evidence is missing")
        identity, token = row[name], row[type_name]
        if identity is None and token is None:
            continue
        if type(identity) is not int or identity <= 0 or not isinstance(token, str):
            raise ValueError("generic reference evidence is incomplete")
        if token not in exported_types:
            raise ValueError("generic reference type is unsupported")


def validate_generic_contract(schema, apps):
    for spec in schema["models"]:
        if spec["disposition"] != "export":
            continue
        fields = {field["name"]: field for field in spec["fields"]}
        generic_names = {name for name, field in fields.items() if field["type"] == "GenericForeignKey"}
        pairs = spec.get("generic_pairs", [])
        if generic_names != {pair["field"] for pair in pairs} or len(generic_names) != len(pairs):
            raise ValueError("generic reference contract is incomplete")
        model = apps.get_model(spec["model"])
        for pair in pairs:
            generic = model._meta.get_field(pair["field"])
            identifier = fields[generic.fk_field]
            type_field = fields[generic.ct_field]
            if (generic.ct_field != pair["type_field"] or identifier.get("output_name") != pair["field"]
                    or identifier["disposition"] != "export" or type_field["disposition"] != "export"):
                raise ValueError("generic reference contract differs from Django metadata")


def plugin_database_evidence(connection, schema, content_types):
    """Census visible DB tables without claiming knowledge of source runtime plugins."""
    known = {model["table"] for model in schema["models"]} | {"django_migrations"}
    known_models = {model["model"] for model in schema["models"]}
    with connection.cursor() as cursor:
        cursor.execute("SELECT n.nspname, c.relname, c.relrowsecurity, "
                       "n.nspname = ANY(current_schemas(false)) AS in_search_path FROM pg_catalog.pg_class c "
                       "JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace "
                       "WHERE c.relkind IN ('r','p') AND left(n.nspname, 3) <> 'pg_' "
                       "AND n.nspname <> 'information_schema' "
                       "ORDER BY in_search_path DESC, array_position(current_schemas(false), n.nspname), n.nspname, c.relname")
        tables = cursor.fetchall()
        if len(tables) > 4096:
            raise ValueError("database table census limit exceeded")
        seen = set()
        core_tables = []
        extras = []
        for namespace, table, row_security, in_search_path in tables:
            if not re.fullmatch(r"[a-z_][a-z0-9_]*", namespace) or not re.fullmatch(r"[a-z_][a-z0-9_]*", table):
                raise ValueError("unsupported database identifier")
            if row_security:
                raise ValueError("row security prevents complete database census")
            if in_search_path and table in known and table not in seen:
                seen.add(table)
                core_tables.append({"schema": namespace, "table": table, "disposition": "core"})
                continue
            cursor.execute(f'SELECT count(*) FROM "{namespace}"."{table}"')
            count = cursor.fetchone()[0]
            extras.append({"schema": namespace, "table": table, "row_count": count,
                           "disposition": "excluded_empty" if count == 0 else "refused_populated"})
            if count:
                raise ValueError("unrecognized populated database table")
    types = [{"content_type": name, "disposition": "core" if name in known_models else "unmapped"}
             for name in sorted(set(content_types.values()))]
    return {"scope": "database tables and content types", "core_tables_seen": len(seen),
            "core_tables": core_tables, "extra_tables": extras, "content_types": types,
            "runtime_configuration_inspected": False, "execution_observed": False}


def relation_values(field, model, row_id, content_types):
    through = field.remote_field.through
    if field.__class__.__name__ == "NetBoxTaggableManagerField":
        source_type = next((pk for pk, name in content_types.items() if name == model._meta.label_lower), None)
        if source_type is None:
            raise ValueError("missing model content type")
        query = through._base_manager.filter(content_type_id=source_type, object_id=row_id).values_list("tag_id", flat=True)
    else:
        source = through._meta.get_field(field.m2m_field_name()).attname
        target = through._meta.get_field(field.m2m_reverse_field_name()).attname
        query = through._base_manager.filter(**{source: row_id}).values_list(target, flat=True)
    ids = list(query.order_by()[:1_000_001])
    if len(ids) > 1_000_000:
        raise ValueError("relation limit exceeded")
    if field.related_model._meta.label_lower == "contenttypes.contenttype":
        return content_type_references(ids, content_types)
    return sorted(ids)


def content_type_references(ids, content_types):
    if any(value not in content_types for value in ids):
        raise ValueError("unknown content type reference")
    return sorted(content_types[value] for value in ids)


def build_projection(model, concrete, selected_custom):
    from django.db.models import F, Func, JSONField, Value

    annotations = {f"_snapshot_cf_{i}": Func(F("custom_field_data"), Value(name),
                   function="jsonb_extract_path", output_field=JSONField())
                   for i, name in enumerate(selected_custom)}
    return model._base_manager.annotate(**annotations).values(
        *(f.attname for f in concrete), *annotations).order_by("pk")


def reconcile_port_template_mappings(resources, apps):
    from django.db.models import Count

    by_key = {resource["resource_key"]: resource["rows"] for resource in resources}
    mappings = by_key["dcim.port-template-mappings"]
    results = {}
    for side, label in (("front", "dcim.frontporttemplate"), ("rear", "dcim.rearporttemplate")):
        expected = {}
        for mapping in mappings:
            object_id = mapping[f"{side}_port"]
            if object_id is not None:
                expected[object_id] = expected.get(object_id, 0) + 1
        model = apps.get_model(label)
        observed = {row["pk"]: row["mapping_count"] for row in
                    model._base_manager.values("pk").annotate(mapping_count=Count("mappings"))
                    if row["mapping_count"]}
        if observed != expected:
            raise ValueError("port template reverse relation differs from mapping rows")
        results[side] = {"mapped_templates": len(expected), "mapping_count": sum(expected.values()), "equal": True}
    return {"owner": "dcim.port-template-mappings", "front": results["front"], "rear": results["rear"]}


def export_snapshot(contract, provider, source_sha, contract_sha):
    """Capture one transaction; project only fields explicitly approved in the contract."""
    from django.apps import apps
    from django.core.serializers.json import DjangoJSONEncoder
    from django.db import connection, transaction

    started = datetime.now(timezone.utc).isoformat()
    schema = contract["schema"]
    exported_types = {m["model"] for m in schema["models"] if m["disposition"] == "export"}
    with transaction.atomic():
        with connection.cursor() as cursor:
            cursor.execute("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            cursor.execute("SELECT pg_current_snapshot()::text")
            snapshot = cursor.fetchone()[0]
        model_count = validate_census(schema, census())
        validate_generic_contract(schema, apps)
        content_type_model = apps.get_model("contenttypes.contenttype")
        content_types = {pk: f"{app}.{model}" for pk, app, model in content_type_model._base_manager.values_list("pk", "app_label", "model")}
        plugin_evidence = plugin_database_evidence(connection, schema, content_types)
        custom_model = apps.get_model("extras.customfield")
        definitions = custom_model._base_manager.values("name", "type", "status", "object_types__app_label", "object_types__model")
        policy = schema["custom_field_policy"]
        custom_fields, custom_excluded = custom_field_plan(definitions, policy)
        allowed_definition_names = approved_definition_names(custom_fields)
        resources = []
        total = 0
        total_bytes = 0
        for spec in schema["models"]:
            if spec["disposition"] != "export":
                continue
            model = apps.get_model(spec["model"])
            approved = [f for f in spec["fields"] if f["disposition"] == "export"]
            fields = [model._meta.get_field(f["name"]) for f in approved]
            for field, field_spec in zip(fields, approved):
                validate_export_array(model, field, field_spec)
            concrete = [f for f in fields if f.concrete and not f.many_to_many]
            many = [f for f in fields if f.many_to_many and not f.auto_created]
            has_custom_fields = any(f["disposition"] == "selective_custom_fields" for f in spec["fields"])
            selected_custom = custom_fields.get(spec["model"], {}) if has_custom_fields else {}
            query = build_projection(model, concrete, selected_custom)
            if spec["model"] == "extras.customfield":
                query = query.filter(name__in=allowed_definition_names)
            rows = []
            for values in query.iterator(chunk_size=500):
                row = {}
                for field in concrete:
                    value = values[field.attname]
                    if field.is_relation and field.related_model._meta.label_lower == "contenttypes.contenttype" and value is not None:
                        if value not in content_types:
                            raise ValueError("unknown content type reference")
                        value = content_types[value]
                    elif value is not None and field.__class__.__name__ in ("IPAddressField", "IPNetworkField", "MACAddressField", "WWNField"):
                        value = str(value)
                    field_spec = next(item for item in approved if item["name"] == field.name)
                    if field_spec.get("value_shape") == "typed_custom_field_default":
                        value = custom_field_default(value, values["type"], policy)
                    key, value = output_field(field_spec, value)
                    row[key] = value
                if has_custom_fields:
                    row["custom_fields"] = {name: custom_field_value(values[f"_snapshot_cf_{i}"], kind, policy)
                                            for i, (name, kind) in enumerate(selected_custom.items())}
                for field in many:
                    row[field.name] = relation_values(field, model, values[model._meta.pk.attname], content_types)
                validate_generic_pairs(row, spec, exported_types)
                total_bytes += len(json.dumps(row, cls=DjangoJSONEncoder, ensure_ascii=False).encode())
                if total_bytes > 240_000_000:
                    raise ValueError("snapshot row data exceeds size limit")
                rows.append(row)
                total += 1
                if total > 1_000_000:
                    raise ValueError("snapshot row limit exceeded")
            resources.append({"resource_key": spec["resource_key"], "endpoint": spec["endpoint"], "rows": rows})
        reconciliation = reconcile_port_template_mappings(resources, apps)
        processes = [{"area": "plugins", "status": "observed", "evidence": plugin_evidence}]
        for area, labels in (
            ("scripts_and_jobs", ("extras.script", "core.job")),
            ("webhooks", ("extras.webhook",)),
            ("event_rules", ("extras.eventrule",)),
            ("permissions", ("users.objectpermission",)),
        ):
            counts = {label: apps.get_model(label)._base_manager.count() for label in labels}
            processes.append({"area": area, "status": "observed", "evidence": {"model_counts": counts, "execution_observed": False}})
        ended = datetime.now(timezone.utc).isoformat()
        return {"format_version": 1, "netbox_version": "4.7.1", "source_provider_id": str(provider),
                "exporter_source_sha": source_sha, "started_at": started, "ended_at": ended,
                "contract_sha256": contract_sha, "schema": schema,
                "discovery": {"complete": True, "model_count": model_count,
                              "exported_model_count": len(resources),
                              "excluded_model_count": model_count - len(resources),
                              "bootstrap": "core-only settings.configure", "source_configuration_imported": False,
                              "credential_policy": "excluded by pinned model and field dispositions before projection",
                              "untyped_payload_policy": "excluded before projection",
                              "custom_field_exclusions": custom_excluded,
                              "relation_reconciliation": reconciliation},
                "consistency": {"isolation_level": "repeatable read", "read_only": True, "snapshot": snapshot},
                "processes": processes, "resources": resources}


if __name__ == "__main__":
    try:
        main()
    except Exception:
        # Driver and framework exception strings may contain connection secrets.
        raise SystemExit("snapshot export failed; no snapshot was emitted") from None
