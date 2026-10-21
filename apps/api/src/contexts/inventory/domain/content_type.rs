//! Single source of truth for the polymorphic generic-FK content-type
//! vocabulary.
//!
//! Every place that resolves a `(content_type_id, object_id)` pair into a human
//! label, or that validates a NetBox-style `app_label.model` token on write,
//! reads from this registry. Native custom-field reads use the same registry.
//! There is exactly one declaration of the
//! `model → table` (and `parent` join for a qualified label) mapping; the
//! resolver, the write-time lookups, the seed migration, and the per-entity
//! allowed-target policies all derive from it.
//!
//! Table / column names below are the ONLY source of dynamic SQL identifiers in
//! [`crate::contexts::inventory::infrastructure::persistence::repositories::object_ref_resolver`].
//! They are `&'static str` constants — never user input — so interpolating them
//! into a query is closed-world and injection-free; all UUIDs stay bound.

/// A join from the target table to a parent table, used to build a qualified
/// label (`"<name> @ <parent name>"`, e.g. `"eth0 @ host01"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParentJoin {
    /// FK column on the target table pointing at the parent's `id`.
    pub fk: &'static str,
    /// Parent table whose `name` qualifies the label.
    pub table: &'static str,
}

/// One registered content type: the canonical `app_label.model` token plus the
/// table it materializes from and the optional parent join for a qualified
/// label. Nameless IPAM objects declare their address/prefix label via
/// [`ContentTypeDef::label_column`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentTypeDef {
    /// NetBox app label, e.g. `"dcim"`.
    pub app_label: &'static str,
    /// NetBox model, e.g. `"interface"`.
    pub model: &'static str,
    /// Backing table, e.g. `"dcim_interface"`.
    pub table: &'static str,
    /// Optional parent join for a qualified label.
    pub parent: Option<ParentJoin>,
}

/// Explicit metadata capabilities for native models whose feature contract is pinned.
#[derive(Debug, Clone, Copy, serde::Serialize, utoipa::ToSchema)]
pub struct ModelCapabilities {
    pub tags: bool,
    pub custom_fields: bool,
    pub change_dates: bool,
}

impl ContentTypeDef {
    /// Unknown feature contracts are not advertised as implemented capabilities.
    #[must_use]
    pub fn capabilities(&self) -> Option<ModelCapabilities> {
        self.is_admin_model().then_some(ModelCapabilities {
            tags: false,
            custom_fields: false,
            change_dates: false,
        })
    }
    /// AdminModel does not inherit inventory extras or change-date fields.
    #[must_use]
    pub fn is_admin_model(&self) -> bool {
        self.app_label == "users" && matches!(self.model, "owner" | "ownergroup")
    }
    /// Native labels for nameless IPAM objects; other registered models use name.
    #[must_use]
    pub fn label_column(&self) -> &'static str {
        match (self.app_label, self.model) {
            ("ipam", "ipaddress") => "address",
            ("ipam", "prefix") => "prefix",
            _ => "name",
        }
    }
    /// The canonical `app_label.model` token.
    #[must_use]
    pub fn token(&self) -> String {
        format!("{}.{}", self.app_label, self.model)
    }
}

/// Build the canonical `app_label.model` token from two segments.
#[must_use]
pub fn token(app_label: &str, model: &str) -> String {
    format!("{app_label}.{model}")
}

/// The full registry. This is the only place `model → table` mappings live.
pub const CONTENT_TYPES: &[ContentTypeDef] = &[
    ContentTypeDef {
        app_label: "users",
        model: "owner",
        table: "users_owner",
        parent: None,
    },
    ContentTypeDef {
        app_label: "users",
        model: "ownergroup",
        table: "users_ownergroup",
        parent: None,
    },
    // dcim — top-level, no parent.
    ContentTypeDef {
        app_label: "dcim",
        model: "device",
        table: "dcim_device",
        parent: None,
    },
    ContentTypeDef {
        app_label: "dcim",
        model: "site",
        table: "dcim_site",
        parent: None,
    },
    ContentTypeDef {
        app_label: "dcim",
        model: "region",
        table: "dcim_region",
        parent: None,
    },
    ContentTypeDef {
        app_label: "dcim",
        model: "sitegroup",
        table: "dcim_sitegroup",
        parent: None,
    },
    // dcim — parented by site.
    ContentTypeDef {
        app_label: "dcim",
        model: "location",
        table: "dcim_location",
        parent: Some(ParentJoin {
            fk: "site_id",
            table: "dcim_site",
        }),
    },
    ContentTypeDef {
        app_label: "dcim",
        model: "rack",
        table: "dcim_rack",
        parent: Some(ParentJoin {
            fk: "site_id",
            table: "dcim_site",
        }),
    },
    // dcim — parented by device.
    ContentTypeDef {
        app_label: "dcim",
        model: "interface",
        table: "dcim_interface",
        parent: Some(ParentJoin {
            fk: "device_id",
            table: "dcim_device",
        }),
    },
    ContentTypeDef {
        app_label: "dcim",
        model: "consoleport",
        table: "dcim_consoleport",
        parent: Some(ParentJoin {
            fk: "device_id",
            table: "dcim_device",
        }),
    },
    ContentTypeDef {
        app_label: "dcim",
        model: "consoleserverport",
        table: "dcim_consoleserverport",
        parent: Some(ParentJoin {
            fk: "device_id",
            table: "dcim_device",
        }),
    },
    ContentTypeDef {
        app_label: "dcim",
        model: "powerport",
        table: "dcim_powerport",
        parent: Some(ParentJoin {
            fk: "device_id",
            table: "dcim_device",
        }),
    },
    ContentTypeDef {
        app_label: "dcim",
        model: "poweroutlet",
        table: "dcim_poweroutlet",
        parent: Some(ParentJoin {
            fk: "device_id",
            table: "dcim_device",
        }),
    },
    ContentTypeDef {
        app_label: "dcim",
        model: "frontport",
        table: "dcim_frontport",
        parent: Some(ParentJoin {
            fk: "device_id",
            table: "dcim_device",
        }),
    },
    ContentTypeDef {
        app_label: "dcim",
        model: "rearport",
        table: "dcim_rearport",
        parent: Some(ParentJoin {
            fk: "device_id",
            table: "dcim_device",
        }),
    },
    ContentTypeDef {
        app_label: "dcim",
        model: "devicebay",
        table: "dcim_devicebay",
        parent: Some(ParentJoin {
            fk: "device_id",
            table: "dcim_device",
        }),
    },
    ContentTypeDef {
        app_label: "dcim",
        model: "modulebay",
        table: "dcim_modulebay",
        parent: Some(ParentJoin {
            fk: "device_id",
            table: "dcim_device",
        }),
    },
    ContentTypeDef {
        app_label: "dcim",
        model: "inventoryitem",
        table: "dcim_inventoryitem",
        parent: Some(ParentJoin {
            fk: "device_id",
            table: "dcim_device",
        }),
    },
    // virtualization.
    ContentTypeDef {
        app_label: "virtualization",
        model: "virtualmachine",
        table: "virtualization_virtualmachine",
        parent: None,
    },
    ContentTypeDef {
        app_label: "virtualization",
        model: "vminterface",
        table: "virtualization_vminterface",
        parent: Some(ParentJoin {
            fk: "virtual_machine_id",
            table: "virtualization_virtualmachine",
        }),
    },
    ContentTypeDef {
        app_label: "virtualization",
        model: "cluster",
        table: "virtualization_cluster",
        parent: None,
    },
    ContentTypeDef {
        app_label: "virtualization",
        model: "clustergroup",
        table: "virtualization_clustergroup",
        parent: None,
    },
    ContentTypeDef {
        app_label: "virtualization",
        model: "virtualdisk",
        table: "virtualization_virtualdisk",
        parent: Some(ParentJoin {
            fk: "virtual_machine_id",
            table: "virtualization_virtualmachine",
        }),
    },
    ContentTypeDef {
        app_label: "ipam",
        model: "ipaddress",
        table: "ipam_ipaddress",
        parent: None,
    },
    ContentTypeDef {
        app_label: "ipam",
        model: "prefix",
        table: "ipam_prefix",
        parent: None,
    },
    ContentTypeDef {
        app_label: "ipam",
        model: "service",
        table: "ipam_service",
        parent: None,
    },
    ContentTypeDef {
        app_label: "virtualization",
        model: "virtualmachinetype",
        table: "virtualization_virtualmachinetype",
        parent: None,
    },
    // tenancy.
    ContentTypeDef {
        app_label: "tenancy",
        model: "tenant",
        table: "tenancy_tenant",
        parent: None,
    },
];

/// Look up a registry def by its canonical `app_label.model` token.
#[must_use]
pub fn by_token(token: &str) -> Option<&'static ContentTypeDef> {
    CONTENT_TYPES.iter().find(|def| {
        def.app_label.len() + 1 + def.model.len() == token.len() && {
            let mut parts = token.splitn(2, '.');
            parts.next() == Some(def.app_label) && parts.next() == Some(def.model)
        }
    })
}

/// Look up a registry def by split `(app_label, model)` segments.
#[must_use]
pub fn by_app_model(app_label: &str, model: &str) -> Option<&'static ContentTypeDef> {
    CONTENT_TYPES
        .iter()
        .find(|def| def.app_label == app_label && def.model == model)
}

// ---------------------------------------------------------------------------
// Per-entity allowed-target subsets (per-field policy, not schema).
//
// Validation on write is two gates: (a) token ∈ registry AND (b) token ∈ the
// entity's allowed subset. "Registered" never implies "assignable here".
// ---------------------------------------------------------------------------

/// `dcim_macaddress.assigned_object` allowed targets.
pub const MAC_ASSIGNED_OBJECT_TYPES: &[&str] = &["dcim.interface", "virtualization.vminterface"];

/// `vpn_tunneltermination.termination` allowed targets.
pub const TUNNEL_TERMINATION_OBJECT_TYPES: &[&str] =
    &["dcim.interface", "virtualization.vminterface"];

/// `ipam_service.parent_object` allowed targets.
pub const SERVICE_PARENT_OBJECT_TYPES: &[&str] = &["dcim.device", "virtualization.virtualmachine"];

/// `ipam_vlangroup.scope` allowed targets.
pub const VLAN_GROUP_SCOPE_OBJECT_TYPES: &[&str] = &[
    "dcim.region",
    "dcim.sitegroup",
    "dcim.site",
    "dcim.location",
    "dcim.rack",
    "virtualization.cluster",
    "virtualization.clustergroup",
];

/// `dcim_inventoryitem.component` allowed targets.
pub const INVENTORY_ITEM_COMPONENT_OBJECT_TYPES: &[&str] = &[
    "dcim.consoleport",
    "dcim.consoleserverport",
    "dcim.powerport",
    "dcim.poweroutlet",
    "dcim.frontport",
    "dcim.rearport",
    "dcim.interface",
];

/// `tenancy_contactassignment.object` allowed targets.
///
/// This MUST stay byte-identical to the literal `CONTACTABLE_OBJECT_TYPES` const
/// in `api/rest/routes/inventory/tenancy/contact_assignments.rs` (which the compliance
/// test `compliance_contact_assignment_vocabulary.rs` text-parses and
/// mirror-locks against the frontend). A unit test below asserts equality.
pub const CONTACT_ASSIGNMENT_OBJECT_TYPES: &[&str] = &[
    "dcim.site",
    "dcim.region",
    "dcim.device",
    "tenancy.tenant",
    "virtualization.cluster",
    "virtualization.virtualmachine",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn by_token_round_trips_every_def() {
        for def in CONTENT_TYPES {
            let token = def.token();
            let found = by_token(&token).expect("registered token must resolve");
            assert_eq!(found, def, "by_token({token}) returned a different def");
        }
    }

    #[test]
    fn by_token_rejects_unknown_and_malformed() {
        assert!(by_token("dcim.sites").is_none(), "pluralised token");
        assert!(by_token("dcim.interface.extra").is_none(), "three segments");
        assert!(by_token("interface").is_none(), "missing app_label");
        assert!(by_token("").is_none(), "empty");
        assert!(by_token("dcim.").is_none(), "empty model");
    }

    #[test]
    fn every_def_has_non_empty_table_and_segments() {
        for def in CONTENT_TYPES {
            assert!(!def.app_label.is_empty(), "empty app_label");
            assert!(!def.model.is_empty(), "empty model");
            assert!(!def.table.is_empty(), "empty table for {}", def.token());
            assert!(
                def.table.starts_with(def.app_label),
                "table {} should start with app_label {}",
                def.table,
                def.app_label
            );
            if let Some(parent) = def.parent {
                assert!(!parent.fk.is_empty(), "empty parent fk for {}", def.token());
                assert!(
                    !parent.table.is_empty(),
                    "empty parent table for {}",
                    def.token()
                );
            }
        }
    }

    #[test]
    fn tokens_are_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for def in CONTENT_TYPES {
            assert!(seen.insert(def.token()), "duplicate token {}", def.token());
        }
    }

    /// Every per-entity subset must reference only registered tokens.
    #[test]
    fn every_subset_token_is_registered() {
        let subsets: &[(&str, &[&str])] = &[
            ("mac.assigned_object", MAC_ASSIGNED_OBJECT_TYPES),
            ("tunnel.termination", TUNNEL_TERMINATION_OBJECT_TYPES),
            ("service.parent_object", SERVICE_PARENT_OBJECT_TYPES),
            ("vlan_group.scope", VLAN_GROUP_SCOPE_OBJECT_TYPES),
            (
                "inventory_item.component",
                INVENTORY_ITEM_COMPONENT_OBJECT_TYPES,
            ),
            ("contact_assignment.object", CONTACT_ASSIGNMENT_OBJECT_TYPES),
        ];
        for (label, subset) in subsets {
            for token in *subset {
                assert!(
                    by_token(token).is_some(),
                    "{label}: token {token:?} is not in the registry"
                );
            }
        }
    }
}
