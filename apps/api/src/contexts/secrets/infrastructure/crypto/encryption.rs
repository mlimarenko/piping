use std::env;

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use once_cell::sync::OnceCell;
use openssl::rand::rand_bytes;
use openssl::symm::{Cipher, decrypt_aead, encrypt_aead};

use crate::errors::AppError;

const DEFAULT_KEY_MATERIAL_REF: &str = "env:PIPING_SECRETS__CIPHER_KEY_B64";
const INLINE_KEY_MATERIAL_PREFIX: &str = "inline:";
const ENVELOPE_PREFIX: &str = "enc:v1:";
const NONCE_SIZE: usize = 12;
const TAG_SIZE: usize = 16;
const KEY_SIZE: usize = 32;

static CIPHER_KEY: OnceCell<[u8; KEY_SIZE]> = OnceCell::new();
static KEY_MATERIAL_REF: OnceCell<String> = OnceCell::new();

pub fn is_encrypted_secret_envelope(value: &str) -> bool {
    value.starts_with(ENVELOPE_PREFIX)
}

pub fn configure_cipher_key_material_ref(reference: &str) -> Result<(), AppError> {
    let normalized = reference.trim();
    if normalized.is_empty() {
        return Err(AppError::InvalidConfig(
            "secrets.cipher.key_material_ref cannot be empty".to_string(),
        ));
    }

    if let Some(existing) = KEY_MATERIAL_REF.get() {
        if existing == normalized {
            return Ok(());
        }
        return Err(AppError::InvalidConfig(format!(
            "secrets cipher key material reference is already configured as '{}'",
            existing
        )));
    }

    // OnceCell::set is not atomic with our "get then set" check above.
    // Integration tests (and some runtimes) may configure in parallel, so we must
    // tolerate a benign race where another thread sets the same value first.
    if KEY_MATERIAL_REF.set(normalized.to_string()).is_ok() {
        return Ok(());
    }

    let Some(existing) = KEY_MATERIAL_REF.get() else {
        return Err(AppError::InvalidConfig(
            "failed to configure secrets cipher key material reference".to_string(),
        ));
    };
    if existing == normalized {
        Ok(())
    } else {
        Err(AppError::InvalidConfig(format!(
            "secrets cipher key material reference is already configured as '{}'",
            existing
        )))
    }
}

fn current_key_material_ref() -> &'static str {
    KEY_MATERIAL_REF
        .get()
        .map(String::as_str)
        .unwrap_or(DEFAULT_KEY_MATERIAL_REF)
}

fn resolve_cipher_key() -> Result<&'static [u8; KEY_SIZE], AppError> {
    CIPHER_KEY.get_or_try_init(|| resolve_cipher_key_from_reference(current_key_material_ref()))
}

fn resolve_cipher_key_from_reference(reference: &str) -> Result<[u8; KEY_SIZE], AppError> {
    let encoded = load_cipher_key_b64(reference)?;
    decode_cipher_key_material(reference, &encoded)
}

fn decode_cipher_key_material(reference: &str, encoded: &str) -> Result<[u8; KEY_SIZE], AppError> {
    let decoded = STANDARD.decode(encoded.as_bytes()).map_err(|error| {
        AppError::InvalidConfig(format!(
            "Invalid base64 in {}: {}",
            describe_reference(reference),
            error
        ))
    })?;
    decoded.try_into().map_err(|_| {
        AppError::InvalidConfig(format!(
            "{} must decode to {} bytes",
            describe_reference(reference),
            KEY_SIZE
        ))
    })
}

fn describe_reference(reference: &str) -> String {
    if let Some(key) = reference.strip_prefix("env:") {
        return key.trim().to_string();
    }
    if reference.starts_with(INLINE_KEY_MATERIAL_PREFIX) {
        return "inline secrets cipher key material".to_string();
    }
    reference.to_string()
}

fn load_cipher_key_b64(reference: &str) -> Result<String, AppError> {
    if let Some(encoded) = reference.strip_prefix(INLINE_KEY_MATERIAL_PREFIX) {
        let encoded = encoded.trim();
        if encoded.is_empty() {
            return Err(AppError::InvalidConfig(
                "secrets.cipher.key_material_ref inline value is empty".to_string(),
            ));
        }
        return Ok(encoded.to_string());
    }

    let Some(env_key) = reference.strip_prefix("env:") else {
        return Err(AppError::InvalidConfig(format!(
            "Unsupported secrets.cipher.key_material_ref '{}'; expected env:<ENV_VAR> or inline:<BASE64>",
            reference
        )));
    };
    let env_key = env_key.trim();
    if env_key.is_empty() {
        return Err(AppError::InvalidConfig(
            "secrets.cipher.key_material_ref env reference is empty".to_string(),
        ));
    }

    env::var(env_key).map_err(|_| {
        AppError::InvalidConfig(format!(
            "{} is required and must contain a base64-encoded {}-byte key",
            env_key, KEY_SIZE
        ))
    })
}

pub fn encrypt_secret(value: &str) -> Result<String, AppError> {
    let key = resolve_cipher_key()?;
    let mut nonce = [0u8; NONCE_SIZE];
    rand_bytes(&mut nonce).map_err(|error| {
        AppError::Internal(format!("Failed to generate secret nonce: {}", error))
    })?;

    let mut tag = [0u8; TAG_SIZE];
    let ciphertext = encrypt_aead(
        Cipher::aes_256_gcm(),
        key,
        Some(&nonce),
        &[],
        value.as_bytes(),
        &mut tag,
    )
    .map_err(|error| AppError::Internal(format!("Failed to encrypt secret payload: {}", error)))?;

    let mut payload = Vec::with_capacity(NONCE_SIZE + TAG_SIZE + ciphertext.len());
    payload.extend_from_slice(&nonce);
    payload.extend_from_slice(&tag);
    payload.extend_from_slice(&ciphertext);
    let encoded = STANDARD_NO_PAD.encode(payload);

    Ok(format!("{ENVELOPE_PREFIX}{encoded}"))
}

pub fn decrypt_secret(value: &str) -> Result<String, AppError> {
    let encoded = value
        .strip_prefix(ENVELOPE_PREFIX)
        .ok_or_else(|| AppError::Validation("Unsupported secret envelope format".to_string()))?;
    let payload = STANDARD_NO_PAD
        .decode(encoded.as_bytes())
        .map_err(|error| {
            AppError::Validation(format!("Invalid secret envelope payload: {}", error))
        })?;

    if payload.len() <= NONCE_SIZE + TAG_SIZE {
        return Err(AppError::Validation(
            "Secret envelope payload is too short".to_string(),
        ));
    }

    let key = resolve_cipher_key()?;
    let nonce = &payload[..NONCE_SIZE];
    let tag = &payload[NONCE_SIZE..NONCE_SIZE + TAG_SIZE];
    let ciphertext = &payload[NONCE_SIZE + TAG_SIZE..];

    let plaintext = decrypt_aead(
        Cipher::aes_256_gcm(),
        key,
        Some(nonce),
        &[],
        ciphertext,
        tag,
    )
    .map_err(|error| {
        AppError::Validation(format!("Failed to decrypt secret payload: {}", error))
    })?;

    String::from_utf8(plaintext).map_err(|error| {
        AppError::Validation(format!("Secret payload is not valid UTF-8: {}", error))
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Once;

    use super::{
        CIPHER_KEY, DEFAULT_KEY_MATERIAL_REF, decode_cipher_key_material, decrypt_secret,
        encrypt_secret, resolve_cipher_key_from_reference,
    };

    const TEST_KEY_B64: &str = "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=";
    static TEST_ENV_INIT: Once = Once::new();

    fn ensure_test_key_env() {
        TEST_ENV_INIT.call_once(|| {
            let key_result = decode_cipher_key_material(DEFAULT_KEY_MATERIAL_REF, TEST_KEY_B64);
            assert!(
                key_result.is_ok(),
                "failed to decode test key material: {key_result:?}"
            );
            if let Ok(key) = key_result {
                let _ = CIPHER_KEY.set(key);
            }
        });
    }

    #[test]
    fn encrypt_decrypt_round_trip() {
        ensure_test_key_env();
        let plaintext = "piping-secret-roundtrip";
        let encrypt_result = encrypt_secret(plaintext);
        assert!(
            encrypt_result.is_ok(),
            "encrypt should succeed: {encrypt_result:?}"
        );
        let ciphertext = encrypt_result.unwrap_or_default();
        let decrypt_result = decrypt_secret(&ciphertext);
        assert!(
            decrypt_result.is_ok(),
            "decrypt should succeed: {decrypt_result:?}"
        );
        let decrypted = decrypt_result.unwrap_or_default();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn encrypt_output_is_versioned_and_non_identity() {
        ensure_test_key_env();
        let plaintext = "piping-secret-non-identity";
        let encrypt_result = encrypt_secret(plaintext);
        assert!(
            encrypt_result.is_ok(),
            "encrypt should succeed: {encrypt_result:?}"
        );
        let ciphertext = encrypt_result.unwrap_or_default();
        assert_ne!(ciphertext, plaintext);
        assert!(ciphertext.starts_with("enc:v1:"));
    }

    #[test]
    fn decrypt_invalid_payload_fails() {
        ensure_test_key_env();
        assert!(
            matches!(
                &decrypt_secret("enc:v1:not-base64"),
                Err(error) if error.to_string().contains("Invalid secret envelope payload")
            ),
            "expected invalid payload error"
        );
    }

    #[test]
    fn key_resolution_fails_when_env_value_is_missing() {
        assert!(
            matches!(
                &resolve_cipher_key_from_reference("env:PIPING_TEST_MISSING_CIPHER_KEY"),
                Err(error) if error.to_string().contains("PIPING_TEST_MISSING_CIPHER_KEY is required")
            ),
            "expected missing env key error"
        );
    }

    #[test]
    fn key_resolution_fails_on_invalid_base64() {
        let result =
            decode_cipher_key_material("env:PIPING_TEST_INVALID_CIPHER_KEY", "not-base64-value");
        assert!(matches!(&result, Err(error) if error.to_string().contains("Invalid base64")));
    }

    #[test]
    fn default_reference_uses_expected_env_source() {
        assert_eq!(
            DEFAULT_KEY_MATERIAL_REF,
            "env:PIPING_SECRETS__CIPHER_KEY_B64"
        );
    }
}
