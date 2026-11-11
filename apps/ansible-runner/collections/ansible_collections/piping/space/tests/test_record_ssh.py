"""Exercise the adapter through Ansible's actual loader and option resolution."""

import os
import json
from pathlib import Path
import subprocess
import tempfile
import unittest


COLLECTIONS = Path(__file__).resolve().parents[4]
BOOTSTRAP = Path(__file__).resolve().parents[5] / "docker" / "execute.py"


class RecordSSHTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.project = self.root / "project"
        self.project.mkdir()
        self.config = self.root / "ssh.conf"
        self.config.write_text("Host *\n  StrictHostKeyChecking yes\n")
        self.key = self.root / "target.key"
        self.key.touch()
        self.env = {k: v for k, v in os.environ.items() if not k.startswith("ANSIBLE_")}
        self.env["ANSIBLE_COLLECTIONS_PATH"] = str(COLLECTIONS)

    def tearDown(self):
        self.temp.cleanup()

    def run_play(self, *, inventory="127.0.0.1,", extra=(), play_connection=None):
        connection = f"  connection: {play_connection}\n" if play_connection else ""
        (self.project / "play.yml").write_text(
            "- hosts: all\n  gather_facts: false\n" + connection
            + "  tasks:\n    - ansible.builtin.ping:\n")
        command = ["python3", str(BOOTSTRAP), "--project", str(self.project),
                   "--home", str(self.root / "home"), "--record-ssh-config", str(self.config),
                   "--record-ssh-key-file", str(self.key), "--", "-i", inventory,
                   "-e", "ansible_port=1", "-vvv", *extra, "play.yml"]
        return subprocess.run(command, env=self.env, text=True, stdout=subprocess.PIPE,
                              stderr=subprocess.STDOUT, timeout=30)

    def test_default_and_native_identity(self):
        result = self.run_play(extra=("-e", "ansible_user=nativeuser"))
        self.assertEqual(result.returncode, 4, result.stdout)
        self.assertIn("-F " + str(self.config), result.stdout)
        self.assertIn(str(self.key), result.stdout)
        self.assertIn("Port=1", result.stdout)
        self.assertIn("nativeuser", result.stdout)
        self.assertIn("127.0.0.1", result.stdout)
        native_key = self.root / "native.key"
        native_key.touch()
        result = self.run_play(extra=("--private-key", str(native_key),))
        self.assertIn(str(native_key), result.stdout)
        self.assertNotIn(str(self.key), result.stdout)
        result = self.run_play(extra=("-e", json.dumps({"ansible_password": "synthetic-password"}),))
        self.assertIn("-F " + str(self.config), result.stdout)
        self.assertNotIn(str(self.key), result.stdout)

    def test_explicit_transport_opts_out(self):
        for kind in ("config", "env", "cli", "play", "inventory"):
            with self.subTest(kind=kind):
                self.env.pop("ANSIBLE_TRANSPORT", None)
                cfg = self.project / "ansible.cfg"
                if cfg.exists():
                    cfg.unlink()
                extra, play, inventory = (), None, "127.0.0.1,"
                if kind == "config":
                    cfg.write_text("[defaults]\ntransport = ssh\n")
                elif kind == "env":
                    self.env["ANSIBLE_TRANSPORT"] = "ssh"
                elif kind == "cli":
                    extra = ("-c", "ssh")
                elif kind == "play":
                    play = "ssh"
                else:
                    path = self.project / "inventory"
                    path.write_text("127.0.0.1 ansible_connection=ssh\n")
                    inventory = str(path)
                result = self.run_play(inventory=inventory, extra=extra, play_connection=play)
                self.assertEqual(result.returncode, 4, result.stdout)
                self.assertNotIn("-F " + str(self.config), result.stdout)

    def test_argument_validation_and_replacement(self):
        for args in ("-F /dev/null", "-J elsewhere", "-vF/dev/null", "-vJelsewhere", "-o ProxyCommand=none",
                     "-o StrictHostKeyChecking=no", "-o UserKnownHostsFile=/dev/null", "-o ControlPath=/tmp/socket"):
            with self.subTest(args=args):
                result = self.run_play(extra=("-e", json.dumps({"ansible_ssh_common_args": args})))
                self.assertIn("Conflicting SSH configuration or route", result.stdout)
        result = self.run_play(extra=("-e", json.dumps({"ansible_ssh_extra_args": "-vF/dev/null"})))
        self.assertIn("Conflicting SSH configuration or route", result.stdout)
        result = self.run_play(extra=("-e", json.dumps({"ansible_ssh_common_args": "-o ServerAliveInterval=20"})))
        self.assertIn("ServerAliveInterval=20", result.stdout)
        self.assertIn("-F " + str(self.config), result.stdout)
        result = self.run_play(extra=("-e", json.dumps({"ansible_ssh_common_args": "-i /native/identity"})))
        self.assertIn("/native/identity", result.stdout)
        self.assertNotIn(str(self.key), result.stdout)
        result = self.run_play(extra=("-e", "ansible_ssh_executable=/custom/ssh",))
        self.assertIn("Unsupported custom SSH executable", result.stdout)
        result = self.run_play(extra=("-e", "ansible_control_path=/tmp/native-control",))
        self.assertIn("does not support an explicit control path", result.stdout)
        result = self.run_play(extra=("-e", json.dumps({"ansible_ssh_extra_args": "-l nativeuser"})))
        self.assertIn("-l nativeuser", result.stdout)
        self.assertIn("ControlPath=", result.stdout)
        result = self.run_play(extra=("-e", "ansible_host=piping-native-0",))
        self.assertIn("reserved hop namespace", result.stdout)

    def test_command_specific_identity_does_not_replace_other_commands(self):
        for bucket in ("ansible_scp_extra_args", "ansible_sftp_extra_args"):
            with self.subTest(bucket=bucket):
                result = self.run_play(extra=("-e", json.dumps({bucket: "-i /native/transfer"})))
                commands = [line for line in result.stdout.splitlines() if "SSH: EXEC ssh " in line]
                self.assertTrue(commands, result.stdout)
                self.assertIn(str(self.key), commands[0])
                self.assertNotIn("/native/transfer", commands[0])


if __name__ == "__main__":
    unittest.main()
