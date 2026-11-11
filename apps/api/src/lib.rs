#![doc(hidden)]
//! Piping control-plane API library crate root: top-level module declarations.

pub(crate) const PRODUCT_VERSION: &str = env!("PIPING_PRODUCT_VERSION");

pub mod api;
pub mod app;
pub mod config;
pub mod contexts;
pub mod errors;
pub mod integrations;
pub mod observability;
pub mod proxy;
pub mod storage;
