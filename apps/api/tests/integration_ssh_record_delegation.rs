//! Record-only SSH delegation against the real persistence boundary.
mod common;

use std::collections::HashSet;
use std::num::NonZeroU64;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use piping_backend::contexts::connections::application::ConnectionService;
use piping_backend::contexts::connections::application::connection_service::{
    ConnectionServiceDeps, SshRecordDelegationAuthorizer, SshRecordPurpose,
};
use piping_backend::contexts::connections::infrastructure::ConnectionRepository;
use piping_backend::contexts::secrets::application::SecretStore;
use piping_backend::errors::AppError;
use piping_backend::integrations::proxy::{HttpProxyClient, ProxyCredential};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Default)]
struct CountedStore(Mutex<Vec<Uuid>>);

#[async_trait]
impl SecretStore for CountedStore {
    async fn get_secret(&self, id: Uuid) -> Result<String, AppError> {
        self.0.lock().unwrap().push(id);
        Ok(format!("synthetic-marker-{id}"))
    }
    async fn set_secret(&self, _: Uuid, _: &str) -> Result<(), AppError> {
        Err(AppError::Validation("test store is read-only".into()))
    }
    async fn delete_secret(&self, _: Uuid) -> Result<(), AppError> {
        Err(AppError::Validation("test store is read-only".into()))
    }
    async fn ensure_unique_key(&self, _: &str, _: Uuid, _: &str) -> Result<(), AppError> {
        Err(AppError::Validation("test store is read-only".into()))
    }
}

struct Authorizer {
    seen: Mutex<Vec<Uuid>>,
    deny: Option<Uuid>,
    mutation: Option<(&'static str, Uuid, Value)>,
    pool: PgPool,
}

#[async_trait]
impl SshRecordDelegationAuthorizer for Authorizer {
    async fn authorize_records(&self, ids: &[Uuid]) -> Result<(), AppError> {
        *self.seen.lock().unwrap() = ids.to_vec();
        if self.deny.is_some_and(|id| ids.contains(&id)) {
            return Err(AppError::Validation("delegation denied".into()));
        }
        if let Some((kind, id, value)) = &self.mutation {
            if *kind == "params" {
                sqlx::query("UPDATE connconf_record SET params = $1 WHERE id = $2")
                    .bind(value)
                    .bind(id)
                    .execute(&self.pool)
                    .await
                    .map_err(|e| AppError::Database(e.to_string()))?;
            } else {
                sqlx::query("UPDATE connconf_record SET base_record_id = NULL WHERE id = $1")
                    .bind(id)
                    .execute(&self.pool)
                    .await
                    .map_err(|e| AppError::Database(e.to_string()))?;
            }
        }
        Ok(())
    }
}

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

async fn record(pool: &PgPool, id: Uuid, base: Option<Uuid>, params: Value) -> TestResult {
    sqlx::query("INSERT INTO connconf_record (id, name, description, protocol, base_record_id, enabled, params) VALUES ($1,$2,'','ssh',$3,true,$4)")
        .bind(id).bind(format!("delegation-{id}")).bind(base).bind(params).execute(pool).await?;
    Ok(())
}

async fn binding(pool: &PgPool, record: Uuid, key: Uuid) -> TestResult {
    sqlx::query("INSERT INTO secrets_variable (id, key, is_masked, is_protected, kind, description, retire_with_field_usage) VALUES ($1,$2,true,false,'field','',false)")
        .bind(key).bind(format!("delegation-{key}")).execute(pool).await?;
    sqlx::query("INSERT INTO secret_usage_binding (consumer_type, consumer_id, field_key, owner_scope_type, owner_scope_id, secret_id) VALUES ('connection_config:record',$1,'credentials.private_key','connection_config',$1,$2)")
        .bind(record).bind(key).execute(pool).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires the explicitly isolated PostgreSQL/Redis test stack"]
async fn delegated_record_chain_fences_keys_and_preserves_endpoints() -> TestResult {
    let state = common::build_test_app_state_with(|_| {}).await?;
    let pool = &state.db_pool;
    let store = Arc::new(CountedStore::default());
    let proxy = HttpProxyClient::new(
        "http://127.0.0.1:1".into(),
        ProxyCredential::new("test", "synthetic")?,
    )?;
    let service = ConnectionService::new(ConnectionServiceDeps {
        repository: Arc::new(state.repositories.connections.clone()),
        proxy_client: Arc::new(proxy),
        audit_service: state.services.audit.clone(),
        materializer: state.services.materializer.clone(),
        secret_repository: Arc::new(state.repositories.secrets.clone()),
        secret_store: store.clone(),
        auth_session_ttl_seconds: NonZeroU64::new(60).unwrap(),
        connconf_records: Arc::new(state.repositories.connconf_records.clone()),
        connconf_assignments: Arc::new(state.repositories.connconf_assignments.clone()),
    });
    let [base, root, hop1, hop2, key0, key1, key2] = [(); 7].map(|_| Uuid::now_v7());
    let base_params = json!({"credentials":{"private_key":key0.to_string()}});
    record(pool, base, None, base_params.clone()).await?;
    record(pool, hop1, None, json!({"host":"192.0.2.11","port":2201,"credentials":{"username":"hop-one","private_key":key1.to_string()}})).await?;
    record(pool, hop2, None, json!({"host":"192.0.2.12","port":2202,"credentials":{"username":"hop-two","private_key":key2.to_string()}})).await?;
    let root_params = json!({"host":"192.0.2.10","port":2222,"credentials":{"username":"root-user"},"jump":[{"connconf_record_id":hop1.to_string()},{"connconf_record_id":hop2.to_string()}], "native_host_key_endpoints":[{"host":"inventory.example.test","port":2203}]});
    record(pool, root, Some(base), root_params.clone()).await?;
    for (record, key) in [(base, key0), (hop1, key1), (hop2, key2)] {
        binding(pool, record, key).await?;
    }
    let pins = vec![
        "[192.0.2.10]:2222 ssh-ed25519 YWJj".to_string(),
        "[192.0.2.11]:2201 ssh-ed25519 ZGVm".to_string(),
        "[inventory.example.test]:2203 ssh-ed25519 Z2hp".to_string(),
        "hidden.example.test ssh-ed25519 amts".to_string(),
    ];
    state
        .repositories
        .connections
        .persist_observed_host_key_lines(&pins)
        .await?;
    let authorizer = |deny, mutation| Authorizer {
        seen: Mutex::new(Vec::new()),
        deny,
        mutation,
        pool: pool.clone(),
    };
    for denied in [base, root, hop1, hop2] {
        let auth = authorizer(Some(denied), None);
        assert!(
            service
                .resolve_delegated_ssh_record(root, None, SshRecordPurpose::NativeDefaults, &auth)
                .await
                .is_err()
        );
        assert_eq!(
            auth.seen
                .lock()
                .unwrap()
                .iter()
                .copied()
                .collect::<HashSet<_>>(),
            HashSet::from([base, root, hop1, hop2])
        );
        assert!(store.0.lock().unwrap().is_empty());
    }
    for (field, value) in [
        ("consumer_id", root.to_string()),
        ("field_key", "credentials.password".into()),
        ("owner_scope_id", root.to_string()),
        ("owner_scope_type", "user".into()),
    ] {
        let query = format!(
            "UPDATE secret_usage_binding SET {field} = $1::text{} WHERE secret_id = $2",
            if field.ends_with("_id") { "::uuid" } else { "" }
        );
        sqlx::query(sqlx::AssertSqlSafe(query))
            .bind(value)
            .bind(key0)
            .execute(pool)
            .await?;
        assert!(
            service
                .resolve_delegated_ssh_record(
                    root,
                    None,
                    SshRecordPurpose::NativeDefaults,
                    &authorizer(None, None)
                )
                .await
                .is_err()
        );
        assert!(store.0.lock().unwrap().is_empty());
        sqlx::query("UPDATE secret_usage_binding SET consumer_id=$1, field_key='credentials.private_key', owner_scope_type='connection_config', owner_scope_id=$1 WHERE secret_id=$2").bind(base).bind(key0).execute(pool).await?;
    }
    for mutation in [
        (
            "params",
            base,
            json!({"credentials":{"private_key":key1.to_string()}}),
        ),
        ("base", root, Value::Null),
    ] {
        assert!(
            service
                .resolve_delegated_ssh_record(
                    root,
                    None,
                    SshRecordPurpose::NativeDefaults,
                    &authorizer(None, Some(mutation))
                )
                .await
                .is_err()
        );
        assert!(store.0.lock().unwrap().is_empty());
        sqlx::query("UPDATE connconf_record SET params=$1 WHERE id=$2")
            .bind(&base_params)
            .bind(base)
            .execute(pool)
            .await?;
        sqlx::query("UPDATE connconf_record SET base_record_id=$1 WHERE id=$2")
            .bind(base)
            .bind(root)
            .execute(pool)
            .await?;
    }
    let owner = Uuid::parse_str("00000000-0000-7000-8000-000000000001")?;
    sqlx::query("UPDATE connconf_record SET owner_user_id=$1 WHERE id=$2")
        .bind(owner)
        .bind(hop1)
        .execute(pool)
        .await?;
    assert!(
        service
            .resolve_delegated_ssh_record(
                root,
                None,
                SshRecordPurpose::NativeDefaults,
                &authorizer(None, None)
            )
            .await
            .is_err()
    );
    assert!(store.0.lock().unwrap().is_empty());
    sqlx::query("UPDATE connconf_record SET owner_user_id=NULL WHERE id=$1")
        .bind(hop1)
        .execute(pool)
        .await?;
    let auth = authorizer(None, None);
    let payload = service
        .resolve_delegated_ssh_record(root, None, SshRecordPurpose::NativeDefaults, &auth)
        .await?;
    assert_eq!(*store.0.lock().unwrap(), vec![key0, key1, key2]);
    for (endpoint, host, port, user, key) in [
        (&payload, "192.0.2.10", 2222, "root-user", key0),
        (&payload["jump"][0], "192.0.2.11", 2201, "hop-one", key1),
        (&payload["jump"][1], "192.0.2.12", 2202, "hop-two", key2),
    ] {
        assert_eq!(endpoint["host"], host);
        assert_eq!(endpoint["port"], port);
        assert_eq!(endpoint["credentials"]["username"], user);
        assert_eq!(
            endpoint["credentials"]["private_key"],
            format!("synthetic-marker-{key}")
        );
    }
    for pin in &pins[..3] {
        assert!(
            payload["known_hosts_lines"]
                .as_array()
                .unwrap()
                .contains(&json!(pin))
        );
    }
    assert!(
        !payload["known_hosts_lines"]
            .as_array()
            .unwrap()
            .contains(&json!(pins[3]))
    );
    store.0.lock().unwrap().clear();
    let endpoint_bound = service
        .resolve_delegated_ssh_record(
            root,
            None,
            SshRecordPurpose::EndpointBound,
            &authorizer(None, None),
        )
        .await?;
    assert!(
        !endpoint_bound["known_hosts_lines"]
            .as_array()
            .unwrap()
            .contains(&json!(pins[2]))
    );
    for invalid in [
        json!([{"host":"inventory.example.test","port":2204}]),
        json!([{"host":"missing.example.test"}]),
        json!([{"host":"*.example.test"}]),
        json!([{"host":"inventory.example.test","extra":true}]),
    ] {
        store.0.lock().unwrap().clear();
        let mut params = root_params.clone();
        params["native_host_key_endpoints"] = invalid;
        sqlx::query("UPDATE connconf_record SET params=$1 WHERE id=$2")
            .bind(params)
            .bind(root)
            .execute(pool)
            .await?;
        assert!(
            service
                .resolve_delegated_ssh_record(
                    root,
                    None,
                    SshRecordPurpose::NativeDefaults,
                    &authorizer(None, None)
                )
                .await
                .is_err()
        );
        assert!(store.0.lock().unwrap().is_empty());
    }
    // Inheritance uses the normal record merge: replacement and unset, not union.
    let mut inherited = base_params.clone();
    inherited["native_host_key_endpoints"] = json!([{"host":"inventory.example.test","port":2203}]);
    sqlx::query("UPDATE connconf_record SET params=$1 WHERE id=$2")
        .bind(&inherited)
        .bind(base)
        .execute(pool)
        .await?;
    let mut child = root_params.clone();
    child
        .as_object_mut()
        .unwrap()
        .remove("native_host_key_endpoints");
    for (value, expected) in [
        (None, true),
        (Some(json!([])), false),
        (Some(json!({"unset":true})), false),
    ] {
        let mut params = child.clone();
        if let Some(value) = value {
            params["native_host_key_endpoints"] = value;
        }
        sqlx::query("UPDATE connconf_record SET params=$1 WHERE id=$2")
            .bind(&params)
            .bind(root)
            .execute(pool)
            .await?;
        let result = service
            .resolve_delegated_ssh_record(
                root,
                None,
                SshRecordPurpose::NativeDefaults,
                &authorizer(None, None),
            )
            .await?;
        assert_eq!(
            result["known_hosts_lines"]
                .as_array()
                .unwrap()
                .contains(&json!(pins[2])),
            expected
        );
    }
    store.0.lock().unwrap().clear();
    sqlx::query("UPDATE connconf_record SET params=$1 WHERE id=$2")
        .bind(&root_params)
        .bind(root)
        .execute(pool)
        .await?;
    let mut changed = root_params.clone();
    changed["native_host_key_endpoints"] = json!([]);
    assert!(
        service
            .resolve_delegated_ssh_record(
                root,
                None,
                SshRecordPurpose::NativeDefaults,
                &authorizer(None, Some(("params", root, changed)))
            )
            .await
            .is_err()
    );
    assert!(store.0.lock().unwrap().is_empty());
    Ok(())
}
