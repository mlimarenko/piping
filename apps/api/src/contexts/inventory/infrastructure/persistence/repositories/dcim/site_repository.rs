use sqlx::PgPool;
use sqlx::postgres::PgRow;
use uuid::Uuid;

use sqlx::{Postgres, QueryBuilder};

use super::super::super::list_query::{
    ListColumn, push_custom_field_filters, push_id_filter, push_order_by, push_string_in,
    push_uuid_in,
};
use super::super::helpers::derive_slug;
use super::super::repository::{SiteFilter, SiteRepository};
use crate::contexts::inventory::domain::{Site, SiteListView};
use crate::contexts::shared::list_query::WhereState;
use crate::contexts::shared::pagination::{clamp_limit, clamp_offset};
use crate::contexts::shared::row::col;
use crate::errors::AppError;

pub const SITE_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "s.name", "inventory.sites.columns.name"),
    ListColumn::sortable("slug", "s.slug", "inventory.sites.columns.slug"),
    ListColumn::enum_filter(
        "status",
        "s.status",
        "s.status",
        "inventory.sites.columns.status",
    ),
    ListColumn::fk_filter(
        "tenant",
        "t.name",
        "tenant_id",
        "s.tenant_id",
        "tenants",
        "inventory.sites.columns.tenant",
    ),
    ListColumn::fk_filter(
        "region",
        "r.name",
        "region_id",
        "s.region_id",
        "regions",
        "inventory.sites.columns.region",
    ),
    ListColumn::fk_filter(
        "site_group",
        "sg.name",
        "site_group_id",
        "s.group_id",
        "site-groups",
        "inventory.sites.columns.siteGroup",
    ),
    ListColumn::sortable("facility", "s.facility", "inventory.sites.columns.facility"),
    ListColumn::sortable(
        "time_zone",
        "s.time_zone",
        "inventory.sites.columns.timeZone",
    ),
    ListColumn::sortable("latitude", "s.latitude", "inventory.sites.columns.latitude"),
    ListColumn::sortable(
        "longitude",
        "s.longitude",
        "inventory.sites.columns.longitude",
    ),
];

/// The one projection `parse_site_row` understands. Kept as a single constant
/// so the SELECT list and the parser cannot drift apart per call site.
/// (`SITE_COLUMNS` above is the DataTable filter allowlist — a different thing.)
const SITE_ROW_COLUMNS: &str = "id, external_id, name, slug, status, tenant_id, region_id, \
     facility, time_zone, group_id, description, physical_address, \
     shipping_address, latitude::float8 AS latitude, \
     longitude::float8 AS longitude, comments, asns, tags, \
     custom_field_data, piping_synced_at";

#[derive(Clone)]
pub struct SqlxSiteRepository {
    pool: PgPool,
}

impl SqlxSiteRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Parse one `SITE_COLUMNS` row. Every caller selects exactly that list,
    /// so a missing column or a type mismatch is a programming error and fails
    /// the request with the column name — nothing is silently `None`.
    /// Nullability lives in the requested type: several columns are `NOT NULL`
    /// in the schema but optional in the NetBox-shaped domain model, and
    /// reading them as `Option` keeps that contract without swallowing errors.
    fn parse_site_row(row: &PgRow) -> Result<Site, AppError> {
        Ok(Site {
            id: col(row, "id")?,
            external_id: col(row, "external_id")?,
            name: col(row, "name")?,
            slug: col(row, "slug")?,
            status: col(row, "status")?,
            tenant_id: col(row, "tenant_id")?,
            region_id: col(row, "region_id")?,
            site_group_id: col(row, "group_id")?,
            facility: col(row, "facility")?,
            time_zone: col(row, "time_zone")?,
            description: col(row, "description")?,
            physical_address: col(row, "physical_address")?,
            shipping_address: col(row, "shipping_address")?,
            latitude: col(row, "latitude")?,
            longitude: col(row, "longitude")?,
            comments: col(row, "comments")?,
            asns: col(row, "asns")?,
            tags: col(row, "tags")?,
            custom_fields: col(row, "custom_field_data")?,
            piping_synced_at: col(row, "piping_synced_at")?,
        })
    }
}

impl SiteRepository for SqlxSiteRepository {
    async fn get_by_id(&self, site_id: Uuid) -> Result<Option<Site>, AppError> {
        let row = sqlx::query(&format!(
            "SELECT {SITE_ROW_COLUMNS} FROM dcim_site WHERE id = $1"
        ))
        .bind(site_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to load site: {}", e)))?;

        match row {
            Some(row) => Ok(Some(Self::parse_site_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn list_paginated(
        &self,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<Site>, AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let rows = sqlx::query(&format!(
            "SELECT {SITE_ROW_COLUMNS} FROM dcim_site ORDER BY name LIMIT $1 OFFSET $2"
        ))
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to list sites: {}", e)))?;

        rows.iter()
            .map(Self::parse_site_row)
            .collect::<Result<Vec<Site>, AppError>>()
    }

    async fn list_hydrated(
        &self,
        filter: SiteFilter,
        limit: Option<u32>,
        offset: Option<u32>,
        search: Option<&str>,
    ) -> Result<(Vec<SiteListView>, i64), AppError> {
        let limit = clamp_limit(limit) as i64;
        let offset = clamp_offset(offset) as i64;

        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT s.id, s.name, s.slug, s.status,
                   s.tenant_id, t.name AS tenant_name,
                   s.region_id, r.name AS region_name,
                   s.group_id AS site_group_id, sg.name AS site_group_name,
                   s.facility, s.time_zone, s.description, s.physical_address,
                   s.shipping_address, s.latitude::float8 AS latitude,
                   s.longitude::float8 AS longitude,
                   s.custom_field_data,
                   COUNT(*) OVER() AS total_count
            FROM dcim_site s
            LEFT JOIN tenancy_tenant t ON t.id = s.tenant_id
            LEFT JOIN dcim_region r ON r.id = s.region_id
            LEFT JOIN dcim_sitegroup sg ON sg.id = s.group_id
            "#,
        );

        let has_filter = filter.ids.is_some()
            || !filter.region_id.is_empty()
            || !filter.tenant_id.is_empty()
            || !filter.site_group_id.is_empty()
            || !filter.status.is_empty()
            || !filter.custom_fields.is_empty()
            || search.is_some();
        if has_filter {
            builder.push(" WHERE ");
            let mut where_state = WhereState::after_where();
            push_id_filter(
                &mut builder,
                &mut where_state,
                "s.id",
                filter.ids.as_deref(),
            );
            push_uuid_in(
                &mut builder,
                &mut where_state,
                "s.region_id",
                &filter.region_id,
            );
            push_uuid_in(
                &mut builder,
                &mut where_state,
                "s.tenant_id",
                &filter.tenant_id,
            );
            push_uuid_in(
                &mut builder,
                &mut where_state,
                "s.group_id",
                &filter.site_group_id,
            );
            push_string_in(&mut builder, &mut where_state, "s.status", &filter.status);
            push_custom_field_filters(
                &mut builder,
                &mut where_state,
                "s.custom_field_data",
                "s.id",
                &filter.custom_fields,
            );
            if let Some(search_term) = search {
                where_state.and(&mut builder);
                builder.push("s.name ILIKE ");
                builder.push_bind(search_term.to_owned());
            }
        }

        push_order_by(
            &mut builder,
            SITE_COLUMNS,
            filter.sort.as_ref(),
            "s.name ASC, s.id",
        );
        if filter.sort.is_some() {
            builder.push(", s.id");
        }
        builder.push(" LIMIT ");
        builder.push_bind(limit);
        builder.push(" OFFSET ");
        builder.push_bind(offset);

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AppError::Database(format!("Failed to list sites hydrated: {}", e)))?;

        // `unwrap_or(0)` on the window count would report an empty first page
        // on a decode error instead of failing; read it like every other column.
        let total: i64 = match rows.first() {
            Some(row) => col(row, "total_count")?,
            None => 0,
        };

        let items = rows
            .iter()
            .map(|row| {
                Ok(SiteListView {
                    id: col(row, "id")?,
                    name: col(row, "name")?,
                    slug: col(row, "slug")?,
                    status: col(row, "status")?,
                    tenant_id: col(row, "tenant_id")?,
                    tenant_name: col(row, "tenant_name")?,
                    region_id: col(row, "region_id")?,
                    region_name: col(row, "region_name")?,
                    site_group_id: col(row, "site_group_id")?,
                    site_group_name: col(row, "site_group_name")?,
                    facility: col(row, "facility")?,
                    time_zone: col(row, "time_zone")?,
                    description: col(row, "description")?,
                    physical_address: col(row, "physical_address")?,
                    shipping_address: col(row, "shipping_address")?,
                    latitude: col(row, "latitude")?,
                    longitude: col(row, "longitude")?,
                    custom_fields: col(row, "custom_field_data")?,
                })
            })
            .collect::<Result<Vec<SiteListView>, AppError>>()?;

        Ok((items, total))
    }

    async fn create(&self, site: &Site) -> Result<(), AppError> {
        let slug = site.slug.clone().unwrap_or_else(|| derive_slug(&site.name));
        sqlx::query(
            r#"
            INSERT INTO dcim_site (
                id,
                custom_field_data,
                name,
                slug,
                status,
                tenant_id,
                region_id,
                facility,
                time_zone,
                group_id,
                physical_address,
                shipping_address,
                latitude,
                longitude,
                description,
                comments,
                asns,
                tags,
                external_id,
                piping_synced_at
            )
            VALUES (
                $1, COALESCE($2, '{}'::jsonb), $3, $4, $5, $6, $7, $8, $9,
                $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20
            )
            "#,
        )
        .bind(site.id)
        .bind(site.custom_fields.clone())
        .bind(&site.name)
        .bind(slug.as_str())
        .bind(site.status.as_deref().unwrap_or("active"))
        .bind(site.tenant_id)
        .bind(site.region_id)
        .bind(site.facility.as_deref().unwrap_or(""))
        .bind(site.time_zone.as_deref())
        .bind(site.site_group_id)
        .bind(site.physical_address.as_deref().unwrap_or(""))
        .bind(site.shipping_address.as_deref().unwrap_or(""))
        .bind(site.latitude)
        .bind(site.longitude)
        .bind(site.description.as_deref().unwrap_or(""))
        .bind(site.comments.as_deref().unwrap_or(""))
        .bind(&site.asns)
        .bind(&site.tags)
        .bind(site.external_id)
        .bind(site.piping_synced_at)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create site: {}", e)))?;
        Ok(())
    }

    async fn update(&self, site: &Site) -> Result<(), AppError> {
        let slug = site.slug.clone().unwrap_or_else(|| derive_slug(&site.name));
        sqlx::query(
            r#"
            UPDATE dcim_site
            SET name = $2,
                slug = $3,
                status = $4,
                tenant_id = $5,
                region_id = $6,
                facility = $7,
                time_zone = $8,
                group_id = $9,
                physical_address = $10,
                shipping_address = $11,
                latitude = $12,
                longitude = $13,
                description = $14,
                comments = $15,
                asns = $16,
                tags = $17,
                custom_field_data = COALESCE($18, custom_field_data),
                external_id = COALESCE($19, external_id),
                piping_synced_at = COALESCE($20, piping_synced_at)
            WHERE id = $1
            "#,
        )
        .bind(site.id)
        .bind(&site.name)
        .bind(slug.as_str())
        .bind(site.status.as_deref().unwrap_or("active"))
        .bind(site.tenant_id)
        .bind(site.region_id)
        .bind(site.facility.as_deref().unwrap_or(""))
        .bind(site.time_zone.as_deref())
        .bind(site.site_group_id)
        .bind(site.physical_address.as_deref().unwrap_or(""))
        .bind(site.shipping_address.as_deref().unwrap_or(""))
        .bind(site.latitude)
        .bind(site.longitude)
        .bind(site.description.as_deref().unwrap_or(""))
        .bind(site.comments.as_deref().unwrap_or(""))
        .bind(&site.asns)
        .bind(&site.tags)
        .bind(site.custom_fields.clone())
        .bind(site.external_id)
        .bind(site.piping_synced_at)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update site: {}", e)))?;
        Ok(())
    }

    async fn delete(&self, site_id: Uuid) -> Result<(), AppError> {
        super::super::delete_guard::delete_inventory_record(&self.pool, "dcim_site", site_id)
            .await
            .map(|_| ())
    }
}
