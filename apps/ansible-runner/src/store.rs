//! Supervisor-only durable state. No path in this store is mounted into a job.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, bail};
use serde::{Serialize, de::DeserializeOwned};
use uuid::Uuid;

const MAX_RECEIPT_BYTES: u64 = 1024 * 1024;

#[derive(Clone)]
pub(crate) struct SupervisorStore {
    root: PathBuf,
    _lock: Arc<fs::File>,
}

impl SupervisorStore {
    pub(crate) fn open(root: &Path) -> anyhow::Result<Self> {
        private_directory(root)?;
        let root = root
            .canonicalize()
            .context("resolve supervisor state root")?;
        let lock_path = root.join("supervisor.lock");
        if let Ok(metadata) = fs::symlink_metadata(&lock_path) {
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                bail!("invalid supervisor lock file");
            }
        }
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(lock_path)?;
        lock.try_lock()
            .context("another supervisor owns this state root")?;
        private_directory(&root.join("dispatches"))?;
        private_directory(&root.join("console"))?;
        private_directory(&root.join("acquisitions"))?;
        private_directory(&root.join("acquisition-results"))?;
        Ok(Self {
            root,
            _lock: Arc::new(lock),
        })
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn dispatch_exists(&self, id: Uuid) -> anyhow::Result<bool> {
        Ok(self
            .root
            .join("dispatches")
            .join(format!("{id}.json"))
            .try_exists()?)
    }

    pub(crate) fn acquisition_exists(&self, id: Uuid) -> anyhow::Result<bool> {
        Ok(self
            .root
            .join("acquisitions")
            .join(format!("{id}.json"))
            .try_exists()?)
    }

    pub(crate) fn save_acquisition<T: Serialize>(&self, id: Uuid, value: &T) -> anyhow::Result<()> {
        write_json_atomic(
            &self.root.join("acquisitions").join(format!("{id}.json")),
            value,
        )
    }

    pub(crate) fn save_acquisition_result<T: Serialize>(
        &self,
        id: Uuid,
        value: &T,
    ) -> anyhow::Result<()> {
        write_json_atomic(
            &self
                .root
                .join("acquisition-results")
                .join(format!("{id}.json")),
            value,
        )
    }

    pub(crate) fn load_acquisition_result<T: DeserializeOwned>(
        &self,
        id: Uuid,
    ) -> anyhow::Result<T> {
        read_json(
            &self
                .root
                .join("acquisition-results")
                .join(format!("{id}.json")),
            32 * 1024 * 1024,
        )
    }

    pub(crate) fn load_acquisitions<T: DeserializeOwned>(&self) -> anyhow::Result<Vec<(Uuid, T)>> {
        let mut records = Vec::new();
        for entry in fs::read_dir(self.root.join("acquisitions"))? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(id) = name.to_str().and_then(|name| name.strip_suffix(".json")) else {
                continue;
            };
            records.push((
                Uuid::parse_str(id)?,
                read_json(&entry.path(), MAX_RECEIPT_BYTES)?,
            ));
        }
        Ok(records)
    }

    pub(crate) fn console_count(&self, id: Uuid) -> anyhow::Result<u64> {
        let path = self.root.join("console").join(id.to_string());
        if !path.try_exists()? {
            return Ok(0);
        }
        let mut sequences = std::collections::BTreeSet::new();
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let name = entry.file_name();
            if let Some(sequence) = name.to_str().and_then(|name| name.strip_suffix(".json")) {
                sequences.insert(sequence.parse::<u64>()?);
            }
        }
        for (expected, sequence) in sequences.iter().enumerate() {
            if expected as u64 != *sequence {
                bail!("persisted console has a sequence gap");
            }
        }
        Ok(sequences.len() as u64)
    }

    pub(crate) fn save_dispatch<T: Serialize>(&self, id: Uuid, value: &T) -> anyhow::Result<()> {
        write_json_atomic(
            &self.root.join("dispatches").join(format!("{id}.json")),
            value,
        )
    }

    pub(crate) fn load_dispatches<T: DeserializeOwned>(&self) -> anyhow::Result<Vec<(Uuid, T)>> {
        let mut result = Vec::new();
        for entry in fs::read_dir(self.root.join("dispatches"))? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str().and_then(|name| name.strip_suffix(".json")) else {
                continue;
            };
            let id = Uuid::parse_str(name).context("invalid dispatch receipt identity")?;
            result.push((id, read_json(&entry.path(), MAX_RECEIPT_BYTES)?));
        }
        Ok(result)
    }

    pub(crate) fn save_console<T: Serialize>(
        &self,
        id: Uuid,
        sequence: u64,
        value: &T,
    ) -> anyhow::Result<()> {
        let directory = self.root.join("console").join(id.to_string());
        private_directory(&directory)?;
        let path = directory.join(format!("{sequence:020}.json"));
        if path.try_exists()? {
            bail!("console sequence was already persisted");
        }
        write_json_atomic(&path, value)
    }

    pub(crate) fn load_console<T: DeserializeOwned>(
        &self,
        id: Uuid,
        sequence: u64,
    ) -> anyhow::Result<T> {
        read_json(
            &self
                .root
                .join("console")
                .join(id.to_string())
                .join(format!("{sequence:020}.json")),
            MAX_RECEIPT_BYTES,
        )
    }
}

pub(crate) fn private_directory(path: &Path) -> anyhow::Result<()> {
    let mut created = false;
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                bail!("private state path is not a directory");
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path).context("create private state directory")?;
            created = true;
        }
        Err(error) => return Err(error.into()),
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    if created {
        fs::File::open(path)?.sync_all()?;
        if let Some(parent) = path.parent() {
            fs::File::open(parent)?.sync_all()?;
        }
    }
    Ok(())
}

fn read_json<T: DeserializeOwned>(path: &Path, limit: u64) -> anyhow::Result<T> {
    let metadata = fs::symlink_metadata(path).context("inspect supervisor record")?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > limit {
        bail!("invalid or oversized supervisor record");
    }
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        bail!("oversized supervisor record");
    }
    serde_json::from_slice(&bytes).context("decode supervisor record")
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> anyhow::Result<()> {
    let parent = path.parent().context("supervisor record has no parent")?;
    let temporary = parent.join(format!(".{}.tmp", Uuid::now_v7()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)?;
    let result = (|| {
        serde_json::to_writer(&mut file, value).context("encode supervisor record")?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;
    use serde_json::{Value, json};

    #[test]
    fn durable_receipt_reopens_and_console_is_sequence_addressed() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let store = SupervisorStore::open(directory.path())?;
        let id = Uuid::now_v7();
        store.save_dispatch(id, &json!({"status":"canceling"}))?;
        store.save_console(id, 1, &json!({"bytes":[27, 91, 51, 49, 109]}))?;
        assert!(store.save_console(id, 1, &json!({})).is_err());
        assert!(SupervisorStore::open(directory.path()).is_err());
        drop(store);
        let reopened = SupervisorStore::open(directory.path())?;
        assert_eq!(
            reopened.load_dispatches::<Value>()?,
            vec![(id, json!({"status":"canceling"}))]
        );
        assert_eq!(
            reopened.load_console::<Value>(id, 1)?,
            json!({"bytes":[27, 91, 51, 49, 109]})
        );
        Ok(())
    }

    #[test]
    fn rejects_symlink_state_roots_and_receipts() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let target = tempfile::tempdir()?;
        symlink(target.path(), directory.path().join("alias"))?;
        assert!(SupervisorStore::open(&directory.path().join("alias")).is_err());
        let store = SupervisorStore::open(&directory.path().join("state"))?;
        let id = Uuid::now_v7();
        fs::write(target.path().join("receipt"), b"{}")?;
        symlink(
            target.path().join("receipt"),
            store.root().join("dispatches").join(format!("{id}.json")),
        )?;
        assert!(store.load_dispatches::<Value>().is_err());
        Ok(())
    }

    #[test]
    fn acquisition_metadata_and_result_survive_independent_reopen() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let store = SupervisorStore::open(directory.path())?;
        let id = Uuid::new_v4();
        store.save_acquisition_result(id, &json!({"entries":[{"bytes":[0,255]}]}))?;
        store.save_acquisition(id, &json!({"response_available":true}))?;
        drop(store);
        let store = SupervisorStore::open(directory.path())?;
        assert_eq!(
            store.load_acquisitions::<Value>()?,
            vec![(id, json!({"response_available":true}))]
        );
        assert_eq!(
            store.load_acquisition_result::<Value>(id)?,
            json!({"entries":[{"bytes":[0,255]}]})
        );
        Ok(())
    }

    #[test]
    fn recovery_counts_only_a_contiguous_durable_console_prefix() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let store = SupervisorStore::open(directory.path())?;
        let id = Uuid::new_v4();
        assert_eq!(store.console_count(id)?, 0);
        store.save_console(id, 0, &json!({"bytes":[255]}))?;
        assert_eq!(store.console_count(id)?, 1);
        store.save_console(id, 2, &json!({"bytes":[0]}))?;
        assert!(store.console_count(id).is_err());
        Ok(())
    }
}
