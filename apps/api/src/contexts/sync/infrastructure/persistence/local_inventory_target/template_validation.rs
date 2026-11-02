//! Validate merged component templates inside the import transaction.

use std::collections::HashSet;

use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::{ColumnValue, FieldKind, InventoryCommitAuthorisation, InventoryEntity as E, InventoryValue, PlannedRow};
use crate::contexts::access_control::domain::Action;
use crate::contexts::inventory::domain::content_type::INVENTORY_ITEM_TEMPLATE_COMPONENT_OBJECT_TYPES;
use crate::contexts::inventory::infrastructure::persistence::repositories::dcim::component_template_repository::{ComponentTemplateAuthorizer, ComponentTemplateKind as K};
use crate::contexts::inventory::infrastructure::persistence::repositories::dcim::component_template_repository::ensure_inventory_sibling_name_unique;
use crate::contexts::inventory::infrastructure::persistence::repositories::dcim::inventory_item_repository::{RuntimeInterfaceComponent, ensure_interface_component_device, lookup_runtime_interface_component};
use crate::errors::AppError;

struct ReferenceAuthorizer<'a>(&'a dyn InventoryCommitAuthorisation);

impl ComponentTemplateAuthorizer for ReferenceAuthorizer<'_> {
    fn authorize<'a>(
        &'a mut self,
        _connection: &'a mut sqlx::PgConnection,
        action: Action,
        reference: Uuid,
        attributes: Value,
    ) -> BoxFuture<'a, Result<(), AppError>> {
        if action == Action::Read {
            self.0.ensure_reference_access(reference, attributes)
        } else {
            Box::pin(async {
                Err(AppError::Forbidden(
                    "inventory_preflight:reference_action".into(),
                ))
            })
        }
    }
}

pub(super) fn kind(entity: E) -> Option<K> {
    Some(match entity {
        E::ConsolePortTemplate => K::ConsolePort,
        E::ConsoleServerPortTemplate => K::ConsoleServerPort,
        E::DeviceBayTemplate => K::DeviceBay,
        E::FrontPortTemplate => K::FrontPort,
        E::InterfaceTemplate => K::Interface,
        E::InventoryItemTemplate => K::InventoryItem,
        E::PowerOutletTemplate => K::PowerOutlet,
        E::PowerPortTemplate => K::PowerPort,
        E::RearPortTemplate => K::RearPort,
        _ => return None,
    })
}

fn native_column(value: &ColumnValue) -> Result<Value, AppError> {
    Ok(match value {
        ColumnValue::Uuid(value) => json!(value),
        ColumnValue::Copy(value) => json!(value),
        ColumnValue::Tags(value) => json!(value),
        ColumnValue::Scalar(_, InventoryValue::Null) => Value::Null,
        ColumnValue::Scalar(_, InventoryValue::Boolean { value }) => json!(value),
        ColumnValue::Scalar(_, InventoryValue::Integer { value }) => json!(value),
        ColumnValue::Scalar(_, InventoryValue::Number { value }) => {
            Value::Number(serde_json::Number::from_f64(*value).ok_or_else(|| {
                AppError::Validation("inventory_preflight:mutation_attributes".into())
            })?)
        }
        ColumnValue::Scalar(_, InventoryValue::Text { value }) => json!(value),
        ColumnValue::Scalar(_, InventoryValue::Json { value }) => value.clone(),
        ColumnValue::Literal("0") => json!(0),
        ColumnValue::Literal("'{}'::jsonb") => json!({}),
        ColumnValue::Literal(_) => {
            return Err(AppError::Validation(
                "inventory_preflight:mutation_attributes".into(),
            ));
        }
    })
}

pub(super) fn admitted_native_mutation(entity: E) -> bool {
    matches!(
        entity,
        E::Tag | E::Vlan | E::Contact | E::ContactGroup | E::ContactRole | E::ContactAssignment
    )
}

async fn project_junctions(
    tx: &mut Transaction<'_, Postgres>,
    entity: E,
    id: Uuid,
    proposed_row: Option<&PlannedRow>,
    record: &mut Value,
) -> Result<(), AppError> {
    for field in entity.field_iter() {
        let (table, local_column, remote_column, content_type_scope) = match field.kind() {
            FieldKind::Junction {
                table,
                local_column,
                remote_column,
                ..
            } => (table, local_column, remote_column, false),
            FieldKind::ContentTypeJunction {
                table,
                local_column,
                remote_column,
            } => (table, local_column, remote_column, true),
            _ => continue,
        };
        let ordering = if content_type_scope {
            remote_column
        } else {
            "id"
        };
        let current: Vec<Uuid> = sqlx::query_scalar(&format!(
            "SELECT {remote_column} FROM {table} WHERE {local_column}=$1 ORDER BY {ordering}"
        ))
        .bind(id)
        .fetch_all(&mut **tx)
        .await?;
        let incoming = proposed_row.and_then(|row| {
            row.junctions.iter().find(|j| {
                j.table == table
                    && j.local_column == local_column
                    && j.remote_column == remote_column
            })
        });
        let ids = if proposed_row.is_some() {
            if let Some(junction) = incoming {
                let mut old_set = current.clone();
                let mut new_set = junction.refs.clone();
                old_set.sort_unstable();
                new_set.sort_unstable();
                new_set.dedup();
                if old_set == new_set {
                    current
                } else {
                    junction.refs.clone()
                }
            } else {
                current
            }
        } else {
            current
        };
        let object = record.as_object_mut().ok_or_else(|| {
            AppError::Validation("inventory_preflight:mutation_attributes".into())
        })?;
        if content_type_scope && entity == E::Tag {
            let tokens: Vec<String> = sqlx::query_scalar(
                "SELECT app_label || '.' || model FROM inventory_content_type WHERE id=ANY($1) ORDER BY app_label,model",
            )
            .bind(&ids)
            .fetch_all(&mut **tx)
            .await?;
            if tokens.len() != ids.iter().collect::<HashSet<_>>().len() {
                return Err(AppError::Validation(
                    "inventory_preflight:mutation_attributes".into(),
                ));
            }
            object.insert(field.name().into(), json!(tokens));
        } else {
            object.insert(field.name().into(), json!(ids));
        }
        if entity == E::Contact && field.name() == "groups" {
            object.insert("group_id".into(), json!(ids.first()));
        }
    }
    Ok(())
}

async fn create_projection(
    tx: &mut Transaction<'_, Postgres>,
    row: &PlannedRow,
) -> Result<Value, AppError> {
    let mut record = json!({"id": row.local_id, "external_id": null, "piping_synced_at": null});
    let object = record.as_object_mut().expect("object literal");
    for field in row.entity.field_iter() {
        match field.kind() {
            FieldKind::Scalar | FieldKind::ForeignKey { .. } => {
                if let Some(default) = field.spec().default {
                    // The expression comes only from the closed catalogue, not
                    // source data. Authorize the native INSERT default as well.
                    let value: Option<Value> =
                        sqlx::query_scalar(&format!("SELECT to_jsonb({default})"))
                            .fetch_one(&mut **tx)
                            .await?;
                    object.insert(field.column().into(), value.unwrap_or(Value::Null));
                } else if field.nullable() {
                    object.insert(field.column().into(), Value::Null);
                }
            }
            FieldKind::GenericReference { type_column, .. } => {
                if field.nullable() {
                    object.insert(field.column().into(), Value::Null);
                    object.insert(type_column.into(), Value::Null);
                }
            }
            FieldKind::Tags => {
                object.insert(field.column().into(), json!([]));
            }
            _ => {}
        }
    }
    Ok(record)
}

pub(super) struct AdmittedMutationReceipt(Vec<(E, Uuid, Action)>);

fn reference_uuid(value: Option<&Value>) -> Result<Option<Uuid>, AppError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Uuid::parse_str(value)
            .map(Some)
            .map_err(|_| AppError::Validation("inventory_preflight:reference_attributes".into())),
        _ => Err(AppError::Validation(
            "inventory_preflight:reference_attributes".into(),
        )),
    }
}

async fn check_admitted_reference(
    tx: &mut Transaction<'_, Postgres>,
    planned: &[PlannedRow],
    target: E,
    id: Uuid,
    original: bool,
    authorisation: &dyn InventoryCommitAuthorisation,
) -> Result<(), AppError> {
    let table = target
        .table()
        .ok_or_else(|| AppError::Validation("inventory_preflight:reference_type".into()))?;
    let (app, model) = target
        .content_type()
        .ok_or_else(|| AppError::Validation("inventory_preflight:reference_type".into()))?;
    let record: Option<Value> =
        sqlx::query_scalar(&format!("SELECT to_jsonb(t) FROM {table} t WHERE id=$1"))
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?;
    match record {
        Some(mut record) => {
            project_junctions(tx, target, id, None, &mut record).await?;
            record["object_type"] = json!(format!("{app}.{model}"));
            authorisation.ensure_reference_access(id, record).await
        }
        None if original
            && planned
                .iter()
                .any(|row| row.entity == target && row.local_id == id) =>
        {
            Ok(())
        }
        None => Err(AppError::Validation(
            "inventory_preflight:reference_missing".into(),
        )),
    }
}

async fn authorize_admitted_references(
    tx: &mut Transaction<'_, Postgres>,
    planned: &[PlannedRow],
    authorisation: &dyn InventoryCommitAuthorisation,
    original: bool,
) -> Result<(), AppError> {
    for row in planned.iter().filter(|row| {
        admitted_native_mutation(row.entity)
            || row.entity == E::Prefix
            || row
                .entity
                .field_iter()
                .any(|field| matches!(field.kind(), FieldKind::Tags))
    }) {
        let table = row
            .entity
            .table()
            .ok_or_else(|| AppError::Internal("inventory_preflight:reference_table".into()))?;
        let stored: Option<Value> =
            sqlx::query_scalar(&format!("SELECT to_jsonb(t) FROM {table} t WHERE id=$1"))
                .bind(row.local_id)
                .fetch_optional(&mut **tx)
                .await?;
        let existed = stored.is_some();
        if !original && !existed {
            return Err(AppError::Validation(
                "inventory_preflight:reference_owner_missing".into(),
            ));
        }
        let mut merged = stored.unwrap_or_else(|| json!({}));
        let all_references = admitted_native_mutation(row.entity) || row.entity == E::Prefix;
        if original {
            let object = merged.as_object_mut().ok_or_else(|| {
                AppError::Validation("inventory_preflight:reference_attributes".into())
            })?;
            for column in &row.columns {
                if !all_references && column.column != "tags" {
                    continue;
                }
                if !existed || column.updatable {
                    object.insert(column.column.to_string(), native_column(&column.value)?);
                }
            }
        }
        for field in row.entity.field_iter() {
            if !all_references && !matches!(field.kind(), FieldKind::Tags) {
                continue;
            }
            // Prefix's existing reference validation is outside this extension.
            if row.entity == E::Prefix && field.name() != "vlan" {
                continue;
            }
            match field.kind() {
                FieldKind::ForeignKey {
                    target: Some(target),
                } => {
                    if let Some(id) = reference_uuid(merged.get(field.column()))? {
                        check_admitted_reference(tx, planned, target, id, original, authorisation)
                            .await?;
                    }
                }
                FieldKind::ForeignKey { target: None } => {
                    if reference_uuid(merged.get(field.column()))?.is_some() {
                        return Err(AppError::Validation(
                            "inventory_preflight:reference_type".into(),
                        ));
                    }
                }
                FieldKind::Tags => match merged.get(field.column()) {
                    None | Some(Value::Null) if original && !existed => {}
                    Some(Value::Array(tags)) => {
                        for tag in tags {
                            let id = reference_uuid(Some(tag))?.ok_or_else(|| {
                                AppError::Validation(
                                    "inventory_preflight:reference_attributes".into(),
                                )
                            })?;
                            check_admitted_reference(
                                tx,
                                planned,
                                E::Tag,
                                id,
                                original,
                                authorisation,
                            )
                            .await?;
                        }
                    }
                    _ => {
                        return Err(AppError::Validation(
                            "inventory_preflight:reference_attributes".into(),
                        ));
                    }
                },
                FieldKind::GenericReference {
                    type_column,
                    allowed,
                    ..
                } => {
                    let id = reference_uuid(merged.get(field.column()))?;
                    let type_id = reference_uuid(merged.get(type_column))?;
                    match (id, type_id) {
                        (None, None) => {}
                        (Some(id), Some(type_id)) => {
                            let token: Option<(String, String)> = sqlx::query_as(
                                "SELECT app_label, model FROM inventory_content_type WHERE id=$1",
                            )
                            .bind(type_id)
                            .fetch_optional(&mut **tx)
                            .await?;
                            let target = allowed
                                .iter()
                                .copied()
                                .find(|target| {
                                    target.content_type().is_some_and(|(app, model)| {
                                        token.as_ref().is_some_and(|(a, m)| a == app && m == model)
                                    })
                                })
                                .ok_or_else(|| {
                                    AppError::Validation(
                                        "inventory_preflight:reference_type".into(),
                                    )
                                })?;
                            check_admitted_reference(
                                tx,
                                planned,
                                target,
                                id,
                                original,
                                authorisation,
                            )
                            .await?;
                        }
                        _ => {
                            return Err(AppError::Validation(
                                "inventory_preflight:reference_type".into(),
                            ));
                        }
                    }
                }
                FieldKind::Junction {
                    table,
                    local_column,
                    remote_column,
                    target: Some(target),
                } => {
                    let incoming = row.junctions.iter().find(|junction| {
                        junction.table == table
                            && junction.local_column == local_column
                            && junction.remote_column == remote_column
                    });
                    let ids: Vec<Uuid> = if original && incoming.is_some() {
                        incoming
                            .map(|junction| junction.refs.clone())
                            .unwrap_or_default()
                    } else {
                        sqlx::query_scalar(&format!(
                            "SELECT {remote_column} FROM {table} WHERE {local_column}=$1"
                        ))
                        .bind(row.local_id)
                        .fetch_all(&mut **tx)
                        .await?
                    };
                    for id in ids {
                        check_admitted_reference(tx, planned, target, id, original, authorisation)
                            .await?;
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

pub(super) async fn authorize_original_admitted_references(
    tx: &mut Transaction<'_, Postgres>,
    planned: &[PlannedRow],
    authorisation: &dyn InventoryCommitAuthorisation,
) -> Result<(), AppError> {
    authorize_admitted_references(tx, planned, authorisation, true).await
}

pub(super) async fn authorize_final_admitted_references(
    tx: &mut Transaction<'_, Postgres>,
    planned: &[PlannedRow],
    authorisation: &dyn InventoryCommitAuthorisation,
) -> Result<(), AppError> {
    authorize_admitted_references(tx, planned, authorisation, false).await
}

/// Check both stored and proposed native records before writing admitted models.
pub(super) async fn authorize_admitted_mutations(
    tx: &mut Transaction<'_, Postgres>,
    planned: &[PlannedRow],
    changed: &HashSet<(E, Uuid)>,
    authorisation: &dyn InventoryCommitAuthorisation,
) -> Result<AdmittedMutationReceipt, AppError> {
    let mut receipt = Vec::new();
    for row in planned.iter().filter(|row| {
        admitted_native_mutation(row.entity) && changed.contains(&(row.entity, row.local_id))
    }) {
        let table = row
            .entity
            .table()
            .ok_or_else(|| AppError::Internal("inventory_preflight:mutation_table".into()))?;
        let (app, model) = row
            .entity
            .content_type()
            .ok_or_else(|| AppError::Internal("inventory_preflight:mutation_type".into()))?;
        let object_type = format!("{app}.{model}");
        let old: Option<Value> =
            sqlx::query_scalar(&format!("SELECT to_jsonb(t) FROM {table} t WHERE id=$1"))
                .bind(row.local_id)
                .fetch_optional(&mut **tx)
                .await?;
        let action = if old.is_some() {
            Action::Update
        } else {
            Action::Create
        };
        if let Some(mut record) = old.clone() {
            project_junctions(tx, row.entity, row.local_id, None, &mut record).await?;
            record["object_type"] = json!(object_type);
            authorisation
                .ensure_mutation_access(action.clone(), row.local_id, record)
                .await?;
        }
        let mut proposed = match old {
            Some(record) => record,
            None => create_projection(tx, row).await?,
        };
        let object = proposed.as_object_mut().ok_or_else(|| {
            AppError::Validation("inventory_preflight:mutation_attributes".into())
        })?;
        for column in &row.columns {
            if action == Action::Update && !column.updatable {
                continue;
            }
            object.insert(column.column.to_string(), native_column(&column.value)?);
        }
        object.insert("object_type".into(), json!(object_type));
        project_junctions(tx, row.entity, row.local_id, Some(row), &mut proposed).await?;
        authorisation
            .ensure_mutation_access(action.clone(), row.local_id, proposed)
            .await?;
        receipt.push((row.entity, row.local_id, action));
    }
    Ok(AdmittedMutationReceipt(receipt))
}

/// Recheck the stored result with the action determined before the write.
pub(super) async fn authorize_final_admitted_mutations(
    tx: &mut Transaction<'_, Postgres>,
    planned: &[PlannedRow],
    receipt: AdmittedMutationReceipt,
    authorisation: &dyn InventoryCommitAuthorisation,
) -> Result<(), AppError> {
    for (entity, id, action) in receipt.0 {
        planned
            .iter()
            .find(|row| row.entity == entity && row.local_id == id)
            .ok_or_else(|| AppError::Internal("inventory_preflight:mutation_receipt".into()))?;
        let table = entity
            .table()
            .ok_or_else(|| AppError::Internal("inventory_preflight:mutation_table".into()))?;
        let (app, model) = entity
            .content_type()
            .ok_or_else(|| AppError::Internal("inventory_preflight:mutation_type".into()))?;
        let mut record: Value =
            sqlx::query_scalar(&format!("SELECT to_jsonb(t) FROM {table} t WHERE id=$1"))
                .bind(id)
                .fetch_optional(&mut **tx)
                .await?
                .ok_or_else(|| {
                    AppError::Validation("inventory_preflight:mutation_missing".into())
                })?;
        project_junctions(tx, entity, id, None, &mut record).await?;
        record["object_type"] = json!(format!("{app}.{model}"));
        authorisation
            .ensure_mutation_access(action, id, record)
            .await?;
    }
    Ok(())
}

async fn authorize_persisted(
    authorisation: &dyn InventoryCommitAuthorisation,
    action: Action,
    id: Uuid,
    template: K,
    mut record: Value,
) -> Result<(), AppError> {
    if !record.is_object() {
        return Err(AppError::Validation(
            "inventory_preflight:mutation_attributes".into(),
        ));
    }
    record["object_type"] = json!(template.content_type());
    authorisation
        .ensure_mutation_access(action, id, record)
        .await
}

/// Authorize actual component-template writes before any inventory mutation.
/// The plan carries resolved final references, not the temporary first-pass row.
pub(super) async fn authorize_mutations(
    tx: &mut Transaction<'_, Postgres>,
    planned: &[PlannedRow],
    changed: &HashSet<(E, Uuid)>,
    authorisation: &dyn InventoryCommitAuthorisation,
) -> Result<(), AppError> {
    authorize_original_inventory_components(tx, planned, authorisation).await?;
    for row in planned
        .iter()
        .filter(|row| changed.contains(&(row.entity, row.local_id)))
    {
        let Some(template) = kind(row.entity) else {
            continue;
        };
        let table = row
            .entity
            .table()
            .ok_or_else(|| AppError::Internal("template table missing".into()))?;
        let old: Option<Value> =
            sqlx::query_scalar(&format!("SELECT to_jsonb(t) FROM {table} t WHERE id=$1"))
                .bind(row.local_id)
                .fetch_optional(&mut **tx)
                .await?;
        let action = if old.is_some() {
            Action::Update
        } else {
            Action::Create
        };
        if let Some(persisted) = old.clone() {
            authorize_persisted(
                authorisation,
                action.clone(),
                row.local_id,
                template,
                persisted,
            )
            .await?;
        }
        let mut proposed = old.unwrap_or_else(|| template.create_defaults(row.local_id));
        let object = proposed.as_object_mut().ok_or_else(|| {
            AppError::Validation("inventory_preflight:mutation_attributes".into())
        })?;
        for column in &row.columns {
            if action == Action::Update && !column.updatable {
                continue;
            }
            object.insert(column.column.to_string(), native_column(&column.value)?);
        }
        object.insert("object_type".into(), json!(template.content_type()));
        authorisation
            .ensure_mutation_access(action, row.local_id, proposed)
            .await?;
    }
    let mut owners = HashSet::new();
    let mut previous_owners = HashSet::new();
    for row in planned.iter().filter(|row| {
        row.entity == E::PortTemplateMapping && changed.contains(&(row.entity, row.local_id))
    }) {
        let previous: Option<Uuid> =
            sqlx::query_scalar("SELECT front_port_id FROM dcim_porttemplatemapping WHERE id=$1")
                .bind(row.local_id)
                .fetch_optional(&mut **tx)
                .await?;
        if let Some(id) = previous {
            owners.insert(id);
            previous_owners.insert(id);
        }
        let next = row
            .columns
            .iter()
            .find(|column| column.column == "front_port_id")
            .and_then(|column| match &column.value {
                ColumnValue::Uuid(Some(id)) => Some(*id),
                _ => None,
            })
            .ok_or_else(|| {
                AppError::Validation("inventory_preflight:template_mapping_front".into())
            })?;
        owners.insert(next);
    }
    let mut owners: Vec<_> = owners.into_iter().collect();
    owners.sort_unstable();
    for id in owners {
        let persisted: Option<Value> =
            sqlx::query_scalar("SELECT to_jsonb(t) FROM dcim_frontporttemplate t WHERE id=$1")
                .bind(id)
                .fetch_optional(&mut **tx)
                .await?;
        if let Some(record) = persisted {
            authorize_persisted(authorisation, Action::Update, id, K::FrontPort, record).await?;
        } else if previous_owners.contains(&id)
            || !planned.iter().any(|row| {
                row.entity == E::FrontPortTemplate
                    && row.local_id == id
                    && changed.contains(&(row.entity, id))
            })
        {
            return Err(AppError::Validation(
                "inventory_preflight:template_mapping_front".into(),
            ));
        }
    }
    Ok(())
}

// Evaluate the final binding against original target attributes before any row
// changes. Explicitly removed bindings require no READ of the former target.
async fn authorize_original_inventory_components(
    tx: &mut Transaction<'_, Postgres>,
    planned: &[PlannedRow],
    authorisation: &dyn InventoryCommitAuthorisation,
) -> Result<(), AppError> {
    let mut items: HashSet<Uuid> = planned
        .iter()
        .filter(|row| row.entity == E::InventoryItem)
        .map(|row| row.local_id)
        .collect();
    for row in planned.iter().filter(|row| row.entity == E::Interface) {
        let dependents: Vec<Uuid> = sqlx::query_scalar(
            "SELECT item.id FROM dcim_inventoryitem item JOIN inventory_content_type ct ON ct.id=item.component_type_id WHERE item.component_id=$1 AND ct.app_label='dcim' AND ct.model='interface'",
        ).bind(row.local_id).fetch_all(&mut **tx).await?;
        items.extend(dependents);
    }
    let mut items: Vec<_> = items.into_iter().collect();
    items.sort_unstable();
    for id in items {
        let old: Option<(Option<Uuid>, Option<Uuid>)> = sqlx::query_as(
            "SELECT component_id, component_type_id FROM dcim_inventoryitem WHERE id=$1",
        )
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?;
        let existed = old.is_some();
        let (mut component, mut component_type) = old.unwrap_or((None, None));
        if let Some(row) = planned
            .iter()
            .find(|row| row.entity == E::InventoryItem && row.local_id == id)
        {
            for column in &row.columns {
                if existed && !column.updatable {
                    continue;
                }
                let destination = match column.column {
                    "component_id" => &mut component,
                    "component_type_id" => &mut component_type,
                    _ => continue,
                };
                *destination = match &column.value {
                    ColumnValue::Uuid(value) => *value,
                    _ => {
                        return Err(AppError::Validation(
                            "inventory_preflight:inventory_component".into(),
                        ));
                    }
                };
            }
        }
        let (component, component_type) = match (component, component_type) {
            (None, None) => continue,
            (Some(component), Some(component_type)) => (component, component_type),
            _ => {
                return Err(AppError::Validation(
                    "inventory_preflight:inventory_component".into(),
                ));
            }
        };
        match lookup_runtime_interface_component(&mut **tx, component_type, component).await? {
            RuntimeInterfaceComponent::Found { mut attributes, .. } => {
                attributes["object_type"] = json!("dcim.interface");
                authorisation
                    .ensure_reference_access(component, attributes)
                    .await?;
            }
            RuntimeInterfaceComponent::Missing
                if planned
                    .iter()
                    .any(|row| row.entity == E::Interface && row.local_id == component) => {}
            RuntimeInterfaceComponent::Missing | RuntimeInterfaceComponent::OtherType => {
                return Err(AppError::Validation(
                    "inventory_preflight:inventory_component".into(),
                ));
            }
        }
    }
    Ok(())
}

/// Called after the two-pass write, before commit, while the hardware graph is locked.
pub(super) async fn validate(
    tx: &mut Transaction<'_, Postgres>,
    planned: &[PlannedRow],
    authorisation: &dyn InventoryCommitAuthorisation,
) -> Result<(), AppError> {
    validate_inventory_components(tx, planned, authorisation).await?;
    let mut affected: HashSet<(E, Uuid)> = planned
        .iter()
        .filter_map(|row| kind(row.entity).map(|_| (row.entity, row.local_id)))
        .collect();
    for row in planned
        .iter()
        .filter(|row| row.entity == E::PortTemplateMapping)
    {
        let front: Uuid =
            sqlx::query_scalar("SELECT front_port_id FROM dcim_porttemplatemapping WHERE id=$1")
                .bind(row.local_id)
                .fetch_one(&mut **tx)
                .await?;
        affected.insert((E::FrontPortTemplate, front));
    }
    for row in planned {
        if let Some((app, model)) = row.entity.content_type() {
            let token = format!("{app}.{model}");
            if INVENTORY_ITEM_TEMPLATE_COMPONENT_OBJECT_TYPES.contains(&token.as_str()) {
                let ids: Vec<Uuid> = sqlx::query_scalar(
                    "SELECT item.id FROM dcim_inventoryitemtemplate item JOIN inventory_content_type ct ON ct.id=item.component_type_id WHERE item.component_id=$1 AND ct.app_label=$2 AND ct.model=$3",
                )
                .bind(row.local_id)
                .bind(app)
                .bind(model)
                .fetch_all(&mut **tx)
                .await?;
                affected.extend(ids.into_iter().map(|id| (E::InventoryItemTemplate, id)));
            }
        }
        let dependency = match row.entity {
            E::DeviceType => Some((
                E::DeviceBayTemplate,
                "SELECT id FROM dcim_devicebaytemplate WHERE device_type_id=$1",
            )),
            E::RearPortTemplate => Some((
                E::FrontPortTemplate,
                "SELECT DISTINCT front_port_id FROM dcim_porttemplatemapping WHERE rear_port_id=$1",
            )),
            E::PowerPortTemplate => Some((
                E::PowerOutletTemplate,
                "SELECT id FROM dcim_poweroutlettemplate WHERE power_port_id=$1",
            )),
            E::InterfaceTemplate => Some((
                E::InterfaceTemplate,
                "SELECT id FROM dcim_interfacetemplate WHERE bridge_id=$1",
            )),
            E::InventoryItemTemplate => Some((
                E::InventoryItemTemplate,
                "SELECT id FROM dcim_inventoryitemtemplate WHERE parent_id=$1",
            )),
            _ => None,
        };
        if let Some((entity, query)) = dependency {
            let ids: Vec<Uuid> = sqlx::query_scalar(query)
                .bind(row.local_id)
                .fetch_all(&mut **tx)
                .await?;
            affected.extend(ids.into_iter().map(|id| (entity, id)));
        }
    }
    let mut affected: Vec<_> = affected.into_iter().collect();
    affected.sort_unstable();
    for (entity, id) in affected {
        let table = entity
            .table()
            .ok_or_else(|| AppError::Internal("template table missing".into()))?;
        let mut record: Value =
            sqlx::query_scalar(&format!("SELECT to_jsonb(t) FROM {table} t WHERE id=$1"))
                .bind(id)
                .fetch_one(&mut **tx)
                .await?;
        let (app, model) = entity
            .content_type()
            .ok_or_else(|| AppError::Internal("template content type missing".into()))?;
        record["object_type"] = json!(format!("{app}.{model}"));
        authorisation
            .ensure_reference_access(id, record.clone())
            .await?;
        let mut changes = json!({});
        if entity == E::FrontPortTemplate {
            let mismatch: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM dcim_porttemplatemapping m JOIN dcim_frontporttemplate f ON f.id=m.front_port_id WHERE m.front_port_id=$1 AND (m.device_type_id IS DISTINCT FROM f.device_type_id OR m.module_type_id IS DISTINCT FROM f.module_type_id))")
                .bind(id).fetch_one(&mut **tx).await?;
            if mismatch {
                return Err(AppError::Validation(
                    "inventory_preflight:template_mapping_parent".into(),
                ));
            }
            let mappings: Value = sqlx::query_scalar("SELECT COALESCE(jsonb_agg(jsonb_build_object('position', front_port_position, 'rear_port', rear_port_id, 'rear_port_position', rear_port_position) ORDER BY front_port_position), '[]'::jsonb) FROM dcim_porttemplatemapping WHERE front_port_id=$1")
                .bind(id).fetch_one(&mut **tx).await?;
            changes["rear_ports"] = mappings;
        }
        let template =
            kind(entity).ok_or_else(|| AppError::Internal("template kind missing".into()))?;
        template.validate(&record)?;
        let mut authorize = ReferenceAuthorizer(authorisation);
        template
            .validate_references(tx, &record, id, &changes, false, &mut authorize)
            .await?;
        ensure_inventory_sibling_name_unique(tx, template, &record, id, false, &mut authorize)
            .await?;
    }
    Ok(())
}

// Validate final stored bindings, including omitted components and reverse
// dependencies of moved interfaces, under the transfer's hardware graph lock.
async fn validate_inventory_components(
    tx: &mut Transaction<'_, Postgres>,
    planned: &[PlannedRow],
    authorisation: &dyn InventoryCommitAuthorisation,
) -> Result<(), AppError> {
    let mut items: HashSet<Uuid> = planned
        .iter()
        .filter(|row| row.entity == E::InventoryItem)
        .map(|row| row.local_id)
        .collect();
    for row in planned.iter().filter(|row| row.entity == E::Interface) {
        let dependents: Vec<Uuid> = sqlx::query_scalar(
            "SELECT item.id FROM dcim_inventoryitem item JOIN inventory_content_type ct ON ct.id=item.component_type_id WHERE item.component_id=$1 AND ct.app_label='dcim' AND ct.model='interface'",
        ).bind(row.local_id).fetch_all(&mut **tx).await?;
        items.extend(dependents);
    }
    let mut items: Vec<_> = items.into_iter().collect();
    items.sort_unstable();
    for id in items {
        let (device, component, component_type): (Uuid, Option<Uuid>, Option<Uuid>) =
            sqlx::query_as("SELECT device_id, component_id, component_type_id FROM dcim_inventoryitem WHERE id=$1")
                .bind(id).fetch_one(&mut **tx).await?;
        let (component, component_type) = match (component, component_type) {
            (None, None) => continue,
            (Some(component), Some(component_type)) => (component, component_type),
            _ => {
                return Err(AppError::Validation(
                    "inventory_preflight:inventory_component".into(),
                ));
            }
        };
        let (target_device, mut attributes) =
            match lookup_runtime_interface_component(&mut **tx, component_type, component).await? {
                RuntimeInterfaceComponent::Found { device, attributes } => (device, attributes),
                RuntimeInterfaceComponent::Missing | RuntimeInterfaceComponent::OtherType => {
                    return Err(AppError::Validation(
                        "inventory_preflight:inventory_component".into(),
                    ));
                }
            };
        attributes["object_type"] = json!("dcim.interface");
        authorisation
            .ensure_reference_access(component, attributes)
            .await?;
        ensure_interface_component_device(device, target_device)?;
    }
    Ok(())
}
