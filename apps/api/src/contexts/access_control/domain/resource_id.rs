use thiserror::Error;

use super::ResourceType;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceId {
    pub resource_type: ResourceType,
    pub resource_id: String,
}

#[derive(Debug, Error)]
pub enum ResourceIdError {
    #[error("resource id must be in '<type>:<id>' format")]
    InvalidFormat,
    #[error("resource type is not recognized")]
    UnknownType,
    #[error("resource id cannot be empty")]
    EmptyId,
}

impl ResourceId {
    pub fn parse(value: &str) -> Result<Self, ResourceIdError> {
        let mut parts = value.splitn(2, ':');
        let resource_type = parts.next().ok_or(ResourceIdError::InvalidFormat)?.trim();
        let resource_id = parts.next().ok_or(ResourceIdError::InvalidFormat)?.trim();

        if resource_type.is_empty() || resource_id.is_empty() {
            return Err(ResourceIdError::InvalidFormat);
        }

        let resource_type =
            ResourceType::parse(resource_type).ok_or(ResourceIdError::UnknownType)?;

        Ok(Self {
            resource_type,
            resource_id: resource_id.to_string(),
        })
    }

    pub fn canonical(&self) -> String {
        format!("{}:{}", self.resource_type.as_str(), self.resource_id)
    }
}
