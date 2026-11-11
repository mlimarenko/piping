use std::sync::Arc;

use crate::app::AppState;
use crate::contexts::audit::application::AuditService;
use crate::contexts::audit::infrastructure::persistence::SqlxAuditRepository;
use crate::contexts::connections::application::ConnectionService;
use crate::contexts::connections::infrastructure::persistence::SqlxConnectionRepository;
use crate::contexts::inventory::application::MaterializationService;
use crate::contexts::inventory::infrastructure::persistence::SqlxConfigContextRepository;
use crate::contexts::secrets::infrastructure::persistence::{DbSecretStore, SqlxSecretRepository};
use crate::errors::AppError;
use crate::integrations::proxy::HttpProxyClient;

pub(crate) type ConnectionServiceType = ConnectionService<
    SqlxConnectionRepository,
    HttpProxyClient,
    AuditService<SqlxAuditRepository>,
    MaterializationService<SqlxConfigContextRepository>,
    SqlxSecretRepository,
    DbSecretStore,
>;

pub fn build_connection_service(state: &AppState) -> Result<Arc<ConnectionServiceType>, AppError> {
    Ok(state.services.connection.clone())
}
