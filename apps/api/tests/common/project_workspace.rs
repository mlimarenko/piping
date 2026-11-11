//! Persisted project workspace used by automation product tests.

use piping_backend::contexts::playbooks::domain::{
    InventorySelection, JobTemplate, LaunchOptions, LaunchSpec, PlaybookExecutionEnvironment,
    Project, ProjectEntry, ProjectEntryContent, ProjectRevision, ProjectSource, ResolvedTarget,
    RevisionChange, RevisionSelection,
};
use piping_backend::contexts::playbooks::infrastructure::persistence::{
    CreateExecutionEnvironmentRecord, CreateJobTemplateRecord, CreateProjectRecord,
    PlaybookRepository, SqlxPlaybookRepository,
};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

type FixtureError = Box<dyn std::error::Error + Send + Sync>;

pub(crate) struct WorkspaceFixture {
    pub(crate) project: Project,
    pub(crate) revision: ProjectRevision,
    pub(crate) execution_environment: PlaybookExecutionEnvironment,
    pub(crate) template: JobTemplate,
}

impl WorkspaceFixture {
    pub(crate) fn launch_spec(&self, targets: Vec<ResolvedTarget>) -> LaunchSpec {
        LaunchSpec {
            template_id: self.template.id,
            project_id: self.project.id,
            revision_id: self.revision.revision_id,
            tree_sha256: self.revision.tree_sha256.clone(),
            execution_environment_id: self.execution_environment.id,
            image: "example.test/ansible".into(),
            image_digest: format!("sha256:{}", "a".repeat(64)),
            workdir: self.template.workdir.clone(),
            playbook_paths: self.template.playbook_paths.clone(),
            inventory: self.template.inventory.clone(),
            vars_schema: None,
            options: self.template.options.clone(),
            targets,
        }
    }

    /// Callers remove their run groups, schedules and assignments first.
    pub(crate) async fn cleanup(&self, pool: &PgPool) -> Result<(), FixtureError> {
        let repo = SqlxPlaybookRepository::new(pool.clone());
        repo.delete_job_template(self.template.id).await?;
        repo.delete_project(self.project.id).await?;
        repo.delete_execution_environment(self.execution_environment.id)
            .await?;
        Ok(())
    }
}

pub(crate) async fn seed_workspace(
    pool: &PgPool,
    inventory: InventorySelection,
) -> Result<WorkspaceFixture, FixtureError> {
    let repo = SqlxPlaybookRepository::new(pool.clone());
    let suffix = Uuid::now_v7();
    let project = repo
        .create_project(CreateProjectRecord {
            name: format!("automation-workspace-{suffix}"),
            description: None,
            source: ProjectSource::Editable,
            user_id: None,
        })
        .await?;
    let revision = repo
        .change_project_revision(
            project.id,
            project.current_revision_id.ok_or("initial revision missing")?,
            vec![
                RevisionChange::Put {
                    entry: ProjectEntry {
                        path: "site.yml".into(),
                        content: ProjectEntryContent::File {
                            bytes: b"- hosts: all\n  tasks:\n    - ansible.builtin.debug:\n        msg: workspace\n"
                                .to_vec(),
                            executable: false,
                        },
                    },
                },
                RevisionChange::Put {
                    entry: ProjectEntry {
                        path: "inventory.yml".into(),
                        content: ProjectEntryContent::File {
                            bytes: b"all:\n  hosts:\n    localhost:\n      ansible_connection: local\n"
                                .to_vec(),
                            executable: false,
                        },
                    },
                },
            ],
            None,
        )
        .await?;
    let project = repo
        .get_project(project.id)
        .await?
        .ok_or("project missing")?;
    let execution_environment = repo
        .create_execution_environment(CreateExecutionEnvironmentRecord {
            name: format!("automation-environment-{suffix}"),
            description: None,
            image: format!("example.test/ansible@sha256:{}", "a".repeat(64)),
            pull_policy: "never".into(),
            runner_group: "default".into(),
            enabled: true,
            env: json!({}),
            settings: json!({}),
            metadata: json!({}),
            user_id: None,
        })
        .await?;
    let template = repo
        .create_job_template(CreateJobTemplateRecord {
            template: JobTemplate {
                id: Uuid::now_v7(),
                name: format!("automation-template-{suffix}"),
                project_id: project.id,
                workdir: ".".into(),
                playbook_paths: vec!["site.yml".into()],
                revision: RevisionSelection::Fixed {
                    revision_id: revision.revision_id,
                },
                inventory,
                vars_schema: Default::default(),
                options: LaunchOptions {
                    argv: vec![],
                    extra_vars: json!({}),
                    limit: None,
                    env: Default::default(),
                    secret_refs: Default::default(),
                    timeout_seconds: 300,
                },
                execution_environment_id: execution_environment.id,
            },
            user_id: None,
        })
        .await?;
    Ok(WorkspaceFixture {
        project,
        revision,
        execution_environment,
        template,
    })
}
