//! Dataflow catalog entities: application roles, applications, hierarchical
//! flow groups and reusable address groups.
//!
//! A flow group is an organisational container; it carries the effective status
//! and inherited tag projection derived from its ancestor chain. An address
//! group is a reusable, ordered member set: each member is a flow selector row
//! (`side = 'member'`), so member validation is [`DataflowEndpointInput`]'s, not
//! a second copy of the selector rules.

use serde::Serialize;
use uuid::Uuid;

use super::dataflow::{DataflowAddressMode, DataflowEndpointInput, DataflowStatus, EndpointType};
use crate::errors::AppError;

pub const MAX_NAME_CHARS: usize = 200;
pub const MAX_TEXT_CHARS: usize = 4000;
/// Upper bound of one address group's ordered member list. NetBox itself allows
/// an unbounded list; the local model keeps a finite bound that covers real
/// estates (an observed group had 94 members) without permitting an
/// unbounded request payload.
pub const MAX_ADDRESS_GROUP_MEMBERS: usize = 256;

/// Trim a name and enforce the catalog name bounds (`btrim` 1..200 characters).
///
/// # Errors
///
/// [`AppError::Validation`] when the trimmed name is empty or too long.
pub fn normalize_name(name: &mut String) -> Result<(), AppError> {
    *name = name.trim().to_owned();
    if name.is_empty() || name.chars().count() > MAX_NAME_CHARS {
        return Err(AppError::Validation(
            "name must contain 1..200 characters".into(),
        ));
    }
    Ok(())
}

/// Enforce the shared free-text bound on a nullable description or comment.
///
/// # Errors
///
/// [`AppError::Validation`] when the text exceeds 4000 characters.
pub fn normalize_optional_text(value: &Option<String>, label: &str) -> Result<(), AppError> {
    if value
        .as_ref()
        .is_some_and(|text| text.chars().count() > MAX_TEXT_CHARS)
    {
        return Err(AppError::Validation(format!(
            "{label} must contain at most 4000 characters"
        )));
    }
    Ok(())
}

/// Canonicalize a direct tag membership array: canonical lowercase UUID strings
/// only, sorted and deduplicated.
///
/// # Errors
///
/// [`AppError::Validation`] for a non-canonical UUID string.
pub fn canonical_tag_ids(tags: &mut Vec<String>) -> Result<(), AppError> {
    for tag in tags.iter() {
        let parsed = Uuid::parse_str(tag)
            .map_err(|_| AppError::Validation("tags must be canonical UUID strings".into()))?;
        if parsed.to_string() != *tag {
            return Err(AppError::Validation(
                "tags must be canonical lowercase UUID strings".into(),
            ));
        }
    }
    tags.sort_unstable();
    tags.dedup();
    Ok(())
}

/// The catalog stores custom field data as a JSON object; the definition-driven
/// value contract belongs to the custom-field subsystem.
///
/// # Errors
///
/// [`AppError::Validation`] when the value is not a JSON object.
pub fn normalize_custom_field_data(value: &serde_json::Value) -> Result<(), AppError> {
    if !value.is_object() {
        return Err(AppError::Validation(
            "custom_field_data must be a JSON object".into(),
        ));
    }
    Ok(())
}

/// Effective-status priority: `disabled` beats `draft` beats `active`.
///
/// [`DataflowStatus`]'s derived `Ord` orders variants by declaration, which is
/// not this priority; callers must compare through this rank.
#[must_use]
pub fn status_rank(status: DataflowStatus) -> u8 {
    match status {
        DataflowStatus::Active => 0,
        DataflowStatus::Draft => 1,
        DataflowStatus::Disabled => 2,
    }
}

/// Application role: a real catalog entity, deliberately not overloaded onto
/// tags. It owns no tenant, tag or custom-field metadata.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ApplicationRole {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub owner_id: Option<Uuid>,
}

impl ApplicationRole {
    /// # Errors
    ///
    /// [`AppError::Validation`] for an invalid name, description or comment.
    pub fn normalize(&mut self) -> Result<(), AppError> {
        normalize_name(&mut self.name)?;
        normalize_optional_text(&self.description, "description")?;
        normalize_optional_text(&self.comments, "comments")?;
        Ok(())
    }
}

/// Application container a group or flow may belong to.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct Application {
    pub id: Uuid,
    pub name: String,
    pub role_id: Option<Uuid>,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub owner_id: Option<Uuid>,
    pub tenant_id: Option<Uuid>,
    pub tags: Vec<String>,
    pub custom_field_data: serde_json::Value,
}

impl Application {
    /// # Errors
    ///
    /// [`AppError::Validation`] for invalid shared metadata.
    pub fn normalize(&mut self) -> Result<(), AppError> {
        normalize_name(&mut self.name)?;
        normalize_optional_text(&self.description, "description")?;
        normalize_optional_text(&self.comments, "comments")?;
        canonical_tag_ids(&mut self.tags)?;
        normalize_custom_field_data(&self.custom_field_data)?;
        Ok(())
    }
}

/// Hierarchical flow group.
///
/// `status` is the stored status. `effective_status`, `blocked_by_group_id`,
/// `inherited_tags` and `effective_tags` are read projections of the ancestor
/// chain and are never written.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct DataflowGroup {
    pub id: Uuid,
    pub parent_id: Option<Uuid>,
    pub application_id: Option<Uuid>,
    pub name: String,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub status: DataflowStatus,
    pub owner_id: Option<Uuid>,
    pub tenant_id: Option<Uuid>,
    pub tags: Vec<String>,
    pub custom_field_data: serde_json::Value,
    pub effective_status: DataflowStatus,
    /// Nearest ancestor whose status explains a non-active effective status;
    /// `None` when the row's own status is the reason.
    pub blocked_by_group_id: Option<Uuid>,
    /// Ancestor-group tag ids only.
    pub inherited_tags: Vec<Uuid>,
    /// Sorted union of the direct and inherited tag ids.
    pub effective_tags: Vec<Uuid>,
}

impl DataflowGroup {
    /// # Errors
    ///
    /// [`AppError::Validation`] for invalid shared metadata.
    pub fn normalize(&mut self) -> Result<(), AppError> {
        normalize_name(&mut self.name)?;
        normalize_optional_text(&self.description, "description")?;
        normalize_optional_text(&self.comments, "comments")?;
        canonical_tag_ids(&mut self.tags)?;
        normalize_custom_field_data(&self.custom_field_data)?;
        Ok(())
    }

    /// Clear the derived projection so a caller can never persist or trust a
    /// stale effective state.
    pub fn reset_effective_projection(&mut self) {
        self.effective_status = self.status;
        self.blocked_by_group_id = None;
        self.inherited_tags.clear();
        self.effective_tags.clear();
    }
}

/// One ordered address-group member. The selector keeps the canonical typed
/// endpoint shape, extended with the member's position.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct AddressGroupMember {
    #[serde(flatten)]
    pub selector: DataflowEndpointInput,
    pub position: i16,
    /// Resolved display label for the referenced object, when it still exists.
    pub label: Option<String>,
}

impl AddressGroupMember {
    /// # Errors
    ///
    /// [`AppError::Validation`] for a nested group, a builtin wildcard or an
    /// invalid selector.
    pub fn normalize(&mut self) -> Result<(), AppError> {
        if self.selector.object_type.is_address_group() {
            return Err(AppError::Validation(
                "Address-group members cannot reference another address group".into(),
            ));
        }
        if self.selector.object_type.is_builtin() {
            return Err(AppError::Validation(
                "Address-group members must use a typed endpoint reference, not a builtin wildcard"
                    .into(),
            ));
        }
        self.selector.normalize()
    }
}

/// Reusable address group with its ordered member set.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct AddressGroup {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub comments: Option<String>,
    pub owner_id: Option<Uuid>,
    pub tenant_id: Option<Uuid>,
    pub tags: Vec<String>,
    pub custom_field_data: serde_json::Value,
    pub members: Vec<AddressGroupMember>,
}

impl AddressGroup {
    /// Enforce the shared metadata bounds without requiring a member set.
    ///
    /// # Errors
    ///
    /// [`AppError::Validation`] for invalid shared metadata.
    pub fn normalize_metadata(&mut self) -> Result<(), AppError> {
        normalize_name(&mut self.name)?;
        normalize_optional_text(&self.description, "description")?;
        normalize_optional_text(&self.comments, "comments")?;
        canonical_tag_ids(&mut self.tags)?;
        normalize_custom_field_data(&self.custom_field_data)?;
        Ok(())
    }

    /// # Errors
    ///
    /// [`AppError::Validation`] for invalid shared metadata or members.
    pub fn normalize(&mut self) -> Result<(), AppError> {
        self.normalize_metadata()?;
        normalize_members(&mut self.members)
    }
}

/// Validate a supplied member set and reassign positions from array order.
///
/// The request array is the only ordering authority: a client cannot supply a
/// position that disagrees with the order it sent.
///
/// # Errors
///
/// [`AppError::Validation`] when the set exceeds 256 members or holds an
/// invalid member. An empty set is valid: NetBox permits a group without
/// members and the import must represent it faithfully.
pub fn normalize_members(members: &mut [AddressGroupMember]) -> Result<(), AppError> {
    if members.len() > MAX_ADDRESS_GROUP_MEMBERS {
        return Err(AppError::Validation(
            "members must contain at most 256 selectors".into(),
        ));
    }
    for (index, member) in members.iter_mut().enumerate() {
        member.position = i16::try_from(index + 1)
            .map_err(|_| AppError::Validation("members exceed the selector bound".into()))?;
        member.normalize()?;
    }
    Ok(())
}

/// Parse a stored selector address mode.
///
/// # Errors
///
/// [`AppError::Database`] for a value the schema does not allow.
pub fn parse_address_mode(raw: &str) -> Result<DataflowAddressMode, AppError> {
    match raw {
        "default" => Ok(DataflowAddressMode::Default),
        "selected" => Ok(DataflowAddressMode::Selected),
        "all" => Ok(DataflowAddressMode::All),
        "oob" => Ok(DataflowAddressMode::Oob),
        _ => Err(AppError::Database(
            "Invalid selector address mode stored for catalog member".into(),
        )),
    }
}

/// Parse a stored flow/group status.
///
/// # Errors
///
/// [`AppError::Database`] for a value the schema does not allow.
pub fn parse_status(raw: &str) -> Result<DataflowStatus, AppError> {
    match raw {
        "draft" => Ok(DataflowStatus::Draft),
        "active" => Ok(DataflowStatus::Active),
        "disabled" => Ok(DataflowStatus::Disabled),
        _ => Err(AppError::Database(
            "Invalid status stored for catalog entity".into(),
        )),
    }
}

/// Parse one stored tag id from the canonical text array form.
///
/// # Errors
///
/// [`AppError::Database`] when a stored tag id is not a UUID.
pub fn parse_stored_tag(raw: &str) -> Result<Uuid, AppError> {
    Uuid::parse_str(raw)
        .map_err(|_| AppError::Database("Invalid tag id stored on catalog entity".into()))
}

/// The typed selector kind must name a real endpoint column; builtins have none.
#[must_use]
pub fn selector_column(object_type: EndpointType) -> Option<&'static str> {
    let column = object_type.column();
    (!column.is_empty()).then_some(column)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_priority_is_disabled_then_draft_then_active() {
        assert!(status_rank(DataflowStatus::Disabled) > status_rank(DataflowStatus::Draft));
        assert!(status_rank(DataflowStatus::Draft) > status_rank(DataflowStatus::Active));
    }

    #[test]
    fn names_and_texts_are_bounded_after_trimming() {
        let mut name = "  core  ".to_string();
        normalize_name(&mut name).expect("trimmed name is valid");
        assert_eq!(name, "core");

        let mut blank = "   ".to_string();
        assert!(normalize_name(&mut blank).is_err());

        let long = "x".repeat(MAX_TEXT_CHARS + 1);
        assert!(normalize_optional_text(&Some(long), "comments").is_err());
        assert!(normalize_optional_text(&Some(String::new()), "comments").is_ok());
    }

    #[test]
    fn address_group_members_reject_nesting_and_builtin_wildcards() {
        let mut nested = AddressGroupMember {
            selector: DataflowEndpointInput {
                object_type: EndpointType::AddressGroup,
                object_id: Some(Uuid::now_v7()),
                address_mode: DataflowAddressMode::Default,
                ipaddresses: Vec::new(),
                tag_object_type: None,
            },
            position: 1,
            label: None,
        };
        assert!(nested.normalize().is_err());

        let mut builtin = AddressGroupMember {
            selector: DataflowEndpointInput {
                object_type: EndpointType::All,
                object_id: None,
                address_mode: DataflowAddressMode::Default,
                ipaddresses: Vec::new(),
                tag_object_type: None,
            },
            position: 1,
            label: None,
        };
        assert!(builtin.normalize().is_err());

        let mut device = AddressGroupMember {
            selector: DataflowEndpointInput {
                object_type: EndpointType::Device,
                object_id: Some(Uuid::now_v7()),
                address_mode: DataflowAddressMode::Oob,
                ipaddresses: Vec::new(),
                tag_object_type: None,
            },
            position: 9,
            label: None,
        };
        normalize_members(std::slice::from_mut(&mut device)).expect("explicit device member");
        assert_eq!(device.position, 1);
    }
}
