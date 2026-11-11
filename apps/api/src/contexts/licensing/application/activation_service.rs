use std::sync::Arc;

use crate::contexts::licensing::application::service::{LicenseActivateResult, LicensingService};
use crate::errors::AppError;

#[derive(Clone)]
pub struct LicenseActivationService {
    licensing: Arc<LicensingService>,
}

impl LicenseActivationService {
    pub fn new(licensing: Arc<LicensingService>) -> Self {
        Self { licensing }
    }

    pub async fn activate(
        &self,
        activation_token: &str,
    ) -> Result<LicenseActivateResult, AppError> {
        self.licensing.activate_license(activation_token).await
    }
}
