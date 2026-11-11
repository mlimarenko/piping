use uuid::Uuid;

/// A subject the access-control core evaluates policies for.
///
/// `Principal` is the single input to the unified `check_access*` core. It has
/// exactly two shapes today:
///
/// - [`Principal::User`] — a human (or OIDC-federated) identity, identified by
///   user id. External groups are materialized into durable group membership
///   before authorization reaches this type.
/// - [`Principal::ServicePrincipal`] — a playbook service principal, identified
///   by its UUID.
///
/// # Security invariant (codex HIGH #5)
///
/// A [`Principal::ServicePrincipal`] MUST NOT be constructible from a bare UUID
/// anywhere in application code. Its `id` field is private and the only
/// constructor is [`Principal::service_principal`], which is documented to be
/// called solely after the existing active/non-revoked load check. The single
/// such site is `PlaybookService::load_validated_service_principal`
/// (`contexts/playbooks/application/playbook_service.rs`), which loads the row,
/// enforces `status == active && revoked_at.is_none()`, and only then builds the
/// principal; both the scheduled authoring path
/// (`ensure_scheduled_run_service_principal`) and the dispatch-time secret
/// re-check (`app::workers::resolve_runtime_secret_env`) reach the SP identity
/// through it. The active/revoked guarantee itself lives upstream at the load
/// site; this type only guarantees that no other code path can fabricate an SP
/// principal out of thin air.
#[derive(Debug, Clone)]
pub enum Principal {
    User { id: Uuid },
    ServicePrincipal(ServicePrincipalId),
}

/// Opaque, validated service-principal identifier.
///
/// The inner UUID is private so the only way to obtain one is through
/// [`Principal::service_principal`], which callers reach after loading and
/// validating the principal. This prevents a bare-UUID bypass of the
/// active/non-revoked gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServicePrincipalId(Uuid);

impl ServicePrincipalId {
    pub fn get(self) -> Uuid {
        self.0
    }
}

impl Principal {
    /// Build a `User` principal from an id.
    pub fn user(id: Uuid) -> Self {
        Self::User { id }
    }

    /// Build a `ServicePrincipal` principal.
    ///
    /// This is the ONLY constructor for the service-principal variant; the
    /// caller is responsible for having already loaded and validated the
    /// principal (active, not revoked) before calling it. See the type-level
    /// docs for the security invariant.
    pub fn service_principal(id: Uuid) -> Self {
        Self::ServicePrincipal(ServicePrincipalId(id))
    }
}
