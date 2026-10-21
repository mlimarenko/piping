//! Secret encryption module and its re-exports.
pub mod encryption;

pub use encryption::{
    configure_cipher_key_material_ref, decrypt_secret, encrypt_secret, is_encrypted_secret_envelope,
};
