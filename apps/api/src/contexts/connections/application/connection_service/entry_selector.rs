use super::*;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) enum EntrySelectorKind {
    Config,
    User,
}

#[derive(Debug, Clone)]
pub(super) struct ConnectionEntrySelector {
    pub(super) kind: EntrySelectorKind,
    pub(super) source_ref: String,
    pub(super) protocol: String,
    pub(super) index: usize,
    pub(super) credential_set_id: Option<Uuid>,
}

pub(super) fn parse_connection_entry_selector(
    value: &str,
) -> Result<Option<ConnectionEntrySelector>, AppError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    let mut parts = trimmed.split(':');
    let prefix = parts.next().unwrap_or_default();
    let kind = match prefix {
        "config" => EntrySelectorKind::Config,
        "user" => EntrySelectorKind::User,
        _ => return Ok(None),
    };

    let source_ref = parts.next().ok_or_else(|| {
        AppError::Validation(format!("Invalid connection entry selector '{}'", trimmed))
    })?;
    let protocol = parts.next().ok_or_else(|| {
        AppError::Validation(format!("Invalid connection entry selector '{}'", trimmed))
    })?;
    let index_raw = parts.next().ok_or_else(|| {
        AppError::Validation(format!("Invalid connection entry selector '{}'", trimmed))
    })?;
    let credential_set_id_raw = parts.next();

    if parts.next().is_some() {
        return Err(AppError::Validation(format!(
            "Invalid connection entry selector '{}'",
            trimmed
        )));
    }

    let protocol = protocol.trim().to_ascii_lowercase();
    let index: usize = index_raw.trim().parse().map_err(|_| {
        AppError::Validation(format!(
            "Invalid connection entry selector index '{}'",
            index_raw.trim()
        ))
    })?;
    let credential_set_id = match credential_set_id_raw.map(str::trim) {
        Some("") => None,
        Some(value) => Some(Uuid::parse_str(value).map_err(|_| {
            AppError::Validation(format!(
                "Invalid connection entry selector credential set id '{}'",
                value
            ))
        })?),
        None => None,
    };

    if kind == EntrySelectorKind::Config && credential_set_id.is_some() {
        return Err(AppError::Validation(format!(
            "Config entry selector cannot include credential set id '{}'",
            trimmed
        )));
    }
    if kind == EntrySelectorKind::User && credential_set_id.is_none() {
        return Err(AppError::Validation(format!(
            "User entry selector must include credential set id '{}'",
            trimmed
        )));
    }

    Ok(Some(ConnectionEntrySelector {
        kind,
        source_ref: source_ref.trim().to_ascii_lowercase(),
        protocol,
        index,
        credential_set_id,
    }))
}
