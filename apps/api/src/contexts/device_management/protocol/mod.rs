//! Wire-level codecs for native device protocols.
//!
//! These are pure, I/O-free encoders/decoders. A driver combines a codec here
//! with a byte transport (routed through a proxy node) to perform an operation.

pub mod dvrip;
