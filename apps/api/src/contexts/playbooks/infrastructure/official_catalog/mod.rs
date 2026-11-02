//! Official git-managed playbook catalog.
//!
//! `manifest.rs` defines and validates `piping-playbooks.yaml`; `loader.rs`
//! resolves the catalog directory (baked snapshot with an optional
//! git-refresh overlay), loads content byte-exact, and syncs it into the
//! database at startup. Official playbooks are read-only in the product
//! (`source_kind` `system`/`official` != `in_app`).

mod loader;
mod manifest;
mod refresh;

pub use loader::{
    OfficialCatalog, OfficialCatalogEntry, SYSTEM_INVENTORY_PLAYBOOK_ID,
    SYSTEM_INVENTORY_PLAYBOOK_SLUG, SyncedOfficialPlaybook, load_configured_catalog,
    load_official_catalog, resolve_playbook_id, sync_official_catalog,
};
pub use manifest::{
    CatalogManifest, ManifestPlaybook, OFFICIAL_CATALOG_API_VERSION,
    OFFICIAL_CATALOG_MANIFEST_FILE, OFFICIAL_CATALOG_MAX_CONTENT_BYTES, parse_manifest,
};
pub use refresh::refresh_official_catalog;
