//! Pure, IO-free validation for custom fields — mirrors NetBox's
//! `CustomField.clean()` (config invariants) and `CustomField.validate(value)`
//! (per-value checks). No database, no choice-set hydration: the value checks
//! that need choice membership take the resolved choice values as an argument.

use regex::Regex;
use serde_json::Value;
use uuid::Uuid;

use super::definition::CustomFieldDefinition;
use super::error::CustomFieldError;
use super::field_type::CustomFieldType;

/// Canonical custom-field key/identifier check, shared by every site that needs
/// to validate a custom-field name (field definition names, connection target
/// variable idents, config-context keys). Pattern: a leading lowercase ASCII
/// letter followed by up to 49 lowercase letters/digits/underscores
/// (`^[a-z][a-z0-9_]{0,49}$`).
///
/// This is the single source of truth; `connection_params_schema.rs` and
/// `config_context_repository.rs` delegate here instead of duplicating the rule.
#[must_use]
pub fn is_valid_custom_field_key(key: &str) -> bool {
    if key.is_empty() || key.len() > 50 {
        return false;
    }
    let mut chars = key.chars();
    match chars.next() {
        Some(first) if first.is_ascii_lowercase() => {}
        _ => return false,
    }
    chars.all(|character| {
        character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
    })
}

/// Validate the static configuration of a definition (NetBox `clean()`):
/// choice-set / related-object presence per type, validator gating, and
/// default-value sanity. Pure — does not touch the database.
///
/// # Errors
/// Returns [`CustomFieldError`] describing the first invariant the definition
/// violates.
pub fn validate_definition(def: &CustomFieldDefinition) -> Result<(), CustomFieldError> {
    if !is_valid_custom_field_key(&def.name) {
        return Err(CustomFieldError::for_field(
            def.name.clone(),
            "name must match ^[a-z][a-z0-9_]{0,49}$",
        ));
    }

    let field_type = def.field_type;

    // choice_set_id required iff select/multiselect.
    if field_type.allows_choice_set() {
        if def.choice_set_id.is_none() {
            return Err(CustomFieldError::for_field(
                def.name.clone(),
                "choice_set_id is required for select/multiselect fields",
            ));
        }
    } else if def.choice_set_id.is_some() {
        return Err(CustomFieldError::for_field(
            def.name.clone(),
            "choice_set_id is only allowed for select/multiselect fields",
        ));
    }

    // related_object_type_id required iff object/multiobject.
    if field_type.allows_related_object() {
        if def.related_object_type_id.is_none() {
            return Err(CustomFieldError::for_field(
                def.name.clone(),
                "related_object_type_id is required for object/multiobject fields",
            ));
        }
    } else if def.related_object_type_id.is_some() {
        return Err(CustomFieldError::for_field(
            def.name.clone(),
            "related_object_type_id is only allowed for object/multiobject fields",
        ));
    }

    // related_object_filter only on object/multiobject.
    if def.related_object_filter.is_some() && !field_type.allows_related_object() {
        return Err(CustomFieldError::for_field(
            def.name.clone(),
            "related_object_filter is only allowed for object/multiobject fields",
        ));
    }

    // Numeric validators only on integer/decimal.
    if (def.validation_minimum.is_some() || def.validation_maximum.is_some())
        && !field_type.is_numeric()
    {
        return Err(CustomFieldError::for_field(
            def.name.clone(),
            "validation_minimum/maximum are only allowed for integer/decimal fields",
        ));
    }
    if let (Some(min), Some(max)) = (def.validation_minimum, def.validation_maximum)
        && min > max
    {
        return Err(CustomFieldError::for_field(
            def.name.clone(),
            "validation_minimum must not exceed validation_maximum",
        ));
    }

    // Regex only on text/longtext/url.
    if !def.validation_regex.is_empty() {
        if !field_type.allows_regex() {
            return Err(CustomFieldError::for_field(
                def.name.clone(),
                "validation_regex is only allowed for text/longtext/url fields",
            ));
        }
        Regex::new(&def.validation_regex).map_err(|error| {
            CustomFieldError::for_field(
                def.name.clone(),
                format!("invalid validation_regex: {error}"),
            )
        })?;
    }

    // validation_schema only on json.
    if def.validation_schema.is_some() && field_type != CustomFieldType::Json {
        return Err(CustomFieldError::for_field(
            def.name.clone(),
            "validation_schema is only allowed for json fields",
        ));
    }

    // enforce_unique is part of the definition surface, but value-level
    // uniqueness is not yet enforced on the write path. Reject it at definition
    // time rather than advertise a guarantee we do not keep — re-enable once the
    // per-content-type uniqueness check on writes lands. (It is also nonsensical
    // for boolean.)
    if def.enforce_unique {
        return Err(CustomFieldError::for_field(
            def.name.clone(),
            "enforce_unique is not yet supported",
        ));
    }

    // is_secret only on free-text types. Secret values are stored as cleartext
    // strings in the secrets context; a non-string value would be silently
    // dropped on write, so the whole class is forbidden here at definition time.
    if def.is_secret && !field_type.allows_secret() {
        return Err(CustomFieldError::for_field(
            def.name.clone(),
            "is_secret is only allowed for text/longtext/url fields",
        ));
    }

    // A select default must be a member of the choice set; checked here only for
    // the explicit extra-choice list is unavailable in this pure layer, so a
    // present default is type-checked against the field's value rules instead.
    if let Some(default) = &def.default_value
        && !default.is_null()
    {
        validate_default_shape(def, default)?;
    }

    Ok(())
}

/// Type-check a default value's shape without choice-set hydration. Membership
/// of select defaults against the choice set is enforced by the service (which
/// has the hydrated choices) via [`validate_value`].
fn validate_default_shape(
    def: &CustomFieldDefinition,
    default: &Value,
) -> Result<(), CustomFieldError> {
    match def.field_type {
        CustomFieldType::Select | CustomFieldType::Multiselect => {
            // Defaults for choice fields are strings / arrays of strings;
            // membership is verified by the service with the resolved choices.
            if def.field_type == CustomFieldType::Select {
                if !default.is_string() {
                    return Err(CustomFieldError::for_field(
                        def.name.clone(),
                        "default for a select field must be a string",
                    ));
                }
            } else if !is_string_array(default) {
                return Err(CustomFieldError::for_field(
                    def.name.clone(),
                    "default for a multiselect field must be an array of strings",
                ));
            }
            Ok(())
        }
        _ => check_typed_value(def, default, &[]),
    }
}

/// Validate a concrete value against a definition (NetBox `validate(value)`).
///
/// `choice_values` is the set of permitted choice strings (the resolved choice
/// set's values plus any extra choices). It is only consulted for
/// select/multiselect fields; pass an empty slice for other types.
///
/// # Errors
/// Returns [`CustomFieldError`] when the value is missing-but-required or fails
/// a type / range / membership / regex check.
pub fn validate_value(
    def: &CustomFieldDefinition,
    value: &Value,
    choice_values: &[String],
) -> Result<(), CustomFieldError> {
    let is_empty = value_is_empty(value);

    if is_empty {
        if def.required {
            return Err(CustomFieldError::for_field(
                def.name.clone(),
                "value is required",
            ));
        }
        return Ok(());
    }

    check_typed_value(def, value, choice_values)
}

fn check_typed_value(
    def: &CustomFieldDefinition,
    value: &Value,
    choice_values: &[String],
) -> Result<(), CustomFieldError> {
    match def.field_type {
        CustomFieldType::Text | CustomFieldType::Longtext => {
            let text = value.as_str().ok_or_else(|| {
                CustomFieldError::for_field(def.name.clone(), "value must be a string")
            })?;
            check_regex(def, text)
        }
        CustomFieldType::Url => {
            let text = value.as_str().ok_or_else(|| {
                CustomFieldError::for_field(def.name.clone(), "value must be a URL string")
            })?;
            check_url(def, text)?;
            check_regex(def, text)
        }
        CustomFieldType::Integer => {
            let number = value.as_i64().map(|n| n as f64).ok_or_else(|| {
                CustomFieldError::for_field(def.name.clone(), "value must be an integer")
            })?;
            check_numeric_bounds(def, number)
        }
        CustomFieldType::Decimal => {
            let number = value.as_f64().ok_or_else(|| {
                CustomFieldError::for_field(def.name.clone(), "value must be a number")
            })?;
            check_numeric_bounds(def, number)
        }
        CustomFieldType::Boolean => {
            if value.is_boolean() {
                Ok(())
            } else {
                Err(CustomFieldError::for_field(
                    def.name.clone(),
                    "value must be a boolean",
                ))
            }
        }
        CustomFieldType::Date => check_date(def, value, false),
        CustomFieldType::Datetime => check_date(def, value, true),
        CustomFieldType::Json => Ok(()),
        CustomFieldType::Select => {
            let selected = value.as_str().ok_or_else(|| {
                CustomFieldError::for_field(def.name.clone(), "value must be a string")
            })?;
            check_choice_member(def, selected, choice_values)
        }
        CustomFieldType::Multiselect => {
            let items = value.as_array().ok_or_else(|| {
                CustomFieldError::for_field(def.name.clone(), "value must be an array")
            })?;
            for item in items {
                let selected = item.as_str().ok_or_else(|| {
                    CustomFieldError::for_field(
                        def.name.clone(),
                        "multiselect items must be strings",
                    )
                })?;
                check_choice_member(def, selected, choice_values)?;
            }
            Ok(())
        }
        CustomFieldType::Object => parse_uuid_value(value)
            .map(|_| ())
            .ok_or_else(|| CustomFieldError::for_field(def.name.clone(), "value must be a UUID")),
        CustomFieldType::Multiobject => {
            let items = value.as_array().ok_or_else(|| {
                CustomFieldError::for_field(def.name.clone(), "value must be an array of UUIDs")
            })?;
            for item in items {
                if parse_uuid_value(item).is_none() {
                    return Err(CustomFieldError::for_field(
                        def.name.clone(),
                        "multiobject items must be UUIDs",
                    ));
                }
            }
            Ok(())
        }
    }
}

fn check_regex(def: &CustomFieldDefinition, text: &str) -> Result<(), CustomFieldError> {
    if def.validation_regex.is_empty() {
        return Ok(());
    }
    let regex = Regex::new(&def.validation_regex).map_err(|error| {
        CustomFieldError::for_field(
            def.name.clone(),
            format!("invalid validation_regex: {error}"),
        )
    })?;
    if regex.is_match(text) {
        Ok(())
    } else {
        Err(CustomFieldError::for_field(
            def.name.clone(),
            "value does not match the required pattern",
        ))
    }
}

fn check_url(def: &CustomFieldDefinition, text: &str) -> Result<(), CustomFieldError> {
    if text.starts_with("http://") || text.starts_with("https://") {
        Ok(())
    } else {
        Err(CustomFieldError::for_field(
            def.name.clone(),
            "value must be an http(s) URL",
        ))
    }
}

fn check_numeric_bounds(def: &CustomFieldDefinition, number: f64) -> Result<(), CustomFieldError> {
    if let Some(min) = def.validation_minimum
        && number < min
    {
        return Err(CustomFieldError::for_field(
            def.name.clone(),
            format!("value must be at least {min}"),
        ));
    }
    if let Some(max) = def.validation_maximum
        && number > max
    {
        return Err(CustomFieldError::for_field(
            def.name.clone(),
            format!("value must be at most {max}"),
        ));
    }
    Ok(())
}

fn check_date(
    def: &CustomFieldDefinition,
    value: &Value,
    with_time: bool,
) -> Result<(), CustomFieldError> {
    let text = value.as_str().ok_or_else(|| {
        CustomFieldError::for_field(def.name.clone(), "value must be an ISO date string")
    })?;
    let parsed = if with_time {
        chrono::DateTime::parse_from_rfc3339(text).is_ok()
            || chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S").is_ok()
    } else {
        chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d").is_ok()
    };
    if parsed {
        Ok(())
    } else if with_time {
        Err(CustomFieldError::for_field(
            def.name.clone(),
            "value must be an ISO 8601 datetime",
        ))
    } else {
        Err(CustomFieldError::for_field(
            def.name.clone(),
            "value must be an ISO 8601 date (YYYY-MM-DD)",
        ))
    }
}

fn check_choice_member(
    def: &CustomFieldDefinition,
    selected: &str,
    choice_values: &[String],
) -> Result<(), CustomFieldError> {
    if choice_values.iter().any(|choice| choice == selected) {
        Ok(())
    } else {
        Err(CustomFieldError::for_field(
            def.name.clone(),
            format!("'{selected}' is not a valid choice"),
        ))
    }
}

fn parse_uuid_value(value: &Value) -> Option<Uuid> {
    value.as_str().and_then(|text| Uuid::parse_str(text).ok())
}

fn is_string_array(value: &Value) -> bool {
    value
        .as_array()
        .is_some_and(|items| items.iter().all(Value::is_string))
}

/// NetBox treats `null`, empty string, and empty array as "no value".
fn value_is_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(text) => text.is_empty(),
        Value::Array(items) => items.is_empty(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contexts::custom_fields::domain::definition::{FilterLogic, UiEditable, UiVisible};
    use chrono::Utc;
    use serde_json::json;

    fn base_def(field_type: CustomFieldType) -> CustomFieldDefinition {
        CustomFieldDefinition {
            id: Uuid::nil(),
            name: "test_field".to_string(),
            label: "Test".to_string(),
            field_type,
            description: String::new(),
            group_name: String::new(),
            required: false,
            enforce_unique: false,
            is_secret: false,
            default_value: None,
            weight: 100,
            validation_minimum: None,
            validation_maximum: None,
            validation_regex: String::new(),
            validation_schema: None,
            choice_set_id: None,
            related_object_type_id: None,
            related_object_filter: None,
            ui_visible: UiVisible::Always,
            ui_editable: UiEditable::Yes,
            is_cloneable: false,
            search_weight: 1000,
            filter_logic: FilterLogic::Loose,
            object_types: Vec::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn key_validator_matches_pattern() {
        assert!(is_valid_custom_field_key("a"));
        assert!(is_valid_custom_field_key("field_1"));
        assert!(is_valid_custom_field_key("field_name_2"));
        assert!(!is_valid_custom_field_key(""));
        assert!(!is_valid_custom_field_key("Field_Name_2"));
        assert!(!is_valid_custom_field_key("1field"));
        assert!(!is_valid_custom_field_key("_field"));
        assert!(!is_valid_custom_field_key("has space"));
        assert!(!is_valid_custom_field_key("has-dash"));
        assert!(!is_valid_custom_field_key(&"x".repeat(51)));
        assert!(is_valid_custom_field_key(&format!("a{}", "x".repeat(49))));
    }

    #[test]
    fn required_rejects_empty_and_accepts_value() {
        let mut def = base_def(CustomFieldType::Text);
        def.required = true;
        assert!(validate_value(&def, &Value::Null, &[]).is_err());
        assert!(validate_value(&def, &json!(""), &[]).is_err());
        assert!(validate_value(&def, &json!("present"), &[]).is_ok());
    }

    #[test]
    fn optional_empty_is_ok() {
        let def = base_def(CustomFieldType::Text);
        assert!(validate_value(&def, &Value::Null, &[]).is_ok());
        assert!(validate_value(&def, &json!(""), &[]).is_ok());
    }

    #[test]
    fn integer_bounds_enforced() {
        let mut def = base_def(CustomFieldType::Integer);
        def.validation_minimum = Some(1.0);
        def.validation_maximum = Some(10.0);
        assert!(validate_value(&def, &json!(5), &[]).is_ok());
        assert!(validate_value(&def, &json!(0), &[]).is_err());
        assert!(validate_value(&def, &json!(11), &[]).is_err());
        assert!(validate_value(&def, &json!("5"), &[]).is_err());
        assert!(validate_value(&def, &json!(5.5), &[]).is_err());
    }

    #[test]
    fn decimal_accepts_fractions() {
        let def = base_def(CustomFieldType::Decimal);
        assert!(validate_value(&def, &json!(5.5), &[]).is_ok());
        assert!(validate_value(&def, &json!(5), &[]).is_ok());
        assert!(validate_value(&def, &json!("nope"), &[]).is_err());
    }

    #[test]
    fn boolean_type_checked() {
        let def = base_def(CustomFieldType::Boolean);
        assert!(validate_value(&def, &json!(true), &[]).is_ok());
        assert!(validate_value(&def, &json!("true"), &[]).is_err());
    }

    #[test]
    fn date_and_datetime_iso_checked() {
        let date = base_def(CustomFieldType::Date);
        assert!(validate_value(&date, &json!("2026-01-31"), &[]).is_ok());
        assert!(validate_value(&date, &json!("2026-13-01"), &[]).is_err());
        assert!(validate_value(&date, &json!("not-a-date"), &[]).is_err());

        let dt = base_def(CustomFieldType::Datetime);
        assert!(validate_value(&dt, &json!("2026-01-31T12:30:00Z"), &[]).is_ok());
        assert!(validate_value(&dt, &json!("2026-01-31T12:30:00"), &[]).is_ok());
        assert!(validate_value(&dt, &json!("2026-01-31"), &[]).is_err());
    }

    #[test]
    fn url_requires_scheme_and_regex() {
        let mut def = base_def(CustomFieldType::Url);
        assert!(validate_value(&def, &json!("https://example.com"), &[]).is_ok());
        assert!(validate_value(&def, &json!("ftp://example.com"), &[]).is_err());
        def.validation_regex = r"^https://".to_string();
        assert!(validate_value(&def, &json!("http://example.com"), &[]).is_err());
        assert!(validate_value(&def, &json!("https://example.com"), &[]).is_ok());
    }

    #[test]
    fn text_regex_enforced() {
        let mut def = base_def(CustomFieldType::Text);
        def.validation_regex = r"^[A-Z]{3}$".to_string();
        assert!(validate_value(&def, &json!("ABC"), &[]).is_ok());
        assert!(validate_value(&def, &json!("abc"), &[]).is_err());
    }

    #[test]
    fn select_membership_enforced() {
        let mut def = base_def(CustomFieldType::Select);
        def.choice_set_id = Some(Uuid::now_v7());
        let choices = vec!["red".to_string(), "green".to_string()];
        assert!(validate_value(&def, &json!("red"), &choices).is_ok());
        assert!(validate_value(&def, &json!("blue"), &choices).is_err());
    }

    #[test]
    fn multiselect_subset_enforced() {
        let mut def = base_def(CustomFieldType::Multiselect);
        def.choice_set_id = Some(Uuid::now_v7());
        let choices = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        assert!(validate_value(&def, &json!(["a", "c"]), &choices).is_ok());
        assert!(validate_value(&def, &json!(["a", "z"]), &choices).is_err());
        assert!(validate_value(&def, &json!("a"), &choices).is_err());
    }

    #[test]
    fn object_and_multiobject_uuid_checked() {
        let mut obj = base_def(CustomFieldType::Object);
        obj.related_object_type_id = Some(Uuid::now_v7());
        let id = Uuid::now_v7();
        assert!(validate_value(&obj, &json!(id.to_string()), &[]).is_ok());
        assert!(validate_value(&obj, &json!("not-a-uuid"), &[]).is_err());

        let mut multi = base_def(CustomFieldType::Multiobject);
        multi.related_object_type_id = Some(Uuid::now_v7());
        assert!(
            validate_value(
                &multi,
                &json!([id.to_string(), Uuid::now_v7().to_string()]),
                &[]
            )
            .is_ok()
        );
        assert!(validate_value(&multi, &json!([id.to_string(), "bad"]), &[]).is_err());
        assert!(validate_value(&multi, &json!(id.to_string()), &[]).is_err());
    }

    #[test]
    fn definition_choice_set_invariant() {
        let mut def = base_def(CustomFieldType::Select);
        assert!(validate_definition(&def).is_err());
        def.choice_set_id = Some(Uuid::now_v7());
        assert!(validate_definition(&def).is_ok());

        let mut text = base_def(CustomFieldType::Text);
        text.choice_set_id = Some(Uuid::now_v7());
        assert!(validate_definition(&text).is_err());
    }

    #[test]
    fn definition_related_object_invariant() {
        let mut def = base_def(CustomFieldType::Object);
        assert!(validate_definition(&def).is_err());
        def.related_object_type_id = Some(Uuid::now_v7());
        assert!(validate_definition(&def).is_ok());

        let mut text = base_def(CustomFieldType::Text);
        text.related_object_type_id = Some(Uuid::now_v7());
        assert!(validate_definition(&text).is_err());
    }

    #[test]
    fn definition_numeric_validator_gating() {
        let mut text = base_def(CustomFieldType::Text);
        text.validation_minimum = Some(1.0);
        assert!(validate_definition(&text).is_err());

        let mut int = base_def(CustomFieldType::Integer);
        int.validation_minimum = Some(10.0);
        int.validation_maximum = Some(1.0);
        assert!(validate_definition(&int).is_err());
        int.validation_maximum = Some(20.0);
        assert!(validate_definition(&int).is_ok());
    }

    #[test]
    fn definition_regex_gating() {
        let mut int = base_def(CustomFieldType::Integer);
        int.validation_regex = r"^\d+$".to_string();
        assert!(validate_definition(&int).is_err());

        let mut text = base_def(CustomFieldType::Text);
        text.validation_regex = r"^([".to_string();
        assert!(validate_definition(&text).is_err());
        text.validation_regex = r"^[A-Z]+$".to_string();
        assert!(validate_definition(&text).is_ok());
    }

    #[test]
    fn definition_schema_gating() {
        let mut text = base_def(CustomFieldType::Text);
        text.validation_schema = Some(json!({"type": "object"}));
        assert!(validate_definition(&text).is_err());

        let mut json_field = base_def(CustomFieldType::Json);
        json_field.validation_schema = Some(json!({"type": "object"}));
        assert!(validate_definition(&json_field).is_ok());
    }

    #[test]
    fn definition_enforce_unique_forbidden_on_boolean() {
        let mut def = base_def(CustomFieldType::Boolean);
        def.enforce_unique = true;
        assert!(validate_definition(&def).is_err());
    }

    #[test]
    fn definition_default_shape_checked() {
        let mut int = base_def(CustomFieldType::Integer);
        int.default_value = Some(json!(5));
        assert!(validate_definition(&int).is_ok());
        int.default_value = Some(json!("five"));
        assert!(validate_definition(&int).is_err());

        let mut select = base_def(CustomFieldType::Select);
        select.choice_set_id = Some(Uuid::now_v7());
        select.default_value = Some(json!("red"));
        assert!(validate_definition(&select).is_ok());
        select.default_value = Some(json!(["red"]));
        assert!(validate_definition(&select).is_err());
    }

    #[test]
    fn definition_rejects_bad_name() {
        let mut def = base_def(CustomFieldType::Text);
        def.name = "1bad".to_string();
        assert!(validate_definition(&def).is_err());
    }

    #[test]
    fn definition_is_secret_restricted_to_text_types() {
        // Allowed: free-text types whose value round-trips as a cleartext string.
        for field_type in [
            CustomFieldType::Text,
            CustomFieldType::Longtext,
            CustomFieldType::Url,
        ] {
            let mut def = base_def(field_type);
            def.is_secret = true;
            assert!(
                validate_definition(&def).is_ok(),
                "is_secret must be allowed for {field_type:?}"
            );
        }
        // Rejected: every non-text type, whose non-string value would be dropped.
        for field_type in [
            CustomFieldType::Integer,
            CustomFieldType::Decimal,
            CustomFieldType::Boolean,
            CustomFieldType::Date,
            CustomFieldType::Datetime,
            CustomFieldType::Json,
        ] {
            let mut def = base_def(field_type);
            def.is_secret = true;
            assert!(
                validate_definition(&def).is_err(),
                "is_secret must be rejected for {field_type:?}"
            );
        }
    }
}
