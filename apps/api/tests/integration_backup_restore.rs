//! DB-backed regression coverage for the destructive backup/restore surface.
//!
//! Requires PostgreSQL and Redis. Run serially because full restore truncates
//! every application table:
//! `cargo test -p piping-backend --test integration_backup_restore -- --ignored --test-threads=1`

mod common;

#[path = "backup_restore/engine_tests.rs"]
mod engine_tests;
#[path = "backup_restore/route_tests.rs"]
mod route_tests;
#[path = "backup_restore/support.rs"]
mod support;
