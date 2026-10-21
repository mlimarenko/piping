//! Connection-configuration domain records, assignments, scopes and effective-config resolution.
pub mod assignment;
pub mod effective;
pub mod record;
pub mod scope;

pub use assignment::{ConnConfAssignment, NewConnConfAssignment, UpdateConnConfAssignment};
pub use effective::{ConnConfSource, EffectiveConnConfig};
pub use record::{ConnConfRecord, NewConnConfRecord, UpdateConnConfRecord};
pub use scope::{ConnConfProtocol, ConnConfScope};
