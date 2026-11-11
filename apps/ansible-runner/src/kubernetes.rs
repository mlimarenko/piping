//! Kubernetes Job adapter and bounded, job-private Secret transport.

mod lifecycle;
pub(crate) use lifecycle::Kubernetes;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use serde::Serialize;

/// Maximum serialized Secret payload, below Kubernetes' one MiB Secret limit.
const MAX_PAYLOAD: usize = 700 * 1024;

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum TreeEntry {
    Directory {
        path: String,
        mode: u32,
    },
    File {
        path: String,
        mode: u32,
        bytes: Vec<u8>,
    },
    Symlink {
        path: String,
        target: String,
    },
}

impl TreeEntry {
    fn path(&self) -> &str {
        match self {
            Self::Directory { path, .. } | Self::File { path, .. } | Self::Symlink { path, .. } => {
                path
            }
        }
    }
}

/// Serialize only a complete tree whose paths and links stay beneath /job.
pub(crate) fn encode_tree(entries: &[TreeEntry]) -> Result<Vec<u8>, &'static str> {
    let mut paths = BTreeMap::new();
    for entry in entries {
        let path = checked_path(entry.path())?;
        if paths.insert(path, entry).is_some() {
            return Err("duplicate tree path");
        }
        match entry {
            TreeEntry::Directory { mode, .. } | TreeEntry::File { mode, .. }
                if mode & !0o777 != 0 =>
            {
                return Err("invalid file mode");
            }
            TreeEntry::Symlink { target, .. } => {
                if target.is_empty() || target.contains('\0') || Path::new(target).is_absolute() {
                    return Err("invalid link target");
                }
                let mut destination = checked_path(entry.path())?;
                destination.pop();
                for part in Path::new(target).components() {
                    match part {
                        Component::Normal(part) => destination.push(part),
                        Component::CurDir => {}
                        Component::ParentDir if destination.pop() => {}
                        _ => return Err("link escapes tree"),
                    }
                }
            }
            _ => {}
        }
    }
    // Require explicit directories and disallow traversing links or files while
    // materializing, independently of the order supplied by the caller.
    for path in paths.keys() {
        let mut parent = path.parent();
        while let Some(dir) = parent {
            if dir.as_os_str().is_empty() {
                break;
            }
            if !matches!(paths.get(dir), Some(TreeEntry::Directory { .. })) {
                return Err("missing or non-directory parent");
            }
            parent = dir.parent();
        }
    }
    for entry in entries {
        if let TreeEntry::Symlink { path, target } = entry {
            let mut destination = checked_path(path)?;
            destination.pop();
            for part in Path::new(target).components() {
                match part {
                    Component::Normal(part) => destination.push(part),
                    Component::ParentDir => {
                        destination.pop();
                    }
                    _ => {}
                }
            }
            if destination
                .ancestors()
                .any(|part| matches!(paths.get(part), Some(TreeEntry::Symlink { .. })))
            {
                return Err("link target traverses another link");
            }
        }
    }
    let payload = serde_json::to_vec(entries).map_err(|_| "cannot serialize tree")?;
    if payload.len() > MAX_PAYLOAD {
        return Err("tree exceeds Secret transport limit");
    }
    Ok(payload)
}

fn checked_path(path: &str) -> Result<PathBuf, &'static str> {
    if path.is_empty()
        || path.contains('\0')
        || path
            .split('/')
            .any(|s| s.is_empty() || s == "." || s == "..")
    {
        return Err("invalid tree path");
    }
    let path = Path::new(path);
    if path
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err("invalid tree path");
    }
    Ok(path.to_path_buf())
}

/// Decode newline-delimited launcher frames without interpreting output as UTF-8.
/// The private exec stream carries console stdout/stderr and the bounded,
/// untrusted structured notification as distinct streams; callers must never
/// merge the notification into console output.
pub(crate) fn decode_frames(
    input: &[u8],
    limit: usize,
) -> Result<(Vec<u8>, Vec<u8>, Vec<u8>), &'static str> {
    if input.len() > limit.saturating_mul(8).saturating_add(4096) {
        return Err("encoded output limit exceeded");
    }
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut notify = Vec::new();
    let mut total = 0usize;
    for line in input.split(|b| *b == b'\n') {
        if line.is_empty() {
            continue;
        }
        let frame: serde_json::Value =
            serde_json::from_slice(line).map_err(|_| "invalid output frame")?;
        let object = frame.as_object().ok_or("invalid output frame")?;
        let keys: BTreeSet<_> = object.keys().map(String::as_str).collect();
        if keys != BTreeSet::from(["bytes", "stream"]) {
            return Err("invalid output frame");
        }
        let bytes = object["bytes"].as_array().ok_or("invalid output frame")?;
        total = total
            .checked_add(bytes.len())
            .ok_or("output limit exceeded")?;
        if total > limit {
            return Err("output limit exceeded");
        }
        let dest = match object["stream"].as_str() {
            Some("stdout") => &mut stdout,
            Some("stderr") => &mut stderr,
            Some("notify") => &mut notify,
            _ => return Err("invalid output frame"),
        };
        for byte in bytes {
            dest.push(
                u8::try_from(byte.as_u64().ok_or("invalid output frame")?)
                    .map_err(|_| "invalid output frame")?,
            );
        }
    }
    Ok((stdout, stderr, notify))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_escape_duplicate_and_non_directory_parent() {
        assert!(
            encode_tree(&[TreeEntry::Symlink {
                path: "a".into(),
                target: "../outside".into()
            }])
            .is_err()
        );
        assert!(
            encode_tree(&[
                TreeEntry::File {
                    path: "a".into(),
                    mode: 0o600,
                    bytes: vec![]
                },
                TreeEntry::File {
                    path: "a/b".into(),
                    mode: 0o600,
                    bytes: vec![]
                }
            ])
            .is_err()
        );
        assert!(
            encode_tree(&[
                TreeEntry::Directory {
                    path: "a".into(),
                    mode: 0o700
                },
                TreeEntry::Directory {
                    path: "a".into(),
                    mode: 0o700
                }
            ])
            .is_err()
        );
    }

    #[test]
    fn frames_preserve_binary_and_keep_notify_separate() {
        let frames = b"{\"stream\":\"stdout\",\"bytes\":[27,0,255]}\n{\"stream\":\"stderr\",\"bytes\":[65]}\n{\"stream\":\"notify\",\"bytes\":[123,125]}\n";
        assert_eq!(
            decode_frames(frames, 6).unwrap(),
            (vec![27, 0, 255], vec![65], vec![123, 125])
        );
        assert!(decode_frames(frames, 5).is_err());
        assert!(decode_frames(b"{\"stream\":\"stdout\",\"bytes\":[256]}", 1).is_err());
        assert!(decode_frames(b"{\"stream\":\"notify\",\"bytes\":[1],\"x\":1}", 2).is_err());
    }
}
