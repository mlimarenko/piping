//! SQLx-backed assistant repository and its trait re-export.
pub mod assistant_repository;

pub use assistant_repository::{AssistantRepository, SqlxAssistantRepository};
