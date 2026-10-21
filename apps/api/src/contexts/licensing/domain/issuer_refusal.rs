/// Refusals emitted by the Cloud instances API, not the broader reason registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum IssuerRefusal {
    #[error("TOKEN_REVOKED")]
    TokenRevoked,
    #[error("TOKEN_NOT_FOUND")]
    TokenNotFound,
    #[error("ENTITLEMENT_BINDING_INVALID")]
    EntitlementBindingInvalid,
    #[error("LICENSE_INSTANCE_CONFLICT")]
    LicenseInstanceConflict,
    #[error("ORGANIZATION_ARCHIVED")]
    OrganizationArchived,
    #[error("TOKEN_ALREADY_CONSUMED")]
    TokenAlreadyConsumed,
}

#[cfg(test)]
mod tests {
    use super::IssuerRefusal;

    #[test]
    fn only_emitted_status_envelope_reason_operation_tuples_are_authoritative() {
        for (status, code, reason, activation, expected) in [
            (
                403,
                "TOKEN_REVOKED",
                "TOKEN_REVOKED",
                false,
                IssuerRefusal::TokenRevoked,
            ),
            (
                403,
                "TOKEN_REVOKED",
                "TOKEN_REVOKED",
                true,
                IssuerRefusal::TokenRevoked,
            ),
            (
                404,
                "NOT_FOUND",
                "TOKEN_NOT_FOUND",
                false,
                IssuerRefusal::TokenNotFound,
            ),
            (
                404,
                "NOT_FOUND",
                "TOKEN_NOT_FOUND",
                true,
                IssuerRefusal::TokenNotFound,
            ),
            (
                409,
                "CONFLICT",
                "ENTITLEMENT_BINDING_INVALID",
                false,
                IssuerRefusal::EntitlementBindingInvalid,
            ),
            (
                409,
                "CONFLICT",
                "LICENSE_INSTANCE_CONFLICT",
                false,
                IssuerRefusal::LicenseInstanceConflict,
            ),
            (
                403,
                "ORGANIZATION_ARCHIVED",
                "ORGANIZATION_ARCHIVED",
                false,
                IssuerRefusal::OrganizationArchived,
            ),
            (
                403,
                "ORGANIZATION_ARCHIVED",
                "ORGANIZATION_ARCHIVED",
                true,
                IssuerRefusal::OrganizationArchived,
            ),
            (
                409,
                "CONFLICT",
                "TOKEN_ALREADY_CONSUMED",
                true,
                IssuerRefusal::TokenAlreadyConsumed,
            ),
        ] {
            assert_eq!(
                IssuerRefusal::from_response(status, code, Some(reason), activation),
                Some(expected)
            );
            assert_eq!(expected.code(), reason);
            assert_eq!(expected.http_status(), status);
            for wrong_status in [200, 400, 403, 404, 409, 422, 429, 500, 503] {
                if wrong_status != status {
                    assert_eq!(
                        IssuerRefusal::from_response(wrong_status, code, Some(reason), activation),
                        None
                    );
                }
            }
            for wrong_code in [
                "UNRECOGNIZED",
                if code == "CONFLICT" {
                    "NOT_FOUND"
                } else {
                    "CONFLICT"
                },
            ] {
                assert_eq!(
                    IssuerRefusal::from_response(status, wrong_code, Some(reason), activation),
                    None
                );
            }
            assert_eq!(
                IssuerRefusal::from_response(status, code, None, activation),
                None
            );
            assert_eq!(
                IssuerRefusal::from_response(status, code, Some("UNRECOGNIZED"), activation),
                None
            );
            if code != reason {
                assert_eq!(
                    IssuerRefusal::from_response(status, reason, Some(reason), activation),
                    None
                );
            }
        }
        for (status, code, reason, activation) in [
            (404, "NOT_FOUND", "NOT_FOUND", false),
            (409, "CONFLICT", "CONFLICT", false),
            (409, "CONFLICT", "TOKEN_ALREADY_CONSUMED", false),
            (409, "CONFLICT", "ENTITLEMENT_BINDING_INVALID", true),
            (409, "CONFLICT", "LICENSE_INSTANCE_CONFLICT", true),
            (403, "SUBSCRIPTION_CANCELED", "SUBSCRIPTION_CANCELED", false),
        ] {
            assert_eq!(
                IssuerRefusal::from_response(status, code, Some(reason), activation),
                None
            );
        }
    }
}

impl IssuerRefusal {
    pub fn from_response(
        status: u16,
        code: &str,
        reason: Option<&str>,
        activation: bool,
    ) -> Option<Self> {
        // Cloud uses generic transport codes for 404/409, with the domain reason
        // in reasonCode. Only emitted tuples authorize refusal, not router errors
        // or transient bodies containing a recognizable word.
        match (status, code, reason, activation) {
            (403, "TOKEN_REVOKED", Some("TOKEN_REVOKED"), _) => Some(Self::TokenRevoked),
            (404, "NOT_FOUND", Some("TOKEN_NOT_FOUND"), _) => Some(Self::TokenNotFound),
            (409, "CONFLICT", Some("ENTITLEMENT_BINDING_INVALID"), false) => {
                Some(Self::EntitlementBindingInvalid)
            }
            (409, "CONFLICT", Some("LICENSE_INSTANCE_CONFLICT"), false) => {
                Some(Self::LicenseInstanceConflict)
            }
            (403, "ORGANIZATION_ARCHIVED", Some("ORGANIZATION_ARCHIVED"), _) => {
                Some(Self::OrganizationArchived)
            }
            (409, "CONFLICT", Some("TOKEN_ALREADY_CONSUMED"), true) => {
                Some(Self::TokenAlreadyConsumed)
            }
            _ => None,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::TokenRevoked => "TOKEN_REVOKED",
            Self::TokenNotFound => "TOKEN_NOT_FOUND",
            Self::EntitlementBindingInvalid => "ENTITLEMENT_BINDING_INVALID",
            Self::LicenseInstanceConflict => "LICENSE_INSTANCE_CONFLICT",
            Self::OrganizationArchived => "ORGANIZATION_ARCHIVED",
            Self::TokenAlreadyConsumed => "TOKEN_ALREADY_CONSUMED",
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Self::TokenRevoked => "The activation token has been revoked.",
            Self::TokenNotFound => {
                "The licensing service no longer recognizes the activation token."
            }
            Self::EntitlementBindingInvalid => "The entitlement does not match this instance.",
            Self::LicenseInstanceConflict => "The license is assigned to another instance.",
            Self::OrganizationArchived => "The license organization has been archived.",
            Self::TokenAlreadyConsumed => "The activation token has already been consumed.",
        }
    }

    pub fn http_status(self) -> u16 {
        match self {
            Self::TokenRevoked | Self::OrganizationArchived => 403,
            Self::TokenNotFound => 404,
            _ => 409,
        }
    }
}
