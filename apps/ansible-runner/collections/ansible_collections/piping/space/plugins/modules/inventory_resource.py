#!/usr/bin/python
"""Manage a Piping inventory resource through the run-bound job token."""

from __future__ import annotations

from ansible.module_utils.basic import AnsibleModule

from ansible_collections.piping.space.plugins.module_utils.piping_api import PipingApiClient, PipingApiError, inventory_path

DOCUMENTATION = r"""
---
module: inventory_resource
short_description: Read or write Piping inventory resources with the runtime job token.
options:
  resource:
    required: true
    type: str
  state:
    required: false
    type: str
    default: get
    choices: [get, create, update]
  id:
    required: false
    type: str
  payload:
    required: false
    type: dict
    default: {}
  query:
    required: false
    type: dict
    default: {}
"""


def main() -> None:
    module = AnsibleModule(
        argument_spec={
            "resource": {"type": "str", "required": True},
            "state": {"type": "str", "default": "get", "choices": ["get", "create", "update"]},
            "id": {"type": "str", "required": False, "default": None},
            "payload": {"type": "dict", "required": False, "default": {}},
            "query": {"type": "dict", "required": False, "default": {}},
        },
        supports_check_mode=True,
    )
    resource = module.params["resource"]
    state = module.params["state"]
    resource_id = module.params.get("id")
    payload = module.params.get("payload") or {}
    query = module.params.get("query") or {}

    if state == "update" and not resource_id:
        module.fail_json(msg="id is required when state=update")
    if state in {"create", "update"} and module.check_mode:
        module.exit_json(changed=True, skipped=True)

    try:
        client = PipingApiClient()
        if state == "get":
            result = client.request("GET", inventory_path(resource, resource_id), params=query)
            module.exit_json(changed=False, result=result)
        if state == "create":
            result = client.request("POST", inventory_path(resource), json_body=payload)
            module.exit_json(changed=True, result=result)
        result = client.request("PUT", inventory_path(resource, resource_id), json_body=payload)
        module.exit_json(changed=True, result=result)
    except PipingApiError as exc:
        module.fail_json(msg=str(exc))


if __name__ == "__main__":
    main()
