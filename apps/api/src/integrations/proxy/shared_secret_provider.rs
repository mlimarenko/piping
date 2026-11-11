use std::env;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use openssl::rand::rand_bytes;

use crate::errors::AppError;

pub const PROXY_NODE_ID_ENV_KEY: &str = "PIPING_PROXY__NODE_ID";
pub const PROXY_NODE_SECRET_ENV_KEY: &str = "PIPING_PROXY__NODE_SECRET";
const EMBEDDED_PROXY_NODE_ID: &str = "embedded";

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ProxyCredential {
    pub node_id: String,
    pub secret: String,
}

impl ProxyCredential {
    pub fn new(node_id: impl Into<String>, secret: impl Into<String>) -> Result<Self, AppError> {
        let node_id = node_id.into().trim().to_string();
        let secret = secret.into().trim().to_string();
        if node_id.is_empty() {
            return Err(AppError::InvalidConfig(
                "Proxy node id is required but was empty".to_string(),
            ));
        }
        if secret.is_empty() {
            return Err(AppError::InvalidConfig(
                "Proxy node secret is required but was empty".to_string(),
            ));
        }
        if let Some(bound_node_id) = secret
            .strip_prefix("pn_")
            .and_then(|value| value.split_once('.').map(|(node_id, _)| node_id))
            && bound_node_id != node_id
        {
            return Err(AppError::InvalidConfig(
                "Proxy node secret is bound to a different node id".to_string(),
            ));
        }
        Ok(Self { node_id, secret })
    }
}

pub trait ProxySharedSecretProvider: Send + Sync {
    fn get(&self) -> Result<ProxyCredential, AppError>;
}

/// Proxy-mode provider: reads this node's own credential from environment.
#[derive(Debug, Clone)]
pub struct EnvProxySharedSecretProvider {
    node_id_env_key: String,
    secret_env_key: String,
}

impl EnvProxySharedSecretProvider {
    pub fn new(node_id_env_key: &str, secret_env_key: &str) -> Self {
        Self {
            node_id_env_key: node_id_env_key.to_string(),
            secret_env_key: secret_env_key.to_string(),
        }
    }
}

impl ProxySharedSecretProvider for EnvProxySharedSecretProvider {
    fn get(&self) -> Result<ProxyCredential, AppError> {
        let node_id = env::var(&self.node_id_env_key).map_err(|_| {
            AppError::InvalidConfig(format!(
                "{} is required and must not be empty",
                self.node_id_env_key
            ))
        })?;
        let secret = env::var(&self.secret_env_key).map_err(|_| {
            AppError::InvalidConfig(format!(
                "{} is required and must not be empty",
                self.secret_env_key
            ))
        })?;
        ProxyCredential::new(node_id, secret)
    }
}

/// Backend embedded-proxy provider: holds one process-local credential in memory.
#[derive(Debug, Clone)]
pub struct StaticProxySharedSecretProvider {
    credential: ProxyCredential,
}

impl StaticProxySharedSecretProvider {
    pub fn new(node_id: String, secret: String) -> Result<Self, AppError> {
        Ok(Self {
            credential: ProxyCredential::new(node_id, secret)?,
        })
    }

    pub fn ephemeral_embedded() -> Result<Self, AppError> {
        Self::new(
            EMBEDDED_PROXY_NODE_ID.to_string(),
            generate_proxy_secret_material()?,
        )
    }
}

impl ProxySharedSecretProvider for StaticProxySharedSecretProvider {
    fn get(&self) -> Result<ProxyCredential, AppError> {
        Ok(self.credential.clone())
    }
}

pub fn generate_proxy_secret_material() -> Result<String, AppError> {
    let mut raw = [0u8; 32];
    rand_bytes(&mut raw).map_err(|error| {
        AppError::External(format!("Failed to generate proxy node secret: {}", error))
    })?;
    Ok(URL_SAFE_NO_PAD.encode(raw))
}

#[cfg(test)]
mod tests {
    use super::ProxyCredential;

    #[test]
    fn accepts_secret_bound_to_same_node_id() {
        let credential = ProxyCredential::new(
            "019f0000-0000-7000-8000-000000000001",
            "pn_019f0000-0000-7000-8000-000000000001.random",
        );

        assert!(credential.is_ok(), "{credential:?}");
    }

    #[test]
    fn rejects_secret_bound_to_different_node_id() {
        let credential = ProxyCredential::new(
            "019f0000-0000-7000-8000-000000000002",
            "pn_019f0000-0000-7000-8000-000000000001.random",
        );

        assert!(credential.is_err(), "{credential:?}");
    }
}
