use sqlx::{PgPool, Postgres, QueryBuilder, Row, postgres::PgRow};
use uuid::Uuid;

use crate::contexts::inventory::domain::dcim::module_bay_template::{
    ModuleBayTemplate, ModuleBayTemplateChanges,
};
use crate::contexts::shared::list_query::{WhereState, push_search_ilike, push_uuid_in};
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::errors::{AppError, classify_sqlx_error_owned};

macro_rules! module_bay_template_projection { () => { "t.*,ARRAY(SELECT modulebaytype_id FROM dcim_modulebaytemplate_module_bay_types WHERE modulebaytemplate_id=t.id ORDER BY modulebaytype_id) AS module_bay_types" }; }

#[derive(Clone)]
pub struct SqlxModuleBayTemplateRepository {
    pool: PgPool,
}

impl SqlxModuleBayTemplateRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn parse(row: &PgRow) -> Result<ModuleBayTemplate, AppError> {
        Ok(ModuleBayTemplate {
            id: row.try_get("id")?,
            name: row.try_get("name")?,
            label: row.try_get("label")?,
            position: row.try_get("position")?,
            enabled: row.try_get("enabled")?,
            description: row.try_get("description")?,
            device_type_id: row.try_get("device_type_id")?,
            module_type_id: row.try_get("module_type_id")?,
            module_bay_types: row.try_get("module_bay_types")?,
        })
    }

    pub async fn get(&self, id: Uuid) -> Result<Option<ModuleBayTemplate>, AppError> {
        sqlx::query(concat!(
            "SELECT ",
            module_bay_template_projection!(),
            " FROM dcim_modulebaytemplate t WHERE id=$1"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(classify_sqlx_error_owned)?
        .as_ref()
        .map(Self::parse)
        .transpose()
    }

    pub async fn list(
        &self,
        devices: &[Uuid],
        modules: &[Uuid],
        search: Option<String>,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<(Vec<ModuleBayTemplate>, i64), AppError> {
        fn filters(
            q: &mut QueryBuilder<Postgres>,
            devices: &[Uuid],
            modules: &[Uuid],
            search: Option<&str>,
        ) {
            let mut state = WhereState::new();
            push_uuid_in(q, &mut state, "t.device_type_id", devices);
            push_uuid_in(q, &mut state, "t.module_type_id", modules);
            push_search_ilike(
                q,
                &mut state,
                &["t.name", "t.label", "t.description"],
                search,
            );
        }
        let mut count = QueryBuilder::new("SELECT COUNT(*) FROM dcim_modulebaytemplate t");
        filters(&mut count, devices, modules, search.as_deref());
        let total = count
            .build_query_scalar()
            .fetch_one(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let mut rows = QueryBuilder::new(concat!(
            "SELECT ",
            module_bay_template_projection!(),
            " FROM dcim_modulebaytemplate t"
        ));
        filters(&mut rows, devices, modules, search.as_deref());
        rows.push(" ORDER BY t.name,t.id LIMIT ")
            .push_bind(i64::from(clamp_limit(limit)))
            .push(" OFFSET ")
            .push_bind(i64::from(clamp_offset(offset)));
        let rows = rows
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        Ok((
            rows.iter().map(Self::parse).collect::<Result<_, _>>()?,
            total,
        ))
    }

    pub async fn save(
        &self,
        id: Uuid,
        changes: ModuleBayTemplateChanges,
        create: bool,
    ) -> Result<Option<ModuleBayTemplate>, AppError> {
        let mut tx = self.pool.begin().await?;
        super::hardware_graph::lock_hardware_graph(&mut tx).await?;
        let mut item = if create {
            ModuleBayTemplate {
                id,
                enabled: true,
                ..Default::default()
            }
        } else {
            let row = sqlx::query(concat!(
                "SELECT ",
                module_bay_template_projection!(),
                " FROM dcim_modulebaytemplate t WHERE id=$1 FOR UPDATE"
            ))
            .bind(id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
            let Some(row) = row else {
                return Ok(None);
            };
            Self::parse(&row)?
        };
        if let Some(value) = changes.name {
            item.name = value;
        }
        if let Some(value) = changes.label {
            item.label = value;
        }
        if let Some(value) = changes.position {
            item.position = value;
        }
        if let Some(value) = changes.description {
            item.description = value;
        }
        if let Some(value) = changes.enabled {
            item.enabled = value;
        }
        if let Some(value) = changes.device_type_id {
            item.device_type_id = value;
        }
        if let Some(value) = changes.module_type_id {
            item.module_type_id = value;
        }
        if item.device_type_id.is_none() == item.module_type_id.is_none() {
            return Err(AppError::Validation(
                "Exactly one hardware template parent is required".into(),
            ));
        }
        for (field, value, limit) in [
            ("name", item.name.as_str(), 64),
            ("label", item.label.as_str(), 64),
            ("position", item.position.as_str(), 30),
            ("description", item.description.as_str(), 200),
        ] {
            if value.chars().count() > limit || (field == "name" && value.trim().is_empty()) {
                return Err(AppError::Validation(format!(
                    "Invalid {field}: maximum {limit} characters"
                )));
            }
        }
        let statement = if create {
            "INSERT INTO dcim_modulebaytemplate (id,name,label,position,enabled,description,device_type_id,module_type_id) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)"
        } else {
            "UPDATE dcim_modulebaytemplate SET name=$2,label=$3,position=$4,enabled=$5,description=$6,device_type_id=$7,module_type_id=$8 WHERE id=$1"
        };
        sqlx::query(statement)
            .bind(id)
            .bind(&item.name)
            .bind(&item.label)
            .bind(&item.position)
            .bind(item.enabled)
            .bind(&item.description)
            .bind(item.device_type_id)
            .bind(item.module_type_id)
            .execute(&mut *tx)
            .await
            .map_err(classify_sqlx_error_owned)?;
        super::module_bay_membership::replace(
            &mut tx,
            super::module_bay_membership::ModuleBayCategoryOwner::Template,
            id,
            changes.module_bay_types.as_ref(),
        )
        .await?;
        if let Some(ids) = changes.module_bay_types {
            item.module_bay_types = ids
                .ok_or_else(|| AppError::Validation("module_bay_types must be an array".into()))?;
            item.module_bay_types.sort();
        }
        tx.commit().await.map_err(classify_sqlx_error_owned)?;
        Ok(Some(item))
    }

    pub async fn delete(&self, id: Uuid) -> Result<(), AppError> {
        if super::super::delete_guard::delete_inventory_record(
            &self.pool,
            "dcim_modulebaytemplate",
            id,
        )
        .await?
            == 0
        {
            return Err(AppError::NotFound("Module bay template not found".into()));
        }
        Ok(())
    }
}
