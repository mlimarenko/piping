//! Types describing a device user-provisioning WRITE and its dry-run preview.
//!
//! A [`WriteOp`] is the requested mutation; a [`WritePreview`] is the result of
//! evaluating that request against the device's current user list **without
//! mutating it**, so a caller can see create-vs-skip / delete-vs-missing before
//! committing. These are vendor-neutral: the preview is derived purely from the
//! requested names and the names the device already reports.

use serde::{Deserialize, Serialize};

use crate::contexts::device_management::domain::NewUser;

/// A requested user-management mutation on a device.
///
/// The password inside each [`NewUser`] stays redacted in `Debug` output (see
/// [`crate::contexts::device_management::domain::Secret`]).
///
/// `Clone` lets the fleet layer apply one requested op across many targets
/// without the caller re-supplying it per target.
#[derive(Debug, Clone)]
pub enum WriteOp {
    /// Provision the given users.
    CreateUsers(Vec<NewUser>),
    /// Remove the users with the given login names.
    DeleteUsers(Vec<String>),
}

/// What a preview determined would happen to one requested name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewAction {
    /// A requested create whose name is absent — it would be created.
    Create,
    /// A requested create whose name already exists — it would be skipped.
    Skip,
    /// A requested delete whose name is present — it would be removed.
    Delete,
    /// A requested delete whose name is absent — nothing to remove.
    Missing,
}

/// The preview verdict for one requested name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewItem {
    /// The requested login name, verbatim.
    pub name: String,
    /// What would happen to this name on a subsequent write.
    pub action: PreviewAction,
    /// A short, human-readable explanation of the action.
    pub reason: String,
}

/// The outcome of a dry-run: per-name verdicts.
///
/// Whether the caller may carry the previewed write out is an RBAC question
/// answered at the route layer, not part of the preview itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WritePreview {
    /// One verdict per requested name, in request order.
    pub per_item: Vec<PreviewItem>,
}
