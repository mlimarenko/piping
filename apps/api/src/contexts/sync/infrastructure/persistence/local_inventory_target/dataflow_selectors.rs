//! Ordered typed selector rows of the optional `data-flows` namespace.
//!
//! `inventory_dataflow_selector` holds one row per selector with an owner
//! column (`dataflow_id` for flow endpoints, `group_id` for address-group
//! members), a side, a position and exactly one typed endpoint column. The
//! generic two-column junction writer cannot express that shape, so this module
//! owns the binder, the change check and the ordered replace write for
//! [`FieldKind::SelectorRows`].
//!
//! Every identity is resolved through the shared provider binding table by the
//! caller: a selector never carries an upstream id into a local column. Array
//! order is the only ordering authority, exactly as the native repository does
//! for `insert_side` and `replace_members`.

use std::collections::{HashMap, HashSet};

use serde_json::Value;
use sqlx::{Postgres, QueryBuilder, Row, Transaction};
use uuid::Uuid;

use crate::contexts::sync::domain::{FieldKind, FieldSpec, InventoryEntity};
use crate::errors::AppError;

use super::{ReferenceBinding, external_id_of};

/// One flow side admits 1..=32 selectors, the native endpoint bound.
const MAX_FLOW_SELECTORS: usize = 32;
/// One address group admits 1..=64 members, the native member bound.
const MAX_MEMBERS: usize = 64;
/// The only address mode the optional projection emits.
const ADDRESS_MODE: &str = "default";
/// The only keys a projected selector object may carry.
const SELECTOR_FIELDS: &[&str] = &["object_type", "object_id", "address_mode"];

fn reject(code: &'static str) -> AppError {
    AppError::Validation(format!("inventory_preflight:{code}"))
}

/// The declared shape of one selector field.
#[derive(Debug, Clone, Copy)]
struct SelectorShape {
    table: &'static str,
    owner_column: &'static str,
    side: &'static str,
    allowed: &'static [(&'static str, &'static str, InventoryEntity)],
}

impl SelectorShape {
    fn of(kind: &FieldKind) -> Option<Self> {
        match *kind {
            FieldKind::SelectorRows {
                table,
                owner_column,
                side,
                allowed,
            } => Some(Self {
                table,
                owner_column,
                side,
                allowed,
            }),
            _ => None,
        }
    }

    fn limit(self) -> usize {
        if self.side == "member" {
            MAX_MEMBERS
        } else {
            MAX_FLOW_SELECTORS
        }
    }

    fn column_of(self, object_type: &str) -> Option<&'static str> {
        self.allowed
            .iter()
            .find(|(token, _, _)| *token == object_type)
            .map(|(_, column, _)| *column)
    }

    fn entity_of(self, object_type: &str) -> Option<InventoryEntity> {
        self.allowed
            .iter()
            .find(|(token, _, _)| *token == object_type)
            .map(|(_, _, entity)| *entity)
    }

    /// Every endpoint column an existing row of this side may use.
    fn endpoint_columns(self) -> Vec<&'static str> {
        let mut columns = Vec::new();
        for (_, column, _) in self.allowed {
            if !columns.contains(column) {
                columns.push(*column);
            }
        }
        columns
    }
}

/// Bind one projected selector array into ordered provider references.
///
/// Every entry names its own admitted object type and provider identity; an
/// unknown key, object type, address mode or duplicate entry fails closed.
pub(super) fn bind(
    kind: &FieldKind,
    source_entity: &str,
    name: &str,
    value: &Value,
) -> Result<Vec<ReferenceBinding>, AppError> {
    let shape = SelectorShape::of(kind).ok_or_else(|| reject("selector_kind"))?;
    let items = value.as_array().ok_or_else(|| {
        AppError::Validation(format!("relation {source_entity}::{name} is not a list"))
    })?;
    if items.is_empty() || items.len() > shape.limit() {
        return Err(AppError::Validation(format!(
            "relation {source_entity}::{name} must contain 1..={} selectors",
            shape.limit()
        )));
    }
    let mut bindings = Vec::with_capacity(items.len());
    let mut seen = HashSet::new();
    for item in items {
        let object = item.as_object().ok_or_else(|| {
            AppError::Validation(format!(
                "relation {source_entity}::{name} has a non-object entry"
            ))
        })?;
        if object
            .keys()
            .any(|key| !SELECTOR_FIELDS.contains(&key.as_str()))
        {
            return Err(AppError::Validation(format!(
                "relation {source_entity}::{name} has an unapproved selector field"
            )));
        }
        let object_type = object
            .get("object_type")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                AppError::Validation(format!(
                    "relation {source_entity}::{name} entry has no object type"
                ))
            })?;
        let entity = shape.entity_of(object_type).ok_or_else(|| {
            AppError::Validation(format!(
                "relation {source_entity}::{name} does not admit object type '{object_type}'"
            ))
        })?;
        if let Some(mode) = object.get("address_mode")
            && mode.as_str() != Some(ADDRESS_MODE)
        {
            return Err(AppError::Validation(format!(
                "relation {source_entity}::{name} only admits the default address mode"
            )));
        }
        let external_id = object
            .get("object_id")
            .and_then(external_id_of)
            .ok_or_else(|| {
                AppError::Validation(format!(
                    "relation {source_entity}::{name} entry has no provider identity"
                ))
            })?;
        if !seen.insert((object_type.to_string(), external_id.clone())) {
            return Err(AppError::Validation(format!(
                "relation {source_entity}::{name} repeats one selector"
            )));
        }
        bindings.push(ReferenceBinding {
            target: Some(entity),
            type_token: Some(object_type.to_string()),
            external_id,
        });
    }
    Ok(bindings)
}

/// One resolved selector row, in source order.
#[derive(Debug, Clone)]
pub(super) struct PlannedSelectorRow {
    object_type: String,
    column: &'static str,
    local_id: Uuid,
}

/// The complete ordered selector set of one owner and side.
#[derive(Debug, Clone)]
pub(super) struct PlannedSelectorRows {
    table: &'static str,
    owner_column: &'static str,
    side: &'static str,
    allowed: &'static [(&'static str, &'static str, InventoryEntity)],
    rows: Vec<PlannedSelectorRow>,
}

impl PlannedSelectorRows {
    /// Hash the resolved selector set so the transfer pin covers it.
    pub(super) fn digest(&self) -> Value {
        let rows: Vec<_> = self
            .rows
            .iter()
            .map(|row| serde_json::json!([row.object_type, row.local_id]))
            .collect();
        serde_json::json!([self.table, self.side, rows])
    }

    fn shape(&self) -> SelectorShape {
        SelectorShape {
            table: self.table,
            owner_column: self.owner_column,
            side: self.side,
            allowed: self.allowed,
        }
    }
}

/// Resolve bound selector identities against the provider binding table.
pub(super) fn plan(
    field: &FieldSpec,
    bindings: &[ReferenceBinding],
    local_ids: &HashMap<(String, String), Uuid>,
) -> Result<PlannedSelectorRows, AppError> {
    let shape = SelectorShape::of(&field.kind).ok_or_else(|| reject("selector_kind"))?;
    let mut rows = Vec::with_capacity(bindings.len());
    for binding in bindings {
        let object_type = binding.type_token.as_deref().ok_or_else(|| {
            AppError::Validation(format!(
                "selector {}::{} has no object type",
                field.entity.source_key(),
                field.name
            ))
        })?;
        let column = shape.column_of(object_type).ok_or_else(|| {
            AppError::Validation(format!(
                "selector {}::{} admits no object type '{}'",
                field.entity.source_key(),
                field.name,
                object_type
            ))
        })?;
        let local_id = super::resolve_reference(binding, local_ids).ok_or_else(|| {
            AppError::Validation(format!(
                "Unresolved relation {}::{}",
                field.entity.source_key(),
                field.name
            ))
        })?;
        rows.push(PlannedSelectorRow {
            object_type: object_type.to_string(),
            column,
            local_id,
        });
    }
    Ok(PlannedSelectorRows {
        table: shape.table,
        owner_column: shape.owner_column,
        side: shape.side,
        allowed: shape.allowed,
        rows,
    })
}

/// Whether the stored rows already match the planned set exactly, in order.
pub(super) async fn changed(
    transaction: &mut Transaction<'_, Postgres>,
    owner_id: Uuid,
    planned: &PlannedSelectorRows,
) -> Result<bool, AppError> {
    let stored = stored_rows(transaction, owner_id, planned).await?;
    let desired: Vec<(String, Uuid)> = planned
        .rows
        .iter()
        .map(|row| (row.object_type.to_string(), row.local_id))
        .collect();
    Ok(stored != desired)
}

async fn stored_rows(
    transaction: &mut Transaction<'_, Postgres>,
    owner_id: Uuid,
    planned: &PlannedSelectorRows,
) -> Result<Vec<(String, Uuid)>, AppError> {
    let columns = planned.shape().endpoint_columns();
    let mut query = QueryBuilder::<Postgres>::new("SELECT object_type");
    for column in &columns {
        query.push(", ").push(*column);
    }
    query
        .push(" FROM ")
        .push(planned.table)
        .push(" WHERE ")
        .push(planned.owner_column)
        .push(" = ")
        .push_bind(owner_id)
        .push(" AND side = ")
        .push_bind(planned.side)
        .push(" ORDER BY position");
    let rows = query.build().fetch_all(&mut **transaction).await?;
    let mut stored = Vec::with_capacity(rows.len());
    for row in rows {
        let object_type: String = row.try_get("object_type")?;
        let mut id = None;
        for column in &columns {
            let value: Option<Uuid> = row.try_get(*column)?;
            if value.is_some() {
                id = value;
            }
        }
        let id = id.ok_or_else(|| reject("selector_endpoint"))?;
        stored.push((object_type, id));
    }
    Ok(stored)
}

/// Replace the owner's rows for one side when the ordered set differs.
///
/// The native repository replaces a side wholesale inside the same transaction;
/// this keeps that behaviour while skipping the write when nothing changed.
pub(super) async fn replace(
    transaction: &mut Transaction<'_, Postgres>,
    owner_id: Uuid,
    planned: &PlannedSelectorRows,
) -> Result<(), AppError> {
    if !changed(transaction, owner_id, planned).await? {
        return Ok(());
    }
    let mut delete = QueryBuilder::<Postgres>::new("DELETE FROM ");
    delete
        .push(planned.table)
        .push(" WHERE ")
        .push(planned.owner_column)
        .push(" = ")
        .push_bind(owner_id)
        .push(" AND side = ")
        .push_bind(planned.side);
    delete
        .build()
        .execute(&mut **transaction)
        .await
        .map_err(|error| AppError::Database(format!("Failed to clear selectors: {error}")))?;

    for (index, row) in planned.rows.iter().enumerate() {
        let position = i16::try_from(index + 1).map_err(|_| reject("selector_position"))?;
        let mut insert = QueryBuilder::<Postgres>::new("INSERT INTO ");
        insert
            .push(planned.table)
            .push(" (")
            .push(planned.owner_column)
            .push(", side, position, object_type, address_mode, ")
            .push(row.column)
            .push(") VALUES (")
            .push_bind(owner_id)
            .push(", ")
            .push_bind(planned.side)
            .push(", ")
            .push_bind(position)
            .push(", ")
            .push_bind(row.object_type.as_str())
            .push(", ")
            .push_bind(ADDRESS_MODE)
            .push(", ")
            .push_bind(row.local_id)
            .push(")");
        insert
            .build()
            .execute(&mut **transaction)
            .await
            .map_err(|error| AppError::Database(format!("Failed to write a selector: {error}")))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::contexts::sync::domain::{Absent, FieldKind, PgType};

    fn flow() -> FieldKind {
        FieldKind::SelectorRows {
            table: "inventory_dataflow_selector",
            owner_column: "dataflow_id",
            side: "source",
            allowed: &[(
                "inventory.addressgroup",
                "addressgroup_id",
                InventoryEntity::AddressGroup,
            )],
        }
    }

    fn members() -> FieldKind {
        FieldKind::SelectorRows {
            table: "inventory_dataflow_selector",
            owner_column: "group_id",
            side: "member",
            allowed: &[
                ("ipam.prefix", "prefix_id", InventoryEntity::Prefix),
                ("ipam.ipaddress", "ipaddress_id", InventoryEntity::IpAddress),
            ],
        }
    }

    fn member_spec() -> FieldSpec {
        FieldSpec {
            entity: InventoryEntity::AddressGroup,
            name: "members",
            column: "inventory_dataflow_selector",
            sql_type: PgType::Json,
            nullable: false,
            default: None,
            kind: members(),
            choice_normalized: false,
            absent: Absent::Reject,
        }
    }

    #[test]
    fn ordered_selector_entries_keep_their_provider_identities() {
        let bindings = bind(
            &members(),
            "data-flows.objectalias",
            "members",
            &json!([
                {"object_type": "ipam.prefix", "object_id": 11},
                {"object_type": "ipam.ipaddress", "object_id": 12},
            ]),
        )
        .expect("canonical selectors");
        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings[0].external_id, "11");
        assert_eq!(bindings[0].target, Some(InventoryEntity::Prefix));
        assert_eq!(bindings[0].type_token.as_deref(), Some("ipam.prefix"));
        assert_eq!(bindings[1].target, Some(InventoryEntity::IpAddress));

        let flows = bind(
            &flow(),
            "data-flows.dataflows",
            "sources",
            &json!([{
                "object_type": "inventory.addressgroup",
                "object_id": 8,
                "address_mode": "default",
            }]),
        )
        .expect("canonical flow endpoints");
        assert_eq!(flows[0].target, Some(InventoryEntity::AddressGroup));
    }

    #[test]
    fn unsupported_selector_shapes_fail_closed() {
        for value in [
            json!([]),
            json!([{"object_type": "dcim.device", "object_id": 1}]),
            json!([{"object_type": "inventory.addressgroup"}]),
            json!([{"object_type": "inventory.addressgroup", "object_id": 1, "widgets": 2}]),
            json!([{
                "object_type": "inventory.addressgroup",
                "object_id": 1,
                "address_mode": "all"
            }]),
            json!([
                {"object_type": "inventory.addressgroup", "object_id": 1},
                {"object_type": "inventory.addressgroup", "object_id": 1}
            ]),
        ] {
            assert!(
                bind(&flow(), "data-flows.dataflows", "sources", &value).is_err(),
                "{value} must fail closed"
            );
        }
        let too_many: Vec<Value> = (1..=33)
            .map(|id| json!({"object_type": "inventory.addressgroup", "object_id": id}))
            .collect();
        assert!(
            bind(
                &flow(),
                "data-flows.dataflows",
                "sources",
                &Value::Array(too_many)
            )
            .is_err(),
            "a flow side admits at most 32 selectors"
        );
    }

    #[test]
    fn planned_rows_stay_in_source_order_and_resolve_through_bindings() {
        let bindings = vec![
            ReferenceBinding {
                target: Some(InventoryEntity::Prefix),
                type_token: Some("ipam.prefix".into()),
                external_id: "11".into(),
            },
            ReferenceBinding {
                target: Some(InventoryEntity::IpAddress),
                type_token: Some("ipam.ipaddress".into()),
                external_id: "12".into(),
            },
        ];
        let mut local_ids = HashMap::new();
        local_ids.insert(
            ("ipam.prefixes".to_string(), "11".to_string()),
            Uuid::from_u128(11),
        );
        local_ids.insert(
            ("ipam.ip-addresses".to_string(), "12".to_string()),
            Uuid::from_u128(12),
        );
        let planned = plan(&member_spec(), &bindings, &local_ids).expect("resolved members");
        assert_eq!(planned.side, "member");
        assert_eq!(planned.owner_column, "group_id");
        let rows: Vec<_> = planned
            .rows
            .iter()
            .map(|row| (row.object_type.as_str(), row.column, row.local_id))
            .collect();
        assert_eq!(
            rows,
            [
                ("ipam.prefix", "prefix_id", Uuid::from_u128(11)),
                ("ipam.ipaddress", "ipaddress_id", Uuid::from_u128(12)),
            ]
        );
        assert_eq!(
            planned.digest(),
            json!([
                "inventory_dataflow_selector",
                "member",
                [
                    ["ipam.prefix", Uuid::from_u128(11)],
                    ["ipam.ipaddress", Uuid::from_u128(12)]
                ]
            ])
        );
    }

    #[test]
    fn an_unresolved_selector_identity_is_not_planned() {
        let bindings = vec![ReferenceBinding {
            target: Some(InventoryEntity::Prefix),
            type_token: Some("ipam.prefix".into()),
            external_id: "11".into(),
        }];
        assert!(plan(&member_spec(), &bindings, &HashMap::new()).is_err());
        let mut untyped = bindings.clone();
        untyped[0].type_token = None;
        assert!(
            plan(&member_spec(), &untyped, &HashMap::new()).is_err(),
            "a selector without an object type is not plannable"
        );
    }
}
