//! Native upgrade, reference integrity and concurrent writer proof for Tags.
use sqlx::{Connection, PgConnection, PgPool, postgres::PgConnectOptions};
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;
const FORWARD: &str = include_str!("../migrations/0006_canonical_tag_ids.sql");

async fn database() -> Result<PgPool, Box<dyn std::error::Error + Send + Sync>> {
    let options: PgConnectOptions = std::env::var("PIPING_DATABASE__URL")?.parse()?;
    let mut admin = PgConnection::connect_with(&options).await?;
    let name = format!("pwa79_tags_{}", Uuid::now_v7().simple());
    let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new("CREATE DATABASE ");
    query.push(&name);
    query.build().execute(&mut admin).await?;
    let pool = PgPool::connect_with(options.database(&name)).await?;
    for migration in [
        include_str!("../migrations/0001_init.sql"),
        include_str!("../migrations/0002_interface_networking.sql"),
        include_str!("../migrations/0003_service_port_mappings.sql"),
        include_str!("../migrations/0004_fhrp_auth_key_secret.sql"),
        include_str!("../migrations/0005_module_bay_types.sql"),
    ] {
        sqlx::raw_sql(migration).execute(&pool).await?;
    }
    Ok(pool)
}

async fn upgrade(pool: &PgPool) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::raw_sql(FORWARD).execute(&mut *tx).await?;
    tx.commit().await
}

async fn tag(pool: &PgPool, name: &str, slug: &str) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO inventory_tag(id,name,slug) VALUES($1,$2,$3)")
        .bind(id)
        .bind(name)
        .bind(slug)
        .execute(pool)
        .await?;
    Ok(id)
}

async fn vm(pool: &PgPool, tags: &[String]) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO virtualization_virtualmachine(id,custom_field_data,name,status,comments,description,interface_count,virtual_disk_count,serial,start_on_boot,tags) VALUES($1,'{}',$2,'active','','',0,0,'','enabled',$3)")
        .bind(id).bind(format!("tag-vm-{id}")).bind(tags).execute(pool).await?;
    Ok(id)
}

#[tokio::test]
#[ignore = "requires exclusively owned PostgreSQL 18 fixture with CREATEDB"]
async fn canonical_tag_upgrade_preserves_untouched_native_factory_memberships() -> TestResult {
    let pool = database().await?;
    let subjects: Vec<String> = sqlx::query_scalar("SELECT r.relname::text FROM pg_class r JOIN pg_namespace n ON n.oid=r.relnamespace JOIN pg_attribute a ON a.attrelid=r.oid WHERE n.nspname='public' AND r.relkind IN ('r','p') AND a.attname='tags' AND a.atttypid='text[]'::regtype AND NOT a.attisdropped ORDER BY r.relname").fetch_all(&pool).await?;
    let mut before = Vec::new();
    for table in subjects {
        let quoted = table.replace('"', "\"\"");
        let mut query =
            sqlx::QueryBuilder::<sqlx::Postgres>::new("SELECT id,to_jsonb(owner) FROM public.\"");
        query
            .push(quoted)
            .push("\" owner WHERE 'factory-default'=ANY(tags) ORDER BY id");
        let rows: Vec<(Uuid, serde_json::Value)> = query.build_query_as().fetch_all(&pool).await?;
        before.extend(rows.into_iter().map(|(id, row)| (table.clone(), id, row)));
    }
    // These are real immutable factory rows, not replacements in a sanitized fixture.
    for suffix in ["1011", "1012", "1013", "1014"] {
        let id: Uuid = format!("00000000-0000-7000-8000-00000000{suffix}").parse()?;
        assert!(
            before
                .iter()
                .any(|(table, key, _)| table == "dcim_devicerole" && *key == id)
        );
    }
    assert!(
        before.len() > 4,
        "the rest of the factory metadata must also retain memberships"
    );
    upgrade(&pool).await?;
    let factory = "00000000-0000-7000-8000-000000001000";
    let catalog: (Uuid, String, String) =
        sqlx::query_as("SELECT id,name,slug FROM inventory_tag WHERE slug='factory-default'")
            .fetch_one(&pool)
            .await?;
    assert_eq!(
        catalog,
        (
            factory.parse::<Uuid>()?,
            "Factory default".to_owned(),
            "factory-default".to_owned()
        )
    );
    for (table, id, mut expected) in before {
        let tags = expected["tags"]
            .as_array_mut()
            .ok_or("factory tags missing")?;
        for token in tags {
            if token.as_str() == Some("factory-default") {
                *token = serde_json::Value::String(factory.into());
            }
        }
        let quoted = table.replace('"', "\"\"");
        let mut query =
            sqlx::QueryBuilder::<sqlx::Postgres>::new("SELECT to_jsonb(owner) FROM public.\"");
        query.push(quoted).push("\" owner WHERE id=$1");
        let after: serde_json::Value = query.build_query_scalar().bind(id).fetch_one(&pool).await?;
        assert_eq!(
            after, expected,
            "factory row {table}/{id} must retain every non-Tag value and membership occurrence"
        );
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires exclusively owned PostgreSQL 18 fixture with CREATEDB"]
async fn canonical_tag_factory_seed_refuses_catalog_conflicts_and_other_unknown_tokens()
-> TestResult {
    let factory: Uuid = "00000000-0000-7000-8000-000000001000".parse()?;
    for scenario in [
        "reserved-id",
        "slug",
        "name",
        "uuid-token",
        "attributes",
        "unknown-token",
        "unknown-uuid",
    ] {
        let pool = database().await?;
        match scenario {
            "reserved-id" => {
                sqlx::query("INSERT INTO inventory_tag(id,name,slug) VALUES($1,'Operator tag','operator-tag')").bind(factory).execute(&pool).await?;
            }
            "slug" => {
                tag(&pool, "Operator tag", "factory-default").await?;
            }
            "name" => {
                tag(&pool, "factory-default", "operator-tag").await?;
            }
            "uuid-token" => {
                tag(&pool, &factory.to_string(), "operator-tag").await?;
            }
            "attributes" => {
                sqlx::query("INSERT INTO inventory_tag(id,name,slug,color,description) VALUES($1,'Factory default','factory-default','ffffff','Marks native factory inventory definitions.')").bind(factory).execute(&pool).await?;
            }
            _ => {
                vm(
                    &pool,
                    &[if scenario == "unknown-token" {
                        "operator-unknown-token".into()
                    } else {
                        Uuid::now_v7().to_string()
                    }],
                )
                .await?;
            }
        }
        let before: serde_json::Value = sqlx::query_scalar(
            "SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY id),'[]') FROM inventory_tag t",
        )
        .fetch_one(&pool)
        .await?;
        assert!(
            upgrade(&pool).await.is_err(),
            "{scenario} must refuse atomically"
        );
        let after: serde_json::Value = sqlx::query_scalar(
            "SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY id),'[]') FROM inventory_tag t",
        )
        .fetch_one(&pool)
        .await?;
        assert_eq!(
            after, before,
            "{scenario} must not adopt or rewrite an operator identity, or retain a partially seeded definition"
        );
        let untouched: i64 = sqlx::query_scalar("SELECT count(*) FROM dcim_devicerole WHERE id=ANY($1) AND tags=ARRAY['factory-default']").bind([
            "00000000-0000-7000-8000-000000001011".parse::<Uuid>()?,
            "00000000-0000-7000-8000-000000001012".parse()?,
            "00000000-0000-7000-8000-000000001013".parse()?,
            "00000000-0000-7000-8000-000000001014".parse()?,
        ].as_slice()).fetch_one(&pool).await?;
        assert_eq!(
            untouched, 4,
            "{scenario} must retain untouched baseline factory memberships"
        );
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires exclusively owned PostgreSQL 18 fixture with CREATEDB"]
async fn canonical_tag_factory_seed_cannot_adopt_unknown_uuid_or_display_name() -> TestResult {
    let factory: Uuid = "00000000-0000-7000-8000-000000001000".parse()?;
    let role: Uuid = "00000000-0000-7000-8000-000000001011".parse()?;
    for (location, token) in [
        ("owner", factory.to_string()),
        ("owner", "Factory default".to_owned()),
        ("selector", format!("\u{2003}{}\t", factory)),
        ("selector", "\u{2003}FACTORY DEFAULT\t".to_owned()),
    ] {
        let pool = database().await?;
        let existed: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM inventory_tag WHERE id=$1)")
                .bind(factory)
                .fetch_one(&pool)
                .await?;
        assert!(
            !existed,
            "regression requires a genuinely unknown pre-seed identity"
        );
        let targets: i64 = sqlx::query_scalar("SELECT (SELECT count(*) FROM dcim_device)+(SELECT count(*) FROM virtualization_virtualmachine)").fetch_one(&pool).await?;
        assert_eq!(
            targets, 0,
            "the untouched baseline has no executable targets; conservation alone must not legitimize an unknown token"
        );
        let set = Uuid::now_v7();
        if location == "owner" {
            sqlx::query("UPDATE dcim_devicerole SET tags=ARRAY[$2] WHERE id=$1")
                .bind(role)
                .bind(&token)
                .execute(&pool)
                .await?;
        } else {
            sqlx::query("INSERT INTO playbook_target_set(id,name,mode,selector) VALUES($1,'unknown-before-seed','dynamic',$2)")
                .bind(set).bind(serde_json::json!({"tag_slugs":[token]})).execute(&pool).await?;
        }
        let role_before: serde_json::Value =
            sqlx::query_scalar("SELECT to_jsonb(r) FROM dcim_devicerole r WHERE id=$1")
                .bind(role)
                .fetch_one(&pool)
                .await?;
        let selector_before: Option<serde_json::Value> =
            sqlx::query_scalar("SELECT selector FROM playbook_target_set WHERE id=$1")
                .bind(set)
                .fetch_optional(&pool)
                .await?;
        let error = upgrade(&pool).await.expect_err(
            "seeding must not turn a previously unknown UUID/name into a legacy reference",
        );
        assert_eq!(
            error.as_database_error().and_then(|e| e.code()).as_deref(),
            Some("23514"),
            "{location} provenance refusal"
        );
        let retained: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM inventory_tag WHERE id=$1)")
                .bind(factory)
                .fetch_one(&pool)
                .await?;
        assert!(
            !retained,
            "the factory seed must roll back with unknown-token refusal"
        );
        let role_after: serde_json::Value =
            sqlx::query_scalar("SELECT to_jsonb(r) FROM dcim_devicerole r WHERE id=$1")
                .bind(role)
                .fetch_one(&pool)
                .await?;
        assert_eq!(
            role_after, role_before,
            "{location}: preserve the complete original owner row"
        );
        let selector_after: Option<serde_json::Value> =
            sqlx::query_scalar("SELECT selector FROM playbook_target_set WHERE id=$1")
                .bind(set)
                .fetch_optional(&pool)
                .await?;
        assert_eq!(
            selector_after, selector_before,
            "{location}: preserve the original selector predicate"
        );
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires exclusively owned PostgreSQL 18 fixture with CREATEDB"]
async fn canonical_tag_factory_seed_retains_genuinely_known_uuid_and_name_references() -> TestResult
{
    let pool = database().await?;
    let factory: Uuid = "00000000-0000-7000-8000-000000001000".parse()?;
    sqlx::query("INSERT INTO inventory_tag(id,name,slug,color,description,weight) VALUES($1,'Factory default','factory-default','9e9e9e','Marks native factory inventory definitions.',0)")
        .bind(factory).execute(&pool).await?;
    let before: serde_json::Value =
        sqlx::query_scalar("SELECT to_jsonb(t) FROM inventory_tag t WHERE id=$1")
            .bind(factory)
            .fetch_one(&pool)
            .await?;
    let roles: [Uuid; 2] = [
        "00000000-0000-7000-8000-000000001011".parse()?,
        "00000000-0000-7000-8000-000000001012".parse()?,
    ];
    for (role, token) in [
        (roles[0], factory.to_string()),
        (roles[1], "Factory default".to_owned()),
    ] {
        sqlx::query("UPDATE dcim_devicerole SET tags=ARRAY[$2] WHERE id=$1")
            .bind(role)
            .bind(token)
            .execute(&pool)
            .await?;
    }
    let set = Uuid::now_v7();
    sqlx::query("INSERT INTO playbook_target_set(id,name,mode,selector) VALUES($1,'known-before-seed','dynamic',$2)")
        .bind(set).bind(serde_json::json!({"tag_slugs":[format!("\u{2003}{}\t",factory)]})).execute(&pool).await?;
    upgrade(&pool).await?;
    for role in roles {
        let tags: Vec<String> = sqlx::query_scalar("SELECT tags FROM dcim_devicerole WHERE id=$1")
            .bind(role)
            .fetch_one(&pool)
            .await?;
        assert_eq!(tags, vec![factory.to_string()]);
    }
    let selector: serde_json::Value =
        sqlx::query_scalar("SELECT selector FROM playbook_target_set WHERE id=$1")
            .bind(set)
            .fetch_one(&pool)
            .await?;
    assert_eq!(
        selector,
        serde_json::json!({"tag_ids":[factory.to_string()]})
    );
    let after: serde_json::Value =
        sqlx::query_scalar("SELECT to_jsonb(t) FROM inventory_tag t WHERE id=$1")
            .bind(factory)
            .fetch_one(&pool)
            .await?;
    assert_eq!(
        after, before,
        "an exact pre-existing factory identity is retained without metadata rewrites"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires exclusively owned PostgreSQL 18 fixture with CREATEDB"]
async fn canonical_tag_upgrade_preserves_identity_order_and_selector_predicate() -> TestResult {
    let pool = database().await?;
    let first = tag(&pool, "Display First", "first").await?;
    let second = tag(&pool, "Display Second", "second").await?;
    let owner = vm(&pool, &["first".into(), "Display Second".into()]).await?;
    let set = Uuid::now_v7();
    sqlx::query("INSERT INTO playbook_target_set(id,name,mode,selector) VALUES($1,'tag-conservation','dynamic',$2)")
        .bind(set).bind(serde_json::json!({"tag_slugs":["\u{2003}FIRST\u{2003}"," "],"status":["active"],"limit":1}))
        .execute(&pool).await?;
    upgrade(&pool).await?;
    let ids: Vec<String> =
        sqlx::query_scalar("SELECT tags FROM virtualization_virtualmachine WHERE id=$1")
            .bind(owner)
            .fetch_one(&pool)
            .await?;
    assert_eq!(ids, vec![first.to_string(), second.to_string()]);
    let selector: serde_json::Value =
        sqlx::query_scalar("SELECT selector FROM playbook_target_set WHERE id=$1")
            .bind(set)
            .fetch_one(&pool)
            .await?;
    assert_eq!(
        selector,
        serde_json::json!({"tag_ids":[first.to_string()],"status":["active"],"limit":1})
    );
    let discovered: i64 = sqlx::query_scalar("SELECT count(*) FROM inventory_tag_subjects()")
        .fetch_one(&pool)
        .await?;
    let installed: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_attribute a JOIN pg_class r ON r.oid=a.attrelid JOIN pg_namespace n ON n.oid=r.relnamespace WHERE n.nspname='public' AND r.relkind IN ('r','p') AND a.attname='tags' AND a.atttypid='text[]'::regtype AND NOT a.attisdropped").fetch_one(&pool).await?;
    assert_eq!(
        discovered, installed,
        "every installed inventory tags column must be guarded"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires exclusively owned PostgreSQL 18 fixture with CREATEDB"]
async fn canonical_tag_upgrade_refuses_unknown_ambiguous_duplicate_and_widened_targets_atomically()
-> TestResult {
    for scenario in [
        "unknown",
        "ambiguous",
        "duplicate",
        "widening",
        "preexisting-key",
    ] {
        let pool = database().await?;
        let id = tag(&pool, "Production", "production").await?;
        let tokens = match scenario {
            "unknown" => vec![Uuid::now_v7().to_string()],
            "ambiguous" => {
                tag(&pool, "production", "another").await?;
                vec!["production".into()]
            }
            "duplicate" => vec!["production".into(), id.to_string()],
            _ => vec![id.to_string()],
        };
        let owner = vm(&pool, &tokens).await?;
        if scenario == "widening" {
            sqlx::query("INSERT INTO playbook_target_set(name,mode,selector) VALUES('would-widen','dynamic','{\"tag_slugs\":[\"production\"]}')").execute(&pool).await?;
        }
        if scenario == "preexisting-key" {
            sqlx::query("INSERT INTO playbook_target_set(name,mode,selector) VALUES('previously-ignored','dynamic',$1)").bind(serde_json::json!({"tag_ids":[id.to_string()]})).execute(&pool).await?;
        }
        assert!(
            upgrade(&pool).await.is_err(),
            "{scenario} must refuse conversion"
        );
        let after: Vec<String> =
            sqlx::query_scalar("SELECT tags FROM virtualization_virtualmachine WHERE id=$1")
                .bind(owner)
                .fetch_one(&pool)
                .await?;
        assert_eq!(after, tokens, "{scenario} must roll back all data rewrites");
        let guard: Option<String> = sqlx::query_scalar(
            "SELECT to_regprocedure('inventory_validate_tag_ids(text[])')::text",
        )
        .fetch_one(&pool)
        .await?;
        assert!(guard.is_none(), "{scenario} must roll back schema too");
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires exclusively owned PostgreSQL 18 fixture with CREATEDB"]
async fn canonical_tag_database_guards_native_writes_and_persisted_selectors() -> TestResult {
    let pool = database().await?;
    upgrade(&pool).await?;
    let id = tag(&pool, "Initial", "initial").await?;
    let owner = vm(&pool, &[id.to_string()]).await?;
    for values in [
        vec!["initial".into()],
        vec![Uuid::now_v7().to_string()],
        vec![id.to_string(), id.to_string()],
    ] {
        assert!(
            sqlx::query("UPDATE virtualization_virtualmachine SET tags=$2 WHERE id=$1")
                .bind(owner)
                .bind(values)
                .execute(&pool)
                .await
                .is_err()
        );
    }
    for statement in [
        "UPDATE virtualization_virtualmachine SET tags=NULL::text[] WHERE id=$1",
        "UPDATE virtualization_virtualmachine SET tags=ARRAY[NULL]::text[] WHERE id=$1",
    ] {
        assert!(
            sqlx::query(statement)
                .bind(owner)
                .execute(&pool)
                .await
                .is_err()
        );
    }
    assert!(
        sqlx::query("UPDATE inventory_tag SET id=$2 WHERE id=$1")
            .bind(id)
            .bind(Uuid::now_v7())
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("TRUNCATE inventory_tag")
            .execute(&pool)
            .await
            .is_err()
    );
    sqlx::query("UPDATE inventory_tag SET name='Renamed',slug='renamed' WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await?;
    assert!(
        sqlx::query("DELETE FROM inventory_tag WHERE id=$1")
            .bind(id)
            .execute(&pool)
            .await
            .is_err()
    );
    sqlx::query("UPDATE virtualization_virtualmachine SET tags='{}' WHERE id=$1")
        .bind(owner)
        .execute(&pool)
        .await?;
    for selector in [
        serde_json::json!({"tag_slugs":[]}),
        serde_json::json!({"tag_ids":null}),
        serde_json::json!({"tag_ids":[id.to_string(),id.to_string()]}),
        serde_json::json!({"tag_ids":[Uuid::now_v7().to_string()]}),
    ] {
        assert!(
            sqlx::query(
                "INSERT INTO playbook_target_set(name,mode,selector) VALUES('invalid','dynamic',$1)"
            )
            .bind(selector)
            .execute(&pool)
            .await
            .is_err()
        );
    }
    let set = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO playbook_target_set(id,name,mode,selector) VALUES($1,'valid','dynamic',$2)",
    )
    .bind(set)
    .bind(serde_json::json!({"tag_ids":[id.to_string()]}))
    .execute(&pool)
    .await?;
    assert!(
        sqlx::query("DELETE FROM inventory_tag WHERE id=$1")
            .bind(id)
            .execute(&pool)
            .await
            .is_err()
    );
    sqlx::query("DELETE FROM playbook_target_set WHERE id=$1")
        .bind(set)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM inventory_tag WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await?;
    let mut assessment = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .execute(&mut *assessment)
        .await?;
    sqlx::query("SELECT tags FROM virtualization_virtualmachine")
        .fetch_all(&mut *assessment)
        .await?;
    assessment.commit().await?;
    let mut stale = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut *stale)
        .await?;
    assert!(
        sqlx::query("UPDATE virtualization_virtualmachine SET tags='{}' WHERE id=$1")
            .bind(owner)
            .execute(&mut *stale)
            .await
            .is_err()
    );
    stale.rollback().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires exclusively owned PostgreSQL 18 fixture with CREATEDB"]
async fn canonical_tag_assignment_and_delete_serialize_in_both_orders() -> TestResult {
    let pool = database().await?;
    upgrade(&pool).await?;
    let id = tag(&pool, "Concurrent", "concurrent").await?;
    let owner = vm(&pool, &[]).await?;
    let mut writer = pool.begin().await?;
    sqlx::query("UPDATE virtualization_virtualmachine SET tags=ARRAY[$2] WHERE id=$1")
        .bind(owner)
        .bind(id.to_string())
        .execute(&mut *writer)
        .await?;
    let mut deletion = pool.begin().await?;
    sqlx::query("SET LOCAL lock_timeout='100ms'")
        .execute(&mut *deletion)
        .await?;
    let blocked = sqlx::query("DELETE FROM inventory_tag WHERE id=$1")
        .bind(id)
        .execute(&mut *deletion)
        .await
        .expect_err("delete waits for membership transaction");
    assert_eq!(
        blocked
            .as_database_error()
            .and_then(|e| e.code())
            .as_deref(),
        Some("55P03")
    );
    deletion.rollback().await?;
    writer.commit().await?;
    assert!(
        sqlx::query("DELETE FROM inventory_tag WHERE id=$1")
            .bind(id)
            .execute(&pool)
            .await
            .is_err()
    );
    sqlx::query("UPDATE virtualization_virtualmachine SET tags='{}' WHERE id=$1")
        .bind(owner)
        .execute(&pool)
        .await?;
    let mut deletion = pool.begin().await?;
    sqlx::query("DELETE FROM inventory_tag WHERE id=$1")
        .bind(id)
        .execute(&mut *deletion)
        .await?;
    let mut writer = pool.begin().await?;
    sqlx::query("SET LOCAL lock_timeout='100ms'")
        .execute(&mut *writer)
        .await?;
    let blocked =
        sqlx::query("UPDATE virtualization_virtualmachine SET tags=ARRAY[$2] WHERE id=$1")
            .bind(owner)
            .bind(id.to_string())
            .execute(&mut *writer)
            .await
            .expect_err("membership waits for catalogue transaction");
    assert_eq!(
        blocked
            .as_database_error()
            .and_then(|e| e.code())
            .as_deref(),
        Some("55P03")
    );
    writer.rollback().await?;
    deletion.commit().await?;
    assert!(
        sqlx::query("UPDATE virtualization_virtualmachine SET tags=ARRAY[$2] WHERE id=$1")
            .bind(owner)
            .bind(id.to_string())
            .execute(&pool)
            .await
            .is_err()
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires exclusively owned PostgreSQL 18 fixture with CREATEDB"]
async fn canonical_tag_selector_reference_and_delete_serialize() -> TestResult {
    let pool = database().await?;
    upgrade(&pool).await?;
    let id = tag(&pool, "Selector concurrent", "selector-concurrent").await?;
    let set = Uuid::now_v7();
    let mut writer = pool.begin().await?;
    sqlx::query("INSERT INTO playbook_target_set(id,name,mode,selector) VALUES($1,'concurrent','dynamic',$2)")
        .bind(set).bind(serde_json::json!({"tag_ids":[id.to_string()]})).execute(&mut *writer).await?;
    let mut deletion = pool.begin().await?;
    sqlx::query("SET LOCAL lock_timeout='100ms'")
        .execute(&mut *deletion)
        .await?;
    let blocked = sqlx::query("DELETE FROM inventory_tag WHERE id=$1")
        .bind(id)
        .execute(&mut *deletion)
        .await
        .expect_err("selector assignment holds shared catalogue lock");
    assert_eq!(
        blocked
            .as_database_error()
            .and_then(|e| e.code())
            .as_deref(),
        Some("55P03")
    );
    deletion.rollback().await?;
    writer.commit().await?;
    assert!(
        sqlx::query("DELETE FROM inventory_tag WHERE id=$1")
            .bind(id)
            .execute(&pool)
            .await
            .is_err()
    );
    sqlx::query("DELETE FROM playbook_target_set WHERE id=$1")
        .bind(set)
        .execute(&pool)
        .await?;
    let mut deletion = pool.begin().await?;
    sqlx::query("DELETE FROM inventory_tag WHERE id=$1")
        .bind(id)
        .execute(&mut *deletion)
        .await?;
    let mut writer = pool.begin().await?;
    sqlx::query("SET LOCAL lock_timeout='100ms'")
        .execute(&mut *writer)
        .await?;
    let blocked = sqlx::query(
        "INSERT INTO playbook_target_set(name,mode,selector) VALUES('must-wait','dynamic',$1)",
    )
    .bind(serde_json::json!({"tag_ids":[id.to_string()]}))
    .execute(&mut *writer)
    .await
    .expect_err("selector writer waits for catalogue deletion");
    assert_eq!(
        blocked
            .as_database_error()
            .and_then(|e| e.code())
            .as_deref(),
        Some("55P03")
    );
    writer.rollback().await?;
    deletion.commit().await?;
    assert!(sqlx::query("INSERT INTO playbook_target_set(name,mode,selector) VALUES('deleted-reference','dynamic',$1)")
        .bind(serde_json::json!({"tag_ids":[id.to_string()]})).execute(&pool).await.is_err());
    Ok(())
}

async fn observe_catalog_waiter(pool: &PgPool, pid: i32) -> TestResult {
    tokio::time::timeout(std::time::Duration::from_secs(30),async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND $1=ANY(pg_blocking_pids(pid)))").bind(pid).fetch_one(pool).await?;
            if waiting { return Ok::<(),sqlx::Error>(()); }
            tokio::task::yield_now().await;
        }
    }).await??;
    Ok(())
}

#[tokio::test]
#[ignore = "requires exclusively owned PostgreSQL 18 fixture with CREATEDB"]
async fn canonical_tag_waiting_statements_recheck_references_after_lock_release() -> TestResult {
    let pool = database().await?;
    upgrade(&pool).await?;
    let id = tag(&pool, "Fresh snapshot", "fresh-snapshot").await?;
    let owner = vm(&pool, &[]).await?;
    let mut writer = pool.begin().await?;
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *writer)
        .await?;
    sqlx::query("UPDATE virtualization_virtualmachine SET tags=ARRAY[$2] WHERE id=$1")
        .bind(owner)
        .bind(id.to_string())
        .execute(&mut *writer)
        .await?;
    let mut tasks = tokio::task::JoinSet::new();
    let waiter_pool = pool.clone();
    tasks.spawn(async move {
        sqlx::query("DELETE FROM inventory_tag WHERE id=$1")
            .bind(id)
            .execute(&waiter_pool)
            .await
    });
    observe_catalog_waiter(&pool, pid).await?;
    writer.commit().await?;
    let result = tokio::time::timeout(std::time::Duration::from_secs(30), tasks.join_next())
        .await?
        .ok_or("delete waiter disappeared")??;
    let error = result.expect_err("waiting delete must observe newly committed membership");
    assert_eq!(
        error.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("23503")
    );
    sqlx::query("UPDATE virtualization_virtualmachine SET tags='{}' WHERE id=$1")
        .bind(owner)
        .execute(&pool)
        .await?;
    let mut deletion = pool.begin().await?;
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *deletion)
        .await?;
    sqlx::query("DELETE FROM inventory_tag WHERE id=$1")
        .bind(id)
        .execute(&mut *deletion)
        .await?;
    let waiter_pool = pool.clone();
    tasks.spawn(async move {
        sqlx::query("UPDATE virtualization_virtualmachine SET tags=ARRAY[$2] WHERE id=$1")
            .bind(owner)
            .bind(id.to_string())
            .execute(&waiter_pool)
            .await
    });
    observe_catalog_waiter(&pool, pid).await?;
    deletion.commit().await?;
    let result = tokio::time::timeout(std::time::Duration::from_secs(30), tasks.join_next())
        .await?
        .ok_or("membership waiter disappeared")??;
    let error = result.expect_err("waiting membership must observe deleted catalogue identity");
    assert_eq!(
        error.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("23503")
    );
    Ok(())
}
