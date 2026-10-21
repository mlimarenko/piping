#!/usr/bin/env python3
"""Idempotent NetBox seeder for the round-trip integration tests.

Reads golden fixtures from `apps/api/tests/fixtures/netbox/*.json` and
upserts the entities into a running NetBox instance via its REST API.
Designed for NetBox 4.x (per `docker-compose.yml`).

Usage:
    NETBOX_URL=http://localhost:28080 \
    NETBOX_API_TOKEN=netbox-admin-token \
    python3 scripts/netbox-seed.py

The script is idempotent: re-running it skips existing rows (matched by
slug for slug-bearing models, or by name/composite key otherwise) and
PATCHes the matching record so the fixture state is the canonical truth
after a successful run. The dependency order must be preserved.

Tries pynetbox first when available; falls back to plain `urllib` so the
script also works on hosts without the pynetbox SDK installed (e.g. CI).
"""

from __future__ import annotations

import json
import os
import sys
import time
from pathlib import Path
from typing import Any
from urllib import error as urlerror
from urllib import parse as urlparse
from urllib import request as urlrequest


REPO_ROOT = Path(__file__).resolve().parent.parent
FIXTURE_DIR = REPO_ROOT / "apps" / "api" / "tests" / "fixtures" / "netbox"


# Endpoint paths reused across the seed order, the nested-FK resolver, and the
# primary-IP link pass. Defined once to avoid literal drift.
DCIM_DEVICES_PATH = "dcim/devices"
VIRTUALIZATION_VMS_PATH = "virtualization/virtual-machines"


# Order matters: dependencies must exist before dependents.
SEED_ORDER: list[tuple[str, str]] = [
    ("region", "dcim/regions"),
    ("site_group", "dcim/site-groups"),
    ("tenant_group", "tenancy/tenant-groups"),
    ("tenant", "tenancy/tenants"),
    ("site", "dcim/sites"),
    ("manufacturer", "dcim/manufacturers"),
    ("platform", "dcim/platforms"),
    ("device_type", "dcim/device-types"),
    ("device_role", "dcim/device-roles"),
    ("rack", "dcim/racks"),
    ("cluster_type", "virtualization/cluster-types"),
    ("cluster_group", "virtualization/cluster-groups"),
    ("cluster", "virtualization/clusters"),
    ("device", DCIM_DEVICES_PATH),
    ("interface", "dcim/interfaces"),
    ("ip_address", "ipam/ip-addresses"),
    ("cable", "dcim/cables"),
    ("vlan", "ipam/vlans"),
    ("prefix", "ipam/prefixes"),
    ("virtual_machine", VIRTUALIZATION_VMS_PATH),
    ("vm_interface", "virtualization/interfaces"),
    ("contact_group", "tenancy/contact-groups"),
    ("contact_role", "tenancy/contact-roles"),
    ("contact", "tenancy/contacts"),
    ("contact_assignment", "tenancy/contact-assignments"),
]


# Fields that reference other entities by NetBox int id. The seed script
# rewrites nested fixture objects (e.g. {"id": 1, "slug": "..."}) into
# whatever shape the NetBox 4.x write API expects.
NESTED_FIELDS_TO_RESOLVE: dict[str, dict[str, str]] = {
    "site": {"region": "dcim/regions", "tenant": "tenancy/tenants"},
    "rack": {"site": "dcim/sites", "tenant": "tenancy/tenants"},
    "platform": {"manufacturer": "dcim/manufacturers"},
    "device_type": {"manufacturer": "dcim/manufacturers"},
    "device": {
        "device_type": "dcim/device-types",
        "role": "dcim/device-roles",
        "site": "dcim/sites",
        "platform": "dcim/platforms",
        "tenant": "tenancy/tenants",
        "rack": "dcim/racks",
        "cluster": "virtualization/clusters",
    },
    "interface": {"device": DCIM_DEVICES_PATH},
    "ip_address": {"tenant": "tenancy/tenants"},
    "cluster": {
        "type": "virtualization/cluster-types",
        "group": "virtualization/cluster-groups",
        "site": "dcim/sites",
        "tenant": "tenancy/tenants",
    },
    "virtual_machine": {
        "cluster": "virtualization/clusters",
        "site": "dcim/sites",
        "role": "dcim/device-roles",
        "tenant": "tenancy/tenants",
        "platform": "dcim/platforms",
    },
    "vm_interface": {"virtual_machine": VIRTUALIZATION_VMS_PATH},
    "vlan": {"site": "dcim/sites", "tenant": "tenancy/tenants"},
    "prefix": {
        "site": "dcim/sites",
        "tenant": "tenancy/tenants",
        "vlan": "ipam/vlans",
    },
    "tenant": {"group": "tenancy/tenant-groups"},
    "contact": {"group": "tenancy/contact-groups"},
    "contact_assignment": {
        "contact": "tenancy/contacts",
        "role": "tenancy/contact-roles",
    },
}


# Track seeded entities so dependencies can be resolved by slug/name.
SEEDED: dict[str, dict[str, int]] = {}


class NetBoxClient:
    def __init__(self, base_url: str, token: str, timeout: float = 30.0) -> None:
        self.base_url = base_url.rstrip("/")
        self.token = token
        self.timeout = timeout

    def _request(
        self,
        method: str,
        path: str,
        payload: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        url = f"{self.base_url}{path}"
        data = None
        if payload is not None:
            data = json.dumps(payload).encode("utf-8")
        req = urlrequest.Request(url=url, method=method, data=data)
        req.add_header("Authorization", f"Token {self.token}")
        req.add_header("Accept", "application/json")
        if data is not None:
            req.add_header("Content-Type", "application/json")
        try:
            with urlrequest.urlopen(req, timeout=self.timeout) as resp:
                body = resp.read().decode("utf-8")
                if not body:
                    return {}
                return json.loads(body)
        except urlerror.HTTPError as exc:
            err_body = exc.read().decode("utf-8", errors="replace")
            raise SystemExit(
                f"[netbox-seed] {method} {path} -> HTTP {exc.code}: {err_body}"
            ) from exc
        except urlerror.URLError as exc:
            raise SystemExit(
                f"[netbox-seed] {method} {path} -> network error: {exc}"
            ) from exc

    def get(self, path: str, query: dict[str, str] | None = None) -> dict[str, Any]:
        if query:
            path = f"{path}?{urlparse.urlencode(query)}"
        return self._request("GET", path)

    def post(self, path: str, payload: dict[str, Any]) -> dict[str, Any]:
        return self._request("POST", path, payload)

    def patch(self, path: str, payload: dict[str, Any]) -> dict[str, Any]:
        return self._request("PATCH", path, payload)


def wait_for_netbox(client: NetBoxClient, timeout_seconds: int = 120) -> None:
    deadline = time.monotonic() + timeout_seconds
    last_err: str | None = None
    while time.monotonic() < deadline:
        try:
            client.get("/api/status/")
            return
        except SystemExit as exc:
            last_err = str(exc)
            time.sleep(2)
    raise SystemExit(
        f"[netbox-seed] NetBox not ready within {timeout_seconds}s: {last_err}"
    )


def lookup_existing(
    client: NetBoxClient,
    endpoint: str,
    fixture_kind: str,
    record: dict[str, Any],
) -> int | None:
    """Return existing NetBox int id for a fixture record, or None."""
    # Slug-keyed entities: dedupe by slug.
    slug = record.get("slug")
    if slug:
        result = client.get(f"/api/{endpoint}/", {"slug": slug, "limit": "1"})
        if result.get("count", 0) > 0:
            return int(result["results"][0]["id"])

    # device, vm, rack, contact: dedupe by name (scoped where possible).
    if fixture_kind == "device":
        params = {"name": record["name"], "limit": "1"}
        site_slug = record.get("site", {}).get("slug")
        if site_slug:
            params["site"] = site_slug
        result = client.get(f"/api/{endpoint}/", params)
        if result.get("count", 0) > 0:
            return int(result["results"][0]["id"])

    if fixture_kind in {
        "virtual_machine", "rack", "contact", "contact_group",
        "cluster", "cluster_type", "cluster_group",
    }:
        result = client.get(f"/api/{endpoint}/", {"name": record["name"], "limit": "1"})
        if result.get("count", 0) > 0:
            return int(result["results"][0]["id"])

    if fixture_kind == "interface":
        device_id = record.get("_device_id")
        if device_id is None:
            return None
        result = client.get(
            f"/api/{endpoint}/",
            {"name": record["name"], "device_id": str(device_id), "limit": "1"},
        )
        if result.get("count", 0) > 0:
            return int(result["results"][0]["id"])

    if fixture_kind == "vm_interface":
        vm_id = record.get("_virtual_machine_id")
        if vm_id is None:
            return None
        result = client.get(
            f"/api/{endpoint}/",
            {
                "name": record["name"],
                "virtual_machine_id": str(vm_id),
                "limit": "1",
            },
        )
        if result.get("count", 0) > 0:
            return int(result["results"][0]["id"])

    if fixture_kind == "ip_address":
        result = client.get(
            f"/api/{endpoint}/", {"address": record["address"], "limit": "1"}
        )
        if result.get("count", 0) > 0:
            return int(result["results"][0]["id"])

    if fixture_kind == "vlan":
        result = client.get(
            f"/api/{endpoint}/",
            {"vid": str(record["vid"]), "name": record["name"], "limit": "1"},
        )
        if result.get("count", 0) > 0:
            return int(result["results"][0]["id"])

    if fixture_kind == "prefix":
        result = client.get(
            f"/api/{endpoint}/", {"prefix": record["prefix"], "limit": "1"}
        )
        if result.get("count", 0) > 0:
            return int(result["results"][0]["id"])

    if fixture_kind == "cable":
        # No simple lookup; defer to label match.
        label = record.get("label")
        if label:
            result = client.get(f"/api/{endpoint}/", {"label": label, "limit": "1"})
            if result.get("count", 0) > 0:
                return int(result["results"][0]["id"])

    if fixture_kind == "contact_assignment":
        params = {
            "object_type": record.get("content_type", ""),
            "object_id": str(record.get("object_id", "")),
            "contact_id": str(record.get("_contact_id", "")),
            "limit": "1",
        }
        result = client.get(f"/api/{endpoint}/", params)
        if result.get("count", 0) > 0:
            return int(result["results"][0]["id"])

    return None


def resolve_nested_id(
    fixture_kind: str,
    field_name: str,
    nested_obj: Any,
) -> int | None:
    """Resolve a nested fixture object to its actual NetBox int id."""
    if nested_obj is None:
        return None
    if not isinstance(nested_obj, dict):
        return None

    parent_kind_map = NESTED_FIELDS_TO_RESOLVE.get(fixture_kind, {})
    parent_endpoint = parent_kind_map.get(field_name)
    if parent_endpoint is None:
        return None

    # Reverse-map endpoint to fixture kind to look up SEEDED
    endpoint_to_kind: dict[str, str] = {
        endpoint: kind for kind, endpoint in SEED_ORDER
    }
    parent_kind = endpoint_to_kind.get(parent_endpoint)
    if parent_kind is None:
        return None

    seeded_for_kind = SEEDED.get(parent_kind, {})
    slug = nested_obj.get("slug")
    if slug and slug in seeded_for_kind:
        return seeded_for_kind[slug]
    name = nested_obj.get("name")
    if name and name in seeded_for_kind:
        return seeded_for_kind[name]
    # Fall back to the literal id from the fixture (only useful for first run).
    return nested_obj.get("id")


def build_payload(fixture_kind: str, record: dict[str, Any]) -> dict[str, Any]:
    """Translate a fixture record into a NetBox 4.x write payload."""
    payload: dict[str, Any] = {}
    nested_map = NESTED_FIELDS_TO_RESOLVE.get(fixture_kind, {})

    for key, value in record.items():
        if key in {"id", "created", "last_updated"}:
            continue
        if key.startswith("_"):
            continue

        # Status / role / type / priority fixtures use {"value": ..., "label": ...}.
        if isinstance(value, dict) and "value" in value and "label" in value:
            payload[key] = value["value"]
            continue

        # Nested FK reference.
        if key in nested_map and isinstance(value, dict):
            resolved = resolve_nested_id(fixture_kind, key, value)
            payload[key] = resolved
            continue

        if key == "type" and fixture_kind == "cable":
            payload[key] = value
            continue

        payload[key] = value

    # NetBox 4.x rejects `null` on several non-nullable string fields
    # (e.g. site.facility / site.physical_address). Drop None entries so
    # NetBox falls back to its column default (empty string).
    payload = {k: v for k, v in payload.items() if v is not None}

    # primary_ip4/primary_ip6 require pre-existing IPAddress rows; seeder
    # creates IPs *after* devices, so strip these on initial POST and rely
    # on a follow-up PATCH pass (or downstream sync) to populate them.
    if fixture_kind in {"device", "virtual_machine"}:
        payload.pop("primary_ip4", None)
        payload.pop("primary_ip6", None)

    # Cable terminations need explicit shape on POST.
    if fixture_kind == "cable":
        a_terms = record.get("a_terminations", [])
        b_terms = record.get("b_terminations", [])
        a_id = SEEDED.get("interface", {}).get(
            f"_id_{a_terms[0].get('object_id')}"
        ) if a_terms else None
        b_id = SEEDED.get("interface", {}).get(
            f"_id_{b_terms[0].get('object_id')}"
        ) if b_terms else None
        payload["a_terminations"] = (
            [{"object_type": "dcim.interface", "object_id": a_id}] if a_id else []
        )
        payload["b_terminations"] = (
            [{"object_type": "dcim.interface", "object_id": b_id}] if b_id else []
        )

    if fixture_kind == "contact_assignment":
        contact_id = SEEDED.get("contact", {}).get(
            record.get("contact", {}).get("name", "")
        )
        role_id = SEEDED.get("contact_role", {}).get(
            record.get("role", {}).get("slug", "")
        ) if record.get("role") else None
        payload["object_type"] = record.get("content_type")
        payload["object_id"] = record.get("object_id")
        payload["contact"] = contact_id
        payload["role"] = role_id

    return payload


def resolve_primary_ip_patch(record: dict[str, Any]) -> dict[str, int]:
    """Map a device/VM record's primary_ip4/ip6 refs to seeded NetBox ids."""
    patch_payload: dict[str, int] = {}
    for ip_field in ("primary_ip4", "primary_ip6"):
        ref = record.get(ip_field)
        if not ref:
            continue
        real_ip_id = SEEDED.get("ip_address", {}).get(f"_id_{ref['id']}")
        if real_ip_id is not None:
            patch_payload[ip_field] = real_ip_id
    return patch_payload


def link_primary_ips_for_kind(
    client: NetBoxClient, fixture_kind: str, endpoint: str
) -> None:
    """Patch primary_ip4/primary_ip6 for every record of one fixture kind."""
    fixture_path = FIXTURE_DIR / f"{fixture_kind}.json"
    if not fixture_path.exists():
        return
    records = json.loads(fixture_path.read_text(encoding="utf-8"))
    for record in records:
        patch_payload = resolve_primary_ip_patch(record)
        if not patch_payload:
            continue
        name = record.get("name")
        entity_id = SEEDED.get(fixture_kind, {}).get(name)
        if entity_id is None:
            continue
        client.patch(f"/api/{endpoint}/{entity_id}/", patch_payload)
        print(f"[netbox-seed] linked {fixture_kind} '{name}' -> {patch_payload}")


def link_primary_ips(client: NetBoxClient) -> None:
    """Second pass: primary_ip4/primary_ip6 reference ip_address rows that
    don't exist yet during the initial device/virtual_machine POST (see the
    strip in build_payload), so patch them in now that ip_address is seeded.
    """
    for fixture_kind, endpoint in (
        ("device", DCIM_DEVICES_PATH),
        ("virtual_machine", VIRTUALIZATION_VMS_PATH),
    ):
        link_primary_ips_for_kind(client, fixture_kind, endpoint)


def seed_fixture_file(client: NetBoxClient, fixture_kind: str, endpoint: str) -> None:
    fixture_path = FIXTURE_DIR / f"{fixture_kind}.json"
    if not fixture_path.exists():
        print(f"[netbox-seed] skip {fixture_kind}: fixture not found at {fixture_path}")
        return
    records = json.loads(fixture_path.read_text(encoding="utf-8"))
    if not isinstance(records, list):
        raise SystemExit(
            f"[netbox-seed] fixture {fixture_path} must be a JSON array"
        )

    SEEDED.setdefault(fixture_kind, {})
    for record in records:
        # Pre-resolve cable termination + contact_assignment references that need
        # the actual seeded ids, captured as private fields on the record.
        if fixture_kind == "interface":
            record["_device_id"] = resolve_nested_id(
                "interface", "device", record.get("device", {})
            )
        if fixture_kind == "vm_interface":
            record["_virtual_machine_id"] = resolve_nested_id(
                "vm_interface",
                "virtual_machine",
                record.get("virtual_machine", {}),
            )
        if fixture_kind == "contact_assignment":
            record["_contact_id"] = SEEDED.get("contact", {}).get(
                record.get("contact", {}).get("name", "")
            )

        payload = build_payload(fixture_kind, record)
        existing_id = lookup_existing(client, endpoint, fixture_kind, record)
        if existing_id is not None:
            updated = client.patch(f"/api/{endpoint}/{existing_id}/", payload)
            entity_id = int(updated.get("id", existing_id))
            action = "patched"
        else:
            created = client.post(f"/api/{endpoint}/", payload)
            entity_id = int(created["id"])
            action = "created"

        # Index by slug, by name, and by fixture id (for cable termination ref).
        slug = record.get("slug")
        if slug:
            SEEDED[fixture_kind][slug] = entity_id
        name = record.get("name")
        if name:
            SEEDED[fixture_kind][name] = entity_id
        if record.get("id") is not None:
            SEEDED[fixture_kind][f"_id_{record['id']}"] = entity_id

        identifier = slug or name or record.get("address") or record.get("prefix")
        print(f"[netbox-seed] {action} {fixture_kind} '{identifier}' -> id={entity_id}")


def main() -> int:
    base_url = os.environ.get("NETBOX_URL") or os.environ.get("NETBOX_BASE_URL")
    if not base_url:
        port = os.environ.get("NETBOX_PORT", "28080")
        base_url = f"http://localhost:{port}"

    token = (
        os.environ.get("NETBOX_API_TOKEN")
        or os.environ.get("NETBOX_SUPERUSER_API_TOKEN")
        or "netbox-admin-token"
    )
    timeout = int(os.environ.get("NETBOX_WAIT_TIMEOUT_SECONDS", "120"))

    print(f"[netbox-seed] target={base_url} fixture_dir={FIXTURE_DIR}")
    if not FIXTURE_DIR.is_dir():
        raise SystemExit(f"[netbox-seed] fixture directory missing: {FIXTURE_DIR}")

    client = NetBoxClient(base_url=base_url, token=token)
    wait_for_netbox(client, timeout_seconds=timeout)
    print("[netbox-seed] NetBox is ready; beginning seed")

    for fixture_kind, endpoint in SEED_ORDER:
        seed_fixture_file(client, fixture_kind, endpoint)

    link_primary_ips(client)

    print(f"[netbox-seed] seed complete: {sum(len(v) for v in SEEDED.values())} indexed entries")
    return 0


if __name__ == "__main__":
    sys.exit(main())
