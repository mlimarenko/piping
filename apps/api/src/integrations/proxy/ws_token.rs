use base64::{Engine as _, engine::general_purpose::URL_SAFE};
use chrono::Utc;
use openssl::memcmp;
use openssl::pkey::PKey;
use openssl::rand::rand_bytes;
use openssl::sha::sha256;
use openssl::sign::Signer;
use openssl::symm::{Cipher, decrypt, encrypt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::errors::AppError;
use crate::integrations::proxy::shared_secret_provider::ProxySharedSecretProvider;

pub fn create_proxy_ws_token(
    shared_secret: &str,
    connection_id: Uuid,
    user_id: Uuid,
) -> Result<String, AppError> {
    create_scoped_proxy_ws_token(
        shared_secret,
        connection_id,
        user_id,
        ProxyWsMode::Operator,
        None,
    )
}

/// Issues an API-internal credential for one observer leg of one runtime generation.
pub fn create_proxy_observer_ws_token(
    shared_secret: &str,
    connection_id: Uuid,
    user_id: Uuid,
    generation: Uuid,
) -> Result<String, AppError> {
    create_scoped_proxy_ws_token(
        shared_secret,
        connection_id,
        user_id,
        ProxyWsMode::Observe,
        Some(generation),
    )
}

fn create_scoped_proxy_ws_token(
    shared_secret: &str,
    connection_id: Uuid,
    user_id: Uuid,
    mode: ProxyWsMode,
    generation: Option<Uuid>,
) -> Result<String, AppError> {
    if shared_secret.trim().is_empty() {
        return Err(AppError::InvalidConfig(
            "Proxy shared secret is required for websocket tokens".to_string(),
        ));
    }
    let payload = json!({
        "connection_id": connection_id.to_string(),
        "user_id": user_id.to_string(),
        "mode": mode,
        "generation": generation,
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

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyWsMode {
    Operator,
    Observe,
}

#[derive(Debug, Clone)]
pub struct ProxyWsTokenPayload {
    pub connection_id: Uuid,
    pub user_id: Uuid,
    pub mode: ProxyWsMode,
    pub generation: Option<Uuid>,
    pub expires_at: u64,
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
    if token.len() > 8192 {
        return Ok(None);
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
    if timestamp > now.saturating_add(5) || now >= timestamp.saturating_add(ttl_seconds) {
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
    let Some(mode) = payload
        .get("mode")
        .and_then(|value| serde_json::from_value::<ProxyWsMode>(value.clone()).ok())
    else {
        return Ok(None);
    };
    let generation = payload
        .get("generation")
        .and_then(serde_json::Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok());
    if mode == ProxyWsMode::Observe && generation.is_none() {
        return Ok(None);
    }

    Ok(Some(ProxyWsTokenPayload {
        connection_id,
        user_id,
        mode,
        generation,
        expires_at: timestamp.saturating_add(ttl_seconds),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observation_credentials_bind_mode_actor_session_and_generation() {
        let secret = Uuid::new_v4().to_string();
        let connection = Uuid::new_v4();
        let actor = Uuid::new_v4();
        let generation = Uuid::new_v4();
        let token =
            create_proxy_observer_ws_token(&secret, connection, actor, generation).expect("issue");
        let payload = validate_proxy_ws_token(&secret, &token, connection, 60)
            .expect("validate")
            .expect("valid");
        assert_eq!(payload.mode, ProxyWsMode::Observe);
        assert_eq!(payload.connection_id, connection);
        assert_eq!(payload.user_id, actor);
        assert_eq!(payload.generation, Some(generation));
        assert!(
            validate_proxy_ws_token(&secret, &token, Uuid::new_v4(), 60)
                .expect("validate")
                .is_none()
        );
        assert!(
            validate_proxy_ws_token(&secret, &token, connection, 0)
                .expect("validate")
                .is_none()
        );
    }

    #[test]
    fn observer_without_generation_and_tampered_credentials_are_rejected() {
        let secret = Uuid::new_v4().to_string();
        let connection = Uuid::new_v4();
        let actor = Uuid::new_v4();
        let token =
            create_scoped_proxy_ws_token(&secret, connection, actor, ProxyWsMode::Observe, None)
                .expect("issue");
        assert!(
            validate_proxy_ws_token(&secret, &token, connection, 60)
                .expect("validate")
                .is_none()
        );
        let operator = create_proxy_ws_token(&secret, connection, actor).expect("issue");
        let payload = validate_proxy_ws_token(&secret, &operator, connection, 60)
            .expect("validate")
            .expect("valid");
        assert_eq!(payload.mode, ProxyWsMode::Operator);
        assert_eq!(payload.generation, None);
        let mut bytes = URL_SAFE.decode(operator.as_bytes()).expect("decode");
        bytes[25] ^= 1;
        assert!(
            validate_proxy_ws_token(&secret, &URL_SAFE.encode(bytes), connection, 60)
                .expect("validate")
                .is_none()
        );
    }
}
