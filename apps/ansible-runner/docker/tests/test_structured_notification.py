"""Structured Ansible notification behaviour: spool, seal and exec frames."""

import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import sys
import tempfile
import types
import unittest

DOCKER = Path(__file__).resolve().parents[1]
EXECUTE_SOURCE = DOCKER / "execute.py"
BOOTSTRAP_SOURCE = DOCKER / "kube_bootstrap.py"
PLUGIN_SOURCE = DOCKER / "callback_plugins" / "piping_space_structured.py"


def load(name, source):
    spec = importlib.util.spec_from_file_location(name, source)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


try:  # The interpreter running the tests may not have ansible-core installed.
    import ansible.plugins.callback  # noqa: F401
except ImportError:
    class _CallbackBase:
        """Minimal stand-in for the Ansible callback base."""

        def __init__(self, *args, **kwargs):
            pass

    _callback = types.ModuleType("ansible.plugins.callback")
    _callback.CallbackBase = _CallbackBase
    sys.modules.setdefault("ansible", types.ModuleType("ansible"))
    sys.modules.setdefault("ansible.plugins", types.ModuleType("ansible.plugins"))
    sys.modules.setdefault("ansible.plugins.callback", _callback)


class _Named:
    def __init__(self, name):
        self._name = name

    def get_name(self):
        return self._name


class _Result:
    def __init__(self, host, task, values=None):
        self._host = _Named(host)
        self._task = _Named(task)
        self._result = values if values is not None else {}


class _Stats:
    def __init__(self, summaries):
        self.processed = dict.fromkeys(summaries, 1)
        self._summaries = summaries

    def summarize(self, host):
        return dict(self._summaries[host])


class _Capture:
    """Text stream with the binary buffer the bootstrap forwards into."""

    def __init__(self):
        self.buffer = io.BytesIO()

    def write(self, text):
        self.buffer.write(text.encode("utf-8", "replace"))

    def flush(self):
        pass

    def value(self):
        return self.buffer.getvalue()


SUMMARY = {"ok": 0, "failures": 0, "unreachable": 0, "changed": 0,
           "skipped": 0, "rescued": 0, "ignored": 0}


class PluginTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.previous = os.environ.get("PIPING_NOTIFY_DIR")
        os.environ["PIPING_NOTIFY_DIR"] = str(self.root)
        self.plugin = load("piping_space_structured_under_test", PLUGIN_SOURCE)

    def tearDown(self):
        if self.previous is None:
            os.environ.pop("PIPING_NOTIFY_DIR", None)
        else:
            os.environ["PIPING_NOTIFY_DIR"] = self.previous
        self.temp.cleanup()

    def records(self, name):
        path = self.root / name
        return [json.loads(line) for line in path.read_bytes().splitlines()]

    def test_stats_write_one_line_per_processed_host(self):
        self.plugin.CallbackModule().v2_playbook_on_stats(_Stats({
            "web-1": {"ok": 4, "failures": 1, "unreachable": 0, "changed": 2,
                      "skipped": 3, "rescued": 1, "ignored": 1},
            "db-1": {"ok": 1, "failures": 0, "unreachable": 2, "changed": 0,
                     "skipped": 0, "rescued": 0, "ignored": 0},
        }))
        self.assertEqual(self.records("hosts.ndjson"), [
            {"host": "db-1", "ok": 1, "changed": 0, "unreachable": 2, "failed": 0,
             "skipped": 0, "rescued": 0, "ignored": 0},
            {"host": "web-1", "ok": 4, "changed": 2, "unreachable": 0, "failed": 1,
             "skipped": 3, "rescued": 1, "ignored": 1},
        ])
        self.assertEqual(stat.S_IMODE((self.root / "hosts.ndjson").stat().st_mode), 0o600)
        self.assertFalse((self.root / "tasks.ndjson").exists(), "streams open lazily")

    def test_task_records_carry_no_result_content(self):
        callback = self.plugin.CallbackModule()
        secret = {"msg": "TOPSECRET", "stdout": "TOPSECRET", "diff": ["TOPSECRET"],
                  "changed": False, "invocation": {"module_args": {"password": "TOPSECRET"}}}
        callback.v2_runner_on_ok(_Result("web-1", "install", dict(secret)))
        callback.v2_runner_on_ok(_Result("web-1", "configure", {"changed": True}))
        callback.v2_runner_on_failed(_Result("web-1", "break", dict(secret)))
        callback.v2_runner_on_failed(_Result("web-1", "tolerate", dict(secret)), ignore_errors=True)
        callback.v2_runner_on_unreachable(_Result("web-2", "probe", dict(secret)))
        callback.v2_runner_on_skipped(_Result("web-2", "optional", dict(secret)))
        self.assertEqual(self.records("tasks.ndjson"), [
            {"host": "web-1", "task": "install", "result": "ok"},
            {"host": "web-1", "task": "configure", "result": "changed"},
            {"host": "web-1", "task": "break", "result": "failed"},
            {"host": "web-1", "task": "tolerate", "result": "ignored"},
            {"host": "web-2", "task": "probe", "result": "unreachable"},
            {"host": "web-2", "task": "optional", "result": "skipped"},
        ])
        self.assertNotIn(b"TOPSECRET", (self.root / "tasks.ndjson").read_bytes())
        self.assertFalse((self.root / "hosts.ndjson").exists(), "task events open no host stream")

    def test_task_name_is_truncated_to_two_hundred_characters(self):
        name = "x" * (self.plugin.TASK_NAME_LIMIT + 300)
        self.plugin.CallbackModule().v2_runner_on_ok(_Result("web-1", name))
        record = self.records("tasks.ndjson")[0]
        self.assertEqual(len(record["task"]), self.plugin.TASK_NAME_LIMIT)

    def test_task_cap_stops_writing_and_records_truncation(self):
        callback = self.plugin.CallbackModule()
        name = "x" * self.plugin.TASK_NAME_LIMIT
        for _ in range(2000):
            callback.v2_runner_on_ok(_Result("web-1", name))
        data = (self.root / "tasks.ndjson").read_bytes()
        self.assertLessEqual(len(data), self.plugin.TASKS_LIMIT)
        self.assertEqual(json.loads(data.splitlines()[-1]), {"kind": "truncated"})
        callback.v2_runner_on_ok(_Result("web-1", name))
        self.assertEqual((self.root / "tasks.ndjson").read_bytes(), data)

    def test_host_cap_stops_writing_and_records_truncation(self):
        callback = self.plugin.CallbackModule()
        hosts = {f"host-{index:05d}-{'y' * 120}": dict(SUMMARY) for index in range(1000)}
        callback.v2_playbook_on_stats(_Stats(hosts))
        data = (self.root / "hosts.ndjson").read_bytes()
        self.assertLessEqual(len(data), self.plugin.HOSTS_LIMIT)
        self.assertEqual(json.loads(data.splitlines()[-1]), {"kind": "truncated"})

    def test_existing_entries_disable_the_stream_silently(self):
        target = self.root / "target.ndjson"
        target.write_text("untouched\n")
        (self.root / "hosts.ndjson").symlink_to(target)
        (self.root / "tasks.ndjson").write_text("untouched\n")
        callback = self.plugin.CallbackModule()
        callback.v2_playbook_on_stats(_Stats({"web-1": dict(SUMMARY)}))
        callback.v2_runner_on_ok(_Result("web-1", "install"))
        self.assertEqual(target.read_text(), "untouched\n")
        self.assertEqual((self.root / "tasks.ndjson").read_text(), "untouched\n")

    def test_missing_directory_is_silent(self):
        for directory in (None, str(self.root / "absent")):
            if directory is None:
                os.environ.pop("PIPING_NOTIFY_DIR")
            else:
                os.environ["PIPING_NOTIFY_DIR"] = directory
            callback = self.plugin.CallbackModule()
            callback.v2_playbook_on_stats(_Stats({"web-1": dict(SUMMARY)}))
            callback.v2_runner_on_ok(_Result("web-1", "install"))
        self.assertEqual(list(self.root.iterdir()), [])


class SealTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.notify = Path(self.temp.name)
        self.execute = load("execute_under_test", EXECUTE_SOURCE)
        self.previous = self.execute.NOTIFY_DIRECTORY
        self.execute.NOTIFY_DIRECTORY = str(self.notify)

    def tearDown(self):
        self.execute.NOTIFY_DIRECTORY = self.previous
        self.temp.cleanup()

    def seal(self):
        stdout, stderr = _Capture(), _Capture()
        saved = sys.stdout, sys.stderr
        try:
            sys.stdout, sys.stderr = stdout, stderr
            self.execute.seal_spool()
        finally:
            sys.stdout, sys.stderr = saved
        self.assertEqual(stderr.value(), b"")
        self.assertEqual(stdout.value(), b"")
        return self.notify / "result.ndjson"

    def test_canonical_result_keeps_valid_records_and_marks_the_rest(self):
        (self.notify / "hosts.ndjson").write_bytes(
            b'{"host":"web-1","ok":2,"changed":1,"unreachable":0,"failed":0,'
            b'"skipped":0,"rescued":0,"ignored":0}\n'
            b'not json\n'
            b'{"host":"web-2","ok":-1,"changed":0,"unreachable":0,"failed":0,'
            b'"skipped":0,"rescued":0,"ignored":0}\n'
            b'{"host":"web-3","ok":1,"changed":0,"unreachable":0,"failed":0,'
            b'"skipped":0,"rescued":0,"ignored":0,"extra":"dropped"}\n')
        (self.notify / "tasks.ndjson").write_bytes(
            b'{"host":"web-1","task":"install","result":"ok"}\n'
            b'{"host":"web-1","task":"install","result":"bogus"}\n'
            b'{"kind":"truncated"}\n')
        path = self.seal()
        self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600)
        self.assertEqual(path.read_bytes().decode().splitlines(), [
            '{"kind":"host","host":"web-1","ok":2,"changed":1,"unreachable":0,'
            '"failed":0,"skipped":0,"rescued":0,"ignored":0}',
            '{"kind":"host","host":"web-3","ok":1,"changed":0,"unreachable":0,'
            '"failed":0,"skipped":0,"rescued":0,"ignored":0}',
            '{"kind":"task","host":"web-1","task":"install","result":"ok"}',
            '{"kind":"truncated"}',
        ])

    def test_absent_spools_create_no_result(self):
        self.seal()
        self.assertFalse((self.notify / "result.ndjson").exists())

    def test_oversized_line_is_dropped_and_marked(self):
        (self.notify / "tasks.ndjson").write_bytes(
            b'{"host":"web-1","task":"' + b"y" * 4000 + b'","result":"ok"}\n'
            b'{"host":"web-1","task":"install","result":"changed"}\n')
        lines = self.seal().read_bytes().splitlines()
        self.assertEqual([json.loads(line) for line in lines], [
            {"kind": "task", "host": "web-1", "task": "install", "result": "changed"},
            {"kind": "truncated"},
        ])

    def test_total_input_overflow_is_bounded_and_marked(self):
        task = "z" * 1900
        line = json.dumps({"host": "web-1", "task": task, "result": "ok"},
                          separators=(",", ":")).encode() + b"\n"
        (self.notify / "tasks.ndjson").write_bytes(line * 200)
        payload = self.seal().read_bytes()
        self.assertLessEqual(len(payload), 256 * 1024)
        self.assertEqual(json.loads(payload.splitlines()[-1]), {"kind": "truncated"})

    def test_unsafe_spool_symlink_is_not_followed(self):
        secret = self.notify.parent / "secret.txt"
        secret.write_text('{"host":"web-1","task":"leak","result":"ok"}\n')
        (self.notify / "hosts.ndjson").symlink_to(secret)
        lines = self.seal().read_bytes().decode().splitlines()
        self.assertEqual(lines, ['{"kind":"truncated"}'])

    def test_existing_result_is_never_replaced(self):
        (self.notify / "hosts.ndjson").write_bytes(
            b'{"host":"web-1","ok":1,"changed":0,"unreachable":0,"failed":0,'
            b'"skipped":0,"rescued":0,"ignored":0}\n')
        (self.notify / "result.ndjson").write_text("previous\n")
        self.seal()
        self.assertEqual((self.notify / "result.ndjson").read_text(), "previous\n")


class ExecuteBootstrapTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.project = self.root / "project"
        self.project.mkdir()
        (self.project / "play.yml").write_text("- hosts: localhost\n  gather_facts: false\n")
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.notify = self.root / "notify"
        self.callbacks = self.root / "callbacks"
        self.record = self.root / "record.json"
        self.galaxy = self.root / "galaxy.json"
        self.execute = load("execute_bootstrap_under_test", EXECUTE_SOURCE)
        self.previous = self.execute.NOTIFY_DIRECTORY, self.execute.CALLBACK_DIRECTORY
        self.execute.NOTIFY_DIRECTORY = str(self.notify)
        self.execute.CALLBACK_DIRECTORY = str(self.callbacks)

    def tearDown(self):
        self.execute.NOTIFY_DIRECTORY, self.execute.CALLBACK_DIRECTORY = self.previous
        self.temp.cleanup()

    def fake(self, name, body):
        path = self.bin / name
        path.write_text("#!/usr/bin/env python3\n" + body)
        path.chmod(0o755)

    def configure(self, entries, code=0):
        dump = self.root / "dump.json"
        dump.write_text(json.dumps(entries))
        self.fake("ansible-config",
                  "import sys\n"
                  f"sys.stdout.write(open({str(dump)!r}).read())\n"
                  f"sys.exit({code})\n")

    def playbook(self, code=0, spools=None):
        written = ""
        if spools:
            hosts, tasks = spools
            written = (f"open(os.path.join(os.environ['PIPING_NOTIFY_DIR'], 'hosts.ndjson'), 'w').write({hosts!r})\n"
                       f"open(os.path.join(os.environ['PIPING_NOTIFY_DIR'], 'tasks.ndjson'), 'w').write({tasks!r})\n")
        recorder = (
            "import json, os, sys\n"
            "recorded = {'argv': sys.argv[1:],\n"
            "            'environment': {key: value for key, value in os.environ.items()\n"
            "                            if key.startswith(('ANSIBLE_', 'PIPING_'))}}\n"
            f"open({str(self.record)!r}, 'w').write(json.dumps(recorded))\n")
        self.fake("ansible-playbook", recorder + written + f"sys.exit({code})\n")
        self.fake("ansible-galaxy",
                  f"import json, sys\nopen({str(self.galaxy)!r}, 'w').write(json.dumps(sys.argv[1:]))\n")

    def run_main(self, arguments):
        environment = {key: value for key, value in os.environ.items()
                       if not key.startswith(("ANSIBLE_", "PIPING_"))}
        environment["PATH"] = f"{self.bin}:{environment.get('PATH', '')}"
        stdout, stderr = _Capture(), _Capture()
        saved = sys.argv, sys.stdout, sys.stderr, dict(os.environ)
        try:
            sys.argv = ["execute.py", "--project", str(self.project),
                        "--home", str(self.root / "home"), *arguments]
            sys.stdout, sys.stderr = stdout, stderr
            os.environ.clear()
            os.environ.update(environment)
            code = self.execute.main()
        finally:
            sys.argv, sys.stdout, sys.stderr = saved[0], saved[1], saved[2]
            os.environ.clear()
            os.environ.update(saved[3])
        return code, stdout.value(), stderr.value()

    def test_effective_callback_configuration_is_extended_not_replaced(self):
        self.configure([
            {"name": "DEFAULT_CALLBACK_PLUGIN_PATH", "origin": "default",
             "value": [str(self.callbacks), "/opt/callbacks"]},
            {"name": "CALLBACKS_ENABLED", "origin": "default",
             "value": ["community.general.profile_tasks", "piping_space_structured"]},
            {"name": "DEFAULT_STDOUT_CALLBACK", "origin": "default", "value": "community.general.yaml"},
            {"name": "DEFAULT_ROLES_PATH", "origin": "default", "value": ["/opt/roles"]},
            {"name": "COLLECTIONS_PATHS", "origin": "default", "value": ["/opt/collections"]},
        ])
        self.playbook()
        code, _, stderr = self.run_main(["--", "play.yml"])
        self.assertEqual(code, 0, stderr)
        record = json.loads(self.record.read_text())
        self.assertEqual(record["argv"], ["play.yml"])
        self.assertEqual(record["environment"]["ANSIBLE_CALLBACK_PLUGINS"],
                         f"{self.callbacks}:/opt/callbacks")
        self.assertEqual(record["environment"]["ANSIBLE_CALLBACKS_ENABLED"],
                         "community.general.profile_tasks,piping_space_structured")
        self.assertEqual(record["environment"]["PIPING_NOTIFY_DIR"], str(self.notify))
        self.assertNotIn("ANSIBLE_STDOUT_CALLBACK", record["environment"])
        self.assertNotIn("--callback", record["argv"])
        self.assertNotIn("--stdout-callback", record["argv"])
        self.assertEqual(sorted(path.name for path in self.notify.iterdir()), [])
        self.assertTrue(self.notify.is_dir())
        self.assertEqual(stat.S_IMODE(self.notify.stat().st_mode), 0o700)

    def test_unavailable_dump_without_requirements_still_runs_ansible(self):
        self.configure([], code=1)
        self.playbook()
        code, _, stderr = self.run_main(["--", "play.yml"])
        self.assertEqual(code, 0, stderr)
        record = json.loads(self.record.read_text())
        self.assertEqual(record["argv"], ["play.yml"])
        self.assertNotIn("ANSIBLE_CALLBACK_PLUGINS", record["environment"])
        self.assertNotIn("ANSIBLE_CALLBACKS_ENABLED", record["environment"])
        self.assertEqual(record["environment"]["PIPING_NOTIFY_DIR"], str(self.notify))
        self.assertFalse((self.notify / "result.ndjson").exists())

    def test_unavailable_dump_with_requirements_stops_before_ansible(self):
        requirements = self.project / "requirements.yml"
        requirements.write_text("- src: example.role\n")
        self.configure([], code=1)
        self.playbook()
        with self.assertRaises(subprocess.CalledProcessError):
            self.run_main(["--requirements", "requirements.yml", "--", "play.yml"])
        self.assertFalse(self.record.exists())

    def test_stale_spool_files_are_removed_before_ansible_starts(self):
        self.notify.mkdir(mode=0o700)
        for name in ("hosts.ndjson", "tasks.ndjson", "result.ndjson"):
            (self.notify / name).write_text("stale\n")
        self.configure([
            {"name": "DEFAULT_CALLBACK_PLUGIN_PATH", "origin": "default", "value": []},
            {"name": "CALLBACKS_ENABLED", "origin": "default", "value": []},
        ])
        self.playbook()
        code, _, stderr = self.run_main(["--", "play.yml"])
        self.assertEqual(code, 0, stderr)
        # A surviving stale record would be sealed; the empty directory proves
        # it was dropped before Ansible started.
        self.assertEqual(sorted(path.name for path in self.notify.iterdir()), [])

    def test_playbook_failure_still_seals_the_spool(self):
        self.configure([
            {"name": "DEFAULT_CALLBACK_PLUGIN_PATH", "origin": "default", "value": []},
            {"name": "CALLBACKS_ENABLED", "origin": "default", "value": []},
        ])
        host = ('{"host":"web-1","ok":1,"changed":0,"unreachable":0,"failed":0,'
                '"skipped":0,"rescued":0,"ignored":0}\n')
        self.playbook(code=2, spools=(host, ""))
        code, _, stderr = self.run_main(["--", "play.yml"])
        self.assertEqual(code, 2, stderr)
        self.assertEqual((self.notify / "result.ndjson").read_text(),
                         '{"kind":"host","host":"web-1","ok":1,"changed":0,'
                         '"unreachable":0,"failed":0,"skipped":0,"rescued":0,"ignored":0}\n')

    def test_unsafe_notify_directory_is_replaced(self):
        target = self.root / "elsewhere"
        target.mkdir()
        self.notify.symlink_to(target)
        environment = {}
        self.assertTrue(self.execute.prepare_spool(environment))
        self.assertEqual(environment["PIPING_NOTIFY_DIR"], str(self.notify))
        self.assertFalse(self.notify.is_symlink())
        self.assertEqual(stat.S_IMODE(self.notify.stat().st_mode), 0o700)
        self.assertEqual(list(target.iterdir()), [])


class NotifyFrameTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name) / "notify"
        self.root.mkdir(mode=0o700)
        self.bootstrap = load("kube_bootstrap_under_test", BOOTSTRAP_SOURCE)

    def tearDown(self):
        self.temp.cleanup()

    def emit(self, directory=None):
        stream = io.StringIO()
        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            self.bootstrap.emit_notify(stream, directory or self.root)
        self.assertEqual(stderr.getvalue(), "")
        return stream.getvalue()

    def frames(self, output):
        return [json.loads(line) for line in output.splitlines()]

    def test_payload_is_split_into_bounded_notify_frames(self):
        payload = bytes(range(256)) * 40
        (self.root / "result.ndjson").write_bytes(payload)
        frames = self.frames(self.emit())
        self.assertEqual(len(frames), 3)
        self.assertEqual({frame["stream"] for frame in frames}, {"notify"})
        self.assertTrue(all(len(frame["bytes"]) <= 4096 for frame in frames))
        self.assertTrue(all(isinstance(value, int) and 0 <= value < 256
                            for frame in frames for value in frame["bytes"]))
        self.assertEqual(bytes(value for frame in frames for value in frame["bytes"]), payload)

    def test_frames_go_to_stdout_only(self):
        (self.root / "result.ndjson").write_bytes(b'{"kind":"host","host":"web-1"}\n')
        stdout, stderr = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            self.bootstrap.emit_notify(sys.stdout, self.root)
        self.assertEqual(stderr.getvalue(), "")
        frames = self.frames(stdout.getvalue())
        self.assertEqual(len(frames), 1)
        self.assertEqual(frames[0]["stream"], "notify")

    def test_symlinked_directory_yields_no_frame(self):
        real = Path(self.temp.name) / "real"
        real.mkdir()
        (real / "result.ndjson").write_bytes(b'{"kind":"host","host":"web-1"}\n')
        link = Path(self.temp.name) / "link"
        link.symlink_to(real)
        self.assertEqual(self.emit(link), "")

    def test_symlinked_record_yields_no_frame(self):
        secret = Path(self.temp.name) / "secret"
        secret.write_bytes(b'{"kind":"host","host":"web-1"}\n')
        (self.root / "result.ndjson").symlink_to(secret)
        self.assertEqual(self.emit(), "")

    def test_oversized_record_yields_no_frame(self):
        (self.root / "result.ndjson").write_bytes(b"{\"kind\":\"host\"}\n" * 20000)
        self.assertEqual(self.emit(), "")

    def test_special_file_yields_no_frame(self):
        os.mkfifo(self.root / "result.ndjson")
        self.assertEqual(self.emit(), "")

    def test_absent_or_empty_record_yields_no_frame(self):
        self.assertEqual(self.emit(), "")
        (self.root / "result.ndjson").write_bytes(b"")
        self.assertEqual(self.emit(), "")


@unittest.skipUnless(shutil.which("ansible-playbook"), "ansible-core is not installed")
class EndToEndTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.project = self.root / "project"
        self.project.mkdir()
        self.callbacks = self.root / "callbacks"
        self.callbacks.mkdir()
        shutil.copy(PLUGIN_SOURCE, self.callbacks / "piping_space_structured.py")
        self.notify = self.root / "notify"
        (self.project / "play.yml").write_text(
            "- hosts: localhost\n"
            "  gather_facts: false\n"
            "  tasks:\n"
            "    - ansible.builtin.debug: msg=hidden\n"
            "    - ansible.builtin.command: /bin/true\n"
            "    - ansible.builtin.debug: msg=skipped\n"
            "      when: false\n"
            "    - ansible.builtin.fail: msg=hidden\n"
            "      ignore_errors: true\n")
        self.execute = load("execute_end_to_end", EXECUTE_SOURCE)
        self.previous = self.execute.NOTIFY_DIRECTORY, self.execute.CALLBACK_DIRECTORY
        self.execute.NOTIFY_DIRECTORY = str(self.notify)
        self.execute.CALLBACK_DIRECTORY = str(self.callbacks)

    def tearDown(self):
        self.execute.NOTIFY_DIRECTORY, self.execute.CALLBACK_DIRECTORY = self.previous
        self.temp.cleanup()

    def test_native_console_is_preserved_and_records_are_sealed(self):
        environment = {key: value for key, value in os.environ.items()
                       if not key.startswith(("ANSIBLE_", "PIPING_"))}
        stdout, stderr = _Capture(), _Capture()
        saved = sys.argv, sys.stdout, sys.stderr, dict(os.environ)
        try:
            sys.argv = ["execute.py", "--project", str(self.project),
                        "--home", str(self.root / "home"), "--", "play.yml"]
            sys.stdout, sys.stderr = stdout, stderr
            os.environ.clear()
            os.environ.update(environment)
            code = self.execute.main()
        finally:
            sys.argv, sys.stdout, sys.stderr = saved[0], saved[1], saved[2]
            os.environ.clear()
            os.environ.update(saved[3])
        console = stdout.value()
        self.assertEqual(code, 0, console + stderr.value())
        self.assertIn(b"PLAY RECAP", console)
        self.assertNotIn(b'"plays"', console)
        self.assertNotIn(b'"stats"', console)
        result = (self.notify / "result.ndjson").read_bytes()
        self.assertEqual(stat.S_IMODE((self.notify / "result.ndjson").stat().st_mode), 0o600)
        records = [json.loads(line) for line in result.splitlines()]
        hosts = [record for record in records if record["kind"] == "host"]
        self.assertEqual(len(hosts), 1)
        self.assertEqual(hosts[0]["host"], "localhost")
        self.assertGreaterEqual(hosts[0]["ok"], 2)
        self.assertEqual(hosts[0]["changed"], 1)
        self.assertEqual(hosts[0]["skipped"], 1)
        self.assertEqual(hosts[0]["ignored"], 1)
        self.assertEqual(hosts[0]["failed"], 0)
        self.assertEqual(hosts[0]["unreachable"], 0)
        self.assertEqual(hosts[0]["rescued"], 0)
        self.assertEqual(records[0]["kind"], "host")
        tasks = [record for record in records if record["kind"] == "task"]
        self.assertEqual(
            {(record["task"], record["result"]) for record in tasks},
            {("ansible.builtin.debug", "ok"), ("ansible.builtin.command", "changed"),
             ("ansible.builtin.debug", "skipped"), ("ansible.builtin.fail", "ignored")})
        self.assertNotIn(b"hidden", result)


if __name__ == "__main__":
    unittest.main()
