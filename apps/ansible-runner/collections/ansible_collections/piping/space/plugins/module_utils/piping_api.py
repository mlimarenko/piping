"""Shared Piping API client for the local ``piping.space`` collection."""

from __future__ import annotations

import os
from typing import Any

import requests


class PipingApiError(RuntimeError):
    """Raised for controlled Piping API failures."""


class PipingApiClient:
    """Tiny env-backed client for run-bound Piping REST access."""

    def __init__(self, base_url: str | None = None, token: str | None = None, timeout: float = 30.0) -> None:
        self.base_url = (base_url or os.environ.get("PIPING_API_BASE_URL") or "").rstrip("/")
        self.token = token or os.environ.get("PIPING_API_TOKEN") or ""
        self.timeout = timeout
        if not self.base_url:
            raise PipingApiError("PIPING_API_BASE_URL is not available for this playbook")
        if not self.token:
            raise PipingApiError("PIPING_API_TOKEN is not available for this playbook")

    def request(self, method: str, path: str, *, json_body: Any | None = None, params: dict[str, Any] | None = None) -> Any:
        method = method.upper()
        if method not in {"GET", "POST", "PUT", "PATCH"}:
            raise PipingApiError(f"Unsupported Piping API method: {method}")
        url = f"{self.base_url}/{path.lstrip('/')}"
        headers = {"Authorization": f"Bearer {self.token}", "Accept": "application/json"}
        if json_body is not None:
            headers["Content-Type"] = "application/json"
        try:
            response = requests.request(
                method,
                url,
                headers=headers,
                json=json_body,
                params=params,
                timeout=self.timeout,
            )
        except requests.RequestException as exc:
            raise PipingApiError(f"Piping API request failed: {type(exc).__name__}: {exc}") from exc
        if response.status_code >= 400:
            if response.status_code in {401, 403}:
                detail = "authentication failed"
            else:
                detail = response.text[:500]
            raise PipingApiError(f"Piping API {method} {path} returned HTTP {response.status_code}: {detail}")
        if not response.content:
            return None
        content_type = response.headers.get("content-type", "")
        if "application/json" not in content_type:
            return response.text
        return response.json()


def inventory_path(resource: str, resource_id: str | None = None) -> str:
    resource = resource.strip().strip("/")
    if not resource:
        raise PipingApiError("inventory resource path is required")
    path = f"/api/v1/inventory/{resource}"
    if resource_id:
        path = f"{path}/{resource_id.strip()}"
    return path
