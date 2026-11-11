//! Licensing repository interfaces, SQLx implementations, and their re-exports.
pub mod repository;
pub mod sqlx_licensing_repository;
pub mod sqlx_seat_ledger_repository;

pub use repository::{
    ActivationContext, EntitlementMetadata, LicenseStateRecord, LicensingRepository,
    RefreshFailure, SeatLedgerEvent, SeatLedgerRepository,
};
pub use sqlx_licensing_repository::SqlxLicensingRepository;
pub use sqlx_seat_ledger_repository::SqlxSeatLedgerRepository;
