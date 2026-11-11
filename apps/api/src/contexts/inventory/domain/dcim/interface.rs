use serde::{Deserialize, Deserializer, Serialize};

use crate::errors::AppError;

/// Numeric-only RF scalar, checked at scale three before float conversion.
/// Decode Number directly because flattened arbitrary-precision JSON carries a
/// private numeric map token that serde's generic f64 visitor cannot consume.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(transparent)]
pub struct InterfaceRfDecimal(pub f64);

impl<'de> Deserialize<'de> for InterfaceRfDecimal {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let number = serde_json::Number::deserialize(d)?;
        // Standalone scalars use the wider frequency policy. Native networking
        // fields use optional_rf to apply their own bound to the original token.
        InterfaceRfMeasurement::Frequency
            .validate(&number)
            .map(Self)
            .map_err(serde::de::Error::custom)
    }
}

/// Exact native and source RF decimal policy, matching the signed SQL columns.
#[derive(Debug, Clone, Copy)]
pub enum InterfaceRfMeasurement {
    Frequency,
    Width,
}

impl InterfaceRfMeasurement {
    /// Validate the original JSON number before converting to a binary float.
    pub fn validate(self, value: &serde_json::Number) -> Result<f64, AppError> {
        self.validate_decimal(&value.to_string())
    }

    /// Validate a source decimal string, including padding and signed exponents.
    pub fn validate_decimal(self, raw: &str) -> Result<f64, AppError> {
        let (field, bound) = match self {
            Self::Frequency => ("rf_channel_frequency", 100_000_000),
            Self::Width => ("rf_channel_width", 10_000_000),
        };
        let thousandths = super::super::decimal::scaled_decimal(raw, 3)
            .filter(|value| *value > -bound && *value < bound)
            .ok_or_else(|| AppError::Validation(format!("Invalid interface {field}")))?;
        // Bounded integer thousandths are exact in f64. Their quotient retains
        // the validated decimal in its shortest round-trip representation.
        Ok(thousandths as f64 / 1000.0)
    }

    /// Source conversion must call this before its generic f64 scalar adapter.
    pub fn validate_source(self, value: &serde_json::Value) -> Result<f64, AppError> {
        match value {
            serde_json::Value::Number(number) => self.validate(number),
            serde_json::Value::String(raw) => self.validate_decimal(raw),
            _ => Err(AppError::Validation("Invalid interface RF decimal".into())),
        }
    }

    /// Validate an already decoded scalar; an earlier lexeme cannot be recovered.
    pub fn validate_float(self, value: f64) -> Result<(), AppError> {
        let number = serde_json::Number::from_f64(value)
            .ok_or_else(|| AppError::Validation("RF decimal is not finite".into()))?;
        self.validate(&number).map(|_| ())
    }
}

fn nullable<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}

fn optional_rf<'de, D: Deserializer<'de>>(
    d: D,
    measurement: InterfaceRfMeasurement,
) -> Result<Option<InterfaceRfDecimal>, D::Error> {
    Option::<serde_json::Number>::deserialize(d)?
        .as_ref()
        .map(|number| {
            measurement
                .validate(number)
                .map(InterfaceRfDecimal)
                .map_err(serde::de::Error::custom)
        })
        .transpose()
}

macro_rules! rf_deserializers {
    ($optional:ident, $patch:ident, $measurement:ident) => {
        fn $optional<'de, D: Deserializer<'de>>(
            d: D,
        ) -> Result<Option<InterfaceRfDecimal>, D::Error> {
            optional_rf(d, InterfaceRfMeasurement::$measurement)
        }

        fn $patch<'de, D: Deserializer<'de>>(
            d: D,
        ) -> Result<Option<Option<InterfaceRfDecimal>>, D::Error> {
            $optional(d).map(Some)
        }
    };
}

rf_deserializers!(optional_rf_frequency, patch_rf_frequency, Frequency);
rf_deserializers!(optional_rf_width, patch_rf_width, Width);

fn merged<T: Clone>(patch: &Option<Option<T>>, current: &Option<T>) -> Option<T> {
    patch.clone().unwrap_or_else(|| current.clone())
}

macro_rules! choices {
    // Keep rename tokens unwrapped: utoipa 5.5's serde parser does not descend
    // into the invisible groups introduced by a forwarded `literal` fragment.
    ($name:ident { $($variant:ident = $value:tt),+ $(,)? }) => {
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
        pub enum $name {
            $(#[serde(rename = $value)] $variant,)+
        }

        #[cfg(test)]
        impl $name {
            fn assert_wire_schema() {
                let schema = serde_json::to_value(<Self as utoipa::PartialSchema>::schema())
                    .expect("choice schema");
                let wire_values = vec![$(serde_json::to_value(Self::$variant)
                    .expect("serialized choice")),+];
                assert_eq!(schema["enum"], serde_json::json!(wire_values));
                for value in wire_values {
                    let decoded: Self = serde_json::from_value(value.clone())
                        .expect("schema choice accepted on wire");
                    assert_eq!(serde_json::to_value(decoded).expect("choice round-trip"), value);
                }
            }
        }
    };
}

choices!(InterfaceMode { Access = "access", Tagged = "tagged", TaggedAll = "tagged-all", QInQ = "q-in-q" });
choices!(InterfaceDuplex { Half = "half", Full = "full", Auto = "auto" });
choices!(InterfacePoeMode { Pd = "pd", Pse = "pse" });
choices!(InterfacePoeType {
    Type1 = "type1-ieee802.3af", Type2 = "type2-ieee802.3at",
    Type3 = "type3-ieee802.3bt", Type4 = "type4-ieee802.3bt",
    Passive24v2Pair = "passive-24v-2pair", Passive24v4Pair = "passive-24v-4pair",
    Passive48v2Pair = "passive-48v-2pair", Passive48v4Pair = "passive-48v-4pair",
});
choices!(InterfaceRfRole { Ap = "ap", Station = "station" });
choices!(InterfaceCableEnd { A = "A", B = "B" });

macro_rules! networking_attributes {
    (
        $($(#[$schema:meta])* $field:ident: $ty:ty),+ $(,)?;
        $($(#[$rf_schema:meta])* $rf_field:ident => ($optional:tt, $patch:tt)),+ $(,)?
    ) => {
        /// Nullable attributes of NetBox 4.7.1 dcim.Interface.
        #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
        pub struct InterfaceNetworking {
            $($(#[$schema])* pub $field: Option<$ty>,)+
            $(#[serde(default, deserialize_with = $optional)]
            $(#[$rf_schema])*
            pub $rf_field: Option<InterfaceRfDecimal>,)+
        }

        /// Omission preserves the existing value; explicit null clears it.
        #[derive(Debug, Clone, Default, Deserialize, utoipa::ToSchema)]
        pub struct InterfaceNetworkingPatch {
            $(#[serde(default, deserialize_with = "nullable")]
            $(#[$schema])*
            pub $field: Option<Option<$ty>>,)+
            $(#[serde(default, deserialize_with = $patch)]
            $(#[$rf_schema])*
            pub $rf_field: Option<Option<InterfaceRfDecimal>>,)+
        }

        impl InterfaceNetworkingPatch {
            pub fn apply(&self, current: &InterfaceNetworking) -> InterfaceNetworking {
                InterfaceNetworking {
                    $($field: merged(&self.$field, &current.$field),)+
                    $($rf_field: merged(&self.$rf_field, &current.$rf_field),)+
                }
            }
        }
    };
}

networking_attributes! {
    vlan_translation_policy_id: uuid::Uuid,
    #[schema(minimum = 1, maximum = 65536)]
    mtu: i32,
    mode: InterfaceMode,
    /// Decimal Kbps string preserves the full PostgreSQL positive bigint range.
    #[schema(pattern = "^(0|[1-9][0-9]{0,18})$", max_length = 19)]
    speed: String,
    duplex: InterfaceDuplex,
    poe_mode: InterfacePoeMode,
    poe_type: InterfacePoeType,
    rf_role: InterfaceRfRole,
    #[schema(max_length = 50)]
    rf_channel: String,
    #[schema(minimum = -40, maximum = 127)]
    tx_power: i16,
    #[schema(pattern = "^[0-9a-fA-F]{2}(:[0-9a-fA-F]{2}){7}$")]
    wwn: String,
    #[schema(minimum = 1, maximum = 1024)]
    channel_id: i16,
    #[schema(minimum = 1, maximum = 1024)]
    channels: i16,
    #[schema(minimum = 1, maximum = 256)]
    cable_connector: i16,
    cable_end: InterfaceCableEnd,
    cable_positions: Vec<i16>,
    ;
    #[schema(value_type = Option<f64>, minimum = -99999.999, maximum = 99999.999, multiple_of = 0.001)]
    rf_channel_frequency => ("optional_rf_frequency", "patch_rf_frequency"),
    #[schema(value_type = Option<f64>, minimum = -9999.999, maximum = 9999.999, multiple_of = 0.001)]
    rf_channel_width => ("optional_rf_width", "patch_rf_width"),
}

impl InterfaceNetworking {
    pub fn validate(&self) -> Result<(), AppError> {
        let invalid = |field: &str| AppError::Validation(format!("Invalid interface {field}"));
        for (field, value, min, max) in [
            ("mtu", self.mtu.map(i64::from), 1, 65536),
            ("tx_power", self.tx_power.map(i64::from), -40, 127),
            ("channel_id", self.channel_id.map(i64::from), 1, 1024),
            ("channels", self.channels.map(i64::from), 1, 1024),
            (
                "cable_connector",
                self.cable_connector.map(i64::from),
                1,
                256,
            ),
        ] {
            if value.is_some_and(|v| !(min..=max).contains(&v)) {
                return Err(invalid(field));
            }
        }
        if self.speed.as_ref().is_some_and(|v| {
            v.parse::<i64>()
                .map_or(true, |n| n < 0 || n.to_string() != *v)
        }) {
            return Err(invalid("speed"));
        }
        if self
            .rf_channel
            .as_deref()
            .is_some_and(|v| !super::interface_channels::RF_CHANNELS.contains(&v))
        {
            return Err(invalid("rf_channel"));
        }
        for (measurement, value) in [
            (
                InterfaceRfMeasurement::Frequency,
                self.rf_channel_frequency.map(|v| v.0),
            ),
            (
                InterfaceRfMeasurement::Width,
                self.rf_channel_width.map(|v| v.0),
            ),
        ] {
            if let Some(value) = value {
                measurement.validate_float(value)?;
            }
        }
        if self
            .cable_positions
            .as_ref()
            .is_some_and(|positions| positions.iter().any(|p| !(1..=1024).contains(p)))
        {
            return Err(invalid("cable_positions"));
        }
        if self.wwn.as_ref().is_some_and(|v| {
            v.len() != 23
                || !v
                    .split(':')
                    .all(|octet| octet.len() == 2 && octet.bytes().all(|b| b.is_ascii_hexdigit()))
        }) {
            return Err(invalid("wwn"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{InterfaceNetworking, InterfaceNetworkingPatch, InterfaceRfDecimal};

    #[test]
    fn networking_choice_schemas_match_serde_wire_values() {
        super::InterfaceMode::assert_wire_schema();
        super::InterfaceDuplex::assert_wire_schema();
        super::InterfacePoeMode::assert_wire_schema();
        super::InterfacePoeType::assert_wire_schema();
        super::InterfaceRfRole::assert_wire_schema();
        super::InterfaceCableEnd::assert_wire_schema();
    }

    #[test]
    fn networking_rf_schemas_describe_wire_numbers_and_validation() {
        let schemas = [
            <InterfaceNetworking as utoipa::PartialSchema>::schema(),
            <InterfaceNetworkingPatch as utoipa::PartialSchema>::schema(),
        ];
        for schema in schemas {
            let schema = serde_json::to_value(schema).expect("networking schema");
            for (field, bound) in [
                ("rf_channel_frequency", 99999.999),
                ("rf_channel_width", 9999.999),
            ] {
                let property = &schema["properties"][field];
                assert_eq!(property["type"], serde_json::json!(["number", "null"]));
                assert_eq!(property["minimum"], -bound);
                assert_eq!(property["maximum"], bound);
                assert_eq!(property["multipleOf"], 0.001);
                assert!(property.get("$ref").is_none());
                assert!(
                    schema["required"]
                        .as_array()
                        .is_none_or(|required| { !required.iter().any(|name| name == field) })
                );
                for value in [-bound, 0.0, bound] {
                    let payload = serde_json::json!({field: value});
                    let attrs: InterfaceNetworking =
                        serde_json::from_value(payload.clone()).expect("numeric wire value");
                    attrs.validate().expect("schema boundary accepted");
                    assert_eq!(
                        serde_json::to_value(attrs).expect("wire response")[field],
                        value
                    );
                    let patch: InterfaceNetworkingPatch =
                        serde_json::from_value(payload).expect("numeric patch");
                    patch
                        .apply(&InterfaceNetworking::default())
                        .validate()
                        .expect("patch schema boundary accepted");
                }
                for value in [bound + 0.001, -bound - 0.001, 0.0001] {
                    let result = serde_json::from_value::<InterfaceNetworking>(
                        serde_json::json!({field: value}),
                    );
                    assert!(
                        result.is_err() || result.is_ok_and(|attrs| attrs.validate().is_err()),
                        "{field}: {value}"
                    );
                }
            }
        }
    }

    #[test]
    fn networking_flattened_rf_decimal_accepts_arbitrary_precision_json() {
        #[derive(serde::Deserialize)]
        struct Create {
            #[serde(flatten)]
            networking: InterfaceNetworking,
        }
        #[derive(serde::Deserialize)]
        struct Update {
            #[serde(flatten)]
            networking: InterfaceNetworkingPatch,
        }
        let create: Create =
            serde_json::from_str(r#"{"rf_channel_frequency":5180.125,"rf_channel_width":20.125}"#)
                .expect("native flattened create");
        create.networking.validate().expect("valid RF decimal");
        assert_eq!(
            create.networking.rf_channel_frequency,
            Some(InterfaceRfDecimal(5180.125))
        );
        let update: Update =
            serde_json::from_str(r#"{"rf_channel_frequency":2412.125,"rf_channel_width":null}"#)
                .expect("native flattened update");
        let updated = update.networking.apply(&create.networking);
        assert_eq!(
            updated.rf_channel_frequency,
            Some(InterfaceRfDecimal(2412.125))
        );
        assert_eq!(updated.rf_channel_width, None);
    }

    #[test]
    fn networking_speed_is_lossless_and_has_one_wire_representation() {
        for speed in ["0", "9007199254740993", "9223372036854775807"] {
            let json = serde_json::json!({"speed": speed});
            let attrs: InterfaceNetworking = serde_json::from_value(json).expect("decimal string");
            attrs.validate().expect("positive bigint range");
            assert_eq!(
                serde_json::to_value(&attrs).expect("response")["speed"],
                speed
            );
        }
        for speed in ["-1", "9223372036854775808", "01", "+1", "1e3"] {
            let attrs: InterfaceNetworking =
                serde_json::from_value(serde_json::json!({"speed": speed})).expect("string");
            assert!(attrs.validate().is_err(), "{speed}");
        }
        assert!(
            serde_json::from_value::<InterfaceNetworking>(serde_json::json!({"speed": 1})).is_err()
        );
    }

    #[test]
    fn networking_patch_distinguishes_omission_null_and_value() {
        let current: InterfaceNetworking = serde_json::from_value(serde_json::json!({
            "mtu": 9000, "mode": "tagged", "speed": "10000", "cable_positions": [1, 2]
        }))
        .expect("valid attributes");
        let patch: InterfaceNetworkingPatch = serde_json::from_value(serde_json::json!({
            "mtu": null, "mode": "access", "cable_positions": []
        }))
        .expect("patch");
        let updated = patch.apply(&current);
        assert_eq!(updated.mtu, None);
        assert_eq!(updated.speed.as_deref(), Some("10000"));
        assert_eq!(serde_json::to_value(&updated.mode).expect("mode"), "access");
        assert_eq!(updated.cable_positions, Some(vec![]));
        updated.validate().expect("valid patch result");
    }

    #[test]
    fn networking_rejects_non_finite_and_excess_decimal_precision() {
        for value in [f64::NAN, f64::INFINITY, 100000.0, 0.0001] {
            let attrs = InterfaceNetworking {
                rf_channel_frequency: Some(InterfaceRfDecimal(value)),
                ..Default::default()
            };
            assert!(attrs.validate().is_err(), "frequency {value}");
        }
        for value in [-99999.999, 0.0, 5180.125, 99999.999] {
            let attrs = InterfaceNetworking {
                rf_channel_frequency: Some(InterfaceRfDecimal(value)),
                ..Default::default()
            };
            attrs.validate().expect("upstream decimal boundary");
        }
    }
}
