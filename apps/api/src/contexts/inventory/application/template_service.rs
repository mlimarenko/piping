//! Template service — orchestrates validation, preview, and run execution on
//! top of `TemplateRepository` and `StepResourceExecutor`. Handlers in
//! `api::rest::routes::inventory::templates` delegate to this service.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::contexts::inventory::domain::native::template_reference::{
    TemplateSelection, selection, validate_run_selections,
};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::contexts::inventory::infrastructure::persistence::repositories::{
    PreparedTemplateStep, PreparedTemplateVariable, StepOutcome, StepOutcomeProducer,
    StepResourceExecutor, TemplateHeaderRow, TemplateInsertRow, TemplateRepository,
    TemplateRunInput, TemplateRunListFilter, TemplateRunRow, TemplateRunStepRow, TemplateUpdateRow,
    is_supported_resource_type, parse_step_dependencies, resolve_step_previews,
    supported_resource_types_csv,
};
use crate::contexts::inventory::infrastructure::persistence::repositories::native::template_repository::is_safe_iteration_identifier;
use crate::contexts::shared::list_query::SortSpec;
use crate::contexts::shared::pagination::PageResult;
use crate::errors::AppError;

// ---------------------------------------------------------------------------
// Response DTOs (exposed to handlers for JSON envelopes)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct TemplateResponse {
    pub id: Uuid,
    pub name: String,
    pub slug: Option<String>,
    pub description: Option<String>,
    pub status: String,
    pub is_system: bool,
    pub owner_user_id: Option<Uuid>,
    pub updated_by_user_id: Option<Uuid>,
    pub created_at: String,
    pub updated_at: String,
    pub variables: Vec<Value>,
    pub steps: Vec<Value>,
}

#[derive(Debug, Clone)]
pub struct TemplateRunResponse {
    pub id: Uuid,
    pub template_id: Uuid,
    pub requested_by_user_id: Option<Uuid>,
    pub status: String,
    pub input_variables: Value,
    pub template_snapshot: Value,
    pub summary: Value,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct TemplateRunStepResponse {
    pub id: Uuid,
    pub run_id: Uuid,
    pub step_key: String,
    pub status: String,
    pub created_resource_ids: Value,
    pub resolved_payload: Value,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub duration_ms: Option<i32>,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct TemplatePreview {
    pub template_id: Uuid,
    pub variables: Value,
    pub steps: Vec<Value>,
}

#[derive(Debug, Clone, Default)]
pub struct TemplateUpsertInput {
    pub name: String,
    pub slug: Option<String>,
    pub description: Option<String>,
    pub status: Option<String>,
    pub variables: Option<Vec<Value>>,
    pub steps: Option<Vec<Value>>,
    pub is_system: Option<bool>,
}

// ---------------------------------------------------------------------------
// Service
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct TemplateService {
    repo: Arc<dyn TemplateRepository>,
    step_resources: Arc<dyn StepResourceExecutor>,
}

impl TemplateService {
    pub fn new(
        repo: Arc<dyn TemplateRepository>,
        step_resources: Arc<dyn StepResourceExecutor>,
    ) -> Self {
        Self {
            repo,
            step_resources,
        }
    }

    // --------------- Listing ---------------

    pub async fn list_templates(
        &self,
        status_filter: Vec<String>,
        search_filter: Option<String>,
        sort: Option<SortSpec>,
        limit: u32,
        offset: u32,
    ) -> Result<(Vec<TemplateResponse>, PageResult<()>), AppError> {
        let page = self
            .repo
            .list_templates_page(status_filter, search_filter, sort, limit, offset)
            .await?;

        // Batch-load all template data in 3 queries instead of 3 per template
        // (avoids N+1: 1 list + 3 batch = 4 queries per page).
        let ids: Vec<Uuid> = page.items.iter().map(|row| row.id).collect();
        let headers = self.repo.load_template_headers(&ids).await?;
        let variable_rows = self.repo.load_template_variables_for(&ids).await?;
        let step_rows = self.repo.load_template_steps_for(&ids).await?;

        // Group children by template_id; insertion order is preserved by the
        // batch query's `ORDER BY template_id, sort_order, created_at`.
        let mut headers_by_id: HashMap<Uuid, TemplateHeaderRow> =
            headers.into_iter().map(|h| (h.id, h)).collect();
        let mut variables_by_id: HashMap<Uuid, Vec<Value>> = HashMap::new();
        for vr in variable_rows {
            variables_by_id
                .entry(vr.template_id)
                .or_default()
                .push(vr.variable);
        }
        let mut steps_by_id: HashMap<Uuid, Vec<Value>> = HashMap::new();
        for sr in step_rows {
            steps_by_id.entry(sr.template_id).or_default().push(sr.step);
        }

        // Assemble responses in the original page order.
        let mut out = Vec::with_capacity(page.items.len());
        for row in &page.items {
            if let Some(header) = headers_by_id.remove(&row.id) {
                let variables = variables_by_id.remove(&row.id).unwrap_or_default();
                let steps = steps_by_id.remove(&row.id).unwrap_or_default();
                out.push(build_template_response(header, variables, steps));
            }
        }

        let meta = PageResult {
            items: vec![],
            total: page.total,
            limit: page.limit,
            offset: page.offset,
        };
        Ok((out, meta))
    }

    pub async fn get_template(
        &self,
        template_id: Uuid,
    ) -> Result<Option<TemplateResponse>, AppError> {
        self.load_template_response(template_id).await
    }

    pub async fn load_template_response(
        &self,
        template_id: Uuid,
    ) -> Result<Option<TemplateResponse>, AppError> {
        let Some(header) = self.repo.get_template_header(template_id).await? else {
            return Ok(None);
        };
        let variables = self.repo.load_template_variables(template_id).await?;
        let steps = self.repo.load_template_steps(template_id).await?;
        Ok(Some(build_template_response(header, variables, steps)))
    }

    // --------------- Create / update / delete ---------------

    pub async fn create_template(
        &self,
        input: TemplateUpsertInput,
        actor_id: Option<Uuid>,
    ) -> Result<TemplateResponse, AppError> {
        let name = require_non_empty(&input.name, "name")?;
        let status = parse_template_status(input.status.as_deref())?;
        let slug = normalize_optional_string(input.slug);
        let description = normalize_optional_string(input.description);
        let variables =
            validate_template_variables(input.variables.unwrap_or_default().as_slice())?;
        let steps = validate_template_steps(input.steps.unwrap_or_default().as_slice())?;
        let id = Uuid::now_v7();

        self.repo
            .create_template(TemplateInsertRow {
                id,
                name,
                slug,
                description,
                status,
                owner_user_id: actor_id,
                is_system: input.is_system.unwrap_or(false),
                updated_by_user_id: actor_id,
                variables,
                steps,
            })
            .await?;

        self.load_template_response(id)
            .await?
            .ok_or_else(|| AppError::NotFound("Template not found".to_string()))
    }

    pub async fn update_template(
        &self,
        template_id: Uuid,
        input: TemplateUpsertInput,
        actor_id: Option<Uuid>,
    ) -> Result<TemplateResponse, AppError> {
        let name = require_non_empty(&input.name, "name")?;
        let status = parse_template_status_change(input.status.as_deref())?;
        let slug = normalize_optional_string(input.slug);
        let description = normalize_optional_string(input.description);

        if !self.repo.template_exists(template_id).await? {
            return Err(AppError::NotFound("Template not found".to_string()));
        }

        // A field the caller did not send is not a field the caller emptied.
        // `unwrap_or_default()` collapsed those two into one, and the repository
        // replaces variables and steps by deleting them first - so a PUT that
        // only changed the name wiped every variable and every step the template
        // had. `is_system` was the same mistake with a worse trigger: the
        // template editor never sends it, so saving a built-in template from the
        // UI quietly demoted it to an ordinary one.
        let variables = input
            .variables
            .map(|values| validate_template_variables(values.as_slice()))
            .transpose()?;
        let steps = input
            .steps
            .map(|values| validate_template_steps(values.as_slice()))
            .transpose()?;

        self.repo
            .update_template(TemplateUpdateRow {
                id: template_id,
                name,
                slug,
                description,
                status,
                is_system: input.is_system,
                updated_by_user_id: actor_id,
                variables,
                steps,
            })
            .await?;

        self.load_template_response(template_id)
            .await?
            .ok_or_else(|| AppError::NotFound("Template not found".to_string()))
    }

    pub async fn delete_template(&self, template_id: Uuid) -> Result<(), AppError> {
        let affected = self.repo.delete_template(template_id).await?;
        if affected == 0 {
            return Err(AppError::NotFound("Template not found".to_string()));
        }
        Ok(())
    }

    // --------------- Preview / runs ---------------

    async fn prepare_run_variables(
        &self,
        template: &TemplateResponse,
        input_variables: Option<Value>,
        enforce_required: bool,
    ) -> Result<Value, AppError> {
        let merged = merge_run_variables(&template.variables, input_variables)?;
        let mut selections =
            validate_run_selections(&template.variables, &template.steps, &merged)?;
        selections.extend(validate_run_variable_values(
            &template.variables,
            &merged,
            enforce_required,
        )?);
        self.repo.validate_selections(&selections).await?;
        let mut tokens = HashMap::new();
        tokens_from_variable_map(&mut tokens, &merged);
        for step in &template.steps {
            crate::contexts::inventory::infrastructure::persistence::repositories::native::template_repository::resolve_step_payload(step, &tokens)?;
        }
        Ok(merged)
    }

    pub async fn preview_run(
        &self,
        template_id: Uuid,
        input_variables: Option<Value>,
    ) -> Result<TemplatePreview, AppError> {
        let template = self
            .load_template_response(template_id)
            .await?
            .ok_or_else(|| AppError::NotFound("Template not found".to_string()))?;

        let merged = self
            .prepare_run_variables(&template, input_variables, false)
            .await?;
        let mut tokens = HashMap::new();
        tokens_from_variable_map(&mut tokens, &merged);

        let mut resolved_steps = Vec::new();
        for step in &template.steps {
            resolved_steps.extend(resolve_step_previews(step, &mut tokens)?);
        }

        Ok(TemplatePreview {
            template_id,
            variables: merged,
            steps: resolved_steps,
        })
    }

    pub async fn create_run(
        &self,
        template_id: Uuid,
        requested_by_user_id: Uuid,
        input_variables: Option<Value>,
    ) -> Result<TemplateRunResponse, AppError> {
        let template = self
            .load_template_response(template_id)
            .await?
            .ok_or_else(|| AppError::NotFound("Template not found".to_string()))?;

        if template.status != "active" {
            return Err(AppError::Validation(
                "Template must be active to run".to_string(),
            ));
        }

        let merged = self
            .prepare_run_variables(&template, input_variables, true)
            .await?;
        let mut tokens = HashMap::new();
        tokens_from_variable_map(&mut tokens, &merged);

        let snapshot = json!({
            "id": template.id,
            "name": template.name,
            "slug": template.slug,
            "status": template.status,
            "variables": template.variables,
            "steps": template.steps,
        });

        let producer = DefaultOutcomeProducer;
        let run_id = self
            .repo
            .execute_template_run(
                TemplateRunInput {
                    template_id,
                    template_snapshot: snapshot,
                    requested_by_user_id,
                    input_variables: merged,
                    steps: template.steps.clone(),
                    initial_tokens: tokens,
                },
                self.step_resources.as_ref(),
                &producer,
            )
            .await?;

        self.get_run(run_id)
            .await?
            .ok_or_else(|| AppError::NotFound("Run not found".to_string()))
    }

    pub async fn list_runs(
        &self,
        template_id: Uuid,
        search_filter: Option<String>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<TemplateRunResponse>, AppError> {
        let run_ids = self
            .repo
            .list_template_run_ids_for_template(template_id, search_filter, limit, offset)
            .await?;
        let mut out = Vec::with_capacity(run_ids.len());
        for run_id in run_ids {
            if let Some(r) = self.get_run(run_id).await? {
                out.push(r);
            }
        }
        Ok(out)
    }

    pub async fn list_runs_page(
        &self,
        filter: TemplateRunListFilter,
        limit: u32,
        offset: u32,
    ) -> Result<(Vec<TemplateRunResponse>, PageResult<()>), AppError> {
        let page = self
            .repo
            .list_template_runs_page(filter, limit, offset)
            .await?;
        let out = page.items.into_iter().map(build_run_response).collect();
        let meta = PageResult {
            items: vec![],
            total: page.total,
            limit: page.limit,
            offset: page.offset,
        };
        Ok((out, meta))
    }

    pub async fn get_run(&self, run_id: Uuid) -> Result<Option<TemplateRunResponse>, AppError> {
        Ok(self
            .repo
            .get_template_run(run_id)
            .await?
            .map(build_run_response))
    }

    pub async fn list_run_steps(
        &self,
        run_id: Uuid,
    ) -> Result<Vec<TemplateRunStepResponse>, AppError> {
        let rows = self.repo.list_run_step_results(run_id).await?;
        Ok(rows.into_iter().map(build_run_step_response).collect())
    }
}

// ---------------------------------------------------------------------------
// Response builders
// ---------------------------------------------------------------------------

fn build_template_response(
    header: TemplateHeaderRow,
    variables: Vec<Value>,
    steps: Vec<Value>,
) -> TemplateResponse {
    TemplateResponse {
        id: header.id,
        name: header.name,
        slug: header.slug,
        description: header.description,
        status: header.status,
        is_system: header.is_system,
        owner_user_id: header.owner_user_id,
        updated_by_user_id: header.updated_by_user_id,
        created_at: header.created_at.to_rfc3339(),
        updated_at: header.updated_at.to_rfc3339(),
        variables,
        steps,
    }
}

fn build_run_response(row: TemplateRunRow) -> TemplateRunResponse {
    TemplateRunResponse {
        id: row.id,
        template_id: row.template_id,
        requested_by_user_id: row.requested_by_user_id,
        status: row.status,
        input_variables: row.input_variables,
        template_snapshot: row.template_snapshot,
        summary: row.summary,
        started_at: row.started_at.map(|v| v.to_rfc3339()),
        finished_at: row.finished_at.map(|v| v.to_rfc3339()),
        created_at: row.created_at.to_rfc3339(),
    }
}

fn build_run_step_response(row: TemplateRunStepRow) -> TemplateRunStepResponse {
    TemplateRunStepResponse {
        id: row.id,
        run_id: row.run_id,
        step_key: row.step_key,
        status: row.status,
        created_resource_ids: row.created_resource_ids,
        resolved_payload: row.resolved_payload,
        error_code: row.error_code,
        error_message: row.error_message,
        duration_ms: row.duration_ms,
        created_at: row.created_at.to_rfc3339(),
    }
}

// ---------------------------------------------------------------------------
// Validation helpers
// ---------------------------------------------------------------------------

fn require_non_empty(raw: &str, field: &str) -> Result<String, AppError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(AppError::Validation(format!("{field} is required")));
    }
    Ok(trimmed.to_string())
}

fn normalize_optional_string(value: Option<String>) -> Option<String> {
    value.and_then(|v| {
        let t = v.trim();
        if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        }
    })
}

/// The status a new template gets. Absent means `draft`, which is the right
/// default for something being created.
fn parse_template_status(value: Option<&str>) -> Result<&'static str, AppError> {
    Ok(parse_template_status_change(value)?.unwrap_or("draft"))
}

/// The status an existing template moves to, or `None` to stay where it is.
///
/// Creating and updating disagree about what an absent status means, and
/// collapsing them is how a rename demoted active templates to drafts: the
/// caller said nothing about status and got the creation default applied to a
/// template that had already left it.
fn parse_template_status_change(value: Option<&str>) -> Result<Option<&'static str>, AppError> {
    match value.map(str::trim) {
        None => Ok(None),
        Some("") | Some("draft") => Ok(Some("draft")),
        Some("active") => Ok(Some("active")),
        Some(other) => Err(AppError::Validation(format!(
            "Unsupported template status: {other}"
        ))),
    }
}

fn ensure_allowed_variable_type(value_type: &str) -> Result<(), AppError> {
    match value_type {
        "string" | "integer" | "number" | "boolean" | "json" | "resource_ref" => Ok(()),
        _ => Err(AppError::Validation(format!(
            "Unsupported variable value_type: {value_type}"
        ))),
    }
}

fn ensure_allowed_conflict_policy(policy: &str) -> Result<(), AppError> {
    match policy {
        "fail" | "skip" | "update_selected_fields" => Ok(()),
        _ => Err(AppError::Validation(format!(
            "Unsupported on_conflict_policy: {policy}"
        ))),
    }
}

fn validate_template_variables(raw: &[Value]) -> Result<Vec<PreparedTemplateVariable>, AppError> {
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(raw.len());

    for (index, variable) in raw.iter().enumerate() {
        let key = variable
            .get("key")
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::Validation(format!("variables[{index}].key is required")))?
            .trim()
            .to_string();
        if key.is_empty() {
            return Err(AppError::Validation(format!(
                "variables[{index}].key cannot be empty"
            )));
        }
        if !seen.insert(key.clone()) {
            return Err(AppError::Validation(format!(
                "variables[{index}].key duplicates a previous variable"
            )));
        }

        let label = variable
            .get("label")
            .and_then(Value::as_str)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| key.clone());

        let value_type = variable
            .get("value_type")
            .and_then(Value::as_str)
            .unwrap_or("string");
        ensure_allowed_variable_type(value_type)?;

        let required = variable
            .get("required")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let description = variable
            .get("description")
            .and_then(Value::as_str)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        let default_value = variable.get("default_value").cloned();
        let validation_rules = variable
            .get("validation_rules")
            .cloned()
            .unwrap_or_else(|| Value::Object(Map::new()));
        if !validation_rules.is_object() {
            return Err(AppError::Validation(format!(
                "variables[{index}].validation_rules must be an object"
            )));
        }
        if let Some(options) = validation_rules
            .get("options")
            .or_else(|| validation_rules.get("enum"))
        {
            let options = options.as_array().ok_or_else(|| {
                AppError::Validation(format!(
                    "variables[{index}].validation_rules.options must be an array of strings"
                ))
            })?;
            if options
                .iter()
                .any(|option| parse_template_option(value_type, option).is_none())
                || (!options.is_empty() && validation_rules.get("fields").is_some())
            {
                return Err(AppError::Validation(format!(
                    "variables[{index}].validation_rules.options must contain values of the declared type and cannot constrain object lists"
                )));
            }
        }
        if let Some(fields) = validation_rules.get("fields") {
            let fields = fields
                .as_array()
                .filter(|fields| !fields.is_empty())
                .ok_or_else(|| {
                    AppError::Validation(format!(
                        "variables[{index}].validation_rules.fields must be a nonempty array"
                    ))
                })?;
            if value_type != "json" {
                return Err(AppError::Validation(format!(
                    "variables[{index}] must use json for repeated object fields"
                )));
            }
            if fields.iter().any(|field| {
                !field
                    .get("key")
                    .and_then(Value::as_str)
                    .is_some_and(is_safe_iteration_identifier)
                    || !field.get("label").is_some_and(Value::is_string)
                    || !field.get("value_type").and_then(Value::as_str).is_some_and(|kind| ensure_allowed_variable_type(kind).is_ok())
                    || (field.get("value_type").and_then(Value::as_str) == Some("resource_ref")
                        && selection(&json!({"resource":field.pointer("/validation_rules/resource"),"id":Uuid::nil()})).is_err())
                    || field
                        .get("required")
                        .is_some_and(|value| !value.is_boolean())
                    || field
                        .get("description")
                        .is_some_and(|value| !value.is_string())
                    || field.pointer("/validation_rules/fields").is_some()
            }) {
                return Err(AppError::Validation(format!(
                    "variables[{index}].validation_rules.fields require labelled scalar fields with safe unique keys"
                )));
            }
            validate_template_variables(fields)?;
        }
        let sort_order = variable
            .get("sort_order")
            .and_then(Value::as_i64)
            .map(|v| v as i32)
            .unwrap_or(index as i32);

        out.push(PreparedTemplateVariable {
            key,
            label,
            description,
            value_type: value_type.to_string(),
            required,
            default_value,
            validation_rules,
            sort_order,
        });
    }

    Ok(out)
}

fn validate_template_steps(raw: &[Value]) -> Result<Vec<PreparedTemplateStep>, AppError> {
    let mut seen = HashSet::new();
    let mut parsed = Vec::with_capacity(raw.len());

    for (index, step) in raw.iter().enumerate() {
        let step_key = step
            .get("step_key")
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::Validation(format!("steps[{index}].step_key is required")))?
            .trim()
            .to_string();
        if step_key.is_empty() {
            return Err(AppError::Validation(format!(
                "steps[{index}].step_key cannot be empty"
            )));
        }
        if !seen.insert(step_key.clone()) {
            return Err(AppError::Validation(format!(
                "steps[{index}].step_key duplicates a previous step"
            )));
        }

        let title = step
            .get("title")
            .and_then(Value::as_str)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| step_key.clone());

        let resource_type_raw = step
            .get("resource_type")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                AppError::Validation(format!("steps[{index}].resource_type is required"))
            })?
            .trim();
        if resource_type_raw.is_empty() {
            return Err(AppError::Validation(format!(
                "steps[{index}].resource_type cannot be empty"
            )));
        }
        if !is_supported_resource_type(resource_type_raw) {
            return Err(AppError::Validation(format!(
                "steps[{index}].resource_type must be one of: {}",
                supported_resource_types_csv()
            )));
        }
        let resource_type = resource_type_raw.to_string();

        let operation = step
            .get("operation")
            .and_then(Value::as_str)
            .unwrap_or("create")
            .trim()
            .to_string();
        if operation != "create" {
            return Err(AppError::Validation(format!(
                "steps[{index}].operation must be 'create'"
            )));
        }

        let description = step
            .get("description")
            .and_then(Value::as_str)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());

        let depends_on_step_keys = parse_step_dependencies(step)
            .map_err(|e| AppError::Validation(format!("steps[{index}].{e}")))?;
        if depends_on_step_keys.iter().any(|d| d == &step_key) {
            return Err(AppError::Validation(format!(
                "steps[{index}].depends_on_step_keys cannot reference the step itself"
            )));
        }

        let payload_template = step
            .get("payload_template")
            .cloned()
            .unwrap_or_else(|| Value::Object(Map::new()));
        if !payload_template.is_object() {
            return Err(AppError::Validation(format!(
                "steps[{index}].payload_template must be an object"
            )));
        }

        let repeat = step
            .get("repeat")
            .cloned()
            .unwrap_or_else(|| Value::Object(Map::new()));
        validate_step_repeat(&repeat, index)?;

        let on_conflict_policy = step
            .get("on_conflict_policy")
            .and_then(Value::as_str)
            .unwrap_or("fail")
            .trim()
            .to_string();
        ensure_allowed_conflict_policy(&on_conflict_policy)?;

        let sort_order = step
            .get("sort_order")
            .and_then(Value::as_i64)
            .map(|v| v as i32)
            .unwrap_or(index as i32);

        parsed.push((
            index,
            PreparedTemplateStep {
                step_key,
                title,
                description,
                resource_type,
                operation,
                depends_on_step_keys,
                payload_template,
                repeat,
                on_conflict_policy,
                sort_order,
            },
        ));
    }

    // Cross-check dependency references
    let keys: HashSet<String> = parsed.iter().map(|(_, s)| s.step_key.clone()).collect();
    for (index, step) in &parsed {
        for dep in &step.depends_on_step_keys {
            if !keys.contains(dep) {
                return Err(AppError::Validation(format!(
                    "steps[{index}].depends_on_step_keys references unknown step '{dep}'"
                )));
            }
        }
    }

    Ok(parsed.into_iter().map(|(_, s)| s).collect())
}

fn validate_step_repeat(repeat: &Value, index: usize) -> Result<(), AppError> {
    if repeat.is_null() {
        return Ok(());
    }
    let Some(object) = repeat.as_object() else {
        return Err(AppError::Validation(format!(
            "steps[{index}].repeat must be an object"
        )));
    };
    if object.is_empty() {
        return Ok(());
    }
    if let Some(items) = object.get("for_each") {
        if object.contains_key("count")
            || object.contains_key("start")
            || object.contains_key("index_token")
        {
            return Err(AppError::Validation(format!(
                "steps[{index}].repeat.for_each cannot be combined with count, start or index_token"
            )));
        }
        if !items.is_array()
            && !items.as_str().is_some_and(|value| {
                let value = value.trim();
                value.starts_with("{{") && value.ends_with("}}")
            })
        {
            return Err(AppError::Validation(format!(
                "steps[{index}].repeat.for_each must be an array or template variable"
            )));
        }
        let item_token = object
            .get("item_token")
            .and_then(Value::as_str)
            .unwrap_or("item");
        if !is_safe_iteration_identifier(item_token) {
            return Err(AppError::Validation(format!(
                "steps[{index}].repeat.item_token must be an identifier other than position or steps"
            )));
        }
        return Ok(());
    }
    if object.contains_key("item_token") {
        return Err(AppError::Validation(format!(
            "steps[{index}].repeat.item_token requires for_each"
        )));
    }
    let count = object.get("count").ok_or_else(|| {
        AppError::Validation(format!(
            "steps[{index}].repeat.count is required when repeat is configured"
        ))
    })?;
    let count_is_valid = count.as_i64().is_some()
        || count
            .as_str()
            .map(|value| {
                let trimmed = value.trim();
                !trimmed.is_empty()
                    && (trimmed.starts_with("{{") && trimmed.ends_with("}}")
                        || trimmed.parse::<i64>().is_ok())
            })
            .unwrap_or(false);
    if !count_is_valid {
        return Err(AppError::Validation(format!(
            "steps[{index}].repeat.count must be an integer or template variable"
        )));
    }
    if let Some(start) = object.get("start") {
        let start_is_valid = start.as_i64().is_some()
            || start
                .as_str()
                .map(|value| {
                    let trimmed = value.trim();
                    !trimmed.is_empty()
                        && (trimmed.starts_with("{{") && trimmed.ends_with("}}")
                            || trimmed.parse::<i64>().is_ok())
                })
                .unwrap_or(false);
        if !start_is_valid {
            return Err(AppError::Validation(format!(
                "steps[{index}].repeat.start must be an integer or template variable"
            )));
        }
    }
    if let Some(index_token) = object.get("index_token") {
        let Some(index_token) = index_token.as_str().map(str::trim) else {
            return Err(AppError::Validation(format!(
                "steps[{index}].repeat.index_token must be a non-empty string"
            )));
        };
        if index_token.is_empty() {
            return Err(AppError::Validation(format!(
                "steps[{index}].repeat.index_token must be a non-empty string"
            )));
        }
    }
    Ok(())
}

fn merge_run_variables(
    variable_definitions: &[Value],
    input_variables: Option<Value>,
) -> Result<Value, AppError> {
    let mut merged = Map::new();

    for variable in variable_definitions {
        let key = variable
            .get("key")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if key.is_empty() {
            continue;
        }
        if let Some(default_value) = variable.get("default_value") {
            merged.insert(key.clone(), default_value.clone());
        }
        let required = variable
            .get("required")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if required && !merged.contains_key(&key) {
            merged.insert(key, Value::Null);
        }
    }

    if let Some(input_variables) = input_variables {
        let Value::Object(input_map) = input_variables else {
            return Err(AppError::Validation(
                "Template run variables must be an object".to_string(),
            ));
        };
        for (k, v) in input_map {
            merged.insert(k, v);
        }
    }

    for definition in variable_definitions {
        let Some(key) = definition.get("key").and_then(Value::as_str) else {
            continue;
        };
        let Some(fields) = definition
            .pointer("/validation_rules/fields")
            .and_then(Value::as_array)
        else {
            continue;
        };
        if let Some(rows) = merged.get_mut(key).and_then(Value::as_array_mut) {
            for row in rows {
                let Some(row) = row.as_object_mut() else {
                    continue;
                };
                for field in fields {
                    if let Some(key) = field.get("key").and_then(Value::as_str) {
                        row.entry(key.to_string()).or_insert_with(|| {
                            field.get("default_value").cloned().unwrap_or(Value::Null)
                        });
                    }
                }
            }
        }
    }

    Ok(Value::Object(merged))
}

fn tokens_from_variable_map(tokens: &mut HashMap<String, Value>, variables: &Value) {
    let Some(variables) = variables.as_object() else {
        return;
    };
    for (k, v) in variables {
        tokens.insert(k.clone(), v.clone());
    }
}

fn validate_run_variable_values(
    definitions: &[Value],
    values: &Value,
    enforce_required: bool,
) -> Result<Vec<TemplateSelection>, AppError> {
    let mut selections = Vec::new();
    for definition in definitions {
        let key = definition.get("key").and_then(Value::as_str).unwrap_or("");
        validate_run_variable_value(
            definition,
            values.get(key),
            key,
            enforce_required,
            false,
            &mut selections,
        )?;
    }
    Ok(selections)
}

fn validate_run_variable_value(
    definition: &Value,
    value: Option<&Value>,
    path: &str,
    enforce_required: bool,
    collection_field: bool,
    selections: &mut Vec<TemplateSelection>,
) -> Result<(), AppError> {
    let invalid = |code: &str| {
        AppError::ValidationFields([(path.to_string(), code.to_string())].into_iter().collect())
    };
    let missing = value.is_none_or(|value| {
        value.is_null()
            || value.as_str().is_some_and(|value| value.trim().is_empty())
            || (definition.pointer("/validation_rules/fields").is_some()
                && value.as_array().is_some_and(Vec::is_empty))
    });
    if missing {
        if enforce_required && definition.get("required").and_then(Value::as_bool) == Some(true) {
            return Err(invalid("required"));
        }
        return Ok(());
    }
    let Some(value) = value else {
        return Ok(());
    };
    let value_type = definition
        .get("value_type")
        .and_then(Value::as_str)
        .unwrap_or("string");
    let rules = definition
        .get("validation_rules")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let valid_type = match value_type {
        "string" => value.is_string() || (!collection_field && value.is_object()),
        "resource_ref" => value.is_string() || (!collection_field && value.is_object()),
        "integer" => value.as_i64().is_some(),
        "number" => value.as_f64().is_some_and(f64::is_finite),
        "boolean" => value.is_boolean(),
        "json" => true,
        _ => false,
    };
    if !valid_type {
        return Err(invalid(match value_type {
            "integer" => "invalid_integer",
            "number" => "invalid_number",
            "boolean" => "invalid_boolean",
            "json" => "invalid_json",
            _ => "invalid_reference",
        }));
    }
    if let Some(options) = rules
        .get("options")
        .or_else(|| rules.get("enum"))
        .and_then(Value::as_array)
        && !options.is_empty()
        && !options
            .iter()
            .filter_map(|option| parse_template_option(value_type, option))
            .any(|option| {
                if value_type == "number" {
                    option.as_f64() == value.as_f64()
                } else {
                    option == *value
                }
            })
    {
        return Err(invalid("invalid_option"));
    }
    if let Some(number) = value.as_f64()
        && (rules
            .get("min")
            .and_then(Value::as_f64)
            .is_some_and(|min| number < min)
            || rules
                .get("max")
                .and_then(Value::as_f64)
                .is_some_and(|max| number > max))
    {
        return Err(invalid("invalid_range"));
    }
    if rules.get("format").and_then(Value::as_str) == Some("email") {
        let valid_email = value.as_str().is_some_and(|email| {
            !email.chars().any(char::is_whitespace)
                && email.split_once('@').is_some_and(|(name, domain)| {
                    !name.is_empty()
                        && !domain.contains('@')
                        && domain
                            .rsplit_once('.')
                            .is_some_and(|(host, suffix)| !host.is_empty() && !suffix.is_empty())
                })
        });
        if !valid_email {
            return Err(invalid("invalid_email"));
        }
    }
    if collection_field && value_type == "resource_ref" {
        let resource = rules
            .get("resource")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("invalid_fields"))?;
        selections.push(
            selection(&json!({"resource":resource,"id":value}))
                .map_err(|_| invalid("invalid_reference"))?,
        );
    }
    if let Some(fields) = rules.get("fields") {
        let fields = fields.as_array().ok_or_else(|| invalid("invalid_fields"))?;
        let rows = value
            .as_array()
            .ok_or_else(|| invalid("invalid_collection"))?;
        if rows.len() > 500 {
            return Err(invalid("invalid_collection"));
        }
        for (index, row) in rows.iter().enumerate() {
            if !row.is_object()
                || row.as_object().is_some_and(|row| {
                    row.keys().any(|key| {
                        !fields
                            .iter()
                            .any(|field| field.get("key").and_then(Value::as_str) == Some(key))
                    })
                })
            {
                return Err(invalid("invalid_collection"));
            }
            for field in fields {
                let key = field.get("key").and_then(Value::as_str).unwrap_or("");
                validate_run_variable_value(
                    field,
                    row.get(key),
                    &format!("{path}.{index}.{key}"),
                    true,
                    true,
                    selections,
                )?;
            }
        }
    }
    Ok(())
}

fn parse_template_option(value_type: &str, option: &Value) -> Option<Value> {
    let text = option.as_str()?;
    if text.is_empty() || text != text.trim() {
        return None;
    }
    match value_type {
        "string" => Some(Value::String(text.to_string())),
        "integer" => {
            let digits = text.strip_prefix('-').unwrap_or(text);
            if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) {
                return None;
            }
            let number = text.parse::<i64>().ok()?;
            (number.unsigned_abs() <= 9_007_199_254_740_991).then(|| json!(number))
        }
        "number" => serde_json::Number::from_f64(text.parse::<f64>().ok()?).map(Value::Number),
        "boolean" => text.parse::<bool>().ok().map(Value::Bool),
        "json" => serde_json::from_str(text).ok(),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Outcome producer — maps execution events to `StepOutcome` records persisted
// by the repository's run executor.
// ---------------------------------------------------------------------------

const STATUS_SUCCESS: &str = "success";
const STATUS_SKIPPED: &str = "skipped";
const STATUS_FAILED: &str = "failed";
const ERROR_INVALID_DEPENDENCIES: &str = "invalid_dependencies";
const ERROR_DEPENDENCY_NOT_MET: &str = "dependency_not_met";
const ERROR_CONFLICT_SKIPPED: &str = "conflict_skipped";
const ERROR_EXECUTION_FAILED: &str = "step_execution_error";

struct DefaultOutcomeProducer;

impl StepOutcomeProducer for DefaultOutcomeProducer {
    fn on_invalid_dependencies(&self, step_key: &str, err: &AppError) -> StepOutcome {
        StepOutcome {
            step_key: step_key.to_string(),
            status: STATUS_FAILED.to_string(),
            created_resource_ids: Value::Array(Vec::new()),
            resolved_payload: Value::Object(Map::new()),
            error_code: Some(ERROR_INVALID_DEPENDENCIES.to_string()),
            error_message: Some(err.to_string()),
            duration_ms: None,
        }
    }

    fn on_dependency_not_met(&self, step_key: &str, blocking: &str) -> StepOutcome {
        StepOutcome {
            step_key: step_key.to_string(),
            status: STATUS_SKIPPED.to_string(),
            created_resource_ids: Value::Array(Vec::new()),
            resolved_payload: Value::Object(Map::new()),
            error_code: Some(ERROR_DEPENDENCY_NOT_MET.to_string()),
            error_message: Some(format!(
                "Dependency step '{blocking}' is missing or not successful"
            )),
            duration_ms: None,
        }
    }

    fn on_success(
        &self,
        step_key: &str,
        resolved_payload: &Value,
        created_ids: &[Uuid],
    ) -> StepOutcome {
        StepOutcome {
            step_key: step_key.to_string(),
            status: STATUS_SUCCESS.to_string(),
            created_resource_ids: Value::Array(
                created_ids
                    .iter()
                    .map(|v| Value::String(v.to_string()))
                    .collect(),
            ),
            resolved_payload: resolved_payload.clone(),
            error_code: None,
            error_message: None,
            duration_ms: None,
        }
    }

    fn on_failure(
        &self,
        step_key: &str,
        resolved_payload: &Value,
        err: &AppError,
        is_conflict_skip: bool,
    ) -> StepOutcome {
        if is_conflict_skip {
            StepOutcome {
                step_key: step_key.to_string(),
                status: STATUS_SKIPPED.to_string(),
                created_resource_ids: Value::Array(Vec::new()),
                resolved_payload: resolved_payload.clone(),
                error_code: Some(ERROR_CONFLICT_SKIPPED.to_string()),
                error_message: Some(err.to_string()),
                duration_ms: None,
            }
        } else {
            StepOutcome {
                step_key: step_key.to_string(),
                status: STATUS_FAILED.to_string(),
                created_resource_ids: Value::Array(Vec::new()),
                resolved_payload: resolved_payload.clone(),
                error_code: Some(ERROR_EXECUTION_FAILED.to_string()),
                error_message: Some(err.to_string()),
                duration_ms: None,
            }
        }
    }
}

// `extract_step_key` re-export for external callers (handlers don't need it,
// but keeping this ergonomic).
pub use crate::contexts::inventory::infrastructure::persistence::repositories::extract_step_key as extract_template_step_key;

#[cfg(test)]
mod template_reference_input_tests {
    use super::*;

    #[test]
    fn template_reference_run_input_requires_an_object() {
        for input in [json!([]), json!("bad"), json!(123)] {
            assert!(merge_run_variables(&[], Some(input)).is_err());
        }
        assert_eq!(
            merge_run_variables(&[], Some(json!({"manual":"Core Switch"})))
                .expect("object variables"),
            json!({"manual":"Core Switch"})
        );
    }

    fn collection_definition() -> Vec<Value> {
        vec![
            json!({"key":"contacts","label":"Contacts","value_type":"json",
            "default_value":[],"validation_rules":{"fields":[
                {"key":"contact_id","label":"Contact","value_type":"resource_ref","required":true,
                    "validation_rules":{"resource":"contact"}},
                {"key":"role_id","label":"Role","value_type":"resource_ref","required":true,
                    "validation_rules":{"resource":"contact_role"}},
                {"key":"priority","label":"Priority","value_type":"string","required":true,
                    "validation_rules":{"options":["primary","secondary"]}}
            ]}}),
        ]
    }

    #[test]
    fn template_collection_validates_each_reference_and_preserves_distinct_rows() {
        let definitions = collection_definition();
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        let role = Uuid::new_v4();
        let input = json!({"contacts":[
            {"contact_id":first,"role_id":role,"priority":"primary"},
            {"contact_id":second,"role_id":role,"priority":"secondary"}
        ]});
        let merged = merge_run_variables(&definitions, Some(input.clone())).expect("merge");
        assert_eq!(merged, input);
        let selections =
            validate_run_variable_values(&definitions, &merged, true).expect("valid rows");
        assert_eq!(
            selections
                .iter()
                .filter(|s| s.resource == "contact")
                .map(|s| s.id)
                .collect::<Vec<_>>(),
            vec![first, second]
        );
        assert!(
            validate_run_variable_values(&definitions, &json!({"contacts":[]}), true)
                .expect("no contacts")
                .is_empty()
        );
    }

    #[test]
    fn template_collection_rejects_malformed_rows_references_and_choices_before_execution() {
        let definitions = collection_definition();
        let contact = Uuid::new_v4();
        let role = Uuid::new_v4();
        for rows in [
            json!({}),
            json!("bad"),
            json!([null]),
            json!([{"contact_id":contact,"priority":"primary"}]),
            json!([{"contact_id":"bad","role_id":role,"priority":"primary"}]),
            json!([{"contact_id":contact,"role_id":role,"priority":"unknown"}]),
            json!([{"contact_id":{"resource":"contact","id":contact},"role_id":role,"priority":"primary"}]),
            Value::Array(vec![json!({}); 501]),
        ] {
            assert!(
                validate_run_variable_values(&definitions, &json!({"contacts":rows}), true)
                    .is_err()
            );
        }
        let error = validate_run_variable_values(
            &definitions,
            &json!({"contacts":[
                {"contact_id":contact,"role_id":role,"priority":"primary"},
                {"contact_id":contact,"priority":"primary"}
            ]}),
            false,
        )
        .expect_err("row 2 missing role")
        .to_string();
        assert!(error.contains("contacts.1.role_id"), "{error}");
    }

    #[test]
    fn template_run_validates_required_strings_options_and_numeric_bounds() {
        let definitions = vec![
            json!({"key":"name","value_type":"string","required":true}),
            json!({"key":"count","value_type":"integer","validation_rules":{"min":0,"max":500}}),
            json!({"key":"status","value_type":"string","validation_rules":{"options":["active","planned"]}}),
        ];
        for input in [
            json!({"name":" "}),
            json!({"name":"Site","count":-1}),
            json!({"name":"Site","count":1.5}),
            json!({"name":"Site","status":"bad"}),
        ] {
            assert!(validate_run_variable_values(&definitions, &input, true).is_err());
        }
        validate_run_variable_values(
            &definitions,
            &json!({"name":"Site","count":0,"status":"active"}),
            true,
        )
        .expect("valid inputs");
        validate_run_variable_values(&definitions, &json!({}), false)
            .expect("authoring preview may omit required inputs");
    }

    #[test]
    fn template_collection_authoring_rejects_ambiguous_repeat_modes_and_invalid_fields() {
        validate_step_repeat(
            &json!({"for_each":"{{contacts}}","item_token":"contact"}),
            0,
        )
        .expect("collection repeat");
        for repeat in [
            json!({"for_each":3}),
            json!({"for_each":[],"count":1}),
            json!({"for_each":[],"item_token":"steps"}),
            json!({"for_each":[],"item_token":"bad.name"}),
        ] {
            assert!(validate_step_repeat(&repeat, 0).is_err());
        }
        validate_template_variables(&collection_definition()).expect("object-list schema");
        assert!(
            validate_template_variables(&[json!({"key":"contacts","value_type":"string",
            "validation_rules":{"fields":[{"key":"name"}]}})])
            .is_err()
        );
    }

    #[test]
    fn template_collection_fills_declared_defaults_and_absent_optional_values() {
        let definitions = vec![
            json!({"key":"people","value_type":"json","validation_rules":{"fields":[
                {"key":"name","label":"Name","value_type":"string","required":true},
                {"key":"email","label":"Email","value_type":"string"},
                {"key":"priority","label":"Priority","value_type":"string","default_value":"primary"}
            ]}}),
        ];
        let merged = merge_run_variables(&definitions, Some(json!({"people":[{"name":"Person"}]})))
            .expect("merge defaults");
        assert_eq!(
            merged,
            json!({"people":[{"name":"Person","email":null,"priority":"primary"}]})
        );
        validate_run_variable_values(&definitions, &merged, true).expect("valid optional fields");
    }

    #[test]
    fn template_options_match_typed_form_values() {
        let definitions = vec![
            json!({"key":"count","value_type":"integer","validation_rules":{"options":["1","2"]}}),
            json!({"key":"ratio","value_type":"number","validation_rules":{"options":["1","2.5"]}}),
            json!({"key":"enabled","value_type":"boolean","validation_rules":{"options":["true","false"]}}),
            json!({"key":"data","value_type":"json","required":true,"validation_rules":{"options":["[]","[1]"]}}),
        ];
        validate_template_variables(&definitions).expect("typed declarations consumed by the form");
        validate_run_variable_values(
            &definitions,
            &json!({"count":2,"ratio":1,"enabled":false,"data":[]}),
            true,
        )
        .expect("form selections");
        for input in [
            json!({"count":3,"data":[]}),
            json!({"ratio":3,"data":[]}),
            json!({"enabled":"false","data":[]}),
            json!({"data":[2]}),
        ] {
            assert!(validate_run_variable_values(&definitions, &input, true).is_err());
        }
        for definition in [
            json!({"key":"count","value_type":"integer","validation_rules":{"options":[1,2]}}),
            json!({"key":"count","value_type":"integer","validation_rules":{"options":["1.5"]}}),
            json!({"key":"count","value_type":"integer","validation_rules":{"options":["9007199254740992"]}}),
            json!({"key":"enabled","value_type":"boolean","validation_rules":{"options":["yes"]}}),
            json!({"key":"status","value_type":"string","validation_rules":{"options":[" active "]}}),
        ] {
            assert!(validate_template_variables(&[definition]).is_err());
        }
        validate_run_variable_values(
            &[json!({"key":"data","value_type":"json","required":true})],
            &json!({"data":[]}),
            true,
        )
        .expect("required ordinary JSON can be an empty array");
    }

    #[test]
    fn template_explicit_optional_clears_do_not_restore_defaults() {
        let contact = Uuid::new_v4();
        let definitions = vec![
            json!({"key":"email","value_type":"string","default_value":"default@example.test"}),
            json!({"key":"people","value_type":"json","validation_rules":{"fields":[
                {"key":"email","label":"Email","value_type":"string","default_value":"default@example.test"},
                {"key":"contact_id","label":"Contact","value_type":"resource_ref","default_value":contact,"validation_rules":{"resource":"contact"}},
                {"key":"count","label":"Count","value_type":"integer","default_value":2},
                {"key":"enabled","label":"Enabled","value_type":"boolean","default_value":true},
                {"key":"data","label":"Data","value_type":"json","default_value":{"configured":true}}
            ]}}),
        ];
        let input = json!({"email":"","people":[
            {"email":"","contact_id":null,"count":null,"enabled":null,"data":null},
            {"email":null,"contact_id":"","count":null,"enabled":false,"data":[]}
        ]});
        let merged =
            merge_run_variables(&definitions, Some(input.clone())).expect("explicit values");
        assert_eq!(merged, input);
        validate_run_variable_values(&definitions, &merged, true)
            .expect("optional values stay cleared");
        let missing = merge_run_variables(&definitions, Some(json!({"people":[{}]})))
            .expect("absent defaults");
        assert_eq!(missing["email"], "default@example.test");
        assert_eq!(missing["people"][0]["email"], "default@example.test");
        assert_eq!(missing["people"][0]["contact_id"], json!(contact));
    }

    #[test]
    fn template_collection_authoring_rejects_incomplete_field_types_and_reference_declarations() {
        for field in [
            json!({"key":"name","label":"Name","required":true}),
            json!({"key":"contact","label":"Contact","value_type":"resource_ref"}),
            json!({"key":"contact","label":"Contact","value_type":"resource_ref","validation_rules":{"resource":"unknown"}}),
        ] {
            assert!(validate_template_variables(&[json!({"key":"people","value_type":"json","validation_rules":{"fields":[field]}})]).is_err());
        }
        validate_template_variables(&[json!({"key":"people","value_type":"json","validation_rules":{"fields":[
            {"key":"name","label":"Name","value_type":"string","required":true,"default_value":"Alex"}
        ]}})]).expect("explicit field schema");
    }
}
