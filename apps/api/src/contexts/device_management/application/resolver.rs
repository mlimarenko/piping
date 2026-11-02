//! Resolving a connection target to a concrete, reachable device.

use async_trait::async_trait;
use thiserror::Error;
use uuid::Uuid;

use crate::contexts::device_management::domain::{DeviceFamily, Secret};
use crate::contexts::device_management::infrastructure::transport::DeviceEndpoint;

/// A connection target resolved to everything a driver needs to reach it.
#[derive(Debug)]
pub struct ResolvedDevice {
    /// Protocol family (selects the driver).
    pub family: DeviceFamily,
    /// Where the device's management service listens.
    pub endpoint: DeviceEndpoint,
    /// Admin username for the device.
    pub username: String,
    /// Admin password (redacted in logs). A video family requires one; a Linux
    /// host authenticates through the SSH proxy's own launch material, which
    /// carries the login credentials, so it may be `None`.
    pub password: Option<Secret>,
    /// Optional privilege-escalation ("become") password for a family that runs
    /// commands through `sudo` (the Linux host). `None` when the family does not
    /// use one or the launch credentials provide none (redacted in logs).
    pub become_password: Option<Secret>,
}

/// Why a connection target could not be resolved to a device.
///
/// Each variant is a distinct, actionable configuration gap rather than an
/// opaque failure, so the REST layer can map it to a precise status.
#[derive(Debug, Error)]
pub enum DeviceResolveError {
    /// No connection target exists with this id.
    #[error("connection target {0} not found")]
    NotFound(Uuid),
    /// The target has no device family bound (via its platform).
    #[error("no device family configured for connection target {0}")]
    UnknownFamily(Uuid),
    /// The target has no reachable management address.
    #[error("no management address for connection target {0}")]
    NoAddress(Uuid),
    /// No device admin credentials are stored for this target.
    #[error("no device credentials configured for connection target {0}")]
    NoCredentials(Uuid),
    /// An unexpected internal failure while resolving.
    #[error("failed to resolve connection target: {0}")]
    Internal(String),
}

/// Resolves a connection target id to a [`ResolvedDevice`].
///
/// The production implementation reads the target's address, derives the family
/// from its platform, and fetches admin credentials through the secret vault
/// fence. Tests provide a fake.
#[async_trait]
pub trait DeviceResolver: Send + Sync {
    /// Resolve `target_id` to a concrete device, or explain why it cannot be.
    async fn resolve(&self, target_id: Uuid) -> Result<ResolvedDevice, DeviceResolveError>;
}
