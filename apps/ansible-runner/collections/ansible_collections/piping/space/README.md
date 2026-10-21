# piping.space

Local Ansible collection shipped inside the Piping Ansible runner image.

The collection is designed for playbooks that run on the controller (`hosts: localhost` or `delegate_to: localhost`) and receive runtime access through backend-provided environment variables:

- `PIPING_API_BASE_URL`
- `PIPING_API_TOKEN`
- optional explicitly granted secret env vars

Do not store API tokens or secret values in playbooks. Reference runtime env only.
