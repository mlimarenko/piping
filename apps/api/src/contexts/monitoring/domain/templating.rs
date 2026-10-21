//! `$host*` template rendering for provider queries and Zabbix host
//! selectors, with PromQL string-fragment escaping.

use super::host::DiscoveryHost;

pub fn render_host_metric_query(query: &str, host: &DiscoveryHost) -> String {
    render_host_template(query, host, &host.host_name, true)
}

/// The `$host*` tokens and what each expands to.
///
/// What stops `$host` from eating the `$host` inside `$host_name` is
/// `template_token_is_bounded`, not this order: the character after the short
/// token is `_`, so the match is rejected as unbounded. The longest-first
/// order is kept because it makes the intent readable, but the boundary check
/// is what carries the guarantee — covered by
/// `target_scoped_latest_replaces_specific_host_tokens`.
fn host_substitutions(
    host: &DiscoveryHost,
    default_value: &str,
    escape_for_promql: bool,
) -> [(&'static str, String); 8] {
    let render = |value: &str| {
        if escape_for_promql {
            escape_promql_string_fragment(value)
        } else {
            value.to_string()
        }
    };
    let host_name = render(&host.host_name);
    let host_address = render(&host.host_address);
    let host_id = host.host_id.to_string();
    let default_value = render(default_value);
    [
        ("${host_name}", host_name.clone()),
        ("$host_name", host_name),
        ("${host_address}", host_address.clone()),
        ("$host_address", host_address),
        ("${host_id}", host_id.clone()),
        ("$host_id", host_id),
        ("${host}", default_value.clone()),
        ("$host", default_value),
    ]
}

/// The substitution that starts at this position, if any.
///
/// Split out so the render loop reads as "token or character" instead of
/// nesting the token scan inside the character walk.
fn matching_substitution<'a>(
    remaining: &str,
    substitutions: &'a [(&'static str, String); 8],
) -> Option<(&'static str, &'a str)> {
    substitutions
        .iter()
        .find(|(token, _)| {
            remaining.starts_with(token) && template_token_is_bounded(remaining, token)
        })
        .map(|(token, value)| (*token, value.as_str()))
}

fn render_host_template(
    template: &str,
    host: &DiscoveryHost,
    default_value: &str,
    escape_for_promql: bool,
) -> String {
    if !template.contains('$') {
        return template.to_string();
    }
    let substitutions = host_substitutions(host, default_value, escape_for_promql);

    let mut rendered = String::with_capacity(template.len());
    let mut index = 0;
    while index < template.len() {
        let remaining = &template[index..];
        if let Some((token, value)) = matching_substitution(remaining, &substitutions) {
            rendered.push_str(value);
            index += token.len();
            continue;
        }
        // `index < template.len()` guarantees a remaining char; the `else`
        // arm only guards against a future invariant break.
        let Some(ch) = remaining.chars().next() else {
            break;
        };
        rendered.push(ch);
        index += ch.len_utf8();
    }
    rendered
}

fn template_token_is_bounded(remaining: &str, token: &str) -> bool {
    if token.starts_with("${") {
        return true;
    }
    match remaining[token.len()..].chars().next() {
        Some(ch) => !ch.is_ascii_alphanumeric() && ch != '_',
        None => true,
    }
}

pub fn render_zabbix_host_selector(template: &str, host: &DiscoveryHost) -> String {
    render_host_template(template, host, &host.host_name, false)
}

fn escape_promql_string_fragment(value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::super::host::sample_host;
    use super::*;

    #[test]
    fn target_scoped_latest_renders_host_placeholders_after_selection() {
        let host = sample_host();
        assert_eq!(
            render_host_metric_query(
                "up{piping_host_name=\"$host\",job=\"node\"} or up{instance=\"${host}\"}",
                &host
            ),
            "up{piping_host_name=\"db-01\",job=\"node\"} or up{instance=\"db-01\"}"
        );
    }

    #[test]
    fn target_scoped_latest_escapes_host_name_for_promql_string_fragment() {
        let mut host = sample_host();
        host.host_name = "edge\"a\\b".to_string();
        assert_eq!(
            render_host_metric_query("up{piping_host_name=\"$host\"}", &host),
            "up{piping_host_name=\"edge\\\"a\\\\b\"}"
        );
    }

    #[test]
    fn target_scoped_latest_leaves_queries_without_host_placeholder_unchanged() {
        assert_eq!(
            render_host_metric_query("up{job=\"node\"}", &sample_host()),
            "up{job=\"node\"}"
        );
    }

    #[test]
    fn target_scoped_latest_replaces_specific_host_tokens() {
        assert_eq!(
            render_host_metric_query(
                "up{piping_host_name=\"$host\",address=\"$host_address\"}",
                &sample_host()
            ),
            "up{piping_host_name=\"db-01\",address=\"10.0.0.10\"}"
        );
    }
}
