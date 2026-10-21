use std::collections::{BTreeSet, HashMap};

use serde_json::Value;

use crate::contexts::sync::domain::{InventoryEntity, InventoryValue, SourceSnapshot};
use crate::errors::AppError;

use super::PreparedRow;

fn external_id(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_owned)
        .or_else(|| value.as_u64().map(|id| id.to_string()))
}

/// Embedded tags and the independent membership resource must describe one graph.
pub(super) fn validate(snapshot: &SourceSnapshot, rows: &[PreparedRow]) -> Result<(), AppError> {
    let tag_slugs: HashMap<_, _> = rows
        .iter()
        .filter(|row| row.entity == InventoryEntity::Tag)
        .map(|row| {
            let slug = row
                .entity
                .field("slug")
                .and_then(|field| row.scalars.get(&field))
                .and_then(|value| match value {
                    InventoryValue::Text { value } => Some(value.clone()),
                    _ => None,
                })
                .ok_or_else(|| AppError::Validation("Imported tag has no slug".into()))?;
            Ok((row.external_id.as_str(), slug))
        })
        .collect::<Result<_, AppError>>()?;
    let mut expected = BTreeSet::new();
    for record in &snapshot.records {
        if record.identity.entity.as_str() != InventoryEntity::TaggedObject.source_key() {
            continue;
        }
        let token = record
            .fields
            .get("object_type")
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::Validation("Tag membership has no object type".into()))?;
        let entity = InventoryEntity::from_content_type_token(token).ok_or_else(|| {
            AppError::Validation("Tag membership targets an unsupported object type".into())
        })?;
        let object_id = record
            .fields
            .get("object_id")
            .and_then(external_id)
            .ok_or_else(|| AppError::Validation("Tag membership has no object identity".into()))?;
        let tag_id = record
            .fields
            .get("tag")
            .and_then(|tag| tag.get("id"))
            .and_then(external_id)
            .ok_or_else(|| AppError::Validation("Tag membership has no tag identity".into()))?;
        let slug = tag_slugs.get(tag_id.as_str()).ok_or_else(|| {
            AppError::Validation("Tag membership references a missing tag".into())
        })?;
        if !expected.insert((entity.source_key().to_string(), object_id, slug.clone())) {
            return Err(AppError::Validation(
                "Duplicate source tag membership".into(),
            ));
        }
    }
    let actual: BTreeSet<_> = rows
        .iter()
        .flat_map(|row| {
            row.tags.iter().flatten().map(move |slug| {
                (
                    row.source_entity.clone(),
                    row.external_id.clone(),
                    slug.clone(),
                )
            })
        })
        .collect();
    if actual != expected {
        return Err(AppError::Validation(
            "Embedded tags differ from the complete tag membership resource".into(),
        ));
    }
    Ok(())
}
