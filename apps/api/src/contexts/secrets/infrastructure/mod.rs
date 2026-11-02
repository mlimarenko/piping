//! Secret crypto and persistence infrastructure modules with their re-exports.
pub mod crypto;
pub mod persistence;

pub use crypto::{decrypt_secret, encrypt_secret};
pub use persistence::{DbSecretStore, SecretRepository, SecretScopeSecrets, SqlxSecretRepository};
