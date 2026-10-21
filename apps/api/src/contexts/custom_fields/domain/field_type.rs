use serde::{Deserialize, Serialize};

/// The 13 NetBox-shaped custom-field types. The serde representations are the
/// exact NetBox wire strings and match the `custom_field_type_valid` CHECK
/// constraint in `0056_init_custom_fields.sql`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CustomFieldType {
    Text,
    Longtext,
    Integer,
    Decimal,
    Boolean,
    Date,
    Datetime,
    Url,
    Json,
    Select,
    Multiselect,
    Object,
    Multiobject,
}

impl CustomFieldType {
    /// The canonical wire/storage token (identical to the serde representation).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Longtext => "longtext",
            Self::Integer => "integer",
            Self::Decimal => "decimal",
            Self::Boolean => "boolean",
            Self::Date => "date",
            Self::Datetime => "datetime",
            Self::Url => "url",
            Self::Json => "json",
            Self::Select => "select",
            Self::Multiselect => "multiselect",
            Self::Object => "object",
            Self::Multiobject => "multiobject",
        }
    }

    /// Parse the canonical storage token; `None` for any unknown value.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "text" => Some(Self::Text),
            "longtext" => Some(Self::Longtext),
            "integer" => Some(Self::Integer),
            "decimal" => Some(Self::Decimal),
            "boolean" => Some(Self::Boolean),
            "date" => Some(Self::Date),
            "datetime" => Some(Self::Datetime),
            "url" => Some(Self::Url),
            "json" => Some(Self::Json),
            "select" => Some(Self::Select),
            "multiselect" => Some(Self::Multiselect),
            "object" => Some(Self::Object),
            "multiobject" => Some(Self::Multiobject),
            _ => None,
        }
    }

    /// `integer` / `decimal` — the types that accept numeric min/max validators.
    pub const fn is_numeric(self) -> bool {
        matches!(self, Self::Integer | Self::Decimal)
    }

    /// `select` / `multiselect` — the types backed by a choice set.
    pub const fn is_choice(self) -> bool {
        matches!(self, Self::Select | Self::Multiselect)
    }

    /// `object` / `multiobject` — the types referencing another inventory entity.
    pub const fn is_object(self) -> bool {
        matches!(self, Self::Object | Self::Multiobject)
    }

    /// Multi-valued types whose stored value is a JSON array.
    pub const fn is_multi(self) -> bool {
        matches!(self, Self::Multiselect | Self::Multiobject)
    }

    /// Types that accept a `validation_regex` (free-text-ish values).
    pub const fn allows_regex(self) -> bool {
        matches!(self, Self::Text | Self::Longtext | Self::Url)
    }

    /// Types that may be marked `is_secret`. Secret values round-trip as
    /// cleartext strings through the secrets context, so only free-text types
    /// qualify — a non-string value would be silently dropped on write.
    pub const fn allows_secret(self) -> bool {
        matches!(self, Self::Text | Self::Longtext | Self::Url)
    }

    /// Types that require (and may only carry) a `choice_set_id`.
    pub const fn allows_choice_set(self) -> bool {
        self.is_choice()
    }

    /// Types that require (and may only carry) a `related_object_type_id`.
    pub const fn allows_related_object(self) -> bool {
        self.is_object()
    }
}

#[cfg(test)]
mod tests {
    use super::CustomFieldType;
    use serde_json::json;

    #[test]
    fn serde_uses_exact_netbox_strings() {
        assert_eq!(
            serde_json::to_value(CustomFieldType::Longtext).ok(),
            Some(json!("longtext"))
        );
        assert_eq!(
            serde_json::to_value(CustomFieldType::Multiobject).ok(),
            Some(json!("multiobject"))
        );
        let parsed: Option<CustomFieldType> = serde_json::from_value(json!("multiselect")).ok();
        assert_eq!(parsed, Some(CustomFieldType::Multiselect));
    }

    #[test]
    fn parse_round_trips_as_str() {
        for value in [
            CustomFieldType::Text,
            CustomFieldType::Longtext,
            CustomFieldType::Integer,
            CustomFieldType::Decimal,
            CustomFieldType::Boolean,
            CustomFieldType::Date,
            CustomFieldType::Datetime,
            CustomFieldType::Url,
            CustomFieldType::Json,
            CustomFieldType::Select,
            CustomFieldType::Multiselect,
            CustomFieldType::Object,
            CustomFieldType::Multiobject,
        ] {
            assert_eq!(CustomFieldType::parse(value.as_str()), Some(value));
        }
        assert_eq!(CustomFieldType::parse("unknown"), None);
    }

    #[test]
    fn numeric_predicate_matches_netbox() {
        assert!(CustomFieldType::Integer.is_numeric());
        assert!(CustomFieldType::Decimal.is_numeric());
        assert!(!CustomFieldType::Text.is_numeric());
    }

    #[test]
    fn choice_predicate_matches_netbox() {
        assert!(CustomFieldType::Select.is_choice());
        assert!(CustomFieldType::Multiselect.allows_choice_set());
        assert!(!CustomFieldType::Object.is_choice());
    }

    #[test]
    fn object_predicate_matches_netbox() {
        assert!(CustomFieldType::Object.is_object());
        assert!(CustomFieldType::Multiobject.allows_related_object());
        assert!(!CustomFieldType::Select.is_object());
    }

    #[test]
    fn multi_predicate_matches_netbox() {
        assert!(CustomFieldType::Multiselect.is_multi());
        assert!(CustomFieldType::Multiobject.is_multi());
        assert!(!CustomFieldType::Select.is_multi());
    }

    #[test]
    fn regex_predicate_matches_netbox() {
        assert!(CustomFieldType::Text.allows_regex());
        assert!(CustomFieldType::Url.allows_regex());
        assert!(!CustomFieldType::Integer.allows_regex());
    }
}
