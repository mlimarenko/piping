use std::sync::Arc;

use uuid::Uuid;

use crate::contexts::licensing::application::service::LicensingService;
use crate::errors::AppError;

#[derive(Clone)]
pub struct LicenseLoginGateService {
    licensing: Arc<LicensingService>,
}

impl LicenseLoginGateService {
    pub fn new(licensing: Arc<LicensingService>) -> Self {
        Self { licensing }
    }

    pub async fn enforce_login(&self, user_id: Uuid) -> Result<(), AppError> {
        self.licensing.enforce_seat_on_login(user_id).await
    }

    pub async fn confirm_login(&self, user_id: Uuid, session_id: Uuid) -> Result<(), AppError> {
        self.licensing
            .confirm_seat_on_login(user_id, session_id)
            .await
    }
}
