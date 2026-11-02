//! Typed persistence for the nine native component templates.
//!
//! All templates share one transaction, lock and authorization boundary:
//! `lock_hardware_graph`, then the row lock, then policy evaluation with the
//! attributes read from the persisted row and its persisted references
//! (parent, power port, bridge, inventory parent, component, mapped rear
//! ports). The public methods are typed per model; the shared engine owns merge,
//! validation, SQL, front/rear mapping replacement and inventory nested-set
//! bookkeeping.
//!
//! Table, projection and content-type identifiers below are `&'static str`
//! constants. They are the only dynamic SQL identifiers and never come from
//! request input.

use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use sqlx::{PgConnection, PgPool, Postgres, QueryBuilder, Row, postgres::PgRow};
use uuid::Uuid;

use crate::contexts::access_control::domain::Action;
use crate::contexts::inventory::domain::content_type::{
    INVENTORY_ITEM_TEMPLATE_COMPONENT_OBJECT_TYPES, by_token,
};
use crate::contexts::inventory::domain::dcim::component_template::{
    ConsolePortTemplate, ConsolePortTemplateChanges, ConsoleServerPortTemplate,
    ConsoleServerPortTemplateChanges, DeviceBayTemplate, DeviceBayTemplateChanges,
    FrontPortMappingChanges, FrontPortTemplate, FrontPortTemplateChanges, InterfaceTemplate,
    InterfaceTemplateChanges, InventoryItemTemplate, InventoryItemTemplateChanges,
    PowerOutletTemplate, PowerOutletTemplateChanges, PowerPortTemplate, PowerPortTemplateChanges,
    RearPortTemplate, RearPortTemplateChanges, TEMPLATE_DESCRIPTION_MAX, TEMPLATE_NAME_MAX,
    choices, validate_choice, validate_color, validate_positions, validate_required_choice,
    validate_text,
};
use crate::contexts::inventory::infrastructure::persistence::repositories::dcim::hardware_graph::lock_hardware_graph;
use crate::contexts::shared::list_query::{
    ListColumn, SortSpec, WhereState, push_order_by, push_search_ilike, push_string_in,
    push_uuid_in,
};
use crate::errors::{AppError, classify_sqlx_error_owned};

/// Authorize a template operation using the connection holding the graph lock.
pub trait ComponentTemplateAuthorizer {
    fn authorize<'a>(
        &'a mut self,
        connection: &'a mut PgConnection,
        action: Action,
        id: Uuid,
        attributes: Value,
    ) -> BoxFuture<'a, Result<(), AppError>>;
}

/// Sortable columns shared by the nine component-template collections. The
/// frontend table contract advertises `name` and `description`; no other key is
/// accepted, so a typo is a validation error rather than an ignored sort.
pub const COMPONENT_TEMPLATE_COLUMNS: &[ListColumn] = &[
    ListColumn::sortable("name", "t.name", "inventory.resources.fields.name"),
    ListColumn::sortable(
        "description",
        "t.description",
        "inventory.resources.fields.description",
    ),
];

/// Filters accepted by the nine component-template collections. Only the
/// parameters advertised by a given endpoint are populated by its route; the
/// repository additionally refuses invalid boolean/choice values instead of
/// turning a bad filter into an empty page.
#[derive(Debug, Clone, Default)]
pub struct ComponentTemplateListFilter {
    pub device_type_id: Vec<Uuid>,
    pub module_type_id: Vec<Uuid>,
    pub parent_id: Vec<Uuid>,
    pub power_port_id: Vec<Uuid>,
    pub manufacturer_id: Vec<Uuid>,
    pub role_id: Vec<Uuid>,
    pub bridge_id: Vec<Uuid>,
    pub feed_leg: Vec<String>,
    pub enabled: Vec<String>,
    pub mgmt_only: Vec<String>,
    pub search: Option<String>,
    pub sort: Option<SortSpec>,
}

/// The nine native component-template models.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentTemplateKind {
    ConsolePort,
    ConsoleServerPort,
    PowerPort,
    PowerOutlet,
    Interface,
    FrontPort,
    RearPort,
    DeviceBay,
    InventoryItem,
}

impl ComponentTemplateKind {
    fn table(self) -> &'static str {
        match self {
            Self::ConsolePort => "dcim_consoleporttemplate",
            Self::ConsoleServerPort => "dcim_consoleserverporttemplate",
            Self::PowerPort => "dcim_powerporttemplate",
            Self::PowerOutlet => "dcim_poweroutlettemplate",
            Self::Interface => "dcim_interfacetemplate",
            Self::FrontPort => "dcim_frontporttemplate",
            Self::RearPort => "dcim_rearporttemplate",
            Self::DeviceBay => "dcim_devicebaytemplate",
            Self::InventoryItem => "dcim_inventoryitemtemplate",
        }
    }

    /// Human-readable name used in not-found and validation messages.
    pub fn label(self) -> &'static str {
        match self {
            Self::ConsolePort => "Console port template",
            Self::ConsoleServerPort => "Console server port template",
            Self::PowerPort => "Power port template",
            Self::PowerOutlet => "Power outlet template",
            Self::Interface => "Interface template",
            Self::FrontPort => "Front port template",
            Self::RearPort => "Rear port template",
            Self::DeviceBay => "Device bay template",
            Self::InventoryItem => "Inventory item template",
        }
    }

    pub fn content_type(self) -> &'static str {
        match self {
            Self::ConsolePort => "dcim.consoleporttemplate",
            Self::ConsoleServerPort => "dcim.consoleserverporttemplate",
            Self::PowerPort => "dcim.powerporttemplate",
            Self::PowerOutlet => "dcim.poweroutlettemplate",
            Self::Interface => "dcim.interfacetemplate",
            Self::FrontPort => "dcim.frontporttemplate",
            Self::RearPort => "dcim.rearporttemplate",
            Self::DeviceBay => "dcim.devicebaytemplate",
            Self::InventoryItem => "dcim.inventoryitemtemplate",
        }
    }

    fn modular(self) -> bool {
        matches!(
            self,
            Self::ConsolePort
                | Self::ConsoleServerPort
                | Self::PowerPort
                | Self::PowerOutlet
                | Self::Interface
                | Self::FrontPort
                | Self::RearPort
        )
    }

    /// Row projection. Front/rear templates include their persisted mapping
    /// rows as typed JSON instead of exposing a second writable relation.
    fn projection(self) -> &'static str {
        match self {
            Self::FrontPort => {
                "to_jsonb(t) || jsonb_build_object('rear_ports', COALESCE((SELECT jsonb_agg(jsonb_build_object('position', m.front_port_position, 'rear_port', m.rear_port_id, 'rear_port_position', m.rear_port_position) ORDER BY m.front_port_position) FROM dcim_porttemplatemapping m WHERE m.front_port_id=t.id), '[]'::jsonb))"
            }
            Self::RearPort => {
                "to_jsonb(t) || jsonb_build_object('front_ports', COALESCE((SELECT jsonb_agg(jsonb_build_object('position', m.rear_port_position, 'front_port', m.front_port_id, 'front_port_position', m.front_port_position) ORDER BY m.front_port_position) FROM dcim_porttemplatemapping m WHERE m.rear_port_id=t.id), '[]'::jsonb))"
            }
            _ => "to_jsonb(t)",
        }
    }

    /// Column defaults for a new row. Required-but-missing values stay null and
    /// are rejected by [`Self::validate`] with a field-specific message.
    pub(crate) fn create_defaults(self, id: Uuid) -> Value {
        match self {
            Self::ConsolePort => json!({
                "id": id, "name": "", "label": "", "description": "",
                "type": Value::Null, "device_type_id": Value::Null,
                "module_type_id": Value::Null,
            }),
            Self::ConsoleServerPort => json!({
                "id": id, "name": "", "label": "", "description": "",
                "type": Value::Null, "device_type_id": Value::Null,
                "module_type_id": Value::Null,
            }),
            Self::PowerPort => json!({
                "id": id, "name": "", "label": "", "description": "",
                "type": Value::Null, "maximum_draw": Value::Null,
                "allocated_draw": Value::Null, "device_type_id": Value::Null,
                "module_type_id": Value::Null,
            }),
            Self::PowerOutlet => json!({
                "id": id, "name": "", "label": "", "description": "",
                "type": Value::Null, "color": "", "power_port_id": Value::Null,
                "feed_leg": Value::Null, "device_type_id": Value::Null,
                "module_type_id": Value::Null,
            }),
            Self::Interface => json!({
                "id": id, "name": "", "label": "", "description": "",
                "type": "", "enabled": true, "mgmt_only": false,
                "bridge_id": Value::Null, "poe_mode": Value::Null,
                "poe_type": Value::Null, "rf_role": Value::Null,
                "device_type_id": Value::Null, "module_type_id": Value::Null,
            }),
            Self::FrontPort => json!({
                "id": id, "name": "", "label": "", "description": "",
                "type": "", "color": "", "positions": 1,
                "device_type_id": Value::Null, "module_type_id": Value::Null,
            }),
            Self::RearPort => json!({
                "id": id, "name": "", "label": "", "description": "",
                "type": "", "color": "", "positions": 1,
                "device_type_id": Value::Null, "module_type_id": Value::Null,
            }),
            Self::DeviceBay => json!({
                "id": id, "name": "", "label": "", "description": "",
                "device_type_id": Value::Null, "enabled": true,
            }),
            Self::InventoryItem => json!({
                "id": id, "name": "", "label": "", "description": "",
                "device_type_id": Value::Null, "parent_id": Value::Null,
                "manufacturer_id": Value::Null, "role_id": Value::Null,
                "part_id": "", "component_type_id": Value::Null,
                "component_id": Value::Null,
            }),
        }
    }

    /// Field-level validation of the merged record. Persisted references and
    /// cross-row rules are checked by [`Self::validate_references`].
    ///
    /// Crate-wide because non-REST writers (the inventory importer) must apply
    /// the same merged-record contract inside their own transaction instead of
    /// copying these rules.
    pub(crate) fn validate(self, record: &Value) -> Result<(), AppError> {
        validate_text(
            "name",
            required_str(record, "name")?,
            TEMPLATE_NAME_MAX,
            true,
        )?;
        validate_text(
            "label",
            required_str(record, "label")?,
            TEMPLATE_NAME_MAX,
            false,
        )?;
        validate_text(
            "description",
            required_str(record, "description")?,
            TEMPLATE_DESCRIPTION_MAX,
            false,
        )?;

        if self.modular() {
            let device = optional_uuid(record, "device_type_id")?;
            let module = optional_uuid(record, "module_type_id")?;
            if device.is_some() == module.is_some() {
                return Err(AppError::Validation(
                    "A component template must be associated with either a device type or a module type"
                        .into(),
                ));
            }
        } else {
            required_uuid(record, "device_type_id")?;
        }

        match self {
            Self::ConsolePort | Self::ConsoleServerPort => validate_choice(
                "type",
                optional_str(record, "type")?,
                choices::CONSOLE_PORT_TYPES,
                true,
            )?,
            Self::PowerPort => {
                validate_choice(
                    "type",
                    optional_str(record, "type")?,
                    choices::POWER_PORT_TYPES,
                    true,
                )?;
                let maximum = optional_i32(record, "maximum_draw")?;
                let allocated = optional_i32(record, "allocated_draw")?;
                for (field, value) in [("maximum_draw", maximum), ("allocated_draw", allocated)] {
                    if value.is_some_and(|value| value < 1) {
                        return Err(AppError::Validation(format!("{field} must be at least 1")));
                    }
                }
                if let (Some(maximum), Some(allocated)) = (maximum, allocated)
                    && allocated > maximum
                {
                    return Err(AppError::Validation(
                        "allocated_draw cannot exceed maximum_draw".into(),
                    ));
                }
            }
            Self::PowerOutlet => {
                validate_choice(
                    "type",
                    optional_str(record, "type")?,
                    choices::POWER_OUTLET_TYPES,
                    true,
                )?;
                validate_color("color", required_str(record, "color")?)?;
                validate_choice(
                    "feed_leg",
                    optional_str(record, "feed_leg")?,
                    choices::POWER_OUTLET_FEED_LEGS,
                    true,
                )?;
            }
            Self::Interface => {
                validate_required_choice(
                    "type",
                    required_str(record, "type")?,
                    choices::INTERFACE_TYPES,
                )?;
                validate_choice(
                    "poe_mode",
                    optional_str(record, "poe_mode")?,
                    choices::INTERFACE_POE_MODES,
                    true,
                )?;
                validate_choice(
                    "poe_type",
                    optional_str(record, "poe_type")?,
                    choices::INTERFACE_POE_TYPES,
                    true,
                )?;
                validate_choice(
                    "rf_role",
                    optional_str(record, "rf_role")?,
                    choices::INTERFACE_RF_ROLES,
                    true,
                )?;
            }
            Self::FrontPort | Self::RearPort => {
                validate_required_choice(
                    "type",
                    required_str(record, "type")?,
                    choices::PORT_TYPES,
                )?;
                validate_color("color", required_str(record, "color")?)?;
                validate_positions("positions", required_i16(record, "positions")?)?;
            }
            Self::DeviceBay => {
                required_bool(record, "enabled")?;
            }
            Self::InventoryItem => {
                validate_text("part_id", required_str(record, "part_id")?, 50, false)?;
                let component_type = optional_uuid(record, "component_type_id")?;
                let component = optional_uuid(record, "component_id")?;
                if component_type.is_some() != component.is_some() {
                    return Err(AppError::Validation(
                        "component_type_id and component_id must be supplied together".into(),
                    ));
                }
            }
        }
        Ok(())
    }

    /// Persisted references: existence, same-parent scope, hierarchy cycles and
    /// READ authorization of the attributes read under this transaction.
    ///
    /// A create is authorized as one object-level CREATE with the persisted
    /// reference attributes; it does not demand READ on every parent id named
    /// in the body, which would make a create-scoped PAT unusable.
    ///
    /// Crate-wide for the inventory importer: it runs this same owner inside its
    /// transfer transaction and supplies its own `authorize` hook, so reference,
    /// positioned-mapping and hierarchy validation has one implementation.
    pub(crate) async fn validate_references<F: ComponentTemplateAuthorizer>(
        self,
        transaction: &mut sqlx::Transaction<'_, Postgres>,
        record: &Value,
        id: Uuid,
        changes: &Value,
        create: bool,
        authorize: &mut F,
    ) -> Result<(), AppError> {
        if let Some(device_type_id) = optional_uuid(record, "device_type_id")? {
            let attributes = load_reference(
                transaction,
                "dcim_devicetype",
                "dcim.devicetype",
                device_type_id,
            )
            .await?;
            // Authorize the persisted reference before reading any of its hidden
            // properties. Otherwise a denied caller could tell a valid parent
            // (validation error) from an invalid one (forbidden).
            if !create {
                authorize
                    .authorize(
                        transaction.as_mut(),
                        Action::Read,
                        device_type_id,
                        attributes.clone(),
                    )
                    .await?;
            }
            if self == Self::DeviceBay && attributes["subdevice_role"].as_str() != Some("parent") {
                return Err(AppError::Validation(
                    "Subdevice role of the device type must be set to \"parent\" to allow device bays"
                        .into(),
                ));
            }
        }
        if let Some(module_type_id) = optional_uuid(record, "module_type_id")? {
            let attributes = load_reference(
                transaction,
                "dcim_moduletype",
                "dcim.moduletype",
                module_type_id,
            )
            .await?;
            if !create {
                authorize
                    .authorize(
                        transaction.as_mut(),
                        Action::Read,
                        module_type_id,
                        attributes,
                    )
                    .await?;
            }
        }

        match self {
            Self::PowerOutlet => {
                if let Some(power_port_id) = optional_uuid(record, "power_port_id")? {
                    let attributes = load_reference(
                        transaction,
                        "dcim_powerporttemplate",
                        "dcim.powerporttemplate",
                        power_port_id,
                    )
                    .await?;
                    if !create {
                        authorize
                            .authorize(
                                transaction.as_mut(),
                                Action::Read,
                                power_port_id,
                                attributes.clone(),
                            )
                            .await?;
                    }
                    ensure_same_parent(record, &attributes)?;
                }
            }
            Self::Interface => {
                if let Some(bridge_id) = optional_uuid(record, "bridge_id")? {
                    if bridge_id == id {
                        return Err(AppError::Validation(
                            "An interface template cannot be bridged to itself".into(),
                        ));
                    }
                    let attributes = load_reference(
                        transaction,
                        "dcim_interfacetemplate",
                        "dcim.interfacetemplate",
                        bridge_id,
                    )
                    .await?;
                    if !create {
                        authorize
                            .authorize(
                                transaction.as_mut(),
                                Action::Read,
                                bridge_id,
                                attributes.clone(),
                            )
                            .await?;
                    }
                    ensure_same_parent(record, &attributes)?;
                }
            }
            Self::FrontPort => {
                if let Some(supplied) = changes.get("rear_ports") {
                    let mappings: Vec<FrontPortMappingChanges> =
                        serde_json::from_value(supplied.clone())?;
                    authorize_front_mappings(transaction, record, &mappings, create, authorize)
                        .await?;
                }
            }
            Self::InventoryItem => {
                authorize_inventory_references(transaction, record, id, create, authorize).await?;
            }
            _ => {}
        }
        Ok(())
    }

    /// Persist the merged record. The update statement never writes the
    /// inventory nested-set columns; those are recomputed from the adjacency
    /// list after every mutation.
    async fn write(
        self,
        transaction: &mut sqlx::Transaction<'_, Postgres>,
        id: Uuid,
        record: &Value,
        changes: &Value,
        create: bool,
    ) -> Result<(), AppError> {
        match self {
            Self::ConsolePort | Self::ConsoleServerPort => {
                let statement = if create {
                    format!(
                        "INSERT INTO {} (id,name,label,description,type,device_type_id,module_type_id) VALUES ($1,$2,$3,$4,$5,$6,$7)",
                        self.table()
                    )
                } else {
                    let table = self.table();
                    format!(
                        "UPDATE {table} SET name=$2,label=$3,description=$4,type=$5,device_type_id=$6,module_type_id=$7 WHERE id=$1"
                    )
                };
                sqlx::query(&statement)
                    .bind(id)
                    .bind(required_str(record, "name")?)
                    .bind(required_str(record, "label")?)
                    .bind(required_str(record, "description")?)
                    .bind(optional_str(record, "type")?)
                    .bind(optional_uuid(record, "device_type_id")?)
                    .bind(optional_uuid(record, "module_type_id")?)
                    .execute(&mut **transaction)
                    .await
                    .map_err(classify_sqlx_error_owned)?;
            }
            Self::PowerPort => {
                let statement = if create {
                    let table = self.table();
                    format!(
                        "INSERT INTO {table} (id,name,label,description,type,maximum_draw,allocated_draw,device_type_id,module_type_id) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)"
                    )
                } else {
                    let table = self.table();
                    format!(
                        "UPDATE {table} SET name=$2,label=$3,description=$4,type=$5,maximum_draw=$6,allocated_draw=$7,device_type_id=$8,module_type_id=$9 WHERE id=$1"
                    )
                };
                sqlx::query(&statement)
                    .bind(id)
                    .bind(required_str(record, "name")?)
                    .bind(required_str(record, "label")?)
                    .bind(required_str(record, "description")?)
                    .bind(optional_str(record, "type")?)
                    .bind(optional_i32(record, "maximum_draw")?)
                    .bind(optional_i32(record, "allocated_draw")?)
                    .bind(optional_uuid(record, "device_type_id")?)
                    .bind(optional_uuid(record, "module_type_id")?)
                    .execute(&mut **transaction)
                    .await
                    .map_err(classify_sqlx_error_owned)?;
            }
            Self::PowerOutlet => {
                let statement = if create {
                    let table = self.table();
                    format!(
                        "INSERT INTO {table} (id,name,label,description,type,color,power_port_id,feed_leg,device_type_id,module_type_id) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)"
                    )
                } else {
                    let table = self.table();
                    format!(
                        "UPDATE {table} SET name=$2,label=$3,description=$4,type=$5,color=$6,power_port_id=$7,feed_leg=$8,device_type_id=$9,module_type_id=$10 WHERE id=$1"
                    )
                };
                sqlx::query(&statement)
                    .bind(id)
                    .bind(required_str(record, "name")?)
                    .bind(required_str(record, "label")?)
                    .bind(required_str(record, "description")?)
                    .bind(optional_str(record, "type")?)
                    .bind(required_str(record, "color")?)
                    .bind(optional_uuid(record, "power_port_id")?)
                    .bind(optional_str(record, "feed_leg")?)
                    .bind(optional_uuid(record, "device_type_id")?)
                    .bind(optional_uuid(record, "module_type_id")?)
                    .execute(&mut **transaction)
                    .await
                    .map_err(classify_sqlx_error_owned)?;
            }
            Self::Interface => {
                let statement = if create {
                    let table = self.table();
                    format!(
                        "INSERT INTO {table} (id,name,label,description,_name,type,enabled,mgmt_only,bridge_id,poe_mode,poe_type,rf_role,device_type_id,module_type_id) VALUES ($1,$2,$3,$4,$2,$5,$6,$7,$8,$9,$10,$11,$12,$13)"
                    )
                } else {
                    let table = self.table();
                    format!(
                        "UPDATE {table} SET name=$2,label=$3,description=$4,_name=$2,type=$5,enabled=$6,mgmt_only=$7,bridge_id=$8,poe_mode=$9,poe_type=$10,rf_role=$11,device_type_id=$12,module_type_id=$13 WHERE id=$1"
                    )
                };
                sqlx::query(&statement)
                    .bind(id)
                    .bind(required_str(record, "name")?)
                    .bind(required_str(record, "label")?)
                    .bind(required_str(record, "description")?)
                    .bind(required_str(record, "type")?)
                    .bind(required_bool(record, "enabled")?)
                    .bind(required_bool(record, "mgmt_only")?)
                    .bind(optional_uuid(record, "bridge_id")?)
                    .bind(optional_str(record, "poe_mode")?)
                    .bind(optional_str(record, "poe_type")?)
                    .bind(optional_str(record, "rf_role")?)
                    .bind(optional_uuid(record, "device_type_id")?)
                    .bind(optional_uuid(record, "module_type_id")?)
                    .execute(&mut **transaction)
                    .await
                    .map_err(classify_sqlx_error_owned)?;
            }
            Self::FrontPort | Self::RearPort => {
                let statement = if create {
                    let table = self.table();
                    format!(
                        "INSERT INTO {table} (id,name,label,description,type,color,positions,device_type_id,module_type_id) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)"
                    )
                } else {
                    let table = self.table();
                    format!(
                        "UPDATE {table} SET name=$2,label=$3,description=$4,type=$5,color=$6,positions=$7,device_type_id=$8,module_type_id=$9 WHERE id=$1"
                    )
                };
                sqlx::query(&statement)
                    .bind(id)
                    .bind(required_str(record, "name")?)
                    .bind(required_str(record, "label")?)
                    .bind(required_str(record, "description")?)
                    .bind(required_str(record, "type")?)
                    .bind(required_str(record, "color")?)
                    .bind(required_i16(record, "positions")?)
                    .bind(optional_uuid(record, "device_type_id")?)
                    .bind(optional_uuid(record, "module_type_id")?)
                    .execute(&mut **transaction)
                    .await
                    .map_err(classify_sqlx_error_owned)?;
            }
            Self::DeviceBay => {
                let statement = if create {
                    let table = self.table();
                    format!(
                        "INSERT INTO {table} (id,name,label,description,device_type_id,enabled) VALUES ($1,$2,$3,$4,$5,$6)"
                    )
                } else {
                    let table = self.table();
                    format!(
                        "UPDATE {table} SET name=$2,label=$3,description=$4,device_type_id=$5,enabled=$6 WHERE id=$1"
                    )
                };
                sqlx::query(&statement)
                    .bind(id)
                    .bind(required_str(record, "name")?)
                    .bind(required_str(record, "label")?)
                    .bind(required_str(record, "description")?)
                    .bind(required_uuid(record, "device_type_id")?)
                    .bind(required_bool(record, "enabled")?)
                    .execute(&mut **transaction)
                    .await
                    .map_err(classify_sqlx_error_owned)?;
            }
            Self::InventoryItem => {
                let statement = if create {
                    let table = self.table();
                    format!(
                        "INSERT INTO {table} (id,name,label,description,part_id,device_type_id,manufacturer_id,parent_id,role_id,component_type_id,component_id,lft,rght,tree_id,level) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,0,0,0,0)"
                    )
                } else {
                    let table = self.table();
                    format!(
                        "UPDATE {table} SET name=$2,label=$3,description=$4,part_id=$5,device_type_id=$6,manufacturer_id=$7,parent_id=$8,role_id=$9,component_type_id=$10,component_id=$11 WHERE id=$1"
                    )
                };
                sqlx::query(&statement)
                    .bind(id)
                    .bind(required_str(record, "name")?)
                    .bind(required_str(record, "label")?)
                    .bind(required_str(record, "description")?)
                    .bind(required_str(record, "part_id")?)
                    .bind(required_uuid(record, "device_type_id")?)
                    .bind(optional_uuid(record, "manufacturer_id")?)
                    .bind(optional_uuid(record, "parent_id")?)
                    .bind(optional_uuid(record, "role_id")?)
                    .bind(optional_uuid(record, "component_type_id")?)
                    .bind(optional_uuid(record, "component_id")?)
                    .execute(&mut **transaction)
                    .await
                    .map_err(classify_sqlx_error_owned)?;
            }
        }

        if self == Self::FrontPort && (create || changes.get("rear_ports").is_some()) {
            let mappings: Vec<FrontPortMappingChanges> = match changes.get("rear_ports") {
                Some(value) => serde_json::from_value(value.clone())?,
                None => Vec::new(),
            };
            replace_front_mappings(transaction, id, record, &mappings).await?;
        }
        Ok(())
    }

    /// Cross-row checks that need the stored mapping set or the inventory tree.
    async fn after_write(
        self,
        transaction: &mut sqlx::Transaction<'_, Postgres>,
        record: &Value,
    ) -> Result<(), AppError> {
        match self {
            Self::FrontPort => {
                let count: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM dcim_porttemplatemapping WHERE front_port_id=$1",
                )
                .bind(required_uuid(record, "id")?)
                .fetch_one(&mut **transaction)
                .await
                .map_err(classify_sqlx_error_owned)?;
                let maximum: Option<i16> = sqlx::query_scalar(
                    "SELECT MAX(front_port_position) FROM dcim_porttemplatemapping WHERE front_port_id=$1",
                )
                .bind(required_uuid(record, "id")?)
                .fetch_one(&mut **transaction)
                .await
                .map_err(classify_sqlx_error_owned)?;
                let positions = required_i16(record, "positions")?;
                if count > i64::from(positions)
                    || maximum.is_some_and(|position| position > positions)
                {
                    return Err(AppError::Validation(
                        "The number of positions cannot be less than the number of mapped rear port templates"
                            .into(),
                    ));
                }
            }
            Self::RearPort => {
                let count: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM dcim_porttemplatemapping WHERE rear_port_id=$1",
                )
                .bind(required_uuid(record, "id")?)
                .fetch_one(&mut **transaction)
                .await
                .map_err(classify_sqlx_error_owned)?;
                let maximum: Option<i16> = sqlx::query_scalar(
                    "SELECT MAX(rear_port_position) FROM dcim_porttemplatemapping WHERE rear_port_id=$1",
                )
                .bind(required_uuid(record, "id")?)
                .fetch_one(&mut **transaction)
                .await
                .map_err(classify_sqlx_error_owned)?;
                let positions = required_i16(record, "positions")?;
                if count > i64::from(positions)
                    || maximum.is_some_and(|position| position > positions)
                {
                    return Err(AppError::Validation(
                        "The number of positions cannot be less than the number of mapped front port templates"
                            .into(),
                    ));
                }
            }
            Self::InventoryItem => {
                recompute_inventory_tree(transaction, required_uuid(record, "device_type_id")?)
                    .await?;
            }
            _ => {}
        }
        Ok(())
    }

    /// Deletion is owned by the shared inventory delete guard: every inbound
    /// foreign key (an outlet's power port, a front/rear mapping, a child
    /// inventory template, or any other referencing inventory row) blocks the
    /// delete and is reported as a named blocker. Silently clearing or
    /// cascading a persisted reference would lose configuration or bypass the
    /// caller's READ ceiling on the referencing row.
    async fn load_json(
        self,
        transaction: &mut sqlx::Transaction<'_, Postgres>,
        id: Uuid,
    ) -> Result<Option<Value>, AppError> {
        let statement = format!(
            "SELECT {} AS record FROM {} t WHERE t.id=$1",
            self.projection(),
            self.table()
        );
        let row = sqlx::query(&statement)
            .bind(id)
            .fetch_optional(&mut **transaction)
            .await
            .map_err(classify_sqlx_error_owned)?;
        row.map(|row| {
            let value: Value = row.try_get("record")?;
            Ok(with_object_type(self, value))
        })
        .transpose()
    }
}

/// Shared repository over the nine component-template tables.
#[derive(Clone)]
pub struct SqlxComponentTemplateRepository {
    pool: PgPool,
}

macro_rules! component_template_store {
    ($get:ident, $save:ident, $model:ty, $changes:ty, $kind:expr) => {
        /// Read one persisted template with its mapping projection.
        pub async fn $get(&self, id: Uuid) -> Result<Option<$model>, AppError> {
            let Some(value) = self.get_value($kind, id).await? else {
                return Ok(None);
            };
            Ok(Some(serde_json::from_value(value)?))
        }

        /// Create or replace one template. Omitted change fields keep the
        /// persisted value; returned data is read back inside the transaction.
        pub async fn $save<F: ComponentTemplateAuthorizer>(
            &self,
            id: Uuid,
            changes: $changes,
            create: bool,
            mut authorize: F,
        ) -> Result<Option<$model>, AppError> {
            let changes = serde_json::to_value(changes)?;
            let Some(value) = self
                .save_value($kind, id, changes, create, &mut authorize)
                .await?
            else {
                return Ok(None);
            };
            Ok(Some(serde_json::from_value(value)?))
        }
    };
}

impl SqlxComponentTemplateRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    component_template_store!(
        get_console_port_template,
        save_console_port_template,
        ConsolePortTemplate,
        ConsolePortTemplateChanges,
        ComponentTemplateKind::ConsolePort
    );
    component_template_store!(
        get_console_server_port_template,
        save_console_server_port_template,
        ConsoleServerPortTemplate,
        ConsoleServerPortTemplateChanges,
        ComponentTemplateKind::ConsoleServerPort
    );
    component_template_store!(
        get_power_port_template,
        save_power_port_template,
        PowerPortTemplate,
        PowerPortTemplateChanges,
        ComponentTemplateKind::PowerPort
    );
    component_template_store!(
        get_power_outlet_template,
        save_power_outlet_template,
        PowerOutletTemplate,
        PowerOutletTemplateChanges,
        ComponentTemplateKind::PowerOutlet
    );
    component_template_store!(
        get_interface_template,
        save_interface_template,
        InterfaceTemplate,
        InterfaceTemplateChanges,
        ComponentTemplateKind::Interface
    );
    component_template_store!(
        get_front_port_template,
        save_front_port_template,
        FrontPortTemplate,
        FrontPortTemplateChanges,
        ComponentTemplateKind::FrontPort
    );
    component_template_store!(
        get_rear_port_template,
        save_rear_port_template,
        RearPortTemplate,
        RearPortTemplateChanges,
        ComponentTemplateKind::RearPort
    );
    component_template_store!(
        get_device_bay_template,
        save_device_bay_template,
        DeviceBayTemplate,
        DeviceBayTemplateChanges,
        ComponentTemplateKind::DeviceBay
    );
    component_template_store!(
        get_inventory_item_template,
        save_inventory_item_template,
        InventoryItemTemplate,
        InventoryItemTemplateChanges,
        ComponentTemplateKind::InventoryItem
    );

    async fn get_value(
        &self,
        kind: ComponentTemplateKind,
        id: Uuid,
    ) -> Result<Option<Value>, AppError> {
        let statement = format!(
            "SELECT {} AS record FROM {} t WHERE t.id=$1",
            kind.projection(),
            kind.table()
        );
        let row = sqlx::query(&statement)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        row.map(|row| {
            let value: Value = row.try_get("record")?;
            Ok(with_object_type(kind, value))
        })
        .transpose()
    }

    /// Collection read shared by every template kind.
    ///
    /// Returns every row matching the persisted filters in deterministic order
    /// and deliberately does **not** paginate: the caller must authorize each
    /// row with its persisted attributes before it can truthfully compute
    /// `total` or apply `limit`/`offset`. A count taken before row
    /// authorization would disclose hidden rows.
    pub async fn list(
        &self,
        kind: ComponentTemplateKind,
        filter: &ComponentTemplateListFilter,
    ) -> Result<Vec<Value>, AppError> {
        if kind == ComponentTemplateKind::PowerOutlet
            && filter
                .feed_leg
                .iter()
                .any(|value| !choices::POWER_OUTLET_FEED_LEGS.contains(&value.as_str()))
        {
            return Err(AppError::Validation("Invalid feed_leg filter".into()));
        }
        if matches!(
            kind,
            ComponentTemplateKind::Interface | ComponentTemplateKind::DeviceBay
        ) && filter
            .enabled
            .iter()
            .chain(filter.mgmt_only.iter())
            .any(|value| value.as_str() != "true" && value.as_str() != "false")
        {
            return Err(AppError::Validation(
                "Invalid boolean component template filter".into(),
            ));
        }

        fn filters(
            query: &mut QueryBuilder<'_, Postgres>,
            kind: ComponentTemplateKind,
            filter: &ComponentTemplateListFilter,
        ) {
            let mut state = WhereState::new();
            push_uuid_in(
                query,
                &mut state,
                "t.device_type_id",
                &filter.device_type_id,
            );
            if kind.modular() {
                push_uuid_in(
                    query,
                    &mut state,
                    "t.module_type_id",
                    &filter.module_type_id,
                );
            }
            if kind == ComponentTemplateKind::InventoryItem {
                push_uuid_in(query, &mut state, "t.parent_id", &filter.parent_id);
                push_uuid_in(
                    query,
                    &mut state,
                    "t.manufacturer_id",
                    &filter.manufacturer_id,
                );
                push_uuid_in(query, &mut state, "t.role_id", &filter.role_id);
            }
            if kind == ComponentTemplateKind::PowerOutlet {
                push_uuid_in(query, &mut state, "t.power_port_id", &filter.power_port_id);
                push_string_in(query, &mut state, "t.feed_leg", &filter.feed_leg);
            }
            if kind == ComponentTemplateKind::Interface {
                push_uuid_in(query, &mut state, "t.bridge_id", &filter.bridge_id);
                push_string_in(query, &mut state, "t.mgmt_only::text", &filter.mgmt_only);
            }
            if matches!(
                kind,
                ComponentTemplateKind::Interface | ComponentTemplateKind::DeviceBay
            ) {
                push_string_in(query, &mut state, "t.enabled::text", &filter.enabled);
            }
            let columns: &[&str] = if kind == ComponentTemplateKind::InventoryItem {
                &["t.name", "t.label", "t.description", "t.part_id"]
            } else {
                &["t.name", "t.label", "t.description"]
            };
            push_search_ilike(query, &mut state, columns, filter.search.as_deref());
        }

        let mut rows = QueryBuilder::new(format!(
            "SELECT {} AS record FROM {} t",
            kind.projection(),
            kind.table()
        ));
        filters(&mut rows, kind, filter);
        push_order_by(
            &mut rows,
            COMPONENT_TEMPLATE_COLUMNS,
            filter.sort.as_ref(),
            "t.name",
        );
        rows.push(",t.id");
        let rows: Vec<PgRow> = rows
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(classify_sqlx_error_owned)?;
        rows.iter()
            .map(|row| {
                let value: Value = row.try_get("record")?;
                Ok(with_object_type(kind, value))
            })
            .collect::<Result<Vec<_>, AppError>>()
    }

    async fn begin_native_graph_transaction(
        &self,
    ) -> Result<sqlx::Transaction<'_, Postgres>, AppError> {
        let mut transaction = self.pool.begin().await?;
        // The lock query can take a snapshot before it waits. Each subsequent
        // statement must see authority changes committed while waiting, even
        // when this connection's session default is REPEATABLE READ.
        sqlx::query("SET TRANSACTION ISOLATION LEVEL READ COMMITTED")
            .execute(&mut *transaction)
            .await?;
        lock_hardware_graph(&mut transaction).await?;
        Ok(transaction)
    }

    async fn save_value<F: ComponentTemplateAuthorizer>(
        &self,
        kind: ComponentTemplateKind,
        id: Uuid,
        changes: Value,
        create: bool,
        authorize: &mut F,
    ) -> Result<Option<Value>, AppError> {
        if !changes.is_object() {
            return Err(AppError::Validation("Expected an object".into()));
        }
        let mut transaction = self.begin_native_graph_transaction().await?;

        let mut record = if create {
            kind.create_defaults(id)
        } else {
            let statement = format!(
                "SELECT to_jsonb(t) AS record FROM {} t WHERE t.id=$1 FOR UPDATE",
                kind.table()
            );
            let row = sqlx::query(&statement)
                .bind(id)
                .fetch_optional(&mut *transaction)
                .await
                .map_err(classify_sqlx_error_owned)?;
            let Some(row) = row else {
                return Ok(None);
            };
            let value: Value = row.try_get("record")?;
            value
        };

        if !create {
            let attributes = with_object_type(kind, record.clone());
            authorize
                .authorize(transaction.as_mut(), Action::Update, id, attributes)
                .await?;
            ensure_parent_unchanged(&record, &changes)?;
        }

        merge_columns(&mut record, &changes);
        kind.validate(&record)?;
        kind.validate_references(
            &mut transaction,
            &record,
            id,
            &changes,
            create,
            &mut *authorize,
        )
        .await?;
        if create {
            // The collection-wide CREATE check ran at the route boundary. This
            // object-level call carries the persisted parent/reference
            // attributes into the same policy evaluator, so a create-scoped PAT
            // is accepted without implicitly granting READ on every parent id
            // named in the body.
            authorize
                .authorize(
                    transaction.as_mut(),
                    Action::Create,
                    id,
                    with_object_type(kind, record.clone()),
                )
                .await?;
        }
        ensure_inventory_sibling_name_unique(
            &mut transaction,
            kind,
            &record,
            id,
            create,
            &mut *authorize,
        )
        .await?;
        kind.write(&mut transaction, id, &record, &changes, create)
            .await?;
        kind.after_write(&mut transaction, &record).await?;

        let saved = kind
            .load_json(&mut transaction, id)
            .await?
            .ok_or_else(|| AppError::Internal("Saved component template not found".into()))?;
        transaction.commit().await?;
        Ok(Some(saved))
    }

    /// Delete one template after attribute authorization. The shared inventory
    /// delete guard owns every inbound reference: a mapped front/rear port, a
    /// power port used by an outlet, a child inventory template or any other
    /// referencing row blocks deletion and is reported through the standard
    /// `DELETE_BLOCKED` contract instead of being silently unwired.
    pub async fn delete<F: ComponentTemplateAuthorizer>(
        &self,
        kind: ComponentTemplateKind,
        id: Uuid,
        mut authorize: F,
    ) -> Result<(), AppError> {
        let mut transaction = self.begin_native_graph_transaction().await?;
        let statement = format!(
            "SELECT to_jsonb(t) AS record FROM {} t WHERE t.id=$1 FOR UPDATE",
            kind.table()
        );
        let row = sqlx::query(&statement)
            .bind(id)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(classify_sqlx_error_owned)?;
        let Some(row) = row else {
            return Err(AppError::NotFound(format!("{} not found", kind.label())));
        };
        let record: Value = row.try_get("record")?;
        authorize
            .authorize(
                transaction.as_mut(),
                Action::Delete,
                id,
                with_object_type(kind, record.clone()),
            )
            .await?;
        let deleted = super::super::delete_guard::delete_inventory_record_in_transaction(
            &mut transaction,
            kind.table(),
            id,
        )
        .await?;
        if deleted == 0 {
            return Err(AppError::NotFound(format!("{} not found", kind.label())));
        }
        if kind == ComponentTemplateKind::InventoryItem {
            recompute_inventory_tree(&mut transaction, required_uuid(&record, "device_type_id")?)
                .await?;
        }
        transaction.commit().await?;
        Ok(())
    }
}

/// Attach the canonical content-type token to policy attributes so attribute
/// constraints can name the object class without trusting the request.
fn with_object_type(kind: ComponentTemplateKind, mut value: Value) -> Value {
    if value.is_object() {
        value["object_type"] = Value::String(kind.content_type().to_string());
    }
    value
}

/// Apply supplied columns. `rear_ports` is a relation, not a column.
fn merge_columns(record: &mut Value, changes: &Value) {
    if let (Some(record), Some(changes)) = (record.as_object_mut(), changes.as_object()) {
        for (key, value) in changes {
            if key == "rear_ports" {
                continue;
            }
            record.insert(key.clone(), value.clone());
        }
    }
}

/// A component template's device/module parent defines its uniqueness scope and
/// cannot be moved after creation.
fn ensure_parent_unchanged(persisted: &Value, changes: &Value) -> Result<(), AppError> {
    for field in ["device_type_id", "module_type_id"] {
        if let Some(next) = changes.get(field)
            && persisted.get(field).unwrap_or(&Value::Null) != next
        {
            return Err(AppError::Validation(
                "Component templates cannot be moved to a different parent".into(),
            ));
        }
    }
    Ok(())
}

fn ensure_same_parent(record: &Value, reference: &Value) -> Result<(), AppError> {
    for field in ["device_type_id", "module_type_id"] {
        if record.get(field).unwrap_or(&Value::Null) != reference.get(field).unwrap_or(&Value::Null)
        {
            return Err(AppError::Validation(
                "Referenced template must belong to the same device type or module type".into(),
            ));
        }
    }
    Ok(())
}

async fn load_reference(
    transaction: &mut sqlx::Transaction<'_, Postgres>,
    table: &'static str,
    token: &str,
    id: Uuid,
) -> Result<Value, AppError> {
    let value: Option<Value> =
        sqlx::query_scalar(&format!("SELECT to_jsonb(t) FROM {table} t WHERE t.id=$1"))
            .bind(id)
            .fetch_optional(&mut **transaction)
            .await
            .map_err(classify_sqlx_error_owned)?;
    let mut value =
        value.ok_or_else(|| AppError::Validation(format!("{token} reference does not exist")))?;
    value["object_type"] = Value::String(token.to_string());
    Ok(value)
}

/// Validate and authorize one front-port template's complete positioned mapping
/// set against the persisted rear-port templates it names.
///
/// Crate-wide so the inventory importer reuses the front/rear position and
/// same-parent rules for `dcim.port-template-mappings` entities.
pub(crate) async fn authorize_front_mappings<F: ComponentTemplateAuthorizer>(
    transaction: &mut sqlx::Transaction<'_, Postgres>,
    record: &Value,
    mappings: &[FrontPortMappingChanges],
    create: bool,
    authorize: &mut F,
) -> Result<(), AppError> {
    let front_positions = required_i16(record, "positions")?;
    let mut front_positions_used = std::collections::HashSet::new();
    let mut rear_positions_used = std::collections::HashSet::new();
    for mapping in mappings {
        validate_positions("position", mapping.position)?;
        validate_positions("rear_port_position", mapping.rear_port_position)?;
        if mapping.position > front_positions {
            return Err(AppError::Validation(
                "Mapping position exceeds the front port template positions".into(),
            ));
        }
        if !front_positions_used.insert(mapping.position) {
            return Err(AppError::Validation(
                "Each front port position can be mapped only once".into(),
            ));
        }
        if !rear_positions_used.insert((mapping.rear_port, mapping.rear_port_position)) {
            return Err(AppError::Validation(
                "Each rear port position can be mapped only once".into(),
            ));
        }
        let attributes = load_reference(
            transaction,
            "dcim_rearporttemplate",
            "dcim.rearporttemplate",
            mapping.rear_port,
        )
        .await?;
        // Authorize the persisted rear port before its hidden parent and
        // position properties can distinguish validation failures from denial.
        if !create {
            authorize
                .authorize(
                    transaction.as_mut(),
                    Action::Read,
                    mapping.rear_port,
                    attributes.clone(),
                )
                .await?;
        }
        ensure_same_parent(record, &attributes)?;
        let rear_positions = required_i16(&attributes, "positions")?;
        if mapping.rear_port_position > rear_positions {
            return Err(AppError::Validation(
                "Mapping position exceeds the rear port template positions".into(),
            ));
        }
    }
    Ok(())
}

/// Validate and authorize the references of one inventory-item template:
/// same-device-type parent, acyclic hierarchy, manufacturer/role, and the
/// modular component target.
///
/// Crate-wide so the inventory importer reuses the hierarchy and typed-target
/// rules instead of copying them.
pub(crate) async fn authorize_inventory_references<F: ComponentTemplateAuthorizer>(
    transaction: &mut sqlx::Transaction<'_, Postgres>,
    record: &Value,
    id: Uuid,
    create: bool,
    authorize: &mut F,
) -> Result<(), AppError> {
    let device_type_id = required_uuid(record, "device_type_id")?;
    if let Some(parent_id) = optional_uuid(record, "parent_id")? {
        if parent_id == id {
            return Err(AppError::Validation(
                "An inventory item template cannot be its own parent".into(),
            ));
        }
        let attributes = load_reference(
            transaction,
            "dcim_inventoryitemtemplate",
            "dcim.inventoryitemtemplate",
            parent_id,
        )
        .await?;
        // Authorize the persisted parent before its device type or the
        // hierarchy shape can reveal hidden properties through the error code.
        if !create {
            authorize
                .authorize(
                    transaction.as_mut(),
                    Action::Read,
                    parent_id,
                    attributes.clone(),
                )
                .await?;
        }
        if required_uuid(&attributes, "device_type_id")? != device_type_id {
            return Err(AppError::Validation(
                "Parent inventory item template must belong to the same device type".into(),
            ));
        }
        let cycle: bool = sqlx::query_scalar(
            "WITH RECURSIVE ancestors AS (SELECT id,parent_id FROM dcim_inventoryitemtemplate WHERE id=$1 UNION ALL SELECT parent.id,parent.parent_id FROM dcim_inventoryitemtemplate parent JOIN ancestors child ON parent.id=child.parent_id) SELECT EXISTS(SELECT 1 FROM ancestors WHERE id=$2)",
        )
        .bind(parent_id)
        .bind(id)
        .fetch_one(&mut **transaction)
        .await
        .map_err(classify_sqlx_error_owned)?;
        if cycle {
            return Err(AppError::Validation(
                "Inventory item template hierarchy cannot contain a cycle".into(),
            ));
        }
    }

    for (field, table, token) in [
        ("manufacturer_id", "dcim_manufacturer", "dcim.manufacturer"),
        (
            "role_id",
            "dcim_inventoryitemrole",
            "dcim.inventoryitemrole",
        ),
    ] {
        if let Some(reference_id) = optional_uuid(record, field)? {
            let attributes = load_reference(transaction, table, token, reference_id).await?;
            if !create {
                authorize
                    .authorize(transaction.as_mut(), Action::Read, reference_id, attributes)
                    .await?;
            }
        }
    }

    if let (Some(component_type_id), Some(component_id)) = (
        optional_uuid(record, "component_type_id")?,
        optional_uuid(record, "component_id")?,
    ) {
        let token: Option<String> = sqlx::query_scalar(
            "SELECT app_label || '.' || model FROM inventory_content_type WHERE id=$1",
        )
        .bind(component_type_id)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(classify_sqlx_error_owned)?;
        let token = token.ok_or_else(|| {
            AppError::Validation(
                "component_type_id does not reference a registered content type".into(),
            )
        })?;
        if !INVENTORY_ITEM_TEMPLATE_COMPONENT_OBJECT_TYPES.contains(&token.as_str()) {
            return Err(AppError::Validation(
                "component_type_id is not a modular component template type".into(),
            ));
        }
        let definition = by_token(&token).ok_or_else(|| {
            AppError::Validation(
                "component_type_id is not a registered component template type".into(),
            )
        })?;
        let attributes =
            load_reference(transaction, definition.table, &token, component_id).await?;
        // Authorize the persisted component before its parent scope is checked.
        if !create {
            authorize
                .authorize(
                    transaction.as_mut(),
                    Action::Read,
                    component_id,
                    attributes.clone(),
                )
                .await?;
        }
        if required_uuid(&attributes, "device_type_id")? != device_type_id
            || optional_uuid(&attributes, "module_type_id")?.is_some()
        {
            return Err(AppError::Validation(
                "Referenced component template must belong to the same device type".into(),
            ));
        }
    }
    Ok(())
}

async fn replace_front_mappings(
    transaction: &mut sqlx::Transaction<'_, Postgres>,
    front_port_id: Uuid,
    record: &Value,
    mappings: &[FrontPortMappingChanges],
) -> Result<(), AppError> {
    sqlx::query("DELETE FROM dcim_porttemplatemapping WHERE front_port_id=$1")
        .bind(front_port_id)
        .execute(&mut **transaction)
        .await
        .map_err(classify_sqlx_error_owned)?;
    let device_type_id = optional_uuid(record, "device_type_id")?;
    let module_type_id = optional_uuid(record, "module_type_id")?;
    for mapping in mappings {
        sqlx::query(
            "INSERT INTO dcim_porttemplatemapping (id,front_port_position,rear_port_position,device_type_id,module_type_id,front_port_id,rear_port_id) VALUES ($1,$2,$3,$4,$5,$6,$7)",
        )
        .bind(Uuid::now_v7())
        .bind(mapping.position)
        .bind(mapping.rear_port_position)
        .bind(device_type_id)
        .bind(module_type_id)
        .bind(front_port_id)
        .bind(mapping.rear_port)
        .execute(&mut **transaction)
        .await
        .map_err(classify_sqlx_error_owned)?;
    }
    Ok(())
}

/// PostgreSQL's `(device_type_id, parent_id, name)` unique constraint treats
/// two NULL parents as distinct, so root-level duplicate names are not caught by
/// the database. The native API contract is stricter: siblings must have unique
/// names in both root and child scopes. This runs under the same hardware-graph
/// lock as the write, without adding global DDL that could break existing dumps.
///
/// Only the single retained row matching the exact predicate
/// `(device_type_id, name, null-safe parent_id, id <> self)` is loaded. On an
/// update that row is READ-authorized with its persisted attributes before the
/// conflict is reported, so a caller cannot use the conflict response to probe
/// a hidden sibling, and an unrelated hidden row in the same device type does
/// not block a legitimate write. A create keeps the create-scoped PAT rule and
/// does not demand a new READ grant.
///
/// Crate-wide so the inventory importer applies the same rule inside its
/// transfer transaction; the caller must hold the hardware-graph lock.
pub(crate) async fn ensure_inventory_sibling_name_unique<F: ComponentTemplateAuthorizer>(
    transaction: &mut sqlx::Transaction<'_, Postgres>,
    kind: ComponentTemplateKind,
    record: &Value,
    id: Uuid,
    create: bool,
    authorize: &mut F,
) -> Result<(), AppError> {
    if kind != ComponentTemplateKind::InventoryItem {
        return Ok(());
    }
    let device_type_id = required_uuid(record, "device_type_id")?;
    let parent_id = optional_uuid(record, "parent_id")?;
    let name = required_str(record, "name")?;
    let conflict: Option<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(t) FROM dcim_inventoryitemtemplate t \
         WHERE t.device_type_id=$1 AND t.name=$2 AND t.id<>$3 \
         AND ((t.parent_id IS NULL AND $4::uuid IS NULL) OR t.parent_id=$4) \
         ORDER BY t.id LIMIT 1",
    )
    .bind(device_type_id)
    .bind(name)
    .bind(id)
    .bind(parent_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(classify_sqlx_error_owned)?;
    let Some(conflict) = conflict else {
        return Ok(());
    };
    if !create {
        let conflict_id = required_uuid(&conflict, "id")?;
        authorize
            .authorize(
                transaction.as_mut(),
                Action::Read,
                conflict_id,
                with_object_type(ComponentTemplateKind::InventoryItem, conflict),
            )
            .await?;
    }
    Err(AppError::Conflict(
        "An inventory item template with this name already exists under the same parent".into(),
    ))
}

/// Correct nested-set rebuild for the inventory-item template forest.
///
/// `lft` is the DFS entry number and `rght` the DFS exit number. For a node at
/// preorder `p`, depth `d` and subtree size `s`:
/// `lft = 2p - 1 - d`, `rght = 2p + 2s - d - 2`. This gives root `(1,4)` and
/// its only child `(2,3)` — never a shared boundary. `$1` is the UUID device
/// type; only the hash input is cast to text.
const INVENTORY_TREE_SQL: &str = r#"
WITH RECURSIVE siblings AS (
    SELECT id, parent_id, row_number() OVER (PARTITION BY parent_id ORDER BY name, id) AS ord
    FROM dcim_inventoryitemtemplate
    WHERE device_type_id=$1
), tree AS (
    SELECT id, ARRAY[ord] AS path, 0 AS depth
    FROM siblings
    WHERE parent_id IS NULL
    UNION ALL
    SELECT child.id, parent.path || child.ord, parent.depth + 1
    FROM siblings child
    JOIN tree parent ON child.parent_id = parent.id
), ranked AS (
    SELECT node.id, node.path, node.depth,
           row_number() OVER (ORDER BY node.path) AS preorder,
           (SELECT count(*) FROM tree descendant
            WHERE descendant.path[1:(node.depth + 1)] = node.path) AS size
    FROM tree node
)
UPDATE dcim_inventoryitemtemplate target
SET lft = (2 * ranked.preorder - 1 - ranked.depth)::integer,
    rght = (2 * ranked.preorder + 2 * ranked.size - ranked.depth - 2)::integer,
    level = ranked.depth::integer,
    tree_id = hashtext($1::text)
FROM ranked
WHERE target.id = ranked.id
"#;

/// Rebuild the local nested-set columns for one device type from the adjacency
/// list. One tree per device type, deterministic preorder by `(name, id)`.
/// `tree_id` is a stable internal hash of the device type and is never exposed.
async fn recompute_inventory_tree(
    transaction: &mut sqlx::Transaction<'_, Postgres>,
    device_type_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(INVENTORY_TREE_SQL)
        .bind(device_type_id)
        .execute(&mut **transaction)
        .await
        .map_err(classify_sqlx_error_owned)?;
    Ok(())
}

fn required_str<'a>(record: &'a Value, field: &str) -> Result<&'a str, AppError> {
    record
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::Internal(format!("Component template {field} is not a string")))
}

fn optional_str<'a>(record: &'a Value, field: &str) -> Result<Option<&'a str>, AppError> {
    match record.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value)),
        Some(_) => Err(AppError::Internal(format!(
            "Component template {field} is not a string"
        ))),
    }
}

fn required_uuid(record: &Value, field: &str) -> Result<Uuid, AppError> {
    optional_uuid(record, field)?
        .ok_or_else(|| AppError::Validation(format!("{field} is required")))
}

fn optional_uuid(record: &Value, field: &str) -> Result<Option<Uuid>, AppError> {
    match record.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Uuid::parse_str(value)
            .map(Some)
            .map_err(|_| AppError::Validation(format!("Invalid {field}"))),
        Some(_) => Err(AppError::Internal(format!(
            "Component template {field} is not a UUID"
        ))),
    }
}

fn required_i16(record: &Value, field: &str) -> Result<i16, AppError> {
    record
        .get(field)
        .and_then(Value::as_i64)
        .and_then(|value| i16::try_from(value).ok())
        .ok_or_else(|| AppError::Internal(format!("Component template {field} is not an integer")))
}

fn optional_i32(record: &Value, field: &str) -> Result<Option<i32>, AppError> {
    match record.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_i64()
            .and_then(|value| i32::try_from(value).ok())
            .map(Some)
            .ok_or_else(|| {
                AppError::Internal(format!("Component template {field} is not an integer"))
            }),
    }
}

fn required_bool(record: &Value, field: &str) -> Result<bool, AppError> {
    record
        .get(field)
        .and_then(Value::as_bool)
        .ok_or_else(|| AppError::Internal(format!("Component template {field} is not a boolean")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parent_move_is_rejected_but_an_identical_parent_is_accepted() {
        let persisted = json!({"device_type_id": "a", "module_type_id": null});
        assert!(
            ensure_parent_unchanged(&persisted, &json!({"device_type_id": "a"})).is_ok(),
            "re-sending the same parent is not a move"
        );
        assert!(
            ensure_parent_unchanged(&persisted, &json!({"device_type_id": "b"})).is_err(),
            "changing the parent must be rejected"
        );
        assert!(
            ensure_parent_unchanged(&persisted, &json!({"module_type_id": "c"})).is_err(),
            "moving from device type to module type must be rejected"
        );
    }

    #[test]
    fn merge_columns_ignores_the_mapping_relation() {
        let mut record = json!({"name": "old", "positions": 1});
        merge_columns(
            &mut record,
            &json!({"name": "new", "rear_ports": [], "positions": null}),
        );
        assert_eq!(record["name"], "new");
        assert!(record.get("rear_ports").is_none());
        assert!(record["positions"].is_null());
    }

    #[test]
    fn object_type_is_stamped_for_policy_attributes() {
        let attributes = with_object_type(ComponentTemplateKind::ConsolePort, json!({"id": "x"}));
        assert_eq!(attributes["object_type"], "dcim.consoleporttemplate");
    }

    #[test]
    fn kind_metadata_matches_the_canonical_content_type_registry() {
        use crate::contexts::inventory::domain::content_type::by_token;
        for kind in [
            ComponentTemplateKind::ConsolePort,
            ComponentTemplateKind::ConsoleServerPort,
            ComponentTemplateKind::PowerPort,
            ComponentTemplateKind::PowerOutlet,
            ComponentTemplateKind::Interface,
            ComponentTemplateKind::FrontPort,
            ComponentTemplateKind::RearPort,
            ComponentTemplateKind::DeviceBay,
            ComponentTemplateKind::InventoryItem,
        ] {
            let definition = by_token(kind.content_type())
                .unwrap_or_else(|| panic!("{} is not registered", kind.content_type()));
            assert_eq!(definition.table, kind.table());
            let capabilities = definition
                .capabilities()
                .unwrap_or_else(|| panic!("{} has no pinned capabilities", kind.content_type()));
            assert!(!capabilities.tags);
            assert!(!capabilities.custom_fields);
            assert!(!capabilities.change_dates);
        }
        let cooling = by_token("dcim.coolingintaketemplate").expect("cooling template");
        assert!(
            cooling
                .capabilities()
                .is_some_and(|capabilities| capabilities.change_dates),
            "specialized cooling templates keep their existing metadata contract"
        );
    }

    /// Every component-template route supplies policy attributes. Other native
    /// Inventory owners may register their own attribute-aware object types.
    #[test]
    fn policy_object_types_are_the_attribute_wired_component_kinds() {
        use crate::contexts::access_control::domain::vocabulary::INVENTORY_ATTRIBUTE_OBJECT_TYPES;
        let kind_tokens: Vec<&str> = [
            ComponentTemplateKind::ConsolePort,
            ComponentTemplateKind::ConsoleServerPort,
            ComponentTemplateKind::PowerPort,
            ComponentTemplateKind::PowerOutlet,
            ComponentTemplateKind::Interface,
            ComponentTemplateKind::FrontPort,
            ComponentTemplateKind::RearPort,
            ComponentTemplateKind::DeviceBay,
            ComponentTemplateKind::InventoryItem,
        ]
        .into_iter()
        .map(ComponentTemplateKind::content_type)
        .collect();
        for token in kind_tokens {
            assert!(
                INVENTORY_ATTRIBUTE_OBJECT_TYPES.contains(&token),
                "component-template policy object type is not attribute-wired: {token}"
            );
        }
    }
}
