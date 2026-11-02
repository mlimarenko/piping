//! Periodic git refresh of the official playbook catalog overlay.
//!
//! Best-effort by design: the baked snapshot synced at startup is always
//! sufficient. Air-gapped installs disable the refresh outright
//! (`refresh_enabled = false`); online installs treat any fetch/validation
//! failure as a warning and keep serving the current catalog. A refresh
//! failure must never affect product availability.

use std::path::{Path, PathBuf};

use sqlx::PgPool;

use crate::config::OfficialCatalogConfig;
use crate::contexts::playbooks::infrastructure::project_bundle::checkout_git_source;
use crate::errors::AppError;

use super::loader::{SyncedOfficialPlaybook, load_official_catalog, sync_official_catalog};

/// Fetches the configured catalog ref, stages it into the overlay `work_dir`,
/// and syncs the result into the database.
///
/// The fetched tree is fully validated (manifest shape, content files,
/// symlink guard) BEFORE it replaces the overlay, so a broken or malicious
/// fetch leaves the overlay already in place (or the baked snapshot) untouched.
pub async fn refresh_official_catalog(
    db_pool: &PgPool,
    config: &OfficialCatalogConfig,
) -> Result<Vec<SyncedOfficialPlaybook>, AppError> {
    let work_dir = config.work_dir.trim();
    if work_dir.is_empty() {
        return Err(AppError::Config(
            "official catalog refresh requires a non-empty work_dir".to_string(),
        ));
    }
    let repo_url = config.repo_url.trim();
    if repo_url.is_empty() {
        return Err(AppError::Config(
            "official catalog refresh requires a non-empty repo_url".to_string(),
        ));
    }

    // Anonymous checkout: the official catalog repository is public. The
    // shared primitive still applies the non-interactive git env + timeout.
    let checkout = checkout_git_source(repo_url, None, Some(config.repo_ref.trim()), None).await?;
    let source = checkout.path().join("repo").join(config.sparse_path.trim());

    // Validate the fetched catalog before it can win the overlay slot.
    load_official_catalog(&source)?;

    let work_dir = PathBuf::from(work_dir);
    let staged_source = source.clone();
    let staged_work_dir = work_dir.clone();
    tokio::task::spawn_blocking(move || stage_overlay(&staged_source, &staged_work_dir))
        .await
        .map_err(|error| {
            AppError::External(format!(
                "official catalog overlay staging task failed: {error}"
            ))
        })??;

    let catalog = load_official_catalog(&work_dir)?;
    sync_official_catalog(db_pool, &catalog).await
}

/// Replaces `work_dir` with a copy of `source` via a sibling staging
/// directory and two renames, so a crash mid-swap can at worst leave a
/// missing/partial overlay — which `load_catalog` already
/// degrades past to the baked snapshot.
fn stage_overlay(source: &Path, work_dir: &Path) -> Result<(), AppError> {
    let staging = sibling_path(work_dir, ".staging");
    let previous = sibling_path(work_dir, ".previous");
    if staging.exists() {
        std::fs::remove_dir_all(&staging).map_err(|error| {
            AppError::External(format!("failed to clear catalog staging dir: {error}"))
        })?;
    }
    if previous.exists() {
        std::fs::remove_dir_all(&previous).map_err(|error| {
            AppError::External(format!("failed to clear stale catalog backup dir: {error}"))
        })?;
    }
    if let Some(parent) = work_dir.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|error| {
            AppError::External(format!("failed to create catalog overlay parent: {error}"))
        })?;
    }
    copy_dir_skipping_symlinks(source, &staging)?;
    if work_dir.exists() {
        std::fs::rename(work_dir, &previous).map_err(|error| {
            AppError::External(format!("failed to back up catalog overlay: {error}"))
        })?;
    }
    let swapped = std::fs::rename(&staging, work_dir);
    if let Err(error) = swapped {
        // Best effort restore of the previous overlay before surfacing.
        if previous.exists() {
            let _ = std::fs::rename(&previous, work_dir);
        }
        return Err(AppError::External(format!(
            "failed to activate catalog overlay: {error}"
        )));
    }
    if previous.exists() {
        std::fs::remove_dir_all(&previous).map_err(|error| {
            AppError::External(format!("failed to remove old catalog overlay: {error}"))
        })?;
    }
    Ok(())
}

fn sibling_path(dir: &Path, suffix: &str) -> PathBuf {
    let mut name = dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "official-catalog".to_string());
    name.push_str(suffix);
    dir.with_file_name(name)
}

/// Recursive copy that skips symlinks: the loader rejects them anyway, and a
/// hostile catalog commit must not be able to plant one in the overlay.
fn copy_dir_skipping_symlinks(source: &Path, destination: &Path) -> Result<(), AppError> {
    std::fs::create_dir_all(destination).map_err(|error| {
        AppError::External(format!("failed to create catalog overlay dir: {error}"))
    })?;
    let entries = std::fs::read_dir(source).map_err(|error| {
        AppError::External(format!("failed to read catalog checkout dir: {error}"))
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            AppError::External(format!("failed to read catalog checkout entry: {error}"))
        })?;
        let file_type = entry.file_type().map_err(|error| {
            AppError::External(format!("failed to stat catalog checkout entry: {error}"))
        })?;
        let target = destination.join(entry.file_name());
        if file_type.is_symlink() {
            continue;
        } else if file_type.is_dir() {
            copy_dir_skipping_symlinks(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target).map_err(|error| {
                AppError::External(format!("failed to copy catalog file: {error}"))
            })?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_dir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("catalog-refresh-{tag}-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn stage_overlay_swaps_existing_dir_and_skips_symlinks() {
        let scratch = scratch_dir("stage");
        let source = scratch.join("source");
        std::fs::create_dir_all(source.join("system")).expect("source dir");
        std::fs::write(source.join("piping-playbooks.yaml"), "manifest").expect("manifest");
        std::fs::write(source.join("system/entry.yml"), "content").expect("content");
        std::fs::write(scratch.join("outside.txt"), "secret").expect("outside");
        std::os::unix::fs::symlink(scratch.join("outside.txt"), source.join("system/link.yml"))
            .expect("symlink");

        let work_dir = scratch.join("overlay/catalog");
        std::fs::create_dir_all(&work_dir).expect("existing overlay");
        std::fs::write(work_dir.join("stale.yaml"), "old").expect("stale file");

        stage_overlay(&source, &work_dir).expect("stage overlay");

        assert!(work_dir.join("piping-playbooks.yaml").is_file());
        assert!(work_dir.join("system/entry.yml").is_file());
        assert!(
            !work_dir.join("system/link.yml").exists(),
            "symlinks must not be copied into the overlay"
        );
        assert!(
            !work_dir.join("stale.yaml").exists(),
            "the previous overlay content must be replaced, not merged"
        );
        assert!(
            !sibling_path(&work_dir, ".staging").exists()
                && !sibling_path(&work_dir, ".previous").exists(),
            "staging/backup dirs must not linger"
        );

        std::fs::remove_dir_all(&scratch).ok();
    }

    #[tokio::test]
    async fn refresh_fails_cleanly_on_unreachable_repo() {
        // connect_lazy never dials; the refresh must fail at the git fetch,
        // before any database use.
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://invalid:invalid@127.0.0.1:1/invalid")
            .expect("lazy pool");
        let scratch = scratch_dir("unreachable");
        let config = OfficialCatalogConfig {
            work_dir: scratch.join("overlay").to_string_lossy().to_string(),
            repo_url: scratch.join("no-such-repo").to_string_lossy().to_string(),
            ..OfficialCatalogConfig::default()
        };

        let error = refresh_official_catalog(&pool, &config)
            .await
            .expect_err("unreachable repo must fail the refresh");
        assert!(
            !error.to_string().is_empty(),
            "refresh error must carry a message"
        );

        std::fs::remove_dir_all(&scratch).ok();
    }

    #[tokio::test]
    async fn refresh_requires_work_dir_and_repo_url() {
        let disabled = OfficialCatalogConfig {
            work_dir: "  ".to_string(),
            ..OfficialCatalogConfig::default()
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://invalid:invalid@127.0.0.1:1/invalid")
            .expect("lazy pool");
        let error = refresh_official_catalog(&pool, &disabled)
            .await
            .expect_err("empty work_dir must be rejected");
        assert!(error.to_string().contains("work_dir"));
    }
}
