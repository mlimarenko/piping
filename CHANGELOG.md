# Changelog

## 1.0.0

### Added

- Infrastructure inventory for devices, virtual machines, sites and IP addresses.
- Browser-based SSH, RDP and VNC sessions with role and policy controls.
- Ansible playbook execution against inventory targets, NetBox synchronization
  and Prometheus-compatible service discovery.
- Scoped MCP and API access, including explicitly permitted SSH actions with
  the configured infrastructure identity and audit log.
- A self-hosted installer and public container images.

### Installation and upgrades

The installer requires Linux/amd64 and Docker Compose. The initial self-signed
certificate needs browser trust. Preserve the installation's private `.env`
file and data: rerunning the installer cannot recover lost secrets and does not
automatically upgrade images. Explicit updates use the guarded backup and
migration flow. Existing plaintext FHRP keys require the documented preparation
and recovery procedure, not ordinary startup.
