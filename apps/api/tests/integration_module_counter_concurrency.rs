//! Native observation of shared-type counters across independent device graphs.
//! Run only against an owned PostgreSQL fixture after applying the current chain.

use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

type TestError = Box<dyn std::error::Error + Send + Sync>;

async fn counter_fixture_pool() -> Result<PgPool, TestError> {
    let url = std::env::var("PIPING_DATABASE__URL")
        .map_err(|_| "select an owned PostgreSQL fixture through PIPING_DATABASE__URL")?;
    // Two competing transactions must not exhaust the observer/probe capacity.
    let db = PgPoolOptions::new()
        .max_connections(4)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .connect(&url)
        .await?;
    piping_backend::storage::database::migrations::run_migrations(&db).await?;
    Ok(db)
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL; observes actual row-lock synchronization"]
async fn concurrent_device_uninstalls_preserve_shared_type_count() -> Result<(), TestError> {
    let db = counter_fixture_pool().await?;
    let site = Uuid::now_v7();
    let module_type = Uuid::now_v7();
    let devices = [Uuid::now_v7(), Uuid::now_v7()];
    let bays = [Uuid::now_v7(), Uuid::now_v7()];
    let modules = [Uuid::now_v7(), Uuid::now_v7()];
    let name = format!("uninstall-race-{site}");

    let observation: Result<(i64, i64), TestError> = async {
        sqlx::query("INSERT INTO dcim_site (id,name,slug,status,facility,description,physical_address,shipping_address,comments,custom_field_data) VALUES ($1,$2,$2,'active','','','','','','{}')")
            .bind(site).bind(&name).execute(&db).await?;
        let inserted = sqlx::query("INSERT INTO dcim_moduletype (id,manufacturer_id,model,part_number,description,comments,custom_field_data,module_count) SELECT $1,id,$2,'','','','{}',0 FROM dcim_manufacturer ORDER BY id LIMIT 1")
            .bind(module_type).bind(&name).execute(&db).await?;
        if inserted.rows_affected() != 1 {
            return Err("the isolated fixture has no manufacturer".into());
        }
        for index in 0..2 {
            sqlx::query("INSERT INTO dcim_device (id,name,serial,status,comments,role_id,device_type_id,site_id,description,custom_field_data,console_port_count,console_server_port_count,power_port_count,power_outlet_count,interface_count,front_port_count,rear_port_count,device_bay_count,module_bay_count,inventory_item_count) SELECT $1,$2,'','active','',(SELECT id FROM dcim_devicerole ORDER BY id LIMIT 1),(SELECT id FROM dcim_devicetype ORDER BY id LIMIT 1),$3,'','{}',0,0,0,0,0,0,0,0,0,0")
                .bind(devices[index]).bind(format!("{name}-{index}")).bind(site).execute(&db).await?;
            sqlx::query("INSERT INTO dcim_modulebay (id,name,label,position,description,device_id,custom_field_data,level,lft,rght,tree_id) VALUES ($1,'slot','','','',$2,'{}',0,0,0,0)")
                .bind(bays[index]).bind(devices[index]).execute(&db).await?;
            sqlx::query("INSERT INTO dcim_module (id,device_id,module_bay_id,module_type_id,serial,status,description,comments,custom_field_data) VALUES ($1,$2,$3,$4,'','active','','','{}')")
                .bind(modules[index]).bind(devices[index]).bind(bays[index]).bind(module_type).execute(&db).await?;
        }
        let initial: i64 = sqlx::query_scalar("SELECT module_count FROM dcim_moduletype WHERE id=$1")
            .bind(module_type).fetch_one(&db).await?;
        if initial != 2 {
            return Err(format!("fixture counter was {initial}, expected two installed modules").into());
        }

        let mut first = db.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
            .execute(&mut *first).await?;
        let first_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *first).await?;
        sqlx::query("DELETE FROM dcim_module WHERE id=$1")
            .bind(modules[0]).execute(&mut *first).await?;
        let mut second = db.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
            .execute(&mut *second).await?;
        let second_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *second).await?;
        let mut pending = tokio::spawn(async move {
            sqlx::query("DELETE FROM dcim_module WHERE id=$1")
                .bind(modules[1]).execute(&mut *second).await?;
            second.commit().await
        });

        // The device rows differ. Observe a real shared graph/type blocker,
        // not an elapsed delay or an assumption about scheduler ordering.
        let blocked = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let waiting: bool = sqlx::query_scalar("SELECT $1=ANY(pg_blocking_pids($2))")
                    .bind(first_pid).bind(second_pid).fetch_one(&db).await?;
                if waiting {
                    return Ok::<(), sqlx::Error>(());
                }
                tokio::task::yield_now().await;
            }
        }).await;
        if !matches!(blocked, Ok(Ok(()))) {
            first.rollback().await?;
            abort_delete(pending).await;
            return Err("second uninstall did not reach the observed shared graph/type lock".into());
        }
        if let Err(error) = first.commit().await {
            abort_delete(pending).await;
            return Err(error.into());
        }
        match tokio::time::timeout(std::time::Duration::from_secs(10), &mut pending).await {
            Ok(joined) => joined??,
            Err(error) => {
                abort_delete(pending).await;
                return Err(error.into());
            }
        }
        let state = sqlx::query_as::<_, (i64, i64)>("SELECT module_count,(SELECT COUNT(*) FROM dcim_module WHERE module_type_id=$1) FROM dcim_moduletype WHERE id=$1")
            .bind(module_type).fetch_one(&db).await?;
        Ok(state)
    }.await;

    // Cleanup precedes the assertion so even a reproduced stale counter does
    // not leave synthetic inventory in the next focused test's fixture.
    cleanup(&db, &modules, &bays, &devices, module_type, site).await?;
    assert_eq!(
        observation?,
        (0, 0),
        "committed module_count must equal the committed graph"
    );
    Ok(())
}

async fn abort_delete(pending: tokio::task::JoinHandle<Result<(), sqlx::Error>>) {
    pending.abort();
    // Cancellation is expected; await termination before deleting fixture rows.
    let _ = pending.await;
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL; observes native install versus guarded uninstall with cooling cascades"]
async fn install_and_component_bearing_uninstall_follow_catalog_graph_resource_order()
-> Result<(), TestError> {
    let db = counter_fixture_pool().await?;
    let site = Uuid::now_v7();
    let module_type = Uuid::now_v7();
    let devices = [Uuid::now_v7(), Uuid::now_v7()];
    let bays = [Uuid::now_v7(), Uuid::now_v7()];
    let modules = [Uuid::now_v7(), Uuid::now_v7()];
    let intake_template = Uuid::now_v7();
    let outflow_template = Uuid::now_v7();
    let name = format!("install-delete-order-{site}");
    let observation:Result<(i64,i64,i64,i64),TestError>=async {
        sqlx::query("INSERT INTO dcim_site (id,name,slug,status,facility,description,physical_address,shipping_address,comments,custom_field_data) VALUES ($1,$2,$2,'active','','','','','','{}')")
            .bind(site).bind(&name).execute(&db).await?;
        let inserted=sqlx::query("INSERT INTO dcim_moduletype (id,manufacturer_id,model,part_number,description,comments,custom_field_data,module_count) SELECT $1,id,$2,'','','','{}',0 FROM dcim_manufacturer ORDER BY id LIMIT 1")
            .bind(module_type).bind(&name).execute(&db).await?;
        if inserted.rows_affected()!=1 { return Err("fixture manufacturer missing".into()); }
        sqlx::query("INSERT INTO dcim_coolingintaketemplate (id,name,label,description,module_type_id) VALUES ($1,'in','','',$2)")
            .bind(intake_template).bind(module_type).execute(&db).await?;
        sqlx::query("INSERT INTO dcim_coolingoutflowtemplate (id,name,label,description,module_type_id,cooling_intake_id) VALUES ($1,'out','','',$2,$3)")
            .bind(outflow_template).bind(module_type).bind(intake_template).execute(&db).await?;
        for index in 0..2 {
            sqlx::query("INSERT INTO dcim_device (id,name,serial,status,comments,role_id,device_type_id,site_id,description,custom_field_data,console_port_count,console_server_port_count,power_port_count,power_outlet_count,interface_count,front_port_count,rear_port_count,device_bay_count,module_bay_count,inventory_item_count) SELECT $1,$2,'','active','',(SELECT id FROM dcim_devicerole ORDER BY id LIMIT 1),(SELECT id FROM dcim_devicetype ORDER BY id LIMIT 1),$3,'','{}',0,0,0,0,0,0,0,0,0,0")
                .bind(devices[index]).bind(format!("{name}-{index}")).bind(site).execute(&db).await?;
            sqlx::query("INSERT INTO dcim_modulebay (id,name,label,position,description,device_id,custom_field_data,level,lft,rght,tree_id) VALUES ($1,'slot','','','',$2,'{}',0,0,0,0)")
                .bind(bays[index]).bind(devices[index]).execute(&db).await?;
        }
        sqlx::query("INSERT INTO dcim_module (id,device_id,module_bay_id,module_type_id,serial,status,description,comments,custom_field_data) VALUES ($1,$2,$3,$4,'','active','','','{}')")
            .bind(modules[0]).bind(devices[0]).bind(bays[0]).bind(module_type).execute(&db).await?;
        let initial:i64=sqlx::query_scalar("SELECT COUNT(*) FROM dcim_coolingintake WHERE module_id=$1").bind(modules[0]).fetch_one(&db).await?;
        if initial!=1 { return Err("uninstall fixture must contain a real cooling component".into()); }

        let mut installer=db.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL READ COMMITTED").execute(&mut *installer).await?;
        // Hold the actual statement-trigger lock owner, not a type/resource row.
        sqlx::query("SELECT dcim_acquire_hardware_graph()").execute(&mut *installer).await?;
        let installer_pid:i32=sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut *installer).await?;
        let mut uninstall=db.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL READ COMMITTED").execute(&mut *uninstall).await?;
        let uninstall_pid:i32=sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut *uninstall).await?;
        let mut pending=tokio::spawn(async move {
            let deleted=piping_backend::contexts::inventory::infrastructure::persistence::repositories::delete_guard::delete_inventory_record_in_transaction(&mut uninstall,"dcim_module",modules[0]).await?;
            if deleted!=1 { return Err::<(),TestError>("guarded uninstall did not delete its module".into()); }
            uninstall.commit().await?;
            Ok::<(),TestError>(())
        });
        let blocked=tokio::time::timeout(std::time::Duration::from_secs(10),async {
            loop {
                let waiting:bool=sqlx::query_scalar("SELECT $1=ANY(pg_blocking_pids($2))").bind(installer_pid).bind(uninstall_pid).fetch_one(&db).await?;
                if waiting { return Ok::<(),sqlx::Error>(()); }
                tokio::task::yield_now().await;
            }
        }).await;
        if !matches!(blocked,Ok(Ok(()))) {
            pending.abort();
            let _=pending.await; // Await cancellation before touching fixture rows.
            installer.rollback().await?;
            return Err("uninstall did not reach the observed installer graph blocker".into());
        }
        // With the old order the waiting uninstall already held the shared type,
        // so this real INSERT completed a deadlock cycle. It must now succeed.
        let install=tokio::time::timeout(std::time::Duration::from_secs(10),sqlx::query("INSERT INTO dcim_module (id,device_id,module_bay_id,module_type_id,serial,status,description,comments,custom_field_data) VALUES ($1,$2,$3,$4,'','active','','','{}')")
            .bind(modules[1]).bind(devices[1]).bind(bays[1]).bind(module_type).execute(&mut *installer)).await;
        if !matches!(install,Ok(Ok(_))) {
            pending.abort();
            let _=pending.await; // A failed/aborted transaction must not retain a writer.
            installer.rollback().await?;
            return Err("native install failed or deadlocked while uninstall waited for the graph".into());
        }
        if let Err(error)=installer.commit().await {
            pending.abort(); let _=pending.await;
            return Err(error.into());
        }
        match tokio::time::timeout(std::time::Duration::from_secs(10),&mut pending).await {
            Ok(joined)=>joined??,
            Err(error)=>{pending.abort();let _=pending.await;return Err(error.into());}
        }
        let (stored,actual):(i64,i64)=sqlx::query_as("SELECT module_count,(SELECT COUNT(*) FROM dcim_module WHERE module_type_id=$1) FROM dcim_moduletype WHERE id=$1").bind(module_type).fetch_one(&db).await?;
        let removed:i64=sqlx::query_scalar("SELECT COUNT(*) FROM dcim_coolingintake WHERE module_id=$1").bind(modules[0]).fetch_one(&db).await?;
        let inserted:i64=sqlx::query_scalar("SELECT COUNT(*) FROM dcim_coolingintake i JOIN dcim_coolingoutflow o ON o.cooling_intake_id=i.id WHERE i.module_id=$1 AND o.module_id=$1").bind(modules[1]).fetch_one(&db).await?;
        Ok((stored,actual,removed,inserted))
    }.await;
    cleanup(&db, &modules, &bays, &devices, module_type, site).await?;
    assert_eq!(
        observation?,
        (1, 1, 0, 1),
        "install and uninstall must commit, preserving counters and component cascades"
    );
    Ok(())
}

async fn cleanup(
    db: &PgPool,
    modules: &[Uuid],
    bays: &[Uuid],
    devices: &[Uuid],
    module_type: Uuid,
    site: Uuid,
) -> Result<(), TestError> {
    sqlx::query("DELETE FROM dcim_module WHERE id=ANY($1)")
        .bind(modules)
        .execute(db)
        .await?;
    sqlx::query("DELETE FROM dcim_modulebay WHERE id=ANY($1)")
        .bind(bays)
        .execute(db)
        .await?;
    sqlx::query("DELETE FROM dcim_device WHERE id=ANY($1)")
        .bind(devices)
        .execute(db)
        .await?;
    sqlx::query("DELETE FROM dcim_moduletype WHERE id=$1")
        .bind(module_type)
        .execute(db)
        .await?;
    sqlx::query("DELETE FROM dcim_site WHERE id=$1")
        .bind(site)
        .execute(db)
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL; observes Tag-exclusive versus native hardware graph lock order"]
async fn catalog_exclusive_wait_does_not_hold_the_hardware_graph() -> Result<(), TestError> {
    let db = counter_fixture_pool().await?;
    let mut catalog = db.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
        .execute(&mut *catalog)
        .await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('inventory-tag-catalog',0))")
        .execute(&mut *catalog)
        .await?;
    let catalog_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *catalog)
        .await?;
    let mut native = db.begin().await?;
    let native_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *native)
        .await?;
    let mut pending = tokio::spawn(async move {
        sqlx::query("SELECT dcim_acquire_hardware_graph()")
            .execute(&mut *native)
            .await?;
        native.rollback().await
    });
    let observation: Result<(), TestError> = async {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let blocked: bool = sqlx::query_scalar("SELECT $1=ANY(pg_blocking_pids($2))")
                    .bind(catalog_pid)
                    .bind(native_pid)
                    .fetch_one(&db)
                    .await?;
                if blocked {
                    return Ok::<(), sqlx::Error>(());
                }
                tokio::task::yield_now().await;
            }
        })
        .await??;
        let mut probe = db.begin().await?;
        let available: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(780007)")
            .fetch_one(&mut *probe)
            .await?;
        probe.rollback().await?;
        if !available {
            return Err("native Tag waiter held the hardware graph ahead of the catalog".into());
        }
        Ok(())
    }
    .await;
    if let Err(error) = observation {
        abort_delete(pending).await;
        catalog.rollback().await?;
        return Err(error);
    }
    catalog.rollback().await?;
    match tokio::time::timeout(std::time::Duration::from_secs(10), &mut pending).await {
        Ok(joined) => joined??,
        Err(error) => {
            abort_delete(pending).await;
            return Err(error.into());
        }
    }
    Ok(())
}
