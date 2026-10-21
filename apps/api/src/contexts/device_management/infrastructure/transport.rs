//! Byte transport to a device.
//!
//! Drivers speak their native protocol over a [`DeviceStream`] — a minimal
//! object-safe duplex with exactly the two operations a request/response
//! protocol needs. A [`DeviceTransport`] opens streams to a device endpoint.
//!
//! Two transports exist:
//! - [`DirectTcpTransport`] dials the device directly. It is usable only where
//!   the backend can reach the device network (the embedded-proxy / dev case).
//! - The production transport routes every connection through an edge proxy node
//!   (the same path RDP/VNC/SSH use); it is added in a later phase. Because both
//!   implement [`DeviceTransport`], drivers do not change when it lands.

use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::contexts::device_management::domain::DriverError;

/// Where a device's management service listens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceEndpoint {
    /// Device host or IP (as reachable from the transport).
    pub host: String,
    /// Management TCP port.
    pub port: u16,
}

impl DeviceEndpoint {
    /// Construct an endpoint.
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
        }
    }
}

/// An open, object-safe duplex byte stream to a device.
///
/// The two methods map I/O failures into typed driver transport errors so
/// drivers work purely in terms of the driver error type.
#[async_trait]
pub trait DeviceStream: Send {
    /// Write the whole buffer.
    async fn send(&mut self, bytes: &[u8]) -> Result<(), DriverError>;
    /// Read exactly `buf.len()` bytes, erroring on early EOF.
    async fn recv_exact(&mut self, buf: &mut [u8]) -> Result<(), DriverError>;
}

#[async_trait]
impl<S: AsyncRead + AsyncWrite + Unpin + Send> DeviceStream for S {
    async fn send(&mut self, bytes: &[u8]) -> Result<(), DriverError> {
        self.write_all(bytes)
            .await
            .map_err(classify_io_transport_error)
    }

    async fn recv_exact(&mut self, buf: &mut [u8]) -> Result<(), DriverError> {
        self.read_exact(buf)
            .await
            .map(|_read| ())
            .map_err(classify_io_transport_error)
    }
}

/// Opens byte streams to device endpoints.
#[async_trait]
pub trait DeviceTransport: Send + Sync {
    /// Open a stream to `endpoint`.
    async fn connect(
        &self,
        endpoint: &DeviceEndpoint,
    ) -> Result<Box<dyn DeviceStream>, DriverError>;
}

/// A transport that dials the device directly over TCP.
///
/// Only correct where the backend process can reach the device network. In
/// production, devices live on a customer LAN and must be reached through a
/// proxy node instead.
#[derive(Debug, Clone)]
pub struct DirectTcpTransport {
    connect_timeout: Duration,
}

impl DirectTcpTransport {
    /// Construct a direct transport with the given connect timeout.
    pub fn new(connect_timeout: Duration) -> Self {
        Self { connect_timeout }
    }
}

#[async_trait]
impl DeviceTransport for DirectTcpTransport {
    async fn connect(
        &self,
        endpoint: &DeviceEndpoint,
    ) -> Result<Box<dyn DeviceStream>, DriverError> {
        let connecting = TcpStream::connect((endpoint.host.as_str(), endpoint.port));
        let stream = tokio::time::timeout(self.connect_timeout, connecting)
            .await
            .map_err(|_elapsed| {
                DriverError::Timeout("device connect deadline exceeded".to_owned())
            })?
            .map_err(classify_io_transport_error)?;
        Ok(Box::new(stream))
    }
}

fn classify_io_transport_error(error: std::io::Error) -> DriverError {
    match error.kind() {
        std::io::ErrorKind::TimedOut => {
            DriverError::Timeout("device I/O deadline exceeded".to_owned())
        }
        std::io::ErrorKind::ConnectionRefused
        | std::io::ErrorKind::ConnectionReset
        | std::io::ErrorKind::ConnectionAborted
        | std::io::ErrorKind::NotConnected
        | std::io::ErrorKind::AddrNotAvailable
        | std::io::ErrorKind::BrokenPipe
        | std::io::ErrorKind::UnexpectedEof => {
            DriverError::Unreachable("device transport is unavailable".to_owned())
        }
        _ => DriverError::Transport("device transport failed".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::classify_io_transport_error;
    use crate::contexts::device_management::domain::DriverError;

    #[test]
    fn classifies_io_timeout_and_unreachable_without_message_matching() {
        assert!(matches!(
            classify_io_transport_error(std::io::Error::from(std::io::ErrorKind::TimedOut)),
            DriverError::Timeout(_)
        ));
        assert!(matches!(
            classify_io_transport_error(std::io::Error::from(
                std::io::ErrorKind::ConnectionRefused
            )),
            DriverError::Unreachable(_)
        ));
        assert!(matches!(
            classify_io_transport_error(std::io::Error::from(std::io::ErrorKind::Other)),
            DriverError::Transport(_)
        ));
    }
}
