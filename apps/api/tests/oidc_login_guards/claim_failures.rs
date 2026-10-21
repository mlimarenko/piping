use std::sync::Arc;

use piping_backend::contexts::access_control::application::AuthService;
use piping_backend::contexts::access_control::domain::User;
use piping_backend::contexts::access_control::infrastructure::persistence::{
    AuthSessionRepository, SqlxAuthSessionRepository, SqlxUserCredentialRepository,
    SqlxUserRepository, UserCredentialRepository, UserRepository,
};
use uuid::Uuid;

use super::{BoxError, TestResult, TokenPlan, UserinfoPlan, exchange_with, spawn_fake_idp};

#[tokio::test]
async fn present_empty_group_claim_is_authoritative() -> TestResult {
    let idp = spawn_fake_idp().await?;
    let result = exchange_with(
        &idp,
        |login| TokenPlan {
            userinfo: UserinfoPlan::Groups(Vec::new()),
            ..TokenPlan::valid(&login.nonce)
        },
        |login| login.nonce.clone(),
    )
    .await?;

    let claims = result.map_err(|error| format!("present empty claim was rejected: {error:?}"))?;
    assert!(claims.groups.is_empty());
    Ok(())
}

#[tokio::test]
#[ignore = "requires PostgreSQL"]
async fn userinfo_failure_issues_no_session_and_preserves_memberships() -> TestResult {
    assert_failed_claim_fetch_preserves_authorization(UserinfoPlan::ServerError).await
}

#[tokio::test]
#[ignore = "requires PostgreSQL"]
async fn missing_group_claim_issues_no_session_and_preserves_memberships() -> TestResult {
    assert_failed_claim_fetch_preserves_authorization(UserinfoPlan::MissingGroupClaim).await
}

async fn assert_failed_claim_fetch_preserves_authorization(userinfo: UserinfoPlan) -> TestResult {
    let expected_error = match &userinfo {
        UserinfoPlan::MissingGroupClaim => "did not include configured group claim",
        UserinfoPlan::ServerError => "user info failed",
        UserinfoPlan::Groups(_) => return Err("failure test requires unavailable claims".into()),
    };
    let idp = spawn_fake_idp().await?;
    let pool = super::common::build_test_db_pool().await?;
    piping_backend::storage::database::migrations::run_migrations(&pool).await?;

    let fixture = AuthorizationFixture::insert(&pool).await?;
    let exchange = exchange_with(
        &idp,
        |login| TokenPlan {
            subject: fixture.subject.clone(),
            userinfo,
            ..TokenPlan::valid(&login.nonce)
        },
        |login| login.nonce.clone(),
    )
    .await?;

    let login_result = match exchange {
        Ok(claims) => login_with_claims(&idp, &pool, claims).await,
        Err(error) => Err(BoxError::from(error)),
    };

    let memberships = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT group_id
        FROM identity_group_membership
        WHERE user_id = $1 AND source = 'oidc'
        "#,
    )
    .bind(fixture.user_id)
    .fetch_all(&pool)
    .await?;
    let session_count =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM auth_session WHERE user_id = $1")
            .bind(fixture.user_id)
            .fetch_one(&pool)
            .await?;
    let rejected_for_claim_failure = login_result.as_ref().is_err_and(|error| {
        error
            .to_string()
            .to_ascii_lowercase()
            .contains(expected_error)
    });

    assert!(
        rejected_for_claim_failure && memberships == vec![fixture.group_id] && session_count == 0,
        "non-authoritative OIDC claims changed authorization: rejected={}, memberships={memberships:?}, sessions={session_count}",
        rejected_for_claim_failure,
    );

    fixture.delete(&pool).await?;
    Ok(())
}

async fn login_with_claims(
    idp: &super::FakeIdp,
    pool: &sqlx::PgPool,
    claims: piping_backend::integrations::oidc::OidcClaims,
) -> Result<User, BoxError> {
    let users: Arc<dyn UserRepository> = Arc::new(SqlxUserRepository::new(pool.clone()));
    let credentials: Arc<dyn UserCredentialRepository> =
        Arc::new(SqlxUserCredentialRepository::new(pool.clone()));
    let sessions: Arc<dyn AuthSessionRepository> =
        Arc::new(SqlxAuthSessionRepository::new(pool.clone()));
    let auth = AuthService::new(users, credentials, sessions, idp.auth_config()?);

    auth.authenticate_oidc(
        &claims.subject,
        claims.preferred_username.as_deref(),
        claims.email.as_deref(),
        &claims.groups,
        piping_backend::contexts::access_control::domain::OidcProvisioningMode::Manual,
    )
    .await
    .map_err(BoxError::from)
}

struct AuthorizationFixture {
    user_id: Uuid,
    group_id: Uuid,
    subject: String,
}

impl AuthorizationFixture {
    async fn insert(pool: &sqlx::PgPool) -> Result<Self, BoxError> {
        let user_id = Uuid::now_v7();
        let group_id = Uuid::now_v7();
        let subject = format!("oidc-test-{user_id}");

        sqlx::query(
            r#"
            INSERT INTO identity_user (id, username, email, status, auth_source, external_id)
            VALUES ($1, $2, $3, 'active', 'oidc', $4)
            "#,
        )
        .bind(user_id)
        .bind(format!("oidc-{user_id}"))
        .bind(format!("oidc-{user_id}@example.test"))
        .bind(&subject)
        .execute(pool)
        .await?;
        sqlx::query("INSERT INTO identity_group (id, name, external_id) VALUES ($1, $2, $3)")
            .bind(group_id)
            .bind(format!("deny-{group_id}"))
            .bind(format!("deny-external-{group_id}"))
            .execute(pool)
            .await?;
        sqlx::query(
            r#"
            INSERT INTO identity_group_membership (user_id, group_id, source, external_id)
            VALUES ($1, $2, 'oidc', $3)
            "#,
        )
        .bind(user_id)
        .bind(group_id)
        .bind(format!("deny-external-{group_id}"))
        .execute(pool)
        .await?;

        Ok(Self {
            user_id,
            group_id,
            subject,
        })
    }

    async fn delete(self, pool: &sqlx::PgPool) -> Result<(), BoxError> {
        sqlx::query("DELETE FROM identity_user WHERE id = $1")
            .bind(self.user_id)
            .execute(pool)
            .await?;
        sqlx::query("DELETE FROM identity_group WHERE id = $1")
            .bind(self.group_id)
            .execute(pool)
            .await?;
        Ok(())
    }
}
