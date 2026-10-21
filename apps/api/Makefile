SHELL := /bin/bash
COMPOSE_DEV := docker compose -f docker-compose.dev.yml

PIPING_DATABASE__URL ?= postgresql://piping:piping-dev-password@localhost:5432/piping
PIPING_REDIS__URL ?= redis://:dev-redis-password@localhost:6379/0
PIPING_SECRETS__CIPHER_KEY_B64 ?= MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=

.PHONY: build test run db-reset lint check ci

db-reset:
	$(COMPOSE_DEV) down -v
	$(COMPOSE_DEV) up -d

build:
	cargo build

lint:
	cargo fmt --check
	cargo clippy --workspace --all-targets -- -D warnings
	cargo-machete

check:
	cargo check

ci: lint check

# Runs the whole workspace against the dev stack. Beyond the unit/contract
# layers this exercises the database-backed guarantees: every OpenAPI path is
# mounted, critical mutation statuses, registry sort acceptance, batched
# playbook authorization and playbook collection pages.
test: run-up run-wait
	@PIPING_DATABASE__URL="$(PIPING_DATABASE__URL)" \
	PIPING_REDIS__URL="$(PIPING_REDIS__URL)" \
	REDIS_URL="$(PIPING_REDIS__URL)" \
	PIPING_SECRETS__CIPHER_KEY_B64="$(PIPING_SECRETS__CIPHER_KEY_B64)" \
	PIPING_AUTH__LOCAL_ENABLED=true \
	PIPING_AUTH__PASSWORD_PEPPER=test-pepper \
	cargo test --workspace -j1 -- --include-ignored --test-threads=1

run: run-up run-wait
	PIPING_DATABASE__URL="$(PIPING_DATABASE__URL)" \
	PIPING_REDIS__URL="$(PIPING_REDIS__URL)" \
	PIPING_SECRETS__CIPHER_KEY_B64="$(PIPING_SECRETS__CIPHER_KEY_B64)" \
	cargo run

run-up:
	$(COMPOSE_DEV) up -d

run-wait:
	@echo "Waiting for Postgres and Redis..."
	@for i in $$(seq 1 45); do \
	  if $(COMPOSE_DEV) exec -T postgres pg_isready -U piping -d piping >/dev/null 2>&1 && \
	     $(COMPOSE_DEV) exec -T redis redis-cli -a dev-redis-password ping 2>/dev/null | grep -q PONG; then \
	    echo "Postgres and Redis are ready."; \
	    exit 0; \
	  fi; \
	  sleep 1; \
	done; \
	echo "Timeout waiting for Postgres/Redis."; exit 1
