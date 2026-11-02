//! Cctv-device (DVR / NVR / IP-camera) management.
//!
//! This context models the *management plane* of recorders and cameras: reading
//! device identity, users, disk health and recording state, and — in later
//! phases — provisioning users in bulk. It is deliberately separate from the
//! interactive streaming plane (live view), which reuses the existing
//! connection/session machinery.
//!
//! # Layers
//!
//! - [`domain`] — the vendor-neutral [`domain::DeviceDriver`] contract and its
//!   normalized data model. A driver hides one device family's native protocol
//!   behind a small set of request/response operations.
//! - [`protocol`] — wire-level codecs for the native protocols, starting with
//!   [`protocol::dvrip`] (the Sofia / XMeye protocol on TCP 34567).
//!
//! The management operations are stateless request/response calls; they do not
//! open or track a session. Transport to devices on a customer LAN is routed
//! through the same edge proxy nodes used by RDP/VNC/SSH — wired in a later
//! phase.

pub mod application;
pub mod domain;
pub mod infrastructure;
pub mod protocol;
