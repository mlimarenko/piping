//! Secret persistence module re-exports.
pub mod repositories;

pub use repositories::{DbSecretStore, SecretRepository, SecretScopeSecrets, SqlxSecretRepository};
