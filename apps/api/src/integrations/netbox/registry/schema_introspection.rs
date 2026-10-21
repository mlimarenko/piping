use super::*;

impl NetboxResourceRegistry {
    pub(super) fn extract_fields_meta(
        &self,
        resource_key: &str,
        request_schema_name: Option<&str>,
        response_schema_name: Option<&str>,
    ) -> Vec<FieldMeta> {
        let schemas = self.schemas();
        let mut schema_def = None;
        let mut chosen_name = None;

        for name in [request_schema_name, response_schema_name]
            .into_iter()
            .flatten()
        {
            if let Some(schema) = schemas.and_then(|s| s.get(name))
                && schema.is_object()
            {
                schema_def = Some(schema);
                chosen_name = Some(name.to_string());
                break;
            }
        }

        if schema_def.is_none() {
            let resource_name = resource_key.split('.').next_back().unwrap_or(resource_key);
            let singular = resource_name.strip_suffix('s').unwrap_or(resource_name);
            let singular = singular.replace(['-', '_'], " ");
            let schema_name = singular
                .split_whitespace()
                .map(|part| {
                    let mut chars = part.chars();
                    match chars.next() {
                        Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                        None => "".to_string(),
                    }
                })
                .collect::<String>();
            let resource_capitalized = resource_name
                .chars()
                .next()
                .map(|first| first.to_ascii_uppercase().to_string() + &resource_name[1..])
                .unwrap_or_else(|| resource_name.to_string());
            let schema_variants = vec![
                format!("{}Request", schema_name),
                schema_name.clone(),
                format!("{}Serializer", schema_name),
                resource_capitalized.clone(),
                format!("{}Request", resource_capitalized),
                format!("{}Serializer", resource_capitalized),
            ];
            if let Some(schemas) = schemas {
                for variant in schema_variants {
                    if let Some(schema) = schemas.get(&variant)
                        && schema.is_object()
                    {
                        schema_def = Some(schema);
                        chosen_name = Some(variant);
                        break;
                    }
                }
            }
        }

        let Some(schema_def) = schema_def else {
            crate::observability::log_event(
                crate::observability::LogLevel::Debug,
                "integration.netbox.registry.schema_component.missing",
                crate::observability::EventFields::default()
                    .with_operation("netbox.registry.extract_fields_meta")
                    .with_resource("integration_call", "netbox.registry")
                    .with_context("provider", "netbox")
                    .with_context("resource_key", resource_key.to_string())
                    .with_context("request_schema", format!("{:?}", request_schema_name))
                    .with_context("response_schema", format!("{:?}", response_schema_name)),
            );
            return Vec::new();
        };

        let mut read_only_map: HashMap<String, bool> = HashMap::new();
        if let Some(response_schema_name) = response_schema_name
            && let Some(response_def) = schemas.and_then(|s| s.get(response_schema_name))
            && let Some(props) = response_def.get("properties").and_then(Value::as_object)
        {
            for (fname, fschema) in props {
                if fschema
                    .get("readOnly")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    read_only_map.insert(fname.to_string(), true);
                }
            }
        }

        let Some(properties) = schema_def.get("properties").and_then(Value::as_object) else {
            return Vec::new();
        };
        let required_fields: HashSet<String> = schema_def
            .get("required")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect::<HashSet<_>>()
            })
            .unwrap_or_default();

        let mut fields_meta = Vec::new();
        let mut field_names: HashSet<String> = HashSet::new();
        for (field_name, field_schema) in properties {
            let Some(field_schema) = field_schema.as_object() else {
                continue;
            };

            let choices = self.extract_choices(field_schema);
            let description = field_schema.get("description").and_then(Value::as_str);
            let help_text = self.clean_help_text(description, choices.as_ref());

            let field_metadata = FieldMeta {
                name: field_name.to_string(),
                field_type: self.infer_openapi_type(field_schema),
                required: required_fields.contains(field_name),
                read_only: field_schema
                    .get("readOnly")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                    || read_only_map.get(field_name).copied().unwrap_or(false),
                description: help_text.unwrap_or_default(),
                max_length: field_schema.get("maxLength").cloned(),
                minimum: field_schema.get("minimum").cloned(),
                maximum: field_schema.get("maximum").cloned(),
                pattern: field_schema
                    .get("pattern")
                    .and_then(Value::as_str)
                    .map(|s| s.to_string()),
                choices,
                ref_resource_key: self.extract_ref_resource_key(field_schema),
                default: field_schema.get("default").cloned(),
            };

            field_names.insert(field_name.to_string());
            fields_meta.push(field_metadata);
        }

        // Add read-only fields from response schema that are not in request schema
        if let Some(response_schema_name) = response_schema_name
            && let Some(response_def) = schemas.and_then(|s| s.get(response_schema_name))
            && let Some(props) = response_def.get("properties").and_then(Value::as_object)
        {
            for (fname, fschema) in props {
                if field_names.contains(fname) {
                    continue;
                }
                let Some(fschema) = fschema.as_object() else {
                    continue;
                };
                if !fschema
                    .get("readOnly")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    continue;
                }
                let choices = self.extract_choices(fschema);
                let description = fschema.get("description").and_then(Value::as_str);
                let help_text = self.clean_help_text(description, choices.as_ref());
                fields_meta.push(FieldMeta {
                    name: fname.to_string(),
                    field_type: self.infer_openapi_type(fschema),
                    required: false,
                    read_only: true,
                    description: help_text.unwrap_or_default(),
                    max_length: fschema.get("maxLength").cloned(),
                    minimum: fschema.get("minimum").cloned(),
                    maximum: fschema.get("maximum").cloned(),
                    pattern: fschema
                        .get("pattern")
                        .and_then(Value::as_str)
                        .map(|s| s.to_string()),
                    choices,
                    ref_resource_key: self.extract_ref_resource_key(fschema),
                    default: fschema.get("default").cloned(),
                });
            }
        }

        crate::observability::log_event(
            crate::observability::LogLevel::Debug,
            "integration.netbox.registry.fields_extracted",
            crate::observability::EventFields::default()
                .with_operation("netbox.registry.extract_fields_meta")
                .with_resource("integration_call", "netbox.registry")
                .with_status("ok")
                .with_context("provider", "netbox")
                .with_context("resource_key", resource_key.to_string())
                .with_context("field_count", fields_meta.len().to_string())
                .with_context("schema_name", format!("{:?}", chosen_name)),
        );
        fields_meta
    }

    pub(super) fn schema_has_field(&self, schema_name: Option<&str>, field_name: &str) -> bool {
        let Some(schema_name) = schema_name else {
            return false;
        };
        let Some(schemas) = self.schemas() else {
            return false;
        };
        let Some(schema_def) = schemas.get(schema_name) else {
            return false;
        };
        let Some(properties) = schema_def.get("properties").and_then(Value::as_object) else {
            return false;
        };
        properties.contains_key(field_name)
    }

    pub(super) fn schemas(&self) -> Option<&Map<String, Value>> {
        self.schema
            .get("components")
            .and_then(Value::as_object)
            .and_then(|components| components.get("schemas"))
            .and_then(Value::as_object)
    }

    pub(super) fn schema_ref_to_name(&self, reference: &str) -> Option<String> {
        const PREFIX: &str = "#/components/schemas/";
        if reference.starts_with(PREFIX) {
            return Some(reference.trim_start_matches(PREFIX).to_string());
        }
        if reference.contains("/schemas/") {
            return reference.split('/').next_back().map(ToString::to_string);
        }
        None
    }

    pub(super) fn iter_schema_ref_names(&self, node: &Value) -> Vec<String> {
        let mut names = Vec::new();
        fn walk(value: &Value, names: &mut Vec<String>, registry: &NetboxResourceRegistry) {
            match value {
                Value::Object(map) => {
                    if let Some(Value::String(reference)) = map.get("$ref")
                        && let Some(name) = registry.schema_ref_to_name(reference)
                    {
                        names.push(name);
                    }
                    for child in map.values() {
                        walk(child, names, registry);
                    }
                }
                Value::Array(list) => {
                    for child in list {
                        walk(child, names, registry);
                    }
                }
                _ => {}
            }
        }
        walk(node, &mut names, self);
        names
    }

    pub(super) fn normalize_schema_base_name(&self, name: &str) -> String {
        let mut base = name.to_string();
        let prefixes = ["Brief", "Nested", "Patched"];
        let suffixes = ["Request", "Serializer"];
        let mut changed = true;
        while changed {
            changed = false;
            for prefix in prefixes {
                if base.starts_with(prefix) && base.len() > prefix.len() {
                    base = base[prefix.len()..].to_string();
                    changed = true;
                }
            }
            for suffix in suffixes {
                if base.ends_with(suffix) && base.len() > suffix.len() {
                    base = base[..base.len() - suffix.len()].to_string();
                    changed = true;
                }
            }
        }
        base
    }

    pub(super) fn map_ref_schema_name_to_resource_key(
        &self,
        ref_schema_name: &str,
    ) -> Option<String> {
        let base = self.normalize_schema_base_name(ref_schema_name);
        self.schema_base_name_to_resource_key.get(&base).cloned()
    }

    pub(super) fn extract_json_schema_from_content<'a>(
        &self,
        content: Option<&'a Value>,
    ) -> Option<&'a Value> {
        let content = content?.as_object()?;
        let json_content = content.get("application/json")?.as_object()?;
        json_content.get("schema")
    }

    pub(super) fn extract_response_schema_name(&self, operation: Option<&Value>) -> Option<String> {
        let operation = operation?.as_object()?;
        let responses = operation.get("responses")?.as_object()?;

        for status in ["200", "201"] {
            if let Some(resp) = responses.get(status).and_then(Value::as_object)
                && let Some(schema) = self.extract_json_schema_from_content(resp.get("content"))
            {
                let refs = self.iter_schema_ref_names(schema);
                if let Some(first) = refs.first() {
                    return Some(first.to_string());
                }
            }
        }

        for resp in responses.values() {
            if let Some(resp) = resp.as_object()
                && let Some(schema) = self.extract_json_schema_from_content(resp.get("content"))
            {
                let refs = self.iter_schema_ref_names(schema);
                if let Some(first) = refs.first() {
                    return Some(first.to_string());
                }
            }
        }
        None
    }

    pub(super) fn extract_request_schema_name(&self, operation: Option<&Value>) -> Option<String> {
        let operation = operation?.as_object()?;
        let request_body = operation.get("requestBody")?.as_object()?;
        let schema = self.extract_json_schema_from_content(request_body.get("content"))?;
        let refs = self.iter_schema_ref_names(schema);
        refs.first().cloned()
    }

    pub(super) fn infer_openapi_type(&self, schema: &Map<String, Value>) -> String {
        if let Some(Value::String(t)) = schema.get("type") {
            return t.to_string();
        }
        if schema.get("items").and_then(Value::as_object).is_some() {
            return "array".to_string();
        }
        for key in ["oneOf", "anyOf", "allOf"] {
            if let Some(Value::Array(variants)) = schema.get(key) {
                let mut inferred = Vec::new();
                for variant in variants {
                    if let Some(variant) = variant.as_object() {
                        inferred.push(self.infer_openapi_type(variant));
                    }
                }
                for preferred in ["integer", "number", "string", "boolean", "array", "object"] {
                    if inferred.iter().any(|t| t == preferred) {
                        return preferred.to_string();
                    }
                }
            }
        }
        if schema.contains_key("$ref") {
            return "object".to_string();
        }
        "string".to_string()
    }

    pub(super) fn enum_desc_line_re() -> &'static Regex {
        static RE: OnceLock<Regex> = OnceLock::new();
        RE.get_or_init(|| {
            // Compile-time-constant pattern; a parse failure is a programmer
            // error caught deterministically at first use, not a runtime condition.
            #[expect(clippy::expect_used, reason = "compile-time-constant pattern; a parse failure would be a programmer error")]
            Regex::new(r"^\s*\*\s*`(?P<value>[^`]+)`\s*-\s*(?P<label>.+?)\s*$")
                .expect("enum description regex should compile")
        })
    }

    pub(super) fn parse_enum_description_labels(
        &self,
        description: &str,
    ) -> HashMap<String, String> {
        let mut mapping = HashMap::new();
        for line in description.lines() {
            if let Some(caps) = Self::enum_desc_line_re().captures(line) {
                let value = caps.name("value").map(|m| m.as_str()).unwrap_or("").trim();
                let label = caps.name("label").map(|m| m.as_str()).unwrap_or("").trim();
                if !value.is_empty() && !label.is_empty() {
                    mapping.insert(value.to_string(), label.to_string());
                }
            }
        }
        mapping
    }

    pub(super) fn extract_choices(&self, schema: &Map<String, Value>) -> Option<Vec<FieldChoice>> {
        if let Some(extracted) = self.extract_choices_from_enum_values(schema) {
            return Some(extracted);
        }
        if let Some(extracted) = self.extract_choices_from_value_label_object(schema) {
            return Some(extracted);
        }
        self.extract_choices_from_variants(schema)
    }

    pub(super) fn extract_choices_from_enum_values(
        &self,
        schema: &Map<String, Value>,
    ) -> Option<Vec<FieldChoice>> {
        let enum_values = schema.get("enum").and_then(Value::as_array)?;
        let labels = schema
            .get("description")
            .and_then(Value::as_str)
            .map(|desc| self.parse_enum_description_labels(desc))
            .unwrap_or_default();

        let mut out = Vec::new();
        for value in enum_values {
            if !matches!(value, Value::String(_) | Value::Number(_) | Value::Bool(_)) {
                continue;
            }
            let label = value
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|| value.to_string());
            out.push(FieldChoice {
                value: value.clone(),
                label: labels.get(&label).cloned().unwrap_or(label),
            });
        }

        if out.is_empty() {
            return None;
        }
        Some(out)
    }

    pub(super) fn extract_choices_from_value_label_object(
        &self,
        schema: &Map<String, Value>,
    ) -> Option<Vec<FieldChoice>> {
        if schema.get("type") != Some(&Value::String("object".to_string())) {
            return None;
        }

        let props = schema.get("properties").and_then(Value::as_object)?;
        let value_schema = props.get("value").and_then(Value::as_object)?;
        let label_schema = props.get("label").and_then(Value::as_object)?;
        let value_enum = value_schema.get("enum").and_then(Value::as_array)?;
        let label_enum = label_schema.get("enum").and_then(Value::as_array)?;
        if value_enum.len() != label_enum.len() {
            return None;
        }

        let mut paired = Vec::new();
        for (value, label) in value_enum.iter().zip(label_enum.iter()) {
            if !matches!(value, Value::String(_) | Value::Number(_) | Value::Bool(_)) {
                continue;
            }
            let label_str = label
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|| value.to_string());
            paired.push(FieldChoice {
                value: value.clone(),
                label: label_str,
            });
        }

        if paired.is_empty() {
            return None;
        }
        Some(paired)
    }

    pub(super) fn extract_choices_from_variants(
        &self,
        schema: &Map<String, Value>,
    ) -> Option<Vec<FieldChoice>> {
        for key in ["oneOf", "anyOf", "allOf"] {
            let Some(Value::Array(variants)) = schema.get(key) else {
                continue;
            };
            for variant in variants {
                let Some(variant) = variant.as_object() else {
                    continue;
                };
                if let Some(extracted) = self.extract_choices(variant) {
                    return Some(extracted);
                }
            }
        }
        None
    }

    pub(super) fn clean_help_text(
        &self,
        description: Option<&str>,
        choices: Option<&Vec<FieldChoice>>,
    ) -> Option<String> {
        let description = description?;
        let Some(choices) = choices else {
            return Some(description.to_string());
        };
        if choices.is_empty() {
            return Some(description.to_string());
        }
        let all_enum_lines = description
            .lines()
            .filter(|line| !line.trim().is_empty())
            .all(|line| Self::enum_desc_line_re().is_match(line));
        if all_enum_lines {
            return None;
        }
        Some(description.to_string())
    }

    pub(super) fn extract_ref_resource_key(&self, schema: &Map<String, Value>) -> Option<String> {
        let refs = self.iter_schema_ref_names(&Value::Object(schema.clone()));
        for ref_name in refs {
            if let Some(mapped) = self.map_ref_schema_name_to_resource_key(&ref_name) {
                return Some(mapped);
            }
        }
        None
    }
}
