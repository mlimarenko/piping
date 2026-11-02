use base64::{Engine as _, engine::general_purpose::URL_SAFE};
use chrono::Utc;
use openssl::memcmp;
use openssl::pkey::PKey;
use openssl::rand::rand_bytes;
use openssl::sha::sha256;
use openssl::sign::Signer;
use openssl::symm::{Cipher, decrypt, encrypt};
use serde_json::json;
use uuid::Uuid;

use crate::errors::AppError;
use crate::integrations::proxy::shared_secret_provider::ProxySharedSecretProvider;

pub fn create_proxy_ws_token(
    shared_secret: &str,
    connection_id: Uuid,
    user_id: Uuid,
) -> Result<String, AppError> {
    if shared_secret.trim().is_empty() {
        return Err(AppError::InvalidConfig(
            "Proxy shared secret is required for websocket tokens".to_string(),
        ));
    }
    let payload = json!({
        "connection_id": connection_id.to_string(),
        "user_id": user_id.to_string(),
    });
    let payload_bytes = serde_json::to_vec(&payload)
        .map_err(|error| AppError::Internal(format!("Failed to encode proxy token: {}", error)))?;

    let key = sha256(shared_secret.as_bytes());
    let (signing_key, encryption_key) = key.split_at(16);

    let mut iv = [0u8; 16];
    rand_bytes(&mut iv).map_err(|error| {
        AppError::External(format!("Failed to generate proxy token IV: {}", error))
    })?;

    let ciphertext = encrypt(
        Cipher::aes_128_cbc(),
        encryption_key,
        Some(&iv),
        &payload_bytes,
    )
    .map_err(|error| AppError::External(format!("Failed to encrypt proxy token: {}", error)))?;

    let timestamp = Utc::now().timestamp() as u64;
    let mut token_body = Vec::with_capacity(1 + 8 + 16 + ciphertext.len() + 32);
    token_body.push(0x80);
    token_body.extend_from_slice(&timestamp.to_be_bytes());
    token_body.extend_from_slice(&iv);
    token_body.extend_from_slice(&ciphertext);

    let hmac = sign_hmac(signing_key, &token_body)?;
    token_body.extend_from_slice(&hmac);

    Ok(URL_SAFE.encode(token_body))
}

#[derive(Debug, Clone)]
pub struct ProxyWsTokenPayload {
    pub connection_id: Uuid,
    pub user_id: Uuid,
}

pub fn validate_proxy_ws_token(
    shared_secret: &str,
    token: &str,
    expected_connection_id: Uuid,
    ttl_seconds: u64,
) -> Result<Option<ProxyWsTokenPayload>, AppError> {
    if shared_secret.trim().is_empty() {
        return Err(AppError::InvalidConfig(
            "Proxy shared secret is required for websocket tokens".to_string(),
        ));
    }
    let raw = URL_SAFE.decode(token.as_bytes()).map_err(|error| {
        AppError::Validation(format!("Invalid proxy token encoding: {}", error))
    })?;
    if raw.len() < 1 + 8 + 16 + 32 {
        return Ok(None);
    }

    let version = raw[0];
    if version != 0x80 {
        return Ok(None);
    }
    let timestamp = u64::from_be_bytes(
        raw[1..9]
            .try_into()
            .map_err(|_| AppError::Validation("Invalid proxy token timestamp".to_string()))?,
    );
    let now = Utc::now().timestamp() as u64;
    if now.saturating_sub(timestamp) > ttl_seconds {
        return Ok(None);
    }

    let iv = &raw[9..25];
    let cipher_and_hmac = &raw[25..];
    if cipher_and_hmac.len() <= 32 {
        return Ok(None);
    }
    let (ciphertext, hmac) = cipher_and_hmac.split_at(cipher_and_hmac.len() - 32);

    let key = sha256(shared_secret.as_bytes());
    let (signing_key, encryption_key) = key.split_at(16);
    let mut token_body = Vec::with_capacity(1 + 8 + 16 + ciphertext.len());
    token_body.push(version);
    token_body.extend_from_slice(&timestamp.to_be_bytes());
    token_body.extend_from_slice(iv);
    token_body.extend_from_slice(ciphertext);
    let expected_hmac = sign_hmac(signing_key, &token_body)?;
    if !memcmp::eq(&expected_hmac, hmac) {
        return Ok(None);
    }

    let plaintext = decrypt(Cipher::aes_128_cbc(), encryption_key, Some(iv), ciphertext)
        .map_err(|error| AppError::External(format!("Failed to decrypt proxy token: {}", error)))?;
    let payload: serde_json::Value = serde_json::from_slice(&plaintext).map_err(|error| {
        AppError::External(format!("Failed to decode proxy token payload: {}", error))
    })?;
    let connection_id = payload
        .get("connection_id")
        .and_then(|value| value.as_str())
        .and_then(|value| Uuid::parse_str(value).ok());
    let user_id = payload
        .get("user_id")
        .and_then(|value| value.as_str())
        .and_then(|value| Uuid::parse_str(value).ok());
    let (Some(connection_id), Some(user_id)) = (connection_id, user_id) else {
        return Ok(None);
    };
    if connection_id != expected_connection_id {
        return Ok(None);
    }

    Ok(Some(ProxyWsTokenPayload {
        connection_id,
        user_id,
    }))
}

pub fn validate_proxy_ws_token_from_provider(
    provider: &dyn ProxySharedSecretProvider,
    token: &str,
    expected_connection_id: Uuid,
    ttl_seconds: u64,
) -> Result<Option<ProxyWsTokenPayload>, AppError> {
    let credential = provider.get()?;
    validate_proxy_ws_token(
        &credential.secret,
        token,
        expected_connection_id,
        ttl_seconds,
    )
}

fn sign_hmac(key: &[u8], data: &[u8]) -> Result<Vec<u8>, AppError> {
    let key = PKey::hmac(key).map_err(|error| {
        AppError::External(format!("Failed to build proxy token HMAC key: {}", error))
    })?;
    let mut signer =
        Signer::new(openssl::hash::MessageDigest::sha256(), &key).map_err(|error| {
            AppError::External(format!("Failed to initialize HMAC signer: {}", error))
        })?;
    signer
        .update(data)
        .map_err(|error| AppError::External(format!("Failed to sign proxy token: {}", error)))?;
    signer
        .sign_to_vec()
        .map_err(|error| AppError::External(format!("Failed to finalize proxy token: {}", error)))
}
