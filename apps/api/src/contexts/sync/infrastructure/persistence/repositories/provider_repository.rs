use std::sync::Arc;

use serde_json::{Map, Value};
use sqlx::{Error as SqlxError, PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use super::repository::{ProviderListFilter, ProviderRepository};
use crate::contexts::secrets::application::{
    ApplySecretFieldIntentInput, SecretFieldIntent, SecretFieldIntentService,
};
use crate::contexts::secrets::infrastructure::persistence::SqlxSecretRepository;
use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, list_sort_column, push_string_in,
};
use crate::contexts::sync::domain::{
    Provider, ProviderType, SYNC_PROVIDER_CONSUMER_TYPE, SYNC_PROVIDER_FIELD_KEY_API_TOKEN,
    SYNC_PROVIDER_FIELD_KEY_WEBHOOK_SECRET, SYNC_PROVIDER_OWNER_SCOPE_TYPE,
};
use crate::errors::{AppError, classify_sqlx_error_owned};

pub const PROVIDER_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "name", "sources.columns.name"),
    ListColumn::enum_filter(
        "provider_type",
        "provider_type",
        "provider_type",
        "sources.columns.providerType",
    ),
    ListColumn::enum_filter(
        "is_enabled",
        "is_enabled",
        "is_enabled",
        "sources.columns.enabled",
    ),
];

#[derive(Clone)]
pub struct SqlxProviderRepository {
    pool: PgPool,
    secret_repository: SqlxSecretRepository,
}

#[derive(Debug)]
enum ProviderSecretCommand {
    Clear {
        field_key: &'static str,
    },
    Set {
        field_key: &'static str,
        value: String,
    },
}

impl SqlxProviderRepository {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool: pool.clone(),
            secret_repository: SqlxSecretRepository::new(pool.clone()),
        }
    }

    async fn apply_secret_commands(
        &self,
        provider_id: Uuid,
        commands: Vec<ProviderSecretCommand>,
    ) -> Result<(), AppError> {
        if commands.is_empty() {
            return Ok(());
        }

        let intent_service =
            SecretFieldIntentService::new(Arc::new(self.secret_repository.clone()));

        for command in commands {
            let (field_key, intent, value) = match command {
                ProviderSecretCommand::Clear { field_key } => {
                    (field_key.to_string(), SecretFieldIntent::Clear, None)
                }
                ProviderSecretCommand::Set { field_key, value } => {
                    (field_key.to_string(), SecretFieldIntent::Set, Some(value))
                }
            };

            let input = ApplySecretFieldIntentInput {
                consumer_type: SYNC_PROVIDER_CONSUMER_TYPE.to_string(),
                consumer_id: provider_id,
                owner_scope_type: SYNC_PROVIDER_OWNER_SCOPE_TYPE.to_string(),
                owner_scope_id: provider_id,
                field_key,
                intent,
                value,
                secret_id: None,
            };
            let _ = intent_service.apply_intent(input).await?;
        }

        Ok(())
    }

    fn filter_sql(param: &str) -> Option<&'static str> {
        PROVIDER_COLUMNS
            .iter()
            .find_map(|column| column.filter.filter(|filter| filter.param == param))
            .map(|filter| filter.sql)
    }

    fn push_filters(builder: &mut QueryBuilder<'_, Postgres>, filter: &ProviderListFilter) {
        let has_filter = !filter.provider_type.is_empty() || !filter.is_enabled.is_empty();
        if !has_filter {
            return;
        }

        builder.push(" WHERE ");
        let mut where_state = WhereState::after_where();

        if let Some(sql) = Self::filter_sql("provider_type") {
            push_string_in(builder, &mut where_state, sql, &filter.provider_type);
        }
        if !filter.is_enabled.is_empty() {
            where_state.and(builder);
            builder.push("is_enabled IN (");
            let mut separated = builder.separated(", ");
            for value in &filter.is_enabled {
                separated.push_bind(*value);
            }
            builder.push(")");
        }
    }
}

impl ProviderRepository for SqlxProviderRepository {
    async fn get_by_id(&self, provider_id: Uuid) -> Result<Option<Provider>, AppError> {
        let row = sqlx::query(
            r#"
            SELECT id, name, provider_type, config, is_enabled
            FROM sync_provider
            WHERE id = $1
            "#,
        )
        .bind(provider_id)
        .fetch_optional(&self.pool)
        .await?;

        let provider = row.map(|row| map_provider_row(&row)).transpose()?;
        let Some(provider) = provider else {
            return Ok(None);
        };
        Ok(Some(provider))
    }

    async fn list(&self) -> Result<Vec<Provider>, AppError> {
        self.list_paginated(i64::MAX as usize, 0, None, ProviderListFilter::default())
            .await
    }

    async fn list_paginated(
        &self,
        limit: usize,
        offset: usize,
        sort: Option<SortSpec>,
        filter: ProviderListFilter,
    ) -> Result<Vec<Provider>, AppError> {
        let mut builder = QueryBuilder::new(
            "SELECT id, name, provider_type, config, is_enabled FROM sync_provider",
        );
        Self::push_filters(&mut builder, &filter);
        let (sort_column, descending) = sort
            .as_ref()
            .and_then(|spec| {
                list_sort_column(PROVIDER_COLUMNS, &spec.key)
                    .map(|column| (column, spec.descending))
            })
            .unwrap_or(("name", false));
        builder.push(" ORDER BY ");
        builder.push(sort_column);
        builder.push(if descending { " DESC" } else { " ASC" });
        builder.push(", id ASC LIMIT ");
        builder.push_bind(limit as i64);
        builder.push(" OFFSET ");
        builder.push_bind(offset as i64);
        let rows = builder.build().fetch_all(&self.pool).await?;

        let mut providers = Vec::with_capacity(rows.len());
        for row in rows {
            let provider = map_provider_row(&row)?;
            providers.push(provider);
        }
        Ok(providers)
    }

    async fn count(&self, filter: ProviderListFilter) -> Result<usize, AppError> {
        let mut builder = QueryBuilder::new("SELECT COUNT(*) AS total FROM sync_provider");
        Self::push_filters(&mut builder, &filter);
        let row = builder.build().fetch_one(&self.pool).await?;
        let total: i64 = row.try_get("total")?;
        usize::try_from(total)
            .map_err(|_| AppError::Database("Provider count overflowed usize".to_string()))
    }

    async fn create(&self, provider: Provider) -> Result<Provider, AppError> {
        let (sanitized_config, secret_commands) =
            sanitize_and_extract_secret_commands(provider.config)?;

        let row = sqlx::query(
            r#"
            INSERT INTO sync_provider (id, name, provider_type, config, is_enabled)
            VALUES ($1, $2, $3, $4, $5)
            RETURNING id, name, provider_type, config, is_enabled
            "#,
        )
        .bind(provider.id)
        .bind(provider.name)
        .bind(provider_type_to_string(&provider.provider_type))
        .bind(sanitized_config)
        .bind(provider.is_enabled)
        .fetch_one(&self.pool)
        .await
        .map_err(map_provider_write_error)?;

        let created = map_provider_row(&row)?;
        self.apply_secret_commands(created.id, secret_commands)
            .await?;
        Ok(created)
    }

    async fn update(&self, provider: Provider) -> Result<Provider, AppError> {
        let (sanitized_config, secret_commands) =
            sanitize_and_extract_secret_commands(provider.config)?;

        let row = sqlx::query(
            r#"
            UPDATE sync_provider
            SET name = $2,
                provider_type = $3,
                config = $4,
                is_enabled = $5
            WHERE id = $1
            RETURNING id, name, provider_type, config, is_enabled
            "#,
        )
        .bind(provider.id)
        .bind(provider.name)
        .bind(provider_type_to_string(&provider.provider_type))
        .bind(sanitized_config)
        .bind(provider.is_enabled)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_provider_write_error)?;

        let updated = row
            .map(|row| map_provider_row(&row))
            .transpose()?
            .ok_or_else(|| AppError::NotFound("Provider not found".to_string()))?;
        self.apply_secret_commands(updated.id, secret_commands)
            .await?;
        Ok(updated)
    }

    async fn delete(&self, provider_id: Uuid) -> Result<(), AppError> {
        let mut transaction = self.pool.begin().await?;
        if sqlx::query!(
            "SELECT id FROM sync_provider WHERE id = $1 FOR UPDATE",
            provider_id
        )
        .fetch_optional(&mut *transaction)
        .await?
        .is_none()
        {
            return Err(AppError::NotFound("Provider not found".into()));
        }
        crate::contexts::secrets::infrastructure::persistence::repositories::secret_field_lifecycle::detach_consumer(
            &mut transaction, SYNC_PROVIDER_CONSUMER_TYPE, provider_id, SYNC_PROVIDER_OWNER_SCOPE_TYPE, provider_id,
        ).await?;
        let result = sqlx::query(
            r#"
            DELETE FROM sync_provider
            WHERE id = $1
            "#,
        )
        .bind(provider_id)
        .execute(&mut *transaction)
        .await?;

        if result.rows_affected() == 0 {
            return Err(AppError::NotFound("Provider not found".to_string()));
        }
        transaction.commit().await?;
        Ok(())
    }

    async fn set_enabled(&self, provider_id: Uuid, is_enabled: bool) -> Result<Provider, AppError> {
        let row = sqlx::query(
            r#"
            UPDATE sync_provider
            SET is_enabled = $2
            WHERE id = $1
            RETURNING id, name, provider_type, config, is_enabled
            "#,
        )
        .bind(provider_id)
        .bind(is_enabled)
        .fetch_optional(&self.pool)
        .await?;

        row.map(|row| map_provider_row(&row))
            .transpose()?
            .ok_or_else(|| AppError::NotFound("Provider not found".to_string()))
    }
}

fn map_provider_write_error(error: SqlxError) -> AppError {
    if let SqlxError::Database(db_error) = &error {
        let is_unique = db_error.code().map(|code| code == "23505").unwrap_or(false);
        let constraint = db_error.constraint().unwrap_or_default();
        if is_unique && constraint.contains("sync_provider_name") {
            return AppError::Conflict("Provider name already exists".to_string());
        }
    }
    // Fall back to the shared classifier so an outage surfaces as 503 and any
    // other unique violation still becomes a 409 instead of a blanket 500.
    classify_sqlx_error_owned(error)
}

fn map_provider_row(row: &sqlx::postgres::PgRow) -> Result<Provider, AppError> {
    let provider_type: String = row.try_get("provider_type")?;
    let provider_type = parse_provider_type(&provider_type)?;

    Ok(Provider {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        provider_type,
        config: row.try_get("config")?,
        is_enabled: row.try_get("is_enabled")?,
    })
}

fn parse_provider_type(value: &str) -> Result<ProviderType, AppError> {
    match value {
        "netbox" => Ok(ProviderType::Netbox),
        "custom" => Ok(ProviderType::Custom),
        other => Err(AppError::Validation(format!(
            "Unknown provider type '{}'",
            other
        ))),
    }
}

fn provider_type_to_string(value: &ProviderType) -> &'static str {
    match value {
        ProviderType::Netbox => "netbox",
        ProviderType::Custom => "custom",
    }
}

fn sanitize_and_extract_secret_commands(
    config: Value,
) -> Result<(Value, Vec<ProviderSecretCommand>), AppError> {
    let Value::Object(mut map) = config else {
        return Err(AppError::Validation(
            "Provider config must be a JSON object".to_string(),
        ));
    };

    let mut commands = Vec::new();
    if let Some(command) =
        extract_secret_command(&mut map, SYNC_PROVIDER_FIELD_KEY_API_TOKEN, "api_token")?
    {
        commands.push(command);
    }
    if let Some(command) = extract_secret_command(
        &mut map,
        SYNC_PROVIDER_FIELD_KEY_WEBHOOK_SECRET,
        "webhook_secret",
    )? {
        commands.push(command);
    }

    Ok((Value::Object(map), commands))
}

fn extract_secret_command(
    map: &mut Map<String, Value>,
    key: &'static str,
    label: &'static str,
) -> Result<Option<ProviderSecretCommand>, AppError> {
    let Some(raw_value) = map.remove(key) else {
        return Ok(None);
    };

    match raw_value {
        Value::String(value) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                return Ok(None);
            }
            Ok(Some(ProviderSecretCommand::Set {
                field_key: key,
                value: trimmed.to_string(),
            }))
        }
        Value::Null => Ok(Some(ProviderSecretCommand::Clear { field_key: key })),
        _ => Err(AppError::Validation(format!(
            "Provider config {label} must be a string or null",
        ))),
    }
}

// No "loose"/fallback extraction: provider configs must be sanitized on write.
