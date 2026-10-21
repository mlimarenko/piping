"""Lookup explicitly injected Piping runtime secret environment variables."""

from __future__ import annotations

import os
import re

from ansible.errors import AnsibleError
from ansible.plugins.lookup import LookupBase

_RESERVED_PREFIXES = ("PIPING_", "ANSIBLE_", "SSH_")
_RESERVED_NAMES = {"HOME", "PATH"}
_ENV_RE = re.compile(r"^[A-Z_][A-Z0-9_]*$")


class LookupModule(LookupBase):
    def run(self, terms, variables=None, **kwargs):  # noqa: D401, ANN001
        results = []
        for term in terms:
            name = str(term)
            if not _ENV_RE.match(name):
                raise AnsibleError("runtime_secret expects an env name matching [A-Z_][A-Z0-9_]*")
            if name in _RESERVED_NAMES or name.startswith(_RESERVED_PREFIXES):
                raise AnsibleError(f"runtime_secret refuses reserved env name {name}")
            value = os.environ.get(name)
            if value is None:
                raise AnsibleError(f"runtime secret env {name} is not available")
            results.append(value)
        return results
