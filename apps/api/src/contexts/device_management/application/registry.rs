//! Maps a resolved device's family to its native driver.
//!
//! This registry is the device-management composition seam: the one place that
//! deliberately references concrete `infrastructure` adapters (the native
//! drivers and the transport port) to wire a family to its driver. Callers in
//! `application` depend only on the `DeviceDriver` abstraction, not on this map.

use std::sync::Arc;

use uuid::Uuid;

use crate::contexts::device_management::application::device_service::DeviceServiceError;
use crate::contexts::device_management::application::resolver::{
    DeviceResolveError, ResolvedDevice,
};
use crate::contexts::device_management::application::ssh_exec::SshExecPort;
use crate::contexts::device_management::domain::{DeviceDriver, DeviceFamily, DriverError, Secret};
use crate::contexts::device_management::infrastructure::drivers::dahua::DahuaDriver;
use crate::contexts::device_management::infrastructure::drivers::dvrip::DvripDriver;
use crate::contexts::device_management::infrastructure::drivers::hikvision::HikvisionDriver;
use crate::contexts::device_management::infrastructure::drivers::linux::LinuxDriver;
use crate::contexts::device_management::infrastructure::drivers::onvif::OnvifDriver;
use crate::contexts::device_management::infrastructure::http_client::ReqwestDigestClient;
use crate::contexts::device_management::infrastructure::transport::DeviceTransport;

/// Builds a native [`DeviceDriver`] for a resolved device.
///
/// Adding a family is one arm here plus its driver impl. Families whose driver
/// is not yet implemented return [`DriverError::Unsupported`].
#[derive(Clone)]
pub struct DeviceDriverRegistry {
    transport: Arc<dyn DeviceTransport>,
    /// SSH exec runtime used by the Linux family; `None` when the deployment has
    /// no embedded proxy, in which case the Linux family is unsupported.
    ssh_exec: Option<Arc<dyn SshExecPort>>,
}

impl DeviceDriverRegistry {
    /// Construct a registry whose drivers reach devices via `transport`.
    pub fn new(transport: Arc<dyn DeviceTransport>) -> Self {
        Self {
            transport,
            ssh_exec: None,
        }
    }

    /// Attach the SSH exec runtime the Linux family runs commands through.
    #[must_use]
    pub fn with_ssh_exec(mut self, ssh_exec: Arc<dyn SshExecPort>) -> Self {
        self.ssh_exec = Some(ssh_exec);
        self
    }

    /// Build the driver for `device`, identified by its connection `target_id`.
    ///
    /// # Errors
    ///
    /// Returns [`DeviceServiceError::Resolve`] carrying
    /// [`DeviceResolveError::NoCredentials`] when a video family is resolved
    /// without a password (the same actionable gap the resolver reports), and
    /// [`DeviceServiceError::Driver`] carrying [`DriverError::Unsupported`] for
    /// the Linux family when this deployment has no SSH exec runtime.
    pub fn driver(
        &self,
        target_id: Uuid,
        device: &ResolvedDevice,
    ) -> Result<Box<dyn DeviceDriver>, DeviceServiceError> {
        match device.family {
            DeviceFamily::DvripSofia => Ok(Box::new(DvripDriver::new(
                Arc::clone(&self.transport),
                device.endpoint.clone(),
                device.username.clone(),
                video_password(target_id, device)?,
            ))),
            DeviceFamily::Dahua => {
                // Dahua speaks HTTP CGI with Digest auth, so it builds its own
                // HTTP client from the endpoint rather than using the byte
                // transport the DVRIP driver needs.
                let password = video_password(target_id, device)?;
                let http = ReqwestDigestClient::new(
                    &device.endpoint,
                    device.username.clone(),
                    password.clone(),
                )?;
                Ok(Box::new(DahuaDriver::new(
                    Arc::new(http),
                    device.endpoint.clone(),
                    device.username.clone(),
                    password,
                )))
            }
            DeviceFamily::Hikvision => {
                // Hikvision speaks ISAPI over HTTP with Digest auth — the same
                // HTTP client Dahua uses, built from the endpoint.
                let password = video_password(target_id, device)?;
                let http = ReqwestDigestClient::new(
                    &device.endpoint,
                    device.username.clone(),
                    password.clone(),
                )?;
                Ok(Box::new(HikvisionDriver::new(
                    Arc::new(http),
                    device.endpoint.clone(),
                    device.username.clone(),
                    password,
                )))
            }
            DeviceFamily::Onvif => {
                // Generic ONVIF speaks SOAP/XML over HTTP with a WS-Security
                // UsernameToken in the SOAP header (not HTTP Digest), but it
                // reuses the same HTTP client — the WS-Security auth lives in the
                // request body the driver builds, so the transport is unchanged.
                let password = video_password(target_id, device)?;
                let http = ReqwestDigestClient::new(
                    &device.endpoint,
                    device.username.clone(),
                    password.clone(),
                )?;
                Ok(Box::new(OnvifDriver::new(
                    Arc::new(http),
                    device.endpoint.clone(),
                    device.username.clone(),
                    password,
                )))
            }
            DeviceFamily::Linux => {
                // A Linux host runs commands over the existing proxy SSH exec
                // runtime, which resolves the host/port/login credentials for
                // `target_id`; only the login username is needed locally for the
                // root privilege check, so a missing password is not an error. A
                // deployment without an embedded proxy cannot manage it.
                let ssh = self.ssh_exec.clone().ok_or(DriverError::Unsupported)?;
                Ok(Box::new(LinuxDriver::new(
                    ssh,
                    target_id,
                    device.username.clone(),
                    device.become_password.clone(),
                )))
            }
        }
    }
}

/// The password a video driver authenticates with.
///
/// A video device resolved without one is a configuration gap, not a driver
/// failure: it is reported with the same [`DeviceResolveError::NoCredentials`]
/// the resolver uses, so the REST layer keeps answering
/// `CONNECTION_CREDENTIALS_REQUIRED`.
fn video_password(target_id: Uuid, device: &ResolvedDevice) -> Result<Secret, DeviceServiceError> {
    device.password.clone().ok_or(DeviceServiceError::Resolve(
        DeviceResolveError::NoCredentials(target_id),
    ))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::contexts::device_management::infrastructure::transport::{
        DeviceEndpoint, DirectTcpTransport,
    };

    fn registry() -> DeviceDriverRegistry {
        DeviceDriverRegistry::new(Arc::new(DirectTcpTransport::new(Duration::from_secs(5))))
    }

    fn resolved(family: DeviceFamily, port: u16) -> ResolvedDevice {
        ResolvedDevice {
            family,
            endpoint: DeviceEndpoint::new("device.test", port),
            username: "admin".to_owned(),
            password: Some(Secret::new("pw")),
            become_password: None,
        }
    }

    #[test]
    fn dahua_family_yields_a_driver_with_the_expected_capabilities() {
        let driver = registry()
            .driver(Uuid::now_v7(), &resolved(DeviceFamily::Dahua, 80))
            .expect("dahua driver builds");
        let capabilities = driver.capabilities();
        assert!(capabilities.info);
        assert!(capabilities.users);
        assert!(capabilities.disk);
        assert!(capabilities.streaming);
        assert!(capabilities.recording);
        assert!(capabilities.export);
        assert!(capabilities.snapshot);
        assert!(capabilities.user_update);
    }

    #[test]
    fn dvrip_family_still_builds_its_driver() {
        let driver = registry()
            .driver(Uuid::now_v7(), &resolved(DeviceFamily::DvripSofia, 34567))
            .expect("dvrip driver builds");
        let capabilities = driver.capabilities();
        assert!(capabilities.recording);
        // This family's archive is served over its own control protocol
        // (`OPFileQuery` + `OPPlayBack`) rather than RTSP playback, so export
        // was long reported as unsupported here. The driver now implements that
        // path, and the capability must say so.
        assert!(capabilities.export);
        assert!(capabilities.users);
        assert!(capabilities.user_update);
    }

    #[test]
    fn hikvision_family_yields_a_driver_with_the_expected_capabilities() {
        let driver = registry()
            .driver(Uuid::now_v7(), &resolved(DeviceFamily::Hikvision, 80))
            .expect("hikvision driver builds");
        let capabilities = driver.capabilities();
        assert!(capabilities.info);
        assert!(capabilities.users);
        assert!(capabilities.disk);
        assert!(capabilities.streaming);
        assert!(capabilities.recording);
        assert!(capabilities.export);
        assert!(capabilities.snapshot);
        assert!(capabilities.user_update);
    }

    #[test]
    fn onvif_family_yields_a_view_only_driver() {
        let driver = registry()
            .driver(Uuid::now_v7(), &resolved(DeviceFamily::Onvif, 80))
            .expect("onvif driver builds");
        let capabilities = driver.capabilities();
        // ONVIF is the view fallback: identity/snapshot/streaming/channels/PTZ,
        // but never the management plane (users/disk/recording).
        assert!(capabilities.info);
        assert!(capabilities.snapshot);
        assert!(capabilities.streaming);
        assert!(capabilities.channels);
        assert!(capabilities.ptz);
        assert!(!capabilities.users);
        assert!(!capabilities.disk);
        assert!(!capabilities.recording);
        assert!(!capabilities.export);
        assert!(!capabilities.user_update);
    }

    /// Minimal SSH port double: the registry only wires it, it is never called.
    struct NoopSshExec;

    #[async_trait::async_trait]
    impl SshExecPort for NoopSshExec {
        async fn exec(
            &self,
            _request: crate::contexts::device_management::application::ssh_exec::SshExecRequest,
        ) -> Result<
            crate::contexts::device_management::application::ssh_exec::SshExecOutput,
            DriverError,
        > {
            Err(DriverError::Unsupported)
        }
    }

    #[test]
    fn linux_family_is_unsupported_without_an_ssh_runtime() {
        let result = registry().driver(Uuid::now_v7(), &resolved(DeviceFamily::Linux, 22));
        assert!(matches!(
            result,
            Err(DeviceServiceError::Driver(DriverError::Unsupported))
        ));
    }

    #[test]
    fn video_family_without_a_password_reports_no_credentials() {
        let target_id = Uuid::now_v7();
        let mut device = resolved(DeviceFamily::Dahua, 80);
        device.password = None;
        let result = registry().driver(target_id, &device);
        assert!(matches!(
            result,
            Err(DeviceServiceError::Resolve(
                DeviceResolveError::NoCredentials(got)
            )) if got == target_id
        ));
    }

    #[test]
    fn linux_family_yields_a_user_management_driver_with_an_ssh_runtime() {
        let registry =
            DeviceDriverRegistry::new(Arc::new(DirectTcpTransport::new(Duration::from_secs(5))))
                .with_ssh_exec(Arc::new(NoopSshExec));
        let driver = registry
            .driver(Uuid::now_v7(), &resolved(DeviceFamily::Linux, 22))
            .expect("linux driver builds");
        let capabilities = driver.capabilities();
        assert!(capabilities.info);
        assert!(capabilities.users);
        assert!(capabilities.user_update);
        assert!(!capabilities.streaming);
        assert!(!capabilities.snapshot);
    }
}
