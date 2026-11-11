//! REST proof for the builtin catalog lookup: the endpoint is behind the normal
//! job-template read permission, and startup seeding is visible through it.

mod common;
#[path = "common/isolated_app.rs"]
mod isolated_app;

use std::path::Path;

use axum::http::StatusCode;
use piping_backend::contexts::playbooks::application::CreateProjectInput;
use piping_backend::contexts::playbooks::application::catalog_loader::load_system_catalog;
use piping_backend::contexts::playbooks::domain::{ProjectSource, job_id, project_id};
use piping_backend::contexts::playbooks::infrastructure::persistence::SqlxPlaybookRepository;
use uuid::Uuid;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn shipped_catalog() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../ansible/playbooks")
        .to_string_lossy()
        .into_owned()
}

fn copy_catalog(source: &Path, destination: &Path) -> TestResult {
    std::fs::create_dir_all(destination)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_catalog(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

async fn group_and_run_counts(pool: &sqlx::PgPool, template: Uuid) -> TestResult<(i64, i64)> {
    let groups =
        sqlx::query_scalar("SELECT count(*) FROM playbook_run_group WHERE template_id = $1")
            .bind(template)
            .fetch_one(pool)
            .await?;
    let runs = sqlx::query_scalar("SELECT count(*) FROM playbook_run r JOIN playbook_run_group g ON g.id = r.run_group_id WHERE g.template_id = $1")
        .bind(template).fetch_one(pool).await?;
    Ok((groups, runs))
}

#[tokio::test]
#[ignore = "requires PostgreSQL and Redis for the isolated HTTP harness"]
async fn builtin_lookup_reports_startup_seed_and_requires_permission() -> TestResult {
    let catalog = shipped_catalog();
    let harness = isolated_app::harness_with("playbook_catalog_api", 15, |settings| {
        // The packaged tree is at a different absolute path in the test process.
        settings.playbook_runtime.catalog_path = Some(catalog);
        // No execution environment is enabled for builtin jobs in this instance.
        settings.playbook_runtime.default_execution_environment_id = None;
    })
    .await?;
    let outcome = async {
        let (status, body) = common::request(
            &harness.app,
            "GET",
            "/api/v1/playbooks/catalog/system-auto-update-inventory",
            None,
            Some(&harness.cookie),
        )
        .await?;
        assert_eq!(status, StatusCode::OK);
        let body: serde_json::Value = serde_json::from_slice(&body)?;
        assert_eq!(body["slug"], "system-auto-update-inventory");
        assert_eq!(body["available"], false);
        assert_eq!(
            body["unavailable_reason"],
            "execution_environment_not_configured"
        );
        assert!(body["template_id"].is_null());

        // The lookup stays behind the existing authorization gate.
        let (anonymous, _) = common::request(
            &harness.app,
            "GET",
            "/api/v1/playbooks/catalog/system-auto-update-inventory",
            None,
            None,
        )
        .await?;
        assert_eq!(anonymous, StatusCode::UNAUTHORIZED);

        // A slug outside the catalog is not a builtin job.
        let (missing, _) = common::request(
            &harness.app,
            "GET",
            "/api/v1/playbooks/catalog/not-a-catalog-entry",
            None,
            Some(&harness.cookie),
        )
        .await?;
        assert_eq!(missing, StatusCode::NOT_FOUND);

        // Startup seeded the project and its immutable revision, and no job
        // template without a configured execution environment.
        let projects: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM playbook_project WHERE source->>'kind' = 'system'",
        )
        .fetch_one(&harness.state.db_pool)
        .await?;
        assert_eq!(projects, 1);
        let revisions: i64 = sqlx::query_scalar("SELECT count(*) FROM playbook_project_revision")
            .fetch_one(&harness.state.db_pool)
            .await?;
        assert_eq!(revisions, 1);
        let templates: i64 = sqlx::query_scalar("SELECT count(*) FROM playbook_job_template")
            .fetch_one(&harness.state.db_pool)
            .await?;
        assert_eq!(templates, 0);
        Ok(())
    }
    .await;
    isolated_app::shutdown(harness).await?;
    outcome
}

#[tokio::test]
#[ignore = "requires PostgreSQL and Redis for the isolated HTTP harness"]
async fn builtin_revision_selection_obeys_project_boundary() -> TestResult {
    let harness = isolated_app::harness_with("playbook_catalog_api", 15, |settings| {
        settings.playbook_runtime.catalog_path = Some(shipped_catalog());
        settings.playbook_runtime.default_execution_environment_id =
            Some(Uuid::parse_str("00000000-0000-7000-8000-000000000301").expect("seeded EE"));
    })
    .await?;
    let outcome = async {
        let service = &harness.state.services.playbooks;
        let builtin = service
            .get_project(project_id())
            .await?
            .expect("system project");
        let fixed = builtin.current_revision_id.expect("system revision");
        let other = service
            .create_project(
                CreateProjectInput {
                    name: "Operator project".into(),
                    description: None,
                    source: ProjectSource::Editable,
                },
                None,
            )
            .await?;
        let foreign = other.current_revision_id.expect("editable revision");
        let uri = format!(
            "/api/v1/playbooks/job-templates/{}",
            job_id("system-auto-update-inventory")
        );
        let patch = |revision: Uuid| {
            serde_json::json!({"revision": {"kind": "fixed", "revision_id": revision}}).to_string()
        };
        let (status, _) = common::request(
            &harness.app,
            "PATCH",
            &uri,
            Some(&patch(fixed)),
            Some(&harness.cookie),
        )
        .await?;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = common::request(
            &harness.app,
            "PATCH",
            &uri,
            Some(&patch(foreign)),
            Some(&harness.cookie),
        )
        .await?;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let stored = service
            .get_job_template(job_id("system-auto-update-inventory"))
            .await?
            .expect("builtin job");
        assert_eq!(
            stored.revision,
            piping_backend::contexts::playbooks::domain::RevisionSelection::Fixed {
                revision_id: fixed
            }
        );
        Ok(())
    }
    .await;
    isolated_app::shutdown(harness).await?;
    outcome
}

#[tokio::test]
#[ignore = "requires PostgreSQL and Redis for the isolated HTTP harness"]
async fn builtin_fixed_launch_uses_selected_snapshot_and_rejects_incompatible_snapshots()
-> TestResult {
    let scratch = tempfile::tempdir()?;
    copy_catalog(Path::new(&shipped_catalog()), scratch.path())?;
    let root = scratch.path().to_path_buf();
    let ee = Uuid::parse_str("00000000-0000-7000-8000-000000000301")?;
    let catalog_path = root.to_string_lossy().into_owned();
    let harness = isolated_app::harness_with("playbook_catalog_api", 15, |settings| {
        settings.playbook_runtime.catalog_path = Some(catalog_path);
        settings.playbook_runtime.default_execution_environment_id = Some(ee);
    })
    .await?;
    let outcome = async {
        let pool = &harness.state.db_pool;
        let service = &harness.state.services.playbooks;
        let template = job_id("system-auto-update-inventory");
        let uri = format!("/api/v1/playbooks/job-templates/{template}");
        let launch_uri = format!("{uri}/launch");
        let project = service.get_project(project_id()).await?.expect("system project");
        let revision_a = project.current_revision_id.expect("catalog revision A");
        let tree_a = service.get_project_revision(project.id, revision_a).await?.expect("revision A").tree_sha256;
        // The stock migration uses an unpinned local image; this isolated fixture
        // sets the same digest-shaped image used by the native launch tests.
        sqlx::query("UPDATE playbook_execution_environment SET image = $2 WHERE id = $1")
            .bind(ee).bind(format!("registry.example/ansible@sha256:{}", "0".repeat(64)))
            .execute(pool).await?;
        let update = serde_json::json!({
            "revision": {"kind": "fixed", "revision_id": revision_a},
            "inventory": {"kind": "native", "sources": ["inventory/instance.yml"], "ssh_record_id": null}
        }).to_string();
        let (status, body) = common::request(&harness.app, "PATCH", &uri, Some(&update), Some(&harness.cookie)).await?;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        let schema_a = service.get_job_template_vars_schema(template).await?
            .expect("template schema").revision.expect("schema revision");

        let playbook = root.join("system/auto-update-inventory.yml");
        let mut contents = std::fs::read_to_string(&playbook)?;
        contents.push_str("\n# catalog revision B\n");
        std::fs::write(&playbook, contents)?;
        let schema_path = root.join("system/auto-update-inventory.vars-schema.json");
        let schema = std::fs::read_to_string(&schema_path)?;
        std::fs::write(&schema_path, schema.replace("Update OS platform", "Updated OS platform"))?;
        let repository = SqlxPlaybookRepository::new(pool.clone());
        let second = load_system_catalog(&root, Some(ee), &repository).await?;
        assert_ne!(revision_a, second.revision_id);
        assert_ne!(tree_a, second.tree_sha256);
        let (status, body) = common::request(&harness.app, "POST", &launch_uri, Some("{}"), Some(&harness.cookie)).await?;
        assert_eq!(status, StatusCode::CREATED, "{}", String::from_utf8_lossy(&body));
        let created: serde_json::Value = serde_json::from_slice(&body)?;
        let spec = &created["run_group"]["launch_spec"];
        assert_eq!(spec["revision_id"], revision_a.to_string());
        assert_eq!(spec["tree_sha256"], tree_a);
        assert_eq!(spec["vars_schema"]["revision"], schema_a);
        let group_id = Uuid::parse_str(created["run_group"]["id"].as_str().ok_or("missing run group id")?)?;
        let persisted: serde_json::Value = sqlx::query_scalar("SELECT launch_spec FROM playbook_run_group WHERE id = $1")
            .bind(group_id).fetch_one(pool).await?;
        assert_eq!(&persisted, spec);
        let counts = group_and_run_counts(pool, template).await?;
        assert_eq!(counts.0, 1);

        let other = service.create_project(CreateProjectInput {
            name: "Foreign project for revision test".into(), description: None, source: ProjectSource::Editable,
        }, None).await?;
        let foreign = other.current_revision_id.expect("foreign revision");
        let foreign_patch = serde_json::json!({"revision": {"kind": "fixed", "revision_id": foreign}}).to_string();
        let (status, _) = common::request(&harness.app, "PATCH", &uri, Some(&foreign_patch), Some(&harness.cookie)).await?;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(group_and_run_counts(pool, template).await?, counts);

        let manifest_path = root.join("piping-playbooks.yaml");
        let manifest = std::fs::read_to_string(&manifest_path)?;
        std::fs::copy(&playbook, root.join("system/renamed-inventory.yml"))?;
        std::fs::write(&manifest_path, manifest.replace("entrypoint_file: system/auto-update-inventory.yml", "entrypoint_file: system/renamed-inventory.yml"))?;
        load_system_catalog(&root, Some(ee), &repository).await?;
        let (status, body) = common::request(&harness.app, "POST", &launch_uri, Some("{}"), Some(&harness.cookie)).await?;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{}", String::from_utf8_lossy(&body));
        assert_eq!(group_and_run_counts(pool, template).await?, counts);

        let schema = root.join("system/auto-update-inventory.vars-schema.json");
        std::fs::copy(&schema, root.join("system/renamed-inventory.vars-schema.json"))?;
        let manifest = std::fs::read_to_string(&manifest_path)?;
        let manifest = manifest.replace("entrypoint_file: system/renamed-inventory.yml", "entrypoint_file: system/auto-update-inventory.yml")
            .replace("vars_schema_file: system/auto-update-inventory.vars-schema.json", "vars_schema_file: system/renamed-inventory.vars-schema.json");
        std::fs::write(&manifest_path, manifest)?;
        load_system_catalog(&root, Some(ee), &repository).await?;
        let (status, body) = common::request(&harness.app, "POST", &launch_uri, Some("{}"), Some(&harness.cookie)).await?;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{}", String::from_utf8_lossy(&body));
        assert_eq!(group_and_run_counts(pool, template).await?, counts);
        Ok(())
    }.await;
    isolated_app::shutdown(harness).await?;
    outcome
}
