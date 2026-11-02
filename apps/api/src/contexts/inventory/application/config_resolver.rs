use std::collections::HashMap;

use serde_json::{Map, Value};
use uuid::Uuid;

use crate::contexts::connections::application::connection_params_schema::{
    SECRET_FIELDS_RDP, SECRET_FIELDS_SSH, SECRET_FIELDS_VNC,
};
use crate::contexts::inventory::domain::{
    ConfigContext, ResolvedContextEntry, SecretPresence, SourceAttribution,
};

/// Config context candidate with precomputed scope precedence.
#[derive(Debug, Clone)]
pub struct ConfigContextCandidate {
    pub context: ConfigContext,
    pub scope_precedence: u32,
}

#[derive(Debug, Clone)]
pub struct AnnotatedConfigContextCandidate {
    pub context: ConfigContext,
    pub scope_type: String,
    pub scope_id: Uuid,
    pub scope_name: String,
    pub scope_precedence: u32,
}

/// Resolve config contexts with weight, scope precedence, and UUIDv7 tie-break.
pub fn resolve_config_contexts(contexts: &[ConfigContext]) -> Value {
    let candidates = contexts
        .iter()
        .cloned()
        .map(|context| ConfigContextCandidate {
            context,
            scope_precedence: 0,
        })
        .collect::<Vec<_>>();
    resolve_config_candidates(&candidates)
}

/// Resolve config contexts in the provided order.
pub fn resolve_config_contexts_ordered(contexts: &[ConfigContext]) -> Value {
    let mut merged = Value::Object(Map::new());
    for context in contexts {
        merge_value(&mut merged, &context.data);
    }
    merged
}

pub fn resolve_config_contexts_annotated(
    candidates: &[AnnotatedConfigContextCandidate],
) -> (
    Value,
    HashMap<String, SourceAttribution>,
    Vec<ResolvedContextEntry>,
) {
    let mut sorted = candidates.to_vec();
    sorted.sort_by_key(|candidate| {
        (
            candidate.context.weight,
            candidate.scope_precedence,
            candidate.context.id,
        )
    });

    let mut merged = Value::Object(Map::new());
    let mut attribution = HashMap::new();
    let mut chain = Vec::with_capacity(sorted.len());
    for candidate in sorted {
        let source = SourceAttribution {
            context_id: candidate.context.id,
            context_name: candidate.context.name.clone(),
            scope_type: candidate.scope_type.clone(),
            scope_id: candidate.scope_id,
            scope_name: candidate.scope_name.clone(),
            weight: candidate.context.weight,
            scope_precedence: candidate.scope_precedence,
        };
        merge_value_annotated(
            &mut merged,
            &candidate.context.data,
            "",
            &source,
            &mut attribution,
        );
        chain.push(ResolvedContextEntry {
            context_id: source.context_id,
            context_name: source.context_name.clone(),
            scope_type: source.scope_type.clone(),
            scope_id: source.scope_id,
            scope_name: source.scope_name.clone(),
            weight: source.weight,
            scope_precedence: source.scope_precedence,
        });
    }

    (merged, attribution, chain)
}

/// Resolve config contexts with weight, scope precedence, and UUIDv7 tie-break.
pub fn resolve_config_candidates(candidates: &[ConfigContextCandidate]) -> Value {
    let mut sorted = candidates.to_vec();
    sorted.sort_by_key(|candidate| {
        (
            candidate.context.weight,
            candidate.scope_precedence,
            candidate.context.id,
        )
    });

    let mut merged = Value::Object(Map::new());
    for candidate in sorted {
        merge_value(&mut merged, &candidate.context.data);
    }

    merged
}

const SCOPE_PRECEDENCE: [(&str, u32); 8] = [
    ("tenant_group", 0),
    ("tenant", 1),
    ("site", 2),
    ("cluster", 3),
    ("role", 4),
    ("device", 5),
    ("vm", 6),
    ("connection_target", 7),
];

pub fn scope_precedence(scope_type: &str) -> u32 {
    SCOPE_PRECEDENCE
        .iter()
        .find_map(|(scope, precedence)| {
            if *scope == scope_type {
                Some(*precedence)
            } else {
                None
            }
        })
        .unwrap_or(0)
}

pub(crate) fn merge_value(target: &mut Value, incoming: &Value) {
    match (target, incoming) {
        (Value::Object(target_map), Value::Object(incoming_map)) => {
            for (key, value) in incoming_map {
                match target_map.get_mut(key) {
                    Some(existing) => merge_value(existing, value),
                    None => {
                        target_map.insert(key.clone(), value.clone());
                    }
                }
            }
        }
        (target_value, Value::Array(_)) => {
            *target_value = incoming.clone();
        }
        (target_value, Value::Object(_)) => {
            *target_value = incoming.clone();
        }
        (target_value, _) => {
            *target_value = incoming.clone();
        }
    }
}

pub(crate) fn merge_value_annotated(
    target: &mut Value,
    incoming: &Value,
    current_path: &str,
    source: &SourceAttribution,
    attribution: &mut HashMap<String, SourceAttribution>,
) {
    match (target, incoming) {
        (Value::Object(target_map), Value::Object(incoming_map)) => {
            for (key, value) in incoming_map {
                let child_path = join_path(current_path, key);
                match target_map.get_mut(key) {
                    Some(existing) => {
                        merge_value_annotated(existing, value, &child_path, source, attribution)
                    }
                    None => {
                        target_map.insert(key.clone(), value.clone());
                        assign_attribution_tree(&child_path, value, source, attribution);
                    }
                }
            }
        }
        (target_value, _) => {
            *target_value = incoming.clone();
            assign_attribution_tree(current_path, incoming, source, attribution);
        }
    }
}

pub fn compute_secret_presence(
    config: &Value,
    attribution: &HashMap<String, SourceAttribution>,
) -> HashMap<String, SecretPresence> {
    let mut presence = HashMap::new();
    for (protocol, secret_fields) in [
        ("ssh", SECRET_FIELDS_SSH.as_slice()),
        ("rdp", SECRET_FIELDS_RDP.as_slice()),
        ("vnc", SECRET_FIELDS_VNC.as_slice()),
    ] {
        let Some(protocol_value) = config
            .get("piping")
            .and_then(|v| v.get("connections"))
            .and_then(|v| v.get(protocol))
        else {
            continue;
        };

        match protocol_value {
            Value::Array(items) => {
                for (index, entry) in items.iter().enumerate() {
                    let Some(entry_obj) = entry.as_object() else {
                        continue;
                    };
                    let Some(credentials) = entry_obj.get("credentials").and_then(Value::as_object)
                    else {
                        continue;
                    };
                    for secret_field in secret_fields {
                        if let Some(value) = credentials.get(*secret_field) {
                            let path = format!(
                                "piping.connections.{protocol}.{index}.credentials.{secret_field}"
                            );
                            presence.insert(
                                path.clone(),
                                SecretPresence {
                                    has_value: !value.is_null(),
                                    source_context_name: attribution
                                        .get(&path)
                                        .map(|item| item.context_name.clone()),
                                },
                            );
                        }
                    }
                }
            }
            Value::Object(entry_obj) => {
                let Some(credentials) = entry_obj.get("credentials").and_then(Value::as_object)
                else {
                    continue;
                };
                for secret_field in secret_fields {
                    if let Some(value) = credentials.get(*secret_field) {
                        let path =
                            format!("piping.connections.{protocol}.credentials.{secret_field}");
                        presence.insert(
                            path.clone(),
                            SecretPresence {
                                has_value: !value.is_null(),
                                source_context_name: attribution
                                    .get(&path)
                                    .map(|item| item.context_name.clone()),
                            },
                        );
                    }
                }
            }
            _ => {}
        }
    }

    presence
}

pub fn redact_secret_fields(config: &mut Value, secret_presence: &HashMap<String, SecretPresence>) {
    for path in secret_presence.keys() {
        remove_path(config, path);
    }
}

fn join_path(base: &str, segment: &str) -> String {
    if base.is_empty() {
        segment.to_string()
    } else {
        format!("{base}.{segment}")
    }
}

fn assign_attribution_tree(
    path: &str,
    value: &Value,
    source: &SourceAttribution,
    attribution: &mut HashMap<String, SourceAttribution>,
) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                let child_path = join_path(path, key);
                assign_attribution_tree(&child_path, child, source, attribution);
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                let child_path = join_path(path, &index.to_string());
                assign_attribution_tree(&child_path, child, source, attribution);
            }
        }
        _ => {
            if !path.is_empty() {
                attribution.insert(path.to_string(), source.clone());
            }
        }
    }
}

fn remove_path(value: &mut Value, path: &str) {
    let mut segments = path.split('.').collect::<Vec<_>>();
    if segments.is_empty() {
        return;
    }
    remove_path_segments(value, &mut segments);
}

fn remove_path_segments(current: &mut Value, segments: &mut Vec<&str>) {
    let Some(head) = segments.first().copied() else {
        return;
    };
    if segments.len() == 1 {
        match current {
            Value::Object(map) => {
                map.remove(head);
            }
            Value::Array(items) => {
                if let Ok(index) = head.parse::<usize>()
                    && index < items.len()
                {
                    items[index] = Value::Null;
                }
            }
            _ => {}
        }
        return;
    }

    let tail = segments[1..].to_vec();
    match current {
        Value::Object(map) => {
            if let Some(next) = map.get_mut(head) {
                remove_path_segments(next, &mut tail.clone());
            }
        }
        Value::Array(items) => {
            if let Ok(index) = head.parse::<usize>()
                && let Some(next) = items.get_mut(index)
            {
                remove_path_segments(next, &mut tail.clone());
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use uuid::Uuid;

    fn context(id: u128, weight: i32, data: Value) -> ConfigContext {
        ConfigContext {
            id: Uuid::from_u128(id),
            name: format!("ctx-{}", id),
            description: None,
            weight,
            data,
            is_active: true,
            created_at: None,
            updated_at: None,
        }
    }

    #[test]
    fn ordered_contexts_override_in_sequence() {
        let first = context(1, 0, json!({ "key": "first", "nested": { "a": 1 } }));
        let second = context(2, 0, json!({ "key": "second", "nested": { "b": 2 } }));

        let resolved = resolve_config_contexts_ordered(&[first, second]);
        assert_eq!(resolved["key"], "second");
        assert_eq!(resolved["nested"]["a"], 1);
        assert_eq!(resolved["nested"]["b"], 2);
    }

    #[test]
    fn candidates_apply_weight_then_scope_precedence() {
        let low_weight = ConfigContextCandidate {
            context: context(10, 0, json!({ "key": "low" })),
            scope_precedence: 0,
        };
        let high_weight = ConfigContextCandidate {
            context: context(11, 10, json!({ "key": "high" })),
            scope_precedence: 0,
        };
        let precedence_low = ConfigContextCandidate {
            context: context(12, 5, json!({ "scope": "low" })),
            scope_precedence: 1,
        };
        let precedence_high = ConfigContextCandidate {
            context: context(13, 5, json!({ "scope": "high" })),
            scope_precedence: 7,
        };

        let resolved = resolve_config_candidates(&[
            high_weight.clone(),
            low_weight.clone(),
            precedence_high.clone(),
            precedence_low.clone(),
        ]);

        assert_eq!(resolved["key"], "high");
        assert_eq!(resolved["scope"], "high");
    }

    #[test]
    fn scope_precedence_returns_correct_values() {
        assert_eq!(scope_precedence("tenant_group"), 0);
        assert_eq!(scope_precedence("tenant"), 1);
        assert_eq!(scope_precedence("site"), 2);
        assert_eq!(scope_precedence("connection_target"), 7);
        assert_eq!(scope_precedence("unknown"), 0);
    }

    #[test]
    fn candidates_use_uuid_tiebreaker() {
        let first = ConfigContextCandidate {
            context: context(1, 1, json!({ "tie": "first" })),
            scope_precedence: 2,
        };
        let second = ConfigContextCandidate {
            context: context(2, 1, json!({ "tie": "second" })),
            scope_precedence: 2,
        };

        let resolved = resolve_config_candidates(&[second, first]);
        assert_eq!(resolved["tie"], "second");
    }

    fn annotated_candidate(
        id: u128,
        name: &str,
        weight: i32,
        scope: (&str, &str, u32),
        data: Value,
    ) -> AnnotatedConfigContextCandidate {
        let (scope_type, scope_name, scope_precedence) = scope;
        AnnotatedConfigContextCandidate {
            context: ConfigContext {
                id: Uuid::from_u128(id),
                name: name.to_string(),
                description: None,
                weight,
                data,
                is_active: true,
                created_at: None,
                updated_at: None,
            },
            scope_type: scope_type.to_string(),
            scope_id: Uuid::from_u128(id + 1000),
            scope_name: scope_name.to_string(),
            scope_precedence,
        }
    }

    #[test]
    fn annotated_resolver_tracks_leaf_attribution_and_chain() {
        let tenant = annotated_candidate(
            1,
            "Tenant defaults",
            0,
            ("tenant", "Acme", 1),
            json!({
                "settings": {
                    "a": 1,
                    "b": 2
                }
            }),
        );
        let target = annotated_candidate(
            2,
            "Target override",
            100,
            ("connection_target", "web-01", 7),
            json!({
                "settings": {
                    "b": 3
                }
            }),
        );

        let (config, attribution, chain) = resolve_config_contexts_annotated(&[target, tenant]);
        assert_eq!(config["settings"]["a"], 1);
        assert_eq!(config["settings"]["b"], 3);
        assert_eq!(
            attribution
                .get("settings.a")
                .map(|v| v.context_name.as_str()),
            Some("Tenant defaults")
        );
        assert_eq!(
            attribution
                .get("settings.b")
                .map(|v| v.context_name.as_str()),
            Some("Target override")
        );
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0].context_name, "Tenant defaults");
        assert_eq!(chain[1].context_name, "Target override");
    }

    #[test]
    fn annotated_resolver_is_deterministic_for_input_order() {
        let tenant = annotated_candidate(
            1,
            "Tenant defaults",
            0,
            ("tenant", "Acme", 1),
            json!({
                "settings": {
                    "timeout": 15,
                    "username": "tenant-user"
                }
            }),
        );
        let target = annotated_candidate(
            2,
            "Target override",
            100,
            ("connection_target", "web-01", 7),
            json!({
                "settings": {
                    "timeout": 30
                }
            }),
        );

        let (first_config, first_attribution, first_chain) =
            resolve_config_contexts_annotated(&[target.clone(), tenant.clone()]);
        let (second_config, second_attribution, second_chain) =
            resolve_config_contexts_annotated(&[tenant, target]);

        assert_eq!(first_config, second_config);
        assert_eq!(
            serde_json::to_value(&first_attribution).expect("serialize first attribution"),
            serde_json::to_value(&second_attribution).expect("serialize second attribution")
        );
        assert_eq!(
            serde_json::to_value(&first_chain).expect("serialize first chain"),
            serde_json::to_value(&second_chain).expect("serialize second chain")
        );
    }

    #[test]
    fn compute_secret_presence_and_redaction() {
        let source = SourceAttribution {
            context_id: Uuid::from_u128(1),
            context_name: "Tenant defaults".to_string(),
            scope_type: "tenant".to_string(),
            scope_id: Uuid::from_u128(2),
            scope_name: "Acme".to_string(),
            weight: 0,
            scope_precedence: 1,
        };
        let mut config = json!({
            "piping": {
                "connections": {
                    "ssh": [{
                        "host": "10.0.0.1",
                        "credentials": {
                            "password": "8f8af08c-b57f-420b-bf3a-4aa2f9f09ab2"
                        }
                    }]
                }
            }
        });
        let mut attribution = HashMap::new();
        attribution.insert(
            "piping.connections.ssh.0.credentials.password".to_string(),
            source.clone(),
        );
        attribution.insert("piping.connections.ssh.0.host".to_string(), source);

        let presence = compute_secret_presence(&config, &attribution);
        assert_eq!(
            presence
                .get("piping.connections.ssh.0.credentials.password")
                .map(|v| v.has_value),
            Some(true)
        );
        assert_eq!(
            presence
                .get("piping.connections.ssh.0.credentials.password")
                .and_then(|v| v.source_context_name.as_deref()),
            Some("Tenant defaults")
        );

        redact_secret_fields(&mut config, &presence);
        assert!(
            config["piping"]["connections"]["ssh"][0]["credentials"]
                .get("password")
                .is_none()
        );
        assert_eq!(
            config["piping"]["connections"]["ssh"][0]["host"],
            "10.0.0.1"
        );
    }
}
