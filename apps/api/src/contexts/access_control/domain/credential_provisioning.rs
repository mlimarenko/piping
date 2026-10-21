use crate::errors::AppError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialProvisioningMode {
    Manual,
    Generate,
}

#[derive(Debug, Clone)]
pub struct CredentialProvisioningCommand {
    pub mode: CredentialProvisioningMode,
    pub password: Option<String>,
}

impl CredentialProvisioningCommand {
    pub fn new(
        mode: CredentialProvisioningMode,
        password: Option<String>,
    ) -> Result<Self, AppError> {
        match mode {
            CredentialProvisioningMode::Manual => {
                let password = password
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| {
                        AppError::Validation(
                            "Password is required when credential mode is manual".to_string(),
                        )
                    })?;
                Ok(Self {
                    mode,
                    password: Some(password),
                })
            }
            CredentialProvisioningMode::Generate => {
                if password.is_some() {
                    return Err(AppError::Validation(
                        "Password must be omitted when credential mode is generate".to_string(),
                    ));
                }
                Ok(Self {
                    mode,
                    password: None,
                })
            }
        }
    }
}
