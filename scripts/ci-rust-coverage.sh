#!/usr/bin/env bash
# Rust tests for CI (plain by explicit mode); coverage remains available on demand.
#
# `--include-ignored` is deliberate: it runs `#[ignore]`d tests. The three
# `--skip` filters below remove the live-infrastructure groups a CI runner
# cannot run. They match test names by substring, so beyond the groups named
# here they also stop other tests whose names share those fragments, for
# example inventory_search_queries_execute_against_live_schema and
# get_api_v1_live_returns_ok. The groups the filters exist for:
#   netbox  - every test in integration_netbox_round_trip_test.rs is
#             `#[ignore]`d and gated through `make netbox-test`, which brings
#             up the netbox compose profile and seeds 25 golden fixtures.
#   live_   - integration_device_drivers_live.rs needs real cameras via
#             PIPING_LIVE_<FAMILY>_{HOST,USER,PASS}. Before this filter they
#             ran under --include-ignored, self-skipped and were counted as
#             passes proving nothing (file header, lines 28-33).
#   e2e_rdp - manual_e2e_rdp_guacd.rs needs guacd on 127.0.0.1:4822 and a
#             reachable RDP target.
#
# --fail-under-lines 48 applies only to the on-demand coverage mode.
set -euo pipefail

case "${1:-coverage}" in
    test)
        test_command=(cargo test --workspace --all-features --locked -j1)
        ;;
    coverage)
        test_command=(cargo llvm-cov --workspace --all-features --locked --lcov
            --output-path apps/api/coverage/lcov.info --fail-under-lines 48)
        ;;
    *)
        echo "expected test or coverage mode" >&2
        exit 2
        ;;
esac

# The test job brings postgres and redis up as CI services. Wait until both
# answer from this container: postgres via pg_isready against the coverage
# database, redis via a TCP connect. Bounded at 45 s; on timeout print what was
# not ready and fail before cargo runs without its services.
test_services_ready=0
for _ in $(seq 1 45); do
    if pg_isready -h postgres -U piping -d pwa10_coverage >/dev/null 2>&1 \
        && nc -z redis 6379 >/dev/null 2>&1; then
        test_services_ready=1
        break
    fi
    sleep 1
done
if [ "$test_services_ready" -ne 1 ]; then
    echo "test dependencies did not become ready" >&2
    exit 1
fi

mkdir -p apps/api/coverage

# Environment is scoped to cargo, exactly as when this block was inline in CI.
POSTGRES_USER=piping \
POSTGRES_PASSWORD=piping-dev-password \
POSTGRES_DB=pwa10_coverage \
PIPING_DATABASE__URL="postgresql://piping:piping-dev-password@postgres:5432/pwa10_coverage" \
PIPING_TEST_DISPOSABLE_DB="1" \
PIPING_REDIS__URL="redis://:dev-redis-password@redis:6379/0" \
REDIS_URL="redis://:dev-redis-password@redis:6379/0" \
PIPING_SECRETS__CIPHER_KEY_B64="MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=" \
PIPING_AUTH__LOCAL_ENABLED="true" \
PIPING_AUTH__PASSWORD_PEPPER="test-pepper" \
CARGO_PROFILE_TEST_DEBUG="0" \
CARGO_PROFILE_DEV_DEBUG="0" \
CARGO_BUILD_JOBS="1" \
"${test_command[@]}" -- --include-ignored --test-threads=1 \
    --skip netbox --skip live_ --skip e2e_rdp
