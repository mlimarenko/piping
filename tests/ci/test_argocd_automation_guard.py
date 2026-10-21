"""What the deploy's Argo CD guard has to refuse.

The guard exists because a pushed release that Argo CD never applies is
indistinguishable from a successful deploy: the job goes green either way.
Each case below is a way automation has actually been lost — a block removed
by a merge, an operator's temporary `automated: null`, a file that no longer
holds the Application it is supposed to describe — and each must stop the
push rather than let it report success.

The guard script is byte-identical to the one Piping.Cloud runs; only the
Application name under test is Piping.App's (`piping-app`).
"""

from __future__ import annotations

import subprocess
import sys
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory

REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
GUARD = REPOSITORY_ROOT / "scripts" / "ci" / "assert_argocd_automation.py"

HEALTHY = """
apiVersion: argoproj.io/v1alpha1
kind: Application
metadata:
  name: piping-app
spec:
  syncPolicy:
    automated:
      prune: true
      selfHeal: true
"""


def write(directory: Path, name: str, body: str) -> Path:
    path = directory / name
    path.write_text(body, encoding="utf-8")
    return path


def run_guard(*applications: Path) -> subprocess.CompletedProcess[str]:
    """Drive the guard the way the deploy job does: by region directory.

    It used to take one literal file path per region, which is what broke it —
    the manifests repository moved its apps into subdirectories and the guard
    refused the deploy with "file is missing" about an Application that had
    simply moved. Each test now passes the directory its fixture sits in.
    """
    arguments = [sys.executable, str(GUARD), "--application-name", "piping-app"]
    for application in applications:
        arguments += ["--region-root", str(application.parent)]
    return subprocess.run(arguments, capture_output=True, text=True, check=False)


class ArgocdAutomationGuardTests(unittest.TestCase):
    def test_accepts_an_application_that_argo_cd_will_actually_apply(self) -> None:
        with TemporaryDirectory() as directory:
            path = write(Path(directory), "app.yaml", HEALTHY)
            result = run_guard(path)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_refuses_when_automation_was_removed(self) -> None:
        with TemporaryDirectory() as directory:
            path = write(
                Path(directory),
                "app.yaml",
                HEALTHY.replace(
                    "  syncPolicy:\n    automated:\n      prune: true\n      selfHeal: true\n",
                    "  syncPolicy:\n    syncOptions:\n      - CreateNamespace=true\n",
                ),
            )
            result = run_guard(path)
        self.assertEqual(result.returncode, 1)
        self.assertIn("no syncPolicy.automated", result.stderr)

    def test_refuses_the_temporary_null_an_operator_forgot_to_undo(self) -> None:
        with TemporaryDirectory() as directory:
            path = write(
                Path(directory),
                "app.yaml",
                HEALTHY.replace(
                    "    automated:\n      prune: true\n      selfHeal: true\n",
                    "    automated: null\n",
                ),
            )
            result = run_guard(path)
        self.assertEqual(result.returncode, 1)
        self.assertIn("no syncPolicy.automated", result.stderr)

    def test_refuses_automation_with_self_healing_switched_off(self) -> None:
        # Argo CD would apply the change when it noticed, but a cluster that
        # has drifted stays drifted — which is the state a GitOps push is
        # supposed to correct.
        with TemporaryDirectory() as directory:
            path = write(
                Path(directory), "app.yaml", HEALTHY.replace("selfHeal: true", "selfHeal: false")
            )
            result = run_guard(path)
        self.assertEqual(result.returncode, 1)
        self.assertIn("without selfHeal", result.stderr)

    def test_refuses_automation_with_self_healing_absent(self) -> None:
        # `automated:` with `prune` but no `selfHeal` key at all is the same
        # hazard as an explicit `false`; the guard checks for the true value,
        # not for the key's presence.
        with TemporaryDirectory() as directory:
            path = write(
                Path(directory),
                "app.yaml",
                HEALTHY.replace("      selfHeal: true\n", ""),
            )
            result = run_guard(path)
        self.assertEqual(result.returncode, 1)
        self.assertIn("without selfHeal", result.stderr)

    def test_refuses_an_absent_application_rather_than_passing_over_it(self) -> None:
        # Skipping would be most tempting and most wrong here: nothing would be
        # checked and the deploy would go on.
        with TemporaryDirectory() as directory:
            result = run_guard(Path(directory) / "absent.yaml")
        self.assertEqual(result.returncode, 1)
        self.assertIn("no Argo CD Application named", result.stderr)

    def test_finds_the_application_wherever_the_manifests_repository_puts_it(self) -> None:
        # The failure this replaced: the apps directory was reorganised into
        # subdirectories, the literal path went stale, and the first pipeline
        # to reach the deploy stage refused over a file that had only moved.
        with TemporaryDirectory() as directory:
            nested = Path(directory) / "services" / "regional"
            nested.mkdir(parents=True)
            write(nested, "piping-app.yaml", HEALTHY)
            result = run_guard(Path(directory) / "placeholder.yaml")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("services/regional/piping-app.yaml", result.stdout)

    def test_refuses_two_applications_with_the_same_name(self) -> None:
        # Two regions' manifests merged by mistake, or a copy left behind:
        # picking either one silently would check the wrong deploy.
        with TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "a").mkdir()
            (root / "b").mkdir()
            write(root / "a", "app.yaml", HEALTHY)
            write(root / "b", "app.yaml", HEALTHY)
            result = run_guard(root / "placeholder.yaml")
        self.assertEqual(result.returncode, 1)
        self.assertIn("2 Applications named", result.stderr)

    def test_refuses_a_file_that_is_not_one_application(self) -> None:
        with TemporaryDirectory() as directory:
            path = write(Path(directory), "app.yaml", HEALTHY + "---\nkind: ConfigMap\n")
            result = run_guard(path)
        self.assertEqual(result.returncode, 0, "an unrelated document alongside is fine")

        with TemporaryDirectory() as directory:
            path = write(Path(directory), "two.yaml", HEALTHY + "---" + HEALTHY)
            result = run_guard(path)
        self.assertEqual(result.returncode, 1)
        self.assertIn("expected exactly one", result.stderr)

    def test_every_region_is_checked_not_just_the_first(self) -> None:
        # Both clusters serve customers. A guard that stopped at the first
        # healthy file would let a broken RU deploy through whenever EU was
        # fine, which is the more likely order.
        with TemporaryDirectory() as directory:
            good = write(Path(directory), "eu.yaml", HEALTHY)
            bad = write(
                Path(directory), "ru.yaml", HEALTHY.replace("selfHeal: true", "selfHeal: false")
            )
            result = run_guard(good, bad)
        self.assertEqual(result.returncode, 1)
        self.assertIn("ru.yaml", result.stderr)


if __name__ == "__main__":
    unittest.main()
