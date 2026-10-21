//! DB-backed regression test for VLAN role assignment.
//!
//! `ipam_vlan.role_id` is a `uuid` with a foreign key to `ipam_role.id`, but the
//! whole Rust stack — repository record, create/update inputs, `try_get`, and
//! the REST DTOs — typed it `Option<i64>`. sqlx declares the bind parameter's
//! type from the Rust type even when the value is `None`, so every write
//! carrying a non-null `role_id` was rejected by Postgres with a type mismatch,
//! and any read of a row that had one failed to decode.
//!
//! The feature was therefore entirely non-functional, and silently so: the
//! defect only shows when a VLAN actually has a role, and no row in the
//! database had one. Nothing in the type-checked build could catch it, because
//! both sides compiled fine — only the database disagreed.
//!
//! This pins the round trip end to end: assign a role on create, read it back,
//! change it on update, and clear it.

mod common;

use piping_backend::contexts::inventory::infrastructure::persistence::repositories::ipam::vlan_repository::{
    NewVlan, SqlxVlanRepository, UpdateVlan, VlanRepository,
};
use sqlx::Row;
use uuid::Uuid;

async fn migrated_pool() -> Result<sqlx::PgPool, Box<dyn std::error::Error + Send + Sync>> {
    let pool = common::build_test_db_pool().await?;
    piping_backend::storage::database::migrations::run_migrations(&pool)
        .await
        .map_err(|e| format!("run migrations: {e}"))?;
    Ok(pool)
}

/// Insert a minimal `ipam_role` and return its id.
async fn seed_role(
    pool: &sqlx::PgPool,
    label: &str,
) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
    let id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO ipam_role (id, name, slug, weight, description, comments, custom_field_data)
        VALUES ($1, $2, $3, 1000, '', '', '{}'::jsonb)
        "#,
    )
    .bind(id)
    .bind(label)
    .bind(label)
    .execute(pool)
    .await?;
    Ok(id)
}

#[tokio::test]
#[ignore = "requires database (run with local dev Postgres)"]
async fn vlan_role_id_round_trips_as_a_uuid() -> Result<(), Box<dyn std::error::Error + Send + Sync>>
{
    let pool = migrated_pool().await?;
    let repository = SqlxVlanRepository::new(pool.clone());

    let run = Uuid::now_v7();
    let first_role = seed_role(&pool, &format!("vlan-role-a-{run}")).await?;
    let second_role = seed_role(&pool, &format!("vlan-role-b-{run}")).await?;

    // A vid unlikely to collide with seeded data; ipam_vlan has no unique
    // constraint on vid alone, but keeping it distinct keeps the test readable.
    let vlan_id = Uuid::now_v7();
    let created = repository
        .create(&NewVlan {
            id: vlan_id,
            vid: 3999,
            name: format!("vlan-role-regression-{run}"),
            status: None,
            site_id: None,
            group_id: None,
            tenant_id: None,
            // The whole point: a non-null role on the write path. Under the
            // `i64` typing this bind was rejected by Postgres outright.
            role_id: Some(first_role),
            description: None,
        })
        .await?;
    assert_eq!(
        created.role_id,
        Some(first_role),
        "create did not persist the role it was given"
    );

    // Read back through the same decode path that used to fail on a non-null role.
    let loaded = repository
        .get_by_id(vlan_id)
        .await?
        .ok_or("vlan disappeared after create")?;
    assert_eq!(
        loaded.role_id,
        Some(first_role),
        "the stored role did not survive a read"
    );

    // The column really is a uuid, so a correct write is readable as one.
    let raw: Uuid = sqlx::query("SELECT role_id FROM ipam_vlan WHERE id = $1")
        .bind(vlan_id)
        .fetch_one(&pool)
        .await?
        .try_get("role_id")?;
    assert_eq!(
        raw, first_role,
        "the column does not hold the assigned role"
    );

    let updated = repository
        .update(
            vlan_id,
            &UpdateVlan {
                vid: None,
                name: None,
                status: None,
                site_id: None,
                group_id: None,
                tenant_id: None,
                role_id: Some(second_role),
                description: None,
            },
        )
        .await?
        .ok_or("vlan disappeared after update")?;
    assert_eq!(
        updated.role_id,
        Some(second_role),
        "update did not move the vlan to the new role"
    );

    repository.delete(vlan_id).await?;
    sqlx::query("DELETE FROM ipam_role WHERE id = ANY($1)")
        .bind(vec![first_role, second_role])
        .execute(&pool)
        .await?;
    Ok(())
}
