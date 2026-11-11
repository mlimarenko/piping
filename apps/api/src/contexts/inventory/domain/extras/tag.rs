use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Native inventory membership input: omission is owned by the enclosing
/// payload; a supplied value is a non-null set of canonical catalog UUIDs.
pub fn membership_schema() -> utoipa::openapi::schema::Array {
    use utoipa::openapi::schema::{ArrayBuilder, KnownFormat, ObjectBuilder, SchemaFormat, Type};
    ArrayBuilder::new()
        .items(
            ObjectBuilder::new()
                .schema_type(Type::String)
                .format(Some(SchemaFormat::KnownFormat(KnownFormat::Uuid)))
                .pattern(Some(
                    "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$",
                )),
        )
        .unique_items(true)
        .build()
}

/// Canonical inventory tag definition.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct Tag {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    pub color: String,
    pub description: String,
    pub weight: i32,
    /// Allowed registered inventory content-type tokens. Empty permits every
    /// native taggable target; narrowing cannot detach existing memberships.
    pub object_types: Vec<String>,
}
