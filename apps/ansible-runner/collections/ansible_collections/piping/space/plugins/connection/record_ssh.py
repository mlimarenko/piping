"""Apply delegated SSH defaults after Ansible resolves native SSH options."""

import hashlib
import json
import os
import shlex

from ansible.errors import AnsibleError
from ansible.plugins.connection.ssh import Connection as SSHConnection, DOCUMENTATION


_BLOCKED_OPTIONS = frozenset({
    "proxycommand", "proxyjump", "canonicalizehostname", "hostname", "hostkeyalias",
    "stricthostkeychecking", "userknownhostsfile", "globalknownhostsfile",
    "knownhostscommand", "checkhostip", "verifyhostkeydns", "updatehostkeys",
    "hostkeyalgorithms", "casignaturealgorithms", "revokedhostkeys",
    "proxyusefdpass", "controlpath",
})


def _inspect_args(value, option):
    try:
        words = shlex.split(value or "")
    except ValueError as exc:
        raise AnsibleError("Invalid SSH arguments in " + option) from exc
    identity = False
    index = 0
    while index < len(words):
        word = words[index]
        if word == "--" or not word.startswith("-") or word == "-":
            raise AnsibleError("Unsupported SSH arguments in " + option)
        flags = word[1:]
        while flags and flags[0] in "46Cvq":
            flags = flags[1:]
        if not flags:
            index += 1
            continue
        flag = flags[0]
        if flag in "FJWS":
            raise AnsibleError("Conflicting SSH configuration or route in " + option)
        if flag in "io":
            argument = flags[1:]
            if not argument:
                index += 1
                if index >= len(words):
                    raise AnsibleError("Invalid SSH arguments in " + option)
                argument = words[index]
            if flag == "i":
                identity = True
            else:
                key = argument.split("=", 1)[0].split(None, 1)[0].lower()
                if key in _BLOCKED_OPTIONS:
                    raise AnsibleError("Conflicting SSH configuration or route in " + option)
                identity |= key == "identityfile"
        elif flag in "bcEl" or (flag == "p" and option == "ssh_extra_args"):
            # Only flags with the same known arity for their applicable commands.
            if len(flags) == 1:
                index += 1
                if index >= len(words):
                    raise AnsibleError("Invalid SSH arguments in " + option)
        elif flags == "p" and option == "scp_extra_args":
            pass
        else:
            raise AnsibleError("Unsupported SSH arguments in " + option)
        index += 1
    return identity


class Connection(SSHConnection):
    def set_options(self, task_keys=None, var_options=None, direct=None):
        super().set_options(task_keys=task_keys, var_options=var_options, direct=direct)
        config = os.environ.get("PIPING_RECORD_SSH_CONFIG")
        key = os.environ.get("PIPING_RECORD_SSH_KEY_FILE")
        if not config or not key or not os.path.isabs(config) or not os.path.isabs(key):
            raise AnsibleError("Record SSH defaults require absolute config and key paths")
        host = self.get_option("host")
        if not host:
            raise AnsibleError("Record SSH requires a native destination")
        if host and str(host).lower().startswith("piping-native-"):
            raise AnsibleError("Native SSH destination uses the reserved hop namespace")
        for name, binary in (("ssh_executable", "ssh"), ("scp_executable", "scp"),
                             ("sftp_executable", "sftp")):
            if self.get_option(name) not in (binary, "/usr/bin/" + binary):
                raise AnsibleError("Unsupported custom SSH executable: " + name)
        if not self.get_option("host_key_checking"):
            raise AnsibleError("Record SSH requires host key checking")
        if self.get_option("control_path"):
            raise AnsibleError("Record SSH does not support an explicit control path")
        identities = {name: _inspect_args(self.get_option(name), name) for name in
                      ("ssh_args", "ssh_common_args", "ssh_extra_args", "scp_extra_args", "sftp_extra_args")}
        # Stock Ansible's automatic socket hash omits the transport. Keep its
        # directory and multiplexing, but isolate this bundle from direct SSH.
        socket_inputs = [host, self.get_option("port"), self.get_option("remote_user"), config]
        socket_inputs += [self.get_option(name) for name in identities]
        digest = hashlib.sha256(json.dumps(socket_inputs, ensure_ascii=True).encode()).hexdigest()[:16]
        self.set_option("control_path", "%(directory)s/pr-" + digest)
        common = self.get_option("ssh_common_args") or ""
        self.set_option("ssh_common_args", common + " -F " + shlex.quote(config))
        if (not identities["ssh_args"] and not identities["ssh_common_args"]
                and not self.get_option("private_key") and not self.get_option("private_key_file")
                and not self.get_option("password")):
            specific = ("ssh_extra_args", "scp_extra_args", "sftp_extra_args")
            if any(identities[name] for name in specific):
                for name in specific:
                    if not identities[name]:
                        self.set_option(name, (self.get_option(name) or "") + " -i " + shlex.quote(key))
            else:
                self.set_option("private_key_file", key)
