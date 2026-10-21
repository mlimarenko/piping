use super::*;
use crate::contexts::secrets::application::resolve_secret_visibility;
use crate::contexts::secrets::infrastructure::crypto::{decrypt_secret, encrypt_secret};
use std::num::NonZeroU64;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::Notify;
use tokio::time::Instant;

// No Debug/Serialize on retained entries. The encrypted replay contains only
// original explicit inputs and verified identities, never resolved saved values.
pub(super) struct CredentialReplay {
    encrypted: Option<String>,
    expires_at: Instant,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct LaunchReplay {
    session_id: Uuid,
    target_id: Uuid,
    user_id: Uuid,
    protocol: ConnectionProtocol,
    account_id: Option<Uuid>,
    personal_set_id: Option<Uuid>,
    template_id: Option<Uuid>,
    merge_policy: UserCredentialSetMergePolicy,
    overrides: Option<ConnectionLaunchParams>,
    pub(super) endpoint_host: String,
    endpoint_port: u16,
    required_fields: Vec<String>,
}

pub(super) enum LaunchPreparation<'a> {
    Create {
        metadata: Option<&'a Value>,
        template_id: Option<Uuid>,
        overrides: Option<ConnectionLaunchParams>,
    },
    Replay(&'a LaunchReplay),
}

pub(super) struct LaunchSubject<'a> {
    pub session_id: Uuid,
    pub target_id: Uuid,
    pub user_id: Uuid,
    pub protocol: &'a ConnectionProtocol,
}

struct PreparedLaunchInputs {
    selection: LaunchCredentialSelection,
    template_id: Option<Uuid>,
    template_defaults: Option<ConnectionLaunchParams>,
    overrides: Option<ConnectionLaunchParams>,
    retained_overrides: Option<ConnectionLaunchParams>,
}

pub(super) struct SessionCredentials {
    entries: Arc<Mutex<HashMap<Uuid, CredentialReplay>>>,
    changed: Arc<Notify>,
    cleanup: tokio::task::JoinHandle<()>,
    ttl: Duration,
}

fn unavailable() -> AppError {
    AppError::NotFound("Session credentials unavailable; start a new connection".to_string())
}

fn take_explicit_client_password(
    payload: &mut Value,
    protocol: &ConnectionProtocol,
    overrides: Option<&ConnectionLaunchParams>,
    personal_set_id: Option<Uuid>,
    merge_policy: UserCredentialSetMergePolicy,
) -> Option<Value> {
    // Explicit unsaved passwords are literals, even if UUID-shaped. The merge
    // policy, not value equality, determines whether a personal binding won.
    if matches!(protocol, ConnectionProtocol::Vnc | ConnectionProtocol::Rdp)
        && !(personal_set_id.is_some()
            && merge_policy == UserCredentialSetMergePolicy::OverrideExisting)
        && overrides
            .and_then(|params| params.get("credentials"))
            .and_then(|credentials| credentials.get("password"))
            .is_some()
    {
        payload
            .get_mut("credentials")
            .and_then(Value::as_object_mut)
            .and_then(|credentials| credentials.remove("password"))
    } else {
        None
    }
}

impl SessionCredentials {
    pub(super) fn new(ttl_seconds: NonZeroU64) -> Self {
        let entries = Arc::new(Mutex::new(HashMap::<Uuid, CredentialReplay>::new()));
        let weak = Arc::downgrade(&entries);
        let changed = Arc::new(Notify::new());
        let notification = changed.clone();
        let cleanup = tokio::spawn(async move {
            loop {
                let next = {
                    let Some(entries) = weak.upgrade() else {
                        return;
                    };
                    let Ok(mut entries) = entries.lock() else {
                        return;
                    };
                    let now = Instant::now();
                    entries.retain(|_, entry| entry.expires_at > now);
                    entries.values().map(|entry| entry.expires_at).min()
                };
                match next {
                    Some(deadline) => tokio::select! {
                        () = tokio::time::sleep_until(deadline) => {},
                        () = notification.notified() => {},
                    },
                    None => notification.notified().await,
                }
            }
        });
        Self {
            entries,
            changed,
            cleanup,
            ttl: Duration::from_secs(ttl_seconds.get()),
        }
    }

    pub(super) fn prepare(&self, replay: &LaunchReplay) -> Result<CredentialReplay, AppError> {
        let plaintext = serde_json::to_string(replay).map_err(|_| unavailable())?;
        Ok(CredentialReplay {
            encrypted: Some(encrypt_secret(&plaintext)?),
            expires_at: Instant::now()
                .checked_add(self.ttl)
                .ok_or_else(unavailable)?,
        })
    }

    pub(super) fn remove(&self, id: Uuid) {
        if let Ok(mut entries) = self.entries.lock() {
            // A terminal event can beat create's final audit await. This bounded
            // ciphertext-free tombstone prevents a late publisher resurrecting it.
            entries.insert(
                id,
                CredentialReplay {
                    encrypted: None,
                    expires_at: Instant::now()
                        .checked_add(self.ttl)
                        .unwrap_or_else(Instant::now),
                },
            );
        }
        self.changed.notify_one();
    }

    pub(super) fn publish(&self, id: Uuid, replay: CredentialReplay) -> Result<(), AppError> {
        let mut entries = self.entries.lock().map_err(|_| unavailable())?;
        if entries
            .get(&id)
            .is_some_and(|entry| entry.encrypted.is_none())
        {
            return Err(unavailable());
        }
        entries.insert(id, replay);
        self.changed.notify_one();
        Ok(())
    }

    fn ensure_available(&self, id: Uuid) -> Result<(), AppError> {
        let entries = self.entries.lock().map_err(|_| unavailable())?;
        entries
            .get(&id)
            .filter(|entry| entry.encrypted.is_some() && entry.expires_at > Instant::now())
            .map(|_| ())
            .ok_or_else(unavailable)
    }

    fn read(&self, session: &ConnectionSession) -> Result<LaunchReplay, AppError> {
        let entries = self.entries.lock().map_err(|_| unavailable())?;
        let entry = entries
            .get(&session.id)
            .filter(|entry| entry.expires_at > Instant::now())
            .ok_or_else(unavailable)?;
        let plaintext = decrypt_secret(entry.encrypted.as_deref().ok_or_else(unavailable)?)
            .map_err(|_| unavailable())?;
        let replay: LaunchReplay = serde_json::from_str(&plaintext).map_err(|_| unavailable())?;
        if replay.session_id != session.id
            || replay.user_id != session.user_id
            || replay.target_id != session.target_id
            || replay.protocol != session.protocol
        {
            return Err(unavailable());
        }
        Ok(replay)
    }
}

impl Drop for SessionCredentials {
    fn drop(&mut self) {
        self.cleanup.abort();
    }
}

impl<R, P, A, M, SR, SS> ConnectionService<R, P, A, M, SR, SS>
where
    R: ConnectionRepository,
    P: ProxyClient,
    A: AuditRecorder,
    M: TargetMaterializer,
    SR: SecretRepository,
    SS: SecretStore,
{
    pub(super) fn forget_client_credentials(&self, session: &ConnectionSession) {
        if matches!(
            session.protocol,
            ConnectionProtocol::Vnc | ConnectionProtocol::Rdp
        ) {
            self.session_credentials.remove(session.id);
        }
    }

    async fn resolve_preparation_inputs(
        &self,
        user_id: Uuid,
        protocol: &ConnectionProtocol,
        preparation: LaunchPreparation<'_>,
    ) -> Result<PreparedLaunchInputs, AppError> {
        let (selection, template_id, mut overrides) = match preparation {
            LaunchPreparation::Create {
                metadata,
                template_id,
                overrides,
            } => (
                self.resolve_launch_credential_selection(Some(user_id), protocol, metadata)
                    .await?,
                template_id,
                overrides,
            ),
            LaunchPreparation::Replay(original) => {
                let personal = match original.personal_set_id {
                    Some(id) => Some(
                        self.resolve_user_credential_set_for_launch(user_id, protocol, id)
                            .await?,
                    ),
                    None => None,
                };
                let params = personal
                    .as_ref()
                    .map(|set| {
                        Self::value_to_launch_params_map(
                            set.params.clone(),
                            "user credential set params",
                        )
                    })
                    .transpose()?
                    .filter(|map| !map.is_empty());
                (
                    LaunchCredentialSelection {
                        selected_entry_index: None,
                        selected_config_entry_index: None,
                        allow_missing_selected_config_entry: false,
                        selected_user_credential_set: personal,
                        selected_user_credential_set_params: params,
                        credential_set_merge_policy: original.merge_policy,
                    },
                    original.template_id,
                    original.overrides.clone(),
                )
            }
        };
        let template_defaults = match template_id {
            None => None,
            Some(id) => {
                let template = self
                    .repository
                    .get_template(id)
                    .await?
                    .ok_or_else(unavailable)?;
                if template.protocol != *protocol {
                    return Err(AppError::Validation(
                        "Template protocol does not match requested protocol".to_string(),
                    ));
                }
                Some(
                    normalize_launch_params(protocol, Some(template.defaults))?
                        .ok_or_else(unavailable)?,
                )
            }
        };
        if template_defaults.is_none() && selection.selected_entry_index.is_some() {
            overrides = Self::validate_entry_driven_overrides(protocol, overrides)?;
        }
        // HTTP input is typed. The same bound also protects internal callers
        // before explicit inputs enter the transient VNC/RDP registry.
        if let Some(params) = overrides.as_ref() {
            let bytes = serde_json::to_vec(params).map_err(|_| unavailable())?;
            if matches!(protocol, ConnectionProtocol::Vnc | ConnectionProtocol::Rdp)
                && bytes.len() > 64 * 1024
            {
                return Err(AppError::Validation(
                    "Connection launch overrides are too large".to_string(),
                ));
            }
        }
        let mut retained_overrides = overrides.clone();
        if let Some(params) = retained_overrides.as_mut() {
            params.remove("label");
            params.remove("color");
        }
        Ok(PreparedLaunchInputs {
            selection,
            template_id,
            template_defaults,
            overrides,
            retained_overrides,
        })
    }

    async fn resolve_preparation_account(
        &self,
        target_id: Uuid,
        protocol: &ConnectionProtocol,
        selection: &LaunchCredentialSelection,
        replay: Option<&LaunchReplay>,
    ) -> Result<Option<(Uuid, connconf_resolve::MappedConnConfParams)>, AppError> {
        let accounts = self
            .resolve_shared_launch_accounts(target_id, protocol)
            .await?;
        let account = match replay {
            Some(original) => match original.account_id {
                Some(id) => Some(
                    accounts
                        .iter()
                        .find(|account| account.record_id == id)
                        .ok_or_else(unavailable)?,
                ),
                None => None,
            },
            None => {
                if accounts.is_empty() && selection.allow_missing_selected_config_entry {
                    None
                } else {
                    select_launch_account(&accounts, selection.selected_config_entry_index)?
                }
            }
        };
        account
            .map(|account| {
                let mapped =
                    connconf_resolve::map_effective_params(&account.params, &account.attribution)
                        .ok_or_else(unavailable)?;
                Ok((account.record_id, mapped))
            })
            .transpose()
    }

    /// Shared preparation only: never allocates a proxy, writes a session, or
    /// records a session-created event. Secret access audits remain explicit in
    /// materialization and the existing native-secret resolution path.
    pub(super) async fn prepare_session_launch(
        &self,
        subject: LaunchSubject<'_>,
        preparation: LaunchPreparation<'_>,
        access_control: Option<&dyn SecretVisibilityChecker>,
        request_context: Option<&RequestContext>,
    ) -> Result<(Value, LaunchReplay), AppError> {
        let LaunchSubject {
            session_id,
            target_id,
            user_id,
            protocol,
        } = subject;
        let replay = match &preparation {
            LaunchPreparation::Replay(replay) => Some(*replay),
            _ => None,
        };
        let PreparedLaunchInputs {
            selection,
            template_id,
            mut template_defaults,
            mut overrides,
            retained_overrides,
        } = self
            .resolve_preparation_inputs(user_id, protocol, preparation)
            .await?;
        let materialized = self
            .materialize_target_config_for_connection(
                session_id,
                target_id,
                Some(user_id),
                access_control,
                request_context,
            )
            .await?;
        let mut account_id = None;
        let mut attributions = HashMap::new();
        if template_defaults.is_none() {
            let host_params = match self
                .resolve_preparation_account(target_id, protocol, &selection, replay)
                .await?
            {
                Some((id, mapped)) => {
                    account_id = Some(id);
                    attributions = mapped.secret_attributions;
                    Some(mapped.fields)
                }
                None => None,
            };
            let mut defaults = merge_required_connconf_params(
                protocol,
                host_params,
                selection.selected_user_credential_set_params.clone(),
                selection.credential_set_merge_policy,
            )?;
            if let Some(overrides) = overrides.take() {
                Self::merge_launch_params_map(
                    &mut defaults,
                    overrides,
                    UserCredentialSetMergePolicy::OverrideExisting,
                );
            }
            overrides = Some(defaults);
        }
        let context = self
            .build_launch_resolution_context(target_id, &materialized.config)
            .await?;
        template_defaults =
            Self::resolve_config_placeholders_in_launch_params(template_defaults, &context)?;
        overrides = Self::resolve_config_placeholders_in_launch_params(overrides, &context)?;
        overrides =
            Self::fill_host_from_primary_endpoint(template_defaults.as_ref(), overrides, &context);
        let mut payload =
            Self::build_proxy_payload(protocol.clone(), template_defaults, overrides)?;
        let personal_set_id = selection
            .selected_user_credential_set
            .as_ref()
            .map(|set| set.id);
        if let Some(id) = personal_set_id {
            self.apply_user_credential_set_to_payload(
                protocol,
                id,
                selection.credential_set_merge_policy,
                &mut payload,
            )
            .await?;
        }
        let literal_password = take_explicit_client_password(
            &mut payload,
            protocol,
            retained_overrides.as_ref(),
            personal_set_id,
            selection.credential_set_merge_policy,
        );
        payload = self
            .resolve_proxy_payload_secrets(
                target_id,
                Some(user_id),
                payload,
                Some(&materialized.config),
                Some(&attributions),
            )
            .await?;
        if let Some(password) = literal_password {
            payload
                .get_mut("credentials")
                .and_then(Value::as_object_mut)
                .ok_or_else(unavailable)?
                .insert("password".to_string(), password);
        }
        Self::ensure_required_proxy_payload_credentials(protocol, &payload)?;
        let host = payload.get("host").and_then(Value::as_str);
        let fallback = if host.is_none() {
            resolve_primary_endpoint(&context)
        } else {
            None
        };
        let endpoint_host = self
            .ensure_endpoint_bound_to_target(target_id, protocol, host.or(fallback.as_deref()))
            .await?;
        payload["host"] = Value::String(endpoint_host.clone());
        let (_, endpoint_port) =
            crate::proxy::extract_host_port(&payload).map_err(|_| unavailable())?;
        let credentials = Self::filter_client_credentials(
            protocol,
            payload
                .get("credentials")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default(),
        );
        let required_fields: Vec<_> = credentials
            .iter()
            .filter(|(_, value)| value.as_str().is_some_and(|value| !value.is_empty()))
            .map(|(key, _)| key.clone())
            .collect();
        if let Some(original) = replay
            && (endpoint_host != original.endpoint_host
                || endpoint_port != original.endpoint_port
                || original
                    .required_fields
                    .iter()
                    .any(|key| !required_fields.contains(key)))
        {
            return Err(unavailable());
        }
        Ok((
            payload,
            LaunchReplay {
                session_id,
                target_id,
                user_id,
                protocol: protocol.clone(),
                account_id,
                personal_set_id,
                template_id,
                merge_policy: selection.credential_set_merge_policy,
                overrides: retained_overrides,
                endpoint_host,
                endpoint_port,
                required_fields,
            },
        ))
    }

    pub(super) async fn read_client_credentials(
        &self,
        session: &ConnectionSession,
        actor_id: Uuid,
        access_control: Option<&dyn SecretVisibilityChecker>,
        request_context: Option<&RequestContext>,
    ) -> Result<Map<String, Value>, AppError> {
        if session.user_id != actor_id || session.status.is_terminal() {
            return Err(unavailable());
        }
        if !resolve_secret_visibility(access_control, request_context).await? {
            return Err(AppError::Forbidden("Secret access denied".to_string()));
        }
        let replay = self.session_credentials.read(session)?;
        let (payload, _) = self
            .prepare_session_launch(
                LaunchSubject {
                    session_id: session.id,
                    target_id: session.target_id,
                    user_id: actor_id,
                    protocol: &session.protocol,
                },
                LaunchPreparation::Replay(&replay),
                access_control,
                request_context,
            )
            .await
            .map_err(|error| match error {
                AppError::Validation(_) | AppError::NotFound(_) => unavailable(),
                error => error,
            })?;
        let current = self.require_session(session.id).await?;
        if current.user_id != actor_id
            || current.target_id != session.target_id
            || current.protocol != session.protocol
            || current.status.is_terminal()
        {
            self.forget_client_credentials(&current);
            return Err(unavailable());
        }
        self.session_credentials.ensure_available(session.id)?;
        Ok(Self::filter_client_credentials(
            &session.protocol,
            payload
                .get("credentials")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(deadline: Instant) -> CredentialReplay {
        CredentialReplay {
            encrypted: Some("test-envelope".to_string()),
            expires_at: deadline,
        }
    }

    #[tokio::test]
    async fn replay_expiry_is_nonsliding_and_restart_is_empty() {
        let registry = SessionCredentials::new(NonZeroU64::new(60).expect("positive TTL"));
        let id = Uuid::now_v7();
        let deadline = Instant::now() + Duration::from_secs(60);
        registry.publish(id, entry(deadline)).expect("publish");
        assert!(registry.ensure_available(id).is_ok());
        assert!(registry.ensure_available(id).is_ok());
        assert!(registry.entries.lock().expect("lock")[&id].expires_at == deadline);
        registry
            .entries
            .lock()
            .expect("lock")
            .get_mut(&id)
            .expect("entry")
            .expires_at = Instant::now();
        assert!(
            registry.ensure_available(id).is_err(),
            "expiry must not depend on cleanup scheduling"
        );
        let restarted = SessionCredentials::new(NonZeroU64::new(60).expect("positive TTL"));
        assert!(restarted.ensure_available(id).is_err());
    }

    #[tokio::test]
    async fn terminal_eviction_defeats_late_publication_and_drop_stops_cleanup() {
        let registry = SessionCredentials::new(NonZeroU64::new(60).expect("positive TTL"));
        let id = Uuid::now_v7();
        registry
            .publish(id, entry(Instant::now() + Duration::from_secs(60)))
            .expect("publish");
        registry.remove(id);
        registry.remove(id);
        assert!(registry.ensure_available(id).is_err());
        assert!(
            registry
                .publish(id, entry(Instant::now() + Duration::from_secs(60)))
                .is_err()
        );
        let weak = Arc::downgrade(&registry.entries);
        let cleanup = registry.cleanup.abort_handle();
        drop(registry);
        tokio::task::yield_now().await;
        assert!(weak.upgrade().is_none());
        assert!(cleanup.is_finished());
    }
}
