//! Optional REST descriptions must be writable against the native NOT NULL schema.

mod common;

use piping_backend::api::rest::routes::inventory::tenancy::{
    contact_groups::{ContactGroupCreatePayload, ContactGroupUpdatePayload},
    contact_roles::ContactRoleCreatePayload,
    contacts::ContactCreatePayload,
};
use piping_backend::contexts::inventory::infrastructure::persistence::repositories::tenancy::{
    contact_group_repository::{
        ContactGroupRepository, NewContactGroup, SqlxContactGroupRepository, UpdateContactGroup,
    },
    contact_repository::{ContactRepository, NewContact, SqlxContactRepository, UpdateContact},
    contact_role_repository::{
        ContactRoleRepository, NewContactRole, SqlxContactRoleRepository, UpdateContactRole,
    },
};
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires isolated PostgreSQL with native migrations"]
async fn optional_descriptions_preserve_nested_contact_creation_and_updates()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let pool = common::build_test_db_pool().await?;
    piping_backend::storage::database::migrations::run_migrations(&pool).await?;
    let groups = SqlxContactGroupRepository::new(pool.clone());
    let roles = SqlxContactRoleRepository::new(pool.clone());
    let contacts = SqlxContactRepository::new(pool.clone());
    let mut parent_id = None;
    let mut group_ids = Vec::new();

    for description in [None, Some(json!(null)), Some(json!(""))] {
        let id = Uuid::now_v7();
        let name = format!("contact-description-{id}");
        let mut body = json!({"name": name, "slug": name, "parent_id": parent_id});
        if let Some(description) = description {
            body["description"] = description;
        }
        // Deserialize the real REST DTO, then pass exactly its fields as the handlers do.
        let payload: ContactGroupCreatePayload = serde_json::from_value(body.clone())?;
        let input = NewContactGroup {
            id,
            name: payload.name,
            slug: payload.slug,
            parent_id: payload.parent_id.map(|id| id.parse()).transpose()?,
            description: payload.description,
        };
        let created = groups.create(&input).await?;
        assert_eq!(created.description.as_deref(), Some(""));
        // Cover both repository entrypoints with every nullable input variant.
        let bulk_id = Uuid::now_v7();
        let outcome = groups
            .bulk_create(&[NewContactGroup {
                id: bulk_id,
                name: format!("bulk-{name}"),
                slug: format!("bulk-{name}"),
                parent_id: Some(id),
                ..input
            }])
            .await?;
        assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
        assert_eq!(outcome.created.len(), 1);
        assert_eq!(outcome.created[0].description.as_deref(), Some(""));
        let bulk_stored = groups
            .get_by_id(bulk_id)
            .await?
            .ok_or("bulk group not persisted")?;
        assert_eq!(bulk_stored.description.as_deref(), Some(""));
        assert_eq!(bulk_stored.parent_id, Some(id));
        groups.delete(bulk_id).await?;
        let stored = groups.get_by_id(id).await?.ok_or("group not persisted")?;
        assert_eq!(stored.description.as_deref(), Some(""));
        assert_eq!(stored.parent_id, parent_id);

        let role_payload: ContactRoleCreatePayload = serde_json::from_value(body.clone())?;
        let role_id = Uuid::now_v7();
        roles
            .create(&NewContactRole {
                id: role_id,
                name: role_payload.name,
                slug: role_payload.slug,
                description: role_payload.description,
            })
            .await?;
        assert_eq!(
            roles
                .get_by_id(role_id)
                .await?
                .ok_or("role not persisted")?
                .description
                .as_deref(),
            Some("")
        );

        body["group_id"] = json!(id);
        let payload: ContactCreatePayload = serde_json::from_value(body)?;
        let contact_id = Uuid::now_v7();
        contacts
            .create(&NewContact {
                id: contact_id,
                name: payload.name,
                group_id: payload.group_id.map(|id| id.parse()).transpose()?,
                title: payload.title,
                phone: payload.phone,
                email: payload.email,
                address: payload.address,
                link: payload.link,
                description: payload.description,
            })
            .await?;
        let stored = contacts
            .get_by_id(contact_id)
            .await?
            .ok_or("contact not persisted")?;
        assert_eq!(stored.description.as_deref(), Some(""));
        assert_eq!(stored.group_id, Some(id));

        // An omitted/null update is a no-op; an explicit empty string clears text.
        for description in [Some("retained"), None, Some("")] {
            let updated = contacts
                .update(
                    contact_id,
                    &UpdateContact {
                        name: None,
                        group_id: None,
                        title: None,
                        phone: None,
                        email: None,
                        address: None,
                        link: None,
                        description: description.map(str::to_owned),
                    },
                )
                .await?
                .ok_or("contact update lost row")?;
            assert_eq!(
                updated.description.as_deref(),
                Some(description.unwrap_or("retained"))
            );
            assert_eq!(updated.group_id, Some(id));
            let updated = roles
                .update(
                    role_id,
                    &UpdateContactRole {
                        name: None,
                        slug: None,
                        description: description.map(str::to_owned),
                    },
                )
                .await?
                .ok_or("role update lost row")?;
            assert_eq!(
                updated.description.as_deref(),
                Some(description.unwrap_or("retained"))
            );
        }
        for (body, expected) in [
            (json!({"description": "retained"}), "retained"),
            (json!({}), "retained"),
            (json!({"description": null}), "retained"),
            (json!({"description": ""}), ""),
        ] {
            let payload: ContactGroupUpdatePayload = serde_json::from_value(body)?;
            groups
                .update(
                    id,
                    &UpdateContactGroup {
                        name: payload.name,
                        slug: payload.slug,
                        parent_id: None,
                        description: payload.description,
                    },
                )
                .await?;
            let stored = groups.get_by_id(id).await?.ok_or("group update lost row")?;
            assert_eq!(stored.description.as_deref(), Some(expected));
            assert_eq!(stored.parent_id, parent_id);
        }
        // The delete guard requires removing this fixture-owned association first.
        sqlx::query("DELETE FROM tenancy_contact_groups WHERE contact_id = $1")
            .bind(contact_id)
            .execute(&pool)
            .await?;
        contacts.delete(contact_id).await?;
        roles.delete(role_id).await?;
        group_ids.push(id);
        parent_id = Some(id);
    }
    for id in group_ids.into_iter().rev() {
        groups.delete(id).await?;
    }
    Ok(())
}
