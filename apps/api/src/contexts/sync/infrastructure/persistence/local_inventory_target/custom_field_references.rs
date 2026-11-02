//! Import object custom fields through qualified source bindings, never UUID guesses.

use super::*;

fn invalid_reference() -> AppError {
    AppError::Validation("inventory_transfer:custom_field_reference_invalid".into())
}

/// Offline validation can establish the type token but cannot invent its local
/// content-type UUID. The nil marker must be replaced by `resolve_definition_type`
/// before a definition is exposed to value planning or persistence.
pub(super) fn source_related_type(record: &SourceRecord) -> Result<Option<Uuid>, AppError> {
    match record.fields.get("related_object_type") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(token)) if InventoryEntity::from_content_type_token(token).is_some() => {
            // Defaults have their own binding surface. Until that surface is
            // resolved, even a UUID-shaped source default is not a local UUID.
            if record.fields.get("default").is_some_and(|value| {
                !value.is_null() && value.as_array().is_none_or(|items| !items.is_empty())
            }) {
                return Err(invalid_reference());
            }
            Ok(Some(Uuid::nil()))
        }
        _ => Err(invalid_reference()),
    }
}

pub(super) fn type_binding(value: &Value) -> Result<ReferenceBinding, AppError> {
    let token = value.as_str().ok_or_else(invalid_reference)?;
    InventoryEntity::from_content_type_token(token).ok_or_else(invalid_reference)?;
    Ok(ReferenceBinding {
        target: None,
        type_token: Some(token.into()),
        external_id: String::new(),
    })
}

pub(super) async fn resolve_definition_type(
    transaction: &mut Transaction<'_, Postgres>,
    cache: &mut HashMap<String, Uuid>,
    row: &PreparedRow,
    definition: &mut CustomFieldDefinition,
) -> Result<(), AppError> {
    if !definition.field_type.is_object() {
        return Ok(());
    }
    let token = row
        .references
        .iter()
        .find(|(field, _)| field.name() == "related_object_type")
        .and_then(|(_, binding)| binding.type_token.as_deref())
        .ok_or_else(invalid_reference)?;
    definition.related_object_type_id = Some(
        observed_content_type(transaction, cache, token)
            .await?
            .ok_or_else(invalid_reference)?,
    );
    validate_definition(definition).map_err(|_| invalid_reference())
}

fn source_id(value: &Value, entity: InventoryEntity) -> Result<String, AppError> {
    let id = match value {
        Value::Object(object) => {
            if let Some(token) = object.get("object_type") {
                if token
                    .as_str()
                    .and_then(InventoryEntity::from_content_type_token)
                    != Some(entity)
                {
                    return Err(invalid_reference());
                }
            }
            object.get("id").ok_or_else(invalid_reference)?
        }
        value => value,
    };
    match id {
        Value::String(text) if !text.trim().is_empty() && text.len() <= 256 => Ok(text.clone()),
        Value::Number(number) if number.as_u64().is_some_and(|number| number > 0) => {
            Ok(number.to_string())
        }
        _ => Err(invalid_reference()),
    }
}

fn convert_object_value(
    field_type: CustomFieldType,
    value: &Value,
    entity: InventoryEntity,
    local_ids: &HashMap<(String, String), Uuid>,
) -> Result<(Value, Vec<Uuid>), AppError> {
    if value.is_null() {
        return Ok((Value::Null, Vec::new()));
    }
    let items: Vec<&Value> = match field_type {
        CustomFieldType::Object => vec![value],
        CustomFieldType::Multiobject => value
            .as_array()
            .ok_or_else(invalid_reference)?
            .iter()
            .collect(),
        _ => return Err(invalid_reference()),
    };
    let mut seen = HashSet::new();
    let mut ids = Vec::with_capacity(items.len());
    for item in items {
        let external_id = source_id(item, entity)?;
        let id = local_ids
            .get(&(entity.source_key().into(), external_id))
            .copied()
            .filter(|id| !id.is_nil())
            .ok_or_else(invalid_reference)?;
        if !seen.insert(id) {
            return Err(invalid_reference());
        }
        ids.push(id);
    }
    let converted = match field_type {
        CustomFieldType::Object => Value::String(ids[0].to_string()),
        CustomFieldType::Multiobject => {
            Value::Array(ids.iter().map(|id| Value::String(id.to_string())).collect())
        }
        _ => return Err(invalid_reference()),
    };
    Ok((converted, ids))
}

/// Resolves and validates the native UUID value and every referenced row. The
/// caller converts validation failures to hashed, field-specific findings.
pub(super) async fn resolve_value(
    transaction: &mut Transaction<'_, Postgres>,
    row: &PreparedRow,
    definition: &CustomFieldDefinition,
    value: &Value,
    local_ids: &HashMap<(String, String), Uuid>,
    incoming: &HashSet<(InventoryEntity, Uuid)>,
    field: &str,
    dependencies: &mut Vec<DependencyFact>,
) -> Result<Value, AppError> {
    if !definition.field_type.is_object() {
        return Ok(value.clone());
    }
    let type_id = definition
        .related_object_type_id
        .filter(|id| !id.is_nil())
        .ok_or_else(invalid_reference)?;
    let content_type: Option<(String, String)> =
        sqlx::query_as("SELECT app_label, model FROM inventory_content_type WHERE id=$1")
            .bind(type_id)
            .fetch_optional(&mut **transaction)
            .await?;
    let entity = content_type
        .as_ref()
        .and_then(|(app, model)| InventoryEntity::from_content_type(app, model))
        .ok_or_else(invalid_reference)?;
    let (converted, ids) = convert_object_value(definition.field_type, value, entity, local_ids)?;
    validate_value(definition, &converted, &[]).map_err(|_| invalid_reference())?;
    let from = local_ids
        .get(&(row.source_entity.clone(), row.external_id.clone()))
        .copied()
        .ok_or_else(invalid_reference)?;
    let mut resolved_edges = Vec::new();
    for id in ids {
        if !incoming.contains(&(entity, id)) {
            let table = entity.table().ok_or_else(invalid_reference)?;
            let exists: bool =
                sqlx::query_scalar(&format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE id=$1)"))
                    .bind(id)
                    .fetch_one(&mut **transaction)
                    .await?;
            if !exists {
                return Err(invalid_reference());
            }
        }
        resolved_edges.push(DependencyFact {
            from,
            from_entity: row.source_entity.clone(),
            to: id,
            to_entity: entity.source_key().into(),
            field: field.into(),
        });
    }
    dependencies.extend(resolved_edges);
    Ok(converted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn object_values_resolve_by_type_and_qualified_binding_not_native_uuid() {
        let device = Uuid::from_u128(1);
        let site = Uuid::from_u128(2);
        let ids = HashMap::from([
            (("dcim.devices".into(), "7".into()), device),
            (("dcim.sites".into(), "7".into()), site),
        ]);
        let (value, edges) = convert_object_value(
            CustomFieldType::Object,
            &json!({"id": 7, "object_type": "dcim.device", "display": "private"}),
            InventoryEntity::Device,
            &ids,
        )
        .expect("qualified object reference");
        assert_eq!(value, json!(device));
        assert_eq!(edges, vec![device]);
        assert!(
            convert_object_value(
                CustomFieldType::Object,
                &json!(device),
                InventoryEntity::Device,
                &ids,
            )
            .is_err()
        );
        assert!(
            convert_object_value(
                CustomFieldType::Object,
                &json!({"id": 7, "object_type": "dcim.site"}),
                InventoryEntity::Device,
                &ids,
            )
            .is_err()
        );
    }

    #[test]
    fn multiobject_preserves_null_empty_order_and_rejects_duplicate_or_missing_edges() {
        let ids = HashMap::from([
            (("dcim.devices".into(), "1".into()), Uuid::from_u128(10)),
            (("dcim.devices".into(), "2".into()), Uuid::from_u128(20)),
        ]);
        let convert = |value: Value| {
            convert_object_value(
                CustomFieldType::Multiobject,
                &value,
                InventoryEntity::Device,
                &ids,
            )
        };
        assert_eq!(convert(Value::Null).unwrap(), (Value::Null, vec![]));
        assert_eq!(convert(json!([])).unwrap(), (json!([]), vec![]));
        assert_eq!(
            convert(json!([2, 1])).unwrap().1,
            vec![Uuid::from_u128(20), Uuid::from_u128(10)]
        );
        for value in [
            json!([1, 1]),
            json!([3]),
            json!([true]),
            json!([1.2]),
            json!([0]),
        ] {
            assert!(convert(value).is_err());
        }
    }

    #[test]
    fn nil_binding_and_unknown_private_ids_fail_without_echoing_source_values() {
        let ids = HashMap::from([(("dcim.devices".into(), "private-id".into()), Uuid::nil())]);
        for value in [json!("private-id"), json!({"id": "private-missing"})] {
            let error = convert_object_value(
                CustomFieldType::Object,
                &value,
                InventoryEntity::Device,
                &ids,
            )
            .expect_err("unresolved references are blocked");
            let AppError::Validation(message) = error else {
                panic!("expected sanitized validation failure");
            };
            assert_eq!(message, "inventory_transfer:custom_field_reference_invalid");
        }
    }

    #[test]
    fn object_defaults_do_not_bypass_qualified_reference_conversion() {
        use crate::contexts::sync::domain::inventory_sync::SourceIdentity;
        let identity = SourceIdentity::new(
            Uuid::from_u128(1),
            SourceEntity::new("extras.custom-fields").unwrap(),
            "7",
        )
        .unwrap();
        let mut record = SourceRecord::new(
            identity,
            json!({"related_object_type": "dcim.device", "default": null})
                .as_object()
                .unwrap()
                .clone(),
        );
        assert_eq!(source_related_type(&record).unwrap(), Some(Uuid::nil()));
        record
            .fields
            .insert("default".into(), json!(Uuid::from_u128(9)));
        assert!(source_related_type(&record).is_err());
        record.fields.insert("default".into(), json!([]));
        assert_eq!(source_related_type(&record).unwrap(), Some(Uuid::nil()));
    }

    #[test]
    fn business_owner_types_do_not_alias_access_control_users() {
        for token in ["users.user", "auth.user", "core.owner"] {
            assert!(type_binding(&json!(token)).is_err());
        }
        let binding = type_binding(&json!("dcim.device")).unwrap();
        assert_eq!(binding.type_token.as_deref(), Some("dcim.device"));
        assert!(binding.target.is_none());
        assert!(type_binding(&json!("users.owner")).is_ok());
        let owner_id = Uuid::from_u128(17);
        let ids = HashMap::from([
            (("users.owners".into(), "5".into()), owner_id),
            (("users.users".into(), "5".into()), Uuid::from_u128(23)),
        ]);
        let (converted, edges) = convert_object_value(
            CustomFieldType::Object,
            &json!({"id": 5, "object_type": "users.owner"}),
            InventoryEntity::Owner,
            &ids,
        )
        .unwrap();
        assert_eq!(converted, json!(owner_id));
        assert_eq!(edges, vec![owner_id]);
    }
}
