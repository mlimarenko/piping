//! DB-backed regression test for the `bulk_create` partial-success contract.
//!
//! Every inventory `bulk_create` returns `{ created, errors }` and the REST
//! layer surfaces both, so a caller is entitled to read `created` as "these
//! rows are now in the database".
//!
//! That contract was silently broken: the implementations ran every insert in
//! one flat transaction and caught failures per row. PostgreSQL aborts the
//! whole transaction block on the first failed statement, so the closing
//! COMMIT degraded into a ROLLBACK and discarded the rows that had already
//! succeeded — while still returning them in `created`.
//!
//! The bug is invisible to `cargo check` and to any test that does not hit a
//! real database, which is why it survived. This reproduces exactly the shape
//! that broke: a batch whose FIRST row is valid and whose SECOND row is
//! rejected. It fails against the flat-transaction implementation and passes
//! against the per-row SAVEPOINT one.
//!
//! `tenancy_contact` is the subject because it needs no seeded prerequisites —
//! it has no required foreign keys — so the test runs against a freshly
//! migrated, empty database. It also inserts TWO rows per item (the contact and
//! its group link), which is the only bulk path where one item spans more than
//! one statement, so it covers the per-item atomicity boundary as well.

mod common;

use piping_backend::contexts::inventory::infrastructure::persistence::repositories::tenancy::contact_repository::{
    ContactRepository, NewContact, SqlxContactRepository,
};
use uuid::Uuid;

async fn migrated_pool() -> Result<sqlx::PgPool, Box<dyn std::error::Error + Send + Sync>> {
    let pool = common::build_test_db_pool().await?;
    piping_backend::storage::database::migrations::run_migrations(&pool)
        .await
        .map_err(|e| format!("run migrations: {e}"))?;
    Ok(pool)
}

fn contact(id: Uuid, name: String, group_id: Option<Uuid>) -> NewContact {
    NewContact {
        id,
        name,
        group_id,
        title: None,
        phone: None,
        email: None,
        address: None,
        link: None,
        description: None,
    }
}

/// Everything `bulk_create` reports as created must survive the commit.
#[tokio::test]
#[ignore = "requires database (run with local dev Postgres)"]
async fn bulk_create_persists_every_row_it_reports_as_created()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let pool = migrated_pool().await?;
    let repository = SqlxContactRepository::new(pool.clone());

    // Names are scoped to this run so a run that died before its cleanup cannot
    // poison the next one.
    let run = Uuid::now_v7();
    let name = |suffix: &str| format!("bulk-regression-{run}-{suffix}");
    let good_id = Uuid::now_v7();
    let duplicate_id = Uuid::now_v7();

    // Seed the row the second batch item collides with (on its primary key), so
    // the rejection is a genuine constraint violation against committed state
    // rather than a duplicate inside the batch.
    repository
        .create(&contact(duplicate_id, name("seed"), None))
        .await?;

    // A valid row FIRST, then one that must be rejected. Under the old flat
    // transaction the rejection discarded the valid row too.
    let outcome = repository
        .bulk_create(&[
            contact(good_id, name("valid"), None),
            contact(duplicate_id, name("duplicate"), None),
        ])
        .await?;

    assert_eq!(
        outcome.created.len(),
        1,
        "expected exactly the valid row to be reported as created, got {:?}",
        outcome.created.iter().map(|r| &r.name).collect::<Vec<_>>()
    );
    assert_eq!(
        outcome.errors.len(),
        1,
        "expected exactly the duplicate to be reported as an error, got {:?}",
        outcome.errors
    );

    // The assertion the old implementation failed: everything reported as
    // created must actually be readable back.
    for record in &outcome.created {
        assert!(
            repository.get_by_id(record.id).await?.is_some(),
            "bulk_create reported '{}' as created, but it is not in the database — \
             the transaction that inserted it was rolled back",
            record.name
        );
    }

    for record in &outcome.created {
        repository.delete(record.id).await?;
    }
    repository.delete(duplicate_id).await?;
    Ok(())
}

/// A contact and its group link are one item: a failing link must not leave the
/// contact behind, and must not take the rest of the batch down with it.
#[tokio::test]
#[ignore = "requires database (run with local dev Postgres)"]
async fn bulk_create_rolls_back_a_whole_item_when_its_second_statement_fails()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let pool = migrated_pool().await?;
    let repository = SqlxContactRepository::new(pool.clone());

    let run = Uuid::now_v7();
    let name = |suffix: &str| format!("bulk-atomic-{run}-{suffix}");
    let orphan_id = Uuid::now_v7();
    let survivor_id = Uuid::now_v7();
    // No such group: the contact insert succeeds and the group link then fails
    // its foreign key, which is the two-statement item boundary under test.
    let missing_group = Uuid::now_v7();

    let outcome = repository
        .bulk_create(&[
            contact(orphan_id, name("bad-link"), Some(missing_group)),
            contact(survivor_id, name("after-failure"), None),
        ])
        .await?;

    assert!(
        repository.get_by_id(orphan_id).await?.is_none(),
        "the contact whose group link failed was left behind — the item was not \
         rolled back as a unit, so a row exists that the caller was told had failed"
    );
    assert_eq!(
        outcome.errors.len(),
        1,
        "expected only the bad-link item to fail, got {:?}",
        outcome.errors
    );
    // The item after the failure must be unaffected: under a flat transaction
    // the earlier failure would have aborted the block and taken it down too.
    assert!(
        repository.get_by_id(survivor_id).await?.is_some(),
        "the item following a failed one was lost — a failure is not isolated to \
         its own row"
    );

    repository.delete(survivor_id).await?;
    Ok(())
}
