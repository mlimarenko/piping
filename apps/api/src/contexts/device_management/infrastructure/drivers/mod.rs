//! Native device-family drivers.
//!
//! Each driver implements [`DeviceDriver`](crate::contexts::device_management::domain::DeviceDriver)
//! for one family over a [`DeviceTransport`](super::transport::DeviceTransport).

pub mod dahua;
pub mod dvrip;
pub mod hikvision;
pub mod linux;
pub mod onvif;
