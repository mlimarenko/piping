use super::*;

impl NetboxResourceRegistry {
    pub(super) fn build_descriptors(&mut self) {
        let paths = match self.schema.get("paths").and_then(Value::as_object) {
            Some(paths) => paths,
            None => return,
        };

        let resource_paths = self.collect_resource_paths(paths);
        self.index_response_schemas(&resource_paths);

        for (resource_key, paths_dict) in resource_paths {
            if let Some(descriptor) = self.build_descriptor(&resource_key, &paths_dict) {
                self.descriptors.insert(resource_key, descriptor);
            }
        }

        crate::observability::log_event(
            crate::observability::LogLevel::Info,
            "integration.netbox.registry.descriptors_built",
            crate::observability::EventFields::default()
                .with_operation("netbox.registry.build")
                .with_resource("integration_call", "netbox.registry")
                .with_status("ok")
                .with_context("provider", "netbox")
                .with_context("descriptor_count", self.descriptors.len().to_string()),
        );
    }

    pub(super) fn collect_resource_paths(
        &self,
        paths: &Map<String, Value>,
    ) -> HashMap<String, HashMap<String, Value>> {
        let mut resource_paths: HashMap<String, HashMap<String, Value>> = HashMap::new();

        for (path, path_item) in paths {
            if let Some(resource_key) = self.extract_resource_key(path) {
                resource_paths
                    .entry(resource_key)
                    .or_default()
                    .insert(path.to_string(), path_item.clone());
            }
        }

        resource_paths
    }

    pub(super) fn index_response_schemas(
        &mut self,
        resource_paths: &HashMap<String, HashMap<String, Value>>,
    ) {
        for (resource_key, paths_dict) in resource_paths {
            self.index_response_schema_for_resource(resource_key, paths_dict);
        }
    }

    pub(super) fn index_response_schema_for_resource(
        &mut self,
        resource_key: &str,
        paths_dict: &HashMap<String, Value>,
    ) {
        let Some(path_selection) = self.select_paths(paths_dict) else {
            return;
        };
        if !path_selection.has_detail_candidates {
            return;
        }
        let Some(detail_path_item) = paths_dict.get(&path_selection.detail_path_key) else {
            return;
        };
        let Some(response_schema) = self.extract_response_schema_name(detail_path_item.get("get"))
        else {
            return;
        };

        self.resource_response_schema_name
            .insert(resource_key.to_string(), response_schema.clone());

        let base = self.normalize_schema_base_name(&response_schema);
        if let Some(existing) = self.schema_base_name_to_resource_key.get(&base)
            && existing != resource_key
        {
            crate::observability::log_event(
                crate::observability::LogLevel::Debug,
                "integration.netbox.registry.schema_collision",
                crate::observability::EventFields::default()
                    .with_operation("netbox.registry.build")
                    .with_resource("integration_call", "netbox.registry")
                    .with_context("provider", "netbox")
                    .with_context("schema_base_name", base)
                    .with_context("existing_resource_key", existing.clone())
                    .with_context("conflicting_resource_key", resource_key.to_string()),
            );
            return;
        }

        self.schema_base_name_to_resource_key
            .insert(base, resource_key.to_string());
    }

    pub(super) fn extract_resource_key(&self, path: &str) -> Option<String> {
        let path = path.trim_end_matches('/');
        let api_marker = "/api/";
        let idx = path.find(api_marker)?;
        let path = &path[idx + api_marker.len()..];
        let parts: Vec<&str> = path.split('/').collect();
        if parts.len() < 2 {
            return None;
        }
        let resource_parts: Vec<&str> = parts.into_iter().filter(|p| !p.starts_with('{')).collect();
        if resource_parts.len() < 2 {
            return None;
        }
        Some(format!("{}.{}", resource_parts[0], resource_parts[1]))
    }

    pub(super) fn build_descriptor(
        &self,
        resource_key: &str,
        paths_dict: &HashMap<String, Value>,
    ) -> Option<NetboxResourceDescriptor> {
        let selection = self.select_paths(paths_dict)?;
        let list_path = self.strip_api_prefix(&selection.list_path_key);
        let detail_path_template = if selection.has_detail_candidates {
            self.strip_api_prefix(&selection.detail_path_key)
        } else {
            format!("{}/{{id}}/", list_path.trim_end_matches('/'))
        };
        let list_path_item = paths_dict
            .get(&selection.list_path_key)
            .and_then(Value::as_object);
        let detail_path_item = paths_dict
            .get(&selection.detail_path_key)
            .and_then(Value::as_object);
        let operations = self.collect_operation_summary(list_path_item, detail_path_item);
        let response_schema_name = self
            .resource_response_schema_name
            .get(resource_key)
            .cloned();
        let request_schema_name =
            self.resolve_request_schema_name(list_path_item, detail_path_item);

        let fields_meta = self.extract_fields_meta(
            resource_key,
            request_schema_name.as_deref(),
            response_schema_name.as_deref(),
        );
        let changelog_message_supported = self
            .schema_has_field(request_schema_name.as_deref(), "changelog_message")
            || self.schema_has_field(response_schema_name.as_deref(), "changelog_message");

        Some(NetboxResourceDescriptor {
            resource_key: resource_key.to_string(),
            list_path,
            detail_path_template,
            operations_supported: operations.operations_supported,
            filter_params: operations.filter_params,
            ordering_supported: operations.ordering_supported,
            fields_meta,
            changelog_message_supported,
        })
    }

    pub(super) fn select_paths(
        &self,
        paths_dict: &HashMap<String, Value>,
    ) -> Option<PathSelection> {
        let mut list_candidates: Vec<&String> = Vec::new();
        let mut detail_candidates: Vec<&String> = Vec::new();
        for path in paths_dict.keys() {
            if path.contains("{id}") || path.contains("{pk}") {
                detail_candidates.push(path);
            } else {
                list_candidates.push(path);
            }
        }
        let list_path_key = self.select_path_by_shape(&list_candidates)?.to_string();
        let detail_path_key = self.choose_detail_path_key(&detail_candidates, &list_path_key);
        Some(PathSelection {
            list_path_key,
            detail_path_key,
            has_detail_candidates: !detail_candidates.is_empty(),
        })
    }

    pub(super) fn select_path_by_shape<'a>(&self, candidates: &[&'a String]) -> Option<&'a String> {
        candidates
            .iter()
            .min_by_key(|p| (p.trim_matches('/').split('/').count(), p.len()))
            .copied()
    }

    pub(super) fn choose_detail_path_key(
        &self,
        detail_candidates: &[&String],
        fallback: &str,
    ) -> String {
        if detail_candidates.is_empty() {
            return fallback.to_string();
        }
        let exact: Vec<&String> = detail_candidates
            .iter()
            .copied()
            .filter(|p| {
                p.trim_end_matches('/').ends_with("{id}")
                    || p.trim_end_matches('/').ends_with("{pk}")
            })
            .collect();
        let pool = if exact.is_empty() {
            detail_candidates
        } else {
            exact.as_slice()
        };
        self.select_path_by_shape(pool)
            .map(ToString::to_string)
            .unwrap_or_else(|| fallback.to_string())
    }

    pub(super) fn collect_operation_summary(
        &self,
        list_path_item: Option<&Map<String, Value>>,
        detail_path_item: Option<&Map<String, Value>>,
    ) -> OperationSummary {
        let mut operations_supported: HashSet<String> = HashSet::new();
        let mut filter_params: HashMap<String, FilterParam> = HashMap::new();
        let mut ordering_supported = false;

        if let Some(list_item) = list_path_item {
            self.collect_list_operations(
                list_item,
                &mut operations_supported,
                &mut filter_params,
                &mut ordering_supported,
            );
        }

        if let Some(detail_item) = detail_path_item {
            self.collect_detail_operations(detail_item, &mut operations_supported);
        }

        OperationSummary {
            operations_supported,
            filter_params,
            ordering_supported,
        }
    }

    pub(super) fn collect_list_operations(
        &self,
        list_item: &Map<String, Value>,
        operations_supported: &mut HashSet<String>,
        filter_params: &mut HashMap<String, FilterParam>,
        ordering_supported: &mut bool,
    ) {
        if let Some(op) = list_item.get("get").and_then(Value::as_object) {
            operations_supported.insert("list".to_string());
            *filter_params = self.extract_filter_params(op);
            *ordering_supported = self.check_ordering_support(op);
        }

        if let Some(op) = list_item.get("post").and_then(Value::as_object) {
            let op_name = if self.operation_declares_array_request(op) {
                "bulk_create"
            } else {
                "create"
            };
            operations_supported.insert(op_name.to_string());
        }

        let has_bulk_update = ["patch", "put"]
            .into_iter()
            .filter_map(|method| list_item.get(method))
            .filter_map(Value::as_object)
            .any(|op| self.operation_declares_array_request(op));
        if has_bulk_update {
            operations_supported.insert("bulk_update".to_string());
        }
        if list_item.contains_key("delete") {
            operations_supported.insert("bulk_delete".to_string());
        }
    }

    pub(super) fn collect_detail_operations(
        &self,
        detail_item: &Map<String, Value>,
        operations_supported: &mut HashSet<String>,
    ) {
        if detail_item.contains_key("get") {
            operations_supported.insert("get".to_string());
        }
        if detail_item.contains_key("put") || detail_item.contains_key("patch") {
            operations_supported.insert("update".to_string());
        }
        if detail_item.contains_key("delete") {
            operations_supported.insert("delete".to_string());
        }
    }

    pub(super) fn operation_declares_array_request(&self, operation: &Map<String, Value>) -> bool {
        operation
            .get("requestBody")
            .and_then(Value::as_object)
            .and_then(|body| self.extract_json_schema_from_content(body.get("content")))
            .and_then(Value::as_object)
            .map(|schema| schema.get("type") == Some(&Value::String("array".to_string())))
            .unwrap_or(false)
    }

    pub(super) fn resolve_request_schema_name(
        &self,
        list_path_item: Option<&Map<String, Value>>,
        detail_path_item: Option<&Map<String, Value>>,
    ) -> Option<String> {
        let from_list = list_path_item
            .and_then(|item| item.get("post"))
            .and_then(|op| self.extract_request_schema_name(Some(op)));
        if from_list.is_some() {
            return from_list;
        }

        detail_path_item.and_then(|item| {
            ["patch", "put"]
                .into_iter()
                .find_map(|method| item.get(method))
                .and_then(|op| self.extract_request_schema_name(Some(op)))
        })
    }

    pub(super) fn strip_api_prefix(&self, path: &str) -> String {
        let normalized = path.trim_start_matches('/');
        if let Some(idx) = normalized.find("api/") {
            return normalized[idx..].to_string();
        }
        normalized.to_string()
    }

    pub(super) fn extract_filter_params(
        &self,
        operation: &Map<String, Value>,
    ) -> HashMap<String, FilterParam> {
        let mut filter_params = HashMap::new();
        let parameters = match operation.get("parameters").and_then(Value::as_array) {
            Some(params) => params,
            None => return filter_params,
        };

        for param in parameters {
            let Some(param_obj) = param.as_object() else {
                continue;
            };
            let param_name = param_obj.get("name").and_then(Value::as_str);
            let param_in = param_obj.get("in").and_then(Value::as_str);
            if let (Some(param_name), Some(param_in)) = (param_name, param_in) {
                if param_in != "query" {
                    continue;
                }
                let param_type = param_obj
                    .get("schema")
                    .and_then(Value::as_object)
                    .and_then(|schema| schema.get("type").and_then(Value::as_str))
                    .unwrap_or("string")
                    .to_string();
                let description = param_obj
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                filter_params.insert(
                    param_name.to_string(),
                    FilterParam {
                        param_type,
                        description,
                    },
                );
            }
        }

        filter_params
    }

    pub(super) fn check_ordering_support(&self, operation: &Map<String, Value>) -> bool {
        let Some(parameters) = operation.get("parameters").and_then(Value::as_array) else {
            return false;
        };
        parameters.iter().any(|param| {
            param
                .as_object()
                .and_then(|obj| {
                    let name = obj.get("name").and_then(Value::as_str);
                    let param_in = obj.get("in").and_then(Value::as_str);
                    match (name, param_in) {
                        (Some("ordering"), Some("query")) => Some(true),
                        _ => None,
                    }
                })
                .unwrap_or(false)
        })
    }
}
