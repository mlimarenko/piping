use std::sync::Arc;

use crate::contexts::access_control::domain::{Group, Role, User};
use crate::contexts::access_control::infrastructure::persistence::{
    GroupRepository, NamedListFilter, RoleRepository, UserListFilter, UserRepository,
};
use crate::contexts::shared::pagination::PageResult;
use crate::errors::AppError;

#[derive(Clone)]
pub struct AccessControlQueryService<R, U, G>
where
    R: RoleRepository,
    U: UserRepository,
    G: GroupRepository,
{
    roles: Arc<R>,
    users: Arc<U>,
    groups: Arc<G>,
}

impl<R, U, G> AccessControlQueryService<R, U, G>
where
    R: RoleRepository,
    U: UserRepository,
    G: GroupRepository,
{
    pub fn new(roles: Arc<R>, users: Arc<U>, groups: Arc<G>) -> Self {
        Self {
            roles,
            users,
            groups,
        }
    }

    pub async fn list_roles_page(
        &self,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<Role>, AppError> {
        self.roles
            .list_roles_page(NamedListFilter::default(), limit, offset)
            .await
    }

    pub async fn list_users_page(
        &self,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<User>, AppError> {
        self.users
            .list_users_page(UserListFilter::default(), limit, offset)
            .await
    }

    pub async fn list_groups_page(
        &self,
        limit: u32,
        offset: u32,
    ) -> Result<PageResult<Group>, AppError> {
        self.groups
            .list_groups_page(NamedListFilter::default(), limit, offset)
            .await
    }
}
