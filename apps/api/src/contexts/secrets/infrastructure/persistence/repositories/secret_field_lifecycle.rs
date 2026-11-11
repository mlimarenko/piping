//! Transactional persistence of a field reference, its value and retirement authority.

use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::db_secret_store::DbSecretStore;
use crate::contexts::inventory::domain::content_type::CONTENT_TYPES;
use crate::contexts::secrets::application::consumer_type::INVENTORY_FHRP_GROUP_CONSUMER_TYPE;
use crate::contexts::secrets::application::{
    ApplySecretFieldIntentInput, SecretFieldIntent, SecretFieldState,
};
use crate::contexts::secrets::domain::{ResolvedCredential, SecretUsageBinding};
use crate::contexts::secrets::infrastructure::crypto::encrypt_secret;
use crate::errors::AppError;

/// All persistence writers lock resource rows before secret variables. The
/// advisory identity lock also serializes a missing field (there is no usage row
/// to lock yet); actual native rows remain the resource-existence authority.
pub(crate) async fn lock_field_owner(
    transaction: &mut Transaction<'_, Postgres>,
    input: &ApplySecretFieldIntentInput,
) -> Result<(), AppError> {
    sqlx::query!(
        "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
        format!(
            "secret-owner/{}/{}",
            input.owner_scope_type, input.owner_scope_id
        )
    )
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

pub(crate) async fn lock_field_consumer(
    transaction: &mut Transaction<'_, Postgres>,
    input: &ApplySecretFieldIntentInput,
) -> Result<(), AppError> {
    input.validate_native_shape()?;
    lock_field_owner(transaction, input).await?;
    let (table, scope) = match input.consumer_type.as_str() {
        INVENTORY_FHRP_GROUP_CONSUMER_TYPE => {
            ("ipam_fhrpgroup", INVENTORY_FHRP_GROUP_CONSUMER_TYPE)
        }
        "user_auth_password" => ("identity_user", "user"),
        "user_connection_credential_set" => ("connections_user_credential_set", "user"),
        "connection_config:record" => ("connconf_record", "connection_config"),
        "sync_provider" => (
            "sync_provider",
            crate::contexts::sync::domain::SYNC_PROVIDER_OWNER_SCOPE_TYPE,
        ),
        "proxy_node:credential" => ("connections_proxy_node", "proxy_node"),
        "playbook_service_principal:credential" => {
            ("playbook_service_principal", "playbook_service_principal")
        }
        "connection_scope:connection_target" => {
            ensure_scope(input, "connection_target", input.consumer_id)?;
            for definition in CONTENT_TYPES.iter().filter(|definition| {
                matches!(
                    definition.table,
                    "dcim_device" | "virtualization_virtualmachine"
                )
            }) {
                let mut query = sqlx::QueryBuilder::<Postgres>::new("SELECT id FROM ");
                query
                    .push(definition.table)
                    .push(" WHERE id = ")
                    .push_bind(input.consumer_id)
                    .push(" FOR UPDATE");
                if query
                    .build()
                    .fetch_optional(&mut **transaction)
                    .await?
                    .is_some()
                {
                    return Ok(());
                }
            }
            return Err(AppError::NotFound("Connection target not found".into()));
        }
        "inventory_custom_field" => return lock_native_custom_field(transaction, input).await,
        "oidc_settings" => {
            use crate::contexts::access_control::domain::{
                oidc_client_secret_consumer_id, oidc_client_secret_owner_scope_id,
            };
            ensure_scope(input, "system", oidc_client_secret_owner_scope_id()?)?;
            if input.consumer_id != oidc_client_secret_consumer_id()? {
                return Err(AppError::Validation("Invalid OIDC secret consumer".into()));
            }
            sqlx::query!("SELECT id FROM auth_oidc_settings WHERE id = 1 FOR UPDATE")
                .fetch_one(&mut **transaction)
                .await?;
            return Ok(());
        }
        // These existing virtual-consumer contracts have no native deletable
        // resource. Serialize the declared identity without inventing a table.
        "assistant_instance" => {
            ensure_scope(
                input,
                "instance",
                crate::contexts::secrets::application::consumer_type::assistant_instance_id()?,
            )?;
            if input.consumer_id != input.owner_scope_id {
                return Err(AppError::Validation(
                    "Invalid assistant instance consumer".into(),
                ));
            }
            return Ok(());
        }
        _ => {
            return Err(AppError::Validation(
                "Unsupported secret field consumer".into(),
            ));
        }
    };
    let user_owner = native_user_owner(transaction, table, input.consumer_id).await?;
    if let Some(owner) = user_owner {
        ensure_scope(input, "user", owner)?;
        sqlx::query!(
            "SELECT id FROM identity_user WHERE id = $1 FOR UPDATE",
            owner
        )
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or_else(|| AppError::NotFound("Secret field owner not found".into()))?;
    } else if input.consumer_type != INVENTORY_FHRP_GROUP_CONSUMER_TYPE {
        // FHRP's owner identity is checked by the shared pure shape validator.
        ensure_scope(input, scope, input.consumer_id)?;
    }
    let mut query = sqlx::QueryBuilder::<Postgres>::new("SELECT id FROM ");
    query
        .push(table)
        .push(" WHERE id = ")
        .push_bind(input.consumer_id)
        .push(" FOR UPDATE");
    query
        .build()
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or_else(|| AppError::NotFound("Secret field consumer not found".into()))?;
    let locked_owner = native_user_owner(transaction, table, input.consumer_id).await?;
    if locked_owner != user_owner {
        return Err(AppError::Conflict(
            "Secret field consumer ownership changed during locking".into(),
        ));
    }
    Ok(())
}

async fn native_user_owner(
    transaction: &mut Transaction<'_, Postgres>,
    table: &str,
    consumer_id: Uuid,
) -> Result<Option<Uuid>, AppError> {
    match table {
        "connections_user_credential_set" => Ok(Some(
            sqlx::query!(
                "SELECT user_id FROM connections_user_credential_set WHERE id = $1",
                consumer_id
            )
            .fetch_optional(&mut **transaction)
            .await?
            .ok_or_else(|| AppError::NotFound("Credential set not found".into()))?
            .user_id,
        )),
        "connconf_record" => Ok(sqlx::query!(
            "SELECT owner_user_id FROM connconf_record WHERE id = $1",
            consumer_id
        )
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or_else(|| AppError::NotFound("Connection record not found".into()))?
        .owner_user_id),
        _ => Ok(None),
    }
}

async fn lock_native_custom_field(
    transaction: &mut Transaction<'_, Postgres>,
    input: &ApplySecretFieldIntentInput,
) -> Result<(), AppError> {
    use crate::contexts::custom_fields::infrastructure::persistence::repositories::custom_field_definition_repository::{native_field_subjects, lock_field_catalog_for_usage};
    lock_field_catalog_for_usage(transaction).await?;
    ensure_scope(input, "inventory_custom_field", input.consumer_id)?;
    let mut scopes = None;
    if input.intent != SecretFieldIntent::Clear {
        let definition = sqlx::query!(
            "SELECT id FROM custom_field WHERE name = $1 AND is_secret FOR SHARE",
            input.field_key
        )
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or_else(|| AppError::Validation("Secret custom field definition not found".into()))?;
        scopes = Some(sqlx::query!("SELECT content_type_id FROM custom_field_object_type WHERE custom_field_id = $1 ORDER BY content_type_id", definition.id)
            .fetch_all(&mut **transaction).await?.into_iter().map(|row| row.content_type_id).collect::<Vec<_>>());
    }
    // Explicit Clear remains idempotent after definition removal; it writes no
    // value and still requires the actual native UUID subject to exist.
    for native in native_field_subjects(transaction).await? {
        if scopes
            .as_ref()
            .is_some_and(|ids| !ids.contains(&native.content_type_id))
        {
            continue;
        }
        let table = native.table_name.replace('"', "\"\"");
        let mut query = sqlx::QueryBuilder::<Postgres>::new("SELECT id FROM public.\"");
        query
            .push(table)
            .push("\" WHERE id = ")
            .push_bind(input.consumer_id)
            .push(" FOR UPDATE");
        if query
            .build()
            .fetch_optional(&mut **transaction)
            .await?
            .is_some()
        {
            return Ok(());
        }
    }
    Err(AppError::NotFound(
        "Native custom field subject not found".into(),
    ))
}

fn ensure_scope(
    input: &ApplySecretFieldIntentInput,
    scope: &str,
    id: Uuid,
) -> Result<(), AppError> {
    if input.owner_scope_type != scope || input.owner_scope_id != id {
        return Err(AppError::Validation(
            "Secret field owner does not match native consumer ownership".into(),
        ));
    }
    Ok(())
}

pub(crate) async fn lock_secrets(
    transaction: &mut Transaction<'_, Postgres>,
    ids: &[Uuid],
) -> Result<(), AppError> {
    let mut ids = ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    sqlx::query!(
        "SELECT id FROM secrets_variable WHERE id = ANY($1) ORDER BY id FOR UPDATE",
        &ids
    )
    .fetch_all(&mut **transaction)
    .await?;
    Ok(())
}

pub(crate) async fn retire_usage(
    transaction: &mut Transaction<'_, Postgres>,
    usage: &SecretUsageBinding,
) -> Result<(), AppError> {
    sqlx::query!("DELETE FROM secrets_binding b WHERE b.secret_id = $1 AND b.scope_type = $2 AND b.scope_id = $3 AND b.managed_by_field_usage AND NOT EXISTS (SELECT 1 FROM secret_usage_binding u WHERE u.secret_id = b.secret_id AND u.owner_scope_type = b.scope_type AND u.owner_scope_id = b.scope_id)", usage.secret_id, usage.owner_scope_type, usage.owner_scope_id)
        .execute(&mut **transaction).await?;
    // Both checks defend against borrowed variables, historical ambiguity and
    // protected/foreign bindings. FK cascades remove ciphertext only on success.
    sqlx::query!("DELETE FROM secrets_variable v WHERE v.id = $1 AND v.retire_with_field_usage AND v.kind = 'field' AND v.key LIKE 'secret-field/%' AND NOT EXISTS (SELECT 1 FROM secret_usage_binding u WHERE u.secret_id = v.id) AND NOT EXISTS (SELECT 1 FROM secrets_binding b WHERE b.secret_id = v.id)", usage.secret_id)
        .execute(&mut **transaction).await?;
    Ok(())
}

pub(crate) async fn detach_consumer(
    transaction: &mut Transaction<'_, Postgres>,
    consumer_type: &str,
    consumer_id: Uuid,
    owner_scope_type: &str,
    owner_scope_id: Uuid,
) -> Result<(), AppError> {
    let usages = sqlx::query_as!(SecretUsageBinding, "SELECT id, consumer_type, consumer_id, field_key, owner_scope_type, owner_scope_id, secret_id FROM secret_usage_binding WHERE consumer_type = $1 AND consumer_id = $2 AND owner_scope_type = $3 AND owner_scope_id = $4 ORDER BY secret_id", consumer_type, consumer_id, owner_scope_type, owner_scope_id)
        .fetch_all(&mut **transaction).await?;
    lock_secrets(
        transaction,
        &usages
            .iter()
            .map(|usage| usage.secret_id)
            .collect::<Vec<_>>(),
    )
    .await?;
    for usage in usages {
        sqlx::query!("DELETE FROM secret_usage_binding WHERE id = $1", usage.id)
            .execute(&mut **transaction)
            .await?;
        retire_usage(transaction, &usage).await?;
    }
    Ok(())
}

fn field_state(
    input: ApplySecretFieldIntentInput,
    secret_id: Option<Uuid>,
    has_value: bool,
) -> SecretFieldState {
    SecretFieldState {
        consumer_type: input.consumer_type,
        consumer_id: input.consumer_id,
        owner_scope_type: input.owner_scope_type,
        owner_scope_id: input.owner_scope_id,
        field_key: input.field_key,
        enabled: secret_id.is_some(),
        secret_id,
        has_value,
    }
}

async fn owned_usage(
    transaction: &mut Transaction<'_, Postgres>,
    input: &ApplySecretFieldIntentInput,
) -> Result<Option<SecretUsageBinding>, AppError> {
    let existing = sqlx::query_as!(SecretUsageBinding, "SELECT id, consumer_type, consumer_id, field_key, owner_scope_type, owner_scope_id, secret_id FROM secret_usage_binding WHERE consumer_type = $1 AND consumer_id = $2 AND field_key = $3", input.consumer_type, input.consumer_id, input.field_key)
        .fetch_optional(&mut **transaction).await?;
    if existing.as_ref().is_some_and(|usage| {
        usage.owner_scope_type != input.owner_scope_type
            || usage.owner_scope_id != input.owner_scope_id
    }) {
        return Err(AppError::Validation(
            "Persisted secret field owner differs from supplied owner".into(),
        ));
    }
    Ok(existing)
}

async fn validate_replacement(
    transaction: &mut Transaction<'_, Postgres>,
    input: &ApplySecretFieldIntentInput,
    id: Uuid,
) -> Result<(), AppError> {
    let variable = sqlx::query!("SELECT id, kind FROM secrets_variable WHERE id = $1", id)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or_else(|| AppError::NotFound("Replacement secret not found".into()))?;
    if variable.kind == "field" {
        let foreign = sqlx::query!("SELECT EXISTS (SELECT 1 FROM secret_usage_binding WHERE secret_id = $1 AND (owner_scope_type <> $2 OR owner_scope_id <> $3)) AS \"foreign!: bool\"", id, input.owner_scope_type, input.owner_scope_id)
            .fetch_one(&mut **transaction).await?;
        if foreign.foreign {
            return Err(AppError::Validation(
                "Replacement field secret belongs to another owner".into(),
            ));
        }
    }
    Ok(())
}

async fn select_field_secret(
    transaction: &mut Transaction<'_, Postgres>,
    input: &ApplySecretFieldIntentInput,
    existing: Option<&SecretUsageBinding>,
) -> Result<Uuid, AppError> {
    let selected_id = input
        .secret_id
        .or_else(|| existing.map(|usage| usage.secret_id));
    let ids: Vec<Uuid> = existing
        .map(|usage| usage.secret_id)
        .into_iter()
        .chain(selected_id)
        .collect();
    lock_secrets(transaction, &ids).await?;
    if let Some(id) = selected_id {
        validate_replacement(transaction, input, id).await?;
        return Ok(id);
    }
    let id = Uuid::now_v7();
    let field_key: String = input
        .field_key
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let key: String = format!(
        "secret-field/{id}/{}/{}/{}/{field_key}",
        input.consumer_type, input.consumer_id, input.owner_scope_id
    )
    .chars()
    .take(255)
    .collect();
    sqlx::query!("INSERT INTO secrets_variable (id, key, is_masked, is_protected, kind, retire_with_field_usage) VALUES ($1, $2, true, false, 'field', true)", id, key).execute(&mut **transaction).await?;
    Ok(id)
}

async fn attach_field_usage(
    transaction: &mut Transaction<'_, Postgres>,
    input: &ApplySecretFieldIntentInput,
    secret_id: Uuid,
) -> Result<(), AppError> {
    let usage_id = Uuid::now_v7();
    sqlx::query!("INSERT INTO secret_usage_binding (id, consumer_type, consumer_id, field_key, owner_scope_type, owner_scope_id, secret_id) VALUES ($1, $2, $3, $4, $5, $6, $7) ON CONFLICT (consumer_type, consumer_id, field_key) DO UPDATE SET secret_id = EXCLUDED.secret_id, owner_scope_type = EXCLUDED.owner_scope_type, owner_scope_id = EXCLUDED.owner_scope_id, updated_at = now()", usage_id, input.consumer_type, input.consumer_id, input.field_key, input.owner_scope_type, input.owner_scope_id, secret_id)
        .execute(&mut **transaction).await?;
    let binding_id = Uuid::now_v7();
    sqlx::query!("INSERT INTO secrets_binding (id, secret_id, scope_type, scope_id, weight, managed_by_field_usage) SELECT $1, $2, $3::varchar, $4, 0, true WHERE NOT EXISTS (SELECT 1 FROM secrets_binding WHERE secret_id = $2 AND scope_type = $3::varchar AND scope_id = $4)", binding_id, secret_id, input.owner_scope_type, input.owner_scope_id)
        .execute(&mut **transaction).await?;
    Ok(())
}

async fn persist_field_value(
    transaction: &mut Transaction<'_, Postgres>,
    input: &ApplySecretFieldIntentInput,
    secret_id: Uuid,
) -> Result<bool, AppError> {
    if let Some(value) = input
        .value
        .as_deref()
        .map(|value| {
            if input.consumer_type == INVENTORY_FHRP_GROUP_CONSUMER_TYPE {
                value
            } else {
                value.trim()
            }
        })
        .filter(|value| !value.is_empty())
    {
        let encrypted = encrypt_secret(value)?;
        sqlx::query!("INSERT INTO secrets_store (secret_id, encrypted_value) VALUES ($1, $2) ON CONFLICT (secret_id) DO UPDATE SET encrypted_value = EXCLUDED.encrypted_value", secret_id, encrypted)
            .execute(&mut **transaction).await?;
    }
    let stored = sqlx::query!(
        "SELECT encrypted_value FROM secrets_store WHERE secret_id = $1",
        secret_id
    )
    .fetch_optional(&mut **transaction)
    .await?;
    match stored {
        Some(row) => Ok(!DbSecretStore::decode_stored_secret(&row.encrypted_value)?
            .trim()
            .is_empty()),
        None => Ok(false),
    }
}

pub(crate) async fn apply_field_intent(
    transaction: &mut Transaction<'_, Postgres>,
    input: ApplySecretFieldIntentInput,
) -> Result<SecretFieldState, AppError> {
    lock_field_consumer(transaction, &input).await?;
    let existing = owned_usage(transaction, &input).await?;
    if input.intent == SecretFieldIntent::Clear {
        if let Some(usage) = existing {
            lock_secrets(transaction, &[usage.secret_id]).await?;
            sqlx::query!("DELETE FROM secret_usage_binding WHERE id = $1", usage.id)
                .execute(&mut **transaction)
                .await?;
            retire_usage(transaction, &usage).await?;
        }
        return Ok(field_state(input, None, false));
    }
    let secret_id = select_field_secret(transaction, &input, existing.as_ref()).await?;
    attach_field_usage(transaction, &input, secret_id).await?;
    let has_value = persist_field_value(transaction, &input, secret_id).await?;
    if let Some(usage) = existing.filter(|usage| usage.secret_id != secret_id) {
        retire_usage(transaction, &usage).await?;
    }
    Ok(field_state(input, Some(secret_id), has_value))
}

/// Resolve the credential of one consumer field inside the caller's transaction.
///
/// The usage binding and the stored ciphertext are read under share locks in this
/// one transaction, and both the plaintext and the revision are derived from that
/// single ciphertext string. Reading the value here and the revision somewhere
/// else could bind a token a concurrent rotation had already replaced to the
/// revision of its successor.
pub(crate) async fn resolve_field_credential(
    transaction: &mut Transaction<'_, Postgres>,
    consumer_type: &str,
    consumer_id: Uuid,
    field_key: &str,
) -> Result<Option<ResolvedCredential>, AppError> {
    let Some(secret_id) =
        lock_field_usage_secret(transaction, consumer_type, consumer_id, field_key).await?
    else {
        return Ok(None);
    };
    let Some(encrypted_value) = lock_stored_secret(transaction, secret_id).await? else {
        return Ok(None);
    };
    let value = DbSecretStore::decode_stored_secret(&encrypted_value)?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    Ok(Some(ResolvedCredential::new(
        secret_id,
        credential_revision_of(&encrypted_value),
        trimmed.to_string(),
    )))
}

/// Re-check the credential a commit is about to rely on, under share locks.
///
/// The usage binding must still point at `expected_secret_id` and the stored
/// ciphertext must still hash to `expected_revision`. Both rows stay share-locked
/// for the rest of the caller's transaction, so a rotation, clear, rebind or
/// disable either lands before this check, which then refuses, or waits for the
/// commit that relies on it.
pub(crate) async fn revalidate_credential_revision(
    transaction: &mut Transaction<'_, Postgres>,
    consumer_type: &str,
    consumer_id: Uuid,
    field_key: &str,
    expected_secret_id: Uuid,
    expected_revision: &str,
) -> Result<(), AppError> {
    let bound = lock_field_usage_secret(transaction, consumer_type, consumer_id, field_key).await?;
    if bound != Some(expected_secret_id) {
        return Err(AppError::Validation(
            "sync.credential_binding_stale: the consumer binding no longer points to the credential that was resolved"
                .to_string(),
        ));
    }
    let stored = lock_stored_secret(transaction, expected_secret_id).await?;
    let current = stored.as_deref().map(credential_revision_of);
    if current.as_deref() != Some(expected_revision) {
        return Err(AppError::Validation(
            "sync.credential_revision_stale: the configured credential changed after it was resolved"
                .to_string(),
        ));
    }
    Ok(())
}

/// Share-lock the usage binding of one consumer field and return its secret.
///
/// Every field writer inserts, updates or deletes this row before it touches the
/// ciphertext of the selected secret, so taking the binding first keeps the
/// reader's lock order (binding, then store) identical to the writers' and rules
/// out a lock cycle between a rotation and this resolution.
async fn lock_field_usage_secret(
    transaction: &mut Transaction<'_, Postgres>,
    consumer_type: &str,
    consumer_id: Uuid,
    field_key: &str,
) -> Result<Option<Uuid>, AppError> {
    Ok(sqlx::query_scalar(
        "SELECT secret_id FROM secret_usage_binding \
         WHERE consumer_type = $1 AND consumer_id = $2 AND field_key = $3 \
         ORDER BY id LIMIT 1 FOR SHARE",
    )
    .bind(consumer_type)
    .bind(consumer_id)
    .bind(field_key)
    .fetch_optional(&mut **transaction)
    .await?)
}

/// Share-lock the stored ciphertext row; `None` means the value was cleared.
async fn lock_stored_secret(
    transaction: &mut Transaction<'_, Postgres>,
    secret_id: Uuid,
) -> Result<Option<String>, AppError> {
    Ok(sqlx::query_scalar(
        "SELECT encrypted_value FROM secrets_store WHERE secret_id = $1 FOR SHARE",
    )
    .bind(secret_id)
    .fetch_optional(&mut **transaction)
    .await?)
}

/// Digest of the stored ciphertext, encoded as lowercase hexadecimal.
fn credential_revision_of(encrypted_value: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(encrypted_value.as_bytes()))
}
