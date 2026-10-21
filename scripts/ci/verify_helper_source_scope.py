#!/usr/bin/env python3
"""Restrict source CI to deployment helpers, installer, publisher and direct consumers."""

import hashlib
import os
from pathlib import Path
import re
import subprocess
import sys


# This reviewed list classifies source ownership, not the semantics of an edit.
# In particular, README and CI edits still require independent source review.
ALLOWED_PATHS = frozenset({
    "install.sh",
    "Makefile",
    "docker-compose.yml",
    "docker-compose.host-network.yml",
    "docker-compose.licensing.yml",
    "docker/nginx/README.md",
    "docker/nginx/nginx.conf",
    "docker/nginx/proxy-common.conf",
    "docker/nginx/locations/assets.conf",
    "docker/nginx/locations/backend.conf",
    "docker/nginx/locations/keycloak.conf",
    "docker/nginx/entrypoint.d/10-init-ssl.sh",
    "env/dev.env",
    "env/images.env",
    "scripts/deploy_release_bundle.py",
    "scripts/ci/apply_release_bundle.py",
    "scripts/ci/publish_public_release.py",
    "scripts/ci/select_public_release.py",
    "scripts/ci/github_public_release.py",
    "scripts/license-e2e-fixture.sh",
    "scripts/license-e2e-lifecycle.sh",
    "scripts/license-outage-sim.sh",
    "scripts/public_release.py",
    "scripts/ci/compose_delivery_contract.py",
    "scripts/ci/postgres_migration_contract.py",
    "scripts/rebaseline-sqlx-0001.sh",
    "scripts/rebaseline_sqlx.py",
    "tests/ci/test_sqlx_rebaseline.py",
    "scripts/ci-compose-native.py",
    "tests/ci/test_deploy_compose_release.py",
    "tests/ci/test_deploy_release_bundle.py",
    "tests/ci/test_public_release.py",
    "tests/ci/test_github_public_release.py",
    "tests/ci/test_license_fixture.py",
    "tests/ci/test_public_artifacts.py",
    "tests/ci/test_compose_image_contract.py",
    "tests/ci/test_native_fixture_transport.py",
    "tests/ci/test_postgres_migration_contract.py",
    "tests/compose/test_deploy_release_bundle.py",
    "tests/compose/test_install_https.py",
    "tests/compose/test_install_lifecycle.py",
    "tests/compose/docker-compose.licensing.yml",
    "tests/compose/docker-compose.licensing-cloud.yml",
    "README.md",
    "CONTRIBUTING.md",
    "contrib/demo/piping.gif",
    "apps/desktop/README.md",
    "apps/desktop/SIGNING.md",
    ".github/workflows/release-docker-images.yml",
    ".gitlab-ci.yml",
    "scripts/ci/helper-source.yml",
    "scripts/ci/public-release.yml",
    "scripts/ci/install-source.yml",
    "scripts/ci/verify_helper_source_scope.py",
    "tests/ci/test_helper_source_scope.py",
})

# One-time relocation of the reviewed BASE product route, excluding the common
# root defaults/runtime include. Any subsequent product-config edit is forbidden.
PRODUCT_ROUTE_SHA256 = "655a4730f693369c2b1af950b9389692b1f32b01525e697f1c512a6b61f2e4e7"


class ScopeError(ValueError):
    pass


def git(repo, *args):
    result = subprocess.run(
        ["git", "-C", str(repo), *args], capture_output=True, check=False,
    )
    if result.returncode:
        # Do not include git diagnostics: a configured URL can contain credentials.
        raise ScopeError("required Git history or reference is unavailable")
    return result.stdout


def commit(repo, value):
    if not re.fullmatch(r"[0-9a-f]{40}", value or "") or value == "0" * 40:
        raise ScopeError("a complete nonzero commit SHA is required")
    resolved = git(repo, "rev-parse", "--verify", value + "^{commit}").decode().strip()
    if resolved != value:
        raise ScopeError("commit SHA does not resolve exactly")
    return value


def verify(repo, env):
    if git(repo, "rev-parse", "--is-shallow-repository").strip() != b"false":
        raise ScopeError("full Git history is required")
    head = commit(repo, env.get("CI_COMMIT_SHA"))
    if git(repo, "rev-parse", "HEAD").decode().strip() != head:
        raise ScopeError("checkout does not match CI_COMMIT_SHA")
    default = env.get("CI_DEFAULT_BRANCH", "")
    if not default or default.startswith("-"):
        raise ScopeError("trusted default branch is required")
    git(repo, "check-ref-format", "refs/heads/" + default)
    source = env.get("CI_PIPELINE_SOURCE")
    if source == "push" and env.get("CI_COMMIT_BRANCH") == default:
        if env.get("CI_COMMIT_REF_PROTECTED") != "true":
            raise ScopeError("default push must be protected")
        base = commit(repo, env.get("CI_COMMIT_BEFORE_SHA"))
        git(repo, "merge-base", "--is-ancestor", base, head)
    elif source == "merge_request_event" or (
        source == "push" and env.get("CI_COMMIT_BRANCH")
        and env.get("CI_COMMIT_BRANCH") != default and not env.get("CI_COMMIT_TAG")
    ):
        if source == "merge_request_event" and (
            env.get("CI_MERGE_REQUEST_TARGET_BRANCH_NAME") != default
            or env.get("CI_MERGE_REQUEST_TARGET_BRANCH_PROTECTED") != "true"
        ):
            raise ScopeError("MR must target the protected default branch")
        # Only the runner's origin default ref is trusted, not user-supplied bases.
        target = git(repo, "rev-parse", "--verify", "refs/remotes/origin/" + default + "^{commit}").decode().strip()
        commit(repo, target)
        bases = git(repo, "merge-base", "--all", target, head).decode().splitlines()
        if len(bases) != 1:
            raise ScopeError("a unique merge base is required")
        base = commit(repo, bases[0])
    else:
        raise ScopeError("unsupported helper pipeline source")

    # Disabling rename detection exposes BOTH paths of renames/copies, so an
    # unlisted product deletion cannot hide behind a helper destination name.
    fields = git(repo, "diff", "--raw", "-z", "--no-renames", "--no-ext-diff", "--no-textconv", "--ignore-submodules=none", base, head, "--").split(b"\0")
    changes = []
    for index in range(0, len(fields) - 1, 2):
        metadata, path_bytes = fields[index:index + 2]
        path = path_bytes.decode("utf-8", errors="strict")
        old_mode, new_mode, _, _, status = metadata.decode("ascii").split()
        old_mode = old_mode.removeprefix(":")
        if path == "scripts/ci/product-release.yml" and status == "A" and new_mode == "100644":
            content = git(repo, "show", head + ":" + path)
            if hashlib.sha256(content).hexdigest() != PRODUCT_ROUTE_SHA256:
                raise ScopeError("product route relocation differs from reviewed BASE")
        elif path not in ALLOWED_PATHS:
            raise ScopeError("diff contains a path outside reviewed helper ownership")
        if old_mode not in {"000000", "100644", "100755"} or new_mode not in {"000000", "100644", "100755"}:
            raise ScopeError("symlinks, submodules and nonregular files are forbidden")
        if status not in {"A", "D", "M"}:
            raise ScopeError("unsupported diff status")
        if status == "M" and old_mode != new_mode:
            raise ScopeError("file mode changes require product-route review")
        changes.append(path)
    if not changes:
        raise ScopeError("empty helper diff cannot authorize a helper pipeline")
    return base, head, len(changes)


def main():
    try:
        env = os.environ
        default = env.get("CI_DEFAULT_BRANCH", "")
        if env.get("CI_PIPELINE_SOURCE") == "merge_request_event" or (
            env.get("CI_PIPELINE_SOURCE") == "push" and env.get("CI_COMMIT_BRANCH") != default
        ):
            if not default or default.startswith("-"):
                raise ScopeError("trusted default branch is required")
            git(Path.cwd(), "check-ref-format", "refs/heads/" + default)
            git(Path.cwd(), "fetch", "--no-tags", "origin", "+refs/heads/" + default + ":refs/remotes/origin/" + default)
        base, head, count = verify(Path.cwd(), os.environ)
    except (ScopeError, UnicodeError, ValueError) as error:
        print(f"helper scope rejected: {error}", file=sys.stderr)
        return 1
    print(f"helper scope accepted: base={base} source={head} paths={count}; semantic source review remains required")
    return 0


if __name__ == "__main__":
    sys.exit(main())
