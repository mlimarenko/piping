use piping_backend::contexts::inventory::infrastructure::persistence::repositories::extras::config_context_repository::{ConfigContextRepository, SqlxConfigContextRepository};
use piping_backend::contexts::inventory::domain::ConfigContextBinding;
use piping_backend::contexts::inventory::infrastructure::persistence::repositories::delete_guard::delete_inventory_record;
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires a migrated PostgreSQL database"]
async fn migration_rejects_ambiguous_contexts_atomically() -> Result<(), Box<dyn std::error::Error>>
{
    let pool = sqlx::PgPool::connect(&std::env::var("PIPING_DATABASE__URL")?).await?;
    let sentinel = Uuid::now_v7();
    let binding = Uuid::now_v7();
    let role: Uuid = sqlx::query_scalar("SELECT id FROM dcim_devicerole ORDER BY id LIMIT 1")
        .fetch_one(&pool)
        .await?;
    sqlx::query("INSERT INTO config_context (id,name,data,is_active,weight) VALUES ($1,$2,'{\"preserved\":true}',false,123)")
        .bind(sentinel).bind(sentinel.to_string()).execute(&pool).await?;
    sqlx::query("INSERT INTO config_context_binding (id,context_id,scope_type,scope_id) VALUES ($1,$2,'role',$3)")
        .bind(binding).bind(sentinel).bind(role).execute(&pool).await?;
    let before: (String, serde_json::Value, bool, i32, String, Uuid) = sqlx::query_as(
        "SELECT c.name::text,c.data,c.is_active,c.weight,b.scope_type::text,b.scope_id FROM config_context c JOIN config_context_binding b ON b.context_id=c.id WHERE c.id=$1 AND b.id=$2",
    ).bind(sentinel).bind(binding).fetch_one(&pool).await?;

    for (case, expected) in [
        (
            "inert",
            "config context has a previously inert binding that requires review",
        ),
        (
            "unsupported",
            "config context has a previously inert binding that requires review",
        ),
        (
            "mixed",
            "active config contexts with no bindings or multiple categories must be reviewed and deactivated before upgrade",
        ),
        (
            "unbound",
            "active config contexts with no bindings or multiple categories must be reviewed and deactivated before upgrade",
        ),
        ("data", "config context data must be a JSON object"),
        (
            "weight",
            "config context weight must be between 0 and 32767",
        ),
        (
            "weight-high",
            "config context weight must be between 0 and 32767",
        ),
        (
            "target",
            "config context has an unsupported or missing binding target",
        ),
        (
            "cluster-device",
            "active cluster config contexts with physical device members must be reviewed and deactivated before upgrade",
        ),
        ("cluster-empty", ""),
        ("cluster-inactive", ""),
    ] {
        let id = Uuid::now_v7();
        let scope_id = Uuid::now_v7();
        let mut tx = pool.begin().await?;
        sqlx::raw_sql("ALTER TABLE config_context DROP CONSTRAINT config_context_data_object, DROP CONSTRAINT config_context_weight_range; DROP FUNCTION config_context_scope_exists(text,uuid); UPDATE config_context SET is_active=false;").execute(&mut *tx).await?;
        sqlx::query("DELETE FROM config_context_binding WHERE context_id <> $1")
            .bind(sentinel)
            .execute(&mut *tx)
            .await?;
        let data = if case == "data" { json!([]) } else { json!({}) };
        let weight = match case {
            "weight" => -1,
            "weight-high" => 32768,
            _ => 1000,
        };
        sqlx::query(
            "INSERT INTO config_context (id,name,data,is_active,weight) VALUES ($1,$2,$3,$4,$5)",
        )
        .bind(id)
        .bind(id.to_string())
        .bind(data)
        .bind(matches!(
            case,
            "unbound" | "mixed" | "cluster-device" | "cluster-empty"
        ))
        .bind(weight)
        .execute(&mut *tx)
        .await?;
        if case == "inert" {
            sqlx::query("INSERT INTO inventory_tag (id,name,slug) VALUES ($1,$2,$2)")
                .bind(scope_id)
                .bind(scope_id.to_string())
                .execute(&mut *tx)
                .await?;
            sqlx::query("INSERT INTO config_context_binding (id,context_id,scope_type,scope_id) VALUES ($1,$2,'tag',$3)")
                .bind(Uuid::now_v7()).bind(id).bind(scope_id).execute(&mut *tx).await?;
        } else if case == "unsupported" {
            sqlx::query("INSERT INTO config_context_binding (id,context_id,scope_type,scope_id) VALUES ($1,$2,'unsupported',$3)")
                .bind(Uuid::now_v7()).bind(id).bind(scope_id).execute(&mut *tx).await?;
        } else if case == "mixed" {
            sqlx::query("INSERT INTO virtualization_virtualmachine (id,name,custom_field_data,status,comments,description,interface_count,virtual_disk_count,serial,start_on_boot) VALUES ($1,$2,'{}','active','','',0,0,'','default')")
                .bind(scope_id).bind(scope_id.to_string()).execute(&mut *tx).await?;
            for (kind, target) in [("role", role), ("vm", scope_id)] {
                sqlx::query("INSERT INTO config_context_binding (id,context_id,scope_type,scope_id) VALUES ($1,$2,$3,$4)")
                    .bind(Uuid::now_v7()).bind(id).bind(kind).bind(target).execute(&mut *tx).await?;
            }
        } else if case == "target" {
            sqlx::query("INSERT INTO config_context_binding (id,context_id,scope_type,scope_id) VALUES ($1,$2,'tenant',$3)")
                .bind(Uuid::now_v7()).bind(id).bind(scope_id).execute(&mut *tx).await?;
        } else if matches!(
            case,
            "cluster-device" | "cluster-empty" | "cluster-inactive"
        ) {
            let type_id = Uuid::now_v7();
            sqlx::query("INSERT INTO virtualization_clustertype (id,custom_field_data,name,slug,description,comments) VALUES ($1,'{}',$2,$2,'','')")
                .bind(type_id).bind(type_id.to_string()).execute(&mut *tx).await?;
            sqlx::query("INSERT INTO virtualization_cluster (id,custom_field_data,name,comments,type_id,status,description) VALUES ($1,'{}',$2,'',$3,'active','')")
                .bind(scope_id).bind(scope_id.to_string()).bind(type_id).execute(&mut *tx).await?;
            sqlx::query("INSERT INTO config_context_binding (id,context_id,scope_type,scope_id) VALUES ($1,$2,'cluster',$3)")
                .bind(Uuid::now_v7()).bind(id).bind(scope_id).execute(&mut *tx).await?;
            if case != "cluster-empty" {
                let site_id = Uuid::now_v7();
                let device_id = Uuid::now_v7();
                sqlx::query("INSERT INTO dcim_site (id,custom_field_data,name,slug,status,facility,description,physical_address,shipping_address,comments) VALUES ($1,'{}',$2,$2,'active','','','','','')")
                    .bind(site_id).bind(site_id.to_string()).execute(&mut *tx).await?;
                let inserted = sqlx::query("INSERT INTO dcim_device (id,name,serial,status,comments,role_id,device_type_id,site_id,cluster_id,description,custom_field_data,console_port_count,console_server_port_count,power_port_count,power_outlet_count,interface_count,front_port_count,rear_port_count,device_bay_count,module_bay_count,inventory_item_count) SELECT $1,$2,'','active','',$3,(SELECT id FROM dcim_devicetype ORDER BY id LIMIT 1),$4,$5,'','{}',0,0,0,0,0,0,0,0,0,0")
                    .bind(device_id).bind(device_id.to_string()).bind(role).bind(site_id).bind(scope_id).execute(&mut *tx).await?;
                assert_eq!(
                    inserted.rows_affected(),
                    1,
                    "case {case} needs a device member"
                );
            }
        }
        let result = sqlx::raw_sql(include_str!(
            "../migrations/0019_config_context_semantics.sql"
        ))
        .execute(&mut *tx)
        .await;
        if expected.is_empty() {
            assert!(result.is_ok(), "case {case}: {result:?}");
        } else {
            let error = result.expect_err("migration must reject fixture");
            let database_error = error
                .as_database_error()
                .expect("guard raises a PostgreSQL error");
            assert_eq!(
                database_error.code().as_deref(),
                Some("P0001"),
                "case {case}"
            );
            assert_eq!(database_error.message(), expected, "case {case}");
        }
        tx.rollback().await?;
        let after: (String, serde_json::Value, bool, i32, String, Uuid) = sqlx::query_as(
            "SELECT c.name::text,c.data,c.is_active,c.weight,b.scope_type::text,b.scope_id FROM config_context c JOIN config_context_binding b ON b.context_id=c.id WHERE c.id=$1 AND b.id=$2",
        ).bind(sentinel).bind(binding).fetch_one(&pool).await?;
        assert_eq!(after, before, "case {case} changed existing rows");
        let leaked: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM config_context WHERE id = $1)")
                .bind(id)
                .fetch_one(&pool)
                .await?;
        assert!(!leaked, "case {case} leaked its fixture");
        let schema_restored: bool = sqlx::query_scalar(
            "SELECT to_regprocedure('config_context_scope_exists(text,uuid)') IS NOT NULL AND (SELECT count(*)=2 FROM pg_constraint WHERE conrelid='config_context'::regclass AND conname IN ('config_context_data_object','config_context_weight_range'))",
        ).fetch_one(&pool).await?;
        assert!(schema_restored, "case {case} did not roll back DDL");
    }
    sqlx::query("DELETE FROM config_context WHERE id=$1")
        .bind(sentinel)
        .execute(&pool)
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a migrated PostgreSQL database"]
async fn vm_matching_tracks_tags_and_combines_categories() -> Result<(), Box<dyn std::error::Error>>
{
    let pool = sqlx::PgPool::connect(&std::env::var("PIPING_DATABASE__URL")?).await?;
    let repo = SqlxConfigContextRepository::new(pool.clone());
    let vm = Uuid::now_v7();
    let other_vm = Uuid::now_v7();
    let device = Uuid::now_v7();
    let site = Uuid::now_v7();
    let site_tenant = Uuid::now_v7();
    let tag1 = Uuid::now_v7();
    let tag2 = Uuid::now_v7();
    let global = Uuid::now_v7();
    let matched = Uuid::now_v7();
    let unmatched = Uuid::now_v7();
    let inactive = Uuid::now_v7();
    let wrong_vm = Uuid::now_v7();
    let device_context = Uuid::now_v7();
    let site_tenant_context = Uuid::now_v7();
    for tag in [tag1, tag2] {
        sqlx::query("INSERT INTO inventory_tag (id,name,slug) VALUES ($1,$2,$2)")
            .bind(tag)
            .bind(tag.to_string())
            .execute(&pool)
            .await?;
    }
    sqlx::query("INSERT INTO virtualization_virtualmachine (id, name, custom_field_data, status, comments, description, interface_count, virtual_disk_count, serial, start_on_boot, tags, local_context_data) VALUES ($1,$2,'{}','active','','',0,0,'','default',$3,'{\"winner\":\"local\"}')")
        .bind(vm).bind(vm.to_string()).bind(vec![tag1.to_string()]).execute(&pool).await?;
    sqlx::query("INSERT INTO virtualization_virtualmachine (id, name, custom_field_data, status, comments, description, interface_count, virtual_disk_count, serial, start_on_boot) VALUES ($1,$2,'{}','active','','',0,0,'','default')")
        .bind(other_vm).bind(other_vm.to_string()).execute(&pool).await?;
    sqlx::query("INSERT INTO dcim_site (id,custom_field_data,name,slug,status,facility,description,physical_address,shipping_address,comments) VALUES ($1,'{}',$2,$2,'active','','','','','')")
        .bind(site).bind(site.to_string()).execute(&pool).await?;
    sqlx::query("INSERT INTO tenancy_tenant (id,custom_field_data,name,slug,description,comments) VALUES ($1,'{}',$2,$2,'','')")
        .bind(site_tenant).bind(site_tenant.to_string()).execute(&pool).await?;
    sqlx::query("UPDATE dcim_site SET tenant_id=$2 WHERE id=$1")
        .bind(site)
        .bind(site_tenant)
        .execute(&pool)
        .await?;
    sqlx::query("UPDATE virtualization_virtualmachine SET site_id=$2 WHERE id=$1")
        .bind(vm)
        .bind(site)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO dcim_device (id,name,serial,status,comments,role_id,device_type_id,site_id,description,custom_field_data,console_port_count,console_server_port_count,power_port_count,power_outlet_count,interface_count,front_port_count,rear_port_count,device_bay_count,module_bay_count,inventory_item_count,tags) SELECT $1,$2,'','active','',(SELECT id FROM dcim_devicerole ORDER BY id LIMIT 1),(SELECT id FROM dcim_devicetype ORDER BY id LIMIT 1),$3,'','{}',0,0,0,0,0,0,0,0,0,0,$4")
        .bind(device).bind(device.to_string()).bind(site).bind(vec![tag1.to_string()]).execute(&pool).await?;
    for (id, name, active) in [
        (global, "a-global", true),
        (matched, "b-match", true),
        (unmatched, "c-miss", true),
        (inactive, "d-inactive", false),
        (wrong_vm, "e-wrong-vm", true),
        (device_context, "f-device", true),
        (site_tenant_context, "g-site-tenant", true),
    ] {
        sqlx::query(
            "INSERT INTO config_context (id,name,weight,data,is_active) VALUES ($1,$2,10,$3,$4)",
        )
        .bind(id)
        .bind(name)
        .bind(json!({"winner":name}))
        .bind(active)
        .execute(&pool)
        .await?;
    }
    for (scope_type, scope_id) in [("unknown", tag1), ("tag", Uuid::nil())] {
        let binding = ConfigContextBinding {
            id: Uuid::now_v7(),
            context_id: matched,
            scope_type: scope_type.into(),
            scope_id,
        };
        assert!(repo.create_binding(binding).await.is_err());
    }
    for (context, kind, scope) in [
        (matched, "tag", tag1),
        (matched, "tag", tag2),
        (matched, "vm", vm),
        (unmatched, "tag", tag2),
        (unmatched, "vm", vm),
        (wrong_vm, "tag", tag1),
        (wrong_vm, "vm", other_vm),
        (device_context, "tag", tag1),
        (device_context, "device", device),
        (site_tenant_context, "tenant", site_tenant),
    ] {
        repo.create_binding(ConfigContextBinding {
            id: Uuid::now_v7(),
            context_id: context,
            scope_type: kind.into(),
            scope_id: scope,
        })
        .await?;
    }
    let deletion = delete_inventory_record(&pool, "inventory_tag", tag2).await;
    assert!(matches!(
        deletion,
        Err(piping_backend::errors::AppError::DeleteBlocked(_))
    ));
    let list = repo.list_for_scope_hierarchy("vm", vm).await?;
    let ids: Vec<_> = list.iter().map(|c| c.id).collect();
    assert!(ids.contains(&global) && ids.contains(&matched));
    assert!(!ids.contains(&unmatched) && !ids.contains(&inactive) && !ids.contains(&wrong_vm));
    assert!(!ids.contains(&site_tenant_context));
    assert!(ids.iter().position(|id| *id == global) < ids.iter().position(|id| *id == matched));
    let target_ids: Vec<_> = repo
        .list_for_scope_hierarchy("connection_target", vm)
        .await?
        .iter()
        .map(|c| c.id)
        .collect();
    assert!(target_ids.contains(&matched) && !target_ids.contains(&unmatched));
    let target_contexts = repo
        .list_for_scope_hierarchy("connection_target", vm)
        .await?;
    let rendered =
        piping_backend::contexts::inventory::application::resolve_config_contexts_ordered(
            &target_contexts,
        );
    assert_eq!(rendered["winner"], "local");
    let device_ids: Vec<_> = repo
        .list_for_scope_hierarchy("device", device)
        .await?
        .iter()
        .map(|c| c.id)
        .collect();
    let device_target_ids: Vec<_> = repo
        .list_for_scope_hierarchy("connection_target", device)
        .await?
        .iter()
        .map(|c| c.id)
        .collect();
    assert!(device_ids.contains(&device_context) && device_target_ids.contains(&device_context));
    sqlx::query("UPDATE virtualization_virtualmachine SET tags = $2 WHERE id = $1")
        .bind(vm)
        .bind(Vec::<String>::new())
        .execute(&pool)
        .await?;
    let ids: Vec<_> = repo
        .list_for_scope_hierarchy("vm", vm)
        .await?
        .iter()
        .map(|c| c.id)
        .collect();
    assert!(!ids.contains(&matched) && ids.contains(&global));
    let target_ids: Vec<_> = repo
        .list_for_scope_hierarchy("connection_target", vm)
        .await?
        .iter()
        .map(|c| c.id)
        .collect();
    assert!(!target_ids.contains(&matched) && target_ids.contains(&global));
    sqlx::query("UPDATE dcim_device SET tags = $2 WHERE id = $1")
        .bind(device)
        .bind(Vec::<String>::new())
        .execute(&pool)
        .await?;
    let device_ids: Vec<_> = repo
        .list_for_scope_hierarchy("device", device)
        .await?
        .iter()
        .map(|c| c.id)
        .collect();
    assert!(!device_ids.contains(&device_context) && device_ids.contains(&global));
    assert_criteria_matrix(&pool, &repo, vm, device, site).await?;
    sqlx::query("DELETE FROM config_context WHERE id = ANY($1)")
        .bind(vec![
            global,
            matched,
            unmatched,
            inactive,
            wrong_vm,
            device_context,
            site_tenant_context,
        ])
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM virtualization_virtualmachine WHERE id = ANY($1)")
        .bind(vec![vm, other_vm])
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM dcim_device WHERE id = $1")
        .bind(device)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM dcim_site WHERE id = $1")
        .bind(site)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM tenancy_tenant WHERE id = $1")
        .bind(site_tenant)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM inventory_tag WHERE id = ANY($1)")
        .bind(vec![tag1, tag2])
        .execute(&pool)
        .await?;
    Ok(())
}

async fn assert_criteria_matrix(
    pool: &sqlx::PgPool,
    repo: &SqlxConfigContextRepository,
    vm: Uuid,
    device: Uuid,
    site: Uuid,
) -> Result<(), Box<dyn std::error::Error>> {
    let region = Uuid::now_v7();
    let region_child = Uuid::now_v7();
    let site_group = Uuid::now_v7();
    let site_group_child = Uuid::now_v7();
    let location = Uuid::now_v7();
    let location_child = Uuid::now_v7();
    let tenant_group = Uuid::now_v7();
    let tenant_group_child = Uuid::now_v7();
    let tenant = Uuid::now_v7();
    let platform = Uuid::now_v7();
    let cluster_type = Uuid::now_v7();
    let cluster_group = Uuid::now_v7();
    let cluster = Uuid::now_v7();
    for (id, parent) in [(region, None), (region_child, Some(region))] {
        sqlx::query("INSERT INTO dcim_region (id,custom_field_data,name,slug,description,lft,rght,tree_id,level,comments,parent_id) VALUES ($1,'{}',$2,$2,'',0,0,0,0,'',$3)")
            .bind(id).bind(id.to_string()).bind(parent).execute(pool).await?;
    }
    for (id, parent) in [(site_group, None), (site_group_child, Some(site_group))] {
        sqlx::query("INSERT INTO dcim_sitegroup (id,custom_field_data,name,slug,description,lft,rght,tree_id,level,comments,parent_id) VALUES ($1,'{}',$2,$2,'',0,0,0,0,'',$3)")
            .bind(id).bind(id.to_string()).bind(parent).execute(pool).await?;
    }
    for (id, parent) in [(location, None), (location_child, Some(location))] {
        sqlx::query("INSERT INTO dcim_location (id,custom_field_data,name,slug,description,lft,rght,tree_id,level,site_id,status,facility,comments,parent_id) VALUES ($1,'{}',$2,$2,'',0,0,0,0,$3,'active','','',$4)")
            .bind(id).bind(id.to_string()).bind(site).bind(parent).execute(pool).await?;
    }
    for (id, parent) in [
        (tenant_group, None),
        (tenant_group_child, Some(tenant_group)),
    ] {
        sqlx::query("INSERT INTO tenancy_tenantgroup (id,custom_field_data,name,slug,description,lft,rght,tree_id,level,comments,parent_id) VALUES ($1,'{}',$2,$2,'',0,0,0,0,'',$3)")
            .bind(id).bind(id.to_string()).bind(parent).execute(pool).await?;
    }
    sqlx::query("INSERT INTO tenancy_tenant (id,custom_field_data,name,slug,description,comments,group_id) VALUES ($1,'{}',$2,$2,'','',$3)")
        .bind(tenant).bind(tenant.to_string()).bind(tenant_group_child).execute(pool).await?;
    sqlx::query("INSERT INTO dcim_platform (id,custom_field_data,name,slug,description,level,lft,rght,tree_id,comments) VALUES ($1,'{}',$2,$2,'',0,0,0,0,'')")
        .bind(platform).bind(platform.to_string()).execute(pool).await?;
    sqlx::query("INSERT INTO virtualization_clustertype (id,custom_field_data,name,slug,description,comments) VALUES ($1,'{}',$2,$2,'','')")
        .bind(cluster_type).bind(cluster_type.to_string()).execute(pool).await?;
    sqlx::query("INSERT INTO virtualization_clustergroup (id,custom_field_data,name,slug,description,comments) VALUES ($1,'{}',$2,$2,'','')")
        .bind(cluster_group).bind(cluster_group.to_string()).execute(pool).await?;
    sqlx::query("INSERT INTO virtualization_cluster (id,custom_field_data,name,comments,type_id,status,description,group_id) VALUES ($1,'{}',$2,'',$3,'active','',$4)")
        .bind(cluster).bind(cluster.to_string()).bind(cluster_type).bind(cluster_group).execute(pool).await?;
    let (role, device_type): (Uuid, Uuid) =
        sqlx::query_as("SELECT role_id,device_type_id FROM dcim_device WHERE id=$1")
            .bind(device)
            .fetch_one(pool)
            .await?;
    sqlx::query("UPDATE dcim_site SET region_id=$2,group_id=$3 WHERE id=$1")
        .bind(site)
        .bind(region_child)
        .bind(site_group_child)
        .execute(pool)
        .await?;
    sqlx::query("UPDATE dcim_device SET location_id=$2,platform_id=$3,cluster_id=$4,tenant_id=$5 WHERE id=$1")
        .bind(device).bind(location_child).bind(platform).bind(cluster).bind(tenant).execute(pool).await?;
    sqlx::query("UPDATE virtualization_virtualmachine SET role_id=$2,platform_id=$3,cluster_id=$4,tenant_id=$5 WHERE id=$1")
        .bind(vm).bind(role).bind(platform).bind(cluster).bind(tenant).execute(pool).await?;

    let criteria = [
        ("region", region, true, true),
        ("site_group", site_group, true, true),
        ("site", site, true, true),
        ("location", location, true, false),
        ("device_type", device_type, true, false),
        ("role", role, true, true),
        ("platform", platform, true, true),
        ("cluster_type", cluster_type, true, true),
        ("cluster_group", cluster_group, true, true),
        ("cluster", cluster, true, true),
        ("tenant_group", tenant_group_child, true, true),
        ("tenant_group", tenant_group, false, false),
        ("tenant", tenant, true, true),
    ];
    let mut contexts = Vec::new();
    for (kind, scope_id, device_matches, vm_matches) in criteria {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO config_context (id,name,data,is_active,weight) VALUES ($1,$2,'{}',true,1000)")
            .bind(id).bind(id.to_string()).execute(pool).await?;
        repo.create_binding(ConfigContextBinding {
            id: Uuid::now_v7(),
            context_id: id,
            scope_type: kind.into(),
            scope_id,
        })
        .await?;
        contexts.push((kind, id, device_matches, vm_matches));
    }
    let device_ids: Vec<_> = repo
        .list_for_scope_hierarchy("device", device)
        .await?
        .into_iter()
        .map(|c| c.id)
        .collect();
    let vm_ids: Vec<_> = repo
        .list_for_scope_hierarchy("vm", vm)
        .await?
        .into_iter()
        .map(|c| c.id)
        .collect();
    let device_target_ids: Vec<_> = repo
        .list_for_scope_hierarchy("connection_target", device)
        .await?
        .into_iter()
        .map(|c| c.id)
        .collect();
    let vm_target_ids: Vec<_> = repo
        .list_for_scope_hierarchy("connection_target", vm)
        .await?
        .into_iter()
        .map(|c| c.id)
        .collect();
    for (kind, id, device_matches, vm_matches) in &contexts {
        assert_eq!(
            device_ids.contains(id),
            *device_matches,
            "device criterion {kind}"
        );
        assert_eq!(
            device_target_ids.contains(id),
            *device_matches,
            "device target criterion {kind}"
        );
        assert_eq!(vm_ids.contains(id), *vm_matches, "VM criterion {kind}");
        assert_eq!(
            vm_target_ids.contains(id),
            *vm_matches,
            "VM target criterion {kind}"
        );
    }
    let ids: Vec<_> = contexts.iter().map(|(_, id, _, _)| *id).collect();
    sqlx::query("DELETE FROM config_context WHERE id=ANY($1)")
        .bind(ids)
        .execute(pool)
        .await?;
    sqlx::query("UPDATE dcim_device SET location_id=NULL,platform_id=NULL,cluster_id=NULL,tenant_id=NULL WHERE id=$1").bind(device).execute(pool).await?;
    sqlx::query("UPDATE virtualization_virtualmachine SET role_id=NULL,platform_id=NULL,cluster_id=NULL,tenant_id=NULL WHERE id=$1").bind(vm).execute(pool).await?;
    sqlx::query("UPDATE dcim_site SET region_id=NULL,group_id=NULL WHERE id=$1")
        .bind(site)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM virtualization_cluster WHERE id=$1")
        .bind(cluster)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM virtualization_clustertype WHERE id=$1")
        .bind(cluster_type)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM virtualization_clustergroup WHERE id=$1")
        .bind(cluster_group)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM dcim_platform WHERE id=$1")
        .bind(platform)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM tenancy_tenant WHERE id=$1")
        .bind(tenant)
        .execute(pool)
        .await?;
    for id in [tenant_group_child, tenant_group] {
        sqlx::query("DELETE FROM tenancy_tenantgroup WHERE id=$1")
            .bind(id)
            .execute(pool)
            .await?;
    }
    for id in [location_child, location] {
        sqlx::query("DELETE FROM dcim_location WHERE id=$1")
            .bind(id)
            .execute(pool)
            .await?;
    }
    for id in [site_group_child, site_group] {
        sqlx::query("DELETE FROM dcim_sitegroup WHERE id=$1")
            .bind(id)
            .execute(pool)
            .await?;
    }
    for id in [region_child, region] {
        sqlx::query("DELETE FROM dcim_region WHERE id=$1")
            .bind(id)
            .execute(pool)
            .await?;
    }
    Ok(())
}
