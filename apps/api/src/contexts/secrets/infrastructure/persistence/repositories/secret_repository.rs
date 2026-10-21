use std::collections::{HashMap, HashSet};

use async_trait::async_trait;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use crate::contexts::secrets::application::{ApplySecretFieldIntentInput, SecretFieldState};
use crate::contexts::secrets::domain::{
    NamedSecretSummary, SECRET_KIND_NAMED, SecretBinding, SecretUsageBinding, SecretVariable,
    validate_named_secret_key,
};
use crate::errors::{AppError, classify_sqlx_error_owned};

/// Trims a caller-supplied description, mapping blank input to `None` so an
/// empty string is never persisted as a meaningful value.
fn normalize_optional_description(description: Option<&str>) -> Option<String> {
    description
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

#[derive(Debug, Clone)]
pub struct SecretScopeSecrets {
    pub variables: Vec<SecretVariable>,
    pub bindings: Vec<SecretBinding>,
}

/// Scopes and tenant ids accumulated while walking one scope's hierarchy.
struct ScopeAccumulator {
    scopes: HashSet<(String, Uuid)>,
    tenant_ids: HashSet<Uuid>,
}

impl ScopeAccumulator {
    fn new() -> Self {
        Self {
            scopes: HashSet::new(),
            tenant_ids: HashSet::new(),
        }
    }

    fn insert_scope(&mut self, scope_type: &str, scope_id: Uuid) {
        self.scopes.insert((scope_type.to_string(), scope_id));
    }

    fn insert_tenant(&mut self, tenant_id: Uuid) {
        self.scopes.insert(("tenant".to_string(), tenant_id));
        self.tenant_ids.insert(tenant_id);
    }
}

fn read_optional_uuid(
    row: &sqlx::postgres::PgRow,
    column: &str,
    context: &str,
) -> Result<Option<Uuid>, AppError> {
    row.try_get(column)
        .map_err(|e| AppError::Database(format!("Failed to read {context}: {e}")))
}

#[async_trait]
pub trait SecretRepository: Send + Sync {
    /// Applies a validated field intent, including encrypted value and retirement,
    /// in one persistence transaction.
    async fn apply_field_intent(
        &self,
        input: ApplySecretFieldIntentInput,
    ) -> Result<SecretFieldState, AppError>;
    async fn create_variable(&self, variable: SecretVariable) -> Result<SecretVariable, AppError>;
    async fn get_variable_by_key(&self, key: &str) -> Result<Option<SecretVariable>, AppError>;
    async fn get_variable_by_id(&self, secret_id: Uuid)
    -> Result<Option<SecretVariable>, AppError>;
    async fn variable_exists(&self, secret_id: Uuid) -> Result<bool, AppError>;
    async fn list_variables(&self, key: Option<&str>) -> Result<Vec<SecretVariable>, AppError>;
    /// Lists named (admin-managed) automation secrets with derived value
    /// presence and usage counts. Never returns machine `secret-field/...` rows.
    async fn list_named(&self) -> Result<Vec<NamedSecretSummary>, AppError>;
    /// Creates a named automation secret. Validates the key, rejects the
    /// reserved `secret-field/` prefix, and writes
    /// `kind = 'named', is_masked = true, is_protected = false`. No value is
    /// stored — the value is set later through the write-only value route.
    async fn create_named(
        &self,
        key: &str,
        description: Option<&str>,
        created_by: Option<Uuid>,
    ) -> Result<NamedSecretSummary, AppError>;
    /// Updates the description of a named secret. Returns `None` when the id is
    /// absent or is not a `named` secret.
    async fn update_description(
        &self,
        secret_id: Uuid,
        description: Option<&str>,
    ) -> Result<Option<NamedSecretSummary>, AppError>;
    /// Deletes a named secret. Fails with [`AppError::Conflict`] when it is still
    /// referenced by any `secret_usage_binding` row. Returns `false` when the id
    /// is absent or not a `named` secret.
    async fn delete_named(&self, secret_id: Uuid) -> Result<bool, AppError>;
    async fn list_bindings_for_secret(
        &self,
        secret_id: Uuid,
    ) -> Result<Vec<SecretBinding>, AppError>;
    async fn list_bindings_for_scope(
        &self,
        scope_type: &str,
        scope_id: Uuid,
    ) -> Result<Vec<SecretBinding>, AppError>;
    async fn list_bindings_for_scope_hierarchy(
        &self,
        scope_type: &str,
        scope_id: Uuid,
    ) -> Result<Vec<SecretBinding>, AppError>;
    async fn list_scope_secrets(
        &self,
        scope_type: &str,
        scope_id: Uuid,
    ) -> Result<SecretScopeSecrets, AppError>;
    async fn create_binding(&self, binding: SecretBinding) -> Result<SecretBinding, AppError>;
    async fn delete_binding(&self, binding_id: Uuid) -> Result<bool, AppError>;
    async fn get_usage_binding(
        &self,
        consumer_type: &str,
        consumer_id: Uuid,
        field_key: &str,
    ) -> Result<Option<SecretUsageBinding>, AppError>;
    async fn list_usage_bindings_for_consumer(
        &self,
        consumer_type: &str,
        consumer_id: Uuid,
    ) -> Result<Vec<SecretUsageBinding>, AppError>;
    /// Bindings for a whole page of consumers in one query — the per-entity
    /// variant inside a list loop is an N+1 (13 extra round-trips per device
    /// row on the live schema).
    async fn list_usage_bindings_for_consumers(
        &self,
        consumer_type: &str,
        consumer_ids: &[Uuid],
    ) -> Result<Vec<SecretUsageBinding>, AppError>;
    async fn upsert_usage_binding(
        &self,
        binding: SecretUsageBinding,
    ) -> Result<SecretUsageBinding, AppError>;
    async fn delete_usage_binding(&self, binding_id: Uuid) -> Result<bool, AppError>;
}

#[derive(Debug, Clone)]
pub struct SqlxSecretRepository {
    pool: PgPool,
}

impl SqlxSecretRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse_variable_row(row: &sqlx::postgres::PgRow) -> Result<SecretVariable, AppError> {
        Ok(SecretVariable {
            id: row.try_get("id")?,
            key: row.try_get("key")?,
            is_masked: row.try_get("is_masked")?,
            is_protected: row.try_get("is_protected")?,
            kind: row.try_get("kind")?,
            description: row.try_get("description")?,
            retire_with_field_usage: row.try_get("retire_with_field_usage")?,
        })
    }

    fn parse_named_summary_row(
        row: &sqlx::postgres::PgRow,
    ) -> Result<NamedSecretSummary, AppError> {
        Ok(NamedSecretSummary {
            id: row.try_get("id")?,
            key: row.try_get("key")?,
            description: row.try_get("description")?,
            is_masked: row.try_get("is_masked")?,
            created_at: row.try_get("created_at")?,
            has_value: row.try_get("has_value")?,
            usage_count: row.try_get("usage_count")?,
        })
    }

    /// Loads the enriched read-model for a single named secret (value presence
    /// and usage count derived per query). Returns `None` when the id is absent
    /// or the row is not a `named` secret.
    async fn fetch_named_summary(
        &self,
        secret_id: Uuid,
    ) -> Result<Option<NamedSecretSummary>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT
                sv.id,
                sv.key,
                sv.description,
                sv.is_masked,
                sv.created_at,
                EXISTS (
                    SELECT 1 FROM secrets_store ss WHERE ss.secret_id = sv.id
                ) AS has_value,
                (
                    SELECT COUNT(*)
                    FROM secret_usage_binding sub
                    WHERE sub.secret_id = sv.id
                )::BIGINT AS usage_count
            FROM secrets_variable sv
            WHERE sv.id = $1
              AND sv.kind = $2
            LIMIT 1
            "#,
        )
        .bind(secret_id)
        .bind(SECRET_KIND_NAMED)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| {
            AppError::Database(format!("Failed to load named secret {secret_id}: {error}"))
        })?;

        row.map(|row| Self::parse_named_summary_row(&row))
            .transpose()
    }

    fn parse_binding_row(row: &sqlx::postgres::PgRow) -> Result<SecretBinding, AppError> {
        Ok(SecretBinding {
            id: row.try_get("id")?,
            secret_id: row.try_get("secret_id")?,
            scope_type: row.try_get("scope_type")?,
            scope_id: row.try_get("scope_id")?,
            weight: row.try_get("weight")?,
            managed_by_field_usage: row.try_get("managed_by_field_usage")?,
        })
    }

    fn parse_usage_binding_row(
        row: &sqlx::postgres::PgRow,
    ) -> Result<SecretUsageBinding, AppError> {
        Ok(SecretUsageBinding {
            id: row.try_get("id")?,
            consumer_type: row.try_get("consumer_type")?,
            consumer_id: row.try_get("consumer_id")?,
            field_key: row.try_get("field_key")?,
            owner_scope_type: row.try_get("owner_scope_type")?,
            owner_scope_id: row.try_get("owner_scope_id")?,
            secret_id: row.try_get("secret_id")?,
        })
    }

    async fn fetch_tenant_group_id(&self, tenant_id: Uuid) -> Result<Option<Uuid>, AppError> {
        let row = sqlx::query("SELECT group_id FROM tenancy_tenant WHERE id = $1")
            .bind(tenant_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| {
                AppError::Database(format!(
                    "Failed to load tenant group for tenant {tenant_id}: {e}"
                ))
            })?;

        match row {
            Some(row) => row
                .try_get("group_id")
                .map_err(|e| AppError::Database(format!("Failed to read tenant group_id: {e}"))),
            None => Ok(None),
        }
    }

    async fn build_scope_hierarchy(
        &self,
        scope_type: &str,
        scope_id: Uuid,
    ) -> Result<Vec<(String, Uuid)>, AppError> {
        let mut scopes: HashSet<(String, Uuid)> = HashSet::new();
        let mut tenant_ids: HashSet<Uuid> = HashSet::new();

        match scope_type {
            "connection_target" => {
                let Some(reached) = self.connection_target_scopes(scope_id).await? else {
                    return Ok(Vec::new());
                };
                scopes.extend(reached.scopes);
                tenant_ids.extend(reached.tenant_ids);
            }
            "site" => {
                let Some(reached) = self.site_scope_scopes(scope_id).await? else {
                    return Ok(Vec::new());
                };
                scopes.extend(reached.scopes);
                tenant_ids.extend(reached.tenant_ids);
            }
            "tenant" => {
                scopes.insert((scope_type.to_string(), scope_id));
                tenant_ids.insert(scope_id);
            }
            "tenant_group" => {
                scopes.insert((scope_type.to_string(), scope_id));
            }
            _ => {
                scopes.insert((scope_type.to_string(), scope_id));
            }
        }

        for tenant_id in tenant_ids {
            if let Some(group_id) = self.fetch_tenant_group_id(tenant_id).await? {
                scopes.insert(("tenant_group".to_string(), group_id));
            }
        }

        Ok(scopes.into_iter().collect())
    }

    /// Scopes reachable from a `connection_target`: the device it names, or the
    /// virtual machine that replaces it, plus their site and tenant ancestors.
    /// Returns `None` when neither the device nor the VM row exists.
    async fn connection_target_scopes(
        &self,
        scope_id: Uuid,
    ) -> Result<Option<ScopeAccumulator>, AppError> {
        let device_row = sqlx::query("SELECT site_id, tenant_id FROM dcim_device WHERE id = $1")
            .bind(scope_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to load device scope: {e}")))?;

        if let Some(device_row) = device_row {
            let reached = self.device_scope_scopes(&device_row, scope_id).await?;
            return Ok(Some(reached));
        }

        let vm_row = sqlx::query(
            "SELECT site_id, tenant_id, cluster_id FROM virtualization_virtualmachine WHERE id = $1",
        )
        .bind(scope_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load vm scope: {e}")))?;

        let Some(vm_row) = vm_row else {
            return Ok(None);
        };
        let reached = self.vm_scope_scopes(&vm_row, scope_id).await?;
        Ok(Some(reached))
    }

    async fn device_scope_scopes(
        &self,
        device_row: &sqlx::postgres::PgRow,
        scope_id: Uuid,
    ) -> Result<ScopeAccumulator, AppError> {
        let mut reached = ScopeAccumulator::new();
        reached.insert_scope("connection_target", scope_id);

        let tenant_id: Option<Uuid> =
            read_optional_uuid(device_row, "tenant_id", "device tenant_id")?;
        let site_id: Option<Uuid> = read_optional_uuid(device_row, "site_id", "device site_id")?;

        if let Some(site_id) = site_id {
            reached.insert_scope("site", site_id);
        }
        if let Some(tenant_id) = tenant_id {
            reached.insert_tenant(tenant_id);
        }
        if let Some(site_id) = site_id
            && let Some(tenant_id) = self.load_site_tenant_id(site_id).await?
        {
            reached.insert_tenant(tenant_id);
        }

        Ok(reached)
    }

    async fn vm_scope_scopes(
        &self,
        vm_row: &sqlx::postgres::PgRow,
        scope_id: Uuid,
    ) -> Result<ScopeAccumulator, AppError> {
        let mut reached = ScopeAccumulator::new();
        reached.insert_scope("connection_target", scope_id);

        let tenant_id: Option<Uuid> = read_optional_uuid(vm_row, "tenant_id", "vm tenant_id")?;
        let site_id: Option<Uuid> = read_optional_uuid(vm_row, "site_id", "vm site_id")?;
        let cluster_id: Option<Uuid> = read_optional_uuid(vm_row, "cluster_id", "vm cluster_id")?;

        if let Some(site_id) = site_id {
            reached.insert_scope("site", site_id);
        }
        if let Some(tenant_id) = tenant_id {
            reached.insert_tenant(tenant_id);
        }
        if let Some(cluster_id) = cluster_id {
            self.cluster_scope_scopes(cluster_id, &mut reached).await?;
        }
        if let Some(site_id) = site_id
            && let Some(tenant_id) = self.load_site_tenant_id(site_id).await?
        {
            reached.insert_tenant(tenant_id);
        }

        Ok(reached)
    }

    /// Adds the site and tenant ancestors reached through a VM's cluster.
    async fn cluster_scope_scopes(
        &self,
        cluster_id: Uuid,
        reached: &mut ScopeAccumulator,
    ) -> Result<(), AppError> {
        let cluster_row =
            sqlx::query("SELECT _site_id, tenant_id FROM virtualization_cluster WHERE id = $1")
                .bind(cluster_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| AppError::Database(format!("Failed to load cluster scope: {e}")))?;

        let Some(cluster_row) = cluster_row else {
            return Ok(());
        };

        let cluster_site_id: Option<Uuid> =
            read_optional_uuid(&cluster_row, "_site_id", "cluster site_id")?;
        let cluster_tenant_id: Option<Uuid> =
            read_optional_uuid(&cluster_row, "tenant_id", "cluster tenant_id")?;

        if let Some(site_id) = cluster_site_id {
            reached.insert_scope("site", site_id);
        }
        if let Some(tenant_id) = cluster_tenant_id {
            reached.insert_tenant(tenant_id);
        }
        if let Some(site_id) = cluster_site_id
            && let Some(tenant_id) = self.load_site_tenant_id(site_id).await?
        {
            reached.insert_tenant(tenant_id);
        }

        Ok(())
    }

    /// Scopes reachable from a `site`: the site itself plus its tenant. Returns
    /// `None` when the site row does not exist.
    async fn site_scope_scopes(
        &self,
        scope_id: Uuid,
    ) -> Result<Option<ScopeAccumulator>, AppError> {
        let mut reached = ScopeAccumulator::new();
        reached.insert_scope("site", scope_id);

        let site_row = sqlx::query("SELECT tenant_id FROM dcim_site WHERE id = $1")
            .bind(scope_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to load site scope: {e}")))?;

        let Some(site_row) = site_row else {
            return Ok(None);
        };

        let tenant_id: Option<Uuid> = read_optional_uuid(&site_row, "tenant_id", "site tenant_id")?;
        if let Some(tenant_id) = tenant_id {
            reached.insert_tenant(tenant_id);
        }

        Ok(Some(reached))
    }

    /// Loads the tenant of a site, treating a missing row or NULL tenant as no
    /// tenant.
    async fn load_site_tenant_id(&self, site_id: Uuid) -> Result<Option<Uuid>, AppError> {
        let site_row = sqlx::query("SELECT tenant_id FROM dcim_site WHERE id = $1")
            .bind(site_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to load site scope: {e}")))?;

        let Some(site_row) = site_row else {
            return Ok(None);
        };
        read_optional_uuid(&site_row, "tenant_id", "site tenant_id")
    }
}

#[async_trait]
impl SecretRepository for SqlxSecretRepository {
    async fn apply_field_intent(
        &self,
        input: ApplySecretFieldIntentInput,
    ) -> Result<SecretFieldState, AppError> {
        let mut transaction = self.pool.begin().await?;
        let state =
            super::secret_field_lifecycle::apply_field_intent(&mut transaction, input).await?;
        transaction.commit().await?;
        Ok(state)
    }
    async fn create_variable(&self, variable: SecretVariable) -> Result<SecretVariable, AppError> {
        sqlx::query_as!(SecretVariable, "INSERT INTO secrets_variable (id, key, is_masked, is_protected, kind, description, retire_with_field_usage) VALUES ($1, $2, $3, $4, $5, $6, false) RETURNING id, key, is_masked, is_protected, kind, description, retire_with_field_usage", variable.id, variable.key, variable.is_masked, variable.is_protected, variable.kind, variable.description)
            .fetch_one(&self.pool).await.map_err(|error| AppError::Database(format!("Failed to create secret variable {}: {error}", variable.id)))
    }

    async fn get_variable_by_key(&self, key: &str) -> Result<Option<SecretVariable>, AppError> {
        Ok(sqlx::query_as!(SecretVariable, "SELECT id, key, is_masked, is_protected, kind, description, retire_with_field_usage FROM secrets_variable WHERE key = $1 LIMIT 1", key)
            .fetch_optional(&self.pool).await?)
    }

    async fn get_variable_by_id(
        &self,
        secret_id: Uuid,
    ) -> Result<Option<SecretVariable>, AppError> {
        Ok(sqlx::query_as!(SecretVariable, "SELECT id, key, is_masked, is_protected, kind, description, retire_with_field_usage FROM secrets_variable WHERE id = $1 LIMIT 1", secret_id)
            .fetch_optional(&self.pool).await?)
    }

    async fn variable_exists(&self, secret_id: Uuid) -> Result<bool, AppError> {
        Ok(self.get_variable_by_id(secret_id).await?.is_some())
    }

    async fn list_variables(&self, key: Option<&str>) -> Result<Vec<SecretVariable>, AppError> {
        let mut builder = QueryBuilder::<Postgres>::new(
            "SELECT id, key, is_masked, is_protected, kind, description, retire_with_field_usage FROM secrets_variable WHERE 1 = 1",
        );
        if let Some(key) = key {
            builder.push(" AND key ILIKE ");
            builder.push_bind(format!("%{key}%"));
        }
        builder.push(" ORDER BY key, id");

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list secret variables: {e}")))?;
        rows.iter()
            .map(Self::parse_variable_row)
            .collect::<Result<Vec<_>, AppError>>()
    }

    async fn list_named(&self) -> Result<Vec<NamedSecretSummary>, AppError> {
        let rows = sqlx::query(
            r#"
            SELECT
                sv.id,
                sv.key,
                sv.description,
                sv.is_masked,
                sv.created_at,
                EXISTS (
                    SELECT 1 FROM secrets_store ss WHERE ss.secret_id = sv.id
                ) AS has_value,
                (
                    SELECT COUNT(*)
                    FROM secret_usage_binding sub
                    WHERE sub.secret_id = sv.id
                )::BIGINT AS usage_count
            FROM secrets_variable sv
            WHERE sv.kind = $1
            ORDER BY sv.key, sv.id
            "#,
        )
        .bind(SECRET_KIND_NAMED)
        .fetch_all(&self.pool)
        .await
        .map_err(|error| AppError::Database(format!("Failed to list named secrets: {error}")))?;

        rows.iter()
            .map(Self::parse_named_summary_row)
            .collect::<Result<Vec<_>, AppError>>()
    }

    async fn create_named(
        &self,
        key: &str,
        description: Option<&str>,
        created_by: Option<Uuid>,
    ) -> Result<NamedSecretSummary, AppError> {
        // Fail-closed key validation (also rejects the reserved prefix) before
        // any write. The CHECK constraint and UNIQUE key index are the last-line
        // guards; this gives a clear 400 instead of a raw 409/500.
        let key = validate_named_secret_key(key)?;
        let description = normalize_optional_description(description);
        let secret_id = Uuid::now_v7();

        sqlx::query(
            r#"
            INSERT INTO secrets_variable
                (id, key, is_masked, is_protected, kind, description, retire_with_field_usage, created_by)
            VALUES ($1, $2, true, false, $3, $4, false, $5)
            "#,
        )
        .bind(secret_id)
        .bind(&key)
        .bind(SECRET_KIND_NAMED)
        .bind(description.as_deref())
        .bind(created_by)
        .execute(&self.pool)
        .await
        // Unique key-index violation (23505) maps to 409 Conflict.
        .map_err(classify_sqlx_error_owned)?;

        self.fetch_named_summary(secret_id).await?.ok_or_else(|| {
            AppError::Database(format!(
                "Named secret {secret_id} disappeared immediately after creation"
            ))
        })
    }

    async fn update_description(
        &self,
        secret_id: Uuid,
        description: Option<&str>,
    ) -> Result<Option<NamedSecretSummary>, AppError> {
        let description = normalize_optional_description(description);
        let result = sqlx::query(
            r#"
            UPDATE secrets_variable
            SET description = $2, updated_at = now()
            WHERE id = $1
              AND kind = $3
            "#,
        )
        .bind(secret_id)
        .bind(description.as_deref())
        .bind(SECRET_KIND_NAMED)
        .execute(&self.pool)
        .await
        .map_err(|error| {
            AppError::Database(format!(
                "Failed to update named secret {secret_id}: {error}"
            ))
        })?;

        if result.rows_affected() == 0 {
            return Ok(None);
        }

        self.fetch_named_summary(secret_id).await
    }

    async fn delete_named(&self, secret_id: Uuid) -> Result<bool, AppError> {
        // Lock the secret before checking usages. Query-source binding writers
        // acquire `FOR KEY SHARE` on the same row before projecting their
        // `secret_usage_binding`, so this serializes deletion against a new or
        // replacement reference instead of letting either side strand the
        // other. Callers surface an in-use secret as 409.
        let mut tx = self.pool.begin().await.map_err(classify_sqlx_error_owned)?;
        let exists = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM secrets_variable WHERE id = $1 AND kind = $2 FOR UPDATE",
        )
        .bind(secret_id)
        .bind(SECRET_KIND_NAMED)
        .fetch_optional(&mut *tx)
        .await
        .map_err(classify_sqlx_error_owned)?;

        if exists.is_none() {
            tx.commit().await.map_err(classify_sqlx_error_owned)?;
            return Ok(false);
        }

        let usage_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*)::BIGINT FROM secret_usage_binding WHERE secret_id = $1",
        )
        .bind(secret_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|error| {
            AppError::Database(format!(
                "Failed to check usage for named secret {secret_id}: {error}"
            ))
        })?;

        if usage_count > 0 {
            tx.rollback().await.map_err(classify_sqlx_error_owned)?;
            return Err(AppError::Conflict(format!(
                "Secret is still used by {usage_count} reference(s); remove them first"
            )));
        }

        // `secrets_store` and `secrets_binding` cascade on delete.
        let result = sqlx::query("DELETE FROM secrets_variable WHERE id = $1 AND kind = $2")
            .bind(secret_id)
            .bind(SECRET_KIND_NAMED)
            .execute(&mut *tx)
            .await
            .map_err(|error| {
                AppError::Database(format!(
                    "Failed to delete named secret {secret_id}: {error}"
                ))
            })?;
        tx.commit().await.map_err(classify_sqlx_error_owned)?;

        Ok(result.rows_affected() > 0)
    }

    async fn list_bindings_for_secret(
        &self,
        secret_id: Uuid,
    ) -> Result<Vec<SecretBinding>, AppError> {
        let rows = sqlx::query(
            r#"
            SELECT id, secret_id, scope_type, scope_id, weight, managed_by_field_usage
            FROM secrets_binding
            WHERE secret_id = $1
            ORDER BY scope_type, scope_id, weight, id
            "#,
        )
        .bind(secret_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to list secret bindings: {e}")))?;

        rows.iter()
            .map(Self::parse_binding_row)
            .collect::<Result<Vec<_>, AppError>>()
    }

    async fn list_bindings_for_scope(
        &self,
        scope_type: &str,
        scope_id: Uuid,
    ) -> Result<Vec<SecretBinding>, AppError> {
        let rows = sqlx::query(
            r#"
            SELECT id, secret_id, scope_type, scope_id, weight, managed_by_field_usage
            FROM secrets_binding
            WHERE scope_type = $1
              AND scope_id = $2
            ORDER BY weight, id
            "#,
        )
        .bind(scope_type)
        .bind(scope_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to list secret bindings: {e}")))?;

        rows.iter()
            .map(Self::parse_binding_row)
            .collect::<Result<Vec<_>, AppError>>()
    }

    async fn list_bindings_for_scope_hierarchy(
        &self,
        scope_type: &str,
        scope_id: Uuid,
    ) -> Result<Vec<SecretBinding>, AppError> {
        let scopes = self.build_scope_hierarchy(scope_type, scope_id).await?;
        if scopes.is_empty() {
            return Ok(Vec::new());
        }

        let mut builder = QueryBuilder::<Postgres>::new(
            "SELECT id, secret_id, scope_type, scope_id, weight, managed_by_field_usage FROM secrets_binding WHERE ",
        );
        let mut first = true;
        for (scope_type, scope_id) in scopes {
            if !first {
                builder.push(" OR ");
            }
            builder.push("(scope_type = ");
            builder.push_bind(scope_type);
            builder.push(" AND scope_id = ");
            builder.push_bind(scope_id);
            builder.push(")");
            first = false;
        }
        builder.push(" ORDER BY weight, id");

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list secret bindings: {e}")))?;

        rows.iter()
            .map(Self::parse_binding_row)
            .collect::<Result<Vec<_>, AppError>>()
    }

    async fn list_scope_secrets(
        &self,
        scope_type: &str,
        scope_id: Uuid,
    ) -> Result<SecretScopeSecrets, AppError> {
        let scopes = self.build_scope_hierarchy(scope_type, scope_id).await?;
        if scopes.is_empty() {
            return Ok(SecretScopeSecrets {
                variables: Vec::new(),
                bindings: Vec::new(),
            });
        }

        let mut builder = QueryBuilder::<Postgres>::new(
            "SELECT sv.id, sv.key, sv.is_masked, sv.is_protected, sv.kind, sv.description, sv.retire_with_field_usage, \
             sb.id AS binding_id, sb.secret_id, sb.scope_type, sb.scope_id, sb.weight, sb.managed_by_field_usage \
             FROM secrets_variable sv \
             JOIN secrets_binding sb ON sb.secret_id = sv.id \
             WHERE ",
        );
        let mut first = true;
        for (scope_type, scope_id) in scopes {
            if !first {
                builder.push(" OR ");
            }
            builder.push("(sb.scope_type = ");
            builder.push_bind(scope_type);
            builder.push(" AND sb.scope_id = ");
            builder.push_bind(scope_id);
            builder.push(")");
            first = false;
        }
        builder.push(" ORDER BY sv.key, sb.weight, sb.id");

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list scoped secrets: {e}")))?;

        let mut variables_map: HashMap<Uuid, SecretVariable> = HashMap::new();
        let mut bindings = Vec::new();
        for row in rows {
            let variable = Self::parse_variable_row(&row)?;
            variables_map.entry(variable.id).or_insert(variable);
            bindings.push(SecretBinding {
                id: row.try_get("binding_id")?,
                secret_id: row.try_get("secret_id")?,
                scope_type: row.try_get("scope_type")?,
                scope_id: row.try_get("scope_id")?,
                weight: row.try_get("weight")?,
                managed_by_field_usage: row.try_get("managed_by_field_usage")?,
            });
        }

        let unbound_rows = sqlx::query(
            r#"
            SELECT sv.id, sv.key, sv.is_masked, sv.is_protected, sv.kind, sv.description, sv.retire_with_field_usage
            FROM secrets_variable sv
            WHERE NOT EXISTS (
                SELECT 1 FROM secrets_binding sb WHERE sb.secret_id = sv.id
            )
            ORDER BY sv.key, sv.id
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to list global secrets: {e}")))?;

        for row in unbound_rows {
            let variable = Self::parse_variable_row(&row)?;
            variables_map.entry(variable.id).or_insert(variable);
        }

        let mut variables = variables_map.into_values().collect::<Vec<_>>();
        variables.sort_by(|a, b| a.key.cmp(&b.key).then(a.id.cmp(&b.id)));

        Ok(SecretScopeSecrets {
            variables,
            bindings,
        })
    }

    async fn create_binding(&self, binding: SecretBinding) -> Result<SecretBinding, AppError> {
        let mut transaction = self.pool.begin().await?;
        let variable = sqlx::query!(
            "SELECT id FROM secrets_variable WHERE id = $1 FOR UPDATE",
            binding.secret_id
        )
        .fetch_optional(&mut *transaction)
        .await?;
        if variable.is_none() {
            return Err(AppError::NotFound("Secret variable not found".into()));
        }
        // Explicit retention promotes the existing managed row under retirement's variable lock.
        let promoted = sqlx::query_as!(SecretBinding, "UPDATE secrets_binding SET managed_by_field_usage = false, weight = $4 WHERE secret_id = $1 AND scope_type = $2 AND scope_id = $3 AND managed_by_field_usage RETURNING id, secret_id, scope_type, scope_id, weight, managed_by_field_usage", binding.secret_id, binding.scope_type, binding.scope_id, binding.weight)
            .fetch_optional(&mut *transaction).await?;
        if let Some(saved) = promoted {
            transaction.commit().await?;
            return Ok(saved);
        }
        let saved = sqlx::query_as!(SecretBinding, "INSERT INTO secrets_binding (id, secret_id, scope_type, scope_id, weight, managed_by_field_usage) VALUES ($1, $2, $3, $4, $5, false) RETURNING id, secret_id, scope_type, scope_id, weight, managed_by_field_usage", binding.id, binding.secret_id, binding.scope_type, binding.scope_id, binding.weight)
            .fetch_one(&mut *transaction).await?;
        transaction.commit().await?;
        Ok(saved)
    }

    async fn delete_binding(&self, binding_id: Uuid) -> Result<bool, AppError> {
        let result = sqlx::query("DELETE FROM secrets_binding WHERE id = $1")
            .bind(binding_id)
            .execute(&self.pool)
            .await?;

        Ok(result.rows_affected() > 0)
    }

    async fn get_usage_binding(
        &self,
        consumer_type: &str,
        consumer_id: Uuid,
        field_key: &str,
    ) -> Result<Option<SecretUsageBinding>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, consumer_type, consumer_id, field_key, owner_scope_type, owner_scope_id, secret_id
            FROM secret_usage_binding
            WHERE consumer_type = $1
              AND consumer_id = $2
              AND field_key = $3
            LIMIT 1
            "#,
        )
        .bind(consumer_type)
        .bind(consumer_id)
        .bind(field_key)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| {
            AppError::Database(format!(
                "Failed to load secret usage binding for {consumer_type}:{consumer_id}:{field_key}: {error}"
            ))
        })?;

        row.map(|row| Self::parse_usage_binding_row(&row))
            .transpose()
    }

    async fn list_usage_bindings_for_consumer(
        &self,
        consumer_type: &str,
        consumer_id: Uuid,
    ) -> Result<Vec<SecretUsageBinding>, AppError> {
        let rows = sqlx::query(
            r#"
            SELECT id, consumer_type, consumer_id, field_key, owner_scope_type, owner_scope_id, secret_id
            FROM secret_usage_binding
            WHERE consumer_type = $1
              AND consumer_id = $2
            ORDER BY field_key, id
            "#,
        )
        .bind(consumer_type)
        .bind(consumer_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|error| {
            AppError::Database(format!(
                "Failed to list secret usage bindings for {consumer_type}:{consumer_id}: {error}"
            ))
        })?;

        rows.iter()
            .map(Self::parse_usage_binding_row)
            .collect::<Result<Vec<_>, AppError>>()
    }

    async fn list_usage_bindings_for_consumers(
        &self,
        consumer_type: &str,
        consumer_ids: &[Uuid],
    ) -> Result<Vec<SecretUsageBinding>, AppError> {
        if consumer_ids.is_empty() {
            return Ok(Vec::new());
        }
        let rows = sqlx::query(
            r#"
            SELECT id, consumer_type, consumer_id, field_key, owner_scope_type, owner_scope_id, secret_id
            FROM secret_usage_binding
            WHERE consumer_type = $1
              AND consumer_id = ANY($2)
            ORDER BY consumer_id, field_key, id
            "#,
        )
        .bind(consumer_type)
        .bind(consumer_ids)
        .fetch_all(&self.pool)
        .await
        .map_err(|error| {
            AppError::Database(format!(
                "Failed to list secret usage bindings for {consumer_type} page: {error}"
            ))
        })?;

        rows.iter()
            .map(Self::parse_usage_binding_row)
            .collect::<Result<Vec<_>, AppError>>()
    }

    async fn upsert_usage_binding(
        &self,
        binding: SecretUsageBinding,
    ) -> Result<SecretUsageBinding, AppError> {
        let row = sqlx::query(
            r#"
            INSERT INTO secret_usage_binding (
                id,
                consumer_type,
                consumer_id,
                field_key,
                owner_scope_type,
                owner_scope_id,
                secret_id
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT (consumer_type, consumer_id, field_key)
            DO UPDATE SET
                owner_scope_type = EXCLUDED.owner_scope_type,
                owner_scope_id = EXCLUDED.owner_scope_id,
                secret_id = EXCLUDED.secret_id,
                updated_at = NOW()
            RETURNING id, consumer_type, consumer_id, field_key, owner_scope_type, owner_scope_id, secret_id
            "#,
        )
        .bind(binding.id)
        .bind(&binding.consumer_type)
        .bind(binding.consumer_id)
        .bind(&binding.field_key)
        .bind(&binding.owner_scope_type)
        .bind(binding.owner_scope_id)
        .bind(binding.secret_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|error| {
            AppError::Database(format!(
                "Failed to upsert secret usage binding {}: {}",
                binding.id, error
            ))
        })?;

        Self::parse_usage_binding_row(&row)
    }

    async fn delete_usage_binding(&self, binding_id: Uuid) -> Result<bool, AppError> {
        let result = sqlx::query("DELETE FROM secret_usage_binding WHERE id = $1")
            .bind(binding_id)
            .execute(&self.pool)
            .await
            .map_err(|error| {
                AppError::Database(format!(
                    "Failed to delete secret usage binding {binding_id}: {error}"
                ))
            })?;

        Ok(result.rows_affected() > 0)
    }
}
