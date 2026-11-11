//! Application layer for the connection-config context.
//!
//! The headline piece is [`resolve_effective_config`]: the pure,
//! unit-testable effective-config resolver.

pub mod effective_config_resolver;
pub mod launch_accounts_resolver;

pub use effective_config_resolver::{
    ResolveInput, ResolvedRecordParams, ResolverError, resolve_effective_config,
    resolve_record_params,
};
pub use launch_accounts_resolver::{LaunchAccount, resolve_launch_accounts};
