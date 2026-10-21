//! Integration tests for REST endpoints (health, ready, live, auth).
//!
//! Requires PostgreSQL and Redis (e.g. `make run-up`). Run with:
//! `cargo test --test integration_rest -- --ignored`

mod common;
#[path = "integration_rest/connection_session_credentials_tests.rs"]
mod connection_session_credentials_tests;
#[path = "integration_rest/license_tests.rs"]
mod license_tests;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::{Json, Router, extract::State, routing::post};
use chrono::Utc;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};
use tokio::sync::RwLock;
use tower::ServiceExt;

use common::{
    TEST_ADMIN_PASSWORD, TEST_ADMIN_USERNAME, TEST_PROVIDER_ID, TEST_SECRET_ID,
    TOKEN_SCOPE_CHECK_PATH, build_test_app_state, build_test_db_pool, login_and_get_cookie,
    request, request_with_auth, unique_suffix,
};
use piping_backend::contexts::secrets::application::SecretStore;
use uuid::Uuid;

const REST_TEST_NETBOX_BASE_URL: &str = "http://127.0.0.1:8000";
const REST_TEST_LICENSE_OUTAGE_URL: &str = "http://127.0.0.1:65534";
const REST_TEST_LOCAL_PASSWORD: &str = "Abcd1234";
const REST_TEST_NETBOX_PAGE_SIZE: u32 = 100;

const LICENSE_INSTANCE_ID: &str = "integration-license-instance";
const LICENSE_INSTANCE_A: &str = "integration-instance-a";
const LICENSE_MOCK_ORGANIZATION_ID: &str = "00000000-0000-7000-8000-000000000099";

const LICENSE_MAX_SEATS_COMMUNITY: i32 = 10;
const LICENSE_MAX_SEATS_SIGNED: i32 = 5;
const LICENSE_MAX_SEATS_CONCURRENT_BOUNDARY: i32 = 2;
const LICENSE_LOGIN_ATTEMPTS_AT_BOUNDARY: usize = 10;

const LICENSE_EXPIRY_DAYS_DEFAULT: i64 = 30;
const LICENSE_EXPIRY_DAYS_GRACE: i64 = 7;
const LICENSE_EXPIRED_MINUTES: i64 = 1;
const LICENSE_STALE_TOKEN_MINUTES: i64 = 5;
const LICENSE_TOKEN_VALIDITY_HOURS: i64 = 1;
const LICENSE_CLOCK_SKEW_SECONDS: i64 = 5;

const LICENSE_REFRESH_AFTER_SECONDS: u64 = 60;
const LICENSE_GRACE_FAILURE_THRESHOLD: u32 = 1;
const LICENSE_GRACE_PERIOD_SECONDS: u64 = 3600;
const LICENSE_GRACE_PERIOD_IMMEDIATE_SECONDS: u64 = 1;
const LICENSE_GRACE_END_WAIT_SECONDS: u64 = 2;

const LICENSE_ACTIVATE_PATH: &str = "/api/v1/license/activate";
const LICENSE_REFRESH_PATH: &str = "/api/v1/license/refresh";
const LICENSE_STATUS_PATH: &str = "/api/v1/license/state";
const MOCK_LICENSE_BIND_ADDR: &str = "127.0.0.1:0";

#[path = "integration_rest/admin_user_management_tests.rs"]
mod admin_user_management_tests;
#[path = "integration_rest/config_and_metadata_tests.rs"]
mod config_and_metadata_tests;
#[path = "integration_rest/core_and_auth_tests.rs"]
mod core_and_auth_tests;
#[path = "integration_rest/effective_config_tests.rs"]
mod effective_config_tests;
#[path = "integration_rest/inventory_and_provider_tests.rs"]
mod inventory_and_provider_tests;
#[path = "integration_rest/inventory_template_tests.rs"]
mod inventory_template_tests;
#[path = "integration_rest/monitoring_exporters_tests.rs"]
mod monitoring_exporters_tests;
#[path = "integration_rest/monitoring_sources_tests.rs"]
mod monitoring_sources_tests;
#[path = "integration_rest/playbook_batch_authorization_runtime_tests.rs"]
mod playbook_batch_authorization_runtime_tests;
#[path = "integration_rest/playbook_collections_contract_tests.rs"]
mod playbook_collections_contract_tests;
#[path = "integration_rest/registry_sort_acceptance_tests.rs"]
mod registry_sort_acceptance_tests;
#[path = "integration_rest/router_contract_tests.rs"]
mod router_contract_tests;
#[path = "integration_rest/settings_tests.rs"]
mod settings_tests;
#[path = "integration_rest/setup_wizard_tests.rs"]
mod setup_wizard_tests;
#[path = "integration_rest/status_contract_tests.rs"]
mod status_contract_tests;
#[path = "integration_rest/sync_and_security_tests.rs"]
mod sync_and_security_tests;
