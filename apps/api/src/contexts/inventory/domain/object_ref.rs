use serde::Serialize;
use uuid::Uuid;

/// Typed target supplied on write; display labels are resolved by the server.
#[derive(Debug, Clone, serde::Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ObjectRefInput {
    pub object_type: String,
    pub object_id: Uuid,
}

/// Materialized polymorphic reference to another inventory object, used in
/// responses only.
///
/// - `object_type` is the canonical `app_label.model` token (e.g. `"dcim.interface"`).
/// - `label` is `Some("eth0 @ host01")` when the target exists, and `None` when the
///   reference is dangling. A dangling reference stays `None`; it is never expanded
///   into a null `ObjectRef`.
///
/// A null `ObjectRef` on the owning entity means the assignment itself is absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct ObjectRef {
    /// Canonical `app_label.model` content-type token.
    pub object_type: String,
    /// Target row id.
    pub object_id: Uuid,
    /// Human label, or `None` for a dangling reference (target row missing).
    pub label: Option<String>,
}
