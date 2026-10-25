"""Authenticated Docker-save transport for the guarded Compose deployment owner."""

from __future__ import annotations

import gzip
import hashlib
import json
import re
import tarfile
from pathlib import PurePosixPath
from datetime import datetime, timezone, timedelta

from scripts.ci.apply_release_bundle import reject_duplicate_json_keys


def require(condition, message):
    if not condition:
        raise ValueError(message)


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode()


PRESERVATION_SCHEMA = "piping.compose-preservation/v1"
PRESERVATION_CONTEXT = {"target", "root", "project_name", "application_source", "helper_sha256",
                        "release_sha256", "postgres_image", "backend_image", "ledger_sha256",
                        "prefix_sha256", "installed_catalog_sha256"}


def package_binding(package):
    """Bind the actual prepared image package without a circular approval hash."""
    core = {k: v for k, v in package.items() if k != "preservation"}
    if "images" in core:
        core["images"] = {role: {k: v for k, v in image.items() if k != "local_id"}
                          for role, image in core["images"].items()}
    return hashlib.sha256(canonical(core)).hexdigest()


def preservation_manifest(value, *, context, package_sha256, now=None):
    """Validate operator authority; observing a target grants no permission."""
    require(isinstance(value, dict) and set(value) == {
        "schema", "status", "context", "package_binding_sha256", "issued_at", "expires_at",
        "inventory", "position_pairs", "check_capture_sha256", "append_slots"}, "invalid preservation manifest")
    require(value["schema"] == PRESERVATION_SCHEMA and value["status"] == "approved",
            "preservation approval has not been issued")
    require(set(value["context"]) == PRESERVATION_CONTEXT and value["context"] == context
            and value["package_binding_sha256"] == package_sha256, "preservation target/source/package binding mismatch")
    require(all(isinstance(v, str) and v for v in context.values()), "incomplete preservation context")
    require(re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", context["application_source"])
            and all(re.fullmatch(r"[0-9a-f]{64}", context[k]) for k in (
                "helper_sha256", "release_sha256", "ledger_sha256", "prefix_sha256", "installed_catalog_sha256"))
            and all(re.fullmatch(r"sha256:[0-9a-f]{64}", context[k]) for k in ("postgres_image", "backend_image"))
            and re.fullmatch(r"[0-9a-f]{64}", package_sha256), "invalid preservation source identities")
    def timestamp(raw):
        require(isinstance(raw, str) and raw.endswith("Z"), "preservation time must be UTC")
        return datetime.fromisoformat(raw.removesuffix("Z") + "+00:00")
    issued, expiry = timestamp(value["issued_at"]), timestamp(value["expires_at"])
    current = now or datetime.now(timezone.utc)
    require(issued <= current < expiry and timedelta(0) < expiry - issued <= timedelta(hours=72),
            "preservation approval expired or has invalid freshness")
    def sha256(raw):
        return isinstance(raw, str) and re.fullmatch(r"[0-9a-f]{64}", raw) is not None
    require(value["check_capture_sha256"] is None or sha256(value["check_capture_sha256"]),
            "invalid preservation capture authority")
    inventory = value["inventory"]
    require(isinstance(inventory, list), "preservation requires an exact object inventory")
    identities = set()
    for item in inventory:
        require(isinstance(item, dict) and set(item) == {"kind", "name", "definition_sha256"}
                and isinstance(item["kind"], str) and isinstance(item["name"], str)
                and sha256(item["definition_sha256"]) and (item["kind"], item["name"]) not in identities,
                "invalid or duplicate preservation object")
        identities.add((item["kind"], item["name"]))
    require(isinstance(value["position_pairs"], dict), "invalid preservation position authority")
    require(value["check_capture_sha256"] is not None or not value["position_pairs"],
            "historical layout exceptions require authenticated native CHECK capture")
    for name, pair in value["position_pairs"].items():
        require(isinstance(name, str) and isinstance(pair, list) and len(pair) == 2
                and all(type(p) is int and 1 <= p <= 1600 for p in pair) and pair[0] != pair[1],
                "invalid preservation position pair")
    require(isinstance(value["append_slots"], dict)
            and all(isinstance(k, str) and type(v) is int and 1 <= v < 1600
                    for k, v in value["append_slots"].items()), "invalid preservation append slots")
    return value


def digest(stream):
    return "sha256:" + hashlib.file_digest(stream, "sha256").hexdigest()


def save_authority(path, expected_image_id, config_digest):
    """Bind an offline archive to an operator-approved native image root."""
    require(re.fullmatch(r"sha256:[0-9a-f]{64}", expected_image_id), "invalid approved native image root")
    # Classic image stores use the config as their immutable root. Containerd
    # stores use a manifest or index; traverse its actual content-addressed blobs.
    if expected_image_id == config_digest:
        return
    with tarfile.open(path, mode="r:") as archive:
        members = archive.getmembers()
        require(len(members) <= 4096 and len({m.name for m in members}) == len(members),
                "ambiguous native authority archive")
        pending, visited, matched = [expected_image_id], set(), 0
        while pending:
            identity = pending.pop()
            if identity in visited:
                continue
            require(len(visited) < 64 and re.fullmatch(r"sha256:[0-9a-f]{64}", identity), "invalid native descriptor graph")
            visited.add(identity)
            name = "blobs/sha256/" + identity.removeprefix("sha256:")
            try:
                member = archive.getmember(name)
            except KeyError:
                continue  # Docker save retains only the locally selected platform.
            require(member.isfile() and member.size <= 1024 * 1024, "invalid native authority descriptor")
            with archive.extractfile(member) as stream:
                raw = stream.read()
            require("sha256:" + hashlib.sha256(raw).hexdigest() == identity, "native authority blob digest mismatch")
            node = json.loads(raw, object_pairs_hook=reject_duplicate_json_keys)
            require(node.get("schemaVersion") == 2, "invalid native descriptor schema")
            if "manifests" in node:
                pending.extend(item["digest"] for item in node["manifests"])
            elif node.get("config", {}).get("digest") == config_digest:
                matched += 1
        require(matched == 1, "archive config is not reachable from the approved image root")


def save_identity(path, config_digest, platform, revision=None):
    """Verify config bytes and every ordered uncompressed layer, without extracting tar."""
    require(re.fullmatch(r"sha256:[0-9a-f]{64}", config_digest), "invalid config identity")
    with tarfile.open(fileobj=path, mode="r:") if hasattr(path, "read") else tarfile.open(path, mode="r:") as archive:
        members = {}
        for member in archive:
            require(len(members) < 4096, "image archive member limit exceeded")
            name = PurePosixPath(member.name)
            require(member.name == str(name) and not name.is_absolute() and ".." not in name.parts
                    and member.name not in members and (member.isfile() or member.isdir()),
                    "unsafe or duplicate image archive member")
            members[member.name] = member

        def member_bytes(name):
            require(name in members and members[name].isfile() and members[name].size <= 1024 * 1024,
                    "missing or oversized image metadata")
            with archive.extractfile(members[name]) as stream:
                return stream.read()

        manifest = json.loads(member_bytes("manifest.json"), object_pairs_hook=reject_duplicate_json_keys)
        require(isinstance(manifest, list) and len(manifest) == 1, "ambiguous save manifest")
        config_bytes = member_bytes(manifest[0]["Config"])
        require("sha256:" + hashlib.sha256(config_bytes).hexdigest() == config_digest,
                "archive config digest mismatch")
        config = json.loads(config_bytes, object_pairs_hook=reject_duplicate_json_keys)
        actual = "/".join(filter(None, (config["os"], config["architecture"], config.get("variant", ""))))
        require(actual == platform, "archive platform mismatch")
        if revision is not None:
            require((config["config"].get("Labels") or {}).get("org.opencontainers.image.revision") == revision,
                    "archive source revision mismatch")
        layers = manifest[0]["Layers"]
        diff_ids = config["rootfs"]["diff_ids"]
        require(config["rootfs"]["type"] == "layers" and isinstance(layers, list)
                and len(layers) == len(diff_ids) and len(set(layers)) == len(layers), "invalid layer chain")
        for name, expected in zip(layers, diff_ids, strict=True):
            require(name in members and members[name].isfile(), "missing image layer")
            with archive.extractfile(members[name]) as stream:
                magic = stream.read(2)
                stream.seek(0)
                if magic == b"\x1f\x8b":
                    with gzip.GzipFile(fileobj=stream) as uncompressed:
                        actual = digest(uncompressed)
                else:
                    actual = digest(stream)
                require(actual == expected, "archive ordered layer digest mismatch")
        return config


def verify_manifest(raw, published_ref, config_digest):
    require(re.fullmatch(r"[^\s@]+@sha256:[0-9a-f]{64}", published_ref), "invalid published image reference")
    require("sha256:" + hashlib.sha256(raw).hexdigest() == published_ref.rsplit("@", 1)[1],
            "published manifest digest mismatch")
    manifest = json.loads(raw, object_pairs_hook=reject_duplicate_json_keys)
    require(manifest.get("schemaVersion") == 2 and "manifests" not in manifest
            and manifest.get("mediaType") in ("application/vnd.oci.image.manifest.v1+json",
                                             "application/vnd.docker.distribution.manifest.v2+json")
            and manifest["config"]["digest"] == config_digest, "invalid published manifest/config chain")
    return manifest
