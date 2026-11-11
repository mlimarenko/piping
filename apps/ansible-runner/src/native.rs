//! Internal building blocks for unattended native Ansible jobs.
use std::collections::HashSet;
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt, symlink};
use std::path::{Component, Path, PathBuf};

use piping_contracts::playbook_runner::{
    SnapshotContent, SnapshotEntry, validate_snapshot_entries,
};

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

pub(crate) fn project_path(path: &str) -> io::Result<PathBuf> {
    if path.is_empty() || path.contains(['\0', '\\']) {
        return Err(invalid("invalid project path"));
    }
    let path = Path::new(path);
    if !path
        .components()
        .all(|part| matches!(part, Component::Normal(_)))
    {
        return Err(invalid(
            "project path must be relative and contain no traversal",
        ));
    }
    Ok(path.to_path_buf())
}

#[derive(Debug)]
pub(crate) enum EntryKind {
    File { bytes: Vec<u8>, mode: u32 },
    Symlink { target: String },
}

#[derive(Debug)]
pub(crate) struct Entry {
    pub(crate) path: String,
    pub(crate) kind: EntryKind,
}

/// Creates a fresh private project directory. The parent must be job-owned and
/// inaccessible to untrusted concurrent writers; discard the job directory on error.
pub(crate) fn materialize(root: &Path, entries: &[Entry]) -> io::Result<()> {
    if entries.len() > 2048 {
        return Err(invalid("too many project entries"));
    }
    let snapshot: Vec<_> = entries
        .iter()
        .map(|entry| SnapshotEntry {
            path: entry.path.clone(),
            content: match &entry.kind {
                EntryKind::File { bytes, mode } => SnapshotContent::File {
                    bytes: bytes.clone(),
                    executable: mode & 0o100 != 0,
                },
                EntryKind::Symlink { target } => SnapshotContent::Symlink {
                    target: target.clone(),
                },
            },
        })
        .collect();
    validate_snapshot_entries(&snapshot)
        .map_err(|message| io::Error::new(io::ErrorKind::InvalidInput, message))?;
    let mut total = 0usize;
    for entry in entries {
        match &entry.kind {
            EntryKind::File { bytes, mode } => {
                if bytes.len() > 2 * 1024 * 1024 || mode & !0o777 != 0 {
                    return Err(invalid("invalid file size or mode"));
                }
                total = total
                    .checked_add(bytes.len())
                    .ok_or_else(|| invalid("project too large"))?;
                if total > 64 * 1024 * 1024 {
                    return Err(invalid("project too large"));
                }
            }
            EntryKind::Symlink { .. } => {}
        }
    }
    fs::create_dir(root)?;
    fs::set_permissions(root, fs::Permissions::from_mode(0o700))?;
    for entry in entries {
        let path = root.join(project_path(&entry.path)?);
        fs::create_dir_all(path.parent().ok_or_else(|| invalid("missing parent"))?)?;
        match &entry.kind {
            EntryKind::File { bytes, mode } => {
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&path)?;
                file.write_all(bytes)?;
                file.set_permissions(fs::Permissions::from_mode(*mode))?;
            }
            EntryKind::Symlink { target } => symlink(target, path)?,
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum Inventory<'a> {
    Native(&'a [String]),
    Managed(&'a Path),
}

pub(crate) use piping_contracts::ansible_options::ArgumentError;

/// Platform-owned inputs are appended after validating the supported option set.
pub(crate) fn playbook_argv(
    raw: &[String],
    entrypoints: &[String],
    inventory: Inventory<'_>,
    extra_vars: &[String],
    limit: Option<&str>,
) -> Result<Vec<OsString>, ArgumentError> {
    if entrypoints.is_empty() {
        return Err(ArgumentError::Invalid("missing playbook"));
    }
    let options = piping_contracts::ansible_options::parse_ansible_options(raw)?;
    let mut args: Vec<OsString> = options.argv().into_iter().map(OsString::from).collect();
    if let Inventory::Native(sources) = inventory {
        for source in sources {
            project_path(source)
                .map_err(|_| ArgumentError::Invalid("invalid native inventory path"))?;
            args.push("--inventory".into());
            args.push(source.into());
        }
    }
    if let Inventory::Managed(path) = inventory {
        if !path.is_absolute() {
            return Err(ArgumentError::Invalid("managed inventory must be absolute"));
        }
        args.push("--inventory".into());
        args.push(path.into());
    }
    for value in extra_vars {
        if value.is_empty() || value.contains('\0') {
            return Err(ArgumentError::Invalid("invalid extra vars"));
        }
        args.push("--extra-vars".into());
        args.push(value.into());
    }
    if let Some(value) = limit {
        if value.is_empty() || value.contains('\0') {
            return Err(ArgumentError::Invalid("invalid limit"));
        }
        args.push(format!("--limit={value}").into());
    }
    let mut seen = HashSet::new();
    for entrypoint in entrypoints {
        let path = project_path(entrypoint)
            .map_err(|_| ArgumentError::Invalid("invalid playbook path"))?;
        if entrypoint.starts_with('-') || !seen.insert(path.clone()) {
            return Err(ArgumentError::Invalid("invalid or duplicate playbook"));
        }
        args.push(path.into_os_string());
    }
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owned_inventory_vars_and_limit_follow_supported_options() {
        let raw = [
            "--tags=deploy,verify",
            "--skip-tags=slow",
            "--check",
            "--diff",
            "-vvvv",
            "--become",
            "--become-user=service",
            "--become-method=sudo",
        ]
        .map(String::from);
        let sources = ["inventory/hosts.ini".to_string()];
        let args = playbook_argv(
            &raw,
            &["site.yml".to_string()],
            Inventory::Native(&sources),
            &["@/job/controller/extra_vars.json".to_string()],
            Some("host*:!excluded"),
        )
        .unwrap();
        assert_eq!(
            args,
            [
                "--tags=deploy,verify",
                "--skip-tags=slow",
                "--become-user=service",
                "--become-method=sudo",
                "--check",
                "--diff",
                "--become",
                "-vvvv",
                "--inventory",
                "inventory/hosts.ini",
                "--extra-vars",
                "@/job/controller/extra_vars.json",
                "--limit=host*:!excluded",
                "site.yml"
            ]
            .map(OsString::from)
        );
    }

    #[test]
    #[ignore = "requires an installed ansible-playbook; runs only on synthetic localhost"]
    fn native_ansible_preserves_typed_vars_and_option_semantics() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("hosts.ini"),
            "localhost ansible_connection=local\n",
        )
        .unwrap();
        fs::write(
            directory.path().join("vars.json"),
            serde_json::to_vec(&serde_json::json!({
                "nested": {"flag": false, "count": 0, "empty": "", "nothing": null},
                "marker": "spaces ' $(not-a-command); & < >"
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            directory.path().join("site.yml"),
            r#"- hosts: all
  gather_facts: false
  tasks:
    - name: Verify launch values
      ansible.builtin.assert:
        that:
          - nested.flag == false
          - nested.count == 0
          - nested.empty == ''
          - nested.nothing == none
          - marker == "spaces ' $(not-a-command); & < >"
          - ansible_check_mode
          - ansible_diff_mode
        quiet: true
      tags: [verify]
    - name: Must not execute
      ansible.builtin.fail:
        msg: excluded task executed
      tags: [skip]
"#,
        )
        .unwrap();
        let sources = ["hosts.ini".to_string()];
        let raw = [
            "--tags=verify,skip",
            "--skip-tags=skip",
            "--check",
            "--diff",
            "-vv",
        ]
        .map(String::from);
        let argv = playbook_argv(
            &raw,
            &["site.yml".to_string()],
            Inventory::Native(&sources),
            &["@vars.json".to_string()],
            Some("localhost"),
        )
        .unwrap();
        let executable = std::env::var_os("PIPING_TEST_ANSIBLE_PLAYBOOK")
            .unwrap_or_else(|| "ansible-playbook".into());
        let result = std::process::Command::new(executable)
            .args(argv)
            .current_dir(directory.path())
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env("HOME", directory.path())
            .env("ANSIBLE_LOCAL_TEMP", directory.path().join("ansible-tmp"))
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "native Ansible failed: {}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        let stdout = String::from_utf8_lossy(&result.stdout);
        assert!(stdout.contains("All assertions passed"));
        assert!(!stdout.contains("excluded task executed"));
    }
    #[test]
    fn preserves_contained_symlink_chain() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("project");
        materialize(
            &root,
            &[
                Entry {
                    path: "file".into(),
                    kind: EntryKind::File {
                        bytes: b"ok".to_vec(),
                        mode: 0o600,
                    },
                },
                Entry {
                    path: "first".into(),
                    kind: EntryKind::Symlink {
                        target: "second".into(),
                    },
                },
                Entry {
                    path: "second".into(),
                    kind: EntryKind::Symlink {
                        target: "file".into(),
                    },
                },
            ],
        )
        .unwrap();
        assert_eq!(fs::read(root.join("first")).unwrap(), b"ok");
    }
    #[test]
    fn preserves_bytes_mode_and_contained_link() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("project");
        materialize(
            &root,
            &[
                Entry {
                    path: "bin/run".into(),
                    kind: EntryKind::File {
                        bytes: vec![0, 255, 3],
                        mode: 0o751,
                    },
                },
                Entry {
                    path: "link".into(),
                    kind: EntryKind::Symlink {
                        target: "bin/run".into(),
                    },
                },
            ],
        )
        .unwrap();
        assert_eq!(fs::read(root.join("link")).unwrap(), [0, 255, 3]);
        assert_eq!(
            fs::metadata(root.join("bin/run"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o751
        );
    }
    #[test]
    fn rejects_escape_and_symlink_ancestors() {
        let dir = tempfile::tempdir().unwrap();
        for (index, entries) in [
            vec![Entry {
                path: "a".into(),
                kind: EntryKind::Symlink {
                    target: "../escape".into(),
                },
            }],
            vec![
                Entry {
                    path: "a".into(),
                    kind: EntryKind::Symlink { target: "x".into() },
                },
                Entry {
                    path: "a/file".into(),
                    kind: EntryKind::File {
                        bytes: vec![],
                        mode: 0o600,
                    },
                },
            ],
            vec![
                Entry {
                    path: "a".into(),
                    kind: EntryKind::Symlink { target: "b".into() },
                },
                Entry {
                    path: "b".into(),
                    kind: EntryKind::Symlink { target: "a".into() },
                },
            ],
        ]
        .into_iter()
        .enumerate()
        {
            assert!(materialize(&dir.path().join(index.to_string()), &entries).is_err());
        }
    }
    #[test]
    fn raw_options_precedence_conflicts_and_prompts() {
        let raw = vec!["--tags".into(), "deploy".into(), "-vv".into()];
        let plays = vec!["play.yml".into()];
        assert_eq!(
            playbook_argv(&raw, &plays, Inventory::Native(&[]), &[], None).unwrap(),
            ["--tags=deploy", "-vv", "play.yml"]
        );
        assert_eq!(
            playbook_argv(
                &["-ihosts".into()],
                &plays,
                Inventory::Managed(Path::new("/job/inventory")),
                &[],
                None
            ),
            Err(ArgumentError::Conflict(
                "Inventory is owned by the job inventory selection"
            ))
        );
        assert_eq!(
            playbook_argv(
                &["--ask-pass".into()],
                &plays,
                Inventory::Native(&[]),
                &[],
                None
            ),
            Err(ArgumentError::NeedsInput)
        );
        assert!(
            playbook_argv(
                &["other.yml".into()],
                &plays,
                Inventory::Native(&[]),
                &[],
                None
            )
            .is_err()
        );
        assert_eq!(
            playbook_argv(
                &["-e".into(), "a=b".into()],
                &plays,
                Inventory::Native(&[]),
                &["c=d".into()],
                None
            ),
            Err(ArgumentError::Conflict(
                "Variables are owned by the parameter editor"
            ))
        );
        assert_eq!(
            playbook_argv(
                &["--limit=x".into()],
                &plays,
                Inventory::Native(&[]),
                &[],
                Some("y")
            ),
            Err(ArgumentError::Conflict(
                "Limit is owned by the job limit field"
            ))
        );
        assert!(
            playbook_argv(
                &["-i/path/key".into()],
                &plays,
                Inventory::Native(&[]),
                &[],
                None
            )
            .is_err()
        );
    }
}
