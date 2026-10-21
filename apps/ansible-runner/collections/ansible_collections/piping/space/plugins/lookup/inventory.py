"""Lookup Piping inventory resources through the run-bound job token."""

from __future__ import annotations

from ansible.errors import AnsibleError
from ansible.plugins.lookup import LookupBase

from ansible_collections.piping.space.plugins.module_utils.piping_api import PipingApiClient, PipingApiError, inventory_path


class LookupModule(LookupBase):
    def run(self, terms, variables=None, **kwargs):  # noqa: D401, ANN001
        params = kwargs.get("query") or kwargs.get("params")
        resource_id = kwargs.get("id")
        try:
            client = PipingApiClient()
            return [client.request("GET", inventory_path(str(term), resource_id), params=params) for term in terms]
        except PipingApiError as exc:
            raise AnsibleError(str(exc)) from exc
