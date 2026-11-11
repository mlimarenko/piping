//! Prepared selective-restore regressions; execute only in disposable native services.
#[path = "common/child_database.rs"]
mod child_database;
mod common;

use std::sync::Arc;

use axum::{Router, http::StatusCode};
use piping_backend::contexts::inventory::application::backup_service::FullBackupArchive;
use piping_backend::contexts::inventory::application::{
    BackupService, FullBackupImportAction, FullBackupImportRequest, FullBackupImportRules,
    FullBackupSelection,
};
use piping_backend::errors::AppError;
use serde_json::{Value, json};
use sqlx::{PgPool, migrate::Migrator};
use uuid::Uuid;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
const POLICY_TABLE: &str = "ipam_vlantranslationpolicy";
const RULE_TABLE: &str = "ipam_vlantranslationrule";
const POLICIES: &str = "/api/v1/inventory/vlan-translation-policies";
const RULES: &str = "/api/v1/inventory/vlan-translation-rules";

async fn post(app: &Router, cookie: &str, path: &str, payload: Value) -> TestResult<Value> {
    let (status, body) =
        common::request(app, "POST", path, Some(&payload.to_string()), Some(cookie)).await?;
    assert_eq!(status, StatusCode::CREATED, "POST {path}");
    Ok(serde_json::from_slice(&body)?)
}

fn selection(table: &str) -> FullBackupSelection {
    FullBackupSelection {
        sections: vec![],
        tables: vec![table.into()],
    }
}

fn restore(archive: &FullBackupArchive, table: &str, dry_run: bool) -> FullBackupImportRequest {
    FullBackupImportRequest {
        payload: archive.clone(),
        confirm_full_restore: true,
        dry_run,
        selection: Some(selection(table)),
        rules: Some(FullBackupImportRules {
            default_action: FullBackupImportAction::Delete,
            ..Default::default()
        }),
    }
}

fn empty_archive(mut archive: FullBackupArchive) -> FullBackupArchive {
    for table in &mut archive.tables {
        table.rows.clear();
    }
    archive.row_count = 0;
    archive
}

async fn policy(app: &Router, cookie: &str, label: &str) -> TestResult<Uuid> {
    let row = post(
        app,
        cookie,
        POLICIES,
        json!({
            "name":label, "custom_fields":{"restore_policy_credential":Uuid::now_v7().to_string()}
        }),
    )
    .await?;
    Ok(Uuid::parse_str(row["id"].as_str().ok_or("policy id")?)?)
}

async fn rule(app: &Router, cookie: &str, policy_id: Uuid, vid: i32) -> TestResult<Uuid> {
    let row = post(
        app,
        cookie,
        RULES,
        json!({
            "policy_id":policy_id, "local_vid":vid, "remote_vid":vid + 100,
            "custom_fields":{"restore_rule_credential":Uuid::now_v7().to_string()}
        }),
    )
    .await?;
    Ok(Uuid::parse_str(row["id"].as_str().ok_or("rule id")?)?)
}

async fn secret_ids(pool: &PgPool, consumers: &[Uuid]) -> TestResult<Vec<Uuid>> {
    let ids = sqlx::query_scalar::<_, Uuid>(
        "SELECT secret_id FROM secret_usage_binding WHERE consumer_id=ANY($1) ORDER BY secret_id",
    )
    .bind(consumers)
    .fetch_all(pool)
    .await?;
    assert_eq!(
        ids.len(),
        consumers.len(),
        "one owned secret per seeded consumer"
    );
    for (table, column) in [
        ("secrets_binding", "secret_id"),
        ("secrets_variable", "id"),
        ("secrets_store", "secret_id"),
    ] {
        let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new("SELECT count(*) FROM ");
        query
            .push(table)
            .push(" WHERE ")
            .push(column)
            .push("=ANY($1)");
        let count: i64 = query
            .build_query_scalar()
            .bind(&ids)
            .fetch_one(pool)
            .await?;
        assert_eq!(count, ids.len() as i64, "seeded {table}");
    }
    Ok(ids)
}

async fn assert_retired(pool: &PgPool, consumers: &[Uuid], secrets: &[Uuid]) -> TestResult {
    for table in [POLICY_TABLE, RULE_TABLE] {
        let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new("SELECT count(*) FROM ");
        query.push(table).push(" WHERE id=ANY($1)");
        let count: i64 = query
            .build_query_scalar()
            .bind(consumers)
            .fetch_one(pool)
            .await?;
        assert_eq!(count, 0, "deleted native rows in {table}");
    }
    for (table, column) in [
        ("secret_usage_binding", "secret_id"),
        ("secrets_binding", "secret_id"),
        ("secrets_variable", "id"),
        ("secrets_store", "secret_id"),
    ] {
        let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new("SELECT count(*) FROM ");
        query
            .push(table)
            .push(" WHERE ")
            .push(column)
            .push("=ANY($1)");
        let count: i64 = query
            .build_query_scalar()
            .bind(secrets)
            .fetch_one(pool)
            .await?;
        assert_eq!(count, 0, "retired {table}");
    }
    Ok(())
}

async fn snapshot(pool: &PgPool) -> TestResult<Value> {
    // Include ciphertext and managed bindings, not just the cascade's native rows.
    Ok(sqlx::query_scalar("SELECT jsonb_build_object('policies',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM ipam_vlantranslationpolicy t),'rules',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM ipam_vlantranslationrule t),'usage',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM secret_usage_binding t),'bindings',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM secrets_binding t),'variables',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM secrets_variable t),'store',(SELECT jsonb_agg(to_jsonb(t) ORDER BY secret_id) FROM secrets_store t))").fetch_one(pool).await?)
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 and Redis; main owns native execution"]
async fn selective_vlan_restore_retires_owned_secrets_and_rolls_back_blocked_deletes() -> TestResult
{
    let (child, url) = child_database::ChildDatabase::create("pwa80_vlan_").await?;
    let result = async {
        let pool = PgPool::connect(&url).await?;
        let result = exercise_restore(&url, pool.clone()).await;
        pool.close().await;
        result
    }
    .await;
    let cleanup = child.cleanup().await;
    result?;
    cleanup?;
    Ok(())
}

async fn exercise_restore(url: &str, pool: PgPool) -> TestResult {
    Migrator::new(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("migrations")
            .as_path(),
    )
    .await?
    .run(&pool)
    .await?;
    let mut settings = piping_backend::config::load_settings()?;
    settings.database.url = url.into();
    settings.redis.url = common::redis_url(Some(14))?;
    settings.security.allow_weak_dev_secrets = true;
    settings.auth.local_enabled = true;
    common::configure_test_cipher(&mut settings);
    common::ensure_test_admin(&pool, &settings.auth.password_pepper).await?;
    let mut redis =
        redis::aio::ConnectionManager::new(redis::Client::open(settings.redis.url.clone())?)
            .await?;
    let _: i64 = redis::cmd("INCR")
        .arg("access_control:policies:version")
        .query_async(&mut redis)
        .await?;
    let state =
        piping_backend::app::AppState::build(Arc::new(settings), pool.clone(), redis).await?;
    let app = piping_backend::app::build_router(state);
    let cookie = common::login_and_get_cookie(
        &app,
        common::TEST_ADMIN_USERNAME,
        common::TEST_ADMIN_PASSWORD,
    )
    .await
    .ok_or("admin login")?;
    for (model, field) in [
        ("vlantranslationpolicy", "restore_policy_credential"),
        ("vlantranslationrule", "restore_rule_credential"),
    ] {
        let scope: Uuid = sqlx::query_scalar(
            "SELECT id FROM inventory_content_type WHERE app_label='ipam' AND model=$1",
        )
        .bind(model)
        .fetch_one(&pool)
        .await?;
        post(&app, &cookie, "/api/v1/custom-fields", json!({
            "name":field,"label":"Restore credential","field_type":"text","is_secret":true,"object_types":[scope]
        })).await?;
    }
    let service = BackupService::new(pool.clone());
    let keeper = policy(&app, &cookie, "Kept policy").await?;
    let keeper_rule = rule(&app, &cookie, keeper, 1).await?;
    let kept_secrets = secret_ids(&pool, &[keeper, keeper_rule]).await?;
    let archive = service
        .export_full_backup(true, Some(selection(POLICY_TABLE)))
        .await?;
    assert_eq!(
        archive.tables.len(),
        1,
        "do not select rule or secret tables"
    );

    let first = policy(&app, &cookie, "Missing policy A").await?;
    let second = policy(&app, &cookie, "Missing policy B").await?;
    // UUID order matches enumeration: force an earlier successful deletion
    // before the later blocked deletion to exercise transaction-wide rollback.
    let blocked = first.max(second);
    let first_rule = rule(&app, &cookie, first, 2).await?;
    let second_rule = rule(&app, &cookie, second, 3).await?;
    let removed = [first, second, first_rule, second_rule];
    let removed_secrets = secret_ids(&pool, &removed).await?;

    let before = snapshot(&pool).await?;
    let dry = service
        .import_full_backup(restore(&archive, POLICY_TABLE, true))
        .await?;
    assert!(!dry.applied);
    let plan = dry.plan.ok_or("dry-run plan")?;
    assert_eq!(plan.tables.len(), 1);
    assert_eq!(
        plan.tables[0].deletes, 2,
        "plan counts policies, not cascaded rules"
    );
    assert!(
        snapshot(&pool).await? == before,
        "dry run changed native or secret state"
    );

    let vm = Uuid::now_v7();
    sqlx::query("INSERT INTO virtualization_virtualmachine (id,custom_field_data,name,status,comments,description,interface_count,virtual_disk_count,serial,start_on_boot) VALUES ($1,'{}','Restore VM','active','','',0,0,'','enabled')")
        .bind(vm).execute(&pool).await?;
    for table in ["dcim_interface", "virtualization_vminterface"] {
        let interface = Uuid::now_v7();
        let sql = if table == "dcim_interface" {
            "INSERT INTO dcim_interface (id,custom_field_data,name,label,description,mark_connected,enabled,_name,type,mgmt_only,vlan_translation_policy_id) VALUES ($1,'{}','eth0','','',false,true,'eth0','virtual',false,$2)"
        } else {
            "INSERT INTO virtualization_vminterface (id,custom_field_data,name,_name,enabled,description,mtu,vlan_translation_policy_id,virtual_machine_id) VALUES ($1,'{}','eth0','eth0',true,'',1500,$2,$3)"
        };
        let mut insert = sqlx::query(sql).bind(interface).bind(blocked);
        if table == "virtualization_vminterface" {
            insert = insert.bind(vm);
        }
        insert.execute(&pool).await?;
        let before = snapshot(&pool).await?;
        let error = service
            .import_full_backup(restore(&archive, POLICY_TABLE, false))
            .await
            .expect_err("interface reference must block selective policy deletion");
        assert!(
            matches!(error, AppError::DeleteBlocked(_)),
            "expected canonical blocked-delete error"
        );
        assert!(
            snapshot(&pool).await? == before,
            "blocked restore did not roll back native and secret changes"
        );
        let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new("DELETE FROM ");
        query.push(table).push(" WHERE id=$1");
        query.build().bind(interface).execute(&pool).await?;
    }

    let applied = service
        .import_full_backup(restore(&archive, POLICY_TABLE, false))
        .await?;
    assert!(applied.applied);
    assert_eq!(applied.plan.ok_or("applied plan")?.tables[0].deletes, 2);
    assert_retired(&pool, &removed, &removed_secrets).await?;
    assert_eq!(
        secret_ids(&pool, &[keeper, keeper_rule]).await?,
        kept_secrets,
        "incoming existing policy and its unselected rule must retain their secrets"
    );

    // Empty source must take the same canonical policy cascade path.
    let empty = empty_archive(archive);
    service
        .import_full_backup(restore(&empty, POLICY_TABLE, false))
        .await?;
    assert_retired(&pool, &[keeper, keeper_rule], &kept_secrets).await?;

    // Rule-only restore must retire its own consumers without deleting its policy.
    let parent = policy(&app, &cookie, "Rule-only parent").await?;
    let kept_rule = rule(&app, &cookie, parent, 4).await?;
    let archive = service
        .export_full_backup(true, Some(selection(RULE_TABLE)))
        .await?;
    let missing_rule = rule(&app, &cookie, parent, 5).await?;
    let missing_secret = secret_ids(&pool, &[missing_rule]).await?;
    service
        .import_full_backup(restore(&archive, RULE_TABLE, false))
        .await?;
    assert_retired(&pool, &[missing_rule], &missing_secret).await?;
    secret_ids(&pool, &[parent, kept_rule]).await?;
    let kept_rule_secret = secret_ids(&pool, &[kept_rule]).await?;
    service
        .import_full_backup(restore(&empty_archive(archive), RULE_TABLE, false))
        .await?;
    assert_retired(&pool, &[kept_rule], &kept_rule_secret).await?;
    secret_ids(&pool, &[parent]).await?;
    Ok(())
}
