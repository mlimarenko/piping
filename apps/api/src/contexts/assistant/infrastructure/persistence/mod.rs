//! Assistant persistence adapters re-exported from the repositories module.
pub mod repositories;

pub use repositories::{AssistantRepository, SqlxAssistantRepository};
