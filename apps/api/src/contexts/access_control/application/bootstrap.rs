use argon2::{Argon2, PasswordHasher, password_hash::SaltString};
use chrono::Utc;
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use super::password_policy::validate_local_password;
use crate::config::AuthConfig;
use crate::contexts::access_control::domain::{Action, ResourceType};
use crate::errors::AppError;
use crate::observability::{EventFields, LogLevel, log_event};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalAdminBootstrapResult {
    Skipped,
    Exists { user_id: Uuid },
    Created { user_id: Uuid },
}

const DEFAULT_ADMIN_USERNAME: &str = "admin";
const DEFAULT_ADMIN_EMAIL: &str = "admin@local";
/// Development-only fallback for the local administrator credential.
///
/// Used solely while `security.allow_weak_dev_secrets` is on and no
/// `auth.local_admin_password` override is configured, so a local checkout can
/// sign in. Production never reaches it: configuration validation fails closed
/// when strong secrets are enforced and the override is missing or weak.
/// Do not log this value.
const DEV_LOCAL_ADMIN_PASSWORD: &str = "piping123";
const DEFAULT_ADMIN_ROLE: &str = "admin";
const DEFAULT_EDITOR_ROLE: &str = "editor";
const DEFAULT_VIEWER_ROLE: &str = "viewer";
const DEFAULT_AUTOMATION_ROLE: &str = "automation";
const DEFAULT_AUDITOR_ROLE: &str = "auditor";
const DEFAULT_ADMIN_POLICY: &str = "admin:full-access";
const DEFAULT_EDITOR_POLICY: &str = "editor:workspace-write";
const DEFAULT_VIEWER_POLICY: &str = "viewer:portal-access";
const DEFAULT_AUTOMATION_PLAYBOOK_POLICY: &str = "automation:playbooks";
const DEFAULT_AUTOMATION_TARGETS_POLICY: &str = "automation:targets-read";
const DEFAULT_AUDITOR_POLICY: &str = "auditor:read";
const OIDC_GROUP_PIPING: &str = "piping";
const OIDC_GROUP_ADMIN: &str = "piping-admin";
const OIDC_GROUP_EDITOR: &str = "piping-editor";
const OIDC_GROUP_VIEWER: &str = "piping-viewer";
const OIDC_GROUP_AUTOMATION: &str = "piping-automation";
const OIDC_GROUP_AUDITOR: &str = "piping-auditor";

/// Instance-setting marker key that records the curated starter-role seed has
/// already run once. Its presence makes the curated set seed-once (see
/// [`ensure_curated_default_roles`]) rather than self-healing on every boot.
const CURATED_SEED_MARKER_KEY: &str = "rbac.curated_default_roles_seeded";
const CCTV_CURATED_SEED_MARKER_KEY: &str = "rbac.cctv_operator_role_seeded";

/// A curated starter role: a role plus one allow policy bound to it, over the
/// full resource set (`resources = ["*"]`, no constraints). These are ready-made
/// personas an operator assigns users to (or clones/tunes) instead of building a
/// role from scratch. Resource-type strings are canonical `ResourceType` wire
/// tokens; descriptions are the human explanation shown in the access UI.
struct CuratedRole {
    role: &'static str,
    description: &'static str,
    policy: &'static str,
    actions: &'static [&'static str],
    resource_types: &'static [&'static str],
}

/// Curated starter roles seeded once on first boot. Kept domain-neutral —
/// functional personas, no customer/subject-area specifics.
///
/// A policy grants its whole action set over its whole resource set, so the
/// neutral on-device account plane (`DEVICE_USERS`) rides the connection target
/// rather than the CCTV namespace; `0007` converges an already-seeded row to the
/// same action/resource sets.
const CCTV_OPERATOR_ROLE: CuratedRole = CuratedRole {
    role: "cctv-operator",
    description: "Просмотр CCTV, архива, экспорт видео, управление пользователями устройств, состоянием хранилища и PTZ.",
    policy: "cctv-operator:manage",
    actions: &[
        "CCTV_VIEW",
        "CCTV_RECORDINGS",
        "CCTV_EXPORT",
        "DEVICE_USERS",
        "CCTV_SETTINGS",
        "CCTV_PTZ",
    ],
    resource_types: &["CCTV", "CONNECTION_TARGET"],
};

const CURATED_ROLES: &[CuratedRole] = &[
    CuratedRole {
        role: "helpdesk-operator",
        description: "Поддержка: подключение к хостам, просмотр инвентаря и состояния. Без редактирования и администрирования.",
        policy: "helpdesk-operator:access",
        actions: &["CONNECT", "OBSERVE", "READ"],
        resource_types: &[
            "CONNECTION_TARGET",
            "CONNECTION_SESSION",
            "INVENTORY",
            "CONFIG_CONTEXT",
            "MONITORING",
        ],
    },
    CuratedRole {
        role: "session-operator",
        description: "Открытие и ведение удалённых сессий (SSH/RDP/VNC) к целям подключения.",
        policy: "session-operator:connect",
        actions: &["CONNECT", "OBSERVE", "READ", "UPDATE"],
        resource_types: &["CONNECTION_TARGET", "CONNECTION_SESSION"],
    },
    CuratedRole {
        role: "inventory-manager",
        description: "Ведение инвентаря и контекста конфигурации. Без подключения к хостам.",
        policy: "inventory-manager:write",
        actions: &["CREATE", "READ", "UPDATE", "DELETE"],
        resource_types: &["INVENTORY", "CONFIG_CONTEXT"],
    },
    CuratedRole {
        role: "inventory-viewer",
        description: "Только чтение инвентаря, контекста конфигурации и целей подключения.",
        policy: "inventory-viewer:read",
        actions: &["READ"],
        resource_types: &["INVENTORY", "CONFIG_CONTEXT", "CONNECTION_TARGET"],
    },
    CuratedRole {
        role: "monitoring-viewer",
        description: "Просмотр метрик мониторинга, алертов и состояния хостов.",
        policy: "monitoring-viewer:read",
        actions: &["READ", "OBSERVE"],
        resource_types: &["MONITORING", "CONNECTION_TARGET"],
    },
    CuratedRole {
        role: "playbook-operator",
        description: "Запуск готовых плейбуков и просмотр их результатов. Без редактирования плейбуков.",
        policy: "playbook-operator:execute",
        actions: &["EXECUTE", "READ"],
        resource_types: &[
            "PLAYBOOK",
            "PLAYBOOK_VERSION",
            "PLAYBOOK_TARGET_SET",
            "PLAYBOOK_RUN",
            "PLAYBOOK_RUN_GROUP",
            "PLAYBOOK_JOB_TEMPLATE",
            "PLAYBOOK_ARTIFACT",
            "CONNECTION_TARGET",
            "INVENTORY",
        ],
    },
    CuratedRole {
        role: "playbook-author",
        description: "Создание и сопровождение плейбуков, версий, проектов, шаблонов и расписаний.",
        policy: "playbook-author:manage",
        actions: &["CREATE", "READ", "UPDATE", "DELETE"],
        resource_types: &[
            "PLAYBOOK",
            "PLAYBOOK_VERSION",
            "PLAYBOOK_ASSIGNMENT",
            "PLAYBOOK_TARGET_SET",
            "PLAYBOOK_JOB_TEMPLATE",
            "PLAYBOOK_PROJECT",
            "PLAYBOOK_SCHEDULE",
        ],
    },
    CuratedRole {
        role: "secrets-manager",
        description: "Управление секретными переменными автоматизации и раскрытие их значений.",
        policy: "secrets-manager:manage",
        actions: &["CREATE", "READ", "UPDATE", "DELETE", "SECRET"],
        resource_types: &["SECRET_VARIABLE"],
    },
    CuratedRole {
        role: "access-administrator",
        description: "Делегированное администрирование доступа: пользователи, группы, роли и политики.",
        policy: "access-administrator:manage",
        actions: &[
            "MANAGE_ROLES",
            "MANAGE_POLICIES",
            "CREATE",
            "READ",
            "UPDATE",
            "DELETE",
        ],
        resource_types: &["USER", "GROUP", "ROLE", "POLICY"],
    },
    CuratedRole {
        role: "security-auditor",
        description: "Аудит безопасности: журнал аудита и чтение субъектов доступа, сессий и запусков.",
        policy: "security-auditor:review",
        actions: &["VIEW_AUDIT", "READ"],
        resource_types: &[
            "AUDIT_EVENT",
            "USER",
            "GROUP",
            "ROLE",
            "POLICY",
            "CONNECTION_SESSION",
            "PLAYBOOK_RUN",
        ],
    },
    CuratedRole {
        role: "proxy-administrator",
        description: "Настройка прокси-узлов, маршрутизирующих сессии в сети клиентов.",
        policy: "proxy-administrator:manage",
        actions: &["CREATE", "READ", "UPDATE", "DELETE"],
        resource_types: &["PROXY_NODE"],
    },
    CuratedRole {
        role: "instance-administrator",
        description: "Настройки экземпляра, брендирование, лицензия, бэкапы и интеграции. Без доступа к хостам и секретам.",
        policy: "instance-administrator:manage",
        actions: &["CREATE", "READ", "UPDATE", "DELETE"],
        resource_types: &[
            "SETTINGS", "BRANDING", "LICENSE", "BACKUP", "WEBHOOK", "KEYCLOAK",
        ],
    },
];

pub async fn ensure_local_admin(
    pool: &PgPool,
    auth: &AuthConfig,
) -> Result<LocalAdminBootstrapResult, AppError> {
    let admin_role_id = ensure_default_rbac_baseline(pool).await?;

    if !auth.local_enabled {
        return Ok(LocalAdminBootstrapResult::Skipped);
    }
    let username = DEFAULT_ADMIN_USERNAME.to_string();
    let email = DEFAULT_ADMIN_EMAIL.to_string();

    // The configured override is the single source of truth for the canonical
    // local emergency credential. A blank/whitespace value is treated as absent;
    // `auth.local_admin_password` is validated at startup when strong secrets
    // are enforced, so this branch cannot be reached in production without one.
    let password_override = auth
        .local_admin_password
        .as_deref()
        .map(str::trim)
        .filter(|password| !password.is_empty());

    let user_id = Uuid::now_v7();
    let row = sqlx::query(
        r#"
        INSERT INTO identity_user (id, username, email, status, auth_source, external_id)
        VALUES ($1, $2, $3, 'active', 'local', NULL)
        ON CONFLICT (username)
        DO UPDATE SET email = EXCLUDED.email, status = 'active', auth_source = 'local'
        RETURNING id, (xmax = 0) AS inserted
        "#,
    )
    .bind(user_id)
    .bind(&username)
    .bind(&email)
    .fetch_one(pool)
    .await
    .map_err(|e| AppError::Database(format!("Failed to create local admin user: {e}")))?;

    let resolved_user_id: Uuid = row
        .try_get("id")
        .map_err(|e| AppError::Database(format!("Failed to read user id: {e}")))?;
    let inserted: bool = row
        .try_get("inserted")
        .map_err(|e| AppError::Database(format!("Failed to read user insert status: {e}")))?;

    if inserted {
        log_event(
            LogLevel::Warning,
            "access_control.local_admin.bootstrap.completed",
            EventFields::default()
                .with_operation("access_control.local_admin.bootstrap")
                .with_resource("bootstrap_user", username.clone())
                .with_status("created")
                .with_message("Creating default local admin user"),
        );
    } else {
        log_event(
            LogLevel::Info,
            "access_control.local_admin.bootstrap.completed",
            EventFields::default()
                .with_operation("access_control.local_admin.bootstrap")
                .with_resource("bootstrap_user", username.clone())
                .with_status("exists")
                .with_message("Local admin user already exists"),
        );
    }

    // Never reset credentials implicitly: outside an explicit override, the
    // password is bootstrapped only when the account is new or has none. With a
    // configured override the canonical local credential is upserted on every
    // start — idempotent, and scoped to this user's `auth_user_credential` row,
    // so no other account's credential is touched.
    let has_credentials: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS(
            SELECT 1
            FROM auth_user_credential
            WHERE user_id = $1
        )
        "#,
    )
    .bind(resolved_user_id)
    .fetch_one(pool)
    .await
    .map_err(|e| AppError::Database(format!("Failed to check admin credentials: {e}")))?;

    match password_override {
        Some(password) => {
            upsert_credentials(
                pool,
                resolved_user_id,
                password,
                &auth.password_pepper,
                auth.password_min_length,
            )
            .await?;
            log_event(
                LogLevel::Info,
                "access_control.local_admin.bootstrap.credential_rotated",
                EventFields::default()
                    .with_operation("access_control.local_admin.bootstrap")
                    .with_resource("bootstrap_user", username.clone())
                    .with_status("rotated")
                    .with_message("Applied configured local admin password override"),
            );
        }
        None => {
            if inserted || !has_credentials {
                upsert_credentials(
                    pool,
                    resolved_user_id,
                    DEV_LOCAL_ADMIN_PASSWORD,
                    &auth.password_pepper,
                    auth.password_min_length,
                )
                .await?;
            }
        }
    }
    assign_role_to_user(pool, resolved_user_id, admin_role_id).await?;

    if inserted {
        Ok(LocalAdminBootstrapResult::Created {
            user_id: resolved_user_id,
        })
    } else {
        Ok(LocalAdminBootstrapResult::Exists {
            user_id: resolved_user_id,
        })
    }
}

/// Role ids created by [`ensure_default_rbac_baseline`] and reused when binding
/// policies and assigning groups.
struct BaselineRoles {
    admin: Uuid,
    editor: Uuid,
    viewer: Uuid,
}

pub async fn ensure_default_rbac_baseline(pool: &PgPool) -> Result<Uuid, AppError> {
    let roles = ensure_baseline_roles(pool).await?;
    ensure_baseline_policies(pool, &roles).await?;
    ensure_baseline_groups(pool, &roles).await?;
    let automation_role_id = ensure_automation_rbac(pool).await?;
    let auditor_role_id = ensure_auditor_rbac(pool).await?;
    ensure_auditor_and_automation_groups(pool, auditor_role_id, automation_role_id).await?;

    ensure_curated_default_roles(pool).await?;
    ensure_cctv_operator_role(pool).await?;

    Ok(roles.admin)
}

/// Upsert the three baseline roles (admin, editor, viewer).
async fn ensure_baseline_roles(pool: &PgPool) -> Result<BaselineRoles, AppError> {
    let admin = upsert_role(
        pool,
        DEFAULT_ADMIN_ROLE,
        Some("Full administrative access".to_string()),
    )
    .await?;
    let editor = upsert_role(
        pool,
        DEFAULT_EDITOR_ROLE,
        Some("Workspace editing and connection access without RBAC administration".to_string()),
    )
    .await?;
    let viewer = upsert_role(
        pool,
        DEFAULT_VIEWER_ROLE,
        Some("Read-only portal access with connection launch permissions".to_string()),
    )
    .await?;
    Ok(BaselineRoles {
        admin,
        editor,
        viewer,
    })
}

/// Upsert the baseline admin/editor/viewer policies and bind each to its role.
async fn ensure_baseline_policies(pool: &PgPool, roles: &BaselineRoles) -> Result<(), AppError> {
    let admin_policy_id = upsert_full_access_policy(pool, DEFAULT_ADMIN_POLICY).await?;
    let editor_policy_id = upsert_policy(
        pool,
        DEFAULT_EDITOR_POLICY,
        &["CREATE", "READ", "UPDATE", "CONNECT", "OBSERVE", "EXECUTE"],
        &[
            "CONNECTION_TARGET",
            "CONNECTION_SESSION",
            "INVENTORY",
            "CONFIG_CONTEXT",
            "DEVICE",
            "VIRTUAL_MACHINE",
            "WEBHOOK",
            "MONITORING",
        ],
    )
    .await?;
    let viewer_policy_id = upsert_policy(
        pool,
        DEFAULT_VIEWER_POLICY,
        &["READ", "CONNECT", "OBSERVE"],
        &[
            "CONNECTION_TARGET",
            "CONNECTION_SESSION",
            "INVENTORY",
            "CONFIG_CONTEXT",
            "DEVICE",
            "VIRTUAL_MACHINE",
            "MONITORING",
        ],
    )
    .await?;

    bind_policy_to_role(pool, roles.admin, admin_policy_id).await?;
    bind_policy_to_role(pool, roles.editor, editor_policy_id).await?;
    bind_policy_to_role(pool, roles.viewer, viewer_policy_id).await?;
    Ok(())
}

/// Upsert the baseline OIDC groups and assign the matching baseline roles.
async fn ensure_baseline_groups(pool: &PgPool, roles: &BaselineRoles) -> Result<(), AppError> {
    let piping_group_id = upsert_group(pool, OIDC_GROUP_PIPING, Some("/piping")).await?;
    let admin_group_id = upsert_group(pool, OIDC_GROUP_ADMIN, Some("/piping-admin")).await?;
    let editor_group_id = upsert_group(pool, OIDC_GROUP_EDITOR, Some("/piping-editor")).await?;
    let viewer_group_id = upsert_group(pool, OIDC_GROUP_VIEWER, Some("/piping-viewer")).await?;

    assign_role_to_group(pool, piping_group_id, roles.viewer).await?;
    assign_role_to_group(pool, viewer_group_id, roles.viewer).await?;
    assign_role_to_group(pool, editor_group_id, roles.editor).await?;
    assign_role_to_group(pool, admin_group_id, roles.admin).await?;
    Ok(())
}

/// Upsert the automation role and its two policies, then bind them.
async fn ensure_automation_rbac(pool: &PgPool) -> Result<Uuid, AppError> {
    // automation: the full playbook lifecycle plus read-only access to the
    // inventory/targets a run selects. Two policies so playbook CRUD/EXECUTE does
    // not leak write access to inventory (which only needs READ for targeting).
    let automation_role_id = upsert_role(
        pool,
        DEFAULT_AUTOMATION_ROLE,
        Some("Automation operator: full playbook lifecycle plus read-only inventory".to_string()),
    )
    .await?;
    let automation_playbook_policy_id = upsert_policy(
        pool,
        DEFAULT_AUTOMATION_PLAYBOOK_POLICY,
        &["CREATE", "READ", "UPDATE", "DELETE", "EXECUTE"],
        &[
            "PLAYBOOK",
            "PLAYBOOK_VERSION",
            "PLAYBOOK_ASSIGNMENT",
            "PLAYBOOK_TARGET_SET",
            "PLAYBOOK_RUN_GROUP",
            "PLAYBOOK_RUN",
            "PLAYBOOK_SCHEDULE",
            "PLAYBOOK_ARTIFACT",
            "PLAYBOOK_JOB_TOKEN",
            "PLAYBOOK_SERVICE_PRINCIPAL",
        ],
    )
    .await?;
    let automation_targets_policy_id = upsert_policy(
        pool,
        DEFAULT_AUTOMATION_TARGETS_POLICY,
        &["READ"],
        &[
            "CONNECTION_TARGET",
            "INVENTORY",
            "DEVICE",
            "VIRTUAL_MACHINE",
            "CONFIG_CONTEXT",
        ],
    )
    .await?;
    bind_policy_to_role(pool, automation_role_id, automation_playbook_policy_id).await?;
    bind_policy_to_role(pool, automation_role_id, automation_targets_policy_id).await?;
    Ok(automation_role_id)
}

/// Upsert the read-only auditor role and policy, then bind them.
async fn ensure_auditor_rbac(pool: &PgPool) -> Result<Uuid, AppError> {
    // auditor: read-only access to the audit trail and nothing else.
    let auditor_role_id = upsert_role(
        pool,
        DEFAULT_AUDITOR_ROLE,
        Some("Read-only access to the audit log".to_string()),
    )
    .await?;
    let auditor_policy_id = upsert_policy(
        pool,
        DEFAULT_AUDITOR_POLICY,
        &["VIEW_AUDIT", "READ"],
        &["AUDIT_EVENT"],
    )
    .await?;
    bind_policy_to_role(pool, auditor_role_id, auditor_policy_id).await?;
    Ok(auditor_role_id)
}

/// Upsert the auditor and automation groups and assign their roles.
async fn ensure_auditor_and_automation_groups(
    pool: &PgPool,
    auditor_role_id: Uuid,
    automation_role_id: Uuid,
) -> Result<(), AppError> {
    let automation_group_id =
        upsert_group(pool, OIDC_GROUP_AUTOMATION, Some("/piping-automation")).await?;
    let auditor_group_id = upsert_group(pool, OIDC_GROUP_AUDITOR, Some("/piping-auditor")).await?;
    assign_role_to_group(pool, automation_group_id, automation_role_id).await?;
    assign_role_to_group(pool, auditor_group_id, auditor_role_id).await?;
    Ok(())
}

/// Seed the curated starter roles ONCE, so operators start from ready-made
/// personas instead of hand-building roles. Unlike the baseline above (which
/// upserts on every boot to self-heal system roles), these are guarded by an
/// instance marker: after the first seed, operator edits AND deletes stick —
/// they are starting points, not system-managed rows.
pub async fn ensure_curated_default_roles(pool: &PgPool) -> Result<(), AppError> {
    let already_seeded: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM settings_values WHERE scope_type = 'instance' AND user_id IS NULL AND key = $1",
    )
    .bind(CURATED_SEED_MARKER_KEY)
    .fetch_optional(pool)
    .await
    .map_err(|e| AppError::Database(format!("Failed to read curated-seed marker: {e}")))?;
    if already_seeded.is_some() {
        return Ok(());
    }

    for curated in CURATED_ROLES {
        let role_id =
            upsert_role(pool, curated.role, Some(curated.description.to_string())).await?;
        let policy_id = upsert_policy(
            pool,
            curated.policy,
            curated.actions,
            curated.resource_types,
        )
        .await?;
        bind_policy_to_role(pool, role_id, policy_id).await?;
    }

    sqlx::query(
        r#"
        INSERT INTO settings_values (id, scope_type, user_id, key, value)
        VALUES (gen_random_uuid(), 'instance', NULL, $1, $2)
        ON CONFLICT (key) WHERE scope_type = 'instance' DO NOTHING
        "#,
    )
    .bind(CURATED_SEED_MARKER_KEY)
    .bind(json!({ "version": 1 }))
    .execute(pool)
    .await
    .map_err(|e| AppError::Database(format!("Failed to write curated-seed marker: {e}")))?;

    Ok(())
}

pub async fn ensure_cctv_operator_role(pool: &PgPool) -> Result<(), AppError> {
    let already_seeded: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM settings_values WHERE scope_type = 'instance' AND user_id IS NULL AND key = $1",
    )
    .bind(CCTV_CURATED_SEED_MARKER_KEY)
    .fetch_optional(pool)
    .await
    .map_err(|error| AppError::Database(format!("Failed to read CCTV role marker: {error}")))?;
    if already_seeded.is_some() {
        return Ok(());
    }

    let role_id = upsert_role(
        pool,
        CCTV_OPERATOR_ROLE.role,
        Some(CCTV_OPERATOR_ROLE.description.to_string()),
    )
    .await?;
    let policy_id = upsert_policy(
        pool,
        CCTV_OPERATOR_ROLE.policy,
        CCTV_OPERATOR_ROLE.actions,
        CCTV_OPERATOR_ROLE.resource_types,
    )
    .await?;
    bind_policy_to_role(pool, role_id, policy_id).await?;

    sqlx::query(
        r#"
        INSERT INTO settings_values (id, scope_type, user_id, key, value)
        VALUES (gen_random_uuid(), 'instance', NULL, $1, $2)
        ON CONFLICT (key) WHERE scope_type = 'instance' DO NOTHING
        "#,
    )
    .bind(CCTV_CURATED_SEED_MARKER_KEY)
    .bind(json!({ "version": 1 }))
    .execute(pool)
    .await
    .map_err(|error| AppError::Database(format!("Failed to write CCTV role marker: {error}")))?;

    Ok(())
}

pub async fn upsert_credentials(
    pool: &PgPool,
    user_id: Uuid,
    password: &str,
    pepper: &str,
    password_min_length: usize,
) -> Result<(), AppError> {
    // Bootstrap path still follows the same local password policy as runtime user management.
    validate_local_password(password, password_min_length)?;

    let salted = format!("{password}{pepper}");
    let salt = SaltString::encode_b64(Uuid::now_v7().as_bytes())
        .map_err(|e| AppError::Validation(format!("Failed to build salt: {e}")))?;
    let hash = Argon2::default()
        .hash_password(salted.as_bytes(), &salt)
        .map_err(|e| AppError::Validation(format!("Failed to hash password: {e}")))?
        .to_string();

    sqlx::query(
        r#"
        INSERT INTO auth_user_credential (user_id, password_hash, password_changed_at)
        VALUES ($1, $2, $3)
        ON CONFLICT (user_id)
        DO UPDATE SET password_hash = EXCLUDED.password_hash,
                      password_changed_at = EXCLUDED.password_changed_at
        "#,
    )
    .bind(user_id)
    .bind(hash)
    .bind(Utc::now())
    .execute(pool)
    .await
    .map_err(|e| AppError::Database(format!("Failed to upsert credentials: {e}")))?;

    Ok(())
}

pub async fn upsert_role(
    pool: &PgPool,
    name: &str,
    description: Option<String>,
) -> Result<Uuid, AppError> {
    let row = sqlx::query(
        r#"
        INSERT INTO rbac_role (id, name, description)
        VALUES ($1, $2, $3)
        ON CONFLICT (name) DO UPDATE SET description = EXCLUDED.description
        RETURNING id
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(name)
    .bind(description)
    .fetch_one(pool)
    .await
    .map_err(|e| AppError::Database(format!("Failed to upsert role: {e}")))?;

    row.try_get("id")
        .map_err(|e| AppError::Database(format!("Failed to read role id: {e}")))
}

pub async fn upsert_full_access_policy(pool: &PgPool, name: &str) -> Result<Uuid, AppError> {
    let actions = full_access_policy_actions();
    let resource_types = full_access_policy_resource_types();

    let row = sqlx::query(
        r#"
        INSERT INTO rbac_policy (id, name, effect, actions, resource_types, constraints, subjects, resources, enabled)
        VALUES ($1, $2, 'allow', $3, $4, NULL, '[]'::jsonb, '["*"]'::jsonb, true)
        ON CONFLICT (name)
        DO UPDATE SET actions = EXCLUDED.actions,
                      resource_types = EXCLUDED.resource_types,
                      resources = EXCLUDED.resources,
                      enabled = EXCLUDED.enabled
        RETURNING id
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(name)
    .bind(actions)
    .bind(resource_types)
    .fetch_one(pool)
    .await
    .map_err(|e| AppError::Database(format!("Failed to upsert policy: {e}")))?;

    row.try_get("id")
        .map_err(|e| AppError::Database(format!("Failed to read policy id: {e}")))
}

fn full_access_policy_actions() -> Value {
    json!(Action::ALL.as_slice())
}

fn full_access_policy_resource_types() -> Value {
    json!(ResourceType::ALL.as_slice())
}

pub async fn upsert_policy(
    pool: &PgPool,
    name: &str,
    actions: &[&str],
    resource_types: &[&str],
) -> Result<Uuid, AppError> {
    let row = sqlx::query(
        r#"
        INSERT INTO rbac_policy (id, name, effect, actions, resource_types, constraints, subjects, resources, enabled)
        VALUES ($1, $2, 'allow', $3, $4, NULL, '[]'::jsonb, '["*"]'::jsonb, true)
        ON CONFLICT (name)
        DO UPDATE SET actions = EXCLUDED.actions,
                      resource_types = EXCLUDED.resource_types,
                      resources = EXCLUDED.resources,
                      enabled = EXCLUDED.enabled
        RETURNING id
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(name)
    .bind(json!(actions))
    .bind(json!(resource_types))
    .fetch_one(pool)
    .await
    .map_err(|e| AppError::Database(format!("Failed to upsert policy: {e}")))?;

    row.try_get("id")
        .map_err(|e| AppError::Database(format!("Failed to read policy id: {e}")))
}

pub async fn upsert_group(
    pool: &PgPool,
    name: &str,
    external_id: Option<&str>,
) -> Result<Uuid, AppError> {
    let row = sqlx::query(
        r#"
        INSERT INTO identity_group (id, name, external_id)
        VALUES ($1, $2, $3)
        ON CONFLICT (name) DO UPDATE SET external_id = EXCLUDED.external_id
        RETURNING id
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(name)
    .bind(external_id)
    .fetch_one(pool)
    .await
    .map_err(|e| AppError::Database(format!("Failed to upsert group: {e}")))?;

    row.try_get("id")
        .map_err(|e| AppError::Database(format!("Failed to read group id: {e}")))
}

pub async fn bind_policy_to_role(
    pool: &PgPool,
    role_id: Uuid,
    policy_id: Uuid,
) -> Result<(), AppError> {
    // Subject unification: the `rbac_policy_binding` linkage table is gone. A
    // role grant is now a canonical `role:{uuid}` entry in the policy's
    // `subjects` JSONB array (matched by the same `subjects ?|` operator as
    // user/group subjects). Append idempotently, deduplicating existing entries.
    sqlx::query(
        r#"
        UPDATE rbac_policy
        SET subjects = (
            SELECT COALESCE(jsonb_agg(DISTINCT elem), '[]'::jsonb)
            FROM (
                SELECT jsonb_array_elements_text(subjects) AS elem
                UNION
                SELECT 'role:' || $1::text
            ) merged
        )
        WHERE id = $2
        "#,
    )
    .bind(role_id)
    .bind(policy_id)
    .execute(pool)
    .await
    .map_err(|e| AppError::Database(format!("Failed to bind policy to role: {e}")))?;

    Ok(())
}

pub async fn assign_role_to_user(
    pool: &PgPool,
    user_id: Uuid,
    role_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO rbac_role_assignment (id, user_id, group_id, role_id)
        VALUES ($1, $2, NULL, $3)
        ON CONFLICT (user_id, role_id) DO NOTHING
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(role_id)
    .execute(pool)
    .await
    .map_err(|e| AppError::Database(format!("Failed to assign role: {e}")))?;

    Ok(())
}

pub async fn assign_role_to_group(
    pool: &PgPool,
    group_id: Uuid,
    role_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO rbac_role_assignment (id, user_id, group_id, role_id)
        VALUES ($1, NULL, $2, $3)
        ON CONFLICT (group_id, role_id) DO NOTHING
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(group_id)
    .bind(role_id)
    .execute(pool)
    .await
    .map_err(|e| AppError::Database(format!("Failed to assign group role: {e}")))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use serde::Serialize;
    use serde_json::Value;

    use super::{full_access_policy_actions, full_access_policy_resource_types};
    use crate::contexts::access_control::application::user_api_token_scope_catalog_pairs;

    fn string_values(value: &Value) -> HashSet<String> {
        value
            .as_array()
            .expect("full-access policy values must be arrays")
            .iter()
            .map(|item| {
                item.as_str()
                    .expect("full-access policy values must be strings")
                    .to_string()
            })
            .collect()
    }

    fn serialized_string(value: impl Serialize) -> String {
        serde_json::to_value(value)
            .expect("policy enum serializes")
            .as_str()
            .expect("policy enum serializes as string")
            .to_string()
    }

    #[test]
    fn full_access_policy_covers_every_grantable_pat_scope() {
        let actions = string_values(&full_access_policy_actions());
        let resource_types = string_values(&full_access_policy_resource_types());

        for (action, resource_type) in user_api_token_scope_catalog_pairs() {
            assert!(
                actions.contains(&serialized_string(action)),
                "admin full-access must grant PAT action {action:?}"
            );
            assert!(
                resource_types.contains(&serialized_string(resource_type)),
                "admin full-access must grant PAT resource {resource_type:?}"
            );
        }
    }
}
