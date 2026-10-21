//! Vendor-neutral domain model for cctv-device management.

pub mod device_family;
pub mod driver;

pub use device_family::{DeviceFamily, DeviceFamilyParseError};
pub use driver::{
    BulkOutcome, Capabilities, Channel, ClipEncoding, ClipRef, DeviceDriver, DeviceInfo, DiskState,
    DiskStatus, DownloadStream, DriverError, ExportRequest, LinkFailurePolicy, NewUser, PtzCommand,
    PtzSupport, PtzVector, RecordSpan, RecordingStatus, RemuxHints, Secret, SnapshotRef,
    StreamTransport, StreamUri, UserAccount, UserUpdate,
};
