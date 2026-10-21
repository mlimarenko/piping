#!/usr/bin/env python3
"""Run disposable native fixtures in the Docker daemon's network namespace.

The outer CLI may use CI mutual TLS. The inner client uses only the daemon's
Unix socket and loopback-published fixture ports; no registry trust is changed.
Use a fixture-only daemon: its socket grants the client full engine authority.
"""

from __future__ import annotations
from contextlib import contextmanager


import argparse
import io
import hashlib
import json
import fnmatch
import gzip
import os
import re
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import re
import sys
import unittest
import traceback
import uuid
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
FIXTURE = "tests/compose/test_deploy_release_bundle.py"
SOURCE_PATHS = (
    "tests/compose/__init__.py",
    FIXTURE,
    "tests/ci/test_postgres_migration_contract.py",
    "apps/api/migrations/*.sql",
    "apps/api/migrations/compose-deployment.json",
    "apps/api/config/default.toml",
    "scripts/deploy_release_bundle.py",
    "scripts/ci/apply_release_bundle.py",
    "scripts/ci/postgres_migration_contract.py",
    "scripts/ci/compose_delivery_contract.py",
    "scripts/ci-compose-native.py",
)
BASELINE_SOURCE_PATHS = (*SOURCE_PATHS,
    "tests/ci/test_sqlx_rebaseline.py",
    "apps/api/tests/fixtures/netbox/frozen_migration_prefix.json",
    "apps/api/migrations/maintenance-transition.json",
    "scripts/rebaseline_sqlx.py", "scripts/rebaseline-sqlx-0001.sh",
)
EXPORT_METADATA = "compose-native-source.json"
# Registered historical CHECK oracle used by the mandatory helper CI lane.
HISTORICAL_HELPER_REVISION = "17d6896c7b20437e69bf2b2800a586720fff8a13"
APPLICATION_REVISION = "5e4b78bfe647ea55829f24992b944cd2a8392172"
BACKEND_IMAGE = "gitlab.piping.space:4567/piping-space/products/piping/piping.app/backend@sha256:edf2ae43eb2d0ec13f13830e55afe94cbf7d41d2bd5f4d115b4be92eb069aa25"
BACKEND_BINARY_SHA256 = "bfd2cf6b7056130d534f1c049edfc2966b83c1b4c5ef9c85e9f752e4f2033ab9"
# Current canonical-installation artifact proof is not historical registration.
PRODUCER_APPLICATION_REVISION = "c51fbcc69022176bb01c7917860bb36b2916917a"
# Last pre-baseline helper owns the genuine 23/24 artifact contract. The baseline
# helper must not be applied to those artifacts or relabel their old receipts.
ARTIFACT_HELPER_REVISION = "4c5140d9511fea54e7cb02bfb94799713147b4e5"
PRODUCER_BINARY_SHA256 = "0b823c6df05f67fc6fc5901a2b08d5e266d9866a582f5b3354202b6337ff313d"
# Published release bundle from producer job 216330; no fixture image builds.
PRODUCER_IMAGES = {
    "backend": "gitlab.piping.space:4567/piping-space/products/piping/piping.app/backend@sha256:c6370f2c8085448ad7e0c677c6fcb7b00152bbce73c602475e266e1dd869dd03",
    "frontend": "gitlab.piping.space:4567/piping-space/products/piping/piping.app/frontend@sha256:6e9796a4aad6e166f19936f684204dfe4b0af720235fe4ea830990a40affe8d8",
    "ansible-runner": "gitlab.piping.space:4567/piping-space/products/piping/piping.app/ansible-runner@sha256:31ae777766e8802993ea257e45d5f0600ef3cc3d4da90c9ba8122d6ef1c088ab",
}
PRODUCER_VERSION = "1.0.125"
PRODUCER_PIPELINE_ID = "19141"
CHECK_PREFIX_SHA256 = "45d7d65473b8fe39947105fe6350824fb9ef224b6968596bf86e2aca39316485"
POSTGRES_FIXTURE_SHA256 = "35507deeb9e9f8bf5dfff7ae2bcf197273cd998cb4cdfd0048eca7f8cb32e6a3"
POSTGRES_FIXTURE_SIZE = 114702336
POSTGRES_FIXTURE_IMAGE_ID = "sha256:54451ecb8ab38c24c3ec123f2fd501303a3a1856a5c66e98cecf2460d5e1e9d7"
POSTGRES_FIXTURE_CONFIG = "sha256:f6d4985d97e69bb9de4d5e24f9f24cbfaa90d30877ce935a8ee872f62efbb621"
POSTGRES_FIXTURE_PLATFORM = "sha256:1b13c640ae11f2f165d1e89667e5862b0017baf4c80fec2fb7377d86319859ba"
POSTGRES_FIXTURE_URL = (
    "https://gitlab.piping.space/api/v4/projects/33/packages/generic/piping-compose-pg-fixture/"
    + POSTGRES_FIXTURE_SHA256 + "/image-" + POSTGRES_FIXTURE_IMAGE_ID.split(":")[1] + ".tar"
)
NATIVE_TESTS = (
    "tests.compose.test_deploy_release_bundle.NativeComposeDeploymentTests",
    "tests.compose.test_deploy_release_bundle.NativeArchiveBootstrapTests",
    "tests.compose.test_deploy_release_bundle.NativeRestoreCapacityTests",
    "tests.compose.test_deploy_release_bundle.NativeHistoricalPreservationTests",
)
BASELINE_TESTS = (
    "tests.ci.test_sqlx_rebaseline",
    "tests.compose.test_deploy_release_bundle.NativeMigrationContractTests",
    "tests.compose.test_deploy_release_bundle.NativeComposeDeploymentTests",
)
HTTPS_SOURCE_PATHS = (
    "install.sh", "env/images.env", "docker-compose.yml", "docker/nginx/*",
    "scripts/public_release.py",
    "scripts/deploy_release_bundle.py", "scripts/ci/apply_release_bundle.py",
    "scripts/ci/compose_delivery_contract.py", "scripts/ci/postgres_migration_contract.py",
    "tests/compose/test_install_lifecycle.py",
    "tests/compose/__init__.py", "tests/compose/test_install_https.py",
)
HTTPS_TESTS = ("tests.compose.test_install_https",)

PUBLIC_RECEIPTS = frozenset({
    "native-flow-receipt.json", "native-bootstrap-receipt.json",
    "capacity-receipt.json", "published-restore-receipt.json",
    "historical-preservation-public.json",
})
PRESERVATION_FLAGS = frozenset({
    "all12_native_checks_including_nested_and", "all_preexisting_objects_exact",
    "all_old_column_multisets_conserved", "sequences_and_ledger_metadata_conserved",
    "original_check_bindings_and_layout_conserved", "approval_required_before_writer_stop",
    "incomplete_expired_wrong_target_approval_denied",
})


def postgres_fixture_identity():
    return {"url": POSTGRES_FIXTURE_URL, "archive_sha256": POSTGRES_FIXTURE_SHA256,
            "archive_size": POSTGRES_FIXTURE_SIZE, "image_id": POSTGRES_FIXTURE_IMAGE_ID,
            "config_digest": POSTGRES_FIXTURE_CONFIG, "platform_manifest": POSTGRES_FIXTURE_PLATFORM}


def verify_postgres_fixture(path):
    """Verify the audited image-only save, including uncompressed layer identity."""
    path = Path(path)
    if path.is_symlink() or not path.is_file() or path.stat().st_size != POSTGRES_FIXTURE_SIZE:
        raise ValueError("PostgreSQL fixture size/type mismatch")
    with path.open("rb") as stream:
        if hashlib.file_digest(stream, "sha256").hexdigest() != POSTGRES_FIXTURE_SHA256:
            raise ValueError("PostgreSQL fixture archive SHA256 mismatch")
    with tarfile.open(path) as archive:
        def blob(digest):
            content = archive.extractfile("blobs/sha256/" + digest.split(":")[1]).read()
            if "sha256:" + hashlib.sha256(content).hexdigest() != digest:
                raise ValueError("PostgreSQL OCI blob mismatch")
            return json.loads(content)
        # Docker 29 reports the OCI INDEX digest as image.Id. It is not the
        # config digest: preserve both typed identities rather than alias them.
        index = blob(POSTGRES_FIXTURE_IMAGE_ID)
        manifests = [item for item in index["manifests"]
                     if item.get("platform") == {"architecture": "amd64", "os": "linux"}]
        if len(manifests) != 1 or manifests[0]["digest"] != POSTGRES_FIXTURE_PLATFORM:
            raise ValueError("PostgreSQL platform manifest mismatch")
        platform = blob(POSTGRES_FIXTURE_PLATFORM)
        if platform["config"]["digest"] != POSTGRES_FIXTURE_CONFIG:
            raise ValueError("PostgreSQL config identity mismatch")
        config = blob(POSTGRES_FIXTURE_CONFIG)
        env = dict(item.split("=", 1) for item in config["config"]["Env"])
        if config["os"] != "linux" or config["architecture"] != "amd64" or env.get("PG_VERSION") != "18.3":
            raise ValueError("PostgreSQL platform/version mismatch")
        manifest = json.load(archive.extractfile("manifest.json"))
        if len(manifest) != 1 or manifest[0]["Config"] != "blobs/sha256/" + POSTGRES_FIXTURE_CONFIG.split(":")[1]:
            raise ValueError("Docker-save config mapping mismatch")
        layers = manifest[0]["Layers"]
        diffids = config["rootfs"]["diff_ids"]
        if len(layers) != len(diffids):
            raise ValueError("PostgreSQL layer inventory mismatch")
        for name, expected in zip(layers, diffids):
            with archive.extractfile(name) as stream:
                prefix = stream.read(2)
                stream.seek(0)
                decoded = gzip.GzipFile(fileobj=stream) if prefix == b"\x1f\x8b" else stream
                if "sha256:" + hashlib.file_digest(decoded, "sha256").hexdigest() != expected:
                    raise ValueError("PostgreSQL layer diffID mismatch")
    return postgres_fixture_identity()


def download_postgres_fixture(target, environment):
    """Use only this project's existing job identity; never redirect its token."""
    token = environment.get("CI_JOB_TOKEN")
    if not token:
        raise ValueError("CI_JOB_TOKEN is required for the private fixture")
    target = Path(target)
    if not target.is_absolute() or any(p.is_symlink() or not p.is_dir() for p in target.parents):
        raise ValueError("fixture destination requires existing non-symlink ancestors")

    class NoRedirect(urllib.request.HTTPRedirectHandler):
        def redirect_request(self, *args, **kwargs):
            raise ValueError("fixture redirect refused; job token must stay at its owner")

    request = urllib.request.Request(POSTGRES_FIXTURE_URL, headers={"JOB-TOKEN": token})
    try:
        with urllib.request.build_opener(NoRedirect()).open(request, timeout=120) as response:
            length = response.headers.get("Content-Length")
            if response.status != 200 or (length is not None and length != str(POSTGRES_FIXTURE_SIZE)):
                raise ValueError("fixture response status/size mismatch")
            descriptor = os.open(target, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
            identity = os.fstat(descriptor)
            with os.fdopen(descriptor, "wb") as stream:
                size = 0
                while content := response.read(min(1024 * 1024, POSTGRES_FIXTURE_SIZE - size + 1)):
                    size += len(content)
                    if size > POSTGRES_FIXTURE_SIZE:
                        raise ValueError("fixture response exceeds its pinned size")
                    stream.write(content)
    except urllib.error.HTTPError as error:
        raise ValueError("private fixture download refused: HTTP " + str(error.code)) from None
    observed = target.lstat()
    if target.is_symlink() or (observed.st_dev, observed.st_ino) != (identity.st_dev, identity.st_ino):
        raise ValueError("fixture destination inode changed during download")
    return verify_postgres_fixture(target)


def validate_preservation_receipt(value, helper_sha256):
    """Accept only the bounded public success contract, never an approval."""
    fixed = {"schema", "helper_sha256", "application_source", "approved_objects",
             "synthetic_relations", "physical_layout_tables", "native_check_bindings",
             "native_leader_restore_and_apply", "preservation_sha256", "backend_binary_sha256",
             "backend_image", "prefix_sha256", "postgres_image", "successful_23_empty_tail_restore_apply"}
    if not isinstance(value, dict) or set(value) != fixed | PRESERVATION_FLAGS:
        raise ValueError("incomplete or nonpublic preservation receipt")
    if value["schema"] != "piping.compose-preservation-proof/v1" \
            or value["helper_sha256"] != helper_sha256 \
            or value["application_source"] != APPLICATION_REVISION \
            or value["backend_binary_sha256"] != BACKEND_BINARY_SHA256 \
            or value["backend_image"] != BACKEND_IMAGE \
            or value["prefix_sha256"] != CHECK_PREFIX_SHA256 \
            or value["postgres_image"] != POSTGRES_FIXTURE_IMAGE_ID:
        raise ValueError("preservation source binding mismatch")
    if any(value[key] is not True for key in PRESERVATION_FLAGS) \
            or value["native_leader_restore_and_apply"] != "passed" \
            or value["successful_23_empty_tail_restore_apply"] is not True:
        raise ValueError("native preservation proof did not pass")
    for key, expected in (("approved_objects", 136), ("synthetic_relations", 8),
                          ("physical_layout_tables", 11), ("native_check_bindings", 12)):
        if type(value[key]) is not int or value[key] != expected:
            raise ValueError("incomplete native preservation coverage")
    digest = value["preservation_sha256"]
    if not isinstance(digest, str) or len(digest) != 64 or any(c not in "0123456789abcdef" for c in digest):
        raise ValueError("invalid preservation digest")


def public_bytes(path):
    if path.is_symlink() or not path.is_file() or path.stat().st_size > 65536:
        raise ValueError("unsafe or oversized public proof")
    return path.read_bytes()


def validate_public_proof(directory, helper_sha256):
    directory = Path(directory)
    index = json.loads(public_bytes(directory / "index.json"))
    if index.get("schema") != "piping.compose-native-proof/v1" or set(index) != {"schema", "files_sha256"}:
        raise ValueError("invalid public index")
    files = index["files_sha256"]
    if not isinstance(files, dict) or set(p.name for p in directory.iterdir()) != set(files) | {"index.json"}:
        raise ValueError("public proof inventory mismatch")
    kinds = set()
    for name, digest in files.items():
        if Path(name).name != name or "-" not in name:
            raise ValueError("unsafe public proof name")
        kind = name.split("-", 1)[1]
        if kind not in PUBLIC_RECEIPTS:
            raise ValueError("unreviewed public receipt")
        content = public_bytes(directory / name)
        if hashlib.sha256(content).hexdigest() != digest:
            raise ValueError("public proof hash mismatch")
        if kind == "historical-preservation-public.json":
            validate_preservation_receipt(json.loads(content), helper_sha256)
        kinds.add(kind)
    if kinds != PUBLIC_RECEIPTS:
        raise ValueError("incomplete public native proof")


def collect_public_proof(root, output, helper_sha256):
    root, output = Path(root), Path(output)
    items = sorted(path for name in PUBLIC_RECEIPTS for path in root.rglob(name))
    if {p.name for p in items} != PUBLIC_RECEIPTS:
        raise ValueError("incomplete native public receipts")
    contents = []
    for path in items:
        content = public_bytes(path)
        if path.name == "historical-preservation-public.json":
            validate_preservation_receipt(json.loads(content), helper_sha256)
        contents.append((path.name, content))
    output.mkdir(mode=0o700)
    hashes = {}
    for number, (name, content) in enumerate(contents):
        name = str(number) + "-" + name
        (output / name).write_bytes(content)
        hashes[name] = hashlib.sha256(content).hexdigest()
    (output / "index.json").write_text(json.dumps({"schema": "piping.compose-native-proof/v1",
                                                 "files_sha256": hashes}, sort_keys=True))
    validate_public_proof(output, helper_sha256)


def collect_artifact_proof(root, output, helper_sha256):
    """Export only the existing safe receipt for the canonical artifact scope."""
    paths = list(Path(root).rglob("published-restore-receipt.json"))
    if len(paths) != 1:
        raise ValueError("one completed published artifact receipt is required")
    content = public_bytes(paths[0])
    value = json.loads(content)
    expected = {"application_source": PRODUCER_APPLICATION_REVISION, "backend_binary_sha256": PRODUCER_BINARY_SHA256,
                "published_ref": PRODUCER_IMAGES["backend"], "postgres_image": POSTGRES_FIXTURE_IMAGE_ID,
                "helper_sha256": helper_sha256,
                "scope": "native archive/migration acceptance; no product startup or historical-target approval"}
    if set(value) != set(expected) | {"cohorts"} or any(value[key] != item for key, item in expected.items()):
        raise ValueError("published artifact identity/scope mismatch")
    cohorts = value["cohorts"]
    if not isinstance(cohorts, list) or len(cohorts) != 2:
        raise ValueError("both installed cohorts must pass")
    for cohort, installed in zip(cohorts, (23, 24)):
        if (set(cohort) != {"installed", "dump_sha256", "native_restore_migration_conservation"}
                or type(cohort["installed"]) is not int or cohort["installed"] != installed
                or cohort["native_restore_migration_conservation"] != "passed"
                or not isinstance(cohort["dump_sha256"], str) or len(cohort["dump_sha256"]) != 64
                or any(c not in "0123456789abcdef" for c in cohort["dump_sha256"])):
            raise ValueError("incomplete native artifact conservation receipt")
    Path(output).mkdir(mode=0o700)
    (Path(output) / paths[0].name).write_bytes(content)


def collect_baseline_proof(root, output, helper_sha256, source_sha, backend_sha256, baseline_sha256):
    """Project only current-epoch success fields; historical receipts are separate."""
    receipts = {}
    required = {
        "baseline-migration-receipt.json": {
            "source_sha": source_sha, "backend_sha256": backend_sha256,
            "baseline_sha256": baseline_sha256, "fresh_baseline": True,
            "nonempty_replay_unchanged": True, "foreign_history_refused_unchanged": True,
        },
        "native-flow-receipt.json": {
            "schema": "piping.compose-native-flow/v1", "helper_sha256": helper_sha256,
            "synthetic_transport_cohort": True, "native_migrator_proved": True,
            "application_source": source_sha, "candidate_absent_before_stage": True,
            "registry_unavailable_before_stage": True, "restore_failure_rejected": True,
            "receipt_forgery_rejected": True, "replay_rejected": True,
            "configuration_database_trigger_drift_rejected": True, "env_only_baseline": True,
            "optional_proxy_absent": True, "failed_start_writers_stopped": True, "ledger_versions": 1,
        },
    }
    for name, expected in required.items():
        paths = list(Path(root).rglob(name))
        if len(paths) != 1:
            raise ValueError("one completed current-baseline receipt of each kind is required")
        value = json.loads(public_bytes(paths[0]))
        if value != expected or any(type(value[k]) is not type(v) for k, v in expected.items()):
            raise ValueError("current-baseline receipt identity or coverage mismatch")
        receipts[name] = value
    Path(output).mkdir(mode=0o700)
    (Path(output) / "baseline-native-proof.json").write_text(json.dumps({
        "schema": "piping.compose-baseline-proof/v1", "receipts": receipts,
    }, sort_keys=True))


def source_archive(root: Path, revision: str, fixture_working_tree: bool,
                   application_revision: str = APPLICATION_REVISION, *, source_paths=SOURCE_PATHS,
                   application_paths=("apps/api/migrations", "apps/api/config/default.toml"),
                   source_scope: str | None = None) -> bytes:
    """Export only fixture-owned inputs, never checkout secrets or build caches."""
    tracked = subprocess.run(
        ["git", "ls-tree", "-r", "--name-only", revision, "--",
         *(str(Path(p).parent) if "*" in p else p for p in source_paths)],
        cwd=root, capture_output=True, check=True, timeout=30,
    ).stdout.decode().splitlines()
    tracked = [name for name in tracked if any(fnmatch.fnmatchcase(name, pattern) for pattern in source_paths)]
    if not tracked:
        raise ValueError("fixture source tree is empty")
    # Include paths by actual tree identity, not pathspecs absent on an older local base.
    archive = subprocess.run(
        ["git", "archive", "--format=tar", revision, "--", *tracked],
        cwd=root, capture_output=True, check=True, timeout=30,
    ).stdout
    application = subprocess.run(
        ["git", "archive", "--format=tar", application_revision, "--",
         *application_paths],
        cwd=root, capture_output=True, check=True, timeout=30,
    ).stdout
    contents = {}
    for source in (archive, application):
        with tarfile.open(fileobj=io.BytesIO(source)) as bundle:
            for member in bundle:
                if member.isfile() and any(fnmatch.fnmatchcase(member.name, p) for p in source_paths):
                    contents[member.name] = (member.mode, bundle.extractfile(member).read())
    if fixture_working_tree:
        # Only helper/fixture inputs may overlay the helper SHA. Application SQL,
        # contract JSON and config always come from the immutable image source.
        for pattern in source_paths:
            if pattern.startswith("apps/"):
                continue
            for path in root.glob(pattern):
                if path.is_symlink() or not path.is_file():
                    raise ValueError("fixture source must be a regular non-symlink file")
                contents[str(path.relative_to(root))] = (path.stat().st_mode & 0o777, path.read_bytes())
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode="w") as bundle:
        for name, (mode, content) in sorted(contents.items()):
            member = tarfile.TarInfo(name)
            member.size, member.mode = len(content), mode
            bundle.addfile(member, io.BytesIO(content))
        source_sha = subprocess.run(["git", "rev-parse", revision + "^{commit}"], cwd=root,
                                    capture_output=True, check=True, timeout=30).stdout.decode().strip()
        metadata = {"schema": "piping.compose-native-source/v1", "helper_commit": source_sha,
                                "application_commit": application_revision, "dirty_helper_overlay": fixture_working_tree,
                                "files_sha256": {name: hashlib.sha256(content).hexdigest()
                                                 for name, (_, content) in sorted(contents.items())}}
        if source_scope is not None:
            metadata["scope"] = source_scope
        metadata = json.dumps(metadata, sort_keys=True, separators=(",", ":")).encode()
        member = tarfile.TarInfo(EXPORT_METADATA)
        member.size, member.mode = len(metadata), 0o644
        bundle.addfile(member, io.BytesIO(metadata))
    return output.getvalue()


def baseline_source_archive(root: Path, revision: str) -> tuple[bytes, str]:
    """Use one exact committed revision for both helpers and application inputs."""
    if revision != "HEAD" and not re.fullmatch(r"[0-9a-f]{40}", revision):
        raise ValueError("current-baseline revision must be HEAD or a complete commit SHA")
    source = subprocess.run(["git", "rev-parse", "--verify", revision + "^{commit}"], cwd=root,
                            capture_output=True, check=True, timeout=30).stdout.decode().strip()
    if not re.fullmatch(r"[0-9a-f]{40}", source):
        raise ValueError("current-baseline source must resolve to one commit")
    return source_archive(root, source, False, application_revision=source,
                          source_paths=BASELINE_SOURCE_PATHS), source


def collect_exported_public_proof(workspace, root, output):
    collect_public_proof(root, output, exported_helper_identity(workspace))


def exported_helper_identity(workspace):
    """Invoke the canonical producer on hash-verified exported source only."""
    workspace = Path(workspace).absolute()
    metadata_path = workspace / EXPORT_METADATA
    if metadata_path.is_symlink() or not metadata_path.is_file() or metadata_path.stat().st_size > 1024 * 1024:
        raise ValueError("invalid exported source metadata file")
    metadata = json.loads(metadata_path.read_bytes())
    if metadata.get("schema") != "piping.compose-native-source/v1":
        raise ValueError("invalid exported source metadata")
    paths = ("scripts/deploy_release_bundle.py", "scripts/ci/compose_delivery_contract.py",
             "scripts/ci/apply_release_bundle.py", "scripts/ci/postgres_migration_contract.py")
    for name in paths:
        path = workspace / name
        if path.is_symlink() or not path.is_file() or any(
                parent.is_symlink() for parent in (workspace, workspace / "scripts", workspace / "scripts/ci")):
            raise ValueError("helper identity requires regular exported source")
        expected = metadata["files_sha256"].get(name)
        if hashlib.sha256(path.read_bytes()).hexdigest() != expected:
            raise ValueError("exported helper component hash mismatch")
    # A fresh isolated process prevents module-cache/PYTHONPATH leakage from a
    # checkout. The producer owns both the four-file map and canonical hashing;
    # neither CI consumer implements a second identity formula.
    result = subprocess.run(
        [sys.executable, "-I", "-B", "-c",
         "import runpy,sys; print(runpy.run_path(sys.argv[1])['helper_identity']())",
         str(workspace / paths[0])], capture_output=True, check=False, timeout=30,
    )
    identity = result.stdout.strip()
    if result.returncode or len(identity) != 64 or any(c not in b"0123456789abcdef" for c in identity):
        raise ValueError("canonical exported helper identity failed")
    return identity.decode("ascii")


@contextmanager
def daemon_client(image, archive, environment, variables):
    """Transfer source once into a client sharing only the fixture daemon's socket."""
    name = "pwa-native-client-" + uuid.uuid4().hex[:12]
    artifact_root = "/data/artifacts/" + name
    client_env = {
        "DOCKER_HOST": "unix:///var/run/docker.sock",
        "PIPING_NATIVE_ARTIFACT_ROOT": artifact_root,
        "PIPING_NATIVE_SOCKET_ROOT": artifact_root,
        "PIPING_TEST_ARTIFACTS": artifact_root,
        **variables,
    }
    try:
        command = ["docker", "run", "-d", "--name", name, "--user", "0",
                   "--network", "host", "--mount",
                   "type=bind,src=/var/run/docker.sock,dst=/var/run/docker.sock,readonly",
                   "--volume", artifact_root + ":" + artifact_root]
        for key, value in client_env.items():
            command += ["--env", key + "=" + value]
        command += ["--workdir", "/workspace", "--entrypoint", "tail", image, "-f", "/dev/null"]
        subprocess.run(command, check=True, stdout=subprocess.DEVNULL, timeout=180, env=environment)
        subprocess.run(["docker", "exec", name, "mkdir", "-p", "/workspace"], check=True, timeout=30, env=environment)
        subprocess.run(["docker", "cp", "-", name + ":/workspace"], input=archive, check=True, timeout=60, env=environment)
        yield name, artifact_root
    finally:
        subprocess.run(["docker", "rm", "-f", name], check=False, stdout=subprocess.DEVNULL, timeout=30, env=environment)


def safe_test_identity(test):
    """Use parent definition names, never TestCase.id()/subtest parameter reprs."""
    if isinstance(test, unittest.TestCase):
        identity = type(test).__module__ + "." + type(test).__qualname__ + "." + test._testMethodName
        return identity if re.fullmatch(r"tests(?:\.[A-Za-z_][A-Za-z_0-9]*){2,}", identity) and len(identity) <= 240 else "unittest.test"
    # unittest represents fixture failures as _ErrorHolder, not a TestCase.
    description = getattr(test, "description", "")
    match = re.fullmatch(r"(setUpClass|tearDownClass|setUpModule|tearDownModule) \((tests(?:\.[A-Za-z_][A-Za-z_0-9]*)+)\)", description)
    return match[2] + "." + match[1] if match and len(description) <= 240 else "unittest.fixture"


def safe_failure_location(trace, method=None):
    """Prefer the test call site over a shared helper; never project frame values."""
    # The native runner is piped to Python's stdin inside /workspace.
    root = Path.cwd() if __file__ == "<stdin>" else ROOT
    location = {}
    test_location = {}
    while trace is not None:
        try:
            source = Path(trace.tb_frame.f_code.co_filename).resolve()
            relative = source.relative_to(root.resolve()).as_posix() if source.is_file() else ""
        except (ValueError, OSError):
            relative = ""
        if (len(relative) <= 240 and re.fullmatch(
                r"(?:tests|scripts)/(?:[A-Za-z_][A-Za-z_0-9-]*/)*[A-Za-z_][A-Za-z_0-9-]*\.py", relative)
                and type(trace.tb_lineno) is int and trace.tb_lineno > 0):
            location = {"source_file": relative, "source_line": trace.tb_lineno}
            if trace.tb_frame.f_code.co_name == method:
                test_location = location
        trace = trace.tb_next
    return test_location or location


class ProjectedTestResult(unittest.TextTestResult):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.events = []

    def record(self, test, error):
        if len(self.events) < 12:
            name = error[0].__name__
            self.events.append({"test": safe_test_identity(test),
                                "exception": name if re.fullmatch(r"[A-Za-z_][A-Za-z_0-9]{0,79}", name) else "Exception",
                                **safe_failure_location(error[2], getattr(test, "_testMethodName", None))})

    def addError(self, test, err):
        self.record(test, err)
        super().addError(test, err)

    def addFailure(self, test, err):
        self.record(test, err)
        super().addFailure(test, err)

    def addSubTest(self, test, subtest, err):
        if err is not None:
            self.record(test, err)
        super().addSubTest(test, subtest, err)


def run_client_unittest(tests):
    """Keep raw output private at the fd boundary; export result-owned facts only."""
    os.umask(0o077)
    path = Path(os.environ["PIPING_NATIVE_ARTIFACT_ROOT"]) / "client-output.log"
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    sys.stdout.flush()
    sys.stderr.flush()
    saved = (os.dup(1), os.dup(2))
    summary = {"schema": "piping.native-test-result/v1", "exit": 1}
    try:
        os.dup2(descriptor, 1)
        os.dup2(descriptor, 2)
        with os.fdopen(descriptor, "w") as stream:
            try:
                if not tests or any(not re.fullmatch(r"tests(?:\.[A-Za-z_][A-Za-z_0-9]*)+", t) for t in tests):
                    raise ValueError("native test selectors must be definition names")
                suite = unittest.defaultTestLoader.loadTestsFromNames(tests)
                result = unittest.TextTestRunner(stream=stream, verbosity=2, resultclass=ProjectedTestResult).run(suite)
                summary.update(exit=0 if result.wasSuccessful() else 1, tests_run=result.testsRun,
                               failures=len(result.failures), errors=len(result.errors), skipped=len(result.skipped),
                               expected_failures=len(result.expectedFailures), unexpected_successes=len(result.unexpectedSuccesses),
                               events=result.events)
            except BaseException as error:
                traceback.print_exc(file=stream)
                summary.update(errors=1, events=[{"test": "unittest.runner", "exception": type(error).__name__
                               if type(error).__module__ == "builtins" else "Exception"}])
            sys.stdout.flush()
            sys.stderr.flush()
    finally:
        for target, original in zip((1, 2), saved):
            os.dup2(original, target)
            os.close(original)
    print(json.dumps(summary, sort_keys=True), flush=True)
    return summary["exit"]


def client_tests(name, tests, environment, timeout):
    # Send the canonical runner itself: focused HTTPS/install archives need no
    # additional runner file or duplicate implementation in their inventories.
    return subprocess.run(
        ["docker", "exec", "-i", name, "python3", "-", "--client-unittest", *tests],
        input=Path(__file__).read_bytes(), check=False, timeout=timeout, env=environment,
    ).returncode


def clean_client_artifacts(image, artifact_root, environment):
    cleanup = (
        "import pathlib,shutil; root=pathlib.Path('/cleanup'); "
        "[(shutil.rmtree(p) if p.is_dir() and not p.is_symlink() else p.unlink()) for p in root.iterdir()]"
    )
    # Nested DinD stores contain daemon-root-owned files. Remove only this
    # nonce-owned bind root using the same daemon's root identity.
    subprocess.run(["docker", "run", "--rm", "--pull", "never", "--network", "none",
                    "--user", "0", "--mount", "type=bind,src=" + artifact_root + ",dst=/cleanup",
                    "--entrypoint", "python3", image, "-c", cleanup],
                   check=True, timeout=180, env=environment)


def run_native(image: str, archive: bytes, environment: dict[str, str], *, backend_binary: Path,
               backend_sha256: str, migration23_sha256: str | None = None, proof_directory: Path | None = None,
               artifact_inputs: Path | None = None, baseline_source_sha: str | None = None,
               baseline_sha256: str | None = None) -> int:
    if baseline_source_sha is None and (baseline_sha256 is not None
            or not re.fullmatch(r"[0-9a-f]{64}", migration23_sha256 or "")):
        raise ValueError("historical native identity requires its migration23 digest")
    if baseline_source_sha is not None and (artifact_inputs or migration23_sha256
            or not re.fullmatch(r"[0-9a-f]{40}", baseline_source_sha)
            or not re.fullmatch(r"[0-9a-f]{64}", baseline_sha256 or "")):
        raise ValueError("current-baseline identity cannot mix with historical inputs")
    if proof_directory is not None:
        proof_directory = Path(os.path.abspath(proof_directory))
        if proof_directory.exists() or proof_directory.is_symlink() or any(
                p.is_symlink() or not p.is_dir() for p in proof_directory.parents):
            raise ValueError("public proof output must be fresh with non-symlink ancestors")
    variables = {
        "PIPING_NATIVE_BACKEND_BINARY": "/workspace/piping-backend",
        "PIPING_NATIVE_BACKEND_SHA256": backend_sha256,
        "PIPING_NATIVE_MIGRATIONS_DIR": "/workspace/apps/api/migrations",
    }
    if baseline_source_sha:
        variables.update(PIPING_NATIVE_SOURCE_SHA=baseline_source_sha,
                         PIPING_NATIVE_BASELINE_SHA256=baseline_sha256)
    else:
        variables["PIPING_NATIVE_MIGRATION23_SHA256"] = migration23_sha256
    if artifact_inputs:
        variables["PIPING_NATIVE_ARTIFACT_INPUTS"] = "/workspace/artifact-inputs"
    with daemon_client(image, archive, environment, variables) as (name, artifact_root):
        subprocess.run(["docker", "cp", str(backend_binary), name + ":/workspace/piping-backend"],
                       check=True, timeout=300, env=environment)
        if artifact_inputs:
            subprocess.run(["docker", "cp", str(artifact_inputs), name + ":/workspace/artifact-inputs"],
                           check=True, timeout=300, env=environment)
        tests = BASELINE_TESTS if baseline_source_sha else (
            ("tests.compose.test_deploy_release_bundle.NativePublishedArtifactTests",) if artifact_inputs else NATIVE_TESTS)
        status = client_tests(name, tests, environment, 2400)
        if status == 0 and proof_directory is not None:
            proof_directory.mkdir(mode=0o700)
            # Only explicitly public, success-only receipts leave the disposable
            # client. Never collect keys, env files, dumps, dossiers or journals.
            collector = (
                "import runpy; api=runpy.run_path('/workspace/scripts/ci-compose-native.py'); "
                + ("api['collect_baseline_proof'](" + repr(artifact_root) + ",'/workspace/public-proof',"
                   "api['exported_helper_identity']('/workspace')," + repr(baseline_source_sha) + ","
                   + repr(backend_sha256) + "," + repr(baseline_sha256) + ")" if baseline_source_sha else
                  "api['collect_artifact_proof'](" + repr(artifact_root) + ",'/workspace/public-proof',"
                  "api['exported_helper_identity']('/workspace'))" if artifact_inputs else
                 "api['collect_exported_public_proof']('/workspace'," + repr(artifact_root) + ",'/workspace/public-proof')")
            )
            subprocess.run(["docker", "exec", name, "python3", "-c", collector], check=True, timeout=30, env=environment)
            subprocess.run(["docker", "cp", name + ":/workspace/public-proof/.", str(proof_directory)],
                           check=True, timeout=30, env=environment)
        if status == 0:
            clean_client_artifacts(image, artifact_root, environment)
        print("migration native exit=" + str(status))
        return status



def https_source_archive(root, revision, *, install=False):
    """Export only committed HTTPS inputs; there is no dirty overlay path."""
    if install:
        # The real installed images are the pinned pre-baseline24 cohort. Keep
        # their schema helper with them, while testing the current launcher/TLS
        # inputs. Current baseline helper/runtime acceptance is a separate route.
        return source_archive(root, revision, False, application_revision=ARTIFACT_HELPER_REVISION,
            source_paths=HTTPS_SOURCE_PATHS,
            source_scope="current-launcher/pinned-prebaseline24-installation", application_paths=(
                "env/images.env", "scripts/deploy_release_bundle.py",
                "scripts/ci/apply_release_bundle.py", "scripts/ci/compose_delivery_contract.py",
                "scripts/ci/postgres_migration_contract.py"))
    return subprocess.run(
        ["git", "archive", "--format=tar", revision, "--", *HTTPS_SOURCE_PATHS],
        cwd=root, check=True, capture_output=True, timeout=30,
    ).stdout


def run_https(image, archive, environment, *, install=False):
    variables = {}
    tests, deadline = HTTPS_TESTS, 180
    if install:
        alternate = environment.get("PIPING_INSTALL_TEST_FRONTEND_IMAGE", "")
        if not alternate or not re.fullmatch(r"[^\s]+@sha256:[0-9a-f]{64}", alternate):
            raise ValueError("installation proof requires a genuine alternate frontend digest")
        variables = {"PIPING_INSTALL_TEST_FRONTEND_IMAGE": alternate,
                     "PIPING_RES_SMALL_CPUS": "0.25", "PIPING_RES_SMALL_MEM": "256M",
                     "PIPING_RES_MEDIUM_CPUS": "0.5", "PIPING_RES_MEDIUM_MEM": "512M",
                     "PIPING_RES_LARGE_CPUS": "1", "PIPING_RES_LARGE_MEM": "1G"}
        tests, deadline = (*HTTPS_TESTS, "tests.compose.test_install_lifecycle"), 900
    with daemon_client(image, archive, environment, variables) as (name, artifact_root):
        try:
            status = client_tests(name, tests, environment, deadline)
            print("https native archive_sha256=" + hashlib.sha256(archive).hexdigest() + " exit=" + str(status))
            return status
        finally:
            # HTTPS creates only temporary generated identities. No private
            # client output, .env or key is collected, including on failure.
            clean_client_artifacts(image, artifact_root, environment)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--client-unittest", nargs="+", help=argparse.SUPPRESS)
    parser.add_argument("--image", default=os.environ.get("CI_TOOLS_IMAGE"))
    parser.add_argument("--https", action="store_true", help="run only the committed installer/stock-nginx suite")
    parser.add_argument("--install", action="store_true", help="run committed TLS and full local install/update proofs")
    parser.add_argument("--fixture-working-tree", action="store_true")
    parser.add_argument("--export-source", type=Path)
    parser.add_argument("--proof-directory", type=Path)
    parser.add_argument("--artifact-inputs", type=Path,
                        help="private carried package, fixture signing/verification keys and installed23 archive; no image builds")
    parser.add_argument("--download-postgres-fixture", type=Path)
    parser.add_argument("--backend-binary", type=Path, default=os.environ.get("PIPING_NATIVE_BACKEND_BINARY"))
    parser.add_argument("--backend-sha256", default=os.environ.get("PIPING_NATIVE_BACKEND_SHA256"))
    parser.add_argument("--baseline", action="store_true",
                        help="Current committed-source baseline proof, distinct from pinned historical routes")
    parser.add_argument("--backend-source-sha", help="CI-verified source revision of the supplied backend artifact")
    args = parser.parse_args()
    if args.baseline and (args.fixture_working_tree or args.artifact_inputs or args.https or args.install
                          or args.download_postgres_fixture or args.client_unittest):
        parser.error("baseline requires committed current source without historical inputs or overlays")
    if args.backend_source_sha and not args.baseline:
        parser.error("backend-source-sha belongs only to the current-baseline route")
    if args.client_unittest:
        return run_client_unittest(args.client_unittest)
    if args.https or args.install:
        if args.https and args.install:
            parser.error("select either --https or --install")
        if args.fixture_working_tree or args.export_source or args.proof_directory or args.download_postgres_fixture or args.artifact_inputs:
            parser.error("HTTPS accepts only committed source and emits safe status, not fixture exports or private artifacts")
        if not args.image or args.image.startswith("-"):
            parser.error("--image or CI_TOOLS_IMAGE must identify the native client image")
        revision = os.environ.get("CI_COMMIT_SHA", "HEAD")
        archive = https_source_archive(ROOT, revision, install=args.install)
        return run_https(args.image, archive, dict(os.environ), install=args.install)
    if args.download_postgres_fixture:
        download_postgres_fixture(args.download_postgres_fixture, os.environ)
        return 0
    if args.fixture_working_tree and os.environ.get("CI"):
        parser.error("working-tree fixture export is local-only")
    if args.fixture_working_tree and not args.artifact_inputs:
        parser.error("the registered historical helper is immutable; overlays require current artifact mode")
    revision = (os.environ.get("CI_COMMIT_SHA", "HEAD") if args.baseline else
                ARTIFACT_HELPER_REVISION if args.artifact_inputs else HISTORICAL_HELPER_REVISION)
    source_args = {"application_revision": PRODUCER_APPLICATION_REVISION} if args.artifact_inputs else {}
    if args.export_source:
        target = Path(os.path.abspath(args.export_source))
        if any(parent.is_symlink() or not parent.is_dir() for parent in target.parents):
            parser.error("source export requires existing non-symlink ancestors")
        archive = (baseline_source_archive(ROOT, revision)[0] if args.baseline else
                   source_archive(ROOT, revision, args.fixture_working_tree, **source_args))
        with target.open("xb") as stream:
            stream.write(archive)
        return 0
    if not args.image or args.image.startswith("-"):
        parser.error("--image or CI_TOOLS_IMAGE must identify the native client image")
    if args.artifact_inputs and (not args.artifact_inputs.is_absolute() or args.artifact_inputs.is_symlink()
            or not args.artifact_inputs.is_dir() or args.artifact_inputs.stat().st_mode & 0o077
            or any(p.is_symlink() for p in args.artifact_inputs.parents)):
        parser.error("artifact inputs require a private absolute directory with non-symlink ancestors")
    if not args.backend_binary or not args.backend_binary.is_absolute() or args.backend_binary.is_symlink() \
            or not args.backend_binary.is_file() or not args.backend_sha256:
        parser.error("a regular absolute native backend executable and its SHA-256 are required")
    with args.backend_binary.open("rb") as source:
        if hashlib.file_digest(source, "sha256").hexdigest() != args.backend_sha256:
            parser.error("native backend executable identity mismatch")
    registry = os.environ.get("CI_REGISTRY", "")
    user = os.environ.get("CI_REGISTRY_USER", "")
    password = os.environ.get("CI_REGISTRY_PASSWORD", "")
    if os.environ.get("CI"):
        if not all((registry, user, password)):
            parser.error("CI_REGISTRY, CI_REGISTRY_USER and CI_REGISTRY_PASSWORD are required in CI")
        if args.image.split("/", 1)[0] != registry:
            parser.error("the native client image must belong to CI_REGISTRY")
    archive, baseline_source_sha = (baseline_source_archive(ROOT, revision) if args.baseline else
                                   (source_archive(ROOT, revision, args.fixture_working_tree, **source_args), None))
    if args.baseline and args.backend_source_sha != baseline_source_sha:
        parser.error("backend artifact source must equal the current committed-source export")
    with tarfile.open(fileobj=io.BytesIO(archive)) as bundle:
        migration_path = ("apps/api/migrations/0001_init.sql" if args.baseline else
                          "apps/api/migrations/0023_secret_field_lifecycle_management.sql")
        member = bundle.extractfile(migration_path)
        if member is None:
            parser.error("source-bound migration is required")
        migration_sha256 = hashlib.sha256(member.read()).hexdigest()
    environment = dict(os.environ)
    with tempfile.TemporaryDirectory(prefix="pwa-native-registry-") as auth:
        if os.environ.get("CI"):
            # Runner image-pull credentials do not authenticate the job's Docker
            # CLI. Keep its predefined job identity outside the source payload.
            environment["DOCKER_CONFIG"] = auth
            subprocess.run(
                ["docker", "login", "--username", user, "--password-stdin", registry],
                input=password.encode(), check=True, stdout=subprocess.DEVNULL,
                timeout=60, env=environment,
            )
        proof_args = {"proof_directory": args.proof_directory} if args.proof_directory else {}
        if args.artifact_inputs:
            proof_args["artifact_inputs"] = args.artifact_inputs
        if args.baseline:
            proof_args.update(baseline_source_sha=baseline_source_sha, baseline_sha256=migration_sha256)
        else:
            proof_args["migration23_sha256"] = migration_sha256
        return run_native(args.image, archive, environment, backend_binary=args.backend_binary,
                          backend_sha256=args.backend_sha256, **proof_args)


if __name__ == "__main__":
    raise SystemExit(main())
