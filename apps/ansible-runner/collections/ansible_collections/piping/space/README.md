# piping.space

Local Ansible collection shipped inside the Piping Ansible runner image.

The controller modules and lookups run on the controller (`hosts: localhost` or
`delegate_to: localhost`) and receive runtime access through backend-provided
environment variables:

- `PIPING_API_BASE_URL`
- `PIPING_API_TOKEN`
- optional explicitly granted secret env vars

Do not store API tokens or secret values in playbooks. Reference runtime env only.

The `record_ssh` connection plugin applies delegated record defaults to stock
Ansible SSH without replacing native inventory or configuration. Selection,
credential precedence and pin requirements are documented in the
[runner contract](../../../../README.md#native-ssh-record-defaults).
