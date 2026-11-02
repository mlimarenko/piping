use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::scope::ConnConfScope;

/// Binds a [`super::record::ConnConfRecord`] to a scope layer with a precedence
/// weight. Maps to the `connconf_assignment` table.
///
/// `scope_id` is `None` only for `scope_type = Global`. The effective sort key
/// in the resolver is `(scope_type.specificity(), weight, record_id)` ascending.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnConfAssignment {
    pub id: Uuid,
    pub record_id: Uuid,
    pub scope_type: ConnConfScope,
    pub scope_id: Option<Uuid>,
    pub weight: i32,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NewConnConfAssignment {
    pub record_id: Uuid,
    pub scope_type: ConnConfScope,
    pub scope_id: Option<Uuid>,
    pub weight: Option<i32>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct UpdateConnConfAssignment {
    pub weight: Option<i32>,
    pub enabled: Option<bool>,
    /// When `Some`, retarget the assignment's scope layer. Left unchanged when
    /// `None`. `scope_type`/`scope_id` must be supplied together and satisfy the
    /// `global ⟺ scope_id IS NULL` invariant (validated by the caller).
    pub scope_type: Option<ConnConfScope>,
    pub scope_id: Option<Uuid>,
}
