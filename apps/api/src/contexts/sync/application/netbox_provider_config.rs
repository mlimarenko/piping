//! NetBox endpoint and credential resolution shared by every sync service.
//!
//! A provider row names the remote host and page size; the API token is a secret
//! binding owned by the secret engine. Live inbound reads, outbound pushes and
//! bootstrap all resolve it through one owner call that returns the plaintext and
//! the revision of the stored row it was decrypted from, so the two always
//! describe the same credential.

use std::future::Future;

use serde_json::Value;
use uuid::Uuid;

use crate::config::NetboxConfig;
use crate::contexts::secrets::domain::ResolvedCredential;
use crate::contexts::secrets::infrastructure::SecretRepository;
pub use crate::contexts::sync::domain::absence::{
    ProviderGenerationBinding, SourceCredentialRevision,
};
use crate::contexts::sync::domain::{
    Provider, ProviderType, SYNC_PROVIDER_CONSUMER_TYPE, SYNC_PROVIDER_FIELD_KEY_API_TOKEN,
};
use crate::contexts::sync::infrastructure::ProviderRepository;
use crate::errors::AppError;

/// Provider secret lookup by field key.
///
/// The secret engine returns the value together with the revision of the stored
/// ciphertext it came from; a resolver that cannot produce both cannot prove
/// which credential a client was built with.
pub trait ProviderSecretResolver {
    fn resolve_secret_credential(
        &self,
        provider_id: Uuid,
        field_key: &str,
    ) -> impl Future<Output = Result<Option<ResolvedCredential>, AppError>> + Send;
}

/// The credential a resolved NetBox client actually uses, with the opaque
/// revision its owner computed from the same stored row.
///
/// Destructive work must revalidate the revision under share locks in the target
/// transaction. Provider JSON and provider
/// row timestamps are not a substitute: the provider row and its secret commands
/// are written separately, and the sanitized config can stay byte-identical across
/// a token rotation. That transaction hook is not yet wired into the pull.
///
/// A resolved NetBox client configuration and the credential generation behind it.
pub struct ResolvedNetboxSource {
    pub config: NetboxConfig,
    /// `None` when the token is stored only inside the provider config, which
    /// means the row predates strict sanitization on write. Such a source stays
    /// readable, but it has no sealed credential and therefore no generation
    /// proof — see [`Self::sealed_credential`].
    pub credential: Option<SourceCredentialRevision>,
}

impl ResolvedNetboxSource {
    /// The credential revision destructive work must carry, refusing an unsealed
    /// token rather than inventing an empty revision for it.
    ///
    /// # Errors
    ///
    /// [`AppError::Validation`] when the token did not come from the secret
    /// engine. Saving the provider extracts an inline `api_token` into a secret
    /// binding and seals it; retrying after that yields a generation.
    pub fn sealed_credential(&self) -> Result<&SourceCredentialRevision, AppError> {
        self.credential.as_ref().ok_or_else(|| {
            AppError::Validation(
                "sync.credential_unsealed: the source token is not stored by the secret engine, so this source has no credential generation; save the provider so its api_token is extracted into a secret binding"
                    .to_string(),
            )
        })
    }
}

/// Resolve the NetBox client configuration of `provider` and its credential
/// generation.
///
/// # Errors
///
/// [`AppError::Validation`] when the provider is not a NetBox provider, its
/// config is not an object, `base_url`/`page_size`/`api_token` are missing or
/// malformed.
pub async fn resolve_netbox_source<R: ProviderSecretResolver + ?Sized>(
    provider: &Provider,
    secrets: &R,
) -> Result<ResolvedNetboxSource, AppError> {
    let (base_url, page_size) = netbox_endpoint(provider)?;

    let credential = secrets
        .resolve_secret_credential(provider.id, SYNC_PROVIDER_FIELD_KEY_API_TOKEN)
        .await?;
    let (api_token, credential) = match credential {
        Some(credential) => (
            credential.expose_value().to_string(),
            Some(SourceCredentialRevision {
                secret_id: credential.secret_id(),
                revision: credential.revision().to_string(),
            }),
        ),
        None => (inline_api_token(provider.config.as_object())?, None),
    };
    if api_token.is_empty() {
        return Err(AppError::Validation(
            "Provider config api_token is required for NetBox provider".to_string(),
        ));
    }

    Ok(ResolvedNetboxSource {
        config: NetboxConfig {
            base_url,
            api_token,
            page_size,
        },
        credential,
    })
}

/// Resolve only the client configuration; callers that need generation proof use
/// [`resolve_netbox_source`] instead.
pub async fn resolve_netbox_config<R: ProviderSecretResolver + ?Sized>(
    provider: &Provider,
    secrets: &R,
) -> Result<NetboxConfig, AppError> {
    Ok(resolve_netbox_source(provider, secrets).await?.config)
}

/// Provider endpoint fields the client is built from.
fn netbox_endpoint(provider: &Provider) -> Result<(String, usize), AppError> {
    if provider.provider_type != ProviderType::Netbox {
        return Err(AppError::Validation(format!(
            "Provider type '{:?}' is not supported for sync",
            provider.provider_type
        )));
    }

    let map = provider
        .config
        .as_object()
        .ok_or_else(|| AppError::Validation("Provider config must be a JSON object".to_string()))?;

    let base_url = match map.get("base_url") {
        Some(Value::String(value)) => value.trim().to_string(),
        Some(_) => {
            return Err(AppError::Validation(
                "Provider config base_url must be a string".to_string(),
            ));
        }
        None => String::new(),
    };
    if base_url.is_empty() {
        return Err(AppError::Validation(
            "Provider config base_url is required for NetBox provider".to_string(),
        ));
    }

    let page_size = match map.get("page_size") {
        Some(Value::Number(number)) => number
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .filter(|value| *value > 0)
            .ok_or_else(|| {
                AppError::Validation(
                    "Provider config page_size must be a positive integer".to_string(),
                )
            })?,
        Some(_) => {
            return Err(AppError::Validation(
                "Provider config page_size must be a number".to_string(),
            ));
        }
        None => NetboxConfig::default().page_size,
    };

    Ok((base_url, page_size))
}

/// Token that a pre-extraction provider row still carries inline. It stays usable
/// for reads, but [`ResolvedNetboxSource::sealed_credential`] refuses to treat it
/// as a generation.
fn inline_api_token(map: Option<&serde_json::Map<String, Value>>) -> Result<String, AppError> {
    match map.and_then(|map| map.get("api_token")) {
        Some(Value::String(value)) => Ok(value.trim().to_string()),
        Some(_) => Err(AppError::Validation(
            "Provider config api_token must be a string".to_string(),
        )),
        None => Ok(String::new()),
    }
}

/// Declared sync mode of the provider config, bound into the generation.
fn sync_mode(config: &Value) -> Option<String> {
    config
        .get("sync_mode")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Everything a destructive source generation depends on besides the mapping
/// digest: the source must stay enabled, on the same endpoint and mode, with the
/// same credential revision from resolution until the commit that relies on it.
impl ProviderGenerationBinding {
    /// Capture the generation of `resolved`, resolved from `provider`'s row.
    ///
    /// # Errors
    ///
    /// [`AppError::Conflict`] when the source is disabled, and whatever
    /// [`ResolvedNetboxSource::sealed_credential`] rejects: a disabled or unsealed
    /// source never yields a generation.
    pub fn capture(provider: &Provider, resolved: &ResolvedNetboxSource) -> Result<Self, AppError> {
        if !provider.is_enabled {
            return Err(AppError::Conflict(
                "sync.source_disabled: a disabled source cannot execute a destructive generation"
                    .to_string(),
            ));
        }
        Ok(Self {
            provider_id: provider.id,
            base_url: resolved.config.base_url.clone(),
            sync_mode: sync_mode(&provider.config),
            credential: resolved.sealed_credential()?.clone(),
        })
    }
}

/// Revalidate a captured generation inside the caller's target transaction.
///
/// The provider row, the usage binding and the stored ciphertext are read under
/// share locks in that transaction, so a disable, an endpoint or mode change, a
/// rebind, a clear or a rotation either lands before this check and refuses, or
/// waits until the transaction relying on the generation commits.
pub async fn revalidate_provider_generation<PR, SR>(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    providers: &PR,
    secrets: &SR,
    expected: &ProviderGenerationBinding,
) -> Result<(), AppError>
where
    PR: ProviderRepository + ?Sized,
    SR: SecretRepository + ?Sized,
{
    let state = providers
        .lock_generation_state(transaction, expected.provider_id)
        .await?
        .ok_or_else(|| {
            AppError::Validation(
                "sync.source_generation_stale: the source no longer exists".to_string(),
            )
        })?;
    if !state.is_enabled {
        return Err(AppError::Validation(
            "sync.source_generation_stale: the source was disabled".to_string(),
        ));
    }
    let current_base_url = state
        .config
        .get("base_url")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if current_base_url != expected.base_url || sync_mode(&state.config) != expected.sync_mode {
        return Err(AppError::Validation(
            "sync.source_generation_stale: the source endpoint or mode changed".to_string(),
        ));
    }
    secrets
        .revalidate_consumer_credential(
            transaction,
            SYNC_PROVIDER_CONSUMER_TYPE,
            expected.provider_id,
            SYNC_PROVIDER_FIELD_KEY_API_TOKEN,
            expected.credential.secret_id,
            &expected.credential.revision,
        )
        .await
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use uuid::Uuid;

    use super::{
        ProviderGenerationBinding, ResolvedNetboxSource, SourceCredentialRevision,
        resolve_netbox_source,
    };
    use crate::config::NetboxConfig;
    use crate::contexts::secrets::domain::ResolvedCredential;
    use crate::contexts::sync::domain::{Provider, ProviderType};

    struct FixedCredential(Option<ResolvedCredential>);

    impl super::ProviderSecretResolver for FixedCredential {
        fn resolve_secret_credential(
            &self,
            _provider_id: Uuid,
            _field_key: &str,
        ) -> impl std::future::Future<
            Output = Result<Option<ResolvedCredential>, crate::errors::AppError>,
        > + Send {
            std::future::ready(Ok(self.0.as_ref().map(|credential| {
                ResolvedCredential::new(
                    credential.secret_id(),
                    credential.revision().to_string(),
                    credential.expose_value().to_string(),
                )
            })))
        }
    }

    fn provider(config: serde_json::Value, is_enabled: bool) -> Provider {
        Provider {
            id: Uuid::from_u128(7),
            name: "netbox".to_string(),
            provider_type: ProviderType::Netbox,
            config,
            is_enabled,
        }
    }

    fn sealed(token: &str) -> ResolvedCredential {
        ResolvedCredential::new(
            Uuid::from_u128(11),
            format!("revision-of-{token}"),
            token.to_string(),
        )
    }

    #[tokio::test]
    async fn capture_binds_endpoint_mode_and_credential_revision() {
        let secrets = FixedCredential(Some(sealed("token-one")));
        let source = provider(
            json!({
                "base_url": " https://netbox.example.com ",
                "sync_mode": "bidirectional",
                "page_size": 100
            }),
            true,
        );
        let resolved = resolve_netbox_source(&source, &secrets)
            .await
            .expect("sealed source");
        let binding = ProviderGenerationBinding::capture(&source, &resolved).expect("generation");

        assert_eq!(binding.base_url, "https://netbox.example.com");
        assert_eq!(binding.sync_mode.as_deref(), Some("bidirectional"));
        assert_eq!(binding.credential.secret_id, Uuid::from_u128(11));
        assert_eq!(binding.credential.revision, "revision-of-token-one");
        assert_eq!(resolved.config.api_token, "token-one");
    }

    #[tokio::test]
    async fn pull_only_mode_is_bound_rather_than_dropped() {
        let secrets = FixedCredential(Some(sealed("token-one")));
        let source = provider(
            json!({ "base_url": "https://netbox.example.com", "sync_mode": "pull_only" }),
            true,
        );
        let resolved = resolve_netbox_source(&source, &secrets)
            .await
            .expect("sealed source");
        let binding = ProviderGenerationBinding::capture(&source, &resolved).expect("generation");
        assert_eq!(binding.sync_mode.as_deref(), Some("pull_only"));
        assert!(!source.allows_remote_writes());
    }

    #[tokio::test]
    async fn disabled_source_captures_no_generation() {
        let secrets = FixedCredential(Some(sealed("token-one")));
        let source = provider(json!({ "base_url": "https://netbox.example.com" }), false);
        let resolved = resolve_netbox_source(&source, &secrets)
            .await
            .expect("sealed source");
        let refused = ProviderGenerationBinding::capture(&source, &resolved);
        assert!(
            refused.is_err(),
            "a disabled source must not capture a generation"
        );
    }

    #[tokio::test]
    async fn inline_token_stays_readable_but_yields_no_generation() {
        let secrets = FixedCredential(None);
        let source = provider(
            json!({ "base_url": "https://netbox.example.com", "api_token": "legacy-inline" }),
            true,
        );
        let resolved = resolve_netbox_source(&source, &secrets)
            .await
            .expect("readable row");
        assert_eq!(resolved.config.api_token, "legacy-inline");
        assert!(resolved.credential.is_none());
        let refused = ProviderGenerationBinding::capture(&source, &resolved);
        match refused {
            Ok(_) => panic!("an unsealed token must not prove a generation"),
            Err(error) => assert!(error.to_string().contains("sync.credential_unsealed")),
        }
    }

    #[tokio::test]
    async fn malformed_inline_token_is_rejected_instead_of_used() {
        let secrets = FixedCredential(None);
        let source = provider(
            json!({ "base_url": "https://netbox.example.com", "api_token": 7 }),
            true,
        );
        assert!(resolve_netbox_source(&source, &secrets).await.is_err());
    }

    #[test]
    fn generation_binding_debug_carries_no_plaintext() {
        let binding = ProviderGenerationBinding {
            provider_id: Uuid::from_u128(7),
            base_url: "https://netbox.example.com".to_string(),
            sync_mode: None,
            credential: SourceCredentialRevision {
                secret_id: Uuid::from_u128(11),
                revision: "revision-1".to_string(),
            },
        };
        assert!(format!("{binding:?}").contains("revision-1"));
    }

    #[test]
    fn credential_revision_debug_never_prints_the_token() {
        let resolved = ResolvedNetboxSource {
            config: NetboxConfig {
                base_url: "https://netbox.example.com".to_string(),
                api_token: "token-one".to_string(),
                page_size: 100,
            },
            credential: Some(SourceCredentialRevision {
                secret_id: Uuid::from_u128(11),
                revision: "ciphertext-digest-1".to_string(),
            }),
        };
        let credential = resolved.sealed_credential().expect("sealed");
        assert_eq!(credential.revision, "ciphertext-digest-1");
        let rendered = format!("{credential:?}");
        assert!(rendered.contains("ciphertext-digest-1"), "{rendered}");
        assert!(!rendered.contains("token-one"), "{rendered}");
    }
}
