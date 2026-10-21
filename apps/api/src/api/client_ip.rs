use std::net::{IpAddr, SocketAddr};

use axum::http::HeaderMap;

/// Extract canonical client IP from proxy headers.
///
/// Priority:
/// 1. `x-real-ip` (set by trusted edge proxy)
/// 2. `x-forwarded-for` (first valid address in list)
/// 3. `forwarded` (`for=` parameter, RFC 7239)
pub fn extract_client_ip(headers: &HeaderMap) -> Option<String> {
    header_value(headers, "x-real-ip")
        .and_then(parse_ip_literal)
        .or_else(|| header_value(headers, "x-forwarded-for").and_then(parse_x_forwarded_for))
        .or_else(|| header_value(headers, "forwarded").and_then(parse_forwarded_for))
        .map(|ip| ip.to_string())
}

fn header_value<'a>(headers: &'a HeaderMap, key: &str) -> Option<&'a str> {
    headers.get(key).and_then(|value| value.to_str().ok())
}

fn parse_x_forwarded_for(raw: &str) -> Option<IpAddr> {
    raw.split(',').map(str::trim).find_map(parse_ip_literal)
}

fn parse_forwarded_for(raw: &str) -> Option<IpAddr> {
    raw.split(',')
        .map(str::trim)
        .flat_map(|entry| entry.split(';').map(str::trim))
        .find_map(|part| {
            if part.len() < 4 || !part[..4].eq_ignore_ascii_case("for=") {
                return None;
            }

            let value = part[4..].trim();
            parse_ip_literal(value)
        })
}

fn parse_ip_literal(raw: &str) -> Option<IpAddr> {
    let trimmed = raw.trim().trim_matches('"');
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("unknown") || trimmed.starts_with('_') {
        return None;
    }

    if let Some(ipv6_host) = trimmed
        .strip_prefix('[')
        .and_then(|value| value.split(']').next())
    {
        return ipv6_host.parse::<IpAddr>().ok();
    }

    if let Ok(ip) = trimmed.parse::<IpAddr>() {
        return Some(ip);
    }

    trimmed.parse::<SocketAddr>().ok().map(|addr| addr.ip())
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue};

    use super::extract_client_ip;

    #[test]
    fn prefers_x_real_ip() {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", HeaderValue::from_static("203.0.113.10"));
        headers.insert(
            "x-forwarded-for",
            HeaderValue::from_static("198.51.100.10, 172.18.0.6"),
        );

        let client_ip = extract_client_ip(&headers);
        assert_eq!(client_ip.as_deref(), Some("203.0.113.10"));
    }

    #[test]
    fn reads_first_ip_from_x_forwarded_for() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            HeaderValue::from_static("198.51.100.10, 172.18.0.6"),
        );

        let client_ip = extract_client_ip(&headers);
        assert_eq!(client_ip.as_deref(), Some("198.51.100.10"));
    }

    #[test]
    fn parses_forwarded_header() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "forwarded",
            HeaderValue::from_static("for=\"[2001:db8::10]:443\";proto=https"),
        );

        let client_ip = extract_client_ip(&headers);
        assert_eq!(client_ip.as_deref(), Some("2001:db8::10"));
    }

    #[test]
    fn returns_none_when_no_ip_headers() {
        let headers = HeaderMap::new();
        let client_ip = extract_client_ip(&headers);
        assert_eq!(client_ip, None);
    }
}
