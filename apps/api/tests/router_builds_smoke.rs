//! Smoke gate: constructing the REST router must not panic.
//!
//! The contract/parity gates only read `router.rs` as text or serialize the
//! OpenAPI doc; none of them build the axum `Router`, so a `matchit` route
//! conflict (e.g. a param segment colliding with static siblings) is a startup
//! panic that is otherwise invisible to CI. This test exercises the route-table
//! construction directly (no `AppState`/DB needed — conflicts surface at
//! `.route()` insertion).

use piping_backend::api::rest::router::build_rest_router;

#[test]
fn rest_router_constructs_without_route_conflicts() {
    let _router = build_rest_router();
}
