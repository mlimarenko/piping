SHELL := /bin/bash

DEFAULT_GOAL := help

COMPOSE := docker compose -f docker-compose.yml --env-file env/dev.env
BACKEND_DIR := apps/api
FRONTEND_DIR := apps/web
BACKEND_MAKE := $(MAKE) -C $(BACKEND_DIR)

.PHONY: help up up-build deploy down down-volumes logs ps restart \
	netbox-up netbox-down netbox-wait \
	netbox-seed netbox-test netbox-reset keycloak-up keycloak-down keycloak-smoke \
	keycloak-bootstrap backend-check backend-test frontend-install frontend-lint \
	frontend-test frontend-build frontend-dev licensing-up licensing-cloud-up licensing-e2e contract-bundle \
	contract-drift rebaseline-sqlx-0001 check install-hooks uninstall-hooks

help:
	@echo "Available targets:"
	@echo "  make up                  - Start stack"
	@echo "  make up-build            - Build and start stack"
	@echo "  make deploy              - Install or start the local Compose stack"
	@echo "  make rebaseline-sqlx-0001 - Offline complete-24 transition (requires REBASELINE_ARGS confirmations)"
	@echo "  make down                - Stop stack"
	@echo "  make down-volumes        - Stop stack and remove volumes"
	@echo "  make logs                - Follow stack logs"
	@echo "  make ps                  - Show stack status"
	@echo "  make restart             - Restart stack"
	@echo ""
	@echo "  make netbox-up           - Start temporary NetBox profile"
	@echo "  make netbox-down         - Stop NetBox profile"
	@echo "  make netbox-seed         - Seed deterministic NetBox dataset"
	@echo "  make netbox-test         - Run NetBox integration tests against local stack"
	@echo "  make netbox-reset        - Stop NetBox profile and remove NetBox volumes"
	@echo ""
	@echo "  make keycloak-up         - Start temporary Keycloak profile"
	@echo "  make keycloak-down       - Stop Keycloak profile"
	@echo "  make keycloak-smoke      - Validate Keycloak health endpoint through nginx"
	@echo "  make keycloak-bootstrap  - Provision Keycloak realm/client/groups/test users"
	@echo ""
	@echo "  make backend-check       - Run cargo check on $(BACKEND_DIR)"
	@echo "  make backend-test        - Run backend test suite"
	@echo "  make frontend-install    - pnpm install in $(FRONTEND_DIR)"
	@echo "  make frontend-dev        - Run Vite dev server in $(FRONTEND_DIR)"
	@echo "  make frontend-lint       - Run frontend lint"
	@echo "  make frontend-test       - Run frontend unit tests"
	@echo "  make frontend-build      - Build the frontend app"
	@echo ""
	@echo "  make licensing-e2e       - Run licensing end-to-end lifecycle scenario"
	@echo "  make licensing-up        - Start an isolated licensing fixture"
	@echo "  make licensing-cloud-up  - Start an explicitly selected local Cloud fixture"
	@echo "  make contract-bundle     - Validate/bundle the OpenAPI contract via redocly"
	@echo "  make contract-drift      - Run contract-drift tests"
	@echo "  make check               - Combined gate (backend + frontend + contract)"
	@echo ""
	@echo "  make install-hooks       - Opt-in: wire .pre-commit-config.yaml into git hooks (run once)"
	@echo "  make uninstall-hooks     - Remove the git hooks installed by install-hooks"

up:
	@$(COMPOSE) up -d

up-build:
	@$(COMPOSE) up -d --build

deploy:
	@./install.sh $(INSTALL_ARGS)

rebaseline-sqlx-0001:
	@bash scripts/rebaseline-sqlx-0001.sh $(REBASELINE_ARGS) apply-local-compose

down:
	@$(COMPOSE) down

down-volumes:
	@$(COMPOSE) down -v

logs:
	@$(COMPOSE) logs -f

ps:
	@$(COMPOSE) ps

restart: down up

netbox-up:
	@$(COMPOSE) --profile netbox up -d netbox-postgres netbox-redis netbox netbox-worker
	@$(MAKE) netbox-wait

netbox-down:
	@$(COMPOSE) --profile netbox stop netbox-worker netbox netbox-postgres netbox-redis || true
	@$(COMPOSE) --profile netbox rm -f netbox-worker netbox netbox-postgres netbox-redis || true

netbox-wait:
	@bash scripts/netbox-wait.sh

netbox-seed: netbox-up
	@python3 scripts/netbox-seed.py

netbox-test: netbox-seed
	@cd $(BACKEND_DIR) && \
	PIPING_DATABASE__URL="$${PIPING_DATABASE__URL:-postgresql://piping:piping-dev-password@localhost:$${PIPING_POSTGRES_PORT:-5432}/piping}" \
	PIPING_REDIS__URL="$${PIPING_REDIS__URL:-redis://:dev-redis-password@localhost:$${PIPING_REDIS_PORT:-6379}/0}" \
	REDIS_URL="$${REDIS_URL:-redis://:dev-redis-password@localhost:$${PIPING_REDIS_PORT:-6379}/0}" \
	PIPING_SECRETS__CIPHER_KEY_B64="$${PIPING_SECRETS__CIPHER_KEY_B64:-MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=}" \
	PIPING_AUTH__LOCAL_ENABLED=true \
	PIPING_AUTH__PASSWORD_PEPPER="$${PIPING_AUTH__PASSWORD_PEPPER:-dev-pepper}" \
	NETBOX_URL="$${NETBOX_URL:-http://localhost:$${NETBOX_PORT:-28080}}" \
	NETBOX_API_TOKEN="$${NETBOX_API_TOKEN:-netbox-admin-token}" \
	cargo test --test integration_provider_sources -- --ignored --test-threads=1 && \
	cargo test --test integration_netbox_source_flow -- --ignored --test-threads=1 && \
	cargo test --test integration_netbox_webhook_security -- --ignored --test-threads=1 && \
	cargo test --test integration_netbox_round_trip_test -- --ignored --test-threads=1 && \
	cargo test --test integration_transform_round_trip -- --ignored --test-threads=1

netbox-reset: netbox-down
	@docker volume rm \
		"$${COMPOSE_PROJECT_NAME:-piping}"_netbox_postgres_data \
		"$${COMPOSE_PROJECT_NAME:-piping}"_netbox_redis_data \
		"$${COMPOSE_PROJECT_NAME:-piping}"_netbox_media >/dev/null 2>&1 || true

keycloak-up:
	@$(COMPOSE) --profile keycloak up -d keycloak-postgres keycloak

keycloak-down:
	@$(COMPOSE) --profile keycloak stop keycloak keycloak-postgres || true
	@$(COMPOSE) --profile keycloak rm -f keycloak keycloak-postgres || true

keycloak-smoke:
	@bash scripts/keycloak-smoke.sh

keycloak-bootstrap:
	@bash scripts/keycloak-bootstrap.sh

backend-check:
	@cd $(BACKEND_DIR) && cargo check

backend-test:
	@$(BACKEND_MAKE) test

frontend-install:
	@cd $(FRONTEND_DIR) && pnpm install

frontend-dev:
	@cd $(FRONTEND_DIR) && pnpm run dev

frontend-lint:
	@cd $(FRONTEND_DIR) && pnpm run lint

frontend-test:
	@cd $(FRONTEND_DIR) && pnpm run test

frontend-build:
	@cd $(FRONTEND_DIR) && pnpm run build

licensing-up:
	@source scripts/license-e2e-fixture.sh; \
	project="$${PIPING_LICENSE_E2E_PROJECT:-piping-license-e2e}"; \
	license_fixture_app_project "$$project"; \
	license_fixture_network "$${PIPING_LICENSE_E2E_CLOUD_PROJECT:?Set the isolated Cloud fixture project}"; \
	license_fixture_volume "$$project" postgres_data true; \
	license_fixture_volume "$$project" redis_data true; \
	docker compose --env-file env/dev.env -p "$$project" -f docker-compose.yml -f tests/compose/docker-compose.licensing.yml up -d backend

licensing-cloud-up:
	@source scripts/license-e2e-fixture.sh; \
	project="$${PIPING_LICENSE_E2E_CLOUD_PROJECT:?Set an isolated Cloud fixture project}"; \
	license_fixture_cloud_project "$$project"; \
	license_fixture_network "$$project" true; \
	license_fixture_volume "$$project" postgres-data true; \
	file="$${PIPING_LICENSE_E2E_CLOUD_COMPOSE:?Set the absolute Cloud Compose path}"; \
	[[ "$$file" == /* && -f "$$file" && ! -L "$$file" ]] || exit 1; \
	PIPING_SPACE_POSTGRES_DB=piping_license_e2e_cloud docker compose -p "$$project" -f "$$file" -f "$$PWD/tests/compose/docker-compose.licensing-cloud.yml" --profile billing up -d backend billing-sim

licensing-e2e:
	@bash scripts/license-e2e-lifecycle.sh

contract-bundle:
	@pnpm --package=@redocly/cli@latest dlx redocly lint \
		$(BACKEND_DIR)/contracts/piping.openapi.yaml

contract-drift:
	@cd $(BACKEND_DIR) && cargo test --test contract_drift

check: backend-check backend-test frontend-lint frontend-test contract-drift
	@echo "✓ All quality gates passed"

# Install the repository's staged frontend checks once per clone.
install-hooks:
	@command -v pre-commit >/dev/null 2>&1 || { \
		echo "✗ pre-commit is not installed. Install it first:"; \
		echo "    pip install pre-commit        # or: pipx install pre-commit"; \
		exit 1; \
	}
	@pre-commit install --install-hooks
	@echo "✓ Git hooks installed. Staged apps/web changes now run eslint --max-warnings=0 + typecheck before each commit."

uninstall-hooks:
	@command -v pre-commit >/dev/null 2>&1 || { \
		echo "✗ pre-commit is not installed; nothing to uninstall."; \
		exit 0; \
	}
	@pre-commit uninstall
	@echo "✓ Git hooks removed."
