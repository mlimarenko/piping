# Contributing to Piping

Piping accepts code contributions. Read the [license](LICENSE) and [CLA](CLA.md)
before submitting your work.

Report bugs, discuss changes and open pull requests in
[GitHub](https://github.com/mlimarenko/piping). Use `main` as the base for your work.

## Run the UI demo

Requires Node.js 26 and pnpm compatible with `apps/web/pnpm-lock.yaml`.

```sh
cd apps/web
pnpm install --frozen-lockfile
pnpm dev:demo
```

Open http://localhost:5174/demo-app/?lang=en. This is the real frontend with
synthetic inventory and session fixtures; it needs no backend or credentials.
Demo changes reset on reload.

## Develop against the API

The backend needs Docker, Python 3.12+ and the Rust toolchain specified in
`Cargo.toml`. Start the API and its local PostgreSQL/Redis services from the
repository root:

```sh
make -C apps/api run
```

In another terminal, run `pnpm dev` from `apps/web` after installing its
dependencies. The UI serves on http://localhost:5173 and proxies `/api/v1/*`
to port 7000. Local sign-in is at `/auth/login` with the `admin` account.
Configure its password with `PIPING_AUTH__LOCAL_ADMIN_PASSWORD`; never commit
or share credential values. Weak development secrets are for local use only.

Backend defaults live in `apps/api/config/default.toml`, local overrides in
`apps/api/config/local.toml`, and environment overrides use `PIPING_*` names
with `__` between nested keys.

## Checks and review

Run the relevant checks for your change; `make check` is the combined local gate.
Frontend checks run from `apps/web`:

```sh
pnpm lint
pnpm typecheck
pnpm test
pnpm build
```

`make install-hooks` installs the repository hooks. Include behavior tests for
code changes and keep one canonical implementation, without compatibility shims
or parallel paths. API changes must update the backend, OpenAPI contract,
frontend clients, tests and documentation together. Generate the contract with
`cargo run -p piping-backend --bin dump-openapi`.

Submit only work you own or have permission to contribute. Do not remove or
bypass licensing enforcement. The pull request description must confirm:
"I agree to the CLA".
