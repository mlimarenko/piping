//! Application services for device management.
//!
//! [`DeviceManagementService`] is the orchestration seam the REST/MCP layer
//! calls: it resolves a connection target to a concrete device (family,
//! endpoint, credentials) via a [`resolver::DeviceResolver`], builds the right
//! native driver through the [`registry::DeviceDriverRegistry`], and runs one
//! management operation.

pub mod device_service;
pub mod fleet;
pub mod registry;
pub mod remux;
pub mod resolver;
pub mod rtsp_clip;
pub mod rtsp_live;
pub mod rtsp_still;
pub mod ssh_exec;
pub mod write;

pub use device_service::{DeviceManagementService, DeviceServiceError};
pub use fleet::{
    FleetPreview, FleetPreviewOutcome, FleetTarget, FleetTargetOutcome, FleetWriteResult,
    FleetWriteService,
};
pub use registry::DeviceDriverRegistry;
pub use remux::RemuxSource;
pub use resolver::{DeviceResolveError, DeviceResolver, ResolvedDevice};
pub use rtsp_clip::{ClipStream, RtspClipSource};
pub use rtsp_live::{LiveStreamError, LiveStreamOptions, MjpegStream, RtspLiveSource};
pub use rtsp_still::RtspStillSource;
pub use ssh_exec::{SshExecOutput, SshExecPort, SshExecRequest};
pub use write::{PreviewAction, PreviewItem, WriteOp, WritePreview};
