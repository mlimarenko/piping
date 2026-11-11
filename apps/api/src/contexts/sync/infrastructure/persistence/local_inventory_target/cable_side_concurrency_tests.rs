//! Transaction-level regressions: native mutations and source preflight share
//! graph ownership, including omitted-side movers and attachment phantoms.

use super::*;
use crate::contexts::inventory::infrastructure::persistence::repositories::dcim::interface_repository::{
    InterfaceRepository, SqlxInterfaceRepository, UpdateInterface,
};
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use std::time::Duration;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

async fn pools() -> Result<(PgPool, PgPool), sqlx::Error> {
    let url = std::env::var("PIPING_DATABASE__URL")
        .expect("requires an explicitly owned PostgreSQL fixture");
    let observer = PgPoolOptions::new()
        .max_connections(3)
        .connect(&url)
        .await?;
    // Pin the mutation to one backend so the blocker predicate names the actual
    // repository transaction, not another connection in a shared pool.
    let native = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await?;
    Ok((observer, native))
}

async fn seed(pool: &PgPool, attached: bool) -> Result<(Uuid, Uuid), AppError> {
    let mut tx = pool.begin().await?;
    lock_hardware_graph(&mut tx).await?;
    let cable: Uuid = sqlx::query_scalar("INSERT INTO dcim_cable (custom_field_data,status,label,color,comments,description,profile) VALUES ('{}','connected','side-lock-regression','123456','','','') RETURNING id")
        .fetch_one(&mut *tx).await?;
    let name = format!("side-lock-{}", Uuid::now_v7());
    let endpoint: Uuid = sqlx::query_scalar("INSERT INTO dcim_interface (custom_field_data,name,_name,label,description,mark_connected,enabled,type,mgmt_only,cable_id,cable_end) VALUES ('{}',$1,$1,'','',false,true,'1000base-t',false,$2,$3) RETURNING id")
        .bind(name)
        .bind(attached.then_some(cable))
        .bind(attached.then_some("B"))
        .fetch_one(&mut *tx).await?;
    tx.commit().await?;
    Ok((cable, endpoint))
}

fn empty_selected_side(cable: Uuid) -> (LocalInventoryBatch, HashMap<(String, String), Uuid>) {
    let provider = Uuid::now_v7();
    let identity = SourceIdentity {
        source_provider_id: provider,
        entity: SourceEntity::new(InventoryEntity::Cable.source_key())
            .expect("canonical Cable source entity"),
        external_id: "selected-cable".into(),
    };
    let ids = HashMap::from([(
        (identity.entity.to_string(), identity.external_id.clone()),
        cable,
    )]);
    (
        LocalInventoryBatch {
            source_provider_id: provider,
            cable_sides: vec![SideAssertion {
                cable: identity,
                end: InterfaceCableEnd::A,
                interfaces: Vec::new(),
            }],
            ..LocalInventoryBatch::default()
        },
        ids,
    )
}

async fn wait_for_graph_blocker(pool: &PgPool, holder: i32, writer: i32) -> TestResult {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let blocked: bool = sqlx::query_scalar(
                "SELECT $1 = ANY(pg_blocking_pids($2)) AND EXISTS (SELECT 1 FROM pg_locks WHERE pid=$2 AND locktype='advisory' AND NOT granted)",
            )
            .bind(holder)
            .bind(writer)
            .fetch_one(pool)
            .await?;
            if blocked {
                return Ok::<(), sqlx::Error>(());
            }
            tokio::task::yield_now().await;
        }
    })
    .await??;
    Ok(())
}

async fn assert_empty(
    tx: &mut Transaction<'_, Postgres>,
    batch: &LocalInventoryBatch,
    ids: &HashMap<(String, String), Uuid>,
) -> TestResult {
    let (findings, complete) = assess_target(batch, &[], ids, tx).await?;
    assert!(complete && findings.is_empty(), "{findings:?}");
    Ok(())
}

async fn assert_final_and_cleanup(pool: &PgPool, cable: Uuid, endpoint: Uuid) -> TestResult {
    let mut tx = pool.begin().await?;
    lock_hardware_graph(&mut tx).await?;
    let association: (Option<Uuid>, Option<String>) =
        sqlx::query_as("SELECT cable_id,cable_end::text FROM dcim_interface WHERE id=$1")
            .bind(endpoint)
            .fetch_one(&mut *tx)
            .await?;
    assert_eq!(association, (Some(cable), Some("A".into())));
    // A later preflight must observe the serialized mutation, not keep treating
    // the earlier selected-empty assertion as a reservation after its commit.
    let (batch, ids) = empty_selected_side(cable);
    let (findings, complete) = assess_target(&batch, &[], &ids, &mut tx).await?;
    assert!(!complete);
    assert!(findings.iter().any(|finding| finding.code
        == FindingCode::AnalysisDependencyUnavailable
        && finding.field_path.as_deref() == Some("a_terminations")));
    sqlx::query("DELETE FROM dcim_interface WHERE id=$1")
        .bind(endpoint)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM dcim_cable WHERE id=$1")
        .bind(cable)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires an owned PostgreSQL fixture with integrated schema; commits isolated native rows and removes them on success"]
async fn source_preflight_serializes_native_interface_omitted_b_to_selected_a() -> TestResult {
    let (pool, native) = pools().await?;
    let (cable, endpoint) = seed(&pool, true).await?;
    let writer: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&native)
        .await?;
    let mut source = pool.begin().await?;
    // This is the commit boundary's canonical lock, held before preflight and
    // retained until commit, not a row lock on the incoming Interface cohort.
    lock_hardware_graph(&mut source).await?;
    let holder: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *source)
        .await?;
    let (batch, ids) = empty_selected_side(cable);
    assert_empty(&mut source, &batch, &ids).await?;
    let repository = SqlxInterfaceRepository::new(native.clone());
    let changes = UpdateInterface {
        networking: serde_json::from_value(json!({"cable_end": "A"}))?,
        name: None,
        type_: None,
        label: None,
        description: None,
        enabled: None,
        mgmt_only: None,
        wireless_link_id: None,
        wireless_lans: None,
    };
    let release = async {
        let observed = wait_for_graph_blocker(&pool, holder, writer).await;
        if observed.is_ok() {
            assert_empty(&mut source, &batch, &ids).await?;
            let end: String =
                sqlx::query_scalar("SELECT cable_end::text FROM dcim_interface WHERE id=$1")
                    .bind(endpoint)
                    .fetch_one(&mut *source)
                    .await?;
            assert_eq!(end, "B", "native side change must still be blocked");
        }
        source.commit().await?;
        observed
    };
    let mutation = tokio::time::timeout(
        Duration::from_secs(10),
        repository.update(endpoint, &changes),
    );
    let (released, updated) = tokio::join!(release, mutation);
    released?;
    assert!(updated??.is_some());
    assert_final_and_cleanup(&pool, cable, endpoint).await
}

#[tokio::test]
#[ignore = "requires an owned PostgreSQL fixture with integrated schema; commits isolated native rows and removes them on success"]
async fn source_preflight_serializes_incoming_interface_attachment_phantom() -> TestResult {
    let (pool, incoming) = pools().await?;
    let (cable, endpoint) = seed(&pool, false).await?;
    let writer: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&incoming)
        .await?;
    let mut source = pool.begin().await?;
    lock_hardware_graph(&mut source).await?;
    let holder: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *source)
        .await?;
    let (batch, ids) = empty_selected_side(cable);
    assert_empty(&mut source, &batch, &ids).await?;
    let release = async {
        let observed = wait_for_graph_blocker(&pool, holder, writer).await;
        if observed.is_ok() {
            assert_empty(&mut source, &batch, &ids).await?;
            let attached: Option<Uuid> =
                sqlx::query_scalar("SELECT cable_id FROM dcim_interface WHERE id=$1")
                    .bind(endpoint)
                    .fetch_one(&mut *source)
                    .await?;
            assert!(
                attached.is_none(),
                "an unseen endpoint must not attach before commit"
            );
        }
        source.commit().await?;
        observed
    };
    let mutation = async {
        let mut tx = incoming.begin().await?;
        // The real source replay owner acquires graph intent before upsert;
        // neither a cable-row lock nor a lock on observed endpoints suffices.
        let plans = vec![PlannedRow {
            entity: InventoryEntity::Interface,
            source_entity: InventoryEntity::Interface.source_key().into(),
            external_id: "incoming-endpoint".into(),
            local_id: endpoint,
            source_timestamps: SourceTimestamps::default(),
            columns: vec![
                // INSERT ... ON CONFLICT checks required INSERT fields before
                // choosing UPDATE. Preserve existing metadata on that branch.
                PlannedColumn {
                    column: "custom_field_data",
                    value: ColumnValue::Literal("'{}'::jsonb"),
                    updatable: false,
                },
                PlannedColumn {
                    column: "name",
                    value: ColumnValue::Scalar(
                        PgType::Text,
                        InventoryValue::Text {
                            value: format!("incoming-{endpoint}"),
                        },
                    ),
                    updatable: false,
                },
                PlannedColumn {
                    column: "_name",
                    value: ColumnValue::Scalar(
                        PgType::Text,
                        InventoryValue::Text {
                            value: format!("incoming-{endpoint}"),
                        },
                    ),
                    updatable: false,
                },
                PlannedColumn {
                    column: "label",
                    value: ColumnValue::Literal("''"),
                    updatable: false,
                },
                PlannedColumn {
                    column: "description",
                    value: ColumnValue::Literal("''"),
                    updatable: false,
                },
                PlannedColumn {
                    column: "mark_connected",
                    value: ColumnValue::Literal("false"),
                    updatable: false,
                },
                PlannedColumn {
                    column: "enabled",
                    value: ColumnValue::Literal("true"),
                    updatable: false,
                },
                PlannedColumn {
                    column: "type",
                    value: ColumnValue::Literal("'1000base-t'"),
                    updatable: false,
                },
                PlannedColumn {
                    column: "mgmt_only",
                    value: ColumnValue::Literal("false"),
                    updatable: false,
                },
                PlannedColumn {
                    column: "cable_id",
                    value: ColumnValue::Uuid(Some(cable)),
                    updatable: true,
                },
                PlannedColumn {
                    column: "cable_end",
                    value: ColumnValue::Scalar(
                        PgType::Text,
                        InventoryValue::Text { value: "A".into() },
                    ),
                    updatable: true,
                },
            ],
            relations: Vec::new(),
            junctions: Vec::new(),
            selectors: Vec::new(),
            reference_targets: HashMap::new(),
        }];
        apply_planned_rows(&mut tx, &plans).await?;
        tx.commit().await?;
        Ok::<(), AppError>(())
    };
    let (released, attached) = tokio::join!(
        release,
        tokio::time::timeout(Duration::from_secs(10), mutation)
    );
    released?;
    attached??;
    assert_final_and_cleanup(&pool, cable, endpoint).await
}
