//! Native cooling facilities, device ports and their hardware templates.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

// NetBox v4.7.1 netbox/netbox/choices.py: DiameterUnitChoices and
// FlowRateUnitChoices, consumed by dcim.models.mixins DiameterMixin/MaxFlowMixin.
pub const DIAMETER_UNITS: &[&str] = &["mm", "cm", "in"];
pub const FLOW_RATE_UNITS: &[&str] = &["lpm", "m3ph", "gpm"];

/// A numeric-only native DTO value which retains the original decimal lexeme
/// until the measurement's range and scale are checked. Source decimal strings
/// enter through `CoolingMeasurement::validate_source`, not this native contract.
#[derive(Debug, Clone, Serialize)]
#[serde(transparent)]
pub struct CoolingDecimal(serde_json::Number);

impl<'de> Deserialize<'de> for CoolingDecimal {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Number's arbitrary-precision visitor also accepts a private-token
        // object. Classify the actual JSON token before asking it to decode.
        let raw = Box::<serde_json::value::RawValue>::deserialize(deserializer)?;
        if !matches!(
            raw.get().as_bytes().first().copied(),
            Some(b'-' | b'0'..=b'9')
        ) {
            return Err(serde::de::Error::custom(
                "Cooling measurement must be a JSON number",
            ));
        }
        serde_json::from_str(raw.get())
            .map(Self)
            .map_err(serde::de::Error::custom)
    }
}

/// Rack inputs retain their existing float output model, but must validate the
/// raw numeric token before converting it. Null and absence remain distinct in
/// the patch adapter below; neither adapter accepts native decimal strings.
pub fn deserialize_optional_cooling_capacity<'de, D>(
    deserializer: D,
) -> Result<Option<f64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<CoolingDecimal>::deserialize(deserializer)?
        .as_ref()
        .map(|number| {
            CoolingMeasurement::Capacity
                .validate(&number.0)
                .map_err(serde::de::Error::custom)
        })
        .transpose()
}

pub fn deserialize_cooling_capacity_patch<'de, D>(
    deserializer: D,
) -> Result<Option<Option<f64>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_optional_cooling_capacity(deserializer).map(Some)
}

#[derive(Debug, Clone, Copy)]
pub enum CoolingMeasurement {
    Capacity,
    Diameter,
    MaxFlow,
}

impl CoolingMeasurement {
    /// Validate the original numeric token before any binary float conversion.
    pub fn validate(self, value: &serde_json::Number) -> Result<f64, crate::errors::AppError> {
        self.validate_decimal(&value.to_string())
    }

    fn validate_decimal(self, raw: &str) -> Result<f64, crate::errors::AppError> {
        let (field, minimum, maximum) = match self {
            Self::Capacity => ("cooling_capacity", 0, 9_999_999_999),
            Self::Diameter => ("diameter", 1, 99_999_999),
            Self::MaxFlow => ("max_flow", 1, 99_999_999),
        };
        let cents = super::super::decimal::scaled_decimal(raw, 2)
            .filter(|cents| (minimum..=maximum).contains(cents))
            .ok_or_else(|| {
                crate::errors::AppError::Validation(format!(
                    "{field} must be in range with at most two decimal places"
                ))
            })?;
        // The bounded integer cents are exact in f64; the quotient's shortest
        // decimal representation retains the validated two-decimal value.
        Ok(cents as f64 / 100.0)
    }

    /// NetBox decimal fields may be JSON numbers or signed decimal strings with
    /// optional exponent and zero padding. Whitespace/nonfinite names are invalid.
    pub fn validate_source(
        self,
        value: &serde_json::Value,
    ) -> Result<f64, crate::errors::AppError> {
        match value {
            serde_json::Value::Number(number) => self.validate(number),
            serde_json::Value::String(raw) => self.validate_decimal(raw),
            _ => Err(crate::errors::AppError::Validation(
                "Invalid cooling decimal".into(),
            )),
        }
    }

    /// Float-only callers can prove only that the shortest round-trip decimal
    /// is representable at scale two, not recover an earlier input lexeme.
    pub fn validate_float(self, value: f64) -> Result<(), crate::errors::AppError> {
        let number = serde_json::Number::from_f64(value).ok_or_else(|| {
            crate::errors::AppError::Validation("Cooling measurement must be finite".into())
        })?;
        self.validate(&number).map(|_| ())
    }
}

pub fn validate_cooling_method(value: Option<&str>) -> Result<(), crate::errors::AppError> {
    if value.is_some_and(|v| !["air", "liquid", "hybrid", "immersion"].contains(&v)) {
        return Err(crate::errors::AppError::Validation(
            "Invalid cooling_method".into(),
        ));
    }
    Ok(())
}

pub fn validate_rack_cooling(
    capability: Option<&str>,
    capacity: Option<f64>,
) -> Result<(), crate::errors::AppError> {
    if capability.is_some_and(|value| !["air-only", "hybrid", "liquid-only"].contains(&value)) {
        return Err(crate::errors::AppError::Validation(
            "Invalid cooling_capability".into(),
        ));
    }
    if let Some(capacity) = capacity {
        CoolingMeasurement::Capacity.validate_float(capacity)?;
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoolingKind {
    Source,
    Feed,
    Intake,
    Outflow,
    IntakeTemplate,
    OutflowTemplate,
}

impl CoolingKind {
    pub const fn table(self) -> &'static str {
        match self {
            Self::Source => "dcim_coolingsource",
            Self::Feed => "dcim_coolingfeed",
            Self::Intake => "dcim_coolingintake",
            Self::Outflow => "dcim_coolingoutflow",
            Self::IntakeTemplate => "dcim_coolingintaketemplate",
            Self::OutflowTemplate => "dcim_coolingoutflowtemplate",
        }
    }

    pub const fn content_type(self) -> &'static str {
        match self {
            Self::Source => "dcim.coolingsource",
            Self::Feed => "dcim.coolingfeed",
            Self::Intake => "dcim.coolingintake",
            Self::Outflow => "dcim.coolingoutflow",
            Self::IntakeTemplate => "dcim.coolingintaketemplate",
            Self::OutflowTemplate => "dcim.coolingoutflowtemplate",
        }
    }

    pub const fn has_custom_fields(self) -> bool {
        !matches!(self, Self::IntakeTemplate | Self::OutflowTemplate)
    }

    pub const fn fields(self) -> &'static [&'static str] {
        match self {
            Self::Source => &[
                "name",
                "type",
                "status",
                "fluid_type",
                "cooling_capacity",
                "site_id",
                "location_id",
                "description",
                "comments",
                "owner_id",
                "tags",
                "custom_field_data",
            ],
            Self::Feed => &[
                "name",
                "status",
                "cooling_capacity",
                "max_flow",
                "max_flow_unit",
                "cooling_source_id",
                "rack_id",
                "tenant_id",
                "description",
                "comments",
                "owner_id",
                "tags",
                "custom_field_data",
            ],
            Self::Intake => &[
                "name",
                "label",
                "type",
                "diameter",
                "diameter_unit",
                "max_flow",
                "max_flow_unit",
                "device_id",
                "module_id",
                "cooling_outflow_id",
                "description",
                "owner_id",
                "tags",
                "custom_field_data",
            ],
            Self::Outflow => &[
                "name",
                "label",
                "type",
                "diameter",
                "diameter_unit",
                "device_id",
                "module_id",
                "cooling_intake_id",
                "description",
                "owner_id",
                "tags",
                "custom_field_data",
            ],
            Self::IntakeTemplate => &[
                "name",
                "label",
                "type",
                "diameter",
                "diameter_unit",
                "max_flow",
                "max_flow_unit",
                "device_type_id",
                "module_type_id",
                "description",
            ],
            Self::OutflowTemplate => &[
                "name",
                "label",
                "type",
                "diameter",
                "diameter_unit",
                "device_type_id",
                "module_type_id",
                "cooling_intake_id",
                "description",
            ],
        }
    }
}

// Create and partial-update contracts share field declarations, not a second
// model. The update's outer Option records presence; the inner Option records
// explicit null. Serialization omits absent fields before the repository merge.
macro_rules! cooling_contract {
    ($create:ident, $patch:ident, $model:ident, { $($(#[$field_schema:meta])* $field:ident: $ty:ty => $nullable:literal),* $(,)? } $(, $counter:ident)*) => {
        #[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
        #[serde(deny_unknown_fields)]
        pub struct $create { $($(#[$field_schema])* pub $field: $ty,)* }

        #[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
        #[serde(deny_unknown_fields)]
        pub struct $patch {
            $($(#[$field_schema])*
            #[schema(nullable = $nullable)]
            #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::api::rest::routes::validation::deserialize_nullable_patch")]
            pub $field: Option<Option<$ty>> ,)*
        }

        #[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
        pub struct $model {
            pub id: Uuid,
            $(pub $counter: i64,)*
            #[serde(flatten)]
            pub fields: $create,
            pub created: Option<chrono::DateTime<chrono::Utc>>,
            pub last_updated: Option<chrono::DateTime<chrono::Utc>>,
            pub external_id: Option<i64>,
            pub piping_synced_at: Option<chrono::DateTime<chrono::Utc>>,
        }

        impl<'de> Deserialize<'de> for $model {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                // Serde flatten buffers fields as Content, whose deserializer
                // does not support RawValue. Decode the same field declarations
                // directly so both JSON bytes and from_value retain support.
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Wire {
                    id: Uuid,
                    $($counter: i64,)*
                    $($field: $ty,)*
                    created: Option<chrono::DateTime<chrono::Utc>>,
                    last_updated: Option<chrono::DateTime<chrono::Utc>>,
                    external_id: Option<i64>,
                    piping_synced_at: Option<chrono::DateTime<chrono::Utc>>,
                }
                let wire = Wire::deserialize(deserializer)?;
                Ok(Self {
                    id: wire.id,
                    $($counter: wire.$counter,)*
                    fields: $create { $($field: wire.$field,)* },
                    created: wire.created,
                    last_updated: wire.last_updated,
                    external_id: wire.external_id,
                    piping_synced_at: wire.piping_synced_at,
                })
            }
        }
    };
}

cooling_contract!(CoolingSourceCreate, CoolingSourcePatch, CoolingSource, {
    name: String => false, r#type: String => false, status: Option<String> => false,
    fluid_type: Option<String> => true,
    #[schema(value_type = Option<f64>)]
    cooling_capacity: Option<CoolingDecimal> => true,
    site_id: Uuid => false, location_id: Option<Uuid> => true,
    description: Option<String> => false, comments: Option<String> => false,
    owner_id: Option<Uuid> => true,
    #[schema(schema_with = crate::contexts::inventory::domain::extras::tag::membership_schema)]
    tags: Option<Vec<String>> => false,
    #[schema(value_type = Option<std::collections::HashMap<String, serde_json::Value>>, additional_properties = true)]
    custom_fields: Option<serde_json::Value> => false,
}, coolingfeed_count);
cooling_contract!(CoolingFeedCreate, CoolingFeedPatch, CoolingFeed, {
    name: String => false, status: Option<String> => false,
    #[schema(value_type = Option<f64>)]
    cooling_capacity: Option<CoolingDecimal> => true,
    #[schema(value_type = Option<f64>)]
    max_flow: Option<CoolingDecimal> => true,
    max_flow_unit: Option<String> => true, cooling_source_id: Uuid => false,
    rack_id: Option<Uuid> => true, tenant_id: Option<Uuid> => true,
    description: Option<String> => false, comments: Option<String> => false,
    owner_id: Option<Uuid> => true,
    #[schema(schema_with = crate::contexts::inventory::domain::extras::tag::membership_schema)]
    tags: Option<Vec<String>> => false,
    #[schema(value_type = Option<std::collections::HashMap<String, serde_json::Value>>, additional_properties = true)]
    custom_fields: Option<serde_json::Value> => false,
});
cooling_contract!(CoolingIntakeCreate, CoolingIntakePatch, CoolingIntake, {
    name: String => false, label: Option<String> => false, r#type: Option<String> => true,
    #[schema(value_type = Option<f64>)]
    diameter: Option<CoolingDecimal> => true, diameter_unit: Option<String> => true,
    #[schema(value_type = Option<f64>)]
    max_flow: Option<CoolingDecimal> => true, max_flow_unit: Option<String> => true,
    device_id: Uuid => false, module_id: Option<Uuid> => true,
    cooling_outflow_id: Option<Uuid> => true, description: Option<String> => false,
    owner_id: Option<Uuid> => true,
    #[schema(schema_with = crate::contexts::inventory::domain::extras::tag::membership_schema)]
    tags: Option<Vec<String>> => false,
    #[schema(value_type = Option<std::collections::HashMap<String, serde_json::Value>>, additional_properties = true)]
    custom_fields: Option<serde_json::Value> => false,
});
cooling_contract!(CoolingOutflowCreate, CoolingOutflowPatch, CoolingOutflow, {
    name: String => false, label: Option<String> => false, r#type: Option<String> => true,
    #[schema(value_type = Option<f64>)]
    diameter: Option<CoolingDecimal> => true, diameter_unit: Option<String> => true,
    device_id: Uuid => false, module_id: Option<Uuid> => true,
    cooling_intake_id: Option<Uuid> => true, description: Option<String> => false,
    owner_id: Option<Uuid> => true,
    #[schema(schema_with = crate::contexts::inventory::domain::extras::tag::membership_schema)]
    tags: Option<Vec<String>> => false,
    #[schema(value_type = Option<std::collections::HashMap<String, serde_json::Value>>, additional_properties = true)]
    custom_fields: Option<serde_json::Value> => false,
});
cooling_contract!(CoolingIntakeTemplateCreate, CoolingIntakeTemplatePatch, CoolingIntakeTemplate, {
    name: String => false, label: Option<String> => false, r#type: Option<String> => true,
    #[schema(value_type = Option<f64>)]
    diameter: Option<CoolingDecimal> => true, diameter_unit: Option<String> => true,
    #[schema(value_type = Option<f64>)]
    max_flow: Option<CoolingDecimal> => true, max_flow_unit: Option<String> => true,
    device_type_id: Option<Uuid> => true, module_type_id: Option<Uuid> => true,
    description: Option<String> => false,
});
cooling_contract!(CoolingOutflowTemplateCreate, CoolingOutflowTemplatePatch, CoolingOutflowTemplate, {
    name: String => false, label: Option<String> => false, r#type: Option<String> => true,
    #[schema(value_type = Option<f64>)]
    diameter: Option<CoolingDecimal> => true, diameter_unit: Option<String> => true,
    device_type_id: Option<Uuid> => true, module_type_id: Option<Uuid> => true,
    cooling_intake_id: Option<Uuid> => true, description: Option<String> => false,
});
