pub fn normalize_rdp_username_domain(
    username: Option<String>,
    explicit_domain: Option<String>,
) -> Result<(Option<String>, Option<String>), &'static str> {
    let Some(raw_username) = username else {
        return Ok((None, explicit_domain));
    };

    let Some((domain_part, user_part)) = raw_username.split_once('\\') else {
        return Ok((Some(raw_username), explicit_domain));
    };

    if domain_part.trim().is_empty() || user_part.trim().is_empty() || user_part.contains('\\') {
        return Err("Invalid RDP credentials.username domain-qualified value");
    }

    let derived_domain = domain_part.trim().to_string();
    let normalized_username = user_part.trim().to_string();
    if let Some(domain) = explicit_domain {
        if !domain.eq_ignore_ascii_case(&derived_domain) {
            return Err("RDP credentials.domain conflicts with credentials.username domain");
        }
        return Ok((Some(normalized_username), Some(domain)));
    }

    Ok((Some(normalized_username), Some(derived_domain)))
}

#[cfg(test)]
mod tests {
    use super::normalize_rdp_username_domain;

    #[test]
    fn keeps_upn_username_opaque() {
        let (username, domain) =
            normalize_rdp_username_domain(Some("operator@example.test".to_string()), None)
                .expect("the UPN should be valid");

        assert_eq!(username.as_deref(), Some("operator@example.test"));
        assert_eq!(domain, None);
    }

    #[test]
    fn splits_backslash_domain_username() {
        let (username, domain) =
            normalize_rdp_username_domain(Some("GRAD\\operator".to_string()), None)
                .expect("domain-qualified username should be valid");

        assert_eq!(username.as_deref(), Some("operator"));
        assert_eq!(domain.as_deref(), Some("GRAD"));
    }

    #[test]
    fn accepts_case_insensitive_matching_explicit_domain() {
        let (username, domain) = normalize_rdp_username_domain(
            Some("GRAD\\operator".to_string()),
            Some("grad".to_string()),
        )
        .expect("matching explicit domain should be valid");

        assert_eq!(username.as_deref(), Some("operator"));
        assert_eq!(domain.as_deref(), Some("grad"));
    }

    #[test]
    fn rejects_conflicting_explicit_domain() {
        let result = normalize_rdp_username_domain(
            Some("GRAD\\operator".to_string()),
            Some("OTHER".to_string()),
        );

        assert!(matches!(result, Err(message) if message.contains("conflicts")));
    }
}
