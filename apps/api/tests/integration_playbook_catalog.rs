//! Builtin playbook catalog tests on an owned child database.
//!
//! The canonical project id is product-owned and constant, so these tests run on
//! a child database instead of the shared fixture. They exercise the real
//! startup loader, the real repository upsert and the real availability lookup.

#[path = "common/child_database.rs"]
mod child_database;
mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Utc;
use sqlx::{PgPool, migrate::Migrator};
use uuid::Uuid;

use child_database::ChildDatabase;
use piping_backend::contexts::playbooks::application::catalog_loader::load_system_catalog;
use piping_backend::contexts::playbooks::application::{
    CreateJobTemplateInput, CreateProjectInput, PlaybookService, UpdateJobTemplateInput,
    UpdateProjectInput,
};
use piping_backend::contexts::playbooks::domain::{
    BuiltinCatalog, BuiltinUnavailableReason, InventorySelection, JobTemplate, LaunchOptions,
    ProjectEntry, ProjectEntryContent, ProjectSource, RevisionChange, RevisionSelection,
    VarsSchemaSource, job_id, project_id,
};
use piping_backend::contexts::playbooks::infrastructure::persistence::{
    PlaybookRepository, SqlxPlaybookRepository,
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Execution environment seeded by `0001_init.sql`; an operator still has to name
/// it in `playbook_runtime.default_execution_environment_id`.
fn seeded_execution_environment() -> Uuid {
    Uuid::parse_str("00000000-0000-7000-8000-000000000301").expect("seeded execution environment")
}

const INVENTORY_SLUG: &str = "system-auto-update-inventory";
const EXPORTER_SLUG: &str = "system-exporter-discovery";

fn shipped_catalog() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ansible/playbooks")
}

struct Fixture {
    child: ChildDatabase,
    pool: PgPool,
}

impl Fixture {
    fn repository(&self) -> SqlxPlaybookRepository {
        SqlxPlaybookRepository::new(self.pool.clone())
    }

    async fn catalog_counts(&self) -> TestResult<(i64, i64, i64)> {
        let projects: i64 = sqlx::query_scalar("SELECT count(*) FROM playbook_project")
            .fetch_one(&self.pool)
            .await?;
        let revisions: i64 = sqlx::query_scalar("SELECT count(*) FROM playbook_project_revision")
            .fetch_one(&self.pool)
            .await?;
        let templates: i64 = sqlx::query_scalar("SELECT count(*) FROM playbook_job_template")
            .fetch_one(&self.pool)
            .await?;
        Ok((projects, revisions, templates))
    }

    async fn current_revision(&self) -> TestResult<Option<Uuid>> {
        Ok(
            sqlx::query_scalar("SELECT current_revision_id FROM playbook_project WHERE id = $1")
                .bind(project_id())
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    async fn template_options(&self, id: Uuid) -> TestResult<LaunchOptions> {
        let options: serde_json::Value =
            sqlx::query_scalar("SELECT options FROM playbook_job_template WHERE id = $1")
                .bind(id)
                .fetch_one(&self.pool)
                .await?;
        Ok(serde_json::from_value(options)?)
    }

    async fn template_inventory(&self, id: Uuid) -> TestResult<InventorySelection> {
        let inventory: serde_json::Value =
            sqlx::query_scalar("SELECT inventory FROM playbook_job_template WHERE id = $1")
                .bind(id)
                .fetch_one(&self.pool)
                .await?;
        Ok(serde_json::from_value(inventory)?)
    }
}

async fn fixture() -> TestResult<Fixture> {
    let (child, url) = ChildDatabase::create("playbook_catalog_").await?;
    let pool = PgPool::connect(&url).await?;
    Migrator::new(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("migrations")
            .as_path(),
    )
    .await?
    .run(&pool)
    .await?;
    Ok(Fixture { child, pool })
}

/// A second isolated child database for tests that need a fresh catalog state.
async fn extra_fixture() -> TestResult<Fixture> {
    fixture().await
}

async fn teardown(fixture: Fixture) -> TestResult {
    fixture.pool.close().await;
    fixture.child.cleanup().await?;
    Ok(())
}

/// Copy the shipped catalog so a test can change its content. Symlinks are
/// rejected by the loader, and the shipped tree has none.
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

#[tokio::test]
#[ignore = "requires the guarded playbook_catalog_ child database fixture and PostgreSQL"]
async fn catalog_seed_is_idempotent_and_seeds_both_builtin_jobs() -> TestResult {
    let fixture = fixture().await?;
    let outcome = async {
        let repository = fixture.repository();
        let first = load_system_catalog(
            &shipped_catalog(),
            Some(seeded_execution_environment()),
            &repository,
        )
        .await?;
        assert_eq!(first.project_id, project_id());
        assert_eq!(first.templates.len(), 2);
        assert_eq!(fixture.catalog_counts().await?, (1, 1, 2));

        let second = load_system_catalog(
            &shipped_catalog(),
            Some(seeded_execution_environment()),
            &repository,
        )
        .await?;
        // A repeated start writes nothing new and keeps every identity.
        assert_eq!(second, first);
        assert_eq!(fixture.catalog_counts().await?, (1, 1, 2));
        assert_eq!(fixture.current_revision().await?, Some(first.revision_id));

        for slug in [INVENTORY_SLUG, EXPORTER_SLUG] {
            let template = first.template(slug).expect("seeded builtin entry");
            assert_eq!(template.template_id, Some(job_id(slug)));
            let options = fixture
                .template_options(template.template_id.expect("template id"))
                .await?;
            // The manifest timeout is the only supported launch setting besides
            // the approved argument matrix, which is empty here.
            assert!(options.argv.is_empty());
            assert!(options.env.is_empty());
            assert!(options.timeout_seconds > 0);
        }
        Ok(())
    }
    .await;
    teardown(fixture).await?;
    outcome
}

#[tokio::test]
#[ignore = "requires the guarded playbook_catalog_ child database fixture and PostgreSQL"]
async fn changed_catalog_appends_an_immutable_revision_and_keeps_instance_settings() -> TestResult {
    let fixture = fixture().await?;
    let outcome = async {
        let scratch = std::env::temp_dir().join(format!("piping-catalog-{}", Uuid::now_v7().simple()));
        copy_catalog(&shipped_catalog(), &scratch)?;
        let repository = fixture.repository();
        let first =
            load_system_catalog(&scratch, Some(seeded_execution_environment()), &repository).await?;
        let template_id = first
            .template(INVENTORY_SLUG)
            .and_then(|template| template.template_id)
            .expect("seeded inventory template");

        let service = PlaybookService::new(Arc::new(repository.clone()))
            .with_builtin_catalog(
                BuiltinCatalog::Loaded(first.clone()),
                Some(seeded_execution_environment()),
            );
        // An instance setting: the operator pins real targets on the builtin job.
        let instance_inventory = InventorySelection::Native {
            sources: vec!["inventory/instance.yml".to_string()],
            ssh_record_id: None,
        };
        service
            .update_job_template(
                template_id,
                UpdateJobTemplateInput {
                    name: None,
                    project_id: None,
                    workdir: None,
                    playbook_paths: None,
                    revision: None,
                    inventory: Some(instance_inventory.clone()),
                    vars_schema: None,
                    options: None,
                    execution_environment_id: None,
                },
                None,
            )
            .await?
            .expect("template exists");
        service
            .update_job_template(
                template_id,
                UpdateJobTemplateInput {
                    revision: Some(RevisionSelection::Fixed {
                        revision_id: first.revision_id,
                    }),
                    ..instance_update()
                },
                None,
            )
            .await?
            .expect("fixed revision accepted");
        let foreign = service
            .update_job_template(
                template_id,
                UpdateJobTemplateInput {
                    revision: Some(RevisionSelection::Fixed {
                        revision_id: Uuid::now_v7(),
                    }),
                    ..instance_update()
                },
                None,
            )
            .await;
        assert!(foreign.is_err());
        // The definition itself is not configurable through the public API.
        let definition_edit = service
            .update_job_template(
                template_id,
                UpdateJobTemplateInput {
                    name: None,
                    project_id: None,
                    workdir: None,
                    playbook_paths: Some(vec!["system/other.yml".to_string()]),
                    revision: None,
                    inventory: None,
                    vars_schema: None,
                    options: None,
                    execution_environment_id: None,
                },
                None,
            )
            .await;
        assert!(definition_edit.is_err(), "builtin definition is read-only");

        let playbook = scratch.join("system/auto-update-inventory.yml");
        let mut content = std::fs::read_to_string(&playbook)?;
        content.push_str("\n# catalog revision 2\n");
        std::fs::write(&playbook, content)?;
        // Force the seed's definition UPDATE as well as the new revision.
        let manifest_path = scratch.join("piping-playbooks.yaml");
        let manifest = std::fs::read_to_string(&manifest_path)?;
        std::fs::write(
            &manifest_path,
            manifest.replace("name: Auto-update inventory", "name: Updated inventory"),
        )?;
        let schema_path = scratch.join("system/auto-update-inventory.vars-schema.json");
        let schema = std::fs::read_to_string(&schema_path)?;
        std::fs::write(
            &schema_path,
            schema.replace("Update OS platform", "Updated OS platform"),
        )?;

        let second =
            load_system_catalog(&scratch, Some(seeded_execution_environment()), &repository).await?;
        assert_ne!(second.revision_id, first.revision_id);
        assert_ne!(second.tree_sha256, first.tree_sha256);
        assert_eq!(fixture.catalog_counts().await?, (1, 2, 2));
        assert_eq!(fixture.current_revision().await?, Some(second.revision_id));
        let fixed = repository
            .get_job_template(template_id)
            .await?
            .expect("template");
        assert_eq!(
            fixed.revision,
            RevisionSelection::Fixed {
                revision_id: first.revision_id
            }
        );
        assert_eq!(fixed.name, "Updated inventory");
        assert_eq!(
            repository
                .get_job_template(job_id(EXPORTER_SLUG))
                .await?
                .expect("other job")
                .revision,
            RevisionSelection::Current
        );
        let fixed_schema = service
            .get_job_template_vars_schema(template_id)
            .await?
            .expect("schema");
        service
            .update_job_template(
                template_id,
                UpdateJobTemplateInput {
                    revision: Some(RevisionSelection::Current),
                    ..instance_update()
                },
                None,
            )
            .await?;
        let following_schema = service
            .get_job_template_vars_schema(template_id)
            .await?
            .expect("schema");
        assert_ne!(fixed_schema.revision, following_schema.revision);
        service
            .update_job_template(
                template_id,
                UpdateJobTemplateInput {
                    revision: Some(RevisionSelection::Fixed {
                        revision_id: first.revision_id,
                    }),
                    ..instance_update()
                },
                None,
            )
            .await?;
        assert_eq!(
            service
                .get_job_template_vars_schema(template_id)
                .await?
                .expect("schema")
                .revision,
            fixed_schema.revision
        );
        // The launched revision keeps its exact bytes and hash.
        let preserved: String =
            sqlx::query_scalar("SELECT tree_sha256 FROM playbook_project_revision WHERE revision_id = $1")
                .bind(first.revision_id)
                .fetch_one(&fixture.pool)
                .await?;
        assert_eq!(preserved, first.tree_sha256);
        // The template identity and the operator's targets survive the re-seed.
        assert_eq!(second.template(INVENTORY_SLUG), first.template(INVENTORY_SLUG));
        assert_eq!(
            fixture.template_inventory(template_id).await?,
            instance_inventory
        );
        // A changed catalog definition may no longer exist in a fixed old tree.
        // Lookup must not substitute the current tree.
        std::fs::copy(&playbook, scratch.join("system/renamed-inventory.yml"))?;
        let manifest = std::fs::read_to_string(&manifest_path)?;
        std::fs::write(
            &manifest_path,
            manifest.replace(
                "entrypoint_file: system/auto-update-inventory.yml",
                "entrypoint_file: system/renamed-inventory.yml",
            ),
        )?;
        let third =
            load_system_catalog(&scratch, Some(seeded_execution_environment()), &repository)
                .await?;
        let third_service = PlaybookService::new(Arc::new(repository.clone()))
            .with_builtin_catalog(
                BuiltinCatalog::Loaded(third),
                Some(seeded_execution_environment()),
            );
        let lookup = third_service.resolve_builtin_job(INVENTORY_SLUG).await?;
        assert!(!lookup.available);
        assert_eq!(
            lookup.unavailable_reason,
            Some(BuiltinUnavailableReason::RevisionUnavailable)
        );
        third_service
            .update_job_template(
                template_id,
                UpdateJobTemplateInput {
                    revision: Some(RevisionSelection::Current),
                    ..instance_update()
                },
                None,
            )
            .await?;
        assert!(
            third_service
                .resolve_builtin_job(INVENTORY_SLUG)
                .await?
                .available
        );
        std::fs::remove_dir_all(&scratch)?;
        Ok(())
    }
    .await;
    teardown(fixture).await?;
    outcome
}

#[tokio::test]
#[ignore = "requires the guarded playbook_catalog_ child database fixture and PostgreSQL"]
async fn catalog_seed_fails_closed_on_an_identity_collision() -> TestResult {
    let fixture = fixture().await?;
    let outcome = async {
        sqlx::query(
            "INSERT INTO playbook_project (id, name, description, source) VALUES ($1, 'user project', NULL, $2)",
        )
        .bind(project_id())
        .bind(serde_json::to_value(ProjectSource::Editable)?)
        .execute(&fixture.pool)
        .await?;
        let error =
            load_system_catalog(&shipped_catalog(), None, &fixture.repository()).await;
        assert!(error.is_err(), "collision must fail closed");
        let source: serde_json::Value =
            sqlx::query_scalar("SELECT source FROM playbook_project WHERE id = $1")
                .bind(project_id())
                .fetch_one(&fixture.pool)
                .await?;
        assert_eq!(
            serde_json::from_value::<ProjectSource>(source)?,
            ProjectSource::Editable
        );
        let name: String = sqlx::query_scalar("SELECT name FROM playbook_project WHERE id = $1")
            .bind(project_id())
            .fetch_one(&fixture.pool)
            .await?;
        assert_eq!(name, "user project");
        assert_eq!(fixture.catalog_counts().await?, (1, 0, 0));
        Ok(())
    }
    .await;
    teardown(fixture).await?;
    outcome
}

#[tokio::test]
#[ignore = "requires the guarded playbook_catalog_ child database fixture and PostgreSQL"]
async fn invalid_catalog_writes_nothing() -> TestResult {
    let fixture = fixture().await?;
    let outcome = async {
        let scratch = std::env::temp_dir().join(format!("piping-catalog-{}", Uuid::now_v7().simple()));
        std::fs::create_dir_all(&scratch)?;
        // A stale launch declaration fails the whole document before any write.
        std::fs::write(
            scratch.join("piping-playbooks.yaml"),
            STALE_LAUNCH_OPTIONS_MANIFEST,
        )?;
        let stale = load_system_catalog(&scratch, None, &fixture.repository()).await;
        assert!(stale.is_err(), "unapproved launch declaration must fail");
        assert_eq!(fixture.catalog_counts().await?, (0, 0, 0));
        // A valid document that references a missing file also writes nothing.
        std::fs::write(
            scratch.join("piping-playbooks.yaml"),
            MISSING_ENTRYPOINT_MANIFEST,
        )?;
        let missing = load_system_catalog(&scratch, None, &fixture.repository()).await;
        assert!(missing.is_err(), "missing entrypoint must fail");
        assert_eq!(fixture.catalog_counts().await?, (0, 0, 0));
        std::fs::remove_dir_all(&scratch)?;
        Ok(())
    }
    .await;
    teardown(fixture).await?;
    outcome
}

#[tokio::test]
#[ignore = "requires the guarded playbook_catalog_ child database fixture and PostgreSQL"]
async fn builtin_project_and_jobs_reject_public_mutation() -> TestResult {
    let fixture = fixture().await?;
    let outcome = async {
        let repository = fixture.repository();
        let loaded = load_system_catalog(
            &shipped_catalog(),
            Some(seeded_execution_environment()),
            &repository,
        )
        .await?;
        let service = PlaybookService::new(Arc::new(repository.clone()));
        let catalog_project = loaded.project_id;
        let template_id = loaded
            .template(INVENTORY_SLUG)
            .and_then(|template| template.template_id)
            .expect("seeded template");
        let revision = loaded.revision_id;

        assert!(
            service
                .update_project(
                    catalog_project,
                    UpdateProjectInput {
                        name: Some("renamed".to_string()),
                        description: None,
                        source: None,
                    },
                    None,
                )
                .await
                .is_err()
        );
        assert!(
            service
                .import_project_revision(
                    catalog_project,
                    Some(revision),
                    Utc::now(),
                    "0".repeat(40),
                    Vec::new(),
                    None,
                )
                .await
                .is_err()
        );
        assert!(
            service
                .change_project_revision(
                    catalog_project,
                    revision,
                    vec![RevisionChange::Put {
                        entry: ProjectEntry {
                            path: "extra.yml".to_string(),
                            content: ProjectEntryContent::File {
                                bytes: b"---\n".to_vec(),
                                executable: false,
                            },
                        },
                    }],
                    None,
                )
                .await
                .is_err()
        );
        assert!(
            service
                .fork_project_revision(catalog_project, revision, "fork".to_string(), None)
                .await
                .is_err()
        );
        assert!(
            service
                .create_job_template(
                    CreateJobTemplateInput {
                        template: JobTemplate {
                            id: Uuid::now_v7(),
                            name: "user job".to_string(),
                            project_id: catalog_project,
                            workdir: ".".to_string(),
                            playbook_paths: vec!["system/auto-update-inventory.yml".to_string()],
                            revision: RevisionSelection::Fixed {
                                revision_id: revision,
                            },
                            inventory: InventorySelection::Native {
                                sources: vec!["inventory/instance.yml".to_string()],
                                ssh_record_id: None,
                            },
                            vars_schema: VarsSchemaSource::None,
                            options: LaunchOptions {
                                argv: Vec::new(),
                                extra_vars: serde_json::json!({}),
                                limit: None,
                                env: Default::default(),
                                secret_refs: Default::default(),
                                timeout_seconds: 90,
                            },
                            execution_environment_id: seeded_execution_environment(),
                        },
                    },
                    None,
                )
                .await
                .is_err()
        );
        assert!(
            service.delete_job_template(template_id).await.is_err(),
            "builtin job definition is read-only"
        );
        assert!(
            service.delete_project(catalog_project).await.is_err(),
            "system project is read-only"
        );
        Ok(())
    }
    .await;
    teardown(fixture).await?;
    outcome
}

#[tokio::test]
#[ignore = "requires the guarded playbook_catalog_ child database fixture and PostgreSQL"]
async fn builtin_lookup_reports_truthful_availability() -> TestResult {
    let fixture = fixture().await?;
    let outcome = async {
        let repository = fixture.repository();
        // Without an explicitly configured environment the project and revision
        // are seeded, but no job template exists.
        let unconfigured = load_system_catalog(&shipped_catalog(), None, &repository).await?;
        assert_eq!(fixture.catalog_counts().await?, (1, 1, 0));
        let service = PlaybookService::new(Arc::new(repository.clone()))
            .with_builtin_catalog(BuiltinCatalog::Loaded(unconfigured), None);
        let lookup = service.resolve_builtin_job(INVENTORY_SLUG).await?;
        assert!(!lookup.available);
        assert_eq!(lookup.template_id, None);
        assert_eq!(
            lookup.unavailable_reason,
            Some(BuiltinUnavailableReason::ExecutionEnvironmentNotConfigured)
        );
        // An unknown slug is not a builtin job at all.
        assert!(service.resolve_builtin_job("no-such-entry").await.is_err());

        let seeded = load_system_catalog(
            &shipped_catalog(),
            Some(seeded_execution_environment()),
            &repository,
        )
        .await?;
        let service = PlaybookService::new(Arc::new(repository.clone()))
            .with_builtin_catalog(
                BuiltinCatalog::Loaded(seeded.clone()),
                Some(seeded_execution_environment()),
            );
        let lookup = service.resolve_builtin_job(INVENTORY_SLUG).await?;
        assert!(!lookup.available);
        assert_eq!(
            lookup.unavailable_reason,
            Some(BuiltinUnavailableReason::TargetsNotConfigured)
        );
        assert_eq!(lookup.template_id, Some(job_id(INVENTORY_SLUG)));

        // With configured targets the same row becomes available.
        service
            .update_job_template(
                job_id(INVENTORY_SLUG),
                UpdateJobTemplateInput {
                    name: None,
                    project_id: None,
                    workdir: None,
                    playbook_paths: None,
                    revision: None,
                    inventory: Some(InventorySelection::Native {
                        sources: vec!["inventory/instance.yml".to_string()],
                        ssh_record_id: None,
                    }),
                    vars_schema: None,
                    options: None,
                    execution_environment_id: None,
                },
                None,
            )
            .await?;
        let lookup = service.resolve_builtin_job(INVENTORY_SLUG).await?;
        assert!(lookup.available);
        assert_eq!(lookup.unavailable_reason, None);
        // The exporter entry is seeded by the same loader from the same manifest.
        assert_eq!(
            seeded.template(EXPORTER_SLUG).and_then(|template| template.template_id),
            Some(job_id(EXPORTER_SLUG))
        );
        Ok(())
    }
    .await;
    teardown(fixture).await?;
    outcome
}

const STALE_LAUNCH_OPTIONS_MANIFEST: &str = r#"
apiVersion: piping.space/playbook-catalog/v1
catalog:
  source_repo: null
  base_path: ansible/playbooks
playbooks:
  - slug: system-auto-update-inventory
    name: Auto-update inventory
    description: null
    category: inventory
    risk: low
    runtime: ansible
    connection_default: ssh
    status: active
    entrypoint_file: system/auto-update-inventory.yml
    language: yaml
    timeout_seconds: 90
    input_schema: {}
    output_schema: {type: object}
    token_policy: {ttl_seconds: 60, scopes: [artifact:create]}
    launch_options:
      forks: 64
      strategy: free
      shell_executable: /bin/sh
      ssh_common_args: -o ProxyCommand=x
    vars_schema_file: null
    system: true
    inventory_rest_enabled: true
"#;

const MISSING_ENTRYPOINT_MANIFEST: &str = r#"
apiVersion: piping.space/playbook-catalog/v1
catalog:
  source_repo: null
  base_path: ansible/playbooks
playbooks:
  - slug: system-auto-update-inventory
    name: Auto-update inventory
    description: null
    category: inventory
    risk: low
    runtime: ansible
    connection_default: ssh
    status: active
    entrypoint_file: system/absent.yml
    language: yaml
    timeout_seconds: 90
    input_schema: {}
    output_schema: {type: object}
    token_policy: {ttl_seconds: 60, scopes: [artifact:create]}
    vars_schema_file: null
    system: true
    inventory_rest_enabled: true
"#;

/// Every instance setting left at its default, so a test can override one field.
fn instance_update() -> UpdateJobTemplateInput {
    UpdateJobTemplateInput {
        name: None,
        project_id: None,
        workdir: None,
        playbook_paths: None,
        revision: None,
        inventory: None,
        vars_schema: None,
        options: None,
        execution_environment_id: None,
    }
}

async fn create_second_environment(pool: &PgPool) -> TestResult<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO playbook_execution_environment (id, name, description, image, pull_policy, runner_group, enabled, env, settings, metadata) VALUES ($1,'Reviewer runner',NULL,'registry.example/piping-ansible-runner@sha256:0000000000000000000000000000000000000000000000000000000000000000','if_not_present','default',true,'{}','{}','{}')",
    )
    .bind(id)
    .execute(pool)
    .await?;
    Ok(id)
}

#[tokio::test]
#[ignore = "requires the guarded playbook_catalog_ child database fixture and PostgreSQL"]
async fn operator_execution_environment_survives_reseed() -> TestResult {
    let fixture = fixture().await?;
    let outcome = async {
        let repository = fixture.repository();
        let seeded = load_system_catalog(
            &shipped_catalog(),
            Some(seeded_execution_environment()),
            &repository,
        )
        .await?;
        let template_id = seeded
            .template(INVENTORY_SLUG)
            .and_then(|template| template.template_id)
            .expect("seeded template");
        // The execution environment is an instance setting: an operator retargets
        // the builtin job to another enabled environment.
        let second = create_second_environment(&fixture.pool).await?;
        let service = PlaybookService::new(Arc::new(repository.clone()));
        service
            .update_job_template(
                template_id,
                UpdateJobTemplateInput {
                    execution_environment_id: Some(second),
                    ..instance_update()
                },
                None,
            )
            .await?
            .expect("template exists");
        // The next start re-reads the same configured default and must not undo it.
        load_system_catalog(
            &shipped_catalog(),
            Some(seeded_execution_environment()),
            &repository,
        )
        .await?;
        let persisted: Uuid = sqlx::query_scalar(
            "SELECT execution_environment_id FROM playbook_job_template WHERE id = $1",
        )
        .bind(template_id)
        .fetch_one(&fixture.pool)
        .await?;
        assert_eq!(persisted, second, "operator environment must survive a re-seed");
        assert_eq!(fixture.catalog_counts().await?, (1, 1, 2));
        Ok(())
    }
    .await;
    teardown(fixture).await?;
    outcome
}

#[tokio::test]
#[ignore = "requires the guarded playbook_catalog_ child database fixture and PostgreSQL"]
async fn builtin_job_cannot_move_into_an_editable_project() -> TestResult {
    let fixture = fixture().await?;
    let outcome = async {
        let repository = fixture.repository();
        let seeded = load_system_catalog(
            &shipped_catalog(),
            Some(seeded_execution_environment()),
            &repository,
        )
        .await?;
        let template_id = seeded
            .template(INVENTORY_SLUG)
            .and_then(|template| template.template_id)
            .expect("seeded template");
        let service = PlaybookService::new(Arc::new(repository.clone()));
        let editable = service
            .create_project(
                CreateProjectInput {
                    name: "operator project".to_string(),
                    description: None,
                    source: ProjectSource::Editable,
                },
                None,
            )
            .await?;
        let before = repository
            .get_job_template(template_id)
            .await?
            .expect("seeded template row");
        // Moving the builtin job into a readable, editable project must not bypass
        // the definition guard even though that target project is not a System.
        let attempt = service
            .update_job_template(
                template_id,
                UpdateJobTemplateInput {
                    project_id: Some(editable.id),
                    name: Some("moved".to_string()),
                    playbook_paths: Some(vec!["system/other.yml".to_string()]),
                    options: Some(LaunchOptions {
                        argv: vec!["--diff".to_string()],
                        ..before.options.clone()
                    }),
                    ..instance_update()
                },
                None,
            )
            .await;
        assert!(attempt.is_err(), "builtin job must not leave its project");
        let after = repository
            .get_job_template(template_id)
            .await?
            .expect("template row survives");
        assert_eq!(after.project_id, before.project_id);
        assert_eq!(after.name, before.name);
        assert_eq!(after.playbook_paths, before.playbook_paths);
        assert_eq!(after.options.argv, before.options.argv);
        // A following start still finds its own project and does not collide.
        let reseeded = load_system_catalog(
            &shipped_catalog(),
            Some(seeded_execution_environment()),
            &repository,
        )
        .await?;
        assert_eq!(
            reseeded
                .template(INVENTORY_SLUG)
                .and_then(|template| template.template_id),
            Some(template_id)
        );
        Ok(())
    }
    .await;
    teardown(fixture).await?;
    outcome
}

#[tokio::test]
#[ignore = "requires the guarded playbook_catalog_ child database fixture and PostgreSQL"]
async fn instance_options_survive_reseed_and_parameter_resolution() -> TestResult {
    let fixture = fixture().await?;
    let outcome = async {
        let repository = fixture.repository();
        let seeded = load_system_catalog(
            &shipped_catalog(),
            Some(seeded_execution_environment()),
            &repository,
        )
        .await?;
        let template_id = seeded
            .template(INVENTORY_SLUG)
            .and_then(|template| template.template_id)
            .expect("seeded template");
        let secret_id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO secrets_variable (id, key, kind, is_masked, is_protected) VALUES ($1, 'catalog-review-secret', 'named', true, false)",
        )
        .bind(secret_id)
        .execute(&fixture.pool)
        .await?;
        let service = PlaybookService::new(Arc::new(repository.clone()));
        let instance = LaunchOptions {
            argv: Vec::new(),
            extra_vars: serde_json::json!({ "piping_inventory_gather_timeout": 12 }),
            limit: Some("web".to_string()),
            env: [("HTTP_PROXY".to_string(), "http://proxy.internal".to_string())]
                .into_iter()
                .collect(),
            secret_refs: [("DEPLOY_TOKEN".to_string(), secret_id)].into_iter().collect(),
            timeout_seconds: 321,
        };
        service
            .update_job_template(
                template_id,
                UpdateJobTemplateInput {
                    options: Some(instance.clone()),
                    ..instance_update()
                },
                None,
            )
            .await?
            .expect("template exists");
        // Restart: the catalog definition is unchanged, instance values are not.
        load_system_catalog(
            &shipped_catalog(),
            Some(seeded_execution_environment()),
            &repository,
        )
        .await?;
        let stored = fixture.template_options(template_id).await?;
        assert_eq!(stored.extra_vars, instance.extra_vars);
        assert_eq!(stored.env, instance.env);
        assert_eq!(stored.secret_refs, instance.secret_refs);
        assert_eq!(stored.limit, instance.limit);
        assert_eq!(stored.timeout_seconds, 321);
        assert!(stored.argv.is_empty(), "catalog arguments stay authoritative");
        // Launch parameter resolution reads the preserved overrides, and the
        // secret identifier never appears among the variables.
        let resolved = service
            .get_job_template_vars_schema(template_id)
            .await?
            .expect("effective schema");
        let rendered = serde_json::to_string(&resolved)?;
        assert!(rendered.contains("piping_inventory_gather_timeout"));
        assert!(!rendered.contains(&secret_id.to_string()));
        assert!(stored.extra_vars.get("DEPLOY_TOKEN").is_none());
        Ok(())
    }
    .await;
    teardown(fixture).await?;
    outcome
}

#[tokio::test]
#[ignore = "requires the guarded playbook_catalog_ child database fixture and PostgreSQL"]
async fn concurrent_first_start_seeds_once() -> TestResult {
    let fixture = fixture().await?;
    let outcome = async {
        let repository = fixture.repository();
        let environment = Some(seeded_execution_environment());
        let root = shipped_catalog();
        let (left, right) = tokio::join!(
            load_system_catalog(&root, environment, &repository),
            load_system_catalog(&root, environment, &repository)
        );
        let left = left?;
        let right = right?;
        assert_eq!(left, right);
        assert_eq!(fixture.catalog_counts().await?, (1, 1, 2));
        Ok(())
    }
    .await;
    teardown(fixture).await?;
    outcome
}

// ── Loader contract without a database ─────────────────────────────────────
// These live in the integration target so the station's build admission does not
// have to compile the whole crate's test harness; they call the public loader.

fn scratch_catalog(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!(
        "piping-catalog-loader-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("scratch directory");
    directory
}

#[test]
fn oversized_catalog_file_is_rejected_before_reading() {
    let directory = scratch_catalog("size");
    std::fs::write(directory.join("piping-playbooks.yaml"), MINIMAL_CATALOG_MANIFEST)
        .expect("manifest");
    std::fs::write(directory.join("playbook.yml"), "---\n").expect("playbook");
    let oversized = usize::try_from(
        piping_backend::config::constants::PLAYBOOK_CATALOG_MAX_FILE_BYTES,
    )
    .expect("limit fits") + 1;
    std::fs::write(directory.join("big.log"), vec![b'x'; oversized]).expect("oversized file");
    let error = piping_backend::contexts::playbooks::application::catalog_loader::read_catalog(
        &directory,
    )
    .expect_err("oversized file must fail");
    assert!(error.to_string().contains("exceeds"), "{error}");
    std::fs::remove_dir_all(&directory).expect("cleanup");
}

#[test]
fn symlinked_catalog_file_is_rejected() {
    let directory = scratch_catalog("symlink");
    std::fs::write(directory.join("piping-playbooks.yaml"), MINIMAL_CATALOG_MANIFEST)
        .expect("manifest");
    let real = directory.join("real.yml");
    std::fs::write(&real, "---\n").expect("real file");
    std::os::unix::fs::symlink(&real, directory.join("link.yml")).expect("symlink");
    let error = piping_backend::contexts::playbooks::application::catalog_loader::read_catalog(
        &directory,
    )
    .expect_err("symlink must fail");
    assert!(error.to_string().contains("symlink"), "{error}");
    std::fs::remove_dir_all(&directory).expect("cleanup");
}

#[test]
fn unresolvable_parameter_schema_fails_the_catalog() {
    let directory = scratch_catalog("schema");
    std::fs::write(directory.join("piping-playbooks.yaml"), MINIMAL_CATALOG_MANIFEST)
        .expect("manifest");
    std::fs::write(directory.join("playbook.yml"), "---\n").expect("playbook");
    // Valid JSON that the canonical E1 parameter validator rejects.
    std::fs::write(directory.join("vars.json"), r#"{"type":"string"}"#).expect("schema");
    let error = piping_backend::contexts::playbooks::application::catalog_loader::read_catalog(
        &directory,
    )
    .expect_err("invalid parameter schema must fail");
    assert!(error.to_string().contains("vars_schema"), "{error}");
    std::fs::remove_dir_all(&directory).expect("cleanup");
}

const MINIMAL_CATALOG_MANIFEST: &str = r#"
apiVersion: piping.space/playbook-catalog/v1
catalog:
  source_repo: null
  base_path: ansible/playbooks
playbooks:
  - slug: system-auto-update-inventory
    name: Auto-update inventory
    description: null
    category: inventory
    risk: low
    runtime: ansible
    connection_default: ssh
    status: active
    entrypoint_file: playbook.yml
    language: yaml
    timeout_seconds: 90
    input_schema: {}
    output_schema: {type: object}
    token_policy: {ttl_seconds: 60, scopes: [artifact:create]}
    vars_schema_file: vars.json
    system: true
    inventory_rest_enabled: true
"#;

#[tokio::test]
#[ignore = "requires the guarded playbook_catalog_ child database fixture and PostgreSQL"]
async fn user_job_cannot_move_into_the_catalog_project() -> TestResult {
    let fixture = fixture().await?;
    let outcome = async {
        let repository = fixture.repository();
        let seeded = load_system_catalog(
            &shipped_catalog(),
            Some(seeded_execution_environment()),
            &repository,
        )
        .await?;
        let service = PlaybookService::new(Arc::new(repository.clone()));
        // An ordinary caller who may read the catalog and update their own job.
        let user_project = service
            .create_project(
                CreateProjectInput {
                    name: "own project".to_string(),
                    description: None,
                    source: ProjectSource::Editable,
                },
                None,
            )
            .await?;
        let user_job = service
            .create_job_template(
                CreateJobTemplateInput {
                    template: JobTemplate {
                        id: Uuid::now_v7(),
                        name: "own job".to_string(),
                        project_id: user_project.id,
                        workdir: ".".to_string(),
                        playbook_paths: vec!["site.yml".to_string()],
                        revision: RevisionSelection::Fixed {
                            revision_id: user_project
                                .current_revision_id
                                .expect("editable project revision"),
                        },
                        inventory: InventorySelection::Native {
                            sources: vec!["inventory/hosts.yml".to_string()],
                            ssh_record_id: None,
                        },
                        vars_schema: VarsSchemaSource::None,
                        options: LaunchOptions {
                            argv: Vec::new(),
                            extra_vars: serde_json::json!({}),
                            limit: None,
                            env: Default::default(),
                            secret_refs: Default::default(),
                            timeout_seconds: 90,
                        },
                        execution_environment_id: seeded_execution_environment(),
                    },
                },
                None,
            )
            .await?;
        let before = repository
            .get_job_template(user_job.id)
            .await?
            .expect("user job row");
        // The incoming direction: the persisted project is an ordinary one, the
        // proposed project is the catalog. A read grant on the catalog plus an
        // update grant on this job must not move it into the System project.
        let attempt = service
            .update_job_template(
                user_job.id,
                UpdateJobTemplateInput {
                    project_id: Some(seeded.project_id),
                    playbook_paths: Some(vec!["system/other.yml".to_string()]),
                    options: Some(LaunchOptions {
                        argv: vec!["--diff".to_string()],
                        ..before.options.clone()
                    }),
                    vars_schema: Some(VarsSchemaSource::None),
                    ..instance_update()
                },
                None,
            )
            .await;
        assert!(attempt.is_err(), "a user job must not enter the catalog");
        let after = repository
            .get_job_template(user_job.id)
            .await?
            .expect("user job row survives");
        assert_eq!(after.project_id, before.project_id);
        assert_eq!(after.playbook_paths, before.playbook_paths);
        assert_eq!(after.options.argv, before.options.argv);
        // The catalog itself still seeds and resolves after the rejected move.
        let reseeded = load_system_catalog(
            &shipped_catalog(),
            Some(seeded_execution_environment()),
            &repository,
        )
        .await?;
        assert_eq!(
            reseeded
                .template(INVENTORY_SLUG)
                .and_then(|template| template.template_id),
            seeded
                .template(INVENTORY_SLUG)
                .and_then(|template| template.template_id)
        );
        Ok(())
    }
    .await;
    teardown(fixture).await?;
    outcome
}

#[tokio::test]
#[ignore = "requires the guarded playbook_catalog_ child database fixture and PostgreSQL"]
async fn existing_job_stays_visible_when_the_default_environment_is_removed() -> TestResult {
    let fixture = fixture().await?;
    let outcome = async {
        let repository = fixture.repository();
        let seeded = load_system_catalog(
            &shipped_catalog(),
            Some(seeded_execution_environment()),
            &repository,
        )
        .await?;
        let template_id = seeded
            .template(INVENTORY_SLUG)
            .and_then(|template| template.template_id)
            .expect("seeded template");
        // The instance owns the environment: the operator retargets the job, and
        // the configured default is then removed from the instance config.
        let second = create_second_environment(&fixture.pool).await?;
        let service = PlaybookService::new(Arc::new(repository.clone()));
        service
            .update_job_template(
                template_id,
                UpdateJobTemplateInput {
                    execution_environment_id: Some(second),
                    ..instance_update()
                },
                None,
            )
            .await?
            .expect("template exists");
        let without_default = load_system_catalog(&shipped_catalog(), None, &repository).await?;
        // No default is configured, so no new row is seeded, but the existing row
        // is still refreshed and stays resolvable.
        let service = PlaybookService::new(Arc::new(repository.clone()))
            .with_builtin_catalog(BuiltinCatalog::Loaded(without_default), None);
        let lookup = service.resolve_builtin_job(INVENTORY_SLUG).await?;
        assert_eq!(lookup.template_id, Some(template_id));
        assert!(!lookup.available);
        assert_eq!(
            lookup.unavailable_reason,
            Some(BuiltinUnavailableReason::TargetsNotConfigured)
        );
        let persisted: Uuid = sqlx::query_scalar(
            "SELECT execution_environment_id FROM playbook_job_template WHERE id = $1",
        )
        .bind(template_id)
        .fetch_one(&fixture.pool)
        .await?;
        assert_eq!(persisted, second, "removing the default must not reset the operator environment");
        // Disabling that persisted environment is reported truthfully.
        sqlx::query("UPDATE playbook_execution_environment SET enabled = false WHERE id = $1")
            .bind(second)
            .execute(&fixture.pool)
            .await?;
        let lookup = service.resolve_builtin_job(INVENTORY_SLUG).await?;
        assert_eq!(
            lookup.unavailable_reason,
            Some(BuiltinUnavailableReason::ExecutionEnvironmentUnavailable)
        );
        // A missing row with no configured default stays "not configured".
        let fresh = extra_fixture().await?;
        let fresh_repository = fresh.repository();
        let loaded = load_system_catalog(&shipped_catalog(), None, &fresh_repository).await?;
        let service = PlaybookService::new(Arc::new(fresh_repository))
            .with_builtin_catalog(BuiltinCatalog::Loaded(loaded), None);
        let lookup = service.resolve_builtin_job(INVENTORY_SLUG).await?;
        assert_eq!(lookup.template_id, None);
        assert_eq!(
            lookup.unavailable_reason,
            Some(BuiltinUnavailableReason::ExecutionEnvironmentNotConfigured)
        );
        teardown(fresh).await?;
        Ok(())
    }
    .await;
    teardown(fixture).await?;
    outcome
}

#[tokio::test]
#[ignore = "requires the guarded playbook_catalog_ child database fixture and PostgreSQL"]
async fn catalog_timeout_is_only_the_default_of_a_new_row() -> TestResult {
    let fixture = fixture().await?;
    let outcome = async {
        let repository = fixture.repository();
        let seeded = load_system_catalog(
            &shipped_catalog(),
            Some(seeded_execution_environment()),
            &repository,
        )
        .await?;
        let template_id = seeded
            .template(INVENTORY_SLUG)
            .and_then(|template| template.template_id)
            .expect("seeded template");
        let service = PlaybookService::new(Arc::new(repository.clone()));
        service
            .update_job_template(
                template_id,
                UpdateJobTemplateInput {
                    options: Some(LaunchOptions {
                        timeout_seconds: 321,
                        ..fixture.template_options(template_id).await?
                    }),
                    ..instance_update()
                },
                None,
            )
            .await?
            .expect("template exists");
        // The shipped timeout is a new-row default: a changed catalog timeout must
        // not overwrite the operator's value on an existing row.
        let slower = scratch_catalog("timeout");
        copy_catalog(&shipped_catalog(), &slower)?;
        let manifest_path = slower.join("piping-playbooks.yaml");
        let manifest = std::fs::read_to_string(&manifest_path)?;
        let changed = manifest.replace(
            "  - slug: system-auto-update-inventory\n",
            "  - slug: system-auto-update-inventory\n    # temporary copy with a longer new-row timeout\n",
        );
        // Only the system inventory entry timeout changes; the exporter keeps its own.
        let changed = changed.replace("    timeout_seconds: 90\n", "    timeout_seconds: 95\n");
        std::fs::write(&manifest_path, changed)?;
        load_system_catalog(&slower, Some(seeded_execution_environment()), &repository).await?;
        assert_eq!(fixture.template_options(template_id).await?.timeout_seconds, 321);
        // A brand-new row takes the catalog default.
        let fresh = extra_fixture().await?;
        let fresh_repository = fresh.repository();
        let fresh_seeded = load_system_catalog(
            &slower,
            Some(seeded_execution_environment()),
            &fresh_repository,
        )
        .await?;
        let fresh_template = fresh_seeded
            .template(INVENTORY_SLUG)
            .and_then(|template| template.template_id)
            .expect("new template");
        assert_eq!(
            fresh.template_options(fresh_template).await?.timeout_seconds,
            95
        );
        std::fs::remove_dir_all(&slower)?;
        teardown(fresh).await?;
        Ok(())
    }
    .await;
    teardown(fixture).await?;
    outcome
}
