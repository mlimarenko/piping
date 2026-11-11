//! Isolated child databases for product tests.

use crate::common::database_url;
use sqlx::{Connection, PgConnection};
use uuid::Uuid;

pub(crate) struct ChildDatabase {
    connection: PgConnection,
    name: String,
    oid: i64,
    provider: String,
}

impl ChildDatabase {
    pub(crate) fn validate_url(
        prefix: &str,
        value: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if !matches!(
            prefix,
            "compute_atomic_"
                | "org_atomic_"
                | "playbook_catalog_"
                | "pwa78_migration_"
                | "pwa80_vlan_"
                | "pwa81_transfer_"
        ) {
            return Err("invalid child database prefix".into());
        }
        let child = url::Url::parse(value)?;
        let name = child
            .path()
            .strip_prefix('/')
            .ok_or("invalid child database path")?;
        let suffix = name
            .strip_prefix(prefix)
            .ok_or("invalid child database name")?;
        if suffix.len() != 32 || !suffix.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("invalid child database identifier".into());
        }
        let mut expected = url::Url::parse(&database_url())?;
        expected.set_path(&format!("/{name}"));
        if child != expected {
            return Err("child database provider differs from fixture provider".into());
        }
        Ok(())
    }

    pub(crate) async fn create(
        prefix: &str,
    ) -> Result<(Self, String), Box<dyn std::error::Error + Send + Sync>> {
        let name = format!("{prefix}{}", Uuid::now_v7().simple());
        let base_url = database_url();
        let mut url = url::Url::parse(&base_url)?;
        url.set_path(&format!("/{name}"));
        Self::validate_url(prefix, url.as_str())?;
        let mut connection = PgConnection::connect(&base_url).await?;
        // The server can create the child even if the response is lost.
        eprintln!("PWA79_CHILD_DB {name} unknown");
        let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new("CREATE DATABASE \"");
        query.push(&name).push("\"");
        query.build().execute(&mut connection).await?;
        // Even an identity lookup failure leaves a recoverable, named child.
        let oid: i64 = sqlx::query_scalar("SELECT oid::bigint FROM pg_database WHERE datname = $1")
            .bind(&name)
            .fetch_one(&mut connection)
            .await?;
        eprintln!("PWA79_CHILD_DB {name} {oid}");
        let provider: String =
            sqlx::query_scalar("SELECT system_identifier::text FROM pg_control_system()")
                .fetch_one(&mut connection)
                .await?;
        eprintln!("PWA79_CHILD_PROVIDER {name} {provider}");
        Ok((
            Self {
                connection,
                name,
                oid,
                provider,
            },
            url.to_string(),
        ))
    }

    pub(crate) async fn cleanup(mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let provider: String =
            sqlx::query_scalar("SELECT system_identifier::text FROM pg_control_system()")
                .fetch_one(&mut self.connection)
                .await?;
        // A server lists a backend until it processes the client disconnect, so a
        // pool closed a moment earlier can still appear as one connection. Wait
        // for that observable state; the guard below still refuses if a real
        // client keeps the database open.
        let mut open_backends = u32::MAX;
        for _ in 0..100 {
            open_backends = sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM pg_stat_activity a JOIN pg_database d ON d.oid = a.datid WHERE d.datname = $1",
            )
            .bind(&self.name)
            .fetch_one(&mut self.connection)
            .await? as u32;
            if open_backends == 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let identity: Option<(i64, i64)> = sqlx::query_as(
            "SELECT d.oid::bigint, (SELECT count(*) FROM pg_stat_activity a WHERE a.datid = d.oid) FROM pg_database d WHERE d.datname = $1",
        )
        .bind(&self.name)
        .fetch_optional(&mut self.connection)
        .await?;
        if provider != self.provider || identity != Some((self.oid, 0)) {
            return Err(format!(
                "child database guard refused: {} oid {}",
                self.name, self.oid
            )
            .into());
        }
        // Ordinary DROP refuses a connection that appears after the activity check.
        let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new("DROP DATABASE \"");
        query.push(&self.name).push("\"");
        query.build().execute(&mut self.connection).await?;
        Ok(())
    }
}
