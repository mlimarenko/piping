//! Shared runtime check for the table-registry fixtures
//! (`inventory_table_columns.rs`, `non_inventory_table_columns.rs`).
//!
//! Every registry names the collection path of its REST resource. Instead of
//! searching route sources for identifier text, the check reads the generated
//! `ApiDoc`: the path must exist there as a `get` operation, and every filter a
//! registry column declares must be an actual query parameter of that
//! operation. That ties registry → route → filter parameter to the contract the
//! server publishes at runtime.

use std::collections::BTreeSet;

use piping_backend::api::openapi::ApiDoc;
use piping_backend::contexts::shared::list_query::ListColumn;
use utoipa::OpenApi;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// One table registry: its fixture name, the collection path of the REST
/// resource it describes and the columns whose filters must be settable.
pub(super) struct RegistryRoute {
    pub name: &'static str,
    pub path: &'static str,
    pub columns: &'static [ListColumn],
}

/// A registry filter parameter that no HTTP request can set, so it must not be
/// required from the OpenAPI operation, with the reason that makes it so.
pub(super) struct NonHttpFilter {
    pub registry: &'static str,
    pub param: &'static str,
    pub reason: &'static str,
}

/// Asserts that every registry is served by a documented collection operation
/// declaring its filter query parameters.
///
/// `non_http_filters` is a narrow, self-cleaning exception list: an entry whose
/// registry was reconciled must still be needed, otherwise the check fails and
/// asks for its removal. A registry that already failed on its collection path
/// or `GET` operation never reconciled its filters, so its exceptions are left
/// alone — that registry's own failure is what the report carries.
pub(super) fn assert_registry_collection_operations(
    registries: &[RegistryRoute],
    non_http_filters: &[NonHttpFilter],
) -> Result<(), BoxError> {
    let document = serde_json::to_value(ApiDoc::openapi())?;
    let paths = document
        .get("paths")
        .and_then(serde_json::Value::as_object)
        .ok_or("ApiDoc has no `paths` object")?;

    let mut failures: Vec<String> = Vec::new();
    let mut applied_exceptions: Vec<(&str, &str)> = Vec::new();
    // Registries that failed before their columns were compared against the
    // operation's query parameters.
    let mut unreconciled: BTreeSet<&str> = BTreeSet::new();

    for entry in non_http_filters {
        assert!(
            !entry.reason.trim().is_empty(),
            "{}:{} needs a reason",
            entry.registry,
            entry.param
        );
    }

    for registry in registries {
        if !registry.path.starts_with("/api/v1/") {
            failures.push(format!(
                "{} declares collection path {:?}, which is not an /api/v1/ path",
                registry.name, registry.path
            ));
            unreconciled.insert(registry.name);
            continue;
        }

        let operation = paths
            .get(registry.path)
            .and_then(|path_item| path_item.get("get"))
            .filter(|operation| operation.is_object());
        let Some(operation) = operation else {
            failures.push(format!(
                "{} declares collection path {:?}, which ApiDoc does not expose as a GET operation",
                registry.name, registry.path
            ));
            unreconciled.insert(registry.name);
            continue;
        };

        let query_params: BTreeSet<&str> = operation
            .get("parameters")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter(|parameter| {
                parameter.get("in").and_then(serde_json::Value::as_str) == Some("query")
            })
            .filter_map(|parameter| parameter.get("name").and_then(serde_json::Value::as_str))
            .collect();

        for column in registry.columns {
            let Some(filter) = column.filter else {
                continue;
            };
            if query_params.contains(filter.param) {
                continue;
            }
            if let Some(exception) = non_http_filters
                .iter()
                .find(|entry| entry.registry == registry.name && entry.param == filter.param)
            {
                applied_exceptions.push((exception.registry, exception.param));
                continue;
            }
            failures.push(format!(
                "{} column {:?} filters on {:?}, but {} does not document that query parameter",
                registry.name, column.key, filter.param, registry.path
            ));
        }
    }

    for entry in non_http_filters {
        if applied_exceptions.contains(&(entry.registry, entry.param)) {
            continue;
        }
        // `unreconciled` holds registries that failed on their collection path or
        // GET operation, so their filters were never compared: the exception is
        // not provably unused, and the real failure is already reported.
        if unreconciled.contains(entry.registry) {
            continue;
        }
        failures.push(format!(
            "{} is listed as a non-HTTP filter of {} but the exception is no longer needed \
             (the registry no longer filters on it, or the route documents it): remove it",
            entry.param, entry.registry
        ));
    }

    assert!(
        failures.is_empty(),
        "table registry collection contracts are not satisfied:\n{}",
        failures.join("\n")
    );
    Ok(())
}
