//! Monitoring context.
//!
//! Owns the monitoring host read model (discovery + enrichment), provider
//! source resolution (SSRF-guarded), the provider HTTP clients
//! (Prometheus-compatible, Alertmanager/vmalert, Zabbix JSON-RPC), the
//! observability list-labels/alerts poller, and the per-host Redis cache
//! contract. The REST layer and the background worker are consumers; nothing
//! in this context may depend on the `api` delivery layer (enforced by the
//! `compliance_context_layering` gate).

pub mod application;
pub mod domain;
pub mod infrastructure;
