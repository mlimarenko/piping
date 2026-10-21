//! DB-backed regression test for SSH host key pinning (PWA-14).
//!
//! The pin is trust-on-first-use: the first key observed for a
//! `(host, port, key_type)` is authoritative. A later observation of a
//! different key must not overwrite the stored line or fingerprint — that
//! would turn a detected key change into a silent re-trust. Recovery is the
//! deliberate operator action `delete_pins`, surfaced by
//! `mismatch_diagnosis`.
//!
//! This needs the real `ssh_host_key_pin` table and the real conflict action,
//! so it runs against a migrated PostgreSQL. The store is the only thing under
//! test; no SSH connection is made.

mod common;

use base64::Engine as _;
use piping_backend::contexts::shared::host_key_pins::{
    delete_pins, fetch_pins, mismatch_diagnosis, upsert_lines,
};

async fn migrated_pool() -> Result<sqlx::PgPool, Box<dyn std::error::Error + Send + Sync>> {
    let pool = common::build_test_db_pool().await?;
    piping_backend::storage::database::migrations::run_migrations(&pool)
        .await
        .map_err(|e| format!("run migrations: {e}"))?;
    Ok(pool)
}

/// A valid OpenSSH `ssh-ed25519` public key blob in wire format:
/// `string("ssh-ed25519") || string(32-byte public key)`. The 32 key bytes are
/// a fixed filler, not a secret; two different fillers give two distinct keys.
fn ed25519_blob(key_byte: u8) -> String {
    let mut wire = Vec::with_capacity(4 + 11 + 4 + 32);
    wire.extend_from_slice(&11u32.to_be_bytes());
    wire.extend_from_slice(b"ssh-ed25519");
    wire.extend_from_slice(&32u32.to_be_bytes());
    wire.extend_from_slice(&[key_byte; 32]);
    base64::engine::general_purpose::STANDARD.encode(wire)
}

#[tokio::test]
#[ignore = "requires PostgreSQL with PIPING_DATABASE__URL"]
async fn tofu_first_contact_pins_then_repeat_matches_and_changed_key_is_rejected()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let pool = migrated_pool().await?;

    let host = "pwa14-hostkey-test.example.org";
    let port: u16 = 2222;

    // A prior failed run may have left this endpoint pinned; the test owns it.
    let _ = delete_pins(&pool, host, port).await?;

    let blob_a = ed25519_blob(0x11);
    let blob_b = ed25519_blob(0x22);
    assert_ne!(blob_a, blob_b, "the two test keys must differ");
    let line_a = format!("[{host}]:{port} ssh-ed25519 {blob_a}");
    let line_b = format!("[{host}]:{port} ssh-ed25519 {blob_b}");

    // TOFU: first contact pins exactly the observed key.
    upsert_lines(&pool, std::slice::from_ref(&line_a)).await?;
    let pins = fetch_pins(&pool, host, port).await?;
    assert_eq!(pins.len(), 1, "first contact must pin exactly one key");
    let fingerprint_a = pins[0].fingerprint.clone();
    assert_eq!(pins[0].known_hosts_line, line_a);
    assert_eq!(pins[0].port, port);

    // Successful repeat: the same key is idempotent.
    upsert_lines(&pool, std::slice::from_ref(&line_a)).await?;
    let pins = fetch_pins(&pool, host, port).await?;
    assert_eq!(pins.len(), 1, "a repeat observation must not add a row");
    assert_eq!(pins[0].known_hosts_line, line_a);
    assert_eq!(pins[0].fingerprint, fingerprint_a);

    // Mismatch: a different key for the same host/port/key_type must leave the
    // first-seen pin untouched.
    upsert_lines(&pool, std::slice::from_ref(&line_b)).await?;
    let pins = fetch_pins(&pool, host, port).await?;
    assert_eq!(pins.len(), 1, "a changed key must not add a second row");
    assert_eq!(
        pins[0].known_hosts_line, line_a,
        "first-seen key must win over a later observation"
    );
    assert_eq!(
        pins[0].fingerprint, fingerprint_a,
        "a later observation must not overwrite the stored fingerprint"
    );

    // The diagnosis an operator sees names the endpoint, the stored pin and the
    // only sanctioned repair.
    let diagnosis = mismatch_diagnosis(host, port, &pins[0].fingerprint);
    let endpoint = format!("{host}:{port}");
    assert!(diagnosis.contains(endpoint.as_str()), "{diagnosis}");
    assert!(diagnosis.contains(fingerprint_a.as_str()), "{diagnosis}");
    assert!(diagnosis.contains("delete_pins"), "{diagnosis}");
    assert!(diagnosis.contains("reconnect"), "{diagnosis}");

    // Repair is the deliberate delete, and it clears the endpoint.
    let removed = delete_pins(&pool, host, port).await?;
    assert_eq!(removed, 1, "the pin must be deleted exactly once");
    assert!(
        fetch_pins(&pool, host, port).await?.is_empty(),
        "delete_pins must leave no pin behind"
    );
    Ok(())
}
